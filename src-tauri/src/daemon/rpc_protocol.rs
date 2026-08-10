//! Daemon RPC 协议：GUI ↔ vnt-daemon 的契约（JSON-RPC 2.0 over TCP，\n 分隔）
//!
//! 复用现有配置类型（crate::config::VntConfig / crate::ftp::config::FtpConfig /
//! crate::state::PeerInfo），避免协议层维护双份定义。

use serde::{Deserialize, Serialize};

/// GUI → Daemon 请求
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "method", rename_all = "snake_case")]
pub enum DaemonRequest {
    /// 查询 daemon 健康状态
    Ping,
    /// 获取完整运行时状态
    GetState,
    /// VNT 控制
    VntStart { config: crate::config::VntConfig },
    VntStop,
    VntRestart { config: crate::config::VntConfig },
    /// FTP 控制
    FtpStart { config: crate::ftp::config::FtpConfig },
    FtpStop,
    FtpRestart { config: crate::ftp::config::FtpConfig },
    /// 获取 VNT 节点列表（含延时）
    VntListPeers,
    /// 获取 FTP 连接日志
    FtpGetLogs,
    /// 优雅关闭 daemon（完全退出）
    Shutdown,
}

/// Daemon → GUI 响应
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum DaemonResponse {
    /// 对 Ping 的回复
    Pong { uptime_secs: u64 },
    /// 完整状态快照
    State {
        vnt_running: bool,
        vnt_connected: bool,
        vnt_config: Option<crate::config::VntConfig>,
        ftp_running: bool,
        ftp_config: Option<crate::ftp::config::FtpConfig>,
        peers: Vec<crate::state::PeerInfo>,
        /// 真实连接服务器（IP:端口，连接信息展示）
        vnt_server_host: Option<String>,
        /// 本机虚拟 IP
        vnt_virtual_ip: Option<String>,
        /// NAT 类型（尽力解析，可能为 null）
        vnt_nat_type: Option<String>,
        uptime_secs: u64,
    },
    /// 通用成功
    Ok,
    /// 通用错误
    Error { code: String, message: String },
    /// 事件推送（daemon 主动 → GUI）
    Event {
        event: String, // "vnt_connected" / "vnt_disconnected" / "ftp_started" / etc.
        data: serde_json::Value,
    },
}

impl DaemonResponse {
    /// 转换为 Result（Ok / Error 两态）
    pub fn into_result(self) -> Result<Self, String> {
        match self {
            DaemonResponse::Error { message, .. } => Err(message),
            other => Ok(other),
        }
    }
}

/// RPC 默认监听地址
pub const DAEMON_ADDR: &str = "127.0.0.1:17532";
/// 测试用端口（避免与运行中的 daemon 冲突）
pub const DAEMON_TEST_ADDR: &str = "127.0.0.1:17533";
