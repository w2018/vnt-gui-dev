//! FTP 生命周期管理（daemon 侧）
//!
//! 复用 lib 内现有 ftp 模块（libunftp server / keyring 密码 / 连接日志）。
//!
//! 密码链路（修复"凭据缺失"根因，方案 A）：
//! - GUI 从自己的 keyring 读明文密码 → 经 RPC `FtpConfigWithSecrets` 传给 daemon（127.0.0.1 回环，不落盘）
//! - daemon 直接用 RPC 密码启动，并**同时写入 daemon 侧 keyring**（同进程读写同一 TargetName，可靠）
//! - daemon 重启恢复（无 RPC 来源）→ `start_restored` 从自己 keyring 回填；读不到 → 明确报错，不静默

use std::sync::Arc;

use tokio::sync::Mutex;

use crate::daemon::rpc_protocol::FtpConfigWithSecrets;
use crate::daemon::state_store::{self, RuntimeState};
use crate::ftp::config::{self, FtpConfig, FtpUser};

/// 将 RPC 带密配置转为内存 FtpConfig（密码直接采用，不读 keyring）
fn from_rpc_cfg(rpc: FtpConfigWithSecrets) -> FtpConfig {
    FtpConfig {
        enabled: rpc.enabled,
        auto_start_with_app: rpc.auto_start_with_app,
        auto_start_with_system: false,
        root_dir: rpc.root_dir,
        port: rpc.port,
        so_reuseaddr: rpc.so_reuseaddr,
        pasv_ports: rpc.pasv_ports,
        users: rpc
            .users
            .into_iter()
            .map(|u| FtpUser {
                username: u.username,
                password: u.password,
                permissions: u.permissions,
                password_set: false,
            })
            .collect(),
    }
}

/// 启动 FTP（RPC 路径：密码直接来自 GUI，不再跨进程读 keyring）
pub async fn start(state: Arc<Mutex<RuntimeState>>, rpc_cfg: FtpConfigWithSecrets) -> Result<(), String> {
    let cfg = from_rpc_cfg(rpc_cfg);
    start_inner(state.clone(), cfg).await
}

/// 重启 FTP（RPC 路径）
pub async fn restart(state: Arc<Mutex<RuntimeState>>, rpc_cfg: FtpConfigWithSecrets) -> Result<(), String> {
    let cfg = from_rpc_cfg(rpc_cfg);
    stop(state.clone()).await?;
    start_inner(state, cfg).await
}

/// 内部实现：启动 FTP + 写 daemon 侧 keyring + 持久化状态
async fn start_inner(state: Arc<Mutex<RuntimeState>>, mut cfg: FtpConfig) -> Result<(), String> {
    // 密码写入 daemon 侧 keyring：保证 daemon 重启后能从自己凭据库恢复（同进程读写同一 TargetName）
    for user in &cfg.users {
        if user.password.is_empty() {
            return Err(format!(
                "用户 {} 的密码为空（RPC 未携带或凭据缺失），无法启动 FTP",
                user.username
            ));
        }
        config::set_password(&user.username, &user.password)
            .map_err(|e| format!("写入系统凭据库失败（用户 {}）: {}", user.username, e))?;
    }
    // 内存 FtpConfig 序列化时 password 字段 serde(skip)，state_store 落盘不含密码
    crate::ftp::server::start_ftp(cfg.clone()).await?;

    let mut s = state.lock().await;
    s.ftp_running = true;
    s.ftp_was_running = true;
    s.ftp_config = Some(cfg);
    drop(s);
    state_store::save(&*state.lock().await).await;
    tracing::info!("FTP 服务已启动（密码已从 RPC 正确加载，已写入 daemon 侧凭据库）");
    Ok(())
}

/// 启动 FTP（恢复路径：daemon 重启后按持久化状态自动拉起，无 RPC 来源）
///
/// 从 daemon 侧 keyring 回填明文密码；任一用户密码缺失 → 明确报错（不静默启动）。
pub async fn start_restored(state: Arc<Mutex<RuntimeState>>, mut cfg: FtpConfig) -> Result<(), String> {
    for user in &mut cfg.users {
        if user.password.is_empty() {
            match config::get_password(&user.username) {
                Some(pwd) => {
                    user.password = pwd;
                }
                None => {
                    tracing::warn!(
                        "恢复 FTP 失败：用户 {} 的密码在 daemon 侧凭据库中不存在",
                        user.username
                    );
                    return Err(format!(
                        "用户 {} 的密码在凭据库中不存在，无法自动恢复 FTP 服务（请在 GUI 中重新设置密码）",
                        user.username
                    ));
                }
            }
        }
    }
    start_inner(state, cfg).await
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

/// 当前 FTP 连接日志（转发 ftp::log 缓冲）
pub fn get_logs() -> Vec<crate::ftp::log::FtpLogEntry> {
    crate::ftp::log::get_logs()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::daemon::rpc_protocol::FtpUserWithPassword;
    use crate::ftp::config::FtpPermissions;

    #[test]
    fn rpc_cfg_password_flows_into_memory() {
        // 方案 A 验证：RPC 携带的明文密码必须完整进入内存 FtpConfig
        let rpc = FtpConfigWithSecrets {
            enabled: true,
            auto_start_with_app: false,
            root_dir: "C:/tmp".into(),
            port: 2121,
            so_reuseaddr: true,
            pasv_ports: None,
            users: vec![FtpUserWithPassword {
                username: "admin".into(),
                password: "admin123".into(),
                permissions: FtpPermissions {
                    upload: true,
                    download: true,
                    delete: true,
                    readonly: false,
                },
            }],
        };
        let cfg = from_rpc_cfg(rpc);
        assert_eq!(cfg.users.len(), 1);
        assert_eq!(cfg.users[0].username, "admin");
        assert_eq!(cfg.users[0].password, "admin123");
    }

    #[test]
    fn restored_without_keyring_password_fails_explicitly() {
        // 恢复路径：密码缺失且 keyring 读不到 → 明确报错（禁止静默空密码启动）
        let state = Arc::new(Mutex::new(RuntimeState::default()));
        let cfg = FtpConfig {
            enabled: true,
            root_dir: ".".into(),
            port: 2121,
            users: vec![FtpUser {
                username: "ghost".into(),
                password: String::new(),
                permissions: FtpPermissions::default(),
                password_set: false,
            }],
            ..Default::default()
        };
        let rt = tokio::runtime::Runtime::new().unwrap();
        let err = rt
            .block_on(start_restored(state, cfg))
            .expect_err("空密码必须报错");
        assert!(err.contains("ghost"), "错误信息应指明用户: {}", err);
    }

    #[test]
    fn start_rejects_empty_rpc_password() {
        // RPC 路径：密码为空 → start 明确报错（不静默）
        let state = Arc::new(Mutex::new(RuntimeState::default()));
        let rpc = FtpConfigWithSecrets {
            enabled: true,
            auto_start_with_app: false,
            root_dir: ".".into(),
            port: 2121,
            so_reuseaddr: true,
            pasv_ports: None,
            users: vec![FtpUserWithPassword {
                username: "admin".into(),
                password: String::new(),
                permissions: FtpPermissions::default(),
            }],
        };
        let rt = tokio::runtime::Runtime::new().unwrap();
        let err = rt
            .block_on(start(state, rpc))
            .expect_err("空密码必须报错");
        assert!(err.contains("admin"), "错误信息应指明用户: {}", err);
    }
}
