//! FTP 服务模块（需求 F1-F9）
//!
//! 模块结构：
//! - `config.rs`   配置结构 + JSON 持久化（%APPDATA%/vnt-gui/ftp_config.json）+ keyring 密码存储
//! - `auth.rs`     Authenticator（argon2 校验）+ UserDetail/Provider
//! - `storage.rs`  StorageBackend（ROOT 限制 + 路径穿越防护 + 权限拦截）
//! - `server.rs`   libunftp Server 生命周期与状态
//! - `log.rs`      连接日志环形缓冲

pub mod auth;
pub mod config;
pub mod log;
pub mod server;
pub mod storage;

use tauri::{AppHandle, Manager, State};
use tauri_plugin_autostart::ManagerExt;
use tauri_plugin_dialog::DialogExt;

use crate::daemon::rpc_protocol::{FtpConfigWithSecrets, FtpUserWithPassword};
use crate::state::AppState;

/// 读取当前 FTP 配置（含 keyring 密码回填）
///
/// 禁止静默吞掉 keyring 读取失败：密码缺失必须明确暴露（见 to_rpc_cfg）。
fn load_current(app: &AppHandle) -> config::FtpConfig {
    let state: State<'_, AppState> = app.state();
    let mut cfg = config::load_ftp_config(&state.config_dir);
    // 从 keyring 回填明文密码（登录校验用）
    for user in &mut cfg.users {
        if user.password.is_empty() {
            match config::get_password(&user.username) {
                Some(pwd) => user.password = pwd,
                None => {
                    ::log::warn!(
                        "FTP 用户 {} 的密码在凭据库中不存在（keyring 读取失败或未设置）",
                        user.username
                    );
                }
            }
        }
    }
    cfg
}

/// 将内存中的 FtpConfig（含密码）转换为 RPC 传输结构
///
/// 任一用户密码为空 → 明确报错（不允许静默以空密码启动导致"凭据缺失"）
pub(crate) fn to_rpc_cfg(cfg: &config::FtpConfig) -> Result<FtpConfigWithSecrets, String> {
    let mut users = Vec::with_capacity(cfg.users.len());
    for user in &cfg.users {
        if user.password.is_empty() {
            return Err(format!(
                "用户 {} 的密码未设置（凭据库中不存在）。请在用户管理中为该用户设置密码后重试",
                user.username
            ));
        }
        users.push(FtpUserWithPassword {
            username: user.username.clone(),
            password: user.password.clone(),
            permissions: user.permissions.clone(),
        });
    }
    Ok(FtpConfigWithSecrets {
        enabled: cfg.enabled,
        auto_start_with_app: cfg.auto_start_with_app,
        root_dir: cfg.root_dir.clone(),
        port: cfg.port,
        so_reuseaddr: cfg.so_reuseaddr,
        pasv_ports: cfg.pasv_ports,
        users,
    })
}

/// 启动 FTP 服务（F1）—— 经 daemon RPC 管理（密码随 RPC 传输，daemon 不跨进程读 keyring）
#[tauri::command]
pub async fn ftp_start(app: AppHandle) -> Result<(), String> {
    let state: State<'_, AppState> = app.state();
    let cfg = load_current(&app);
    if !cfg.enabled {
        return Err("FTP 服务总开关未开启".to_string());
    }
    let rpc_cfg = to_rpc_cfg(&cfg)?;
    let _ = state;
    crate::daemon::rpc_client::ftp_start(rpc_cfg).await
}

/// 停止 FTP 服务（F1）—— 经 daemon RPC 管理
#[tauri::command]
pub async fn ftp_stop() -> Result<(), String> {
    crate::daemon::rpc_client::ftp_stop().await
}

/// FTP 服务状态（F8：已停止 / 运行中 / 异常 + 监听地址）—— daemon 状态映射
#[tauri::command]
pub async fn ftp_status(app: AppHandle) -> server::FtpServerStatus {
    use crate::daemon::rpc_protocol::DaemonResponse;
    match crate::daemon::rpc_client::get_state().await {
        Ok(DaemonResponse::State {
            ftp_running,
            ftp_config,
            ..
        }) => {
            if ftp_running {
                let port = ftp_config.map(|c| c.port).unwrap_or(2121);
                server::FtpServerStatus {
                    state: "running".to_string(),
                    listen_addr: Some(format!("0.0.0.0:{}", port)),
                    error: None,
                }
            } else {
                server::FtpServerStatus {
                    state: "stopped".to_string(),
                    listen_addr: None,
                    error: None,
                }
            }
        }
        _ => {
            // daemon 不可达 → 本地缓存状态（服务实际在 daemon 中）
            let _ = app;
            server::ftp_status()
        }
    }
}

/// 获取 FTP 配置（密码不回传：password 字段为空，但回传 password_set 状态）
#[tauri::command]
pub fn ftp_get_config(app: AppHandle) -> Result<config::FtpConfig, String> {
    let state: State<'_, AppState> = app.state();
    let mut cfg = config::load_ftp_config(&state.config_dir);
    for user in &mut cfg.users {
        // 🆕 探测 keyring 是否有密码，设置 password_set 标记（前端展示用）
        user.password_set = config::get_password(&user.username).is_some();
        user.password.clear(); // 明文密码绝不回传
    }
    // F3 状态与系统开机自启保持同步显示
    cfg.auto_start_with_system = app.autolaunch().is_enabled().unwrap_or(false);
    Ok(cfg)
}

/// 保存 FTP 配置（F4-F7 + F3 联动）
///
/// 密码处理：
/// - 新密码（password 非空）→ 明文写入 keyring（DPAPI 加密）
/// - 密码为空（编辑未改密码）→ 保留 keyring 旧密码
/// - 被删除的用户 → 同步删除 keyring 条目
/// 保存后经 RPC 让 daemon 重启 FTP 使配置生效（ROOT/端口/用户即时更新）。
#[tauri::command]
pub async fn ftp_save_config(app: AppHandle, cfg: config::FtpConfig) -> Result<(), String> {
    let state: State<'_, AppState> = app.state();
    let config_dir = state.config_dir.clone();

    // Bug 3：ROOT 目录路径校验（存在性）
    if !cfg.root_dir.trim().is_empty() && !std::path::Path::new(cfg.root_dir.trim()).is_dir() {
        return Err(format!("ROOT 目录不存在或不可访问: {}", cfg.root_dir));
    }

    let old = config::load_ftp_config(&config_dir);
    let mut cfg = cfg;

    // 1. 密码处理（明文 → keyring DPAPI）
    for user in &mut cfg.users {
        user.permissions.normalize();
        if user.username.trim().is_empty() {
            return Err("用户名不能为空".to_string());
        }
        if !user.password.is_empty() {
            // 新密码/修改密码 → 明文写入 keyring
            config::set_password(&user.username, &user.password)?;
            tracing::info!("用户 {} 密码已写入 keyring", user.username);
        } else {
            // 未填密码 → 检查 keyring 是否有旧密码（禁止 unwrap_or_default 静默吞失败）
            match config::get_password(&user.username) {
                Some(pwd) => {
                    // 保留旧密码，确保后续 to_rpc_cfg 不报错
                    user.password = pwd;
                    tracing::debug!("用户 {} 保留 keyring 旧密码", user.username);
                }
                None => {
                    // keyring 也没有 → 明确报错，不静默
                    return Err(format!(
                        "用户 {} 的密码在凭据库中不存在，请填写密码后保存",
                        user.username
                    ));
                }
            }
        }
    }
    // 2. 被删除用户的 keyring 清理
    for old_user in &old.users {
        if !cfg.users.iter().any(|u| u.username == old_user.username) {
            config::delete_password(&old_user.username);
        }
    }

    // 3. 写盘（password 字段 serde(skip) 不落盘）
    config::save_ftp_config(&config_dir, &cfg)?;

    // Bug 3：回读验证 —— 持久化 roundtrip 一致性
    let reloaded = config::load_ftp_config(&config_dir);
    if reloaded.root_dir != cfg.root_dir {
        return Err(format!("ROOT 目录持久化失败: roundtrip mismatch ({:?} != {:?})", reloaded.root_dir, cfg.root_dir));
    }
    if reloaded.port != cfg.port || reloaded.users.len() != cfg.users.len() {
        return Err("FTP 配置持久化失败: roundtrip mismatch".to_string());
    }
    ::log::info!("FTP 配置已保存并回读验证: root_dir={}, port={}, users={}", cfg.root_dir, cfg.port, cfg.users.len());

    // 4. F3 联动：随系统开机自启（复用 tauri-plugin-autostart）
    let current = app.autolaunch().is_enabled().unwrap_or(false);
    if cfg.auto_start_with_system != current {
        if cfg.auto_start_with_system {
            app.autolaunch().enable().map_err(|e| format!("开启开机自启失败: {}", e))?;
        } else {
            app.autolaunch().disable().map_err(|e| format!("关闭开机自启失败: {}", e))?;
        }
    }

    // 5. 运行中 → 经 RPC 让 daemon 重启使配置生效（密码随 RPC 传输）
    if crate::daemon::rpc_client::ping().await.is_ok() {
        if cfg.enabled {
            let rpc_cfg = to_rpc_cfg(&cfg)?;
            crate::daemon::rpc_client::ftp_start(rpc_cfg).await?;
        } else {
            crate::daemon::rpc_client::ftp_stop().await?;
        }
    } else {
        // daemon 不可达：仅持久化，返回提示（不阻塞保存）
        ::log::warn!("daemon 不可达，FTP 配置已保存但未生效（daemon 下次启动时加载）");
    }
    Ok(())
}
/// 选择 ROOT 目录（F4：系统文件夹选择器）
#[tauri::command]
pub async fn ftp_pick_root_dir(app: AppHandle) -> Result<String, String> {
    let (tx, rx) = tokio::sync::oneshot::channel::<Option<String>>();
    app.dialog().file().pick_folder(move |path| {
        let _ = tx.send(path.map(|p| p.into_path().map(|pb| pb.to_string_lossy().to_string()).unwrap_or_default()));
    });
    let picked = rx.await.map_err(|_| "文件选择器未返回结果".to_string())?;
    match picked {
        Some(p) if !p.is_empty() => Ok(p),
        _ => Ok(String::new()), // 用户取消 → 空串，前端忽略
    }
}

/// 获取 FTP 连接日志（F9）—— 经 daemon RPC
#[tauri::command]
pub async fn ftp_get_logs() -> Vec<log::FtpLogEntry> {
    match crate::daemon::rpc_client::ftp_get_logs().await {
        Ok(logs) => logs,
        Err(_) => Vec::new(),
    }
}

/// 获取所有监听地址（Bug 3）：遍历活跃网卡 IPv4 + 端口，如 ["192.168.1.100:2121", "10.26.0.3:2121", "127.0.0.1:2121"]
#[tauri::command]
pub fn ftp_get_listen_addresses(app: AppHandle) -> Result<Vec<String>, String> {
    let state: State<'_, AppState> = app.state();
    let cfg = config::load_ftp_config(&state.config_dir);
    let ips = collect_ipv4_addresses();
    Ok(format_addresses(&ips, cfg.port))
}

/// 遍历所有网络接口，收集 IPv4 地址（去重；含回环 127.0.0.1，含虚拟网卡 10.26.x.x）
fn collect_ipv4_addresses() -> Vec<String> {
    use network_interface::{Addr, NetworkInterface, NetworkInterfaceConfig};

    let mut ips: Vec<String> = Vec::new();
    if let Ok(interfaces) = NetworkInterface::show() {
        for iface in interfaces {
            for addr in iface.addr {
                if let Addr::V4(v4) = addr {
                    let s = v4.ip.to_string();
                    if !ips.contains(&s) {
                        ips.push(s);
                    }
                }
            }
        }
    }
    // 确保回环地址在列（本机访问入口）
    if !ips.iter().any(|ip| ip == "127.0.0.1") {
        ips.push("127.0.0.1".to_string());
    }
    ips
}

/// 纯函数：IP 列表 + 端口 → "ip:port" 列表（可单测）
fn format_addresses(ips: &[String], port: u16) -> Vec<String> {
    ips.iter().map(|ip| format!("{}:{}", ip, port)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_addresses_mock_ips() {
        // Bug 3 验证：mock 接口数据 → 断言格式化输出正确
        let ips = vec!["192.168.1.100".to_string(), "10.26.0.3".to_string(), "127.0.0.1".to_string()];
        let out = format_addresses(&ips, 2121);
        assert_eq!(out, vec!["192.168.1.100:2121", "10.26.0.3:2121", "127.0.0.1:2121"]);
    }

    #[test]
    fn test_format_addresses_custom_port() {
        let ips = vec!["10.0.0.5".to_string()];
        assert_eq!(format_addresses(&ips, 2122), vec!["10.0.0.5:2122"]);
    }

    #[test]
    fn test_format_addresses_empty() {
        assert!(format_addresses(&[], 2121).is_empty());
    }
}
