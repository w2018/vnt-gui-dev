//! 系统托盘（文档 §3.4）
//!
//! 菜单结构：
//!   [状态行]（disabled，动态显示 状态/IP/组网编号）
//!   ----------
//!   连接        （按状态动态启用）
//!   断开        （按状态动态启用）
//!   ----------
//!   显示/隐藏窗口
//!   设置
//!   流量统计
//!   ----------
//!   退出
//!
//! 交互：左键弹菜单；双击图标显示主窗口；图标/tooltip 随连接状态实时切换。

use std::time::Duration;

use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_notification::NotificationExt;
use tauri_plugin_clipboard_manager::ClipboardExt;

use crate::config::load_config_store;
use crate::state::{AppState, ConnectionStatus};

/// 托盘 ID
pub const TRAY_ID: &str = "main-tray";

/// 5 态状态图标（编译期嵌入）
const ICON_CONNECTED: &[u8] = include_bytes!("../icons/tray-connected.png");
const ICON_CONNECTING: &[u8] = include_bytes!("../icons/tray-connecting.png");
const ICON_RECONNECTING: &[u8] = include_bytes!("../icons/tray-reconnecting.png");
const ICON_ERROR: &[u8] = include_bytes!("../icons/tray-error.png");
const ICON_DISCONNECTED: &[u8] = include_bytes!("../icons/tray-disconnected.png");

/// 创建系统托盘
pub fn create_tray(app: &AppHandle) -> tauri::Result<()> {
    // 状态行：可点击（点击复制 IP），动态文本
    let status_item = MenuItem::with_id(app, "copy_ip", "VNT GUI - 未连接", true, None::<&str>)?;
    let connect_item = MenuItem::with_id(app, "connect", "连接", true, None::<&str>)?;
    let disconnect_item = MenuItem::with_id(app, "disconnect", "断开", false, None::<&str>)?;
    let show_item = MenuItem::with_id(app, "toggle_window", "显示/隐藏窗口", true, None::<&str>)?;
    let settings_item = MenuItem::with_id(app, "open_settings", "设置", true, None::<&str>)?;
    let traffic_item = MenuItem::with_id(app, "open_traffic", "流量统计", true, None::<&str>)?;
    let sep1 = PredefinedMenuItem::separator(app)?;
    let sep2 = PredefinedMenuItem::separator(app)?;
    let sep3 = PredefinedMenuItem::separator(app)?;
    let sep4 = PredefinedMenuItem::separator(app)?;
    // 退出拆分：仅退 GUI（daemon 与服务保持运行）/ 完全退出
    let exit_gui_item = MenuItem::with_id(app, "exit_gui_only", "仅退出 GUI（保持服务运行）", true, None::<&str>)?;
    let exit_full_item = MenuItem::with_id(app, "exit_full", "完全退出（含所有服务）", true, None::<&str>)?;

    let menu = Menu::with_items(
        app,
        &[
            &status_item,
            &sep1,
            &connect_item,
            &disconnect_item,
            &sep2,
            &show_item,
            &settings_item,
            &traffic_item,
            &sep3,
            &exit_gui_item,
            &sep4,
            &exit_full_item,
        ],
    )?;

    // 保存动态菜单项句柄，供 update_tray_status 更新文本/启用状态
    {
        let state: tauri::State<'_, AppState> = app.state();
        *state.tray_menu_items.lock() = Some(crate::state::TrayMenuItems {
            status: status_item.clone(),
            connect: connect_item.clone(),
            disconnect: disconnect_item.clone(),
        });
    }

    let icon = tauri::image::Image::from_bytes(ICON_DISCONNECTED)?;

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
        .on_tray_icon_event(|tray, event| {
            // 双击托盘图标 → 显示主窗口（并恢复托盘可见）
            if let TrayIconEvent::DoubleClick { .. } = event {
                show_main_window(&tray.app_handle());
            }
        })
        .build(app)?;

    Ok(())
}

/// 显示主窗口并恢复托盘可见（供菜单/双击/快捷键共用）
pub fn show_main_window(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.set_focus();
    }
    // 恢复托盘可见（后台隐藏托盘开关下，显示窗口时重新出现）
    if let Some(tray) = app.tray_by_id(TRAY_ID) {
        let _ = tray.set_visible(true);
    }
}

/// 托盘菜单事件处理
async fn handle_tray_menu(app: AppHandle, item_id: &str) {
    match item_id {
        // 快速连接：读取活动配置 → daemon RPC 启动
        "connect" => {
            if let Some(config) = crate::config::load_config_store().get_active().cloned() {
                let _ = crate::daemon::rpc_client::vnt_start(config).await;
            }
        }
        // 断开连接 → daemon RPC
        "disconnect" => {
            let _ = crate::daemon::rpc_client::vnt_stop().await;
        }
        // 仅退出 GUI：daemon 独立进程保持运行，服务不断
        "exit_gui_only" => {
            app.exit(0);
        }
        // 完全退出：先停服务 + 关闭 daemon，再退出 GUI
        "exit_full" => {
            let app2 = app.clone();
            tauri::async_runtime::spawn(async move {
                let _ = crate::daemon::rpc_client::vnt_stop().await;
                let _ = crate::daemon::rpc_client::ftp_stop().await;
                let _ = crate::daemon::rpc_client::shutdown_daemon().await;
                tokio::time::sleep(Duration::from_millis(500)).await;
                app2.exit(0);
            });
        }
        // 2b：点击状态行 → 复制当前 IP 到剪贴板
        "copy_ip" => {
            let ip = {
                let state: tauri::State<'_, AppState> = app.state();
                let ip = state.virtual_ip.lock().clone();
                ip
            };
            if let Some(ip) = ip {
                let copied = app.clipboard().write_text(ip.clone()).is_ok();
                let _ = app
                    .notification()
                    .builder()
                    .title(if copied { "IP 已复制" } else { "复制失败" })
                    .body(if copied {
                        format!("{} 已复制到剪贴板", ip)
                    } else {
                        "无法访问系统剪贴板".to_string()
                    })
                    .show();
                if copied {
                    // 临时变更 tooltip 反馈 2 秒后恢复
                    if let Some(tray) = app.tray_by_id(TRAY_ID) {
                        let _ = tray.set_tooltip(Some("IP 已复制"));
                    }
                    let app2 = app.clone();
                    tauri::async_runtime::spawn(async move {
                        tokio::time::sleep(Duration::from_secs(2)).await;
                        let status = {
                            let s: tauri::State<'_, AppState> = app2.state();
                            let status = s.connection.read().clone();
                            status
                        };
                        update_tray_status(&app2, &status);
                    });
                }
            } else {
                let _ = app
                    .notification()
                    .builder()
                    .title("VNT GUI")
                    .body("当前未连接，没有可复制的 IP")
                    .show();
            }
        }
        "toggle_window" => {
            if let Some(window) = app.get_webview_window("main") {
                match window.is_visible() {
                    Ok(true) => {
                        let _ = window.hide();
                    }
                    _ => {
                        show_main_window(&app);
                    }
                }
            }
        }
        "open_settings" => {
            show_main_window(&app);
            let _ = app.emit("navigate", "/settings");
        }
        "open_traffic" => {
            show_main_window(&app);
            let _ = app.emit("navigate", "/traffic");
        }
        // 兼容旧 id（无 daemon 时旧菜单残留不会出现；直接走完全退出逻辑）
        "quit" => {
            app.exit(0);
        }
        _ => {}
    }
}

/// 根据连接状态实时更新托盘：图标、tooltip、菜单启用状态、状态行文本
pub fn update_tray_status(app: &AppHandle, status: &ConnectionStatus) {
    let Some(tray) = app.tray_by_id(TRAY_ID) else {
        return;
    };

    // 1. 图标（5 态颜色切换）
    let icon_bytes: &[u8] = match status {
        ConnectionStatus::Connected => ICON_CONNECTED,
        ConnectionStatus::Starting => ICON_CONNECTING,
        ConnectionStatus::Reconnecting { .. } => ICON_RECONNECTING,
        ConnectionStatus::Error { .. } => ICON_ERROR,
        ConnectionStatus::Stopped => ICON_DISCONNECTED,
    };
    if let Ok(icon) = tauri::image::Image::from_bytes(icon_bytes) {
        let _ = tray.set_icon(Some(icon));
    }

    // 2. tooltip（2a：编号始终可见）
    let token_short = load_config_store()
        .get_active()
        .map(|c| c.token.clone())
        .unwrap_or_else(|| "-".to_string());
    let tooltip = match status {
        ConnectionStatus::Connected => format!("VNT GUI - 已连接  编号:{}", token_short),
        ConnectionStatus::Starting | ConnectionStatus::Reconnecting { .. } => {
            format!("VNT GUI - 连接中...  编号:{}", token_short)
        }
        ConnectionStatus::Error { .. } => format!("VNT GUI - 错误  编号:{}", token_short),
        ConnectionStatus::Stopped => format!("VNT GUI - 未连接  编号:{}", token_short),
    };
    let _ = tray.set_tooltip(Some(&tooltip));

    // 3. 菜单：连接/断开动态启用 + 状态行文本（句柄存于 AppState）
    let items = {
        let state: tauri::State<'_, AppState> = app.state();
        let items = state.tray_menu_items.lock().clone();
        items
    };
    let Some(items) = items else {
        return;
    };
    let running = status.is_running();
    let _ = items.connect.set_enabled(!running);
    let _ = items.disconnect.set_enabled(running);

    // 4. 状态行（可点击复制 IP）：状态 + 当前 IP + 组网编号（脱敏，编号始终展示）
    let ip = {
        let state: tauri::State<'_, AppState> = app.state();
        let ip = state.virtual_ip.lock().clone();
        ip
    };
    let text = match status {
        ConnectionStatus::Connected => format!(
            "已连接  IP:{}  编号:{}",
            ip.as_deref().unwrap_or("-"),
            token_short
        ),
        ConnectionStatus::Starting => format!("连接中...  编号:{}", token_short),
        ConnectionStatus::Reconnecting { attempt } => {
            format!("重连中 (第 {} 次)  编号:{}", attempt, token_short)
        }
        ConnectionStatus::Error { .. } => format!("连接错误  编号:{}", token_short),
        ConnectionStatus::Stopped => format!("未连接  编号:{}", token_short),
    };
    let _ = items.status.set_text(text);
}
