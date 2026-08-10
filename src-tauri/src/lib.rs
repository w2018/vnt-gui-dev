//! VNT GUI —— Tauri 入口、插件注册与全部命令（文档 §3.10 / §3.11）

mod autostart;
pub mod config;
pub mod daemon;
pub mod ftp;
mod logger;
mod settings;
mod state;
mod traffic;
mod tray;
mod updater;

use std::path::PathBuf;
use std::time::Duration;

use tauri::{Emitter, Manager, State};
use tauri_plugin_autostart::ManagerExt;
use tauri_plugin_notification::NotificationExt;

use config::{ConfigStore, VntConfig};
use settings::AppSettings;
use state::{AppState, ConnectionStatus, LogEntry, TrafficSnapshot};
use updater::UpdateInfo;

// ==================== 连接控制（经 daemon RPC） ====================

/// 启动连接（使用指定配置）：GUI 只传配置，进程由 daemon 管理
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
        *state.active_config_id.write() = Some(config_id.clone());
    }

    crate::daemon::rpc_client::vnt_start(cfg).await?;
    // 即时更新 GUI 状态（轮询兜底）
    sync_status_from_daemon(&app).await;
    Ok(())
}

/// 停止连接
#[tauri::command]
async fn stop_connection(app: tauri::AppHandle) -> Result<(), String> {
    crate::daemon::rpc_client::vnt_stop().await?;
    sync_status_from_daemon(&app).await;
    Ok(())
}

/// 获取当前连接状态（daemon 状态映射）
#[tauri::command]
async fn get_status(app: tauri::AppHandle) -> ConnectionStatus {
    match crate::daemon::rpc_client::get_state().await {
        Ok(crate::daemon::rpc_protocol::DaemonResponse::State {
            vnt_running,
            vnt_connected,
            ..
        }) => {
            if vnt_connected {
                ConnectionStatus::Connected
            } else if vnt_running {
                ConnectionStatus::Starting
            } else {
                ConnectionStatus::Stopped
            }
        }
        _ => {
            // daemon 不可达（未启动/异常）→ 保持 GUI 缓存
            let state: State<'_, AppState> = app.state();
            let status = state.connection.read().clone();
            status
        }
    }
}

/// 从 daemon 同步状态到 GUI 内存（AppState + 托盘）
async fn sync_status_from_daemon(app: &tauri::AppHandle) {
    let status = get_status(app.clone()).await;
    {
        let state: State<'_, AppState> = app.state();
        *state.connection.write() = status.clone();
    }
    let _ = tray::update_tray_status(app, &status);
}

/// 从 daemon 同步运行信息（虚拟 IP / 真实服务器 / NAT）到 GUI 内存 + 前端事件
async fn sync_daemon_info(app: &tauri::AppHandle) {    use crate::daemon::rpc_protocol::DaemonResponse;
    match crate::daemon::rpc_client::get_state().await {
        Ok(DaemonResponse::State {
            vnt_virtual_ip,
            vnt_server_host,
            vnt_nat_type,
            ..
        }) => {
            {
                let state: State<'_, AppState> = app.state();
                // 虚拟 IP：托盘状态行 + 前端连接信息
                let mut ip = state.virtual_ip.lock();
                if *ip != vnt_virtual_ip {
                    *ip = vnt_virtual_ip.clone();
                    if let Some(ip) = &vnt_virtual_ip {
                        let _ = app.emit("virtual-ip-assigned", ip);
                    }
                }
                // 真实服务器（连接信息展示）
                let mut host = state.server_host.lock();
                if *host != vnt_server_host {
                    *host = vnt_server_host.clone();
                    if let Some(h) = &vnt_server_host {
                        let _ = app.emit("server-address", h);
                    }
                }
                *state.relay_addr.lock() = vnt_server_host.clone();
                *state.nat_type.lock() = vnt_nat_type;
            }
        }
        _ => {}
    }
}

/// 连接信息（前端挂载时主动拉取：虚拟 IP / 真实服务器）
/// 解决 GUI 重启后事件先于前端监听触发导致的信息丢失（兜底主动拉取）
#[derive(serde::Serialize)]
struct ConnectionInfo {
    virtual_ip: Option<String>,
    server_address: Option<String>,
}

/// 获取连接信息（虚拟 IP / 真实服务器）：daemon 状态优先，不可达时返回 GUI 缓存
#[tauri::command]
async fn get_connection_info(app: tauri::AppHandle) -> ConnectionInfo {
    use crate::daemon::rpc_protocol::DaemonResponse;
    match crate::daemon::rpc_client::get_state().await {
        Ok(DaemonResponse::State {
            vnt_virtual_ip,
            vnt_server_host,
            ..
        }) => ConnectionInfo {
            virtual_ip: vnt_virtual_ip,
            server_address: vnt_server_host,
        },
        _ => {
            // daemon 不可达 → GUI 缓存（sync_daemon_info 已写入）
            let state: State<'_, AppState> = app.state();
            let virtual_ip = state.virtual_ip.lock().clone();
            let server_address = state.server_host.lock().clone();
            ConnectionInfo {
                virtual_ip,
                server_address,
            }
        }
    }
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

/// 获取 ping 目标 host：优先活动配置的服务器地址，其次 daemon 实际连接服务器
#[tauri::command]
async fn get_ping_host(_app: tauri::AppHandle) -> Option<String> {
    // 1. 活动配置 server_address（去协议前缀/端口）
    if let Some(cfg) = config::load_config_store().get_active() {
        if let Some(server) = &cfg.server_address {
            if let Some(host) = extract_host(server) {
                return Some(host);
            }
        }
    }
    // 2. daemon 实际连接服务器（如 "8.134.66.150:29872" → 纯 host）
    if let Ok(crate::daemon::rpc_protocol::DaemonResponse::State {
        vnt_server_host,
        ..
    }) = crate::daemon::rpc_client::get_state().await
    {
        if let Some(addr) = vnt_server_host {
            if let Some(host) = extract_host(&addr) {
                return Some(host);
            }
        }
    }
    None
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

/// 获取历史日志（GUI 进程日志：日志页历史加载）
#[tauri::command]
fn get_logs(_app: tauri::AppHandle) -> Vec<LogEntry> {
    crate::logger::get_global_logs()
}

/// 清空日志
#[tauri::command]
fn clear_logs(_app: tauri::AppHandle) -> Result<(), String> {
    crate::logger::clear_global_logs();
    Ok(())
}

/// 导出日志到文件
#[tauri::command]
fn export_logs(_app: tauri::AppHandle, path: String) -> Result<(), String> {
    let logs = crate::logger::get_global_logs();
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

/// 获取 daemon 运行日志（VNT 实时日志，经 RPC；daemon 不可达返回空）
#[tauri::command]
async fn vnt_get_logs() -> Vec<crate::state::LogEntry> {
    match crate::daemon::rpc_client::vnt_get_logs().await {
        Ok(entries) => entries,
        Err(e) => {
            log::warn!("获取 daemon 日志失败: {}", e);
            Vec::new()
        }
    }
}

/// 清空 daemon 运行日志
#[tauri::command]
async fn vnt_clear_logs() -> Result<(), String> {
    crate::daemon::rpc_client::vnt_clear_logs().await
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

/// 获取在线设备列表（数据来自 daemon 定期 --list 解析；本机识别用 daemon 虚拟 IP）
/// 返回：过滤本机后的设备列表 + 本机设备信息
#[tauri::command]
async fn get_device_list(app: tauri::AppHandle) -> Result<state::DeviceListResult, String> {
    use crate::daemon::rpc_protocol::DaemonResponse;

    // daemon 状态：peers + 本机虚拟 IP
    let (peers, local_ip) = match crate::daemon::rpc_client::get_state().await {
        Ok(DaemonResponse::State {
            peers, vnt_virtual_ip, ..
        }) => (peers, vnt_virtual_ip),
        Ok(other) => return Err(format!("daemon 响应异常: {:?}", other)),
        Err(e) => return Err(format!("获取设备列表失败: {}", e)),
    };

    // 本机名：配置设备名 / 主机名
    let local_name = config::load_config_store()
        .get_active()
        .and_then(|c| c.device_name.clone())
        .filter(|n| !n.is_empty())
        .or_else(|| {
            let h = std::env::var("COMPUTERNAME").unwrap_or_default();
            if h.is_empty() { None } else { Some(h) }
        });

    // 过滤本机：IP 匹配 或 设备名匹配（覆盖残留注册：同名不同 IP 的历史行也过滤）
    let mut devices = Vec::with_capacity(peers.len());
    let mut local = None;
    for peer in peers {
        let is_local = match &local_ip {
            Some(ip) => peer.virtual_ip == *ip
                || local_name
                    .as_ref()
                    .is_some_and(|n| peer.name.eq_ignore_ascii_case(n)),
            None => local_name
                .as_ref()
                .is_some_and(|n| peer.name.eq_ignore_ascii_case(n)),
        };
        if is_local {
            local = Some(peer);
            continue;
        }
        devices.push(peer);
    }
    let _ = app;
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
/// relay_server = 完整地址 "IP:端口"（展示用）；ping 目标仍走 get_ping_host（纯 host，互不影响）
#[tauri::command]
async fn get_local_info(app: tauri::AppHandle) -> LocalInfo {
    use crate::daemon::rpc_protocol::DaemonResponse;
    match crate::daemon::rpc_client::get_state().await {
        Ok(DaemonResponse::State {
            vnt_nat_type,
            vnt_server_host,
            ..
        }) => LocalInfo {
            nat_type: vnt_nat_type,
            relay_server: vnt_server_host,
        },
        _ => {
            // daemon 不可达 → 保持上次本地缓存
            let state: State<'_, AppState> = app.state();
            let nat = state.nat_type.lock().clone();
            let relay = state.relay_addr.lock().clone();
            LocalInfo {
                nat_type: nat,
                relay_server: relay,
            }
        }
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

/// 启动 daemon（独立进程，脱离 tauri Job Object，GUI 退出不影响其存活）
/// - 已存活（pid 文件 + 进程探测）→ 直接复用
/// - 未存活 → 从 resource_dir 直接 spawn vnt-daemon.exe
async fn start_daemon_sidecar(app: &tauri::AppHandle) -> Result<(), String> {
    use tauri::Manager;

    // 1. 已有 daemon 存活则复用（pid 文件优先）
    if crate::daemon::pid_file::is_daemon_running() {
        log::info!("daemon 已在运行，复用");
        return Ok(());
    }

    // 2. 定位 daemon 可执行文件（bundle 后与主程序同目录；dev 在 target/debug）
    let exe_path = app
        .path()
        .resource_dir()
        .map_err(|e| format!("定位资源目录失败: {}", e))?
        .join("vnt-daemon.exe");
    if !exe_path.exists() {
        return Err(format!("daemon 可执行文件不存在: {}", exe_path.display()));
    }

    // 3. 直接 spawn（std::process：无 Job Object，GUI 退出 daemon 继续运行）
    let child = std::process::Command::new(&exe_path)
        .spawn()
        .map_err(|e| format!("daemon 启动失败 ({}): {}", exe_path.display(), e))?;
    log::info!("daemon 已启动（pid={}）", child.id());
    Ok(())
}

// ==================== 应用入口 ====================

/// 应用入口
/// `autostart`: 开机自启模式（--autostart）——不显示主窗口，daemon 恢复服务后最小化到托盘
#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run(autostart: bool) {
    // 全局日志：内存缓冲 + logs/app.log + 前端 log-line 事件
    crate::logger::init(crate::config::log_dir().join("app.log"));

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
        .setup(move |app| {
            // 日志器注入 AppHandle（emit log-line 事件）
            crate::logger::attach(app.handle().clone());
            // 旧版数据迁移（%APPDATA%\vnt-gui → data\；安装目录根残留 → data\ + logs\）
            config::migrate_legacy_data();
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

            // Bug 4：开机自启模式（--autostart）——不显示主窗口（静默后台运行）
            if autostart {
                for window in app.webview_windows().values() {
                    let _ = window.hide();
                }
                log::info!("开机自启模式：主窗口已隐藏，daemon 将按持久化状态恢复服务");
            }

            // 流量监控（每秒采集虚拟网卡统计）
            traffic::start_traffic_monitor(app.handle().clone());

            // 自启参数：按设置隐藏托盘（静默后台运行）
            if autostart && settings::load_settings().hide_tray_on_autostart {
                if let Some(tray) = app.tray_by_id(tray::TRAY_ID) {
                    let _ = tray.set_visible(false);
                    log::info!("开机自启：按设置隐藏托盘");
                }
            }

            // Daemon 生命周期：检测 → 启动（sidecar）→ 状态轮询
            {
                let handle = app.handle().clone();
                tauri::async_runtime::spawn(async move {
                    // 1. daemon 未运行 → 启动
                    if !crate::daemon::pid_file::is_daemon_running() {
                        match start_daemon_sidecar(&handle).await {
                            Ok(()) => log::info!("daemon sidecar 已启动"),
                            Err(e) => log::error!("启动 daemon 失败: {}", e),
                        }
                    } else {
                        log::info!("daemon 已在运行（PID 存活）");
                    }
                    // 2. 等待 RPC 就绪（最多 5 秒）
                    let mut ready = false;
                    for _ in 0..50 {
                        if crate::daemon::rpc_client::ping().await.is_ok() {
                            ready = true;
                            break;
                        }
                        tokio::time::sleep(Duration::from_millis(100)).await;
                    }
                    if ready {
                        log::info!("daemon RPC 就绪");
                        // 3. 初始同步：状态 → GUI + 托盘 + 运行信息（虚拟 IP/服务器/NAT）
                        let status = get_status(handle.clone()).await;
                        let state: State<'_, AppState> = handle.state();
                        *state.connection.write() = status.clone();
                        let _ = tray::update_tray_status(&handle, &status);
                        sync_daemon_info(&handle).await;
                        // 4. 状态轮询（3 秒）：daemon 状态 → GUI 内存 + 托盘
                        loop {
                            tokio::time::sleep(Duration::from_secs(3)).await;
                            let status = get_status(handle.clone()).await;
                            let state: State<'_, AppState> = handle.state();
                            *state.connection.write() = status.clone();
                            let _ = tray::update_tray_status(&handle, &status);
                            sync_daemon_info(&handle).await;
                        }
                    } else {
                        log::error!("daemon RPC 未就绪（5 秒超时）");
                    }
                });
            }

            // F2：FTP 随应用启动（打开 VNT GUI 即自动启动 FTP 服务）—— 经 daemon
            {
                let config_dir = config::get_config_path()
                    .parent()
                    .unwrap_or(&PathBuf::from("."))
                    .to_path_buf();
                let ftp_cfg = ftp::config::load_ftp_config(&config_dir);
                if ftp_cfg.auto_start_with_app && ftp_cfg.enabled {
                    let handle = app.handle().clone();
                    tauri::async_runtime::spawn(async move {
                        tokio::time::sleep(Duration::from_secs(4)).await;
                        let rpc_cfg = {
                            let state: State<'_, AppState> = handle.state();
                            let mut c = ftp::config::load_ftp_config(&state.config_dir);
                            for user in &mut c.users {
                                if user.password.is_empty() {
                                    match ftp::config::get_password(&user.username) {
                                        Some(pwd) => user.password = pwd,
                                        None => log::warn!(
                                            "FTP 自启：用户 {} 密码在凭据库中不存在（keyring 读取失败）",
                                            user.username
                                        ),
                                    }
                                }
                            }
                            ftp::to_rpc_cfg(&c)
                        };
                        match rpc_cfg {
                            Ok(cfg) => {
                                if let Err(e) = crate::daemon::rpc_client::ftp_start(cfg).await {
                                    log::error!("FTP 随应用自启失败: {}", e);
                                }
                            }
                            Err(e) => log::error!("FTP 随应用自启失败: {}", e),
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
            get_connection_info,
            get_logs,
            clear_logs,
            export_logs,
            vnt_get_logs,
            vnt_clear_logs,
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
            ftp::ftp_get_listen_addresses,
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
