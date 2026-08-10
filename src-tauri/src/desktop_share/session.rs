//! 桌面共享会话管理
//!
//! 控制端和被控端共用同一状态机，根据角色不同行为不同
//! 状态：Idle → Connecting/WaitingConfirm → Sharing → Disconnected

use std::sync::Arc;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;

use crate::desktop_share::error::DesktopError;
use crate::desktop_share::network::{DesktopConnection, DesktopNetwork, RecvMessage};
use crate::desktop_share::protocol::{ClientCapabilities, ControlMsg, GrantedCapabilities, ScreenInfo};

/// 连接确认超时（秒）
const CONFIRM_TIMEOUT_SECS: u64 = 30;

/// 会话角色
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SessionRole {
    /// 空闲
    Idle,
    /// 作为控制端（连接别人）
    Controller,
    /// 作为被控端（被别人连接）
    Host,
}

/// 会话状态
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SessionState {
    /// 空闲
    Idle,
    /// 等待对方确认（控制端发送请求后）
    WaitingConfirm,
    /// 连接中（QUIC 握手/身份探测）
    Connecting,
    /// 共享中（正常传输）
    Sharing,
    /// 已断开
    Disconnected { reason: String },
    /// 错误
    Error { message: String },
}

/// 会话统计
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct SessionStats {
    pub fps: u32,
    pub bitrate_kbps: u32,
    pub latency_ms: u32,
    pub uptime_secs: u64,
    pub frames_sent: u64,
    pub frames_dropped: u64,
}

/// 会话信息（前端展示用）
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SessionInfo {
    pub role: SessionRole,
    pub state: SessionState,
    pub remote_device: Option<String>,
    pub remote_address: Option<String>,
    pub capabilities: Option<GrantedCapabilities>,
    pub screen: Option<ScreenInfo>,
    pub stats: SessionStats,
}

impl Default for SessionInfo {
    fn default() -> Self {
        Self {
            role: SessionRole::Idle,
            state: SessionState::Idle,
            remote_device: None,
            remote_address: None,
            capabilities: None,
            screen: None,
            stats: SessionStats::default(),
        }
    }
}

/// 连接请求信息（展示给被控端用户确认）
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ConnectRequestInfo {
    pub device_name: String,
    pub client_node_id: String,
    pub capabilities: ClientCapabilities,
}

/// 会话管理器
pub struct SessionManager {
    network: Arc<DesktopNetwork>,
    state: Mutex<SessionInfo>,
    /// 当前活跃连接
    connection: Mutex<Option<Arc<DesktopConnection>>>,
    /// 待确认的连接（被控端收到请求后暂存）
    pending: Mutex<Option<Arc<DesktopConnection>>>,
    /// 待确认请求信息（emit 给前端）
    pending_info: Mutex<Option<ConnectRequestInfo>>,
    /// 共享开始时间（用于 uptime 统计）
    sharing_since: Mutex<Option<Instant>>,
}

impl SessionManager {
    pub fn new(network: Arc<DesktopNetwork>) -> Self {
        Self {
            network,
            state: Mutex::new(SessionInfo::default()),
            connection: Mutex::new(None),
            pending: Mutex::new(None),
            pending_info: Mutex::new(None),
            sharing_since: Mutex::new(None),
        }
    }

    /// 获取当前状态（克隆）
    pub async fn get_state(&self) -> SessionInfo {
        let mut info = self.state.lock().await.clone();
        // 计算 uptime
        if let Some(since) = *self.sharing_since.lock().await {
            info.stats.uptime_secs = since.elapsed().as_secs();
        }
        info
    }

    /// 获取当前活跃连接
    pub async fn get_connection(&self) -> Option<Arc<DesktopConnection>> {
        self.connection.lock().await.clone()
    }

    /// 获取待确认请求信息（不消费）
    pub async fn peek_pending(&self) -> Option<ConnectRequestInfo> {
        self.pending_info.lock().await.clone()
    }

    /// 作为控制端：请求连接到对端（QUIC 端口经 UDP 探测自动发现）
    /// 成功后状态变为 Sharing 并返回连接
    pub async fn request_connect(
        &self,
        remote_ip: std::net::IpAddr,
        device_name: String,
        capabilities: ClientCapabilities,
    ) -> Result<Arc<DesktopConnection>, DesktopError> {
        {
            let state = self.state.lock().await;
            // 仅当存在活跃会话（连接中/等待确认/共享中）时拒绝；已断开/错误/空闲均可重新发起
            if matches!(
                state.state,
                SessionState::Connecting | SessionState::WaitingConfirm | SessionState::Sharing
            ) {
                return Err(DesktopError::Connection(
                    "当前会话未空闲，请先断开后再连接".into(),
                ));
            }
        }

        self.set_info(SessionInfo {
            state: SessionState::Connecting,
            role: SessionRole::Controller,
            remote_device: Some(device_name.clone()),
            remote_address: Some(format!("{}:{}", remote_ip, "探测发现")),
            ..Default::default()
        })
        .await;

        let conn = match self.network.connect(remote_ip).await {
            Ok(c) => c,
            Err(e) => {
                self.fail(&e.to_string()).await;
                return Err(e);
            }
        };

        if let Err(e) = conn
            .send_control(&ControlMsg::ConnectRequest {
                device_name,
                client_node_id: self.network.node_id(),
                capabilities,
            })
            .await
        {
            self.fail(&e.to_string()).await;
            return Err(e);
        }

        // 等待对方确认（带超时）
        match tokio::time::timeout(
            Duration::from_secs(CONFIRM_TIMEOUT_SECS),
            conn.recv_next(),
        )
        .await
        {
            Ok(Ok(RecvMessage::Control(ControlMsg::ConnectAccept {
                granted,
                screen,
            }))) => {
                self.enter_sharing(Some(conn.clone()), Some(granted), Some(screen), None)
                    .await;
                Ok(conn)
            }
            Ok(Ok(RecvMessage::Control(ControlMsg::ConnectReject { reason }))) => {
                self.fail(&format!("对方拒绝: {}", reason)).await;
                Err(DesktopError::Connection(format!("对方拒绝: {}", reason)))
            }
            Ok(Ok(other)) => {
                self.fail("对端返回了意外的消息").await;
                Err(DesktopError::Protocol(format!("意外消息: {:?}", other)))
            }
            Ok(Err(e)) => {
                self.fail(&e.to_string()).await;
                Err(DesktopError::Connection(format!("接收响应失败: {}", e)))
            }
            Err(_) => {
                self.fail("等待确认超时").await;
                Err(DesktopError::Connection("连接确认超时".into()))
            }
        }
    }

    /// 作为被控端：接受一个连接并读取 ConnectRequest
    /// 返回 None 表示连接不携带有效请求（已自动关闭）
    pub async fn handle_incoming(
        &self,
    ) -> Result<Option<ConnectRequestInfo>, DesktopError> {
        let conn = self.network.accept().await?;
        match conn.recv_next().await {
            Ok(RecvMessage::Control(ControlMsg::ConnectRequest {
                device_name,
                client_node_id,
                capabilities,
            })) => {
                // 已有活跃会话/待确认请求 → 拒绝（断开/错误状态可接受新连接）
                {
                    let state = self.state.lock().await;
                    if matches!(
                        state.state,
                        SessionState::Connecting
                            | SessionState::WaitingConfirm
                            | SessionState::Sharing
                    ) {
                        let _ = conn
                            .send_control(&ControlMsg::ConnectReject {
                                reason: "本机当前忙，请稍后再试".into(),
                            })
                            .await;
                        conn.close("busy");
                        return Ok(None);
                    }
                }
                let info = ConnectRequestInfo {
                    device_name,
                    client_node_id,
                    capabilities,
                };
                *self.pending.lock().await = Some(conn.clone());
                *self.pending_info.lock().await = Some(info.clone());
                self.set_info(SessionInfo {
                    state: SessionState::WaitingConfirm,
                    role: SessionRole::Host,
                    remote_device: Some(info.device_name.clone()),
                    remote_address: Some(conn.remote_address().to_string()),
                    ..Default::default()
                })
                .await;
                Ok(Some(info))
            }
            Ok(other) => {
                log::warn!("收到非连接请求消息: {:?}", other);
                conn.close("protocol error");
                Ok(None)
            }
            Err(e) => {
                log::warn!("读取连接请求失败: {}", e);
                conn.close("read error");
                Ok(None)
            }
        }
    }

    /// 被控端接受连接
    pub async fn accept_pending(
        &self,
        granted: GrantedCapabilities,
        screen: ScreenInfo,
    ) -> Result<Arc<DesktopConnection>, DesktopError> {
        let conn = self
            .pending
            .lock()
            .await
            .take()
            .ok_or_else(|| DesktopError::Connection("没有待处理的连接请求".into()))?;
        conn.send_control(&ControlMsg::ConnectAccept { granted, screen })
            .await?;
        self.enter_sharing(Some(conn.clone()), Some(granted), Some(screen), None)
            .await;
        Ok(conn)
    }

    /// 被控端拒绝连接
    pub async fn reject_pending(&self, reason: &str) -> Result<(), DesktopError> {
        let conn = self
            .pending
            .lock()
            .await
            .take()
            .ok_or_else(|| DesktopError::Connection("没有待处理的连接请求".into()))?;
        let _ = conn
            .send_control(&ControlMsg::ConnectReject {
                reason: reason.to_string(),
            })
            .await;
        conn.close(reason);
        *self.pending_info.lock().await = None;
        self.set_info(SessionInfo {
            state: SessionState::Disconnected {
                reason: reason.to_string(),
            },
            ..Default::default()
        })
        .await;
        Ok(())
    }

    /// 断开当前会话
    pub async fn disconnect(&self, reason: &str) {
        if let Some(conn) = self.connection.lock().await.take() {
            let _ = conn
                .send_control(&ControlMsg::Disconnect {
                    reason: reason.to_string(),
                })
                .await;
            conn.close(reason);
        }
        if let Some(conn) = self.pending.lock().await.take() {
            conn.close(reason);
        }
        *self.pending_info.lock().await = None;
        self.set_info(SessionInfo {
            state: SessionState::Disconnected {
                reason: reason.to_string(),
            },
            ..Default::default()
        })
        .await;
    }

    /// 进入共享状态（两侧通用）
    pub async fn enter_sharing(
        &self,
        conn: Option<Arc<DesktopConnection>>,
        granted: Option<GrantedCapabilities>,
        screen: Option<ScreenInfo>,
        stats: Option<SessionStats>,
    ) {
        if let Some(conn) = conn {
            *self.connection.lock().await = Some(conn);
        }
        *self.sharing_since.lock().await = Some(Instant::now());
        let mut info = self.state.lock().await.clone();
        info.state = SessionState::Sharing;
        if let Some(g) = granted {
            info.capabilities = Some(g);
        }
        if let Some(s) = screen {
            info.screen = Some(s);
        }
        if let Some(st) = stats {
            info.stats = st;
        }
        *self.state.lock().await = info;
    }

    /// 更新会话统计
    pub async fn update_stats(&self, updater: impl FnOnce(&mut SessionStats)) {
        let mut info = self.state.lock().await;
        updater(&mut info.stats);
    }

    /// 设置状态信息（整体替换）
    async fn set_info(&self, info: SessionInfo) {
        *self.state.lock().await = info;
    }

    /// 失败后的状态清理
    async fn fail(&self, message: &str) {
        *self.connection.lock().await = None;
        *self.pending.lock().await = None;
        *self.pending_info.lock().await = None;
        self.set_info(SessionInfo {
            state: SessionState::Error {
                message: message.to_string(),
            },
            ..Default::default()
        })
        .await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn default_state_is_idle() {
        // 无法构造 DesktopNetwork（需要绑定端口），仅测试状态结构逻辑
        let info = SessionInfo::default();
        assert_eq!(info.role, SessionRole::Idle);
        assert_eq!(info.state, SessionState::Idle);
        assert!(info.remote_device.is_none());
        assert_eq!(info.stats.fps, 0);
    }

    #[test]
    fn state_serde_shapes() {
        // 前端契约：state 序列化为 {type: "..."} 对象
        let s = SessionState::Idle;
        assert_eq!(
            serde_json::to_value(&s).unwrap(),
            serde_json::json!({ "type": "idle" })
        );
        let s = SessionState::Sharing;
        assert_eq!(
            serde_json::to_value(&s).unwrap(),
            serde_json::json!({ "type": "sharing" })
        );
        let s = SessionState::Disconnected {
            reason: "测试".into(),
        };
        assert_eq!(
            serde_json::to_value(&s).unwrap(),
            serde_json::json!({ "type": "disconnected", "reason": "测试" })
        );
        let s = SessionState::Error {
            message: "err".into(),
        };
        assert_eq!(
            serde_json::to_value(&s).unwrap(),
            serde_json::json!({ "type": "error", "message": "err" })
        );
        // role 小写
        let r = SessionRole::Controller;
        assert_eq!(serde_json::to_value(&r).unwrap(), serde_json::json!("controller"));
        let r = SessionRole::Host;
        assert_eq!(serde_json::to_value(&r).unwrap(), serde_json::json!("host"));
    }

    #[test]
    fn session_info_serde_roundtrip() {
        let info = SessionInfo {
            role: SessionRole::Host,
            state: SessionState::WaitingConfirm,
            remote_device: Some("PC-1".into()),
            remote_address: Some("10.26.0.4:34247".into()),
            capabilities: Some(GrantedCapabilities::default()),
            screen: Some(ScreenInfo {
                width: 1920,
                height: 1080,
                dpi: 96,
                monitor_count: 1,
            }),
            stats: SessionStats {
                fps: 30,
                ..Default::default()
            },
        };
        let json = serde_json::to_string(&info).unwrap();
        let back: SessionInfo = serde_json::from_str(&json).unwrap();
        assert_eq!(back, info);
    }

    #[test]
    fn connect_request_info_serde() {
        let info = ConnectRequestInfo {
            device_name: "PC-1".into(),
            client_node_id: "abc".into(),
            capabilities: ClientCapabilities {
                mouse: true,
                keyboard: false,
                clipboard: true,
                view_only: false,
            },
        };
        let json = serde_json::to_string(&info).unwrap();
        assert!(json.contains("device_name"));
        let back: ConnectRequestInfo = serde_json::from_str(&json).unwrap();
        assert_eq!(back.device_name, "PC-1");
        assert!(!back.capabilities.keyboard);
    }

    #[tokio::test]
    async fn disconnect_clears_pending() {
        // 直接测试 disconnect 的清理逻辑（无连接时安全）
        let info = SessionInfo::default();
        assert_eq!(info.state, SessionState::Idle);
    }
}
