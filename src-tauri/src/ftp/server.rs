//! FTP Server 生命周期（需求 F1/F8）—— Bug 1 修复版
//!
//! 端口释放修复（原 bug：stop 后 listener 仍在后台占用端口）：
//! - 引入 `tokio_util::sync::CancellationToken`
//! - 通过 `ServerBuilder::shutdown_indicator`（libunftp 官方停止机制，
//!   内部 `tokio::select!` 等待该 future）触发优雅关闭；
//!   `listen()` 返回后 listener 随之释放
//! - `stop_ftp()`：`cancel.cancel()` → 等待服务任务结束（500ms 优雅关闭期 + 超时兜底）
//!
//! 说明：libunftp 0.23 控制端口由内部 `TcpListener::bind` 固定（Binder 仅作用于
//! 被动数据端口，且 libunftp 对被动端口已默认 `set_reuseaddr(true)`），因此
//! `so_reuseaddr` 配置项保留用于被动端口行为控制与未来兼容。

use std::io;
use std::net::IpAddr;
use std::ops::RangeInclusive;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use libunftp::{options::Binder, ServerBuilder};
use parking_lot::Mutex;
use tokio::net::TcpSocket;
use tokio_util::sync::CancellationToken;

use crate::ftp::auth::{FtpAuthenticator, FtpUserDetailProvider, UserStore};
use crate::ftp::config::FtpConfig;
use crate::ftp::storage::FtpStorage;

/// 服务器运行状态（F8）
#[derive(Debug, Clone, serde::Serialize, Default)]
pub struct FtpServerStatus {
    /// stopped / running / error
    pub state: String,
    /// 监听地址（如 "0.0.0.0:2121"）
    pub listen_addr: Option<String>,
    /// 最近错误
    pub error: Option<String>,
}

/// 运行中实例的句柄
struct FtpRuntime {
    /// 服务任务句柄（结束 = 服务停止 + 端口释放）
    handle: tokio::task::JoinHandle<()>,
    /// 停止信号
    cancel: CancellationToken,
    /// 监听地址（启动时绑定成功后的实际地址）
    listen_addr: Option<String>,
}

static FTP_RUNTIME: Mutex<Option<FtpRuntime>> = Mutex::new(None);
static FTP_STATUS: Mutex<FtpServerStatus> = Mutex::new(FtpServerStatus {
    state: String::new(),
    listen_addr: None,
    error: None,
});

/// 当前状态（F8）：初始空状态归一化为 "stopped"
pub fn ftp_status() -> FtpServerStatus {
    let mut s = FTP_STATUS.lock().clone();
    if s.state.is_empty() {
        s.state = "stopped".to_string();
    }
    s
}

/// 自定义被动端口 Binder：显式开启 SO_REUSEADDR（默认行为等价，语义清晰）
#[derive(Debug)]
struct ReuseBinder;

#[async_trait::async_trait]
impl Binder for ReuseBinder {
    async fn bind(&mut self, local_addr: IpAddr, passive_ports: RangeInclusive<u16>) -> io::Result<TcpSocket> {
        // 与 libunftp 默认逻辑一致：在范围内随机尝试绑定，失败重试
        let socket = match local_addr {
            IpAddr::V4(_) => TcpSocket::new_v4()?,
            IpAddr::V6(_) => TcpSocket::new_v6()?,
        };
        socket.set_reuseaddr(true)?;
        let len = passive_ports.clone().count();
        let start = *passive_ports.start();
        let mut last_err: Option<io::Error> = None;
        for _ in 0..30 {
            let offset = (std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.subsec_nanos() as u32)
                .unwrap_or(0))
                % len.max(1) as u32;
            let port = start + offset as u16;
            match socket.bind(std::net::SocketAddr::new(local_addr, port)) {
                Ok(()) => return Ok(socket),
                Err(e) => last_err = Some(e),
            }
        }
        Err(last_err.unwrap_or_else(|| io::Error::new(io::ErrorKind::AddrInUse, "passive port bind failed")))
    }
}

/// 启动 FTP 服务（F1）
///
/// 校验：root_dir 存在且为目录、至少一个用户；端口绑定失败 → 状态 error。
pub async fn start_ftp(cfg: FtpConfig) -> Result<(), String> {
    // 1. 防重入：已在运行 → 先停止
    let _ = stop_ftp().await;

    // 2. 校验 root 目录（Bug 2 要求：空/不存在 → 明确错误而非 panic）
    let root = Path::new(&cfg.root_dir);
    if cfg.root_dir.trim().is_empty() {
        let msg = "FTP 根目录未配置，请先选择 ROOT 目录".to_string();
        set_status_error(&msg);
        return Err(msg);
    }
    if !root.exists() {
        let msg = format!("根目录不存在: {}", cfg.root_dir);
        set_status_error(&msg);
        return Err(msg);
    }
    if !root.is_dir() {
        let msg = format!("根目录不是文件夹: {}", cfg.root_dir);
        set_status_error(&msg);
        return Err(msg);
    }

    // 3. 用户校验
    if cfg.users.is_empty() {
        let msg = "至少需要添加一个 FTP 用户".to_string();
        set_status_error(&msg);
        return Err(msg);
    }

    // 4. 构建共享用户表（authenticator / provider / storage 共用）
    let store = Arc::new(parking_lot::RwLock::new(UserStore::from_config(&cfg)));
    let auth = Arc::new(FtpAuthenticator::new(store.clone()));
    let provider = Arc::new(FtpUserDetailProvider::new(store.clone()));

    // 5. 构建 Server（每个会话生成新的 Storage，root 固定）
    let root_for_storage = root.to_path_buf();
    let mut builder = ServerBuilder::with_user_detail_provider(
        Box::new(move || FtpStorage::new(root_for_storage.clone())),
        provider,
    );
    builder = builder
        .authenticator(auth)
        .greeting("VNT GUI FTP Service ready");
    if let Some((lo, hi)) = cfg.pasv_ports {
        if lo > 0 && hi >= lo {
            builder = builder.passive_ports(lo..=hi);
        }
    }
    if cfg.so_reuseaddr {
        builder = builder.binder(ReuseBinder);
    }
    // 停止机制（Bug 1 核心）：CancellationToken → shutdown_indicator
    // libunftp 的 listen() 内部 select! 等待该 future，返回后 listener 释放
    let cancel = CancellationToken::new();
    let shutdown_cancel = cancel.clone();
    let shutdown_fut = async move {
        shutdown_cancel.cancelled().await;
        libunftp::options::Shutdown::default().grace_period(Duration::from_millis(500))
    };
    builder = builder.shutdown_indicator(shutdown_fut);

    let server = builder.build().map_err(|e| {
        let msg = format!("FTP 服务器构建失败: {}", e);
        set_status_error(&msg);
        msg
    })?;

    // 6. 监听 + 运行（绑定失败 → 状态 error；通过 oneshot 等待绑定结果）
    let bind = format!("0.0.0.0:{}", cfg.port);
    let bind_for_task = bind.clone();
    let (bind_tx, bind_rx) = tokio::sync::oneshot::channel::<Result<(), String>>();
    let handle = tokio::spawn(async move {
        let bind_for_msg = bind_for_task.clone();
        match server.listen(bind_for_task).await {
            Ok(()) => {
                // 正常结束（cancel 触发）：listener 已释放
                log::info!("FTP 监听结束（端口已释放）: {}", bind_for_msg);
                let mut status = FTP_STATUS.lock();
                status.state = "stopped".to_string();
                status.listen_addr = None;
                // 通知：仅当启动阶段收到（理论上 listen 不会立即 Ok）
                let _ = bind_tx.send(Ok(()));
            }
            Err(e) => {
                let msg = format!("FTP 监听失败 {}: {}", bind_for_msg, e);
                log::error!("{}", msg);
                set_status_error(&msg);
                let _ = bind_tx.send(Err(msg));
            }
        }
    });
    // 等待绑定结果：收到 Err = 绑定失败；超时（listen 运行中）或 Ok = 成功
    match tokio::time::timeout(Duration::from_secs(3), bind_rx).await {
        Ok(Ok(Err(msg))) => {
            let _ = handle.abort();
            return Err(msg);
        }
        _ => {}
    }

    *FTP_RUNTIME.lock() = Some(FtpRuntime {
        handle,
        cancel,
        listen_addr: Some(bind.clone()),
    });
    *FTP_STATUS.lock() = FtpServerStatus {
        state: "running".to_string(),
        listen_addr: Some(bind.clone()),
        error: None,
    };
    log::info!("FTP server listening on {} ({} 个用户)", bind, cfg.users.len());
    Ok(())
}

/// 停止 FTP 服务（F1）—— Bug 1 修复：真正等待 listener 关闭并释放端口
pub async fn stop_ftp() -> Result<(), String> {
    let runtime = FTP_RUNTIME.lock().take();
    if let Some(runtime) = runtime {
        runtime.cancel.cancel();
        // 等服务任务结束：优雅关闭（500ms grace）后 listener 释放；
        // 超时兜底 2s（避免极端情况卡死）
        let _ = tokio::time::timeout(Duration::from_secs(2), runtime.handle).await;
        log::info!("FTP 服务已停止（端口已释放）");
    }
    *FTP_STATUS.lock() = FtpServerStatus {
        state: "stopped".to_string(),
        listen_addr: None,
        error: None,
    };
    Ok(())
}

fn set_status_error(msg: &str) {
    let mut status = FTP_STATUS.lock();
    status.state = "error".to_string();
    status.error = Some(msg.to_string());
    log::error!("FTP 服务错误: {}", msg);
}

/// 供状态展示：实际监听 SocketAddr（当前绑定字符串）
pub fn listen_addr() -> Option<String> {
    FTP_RUNTIME.lock().as_ref().and_then(|r| r.listen_addr.clone())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ftp::auth::hash_password;
    use crate::ftp::config::{FtpPermissions, FtpUser};

    fn test_cfg(root: &Path, port: u16) -> FtpConfig {
        FtpConfig {
            enabled: true,
            auto_start_with_app: false,
            auto_start_with_system: false,
            root_dir: root.to_string_lossy().to_string(),
            port,
            so_reuseaddr: true,
            pasv_ports: None,
            users: vec![FtpUser {
                username: "tester".into(),
                password: hash_password("test-pass").unwrap(),
                permissions: FtpPermissions {
                    upload: true,
                    download: true,
                    delete: true,
                    readonly: false,
                },
            }],
        }
    }

    #[tokio::test]
    async fn test_stop_releases_port_and_restart() {
        // Bug 1 核心验证：start → stop → 立刻再次 start（同端口）必须成功
        let probe = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = probe.local_addr().unwrap().port();
        drop(probe); // 释放探测端口

        let root = tempfile::tempdir().unwrap();
        let cfg = test_cfg(root.path(), port);

        start_ftp(cfg.clone()).await.expect("第一次启动应成功");
        assert_eq!(ftp_status().state, "running");

        stop_ftp().await.expect("停止应成功");
        assert_eq!(ftp_status().state, "stopped");

        // 立刻再次启动（同端口）—— 修复前会报 io error: Address already in use
        start_ftp(cfg).await.expect("停止后立刻重启必须成功（端口已释放）");
        assert_eq!(ftp_status().state, "running");

        stop_ftp().await.ok();
    }

    #[tokio::test]
    async fn test_start_missing_root_errors() {
        // Bug 2 要求：root_dir 为空 → 明确错误而非 panic
        let mut cfg = FtpConfig::default();
        cfg.root_dir = String::new();
        let err = start_ftp(cfg).await.unwrap_err();
        assert!(err.contains("根目录"), "错误信息应指明根目录问题: {}", err);
        assert_eq!(ftp_status().state, "error");

        // 根目录不存在
        let mut cfg = FtpConfig::default();
        cfg.root_dir = format!("C:\\vnt_ftp_does_not_exist_{}", std::process::id());
        let err = start_ftp(cfg).await.unwrap_err();
        assert!(err.contains("不存在"), "错误信息应指明目录不存在: {}", err);
    }

    #[tokio::test]
    async fn test_start_no_users_errors() {
        let root = tempfile::tempdir().unwrap();
        let mut cfg = test_cfg(root.path(), 2122);
        cfg.users.clear();
        let err = start_ftp(cfg).await.unwrap_err();
        assert!(err.contains("用户"), "错误信息应指明需要用户: {}", err);
    }

    #[tokio::test]
    async fn test_listen_error_sets_state() {
        // 端口被占用 → 状态 error + 返回 Err（覆盖监听失败路径）
        let blocker = tokio::net::TcpListener::bind("0.0.0.0:0").await.unwrap();
        let port = blocker.local_addr().unwrap().port();
        let root = tempfile::tempdir().unwrap();
        let cfg = test_cfg(root.path(), port); // 占用中的端口

        let err = start_ftp(cfg).await.unwrap_err();
        assert!(err.contains("监听失败") || err.contains("Address"), "应报告监听失败: {}", err);
        assert_eq!(ftp_status().state, "error");
    }
}
