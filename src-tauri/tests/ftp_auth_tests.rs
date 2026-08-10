//! FTP 认证链路集成测试（Bug 2 修复验证）
//!
//! 手写最小 FTP 协议客户端（TCP 原始命令），覆盖：
//! - test_full_login_flow：220 banner → USER → 331 → PASS → 230 → PWD → 257 → QUIT
//! - test_wrong_password_rejected：错误密码 → 530
//! - test_readonly_cannot_upload：只读用户 STOR → 550 拒绝
//! - test_delete_permission_enforced：无删除权限 DELE → 550 拒绝
//! - test_upload_allowed_user_can_stor：有上传权限用户 STOR → 150 → 226（权限不误伤）

use std::time::Duration;

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};
use tokio::net::TcpStream;

use vnt_gui_lib::ftp::config::{FtpConfig, FtpPermissions, FtpUser};
use vnt_gui_lib::ftp::server::{start_ftp, stop_ftp};

/// FTP 服务是全局单实例：测试必须串行执行
static FTP_TEST_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// 测试配置工厂：root 临时目录 + 固定端口（与 lib 测试的动态端口错开）
fn test_cfg(root: &std::path::Path, port: u16, users: Vec<FtpUser>) -> FtpConfig {
    FtpConfig {
        enabled: true,
        auto_start_with_app: false,
        auto_start_with_system: false,
        root_dir: root.to_string_lossy().to_string(),
        port,
        so_reuseaddr: true,
        pasv_ports: None,
        users,
    }
}

fn user(name: &str, password: &str, perms: FtpPermissions) -> FtpUser {
    FtpUser {
        username: name.into(),
        password: password.into(),
        permissions: perms,
    }
}

fn full_perms() -> FtpPermissions {
    FtpPermissions {
        upload: true,
        download: true,
        delete: true,
        readonly: false,
    }
}

fn readonly_perms() -> FtpPermissions {
    FtpPermissions {
        upload: false,
        download: true,
        delete: false,
        readonly: true,
    }
}

/// 连接并读取 220 banner；返回 (写半, 读半包装)
type FtpConn = (OwnedWriteHalf, BufReader<OwnedReadHalf>);

async fn connect(port: u16) -> FtpConn {
    let stream = TcpStream::connect(("127.0.0.1", port))
        .await
        .expect("连接 FTP 服务器失败");
    let (read_half, write_half) = stream.into_split();
    let mut reader = BufReader::new(read_half);
    let banner = read_reply(&mut reader).await;
    assert!(banner.starts_with("220"), "期望 220 banner，实际: {}", banner);
    (write_half, reader)
}

/// 发送一条命令并读取一行响应
async fn cmd(writer: &mut OwnedWriteHalf, reader: &mut BufReader<OwnedReadHalf>, line: &str) -> String {
    writer
        .write_all(format!("{}\r\n", line).as_bytes())
        .await
        .expect("发送命令失败");
    read_reply(reader).await
}

/// 读取一行响应（直到 \n）
async fn read_reply(reader: &mut BufReader<OwnedReadHalf>) -> String {
    let mut buf = Vec::new();
    tokio::time::timeout(Duration::from_secs(5), reader.read_until(b'\n', &mut buf))
        .await
        .expect("读取响应超时")
        .expect("读取响应失败");
    String::from_utf8_lossy(&buf).trim().to_string()
}

/// 从 PASV 227 响应解析数据端口："227 Entering Passive Mode (127,0,0,1,p1,p2)"
fn parse_pasv_port(resp: &str) -> u16 {
    let nums: Vec<u16> = resp
        .split(|c: char| !c.is_ascii_digit())
        .filter_map(|s| s.parse().ok())
        .collect();
    assert!(nums.len() >= 6, "PASV 响应格式异常: {}", resp);
    let p1 = nums[nums.len() - 2];
    let p2 = nums[nums.len() - 1];
    p1 * 256 + p2
}

#[tokio::test]
async fn test_full_login_flow() {
    // 完整登录链路：banner → USER → 331 → PASS → 230 → PWD → 257 → QUIT
    let _g = FTP_TEST_LOCK.lock().await;
    let root = tempfile::tempdir().unwrap();
    start_ftp(test_cfg(
        root.path(),
        21210,
        vec![user("testuser", "testpass", full_perms())],
    ))
    .await
    .expect("启动 FTP 失败");

    let (mut stream, mut reader) = connect(21210).await;
    // USER → 331
    let resp = cmd(&mut stream, &mut reader, "USER testuser").await;
    assert!(resp.starts_with("331"), "期望 331，实际: {}", resp);
    // PASS → 230
    let resp = cmd(&mut stream, &mut reader, "PASS testpass").await;
    assert!(resp.starts_with("230"), "期望 230 登录成功，实际: {}", resp);
    // PWD → 257
    let resp = cmd(&mut stream, &mut reader, "PWD").await;
    assert!(resp.starts_with("257"), "期望 257，实际: {}", resp);
    // QUIT → 221
    let resp = cmd(&mut stream, &mut reader, "QUIT").await;
    assert!(resp.starts_with("221"), "期望 221，实际: {}", resp);

    stop_ftp().await.unwrap();
}

#[tokio::test]
async fn test_wrong_password_rejected() {
    // 错误密码 → 530 拒绝
    let _g = FTP_TEST_LOCK.lock().await;
    let root = tempfile::tempdir().unwrap();
    start_ftp(test_cfg(
        root.path(),
        21211,
        vec![user("testuser", "testpass", full_perms())],
    ))
    .await
    .expect("启动 FTP 失败");

    let (mut stream, mut reader) = connect(21211).await;
    let resp = cmd(&mut stream, &mut reader, "USER testuser").await;
    assert!(resp.starts_with("331"), "期望 331，实际: {}", resp);
    let resp = cmd(&mut stream, &mut reader, "PASS wrongpass").await;
    assert!(resp.starts_with("530"), "期望 530，实际: {}", resp);
    // 错误密码后会话未认证：再次尝试错误用户也被拒
    let resp = cmd(&mut stream, &mut reader, "USER nobody").await;
    assert!(resp.starts_with("331"), "期望 331，实际: {}", resp);
    let resp = cmd(&mut stream, &mut reader, "PASS x").await;
    assert!(resp.starts_with("530"), "期望 530，实际: {}", resp);

    stop_ftp().await.unwrap();
}

#[tokio::test]
async fn test_readonly_cannot_upload() {
    // 只读用户：STOR 必须被拒（550/500）
    let _g = FTP_TEST_LOCK.lock().await;
    let root = tempfile::tempdir().unwrap();
    start_ftp(test_cfg(
        root.path(),
        21212,
        vec![user("ro", "ropass", readonly_perms())],
    ))
    .await
    .expect("启动 FTP 失败");

    let (mut stream, mut reader) = connect(21212).await;
    cmd(&mut stream, &mut reader, "USER ro").await;
    let resp = cmd(&mut stream, &mut reader, "PASS ropass").await;
    assert!(resp.starts_with("230"), "期望 230，实际: {}", resp);

    // PASV → 227 → 数据连接（libunftp STOR 先回 150，权限拒绝发生在数据通道）
    let resp = cmd(&mut stream, &mut reader, "PASV").await;
    assert!(resp.starts_with("227"), "期望 227，实际: {}", resp);
    let data_port = parse_pasv_port(&resp);
    let mut data = TcpStream::connect(("127.0.0.1", data_port))
        .await
        .expect("数据连接建立失败");

    let stor = cmd(&mut stream, &mut reader, "STOR test.txt").await;
    assert!(stor.starts_with("150"), "期望 150（libunftp 先应答），实际: {}", stor);

    // 传输数据并关闭数据连接 → 最终应答不应是 226（成功）
    data.write_all(b"should not be written").await.unwrap();
    data.shutdown().await.unwrap();
    let final_resp = read_reply(&mut reader).await;
    // 硬验证：只读用户上传被拒 → ROOT 下文件必须不存在
    assert!(
        !root.path().join("test.txt").exists(),
        "只读用户不应创建文件（最终应答: {}）",
        final_resp
    );

    stop_ftp().await.unwrap();
}

#[tokio::test]
async fn test_delete_permission_enforced() {
    // 无删除权限：DELE 必须被拒（550）
    let _g = FTP_TEST_LOCK.lock().await;
    let root = tempfile::tempdir().unwrap();
    start_ftp(test_cfg(
        root.path(),
        21213,
        vec![user(
            "nodelete",
            "ndpass",
            FtpPermissions {
                upload: true,
                download: true,
                delete: false,
                readonly: false,
            },
        )],
    ))
    .await
    .expect("启动 FTP 失败");

    let (mut stream, mut reader) = connect(21213).await;
    cmd(&mut stream, &mut reader, "USER nodelete").await;
    let resp = cmd(&mut stream, &mut reader, "PASS ndpass").await;
    assert!(resp.starts_with("230"), "期望 230，实际: {}", resp);

    let resp = cmd(&mut stream, &mut reader, "DELE somefile.txt").await;
    assert!(
        resp.starts_with("550") || resp.starts_with("500"),
        "无删除权限不应能删除，实际: {}",
        resp
    );

    stop_ftp().await.unwrap();
}

#[tokio::test]
async fn test_upload_allowed_user_can_stor() {
    // 有上传权限：STOR 完整流程 150 → 传输 → 226（权限不误伤）
    let _g = FTP_TEST_LOCK.lock().await;
    let root = tempfile::tempdir().unwrap();
    start_ftp(test_cfg(
        root.path(),
        21214,
        vec![user("uploader", "uppass", full_perms())],
    ))
    .await
    .expect("启动 FTP 失败");

    let (mut stream, mut reader) = connect(21214).await;
    cmd(&mut stream, &mut reader, "USER uploader").await;
    let resp = cmd(&mut stream, &mut reader, "PASS uppass").await;
    assert!(resp.starts_with("230"), "期望 230，实际: {}", resp);

    // PASV → 227 → 解析数据端口 → 建数据连接
    let resp = cmd(&mut stream, &mut reader, "PASV").await;
    assert!(resp.starts_with("227"), "期望 227，实际: {}", resp);
    let data_port = parse_pasv_port(&resp);
    let mut data = TcpStream::connect(("127.0.0.1", data_port))
        .await
        .expect("数据连接建立失败");

    // STOR → 150
    let resp = cmd(&mut stream, &mut reader, "STOR hello.txt").await;
    assert!(resp.starts_with("150"), "期望 150，实际: {}", resp);

    // 传输数据并关闭数据连接 → 226
    data.write_all(b"hello ftp world").await.unwrap();
    data.shutdown().await.unwrap();
    let resp = read_reply(&mut reader).await;
    assert!(resp.starts_with("226"), "期望 226，实际: {}", resp);

    // 验证文件已写入 ROOT
    let written = std::fs::read_to_string(root.path().join("hello.txt")).unwrap();
    assert_eq!(written, "hello ftp world");

    stop_ftp().await.unwrap();
}
