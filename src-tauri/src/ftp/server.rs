//! FTP Server 生命周期（需求 F1/F8）
//!
//! 使用 libunftp 0.23（纯 Rust 内嵌，无外部 exe）：
//! - `Server::with_authenticator(generator, auth)` + `.user_detail_provider(provider)`
//! - `.passive_ports(range)` 配置 PASV 端口范围
//! - `listen()` 异步运行，`shutdown_indicator` 触发优雅关闭
//! - 全局运行时状态（running / addr / error）供 F8 状态指示查询

use std::path::Path;
use std::sync::Arc;

use libunftp::ServerBuilder;
use parking_lot::Mutex;
use tokio::sync::Notify;

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
    /// 服务任务句柄（结束 = 服务停止）
    handle: tauri::async_runtime::JoinHandle<()>,
    /// 停止信号
    shutdown: Arc<Notify>,
    /// 监听地址（启动时绑定成功后的实际地址）
    listen_addr: Option<String>,
}

static FTP_RUNTIME: Mutex<Option<FtpRuntime>> = Mutex::new(None);
static FTP_STATUS: Mutex<FtpServerStatus> = Mutex::new(FtpServerStatus {
    state: String::new(),
    listen_addr: None,
    error: None,
});

/// 当前状态（F8）
pub fn ftp_status() -> FtpServerStatus {
    FTP_STATUS.lock().clone()
}

/// 启动 FTP 服务（F1）
///
/// 校验：root_dir 存在；端口绑定失败返回错误并置状态 error。
pub async fn start_ftp(cfg: FtpConfig) -> Result<(), String> {
    // 1. 已在运行 → 先停止
    stop_ftp().await;

    // 2. 校验 root 目录
    let root = Path::new(&cfg.root_dir);
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
    // 入口：with_user_detail_provider（自定义 User 类型，无 DefaultUser 约束）
    //       + .authenticator() 替换匿名认证
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
    let server = builder.build().map_err(|e| {
        let msg = format!("FTP 服务器构建失败: {}", e);
        set_status_error(&msg);
        msg
    })?;

    // 6. 监听 + 运行（绑定失败 → 状态 error）
    let bind = format!("0.0.0.0:{}", cfg.port);
    let bind_for_task = bind.clone();
    let shutdown = Arc::new(Notify::new());
    let shutdown_fut = shutdown.clone();
    let handle = tauri::async_runtime::spawn(async move {
        // listen 内部先绑定再进入服务循环；绑定失败返回 Err
        let bind_for_msg = bind_for_task.clone();
        if let Err(e) = server.listen(bind_for_task).await {
            let msg = format!("FTP 监听失败 {}: {}", bind_for_msg, e);
            log::error!("{}", msg);
            set_status_error(&msg);
        } else {
            // listen 正常结束（shutdown 触发）
            let mut status = FTP_STATUS.lock();
            status.state = "stopped".to_string();
            status.listen_addr = None;
        }
        // 通知 shutdown 已结束（listen 返回后 handle 结束）
        shutdown_fut.notify_waiters();
    });
    // 短暂等待确认绑定成功（端口占用时尽早暴露错误）
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    {
        let status = FTP_STATUS.lock();
        if status.state == "error" {
            let msg = status.error.clone().unwrap_or_else(|| "未知错误".into());
            let _ = handle.abort();
            return Err(msg);
        }
    }

    *FTP_RUNTIME.lock() = Some(FtpRuntime {
        handle,
        shutdown,
        listen_addr: Some(bind.clone()),
    });
    *FTP_STATUS.lock() = FtpServerStatus {
        state: "running".to_string(),
        listen_addr: Some(bind.clone()),
        error: None,
    };
    log::info!("FTP 服务已启动: {} ({} 个用户)", bind, cfg.users.len());
    Ok(())
}

/// 停止 FTP 服务（F1）
pub async fn stop_ftp() -> Result<(), String> {
    let runtime = FTP_RUNTIME.lock().take();
    if let Some(runtime) = runtime {
        runtime.shutdown.notify_one();
        // 等待服务任务结束（上限 3 秒）
        let _ = tokio::time::timeout(std::time::Duration::from_secs(3), runtime.handle).await;
        log::info!("FTP 服务已停止");
    }
    *FTP_STATUS.lock() = FtpServerStatus::default();
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
