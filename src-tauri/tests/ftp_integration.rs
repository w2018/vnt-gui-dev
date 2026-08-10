//! V3 运行时行为验证：启动真实 FTP server → suppaftp 客户端连接
//! → 登录 / 上传 / 下载 / 删除 / 权限拦截 / 错误密码 → 停止 server
//!
//! 运行：cargo test --test ftp_integration -- --nocapture

use std::path::PathBuf;
use std::sync::Arc;

use parking_lot::RwLock;
use suppaftp::tokio::{AsyncNoTlsStream, ImplAsyncFtpStream};
use tokio::io::AsyncReadExt;

/// 测试用 FTP 客户端类型（无 TLS）
type FtpClient = ImplAsyncFtpStream<AsyncNoTlsStream>;

use vnt_gui_lib::ftp::auth::{FtpAuthenticator, FtpUserDetailProvider, UserStore};
use vnt_gui_lib::ftp::config::{FtpConfig, FtpPermissions, FtpUser};
use vnt_gui_lib::ftp::storage::FtpStorage;

fn build_config(root: &std::path::Path, users: Vec<FtpUser>) -> FtpConfig {
    FtpConfig {
        enabled: true,
        auto_start_with_app: false,
        auto_start_with_system: false,
        root_dir: root.to_string_lossy().to_string(),
        port: 0, // 由调用方替换为随机端口
        so_reuseaddr: true,
        pasv_ports: None,
        users,
    }
}

fn admin_user() -> FtpUser {
    // 直接注入 argon2 哈希（绕过 keyring，测试环境无凭据库）
    let hash = vnt_gui_lib::ftp::auth::hash_password("admin123").unwrap();
    FtpUser {
        username: "admin".into(),
        password: hash,
        permissions: FtpPermissions {
            upload: true,
            download: true,
            delete: true,
            readonly: false,
        },
    }
}

fn readonly_user() -> FtpUser {
    let hash = vnt_gui_lib::ftp::auth::hash_password("read123").unwrap();
    FtpUser {
        username: "reader".into(),
        password: hash,
        permissions: FtpPermissions {
            upload: false,
            download: true,
            delete: false,
            readonly: true,
        },
    }
}

fn nodelete_user() -> FtpUser {
    let hash = vnt_gui_lib::ftp::auth::hash_password("dl123").unwrap();
    FtpUser {
        username: "nodelete".into(),
        password: hash,
        permissions: FtpPermissions {
            upload: true,
            download: true,
            delete: false,
            readonly: false,
        },
    }
}

/// 在随机端口启动真实 FTP server（返回端口 + 停止句柄）
async fn start_server(root: &std::path::Path, users: Vec<FtpUser>) -> (u16, tokio::task::JoinHandle<()>) {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);

    let mut cfg = build_config(root, users);
    cfg.port = port;

    let store = Arc::new(RwLock::new(UserStore::from_config(&cfg)));
    let auth = Arc::new(FtpAuthenticator::new(store.clone()));
    let provider = Arc::new(FtpUserDetailProvider::new(store.clone()));

    let root_for_storage = root.to_path_buf();
    let server = libunftp::ServerBuilder::with_user_detail_provider(
        Box::new(move || FtpStorage::new(root_for_storage.clone())),
        provider,
    )
    .authenticator(auth)
    .greeting("VNT GUI FTP test")
    .build()
    .unwrap();

    let bind = format!("127.0.0.1:{}", port);
    let handle = tokio::spawn(async move {
        match server.listen(bind).await {
            Ok(()) => println!("[test] server stopped gracefully"),
            Err(e) => println!("[test] SERVER LISTEN ERROR: {}", e),
        }
    });
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    (port, handle)
}

#[tokio::test]
async fn test_ftp_full_flow() {
    let _ = env_logger::builder().is_test(true).try_init();
    let root = tempfile::tempdir().unwrap();
    let users = vec![admin_user(), readonly_user(), nodelete_user()];
    let (port, handle) = start_server(root.path(), users).await;

    // ---------- 1. admin：登录 + 上传 + 下载 + 删除 ----------
    let mut ftp = match FtpClient::connect(format!("127.0.0.1:{}", port)).await {
        Ok(f) => f,
        Err(e) => panic!("连接 FTP 失败: {:?}", e),
    };
    ftp.login("admin", "admin123").await.expect("admin 登录失败");

    // 上传
    let content = b"hello from integration test";
    let mut cursor = std::io::Cursor::new(content.to_vec());
    ftp.put_file("upload.txt", &mut cursor).await.expect("上传失败");
    assert!(root.path().join("upload.txt").exists());

    // 下载并比对
    let mut stream = ftp.retr_as_stream("upload.txt").await.expect("下载失败");
    let mut downloaded = Vec::new();
    stream.read_to_end(&mut downloaded).await.unwrap();
    assert_eq!(downloaded, content, "下载内容与上传不一致");
    // 注：libunftp 0.23 对 DELE/MKD/RMD 一律返回 226，与 suppaftp 10 严格校验(250/257)不兼容；
    // 删除成功路径由 storage 单测 test_delete_file_succeeds 覆盖，此处权限拒绝由 nodelete 段验证
    ftp.quit().await.ok();

    // ---------- 2. readonly 用户：登录成功，上传被拒 ----------
    let mut ftp = FtpClient::connect(format!("127.0.0.1:{}", port))
        .await
        .unwrap();
    ftp.login("reader", "read123").await.expect("readonly 用户登录失败");
    let mut cursor = std::io::Cursor::new(b"x".to_vec());
    let err = ftp.put_file("forbidden.txt", &mut cursor).await;
    assert!(err.is_err(), "只读用户上传必须被拒绝");
    assert!(!root.path().join("forbidden.txt").exists(), "只读用户不得创建文件");
    ftp.quit().await.ok();

    // ---------- 3. 无删除权限用户：上传成功，删除被拒 ----------
    let mut ftp = FtpClient::connect(format!("127.0.0.1:{}", port))
        .await
        .unwrap();
    ftp.login("nodelete", "dl123").await.unwrap();
    let mut cursor = std::io::Cursor::new(b"payload".to_vec());
    ftp.put_file("can_upload.txt", &mut cursor).await.expect("上传应允许");
    let err = ftp.rm("can_upload.txt").await;
    assert!(err.is_err(), "无删除权限用户删除必须被拒绝");
    // 文件仍在
    assert!(root.path().join("can_upload.txt").exists());
    ftp.quit().await.ok();

    // ---------- 4. 错误密码：登录失败 ----------
    let mut ftp = FtpClient::connect(format!("127.0.0.1:{}", port))
        .await
        .unwrap();
    let err = ftp.login("admin", "wrong-password").await;
    assert!(err.is_err(), "错误密码必须登录失败");

    handle.abort();
}

#[tokio::test]
async fn test_ftp_login_then_list() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("hello.txt"), b"hi").unwrap();
    let (port, handle) = start_server(root.path(), vec![admin_user()]).await;

    let mut ftp = FtpClient::connect(format!("127.0.0.1:{}", port))
        .await
        .unwrap();
    ftp.login("admin", "admin123").await.unwrap();
    let listing = ftp.list(None).await.expect("LIST 失败");
    let joined = listing.join("\n");
    assert!(joined.contains("hello.txt"), "LIST 应包含 hello.txt: {}", joined);
    ftp.quit().await.ok();
    handle.abort();
}
