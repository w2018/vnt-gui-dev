//! VNT GUI —— Tauri 入口、插件注册与全部命令（文档 §3.10 / §3.11）

mod autostart;
mod config;
mod logger;
mod sidecar;
mod state;
mod traffic;
mod tray;
mod updater;

use std::path::PathBuf;
use std::time::Duration;

use tauri::{Manager, State};
use tauri_plugin_autostart::ManagerExt;

use config::{ConfigStore, VntConfig};
use state::{AppState, ConnectionStatus, LogEntry, TrafficSnapshot};
use updater::UpdateInfo;

// ==================== 连接控制 ====================

/// 启动连接（使用指定配置）
#[tauri::command]
async fn start_connection(app: tauri::AppHandle, config_id: String) -> Result<(), String> {
    let mut store = config::load_config_store();
    let cfg = store
        .get(&config_id)
        .cloned()
        .ok_or_else(|| "配置不存在".to_string())?;
    store.set_active(&config_id);
    config::save_config_store(&store)?;

    {
        let state: State<'_, AppState> = app.state();
        *state.active_config_id.write() = Some(config_id);
    }

    sidecar::start_vnt(app, cfg)
}

/// 停止连接
#[tauri::command]
async fn stop_connection(app: tauri::AppHandle) -> Result<(), String> {
    sidecar::stop_vnt(app)
}

/// 获取当前连接状态
#[tauri::command]
async fn get_status(app: tauri::AppHandle) -> ConnectionStatus {
    let state: State<'_, AppState> = app.state();
    let status = state.connection.read().clone();
    status
}

// ==================== 配置管理 ====================

/// 获取全部配置
#[tauri::command]
fn get_configs() -> ConfigStore {
    config::load_config_store()
}

/// 保存/更新配置
#[tauri::command]
fn save_config(config: VntConfig) -> Result<(), String> {
    let mut store = config::load_config_store();
    store.add_or_update(config);
    config::save_config_store(&store)
}

/// 删除配置
#[tauri::command]
fn delete_config(id: String) -> Result<(), String> {
    let mut store = config::load_config_store();
    store.delete(&id);
    config::save_config_store(&store)
}

/// 切换活动配置
#[tauri::command]
async fn set_active_config(app: tauri::AppHandle, id: String) -> Result<(), String> {
    let mut store = config::load_config_store();
    store.set_active(&id);
    config::save_config_store(&store)?;

    let state: State<'_, AppState> = app.state();
    *state.active_config_id.write() = Some(id);
    Ok(())
}

// ==================== 日志 ====================

/// 获取历史日志
#[tauri::command]
fn get_logs(app: tauri::AppHandle) -> Vec<LogEntry> {
    let state: State<'_, AppState> = app.state();
    state.log_buffer.get_all()
}

/// 清空日志
#[tauri::command]
fn clear_logs(app: tauri::AppHandle) -> Result<(), String> {
    let state: State<'_, AppState> = app.state();
    state.log_buffer.clear();
    Ok(())
}

/// 导出日志到文件
#[tauri::command]
fn export_logs(app: tauri::AppHandle, path: String) -> Result<(), String> {
    let state: State<'_, AppState> = app.state();
    let logs = state.log_buffer.get_all();
    let content = logs
        .iter()
        .map(|e| {
            format!(
                "[{}] {:?} {}",
                e.timestamp,
                serde_json::to_value(&e.level).unwrap_or_default(),
                e.message
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    std::fs::write(&path, content).map_err(|e| format!("导出失败: {}", e))?;
    Ok(())
}

// ==================== 更新 ====================

/// 检查更新（对比本地 vnt-cli 版本与 GitHub 最新 release）
#[tauri::command]
async fn check_update(app: tauri::AppHandle) -> Result<UpdateInfo, String> {
    updater::check_update(&app).await
}

/// 下载并替换 vnt-cli 二进制（Phase 4 完整实现）
#[tauri::command]
async fn download_and_replace(app: tauri::AppHandle, url: String) -> Result<(), String> {
    updater::download_and_replace(app, &url).await
}

// ==================== 自启 ====================

/// 设置开机自启
#[tauri::command]
fn set_autostart(app: tauri::AppHandle, enabled: bool) -> Result<(), String> {
    let manager = app.autolaunch();
    if enabled {
        manager.enable().map_err(|e| format!("启用自启失败: {}", e))
    } else {
        manager.disable().map_err(|e| format!("禁用自启失败: {}", e))
    }
}

/// 查询开机自启状态
#[tauri::command]
fn is_autostart_enabled(app: tauri::AppHandle) -> Result<bool, String> {
    app.autolaunch()
        .is_enabled()
        .map_err(|e| format!("查询自启状态失败: {}", e))
}

// ==================== 版本与设备 ====================

/// 获取应用版本号
#[tauri::command]
fn get_app_version() -> String {
    env!("CARGO_PKG_VERSION").to_string()
}

/// 获取 vnt-cli 版本（解析 `--help` 输出中的 version: 行）
#[tauri::command]
async fn get_vnt_version(app: tauri::AppHandle) -> Result<String, String> {
    updater::local_vnt_version(&app).await
}

/// 获取在线设备列表（后台运行 vnt 时执行 `--list` 解析，尽力而为）
#[tauri::command]
async fn get_device_list(app: tauri::AppHandle) -> Result<Vec<state::PeerInfo>, String> {
    use tauri_plugin_shell::ShellExt;
    let output = app
        .shell()
        .sidecar("vnt-cli")
        .map_err(|e| format!("sidecar 不可用: {}", e))?
        .args(["--list"])
        .output()
        .await
        .map_err(|e| format!("执行 vnt-cli --list 失败: {}", e))?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let text = if !stdout.trim().is_empty() { &stdout } else { &stderr };

    // 尝试 JSON 解析
    if let Ok(peers) = serde_json::from_str::<Vec<state::PeerInfo>>(text) {
        return Ok(peers);
    }

    // 尽力解析文本行：提取 IP 与名称
    let mut peers = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with("name") {
            continue;
        }
        if let Some(ip) = extract_ip(line) {
            let name = line
                .split_whitespace()
                .next()
                .unwrap_or("未知设备")
                .trim_matches('|')
                .to_string();
            peers.push(state::PeerInfo {
                name,
                virtual_ip: ip,
                connection_type: if line.to_lowercase().contains("relay") {
                    "relay".to_string()
                } else {
                    "p2p".to_string()
                },
                latency: extract_latency(line),
                status: "online".to_string(),
            });
        }
    }
    Ok(peers)
}

/// 从文本中提取 IPv4 地址
fn extract_ip(line: &str) -> Option<String> {
    let mut best: Option<String> = None;
    for token in line.split(|c: char| !c.is_ascii_digit() && c != '.') {
        let t = token.trim_matches(|c: char| !c.is_ascii_digit() && c != '.');
        let parts: Vec<&str> = t.split('.').collect();
        if parts.len() == 4
            && parts.iter().all(|p| {
                !p.is_empty() && p.len() <= 3 && p.parse::<u16>().map(|v| v <= 255).unwrap_or(false)
            })
        {
            best = Some(t.to_string());
        }
    }
    best
}

/// 从文本中提取延迟（如 12ms / 12 ms）
fn extract_latency(line: &str) -> u64 {
    let lower = line.to_lowercase();
    let idx = lower.find("ms").unwrap_or(usize::MAX);
    if idx == usize::MAX || idx == 0 {
        return 0;
    }
    let before = &lower[..idx];
    let num_part = before
        .chars()
        .rev()
        .take_while(|c| c.is_ascii_digit())
        .collect::<String>()
        .chars()
        .rev()
        .collect::<String>();
    num_part.parse::<u64>().unwrap_or(0)
}

/// 获取流量统计快照
#[tauri::command]
async fn get_traffic_stats(app: tauri::AppHandle) -> Result<TrafficSnapshot, String> {
    let state: State<'_, AppState> = app.state();
    let snap = state.traffic_snapshot.read().clone();
    Ok(snap)
}

// ==================== 应用入口 ====================

/// 应用入口
#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    env_logger::init();

    tauri::Builder::default()
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            Some(vec![autostart::AUTOSTART_FLAG]),
        ))
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_fs::init())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_global_shortcut::Builder::new().build())
        .setup(|app| {
            // 全局状态
            let config_dir = config::get_config_path()
                .parent()
                .unwrap_or(&PathBuf::from("."))
                .to_path_buf();
            app.manage(AppState::new(config_dir));
            {
                let state: State<'_, AppState> = app.state();
                *state.app_handle.lock() = Some(app.handle().clone());
            }

            // 系统托盘
            tray::create_tray(app.handle())?;

            // 流量监控（每秒采集虚拟网卡统计）
            traffic::start_traffic_monitor(app.handle().clone());

            // 开机自启：延迟 3 秒自动连接（等系统网络就绪）
            let args: Vec<String> = std::env::args().collect();
            if args.iter().any(|a| a == autostart::AUTOSTART_FLAG) {
                let handle = app.handle().clone();
                tauri::async_runtime::spawn(async move {
                    tokio::time::sleep(Duration::from_secs(3)).await;
                    if let Some(cfg) = sidecar::load_active_config(&handle) {
                        log::info!("自启参数，自动连接配置: {}", cfg.name);
                        if let Err(e) = sidecar::start_vnt(handle, cfg) {
                            log::error!("自启自动连接失败: {}", e);
                        }
                    }
                });
            }

            Ok(())
        })
        .on_window_event(|window, event| {
            // 关闭窗口 = 最小化到托盘（文档 §3.4.3）
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                let _ = window.hide();
            }
        })
        .invoke_handler(tauri::generate_handler![
            start_connection,
            stop_connection,
            get_status,
            get_configs,
            save_config,
            delete_config,
            set_active_config,
            get_logs,
            clear_logs,
            export_logs,
            check_update,
            download_and_replace,
            set_autostart,
            is_autostart_enabled,
            get_app_version,
            get_vnt_version,
            get_device_list,
            get_traffic_stats,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
