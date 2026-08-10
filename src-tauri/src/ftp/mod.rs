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

use crate::state::AppState;

/// 读取当前 FTP 配置（含 keyring 密码哈希回填）
fn load_current(app: &AppHandle) -> config::FtpConfig {
    let state: State<'_, AppState> = app.state();
    let mut cfg = config::load_ftp_config(&state.config_dir);
    // 从 keyring 回填密码哈希（登录校验用）
    for user in &mut cfg.users {
        if user.password.is_empty() {
            user.password = config::get_password_hash(&user.username).unwrap_or_default();
        }
    }
    cfg
}

/// 启动 FTP 服务（F1）
#[tauri::command]
pub async fn ftp_start(app: AppHandle) -> Result<(), String> {
    let cfg = load_current(&app);
    if !cfg.enabled {
        // 总开关关闭时不允许启动
        return Err("FTP 服务总开关未开启".to_string());
    }
    server::start_ftp(cfg).await
}

/// 停止 FTP 服务（F1）
#[tauri::command]
pub async fn ftp_stop() -> Result<(), String> {
    server::stop_ftp().await
}

/// FTP 服务状态（F8：已停止 / 运行中 / 异常 + 监听地址）
#[tauri::command]
pub fn ftp_status() -> server::FtpServerStatus {
    server::ftp_status()
}

/// 获取 FTP 配置（密码不回传：password 字段为空）
#[tauri::command]
pub fn ftp_get_config(app: AppHandle) -> Result<config::FtpConfig, String> {
    let state: State<'_, AppState> = app.state();
    let mut cfg = config::load_ftp_config(&state.config_dir);
    // 密码哈希绝不出后端（清空回传字段）
    for user in &mut cfg.users {
        user.password.clear();
    }
    // F3 状态与系统开机自启保持同步显示
    cfg.auto_start_with_system = app.autolaunch().is_enabled().unwrap_or(false);
    Ok(cfg)
}

/// 保存 FTP 配置（F4-F7 + F3 联动）
///
/// 密码处理：
/// - 新密码（password 非空）→ argon2 哈希 → keyring（DPAPI）
/// - 密码为空（编辑未改密码）→ 保留 keyring 旧哈希
/// - 被删除的用户 → 同步删除 keyring 条目
/// 服务运行中保存 → 自动重启使配置生效。
#[tauri::command]
pub async fn ftp_save_config(app: AppHandle, cfg: config::FtpConfig) -> Result<(), String> {
    let state: State<'_, AppState> = app.state();
    let config_dir = state.config_dir.clone();

    let old = config::load_ftp_config(&config_dir);
    let mut cfg = cfg;

    // 1. 密码哈希处理
    for user in &mut cfg.users {
        user.permissions.normalize();
        if user.username.trim().is_empty() {
            return Err("用户名不能为空".to_string());
        }
        if !user.password.is_empty() {
            // 新密码/修改密码 → 哈希并存入 keyring
            let hash = auth::hash_password(&user.password)?;
            config::set_password_hash(&user.username, &hash)?;
            user.password = hash;
        } else {
            // 未改密码 → 保留旧哈希（内存中用于校验）
            user.password = config::get_password_hash(&user.username).unwrap_or_default();
        }
    }
    // 2. 被删除用户的 keyring 清理
    for old_user in &old.users {
        if !cfg.users.iter().any(|u| u.username == old_user.username) {
            config::delete_password(&old_user.username);
        }
    }

    // 3. 写盘
    config::save_ftp_config(&config_dir, &cfg)?;

    // 4. F3 联动：随系统开机自启（复用 tauri-plugin-autostart）
    let current = app.autolaunch().is_enabled().unwrap_or(false);
    if cfg.auto_start_with_system != current {
        if cfg.auto_start_with_system {
            app.autolaunch().enable().map_err(|e| format!("开启开机自启失败: {}", e))?;
        } else {
            app.autolaunch().disable().map_err(|e| format!("关闭开机自启失败: {}", e))?;
        }
    }

    // 5. 运行中 → 自动重启使配置生效
    let (user_count, port) = (cfg.users.len(), cfg.port);
    if server::ftp_status().state == "running" {
        server::stop_ftp().await?;
        if cfg.enabled {
            server::start_ftp(cfg).await?;
        }
    }
    ::log::info!("FTP 配置已保存（{} 个用户，端口 {}）", user_count, port);
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

/// 获取 FTP 连接日志（F9）
#[tauri::command]
pub fn ftp_get_logs() -> Vec<log::FtpLogEntry> {
    log::get_logs()
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
