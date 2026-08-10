//! FTP 生命周期管理（daemon 侧）
//!
//! 复用 lib 内现有 ftp 模块（libunftp server / keyring 密码 / 连接日志），
//! 密码哈希由 daemon 从系统凭据库（keyring）回填，RPC 传输不含密码。

use std::sync::Arc;

use tokio::sync::Mutex;

use crate::daemon::state_store::{self, RuntimeState};
use crate::ftp::config::{self, FtpConfig};

/// 启动 FTP（复用 ftp::server::start_ftp；密码从 keyring 回填）
pub async fn start(state: Arc<Mutex<RuntimeState>>, config: FtpConfig) -> Result<(), String> {
    // 从 keyring 回填密码哈希（RPC 传输不含密码）
    let mut cfg = config.clone();
    for user in &mut cfg.users {
        if user.password.is_empty() {
            user.password = config::get_password_hash(&user.username).unwrap_or_default();
        }
    }
    crate::ftp::server::start_ftp(cfg).await?;

    let mut s = state.lock().await;
    s.ftp_running = true;
    s.ftp_was_running = true;
    s.ftp_config = Some(config);
    drop(s);
    state_store::save(&*state.lock().await).await;
    tracing::info!("FTP 服务已启动");
    Ok(())
}

/// 停止 FTP
pub async fn stop(state: Arc<Mutex<RuntimeState>>) -> Result<(), String> {
    crate::ftp::server::stop_ftp().await?;
    let mut s = state.lock().await;
    s.ftp_running = false;
    s.ftp_was_running = false;
    drop(s);
    state_store::save(&*state.lock().await).await;
    tracing::info!("FTP 服务已停止");
    Ok(())
}

/// 重启 FTP
pub async fn restart(state: Arc<Mutex<RuntimeState>>, config: FtpConfig) -> Result<(), String> {
    stop(state.clone()).await?;
    start(state, config).await
}

/// 当前 FTP 连接日志（转发 ftp::log 缓冲）
pub fn get_logs() -> Vec<crate::ftp::log::FtpLogEntry> {
    crate::ftp::log::get_logs()
}
