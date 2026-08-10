//! VNT GUI —— Tauri 入口、插件注册与全部命令（文档 §3.10 / §3.11）

mod autostart;
mod config;
pub mod ftp;
mod logger;
mod settings;
mod sidecar;
mod state;
mod traffic;
mod tray;
mod updater;

use std::path::PathBuf;
use std::time::Duration;

use tauri::{Manager, State};
use tauri_plugin_autostart::ManagerExt;
use tauri_plugin_notification::NotificationExt;

use config::{ConfigStore, VntConfig};
use settings::AppSettings;
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

/// 导出全部配置到 JSON 文件（后端直接写文件，不受 fs 插件路径 scope 限制）
#[tauri::command]
fn export_configs(path: String) -> Result<(), String> {
    let store = config::load_config_store();
    let json = serde_json::to_string_pretty(&store).map_err(|e| format!("序列化失败: {}", e))?;
    std::fs::write(&path, json).map_err(|e| format!("写入失败: {}", e))?;
    Ok(())
}

/// 从 JSON 文件读取配置列表（由前端决定逐条保存，避免覆盖现有配置）
#[tauri::command]
fn import_configs(path: String) -> Result<Vec<VntConfig>, String> {
    let content = std::fs::read_to_string(&path).map_err(|e| format!("读取失败: {}", e))?;
    let store: ConfigStore =
        serde_json::from_str(&content).map_err(|e| format!("解析失败: {}", e))?;
    Ok(store.configs)
}

// ==================== 网络检测 ====================

/// 获取 ping 目标 host：优先活动配置的服务器地址，其次 vnt-cli 日志提取的实际连接服务器
#[tauri::command]
fn get_ping_host(app: tauri::AppHandle) -> Option<String> {
    // 1. 活动配置 server_address（去协议前缀/端口）
    if let Some(cfg) = config::load_config_store().get_active() {
        if let Some(server) = &cfg.server_address {
            if let Some(host) = extract_host(server) {
                return Some(host);
            }
        }
    }
    // 2. 日志提取的实际连接服务器（如 "8.134.66.150"）
    let state: State<'_, AppState> = app.state();
    let host = state.server_host.lock().clone();
    host
}

/// 从服务器地址提取纯 host（去 "tcp://" 前缀与端口；IPv6 不做端口剥离）
fn extract_host(server: &str) -> Option<String> {
    let mut s = server.trim();
    if let Some(i) = s.find("://") {
        s = &s[i + 3..];
    }
    // 去端口：仅当最后一个冒号后全为数字
    if let Some(i) = s.rfind(':') {
        let port = &s[i + 1..];
        if !port.is_empty() && port.chars().all(|c| c.is_ascii_digit()) {
            s = &s[..i];
        }
    }
    if s.is_empty() {
        None
    } else {
        Some(s.to_string())
    }
}

/// Ping 主机（surge-ping 0.9：异步 ICMP，Windows 走系统 IcmpSendEcho 无需提权；
/// 域名走 tokio 异步 DNS；超时 2s 由 Pinger::timeout 内置控制）
#[tauri::command]
async fn ping_host(host: String) -> Result<u64, String> {
    let ip = resolve_host(&host).await?;
    let (ms, _size) = ping_impl(ip).await?;
    Ok(ms)
}

/// 最小化 ping 测试命令：返回 "host (ip) => size bytes, time=xx.xxms" 格式
/// 用于快速验证网络连通性 + surge-ping 是否正常工作
#[tauri::command]
async fn ping_test(host: String) -> Result<String, String> {
    let host = host.trim().to_string();
    log::info!("ping_test: 解析 {}", host);
    let ip = resolve_host(&host).await?;
    log::info!("ping_test: 解析到 {:?}", ip);
    let (ms, size) = ping_impl(ip).await?;
    Ok(format!(
        "{} ({}) => {} bytes, time={:.2}ms",
        host, ip, size, ms as f64
    ))
}

/// 主机名/IP → IpAddr（优先 IPv4：避免 Windows ICMPv6 被防火墙拦截）
async fn resolve_host(host: &str) -> Result<std::net::IpAddr, String> {
    use std::net::IpAddr;
    let host = host.trim();
    if host.is_empty() {
        return Err("主机地址为空".to_string());
    }
    // IP 字面量直接使用
    if let Ok(ip) = host.parse::<IpAddr>() {
        return Ok(ip);
    }
    // 域名解析：收集全部地址，优先 IPv4，兜底 IPv6
    let addrs = tokio::net::lookup_host((host, 0))
        .await
        .map_err(|e| format!("DNS 解析失败: {}", e))?;
    let mut ipv4: Option<IpAddr> = None;
    let mut ipv6: Option<IpAddr> = None;
    for addr in addrs {
        match addr.ip() {
            IpAddr::V4(v4) => {
                if ipv4.is_none() {
                    ipv4 = Some(IpAddr::V4(v4));
                }
            }
            IpAddr::V6(v6) => {
                if ipv6.is_none() {
                    ipv6 = Some(IpAddr::V6(v6));
                }
            }
        }
    }
    ipv4.or(ipv6).ok_or_else(|| "DNS 无解析结果".to_string())
}

/// 核心 ping 实现（可单测）：返回 (毫秒, 发送字节数)
async fn ping_impl(ip: std::net::IpAddr) -> Result<(u64, usize), String> {
    let client = surge_ping::Client::new(&surge_ping::Config::default())
        .map_err(|e| format!("ping 初始化失败: {:?}", e))?;
    let mut pinger = client
        .pinger(ip, surge_ping::PingIdentifier(0x1234))
        .await;
    // 内置超时 2s
    pinger.timeout(std::time::Duration::from_secs(2));
    // 32 字节 payload（过小包可能被部分网络设备丢弃）
    let payload = [0u8; 32];
    let size = payload.len();

    match pinger.ping(surge_ping::PingSequence(0), &payload).await {
        Ok((_packet, rtt)) => Ok((rtt.as_millis() as u64, size)),
        Err(surge_ping::SurgeError::Timeout { .. }) => Err("ping 超时".to_string()),
        Err(e) => Err(format!("ping 失败: {:?}", e)),
    }
}

#[cfg(test)]
mod ping_tests {
    use super::extract_host;
    use super::parse_info;
    use super::parse_list_line;
    use super::parse_nat_type;
    use super::parse_relay_addr;
    use super::ping_impl;

    #[test]
    fn list_line_parses_real_output() {
        // 本机实测 vnt-cli --list -k <token> 输出（真实格式，含空格设备名）
        let table_header = "Name                Virtual Ip    Status     P2P/Relay       Rt";
        assert!(parse_list_line(table_header).is_none());
        // 离线行
        let offline = parse_list_line("Z                   10.26.0.2     Offline").unwrap();
        assert_eq!(offline.name, "Z");
        assert_eq!(offline.virtual_ip, "10.26.0.2");
        assert_eq!(offline.status, "offline");
        // 在线行：设备名含空格 + server-relay + Rt 列 148
        let online = parse_list_line("RNA-AL00 4.2.0.1    10.26.0.4     Online     server-relay    148").unwrap();
        assert_eq!(online.name, "RNA-AL00 4.2.0.1");
        assert_eq!(online.virtual_ip, "10.26.0.4");
        assert_eq!(online.status, "online");
        assert_eq!(online.connection_type, "relay");
        assert_eq!(online.latency, 148);
        // 在线行 P2P
        let p2p = parse_list_line("Phone   10.26.0.9    Online    P2P    12").unwrap();
        assert_eq!(p2p.connection_type, "p2p");
        assert_eq!(p2p.latency, 12);
        assert!(parse_list_line("").is_none());
    }

    #[test]
    fn info_parses_real_output() {
        // 用户实测 vnt --info 输出
        let (name, ip) = parse_info("Name: Z\nVirtual ip: 10.26.0.3");
        assert_eq!(name.as_deref(), Some("Z"));
        assert_eq!(ip.as_deref(), Some("10.26.0.3"));
        // 失败输出（无后台）不产生脏数据
        let (n, i) = parse_info("Os { code: 10054, kind: ConnectionReset }");
        assert!(n.is_none() && i.is_none());
    }

    #[test]
    fn info_extras_parse() {
        let text = "Name: Z\nVirtual ip: 10.26.0.3\nNAT type: Cone\nRelay server: 8.134.66.150:29872";
        // 完整地址（IP:端口，展示用）
        assert_eq!(parse_relay_addr(text).as_deref(), Some("8.134.66.150:29872"));
        // 从完整地址提取纯 host（ping 用，互不影响）
        assert_eq!(extract_host(&parse_relay_addr(text).unwrap()).as_deref(), Some("8.134.66.150"));
        assert_eq!(parse_nat_type(text).as_deref(), Some("Cone"));
        assert_eq!(parse_relay_addr("no relay here"), None);
        assert_eq!(parse_nat_type("NAT type:"), None);
    }

    #[tokio::test]
    async fn ping_loopback_succeeds() {
        let r = ping_impl("127.0.0.1".parse().unwrap()).await;
        assert!(r.is_ok(), "loopback ping 应成功，实际: {:?}", r);
    }

    #[tokio::test]
    async fn ping_unreachable_fails() {
        // 192.0.2.0/24 为保留测试网段，必然不可达（2s 内置超时）
        let r = ping_impl("192.0.2.1".parse().unwrap()).await;
        assert!(r.is_err(), "不可达主机应返回 Err，实际: {:?}", r);
    }

    #[tokio::test]
    async fn ping_domain_resolves_and_succeeds() {
        // 域名解析 + 外网可达性（依赖本机网络）
        let ip = super::resolve_host("www.baidu.com").await;
        assert!(ip.is_ok(), "baidu.com 应可解析，实际: {:?}", ip);
        let r = ping_impl(ip.unwrap()).await;
        assert!(r.is_ok(), "baidu.com 应 ping 通，实际: {:?}", r);
    }
}

// ==================== 应用设置 ====================

/// 获取应用行为设置（托盘可见性等）
#[tauri::command]
fn get_settings() -> AppSettings {
    settings::load_settings()
}

/// 保存应用行为设置
#[tauri::command]
fn save_settings(settings: AppSettings) -> Result<(), String> {
    settings::save_settings(&settings)
}

/// 实时显示/隐藏系统托盘图标（设置开关即时生效）
#[tauri::command]
fn set_tray_visible(app: tauri::AppHandle, visible: bool) -> Result<(), String> {
    if let Some(tray) = app.tray_by_id(crate::tray::TRAY_ID) {
        tray.set_visible(visible)
            .map_err(|e| format!("设置托盘可见性失败: {}", e))?;
        log::info!("托盘图标已{}", if visible { "显示" } else { "隐藏" });
        Ok(())
    } else {
        Err("托盘未创建".to_string())
    }
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
/// 关键：必须携带活动配置的 token/server（-k/-s），否则查询的是默认空 token 组（别人的设备）
/// 本机识别：优先 `--info`（Name/Virtual ip），失败降级 register 日志/配置设备名/主机名
/// 返回：过滤本机后的设备列表 + 本机设备信息
#[tauri::command]
async fn get_device_list(app: tauri::AppHandle) -> Result<state::DeviceListResult, String> {
    use tauri_plugin_shell::ShellExt;

    // 活动配置参数（--list / --info 共用）
    let store = config::load_config_store();
    let active = store.get_active();
    let mut net_args: Vec<String> = Vec::new();
    if let Some(cfg) = &active {
        if !cfg.token.is_empty() {
            net_args.push("-k".to_string());
            net_args.push(cfg.token.clone());
        }
        if let Some(server) = &cfg.server_address {
            net_args.push("-s".to_string());
            net_args.push(server.clone());
        }
    }

    // 1. 本机信息：优先 --info（需后台 vnt-cli 运行；失败自然降级）
    let mut local_name: Option<String> = None;
    let mut local_ip: Option<String> = None;
    if let Ok(info_output) = app
        .shell()
        .sidecar("vnt-cli")
        .map_err(|e| format!("sidecar 不可用: {}", e))?
        .args({
            let mut a = vec!["--info".to_string()];
            a.extend(net_args.iter().cloned());
            a
        })
        .output()
        .await
    {
        let stdout = String::from_utf8_lossy(&info_output.stdout);
        let stderr = String::from_utf8_lossy(&info_output.stderr);
        let text = if !stdout.trim().is_empty() { &stdout } else { &stderr };
        let (n, i) = parse_info(text);
        local_name = n;
        local_ip = i;
        // 真实连接服务器（Relay server: 8.134.66.150:29872）→ 完整地址展示 + 纯 host ping
        if let Some(addr) = parse_relay_addr(text) {
            let state: tauri::State<'_, AppState> = app.state();
            *state.relay_addr.lock() = Some(addr.clone());
            if let Some(host) = extract_host(&addr) {
                *state.server_host.lock() = Some(host);
            }
        }
        // NAT 类型（NAT type: Cone）
        if let Some(nat) = parse_nat_type(text) {
            let state: tauri::State<'_, AppState> = app.state();
            *state.nat_type.lock() = Some(nat);
        }
    }
    // 降级：register 日志解析的本机 IP + 配置设备名/主机名
    if local_name.is_none() {
        local_name = active
            .as_ref()
            .and_then(|c| c.device_name.clone())
            .filter(|n| !n.is_empty())
            .or_else(|| {
                let h = std::env::var("COMPUTERNAME").unwrap_or_default();
                if h.is_empty() { None } else { Some(h) }
            });
    }
    if local_ip.is_none() {
        let state: tauri::State<'_, AppState> = app.state();
        local_ip = state.virtual_ip.lock().clone();
    }

    // 2. --list 解析设备
    let list_args = {
        let mut a = vec!["--list".to_string()];
        a.extend(net_args.iter().cloned());
        a
    };
    let output = app
        .shell()
        .sidecar("vnt-cli")
        .map_err(|e| format!("sidecar 不可用: {}", e))?
        .args(&list_args)
        .output()
        .await
        .map_err(|e| format!("执行 vnt-cli --list 失败: {}", e))?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let text = if !stdout.trim().is_empty() { &stdout } else { &stderr };

    // 先尝试 JSON 解析，失败则按文本行解析（真实格式见 parse_list_line）
    let peers = if let Ok(peers) = serde_json::from_str::<Vec<state::PeerInfo>>(text) {
        peers
    } else {
        let mut peers = Vec::new();
        for line in text.lines() {
            if let Some(peer) = parse_list_line(line) {
                peers.push(peer);
            }
        }
        peers
    };

    // 3. 过滤本机：IP 匹配 或 设备名匹配（覆盖残留注册：同名不同 IP 的历史行也过滤）
    let mut devices = Vec::with_capacity(peers.len());
    let mut local = None;
    for peer in peers {
        let is_local = match &local_ip {
            Some(ip) => peer.virtual_ip == *ip || match &local_name {
                Some(n) => peer.name.eq_ignore_ascii_case(n),
                None => false,
            },
            None => match &local_name {
                Some(n) => peer.name.eq_ignore_ascii_case(n),
                None => false,
            },
        };
        if is_local {
            local = Some(peer);
            continue;
        }
        devices.push(peer);
    }
    Ok(state::DeviceListResult { devices, local })
}

/// 解析 vnt --info 输出（Name: Z / Virtual ip: 10.26.0.3）
fn parse_info(text: &str) -> (Option<String>, Option<String>) {
    let mut name = None;
    let mut ip = None;
    for line in text.lines() {
        let line = line.trim();
        let lower = line.to_lowercase();
        if lower.starts_with("name:") {
            name = Some(line["name:".len()..].trim().to_string());
        } else if lower.starts_with("virtual ip:") {
            ip = Some(line["virtual ip:".len()..].trim().to_string());
        }
    }
    (name, ip)
}

/// 解析 --info 的 Relay server 行（"Relay server: 8.134.66.150:29872"）→ 完整地址
fn parse_relay_addr(text: &str) -> Option<String> {
    for line in text.lines() {
        let line = line.trim();
        if line.to_lowercase().starts_with("relay server:") {
            let addr = line["relay server:".len()..].trim();
            if !addr.is_empty() {
                return Some(addr.to_string());
            }
        }
    }
    None
}

/// 解析 --info 的 NAT 类型行（"NAT type: Cone"）
fn parse_nat_type(text: &str) -> Option<String> {
    for line in text.lines() {
        let line = line.trim();
        if line.to_lowercase().starts_with("nat type:") {
            let nat = line["nat type:".len()..].trim();
            if !nat.is_empty() {
                return Some(nat.to_string());
            }
        }
    }
    None
}

/// 本机增强信息（连接信息展示用）：NAT 类型 + 真实连接服务器（完整地址 IP:端口）
#[derive(Debug, Clone, serde::Serialize)]
pub struct LocalInfo {
    pub nat_type: Option<String>,
    pub relay_server: Option<String>,
}

/// 获取本机 NAT 类型与真实连接服务器（来自 --info 解析，连接后有效）
/// relay_server = 完整地址 "IP:端口"（展示用）；ping 目标仍走 get_ping_host（纯 host，互不影响）
#[tauri::command]
fn get_local_info(app: tauri::AppHandle) -> LocalInfo {
    let state: State<'_, AppState> = app.state();
    let nat_guard = state.nat_type.lock();
    let nat_type = nat_guard.clone();
    drop(nat_guard);
    let addr_guard = state.relay_addr.lock();
    let relay_server = addr_guard.clone();
    LocalInfo {
        nat_type,
        relay_server,
    }
}

/// 解析 vnt-cli --list 单行输出（真实格式，列空格对齐，设备名可含空格）：
///   Z                   10.26.0.2     Offline
///   RNA-AL00 4.2.0.1    10.26.0.4     Online     server-relay    148
fn parse_list_line(line: &str) -> Option<state::PeerInfo> {
    let line = line.trim();
    if line.is_empty() || line.to_lowercase().starts_with("name") {
        return None;
    }
    let ip = extract_ip(line)?;
    let ip_pos = line.find(&ip)?;
    // 设备名 = IP 之前的文本（含空格，去掉右侧空白与表格边框）
    let name = line[..ip_pos].trim().trim_matches('|').trim().to_string();
    let rest = &line[ip_pos + ip.len()..];
    let mut parts = rest.split_whitespace();
    let status = parts.next().unwrap_or("offline").to_string();
    let status_lower = status.to_lowercase();
    // 连接类型：在线行 P2P/Relay 列
    let connection_type = if status_lower == "online" {
        let t = parts.next().unwrap_or("p2p");
        if t.to_lowercase().contains("relay") {
            "relay".to_string()
        } else {
            "p2p".to_string()
        }
    } else {
        "p2p".to_string()
    };
    // Rt 列（服务器中继延迟，纯数字）或行内 ms 值
    let latency = parts
        .next()
        .and_then(|t| t.parse::<u64>().ok())
        .unwrap_or_else(|| extract_latency(line));
    Some(state::PeerInfo {
        name,
        virtual_ip: ip,
        connection_type,
        latency,
        status: status_lower,
    })
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

/// 获取分时间段流量统计（今日/昨日/本月/累计）
#[tauri::command]
async fn get_traffic_period(app: tauri::AppHandle) -> Result<crate::traffic::PeriodTraffic, String> {
    let state: State<'_, AppState> = app.state();
    let daily = state.traffic_daily.lock();
    Ok(daily.period())
}

// ==================== 应用入口 ====================

/// 应用入口
#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    env_logger::init();

    tauri::Builder::default()
        // 单实例：必须最先注册；重复启动时回调聚焦已有窗口（新进程自动退出）
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.show();
                let _ = window.set_focus();
            }
            let _ = app
                .notification()
                .builder()
                .title("VNT GUI")
                .body("应用已在运行中，请勿重复启动")
                .show();
        }))
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            Some(vec![autostart::AUTOSTART_FLAG]),
        ))
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_fs::init())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_clipboard_manager::init())
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
                // 1a：自启启动时按设置隐藏托盘（静默后台运行）
                if settings::load_settings().hide_tray_on_autostart {
                    if let Some(tray) = app.tray_by_id(tray::TRAY_ID) {
                        let _ = tray.set_visible(false);
                        log::info!("开机自启：按设置隐藏托盘");
                    }
                }
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

            // F2：FTP 随应用启动（打开 VNT GUI 即自动启动 FTP 服务）
            {
                let config_dir = config::get_config_path()
                    .parent()
                    .unwrap_or(&PathBuf::from("."))
                    .to_path_buf();
                let ftp_cfg = ftp::config::load_ftp_config(&config_dir);
                if ftp_cfg.auto_start_with_app && ftp_cfg.enabled {
                    let handle = app.handle().clone();
                    tauri::async_runtime::spawn(async move {
                        tokio::time::sleep(Duration::from_secs(3)).await;
                        let cfg = {
                            let state: State<'_, AppState> = handle.state();
                            let mut c = ftp::config::load_ftp_config(&state.config_dir);
                            for user in &mut c.users {
                                if user.password.is_empty() {
                                    user.password = ftp::config::get_password_hash(&user.username).unwrap_or_default();
                                }
                            }
                            c
                        };
                        if let Err(e) = ftp::server::start_ftp(cfg).await {
                            log::error!("FTP 随应用自启失败: {}", e);
                        }
                    });
                }
            }

            Ok(())
        })
        .on_window_event(|window, event| {
            // 关闭窗口 = 最小化到托盘（文档 §3.4.3）
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                let _ = window.hide();
                // 1b：后台运行时按设置隐藏托盘（无托盘入口）
                if settings::load_settings().hide_tray_on_background {
                    if let Some(tray) = window.app_handle().tray_by_id(tray::TRAY_ID) {
                        let _ = tray.set_visible(false);
                        log::info!("进入后台：按设置隐藏托盘");
                    }
                }
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
            export_configs,
            import_configs,
            get_settings,
            save_settings,
            set_tray_visible,
            ping_host,
            ping_test,
            get_ping_host,
            get_local_info,
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
            get_traffic_period,
            ftp::ftp_start,
            ftp::ftp_stop,
            ftp::ftp_status,
            ftp::ftp_get_config,
            ftp::ftp_save_config,
            ftp::ftp_pick_root_dir,
            ftp::ftp_get_logs,
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|app_handle, event| {
            if let tauri::RunEvent::Exit = event {
                // 退出前保存按天流量统计（含最后 60 秒内的增量）
                let state: State<'_, AppState> = app_handle.state();
                let dir = state.config_dir.clone();
                let daily = state.traffic_daily.lock().clone();
                daily.save(&dir);
                log::info!("退出：已保存流量统计");
            }
        });
}
