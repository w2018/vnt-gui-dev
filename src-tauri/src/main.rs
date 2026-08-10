// Prevents additional console window on Windows in release, DO NOT REMOVE!!
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

/// 启动参数解析（Bug 4 自启重构）
///
/// `--autostart`：开机自启（tauri-plugin-autostart 传入）——
/// 不显示主窗口（仅托盘），启动/连接 daemon（daemon 按持久化状态自动拉起 VNT + FTP）。
fn main() {
    let args: Vec<String> = std::env::args().collect();
    let is_autostart = args.iter().any(|a| a == "--autostart");
    vnt_gui_lib::run(is_autostart)
}
