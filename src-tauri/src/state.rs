//! 全局状态定义（文档 §3.2）

use std::path::PathBuf;

use parking_lot::{Mutex, RwLock};
use tauri::menu::MenuItem;
use tauri_plugin_shell::process::CommandChild;

/// 连接状态（5 态状态机）
///
/// 序列化为 `{ "status": "...", ... }` 形式，前端通过 `payload.status` 判断。
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq)]
#[serde(tag = "status", rename_all = "camelCase")]
pub enum ConnectionStatus {
    Stopped,
    Starting,
    Connected,
    Reconnecting { attempt: u32 },
    Error { message: String },
}

impl ConnectionStatus {
    /// 是否处于"正在运行"状态（用于托盘/按钮判断）
    pub fn is_running(&self) -> bool {
        matches!(
            self,
            ConnectionStatus::Starting
                | ConnectionStatus::Connected
                | ConnectionStatus::Reconnecting { .. }
        )
    }

    /// 展示用文本
    pub fn label(&self) -> String {
        match self {
            ConnectionStatus::Stopped => "未连接".to_string(),
            ConnectionStatus::Starting => "连接中...".to_string(),
            ConnectionStatus::Connected => "已连接".to_string(),
            ConnectionStatus::Reconnecting { attempt } => {
                format!("重连中 (第 {} 次)", attempt)
            }
            ConnectionStatus::Error { message } => format!("错误: {}", message),
        }
    }
}

/// 日志级别
#[derive(Debug, Clone, serde::Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum LogLevel {
    Info,
    Warn,
    Error,
    Debug,
}

/// 单条日志记录
#[derive(Debug, Clone, serde::Serialize)]
pub struct LogEntry {
    pub timestamp: String,
    pub level: LogLevel,
    pub message: String,
}

/// 流量统计快照
#[derive(Debug, Default, Clone, serde::Serialize)]
pub struct TrafficSnapshot {
    pub upload_bytes: u64,
    pub download_bytes: u64,
    pub upload_speed: f64, // bytes/s
    pub download_speed: f64,
    pub peers: Vec<PeerTraffic>,
}

/// 单个 peer 的流量
#[derive(Debug, Clone, serde::Serialize)]
pub struct PeerTraffic {
    pub ip: String,
    pub upload_bytes: u64,
    pub download_bytes: u64,
}

/// 设备列表条目（文档 §4.2.6）
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct PeerInfo {
    pub name: String,
    pub virtual_ip: String,
    /// "p2p" | "relay" | "client-relay"
    pub connection_type: String,
    pub latency: u64,
    /// "online" | "offline"
    pub status: String,
}

/// 设备列表查询结果：过滤本机后的设备 + 本机信息（用于连接信息展示）
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct DeviceListResult {
    /// 过滤本机后的设备列表
    pub devices: Vec<PeerInfo>,
    /// 本机在组网中的设备信息（识别不到时为 None）
    pub local: Option<PeerInfo>,
}

/// 托盘动态菜单项句柄（连接状态变化时更新文本/启用状态）
#[derive(Clone)]
pub struct TrayMenuItems {
    /// 状态行（disabled，显示 状态/IP/组网编号）
    pub status: MenuItem<tauri::Wry>,
    /// 连接
    pub connect: MenuItem<tauri::Wry>,
    /// 断开
    pub disconnect: MenuItem<tauri::Wry>,
}

/// 全局应用状态（通过 tauri::State 管理）
pub struct AppState {
    /// 当前连接状态
    pub connection: RwLock<ConnectionStatus>,
    /// 当前活动配置 ID
    pub active_config_id: RwLock<Option<String>>,
    /// 当前分配的虚拟 IP（连接成功后由输出解析写入）
    pub virtual_ip: Mutex<Option<String>>,
    /// 日志提取的实际连接服务器 host（如 "8.134.66.150"，用于未配置地址时 ping）
    pub server_host: Mutex<Option<String>>,
    /// 真实连接服务器完整地址（"8.134.66.150:29872"，连接信息展示用）
    pub relay_addr: Mutex<Option<String>>,
    /// 本机 NAT 类型（--info 解析，如 "Cone"）
    pub nat_type: Mutex<Option<String>>,
    /// sidecar 启停互斥锁（防止 autostart 自动连接与手动连接并发导致双进程）
    pub process_lock: Mutex<()>,
    /// 日志环形缓冲区（最多 2000 行）
    pub log_buffer: crate::logger::LogBuffer,
    /// 流量统计快照
    pub traffic_snapshot: RwLock<TrafficSnapshot>,
    /// 按天累计流量统计（今日/昨日/本月/累计，持久化）
    pub traffic_daily: Mutex<crate::traffic::TrafficStats>,
    /// Sidecar 子进程句柄（必须持有，否则进程会被杀死）
    pub sidecar_child: RwLock<Option<CommandChild>>,
    /// 托盘动态菜单项句柄（用于更新状态行/连接/断开）
    pub tray_menu_items: Mutex<Option<TrayMenuItems>>,
    /// 配置存储路径
    pub config_dir: PathBuf,
    /// 当前应用句柄（用于 Rust → 前端 emit）
    pub app_handle: Mutex<Option<tauri::AppHandle>>,
}

/// 编译期断言：AppState 必须 Send + Sync（tauri::State 跨 await 要求）
const _: () = {
    const fn assert_send_sync<T: Send + Sync>() {}
    let _ = assert_send_sync::<AppState>;
};

impl AppState {
    pub fn new(config_dir: PathBuf) -> Self {
        Self {
            connection: RwLock::new(ConnectionStatus::Stopped),
            active_config_id: RwLock::new(None),
            virtual_ip: Mutex::new(None),
            server_host: Mutex::new(None),
            relay_addr: Mutex::new(None),
            nat_type: Mutex::new(None),
            process_lock: Mutex::new(()),
            log_buffer: crate::logger::LogBuffer::new(),
            traffic_snapshot: RwLock::new(TrafficSnapshot::default()),
            traffic_daily: Mutex::new(crate::traffic::TrafficStats::default()),
            sidecar_child: RwLock::new(None),
            tray_menu_items: Mutex::new(None),
            config_dir,
            app_handle: Mutex::new(None),
        }
    }
}
