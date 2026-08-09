//! Sidecar 进程管理 —— 核心模块（文档 §3.3）
//!
//! vnt-cli 是 long-running 进程：spawn、流式读取输出、解析状态、
//! 崩溃后指数退避自动重连、优雅关闭。
//!
//! 实现说明：本模块全部为同步函数，异步只存在于明确的
//! `tauri::async_runtime::spawn` block 内（rx.recv / sleep 均 Send），
//! 避免 async fn 嵌套导致 future 非 Send 的编译问题。

use std::time::Duration;

use tauri::{AppHandle, Emitter, Manager, State};
use tauri_plugin_shell::process::CommandEvent;
use tauri_plugin_shell::ShellExt;

use crate::config::VntConfig;
use crate::state::{AppState, ConnectionStatus, LogEntry, LogLevel};
use crate::tray;

/// 最大重连次数
pub const MAX_RETRY: u32 = 10;
/// 基础重连延迟（毫秒）
pub const BASE_DELAY_MS: u64 = 1000;

/// 根据配置构建 vnt-cli 命令行参数（文档 §3.3.2 / §9）
pub fn build_args(config: &VntConfig) -> Vec<String> {
    let mut args = Vec::new();

    args.push("-k".to_string());
    args.push(config.token.clone());

    if let Some(ref name) = config.device_name {
        args.push("-n".to_string());
        args.push(name.clone());
    }

    if let Some(ref device_id) = config.device_id {
        args.push("-d".to_string());
        args.push(device_id.clone());
    }

    if let Some(ref ip) = config.virtual_ip {
        args.push("--ip".to_string());
        args.push(ip.clone());
    }

    if let Some(ref server) = config.server_address {
        // vnt-cli 协议通过 -s 地址前缀指定（tcp:// / ws:// / wss://），默认 udp
        let mut s = server.clone();
        if config.use_tcp && !s.contains("://") {
            s = format!("tcp://{}", s);
        } else if config.use_ws && !s.contains("://") {
            s = format!("ws://{}", s);
        }
        args.push("-s".to_string());
        args.push(s);
    }

    if let Some(ref password) = config.password {
        args.push("-w".to_string());
        args.push(password.clone());
    }

    if config.server_encrypt {
        args.push("-W".to_string());
    }

    // 点对网配置
    for in_ip in &config.in_ips {
        args.push("-i".to_string());
        args.push(in_ip.clone());
    }
    for out_ip in &config.out_ips {
        args.push("-o".to_string());
        args.push(out_ip.clone());
    }

    // 压缩
    if let Some(ref comp) = config.compressor {
        args.push("--compressor".to_string());
        args.push(comp.clone());
    }

    // MTU（vnt-cli 参数为 -u）
    if let Some(mtu) = config.mtu {
        args.push("-u".to_string());
        args.push(mtu.to_string());
    }

    if config.no_proxy {
        args.push("--no-proxy".to_string());
    }

    args
}

/// 启动 vnt-cli sidecar（文档 §3.3.1，同步入口，内部 spawn 监听任务）
pub fn start_vnt(app: AppHandle, config: VntConfig) -> Result<(), String> {
    // 先清理可能残留的旧进程
    stop_vnt(app.clone())?;

    // 清除上一轮连接的虚拟 IP
    {
        let state: State<'_, AppState> = app.state();
        *state.virtual_ip.lock() = None;
    }

    let args = build_args(&config);
    log::info!("启动 vnt-cli，参数: {:?}", args);

    let sidecar_cmd = app
        .shell()
        .sidecar("vnt-cli")
        .map_err(|e| format!("找不到 vnt-cli sidecar: {}", e))?;

    let (mut rx, child) = sidecar_cmd
        .args(&args)
        .spawn()
        .map_err(|e| format!("启动 vnt-cli 失败: {}", e))?;

    // 持有 child handle（否则进程会被杀死）
    {
        let state: State<'_, AppState> = app.state();
        *state.sidecar_child.write() = Some(child);
    }

    emit_status(&app, ConnectionStatus::Starting);

    // 异步读取输出流
    let app_clone = app.clone();
    tauri::async_runtime::spawn(async move {
        while let Some(event) = rx.recv().await {
            match event {
                CommandEvent::Stdout(line) => {
                    let text = String::from_utf8_lossy(&line);
                    for l in text.split('\n') {
                        handle_output_line(&app_clone, l, false);
                    }
                }
                CommandEvent::Stderr(line) => {
                    let text = String::from_utf8_lossy(&line);
                    for l in text.split('\n') {
                        handle_output_line(&app_clone, l, true);
                    }
                }
                CommandEvent::Terminated(status) => {
                    log::warn!("vnt-cli terminated: {:?}", status);
                    handle_termination(&app_clone);
                    break;
                }
                _ => {}
            }
        }
    });

    Ok(())
}

/// 优雅停止 vnt-cli（文档 §3.3.4，同步实现）
pub fn stop_vnt(app: AppHandle) -> Result<(), String> {
    // 先置 Stopped，这样 Terminated 事件触发时不重连
    emit_status(&app, ConnectionStatus::Stopped);

    let state: State<'_, AppState> = app.state();
    let child = state.sidecar_child.write().take();
    if let Some(child) = child {
        // CommandChild::kill 为同步调用（消费 self）
        match child.kill() {
            Ok(()) => log::info!("vnt-cli 已退出"),
            Err(e) => log::warn!("kill 失败: {}", e),
        }
    }
    Ok(())
}

/// 处理一行输出：写入日志 + 状态机转换（文档 §3.3.3）
fn handle_output_line(app: &AppHandle, line: &str, is_stderr: bool) {
    let line = line.trim();
    if line.is_empty() {
        return;
    }

    // 写入环形缓冲区并转发前端
    let level = if is_stderr { LogLevel::Warn } else { LogLevel::Info };
    push_log(app, level, line.to_string());

    // 解析状态
    if line.contains("Connect Successfully") || line.contains("Connection status: Connected") {
        emit_status(app, ConnectionStatus::Connected);
    } else if line.contains("register ip=") {
        // 注册成功，获取虚拟 IP
        if let Some(ip) = parse_virtual_ip(line) {
            log::info!("虚拟 IP 已分配: {}", ip);
            {
                let state: State<'_, AppState> = app.state();
                *state.virtual_ip.lock() = Some(ip.clone());
            }
            let _ = app.emit("virtual-ip-assigned", &ip);
        }
    } else if line.to_lowercase().contains("error")
        || line.contains("failed")
        || line.contains("timeout")
        || line.contains("ip conflict")
    {
        let msg = extract_error_message(line);
        emit_status(app, ConnectionStatus::Error { message: msg });
    }

    // 解析延迟（如 "latency: 12ms" / "12 ms"），实时推送到前端
    if let Some(ms) = extract_latency(line) {
        let _ = app.emit("latency-update", ms);
    }
}

/// 进程终止处理：指数退避重连（文档 §3.3.3）
fn handle_termination(app: &AppHandle) {
    let current = read_connection(app);

    // 用户主动停止，不重连
    if matches!(current, ConnectionStatus::Stopped) {
        return;
    }

    // 指数退避
    let attempt = match current {
        ConnectionStatus::Reconnecting { attempt } => attempt + 1,
        _ => 1,
    };

    if attempt > MAX_RETRY {
        emit_status(
            app,
            ConnectionStatus::Error {
                message: format!("重连 {} 次后仍失败，请检查网络", MAX_RETRY),
            },
        );
        return;
    }

    let delay = BASE_DELAY_MS * 2u64.pow(attempt.saturating_sub(1)).min(64);
    log::info!("将在 {}ms 后第 {} 次重连", delay, attempt);
    emit_status(app, ConnectionStatus::Reconnecting { attempt });

    // 延迟在独立任务中等待，避免阻塞
    let app_clone = app.clone();
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(Duration::from_millis(delay)).await;

        // 等待期间用户可能已停止
        if matches!(read_connection(&app_clone), ConnectionStatus::Stopped) {
            return;
        }

        let config = match load_active_config(&app_clone) {
            Some(c) => c,
            None => {
                emit_status(
                    &app_clone,
                    ConnectionStatus::Error {
                        message: "无可用配置".to_string(),
                    },
                );
                return;
            }
        };

        if let Err(e) = start_vnt(app_clone, config) {
            log::error!("重连失败: {}", e);
        }
    });
}

/// 读取当前连接状态
fn read_connection(app: &AppHandle) -> ConnectionStatus {
    let state: State<'_, AppState> = app.state();
    let current = state.connection.read().clone();
    current
}

/// 读取当前活动配置（重连/托盘/自启共用）
pub(crate) fn load_active_config(app: &AppHandle) -> Option<VntConfig> {
    let _ = app;
    crate::config::load_config_store().get_active().cloned()
}

/// 广播连接状态：更新 state + 托盘 + 前端事件
pub(crate) fn emit_status(app: &AppHandle, status: ConnectionStatus) {
    {
        let state: State<'_, AppState> = app.state();
        *state.connection.write() = status.clone();
    }
    tray::update_tray_status(app, &status);
    let _ = app.emit("status-change", &status);
}

/// 写入日志缓冲并转发前端
fn push_log(app: &AppHandle, level: LogLevel, message: String) {
    let state: State<'_, AppState> = app.state();
    let entry = LogEntry {
        timestamp: chrono::Utc::now().to_rfc3339(),
        level: level.clone(),
        message: message.clone(),
    };
    state.log_buffer.push(entry.clone());
    let _ = app.emit("log-line", &entry);
}

/// 从输出行提取延迟毫秒数（匹配 "latency: 12ms"、"延迟 12 ms"、"12ms"）
fn extract_latency(line: &str) -> Option<u64> {
    let lower = line.to_lowercase();
    // 优先在 latency / 延迟 关键字之后查找，避免误匹配其他数字
    let search_in = match lower.find("latency").or_else(|| lower.find("延迟")) {
        Some(i) => &lower[i..],
        None => lower.as_str(),
    };
    let bytes = search_in.as_bytes();
    let mut i = 0;
    while i + 1 < bytes.len() {
        // 定位 "ms" 位置
        if bytes[i] == b'm' && (bytes[i + 1] == b's' || bytes[i + 1] == b'S') {
            // 向前回溯数字（允许空格分隔，如 "12 ms"）
            let mut j = i;
            while j > 0 && (bytes[j - 1] == b' ' || bytes[j - 1] == b'\t') {
                j -= 1;
            }
            let end = j;
            while j > 0 && bytes[j - 1].is_ascii_digit() {
                j -= 1;
            }
            if j < end {
                let num = &search_in[j..end];
                if let Ok(ms) = num.parse::<u64>() {
                    return Some(ms);
                }
            }
            return None;
        }
        i += 1;
    }
    None
}

/// 解析 "register ip=10.26.0.3 ,netmask=..." 中的虚拟 IP
fn parse_virtual_ip(line: &str) -> Option<String> {
    let idx = line.find("ip=")?;
    let rest = &line[idx + 3..];
    let ip = rest
        .split(|c: char| c == ',' || c == ' ' || c == '\t')
        .next()?;
    if ip.contains('.') && !ip.is_empty() {
        Some(ip.to_string())
    } else {
        None
    }
}

/// 从错误行中提取用户可读信息（截断保护）
fn extract_error_message(line: &str) -> String {
    let msg = line.trim();
    if msg.chars().count() > 160 {
        let truncated: String = msg.chars().take(160).collect();
        format!("{}...", truncated)
    } else {
        msg.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn latency_parses_common_formats() {
        assert_eq!(extract_latency("latency: 12ms"), Some(12));
        assert_eq!(extract_latency("latency 12 ms"), Some(12));
        assert_eq!(extract_latency("延迟：8ms"), Some(8));
        assert_eq!(extract_latency("ping 25ms"), Some(25));
        assert_eq!(extract_latency("latency:120ms"), Some(120));
    }

    #[test]
    fn latency_rejects_non_latency_lines() {
        assert_eq!(extract_latency("connect count: 3"), None);
        assert_eq!(extract_latency("handshake with server success"), None);
        assert_eq!(extract_latency(""), None);
        assert_eq!(extract_latency("register ip=10.26.0.3 ,netmask=255.255.255.0"), None);
    }

    #[test]
    fn build_args_maps_config() {
        let cfg = crate::config::VntConfig {
            id: "1".into(),
            name: "t".into(),
            token: "tok".into(),
            device_name: Some("pc".into()),
            device_id: None,
            virtual_ip: Some("10.26.0.9".into()),
            server_address: Some("vnt.example.com:29871".into()),
            password: Some("pwd".into()),
            server_encrypt: true,
            in_ips: vec!["192.168.1.0/24".into()],
            out_ips: vec![],
            compressor: Some("lz4".into()),
            mtu: Some(1400),
            use_tcp: true,
            use_ws: false,
            no_proxy: true,
            created_at: String::new(),
            updated_at: String::new(),
            last_used: None,
        };
        let args = build_args(&cfg);
        assert!(args.contains(&"-k".to_string()));
        assert!(args.contains(&"tok".to_string()));
        // tcp 协议应转成 -s 地址前缀
        let s_idx = args.iter().position(|a| a == "-s").unwrap();
        assert_eq!(args[s_idx + 1], "tcp://vnt.example.com:29871");
        assert!(args.contains(&"-u".to_string()));
        assert!(args.contains(&"1400".to_string()));
        assert!(!args.contains(&"-t".to_string()));
    }
}
