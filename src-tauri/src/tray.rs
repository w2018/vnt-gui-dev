//! 系统托盘（文档 §3.4）

use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::{AppHandle, Emitter, Manager};

use crate::sidecar::{self, load_active_config};
use crate::state::{AppState, ConnectionStatus};

/// 托盘 ID
pub const TRAY_ID: &str = "main-tray";

/// 托盘图标（编译期嵌入，5 态共用，tooltip 区分状态）
const TRAY_ICON: &[u8] = include_bytes!("../icons/32x32.png");

/// 创建系统托盘（菜单结构见文档 §3.4.1）
pub fn create_tray(app: &AppHandle) -> tauri::Result<()> {
    let connect_item = MenuItem::with_id(app, "toggle_connect", "连接", true, None::<&str>)?;
    let show_item = MenuItem::with_id(app, "toggle_window", "显示/隐藏窗口", true, None::<&str>)?;
    let settings_item = MenuItem::with_id(app, "open_settings", "设置", true, None::<&str>)?;
    let traffic_item = MenuItem::with_id(app, "open_traffic", "流量统计", true, None::<&str>)?;
    let separator = PredefinedMenuItem::separator(app)?;
    let quit_item = MenuItem::with_id(app, "quit", "退出", true, None::<&str>)?;

    let menu = Menu::with_items(
        app,
        &[
            &connect_item,
            &show_item,
            &settings_item,
            &traffic_item,
            &separator,
            &quit_item,
        ],
    )?;

    let icon = tauri::image::Image::from_bytes(TRAY_ICON)?;

    TrayIconBuilder::with_id(TRAY_ID)
        .icon(icon)
        .tooltip("VNT GUI - 未连接")
        .menu(&menu)
        .show_menu_on_left_click(true)
        .on_menu_event(|app, event| {
            let app = app.clone();
            tauri::async_runtime::spawn(async move {
                handle_tray_menu(app, event.id.as_ref()).await;
            });
        })
        .build(app)?;

    Ok(())
}

/// 托盘菜单事件处理
async fn handle_tray_menu(app: AppHandle, item_id: &str) {
    match item_id {
        "toggle_connect" => {
            let status = {
                let state: tauri::State<'_, AppState> = app.state();
                let status = state.connection.read().clone();
                status
            };
            if status.is_running() {
                let _ = sidecar::stop_vnt(app);
            } else if let Some(config) = load_active_config(&app) {
                let _ = sidecar::start_vnt(app, config);
            }
        }
        "toggle_window" => {
            if let Some(window) = app.get_webview_window("main") {
                match window.is_visible() {
                    Ok(true) => {
                        let _ = window.hide();
                    }
                    _ => {
                        let _ = window.show();
                        let _ = window.set_focus();
                    }
                }
            }
        }
        "open_settings" => {
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.show();
                let _ = window.set_focus();
                let _ = window.emit("navigate", "/settings");
            }
        }
        "open_traffic" => {
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.show();
                let _ = window.set_focus();
                let _ = window.emit("navigate", "/traffic");
            }
        }
        "quit" => {
            // 先停止 sidecar，再退出
            let _ = sidecar::stop_vnt(app.clone());
            app.exit(0);
        }
        _ => {}
    }
}

/// 根据连接状态更新托盘 tooltip
pub fn update_tray_status(app: &AppHandle, status: &ConnectionStatus) {
    let Some(tray) = app.tray_by_id(TRAY_ID) else {
        return;
    };
    let tooltip = match status {
        ConnectionStatus::Connected => "VNT GUI - 已连接",
        ConnectionStatus::Starting | ConnectionStatus::Reconnecting { .. } => {
            "VNT GUI - 连接中..."
        }
        ConnectionStatus::Error { .. } => "VNT GUI - 错误",
        ConnectionStatus::Stopped => "VNT GUI - 未连接",
    };
    let _ = tray.set_tooltip(Some(tooltip));
}
