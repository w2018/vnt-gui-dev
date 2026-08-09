//! 全局状态定义（文档 §3.2）

use std::path::PathBuf;

use parking_lot::{Mutex, RwLock};
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
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct PeerInfo {
    pub name: String,
    pub virtual_ip: String,
    /// "p2p" | "relay" | "client-relay"
    pub connection_type: String,
    pub latency: u64,
    /// "online" | "offline"
    pub status: String,
}

/// 全局应用状态（通过 tauri::State 管理）
pub struct AppState {
    /// 当前连接状态
    pub connection: RwLock<ConnectionStatus>,
    /// 当前活动配置 ID
    pub active_config_id: RwLock<Option<String>>,
    /// 日志环形缓冲区（最多 2000 行）
    pub log_buffer: crate::logger::LogBuffer,
    /// 流量统计快照
    pub traffic_snapshot: RwLock<TrafficSnapshot>,
    /// Sidecar 子进程句柄（必须持有，否则进程会被杀死）
    pub sidecar_child: RwLock<Option<CommandChild>>,
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
            log_buffer: crate::logger::LogBuffer::new(),
            traffic_snapshot: RwLock::new(TrafficSnapshot::default()),
            sidecar_child: RwLock::new(None),
            config_dir,
            app_handle: Mutex::new(None),
        }
    }
}
