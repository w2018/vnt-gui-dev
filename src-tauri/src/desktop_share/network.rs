//! Iroh QUIC 网络层
//!
//! 设计要点（相对文档 v4.0 的修正）：
//! - iroh-net 0.17 的 Endpoint::builder 仅支持 bind(port)（绑定所有本机接口），
//!   不支持绑定特定 IP；VNT 虚拟 IP 是本机接口之一，控制端 connect 到 VNT 虚拟 IP
//!   时数据包自然走 VNT 虚拟网卡，等价于"绑定 VNT 虚拟 IP"
//! - 关闭 Iroh Relay（RelayMode::Disabled），NAT 穿透由 VNT 完成
//! - 身份交换：QUIC 握手需要对端 NodeId（公钥）。控制端先通过 UDP 探测
//!   （listen_port+1 端口）获取被控端 NodeId，再构造 NodeAddr 建立 QUIC 连接
//! - 消息 framing：[u8 类型][u32 长度][bincode 载荷]，每条消息使用独立的
//!   uni stream（发送方 open_uni → 写 → finish；接收方 accept_uni → 读）
//! - 视频帧载荷 = bincode(VideoFrameHeader) + 原始 H.264 数据（header 固定 21 字节）

use std::net::{IpAddr, SocketAddr};
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

use iroh_net::endpoint::Connection;
use iroh_net::relay::RelayMode;
use iroh_net::{Endpoint, NodeAddr, NodeId};
use serde::de::DeserializeOwned;
use serde::Serialize;
use tokio::net::UdpSocket;

use crate::desktop_share::error::DesktopError;
use crate::desktop_share::protocol::{ControlMsg, InputEvent, VideoFrameHeader};

/// 桌面共享默认监听端口（QUIC）
pub const DEFAULT_PORT: u16 = 34247;
/// ALPN 协议标识
pub const ALPN: &[u8] = b"vnt-desktop-share/1";
/// UDP 身份探测固定端口（与 QUIC 端口解耦，控制端通过它发现被控端实际 QUIC 端口）
pub const PROBE_PORT: u16 = 34247;
/// UDP 探测请求魔数
const PROBE_MAGIC: &str = "VNTDS1";
/// 探测响应前缀
const PROBE_PREFIX: &str = "VNTDS1:";
/// 单条消息最大长度（8 MiB，防恶意超大帧）
const MAX_FRAME_SIZE: usize = 8 * 1024 * 1024;
/// VideoFrameHeader 的 bincode 序列化固定长度（u64 + bool + 3×u32）
const VIDEO_HEADER_LEN: usize = 8 + 1 + 12;

/// 消息类型字节
const TYPE_CONTROL: u8 = 0;
const TYPE_INPUT: u8 = 1;
const TYPE_CLIPBOARD: u8 = 2;
const TYPE_VIDEO: u8 = 3;

/// 统一接收的已分类消息
#[derive(Debug)]
pub enum RecvMessage {
    Control(ControlMsg),
    Input(InputEvent),
    /// 剪贴板文本（发送端用 send_clipboard）
    Clipboard(String),
    /// 视频帧（H.264 Annex-B 数据）
    Video(VideoFrameHeader, Vec<u8>),
}

/// 桌面共享网络管理器
pub struct DesktopNetwork {
    endpoint: Endpoint,
    /// 实际绑定端口
    bound_port: u16,
}

impl DesktopNetwork {
    /// 创建 Iroh Endpoint（绑定所有本机接口，含 VNT 虚拟 IP）
    /// port=0 时由系统分配随机空闲端口；QUIC 端口与探测端口（PROBE_PORT）解耦
    pub async fn new(port: u16) -> Result<Self, DesktopError> {
        let endpoint = Endpoint::builder()
            .relay_mode(RelayMode::Disabled) // 不用 Iroh Relay，走 VNT 虚拟网卡
            .alpns(vec![ALPN.to_vec()])
            .bind(port)
            .await
            .map_err(|e| DesktopError::Network(format!("创建 Endpoint 失败: {}", e)))?;

        let bound_port = endpoint.local_addr().0.port();
        log::info!(
            "桌面共享 Iroh Node: {} (端口 {})",
            endpoint.node_id(),
            bound_port
        );

        Ok(Self {
            endpoint,
            bound_port,
        })
    }

    /// 本机 Node ID（hex 字符串，用于展示与探测交换）
    pub fn node_id(&self) -> String {
        self.endpoint.node_id().to_string()
    }

    /// 实际绑定端口
    pub fn bound_port(&self) -> u16 {
        self.bound_port
    }

    /// 启动 UDP 身份探测服务器（固定 PROBE_PORT，响应本机 NodeId + QUIC 实际端口）
    /// 返回任务句柄；endpoint 关闭时 abort
    pub async fn start_probe_server(&self) -> Result<tokio::task::JoinHandle<()>, DesktopError> {
        self.start_probe_server_on(PROBE_PORT).await
    }

    /// 指定端口启动探测服务器（生产用 [start_probe_server]；测试用独立端口避免与运行实例冲突）
    pub(crate) async fn start_probe_server_on(
        &self,
        port: u16,
    ) -> Result<tokio::task::JoinHandle<()>, DesktopError> {
        let socket = UdpSocket::bind((IpAddr::from([0, 0, 0, 0]), port))
            .await
            .map_err(|e| DesktopError::Network(format!("绑定探测端口 {} 失败: {}", port, e)))?;
        log::info!("桌面共享身份探测监听端口 {}", port);

        let node_id = self.node_id();
        let quic_port = self.bound_port();
        Ok(tokio::spawn(async move {
            let mut buf = [0u8; 256];
            loop {
                match socket.recv_from(&mut buf).await {
                    Ok((n, peer)) => {
                        let msg = &buf[..n];
                        if msg == PROBE_MAGIC.as_bytes() {
                            // 响应格式：VNTDS1:{node_id}:{quic_port}
                            let reply = format!("{}{}:{}\n", PROBE_PREFIX, node_id, quic_port);
                            if let Err(e) = socket.send_to(reply.as_bytes(), peer).await {
                                log::warn!("探测响应发送失败: {}", e);
                            }
                        }
                    }
                    Err(e) => {
                        log::warn!("探测监听错误: {}", e);
                        break;
                    }
                }
            }
        }))
    }

    /// 连接到对端：先经 PROBE_PORT UDP 探测发现其 NodeId 与 QUIC 端口，再建立 QUIC 连接
    pub async fn connect(&self, remote_ip: IpAddr) -> Result<Arc<DesktopConnection>, DesktopError> {
        let (node_id, quic_port) = probe_remote_on(remote_ip, PROBE_PORT).await?;
        self.connect_with_node_id(remote_ip, quic_port, node_id).await
    }

    /// 直接以已知 NodeId 连接（供内部与测试使用）
    pub async fn connect_with_node_id(
        &self,
        remote_ip: IpAddr,
        port: u16,
        node_id: NodeId,
    ) -> Result<Arc<DesktopConnection>, DesktopError> {
        let addr = SocketAddr::new(remote_ip, port);
        let node_addr = NodeAddr::from_parts(node_id, None, vec![addr]);
        log::info!("正在连接 {} (node_id={})...", addr, node_id);
        let conn = self
            .endpoint
            .connect(node_addr, ALPN)
            .await
            .map_err(|e| DesktopError::Connection(format!("连接失败: {}", e)))?;
        log::info!("已连接到 {}", conn.remote_address());
        Ok(Arc::new(DesktopConnection::new(conn)))
    }

    /// 接受对端连接（监听循环中调用）
    pub async fn accept(&self) -> Result<Arc<DesktopConnection>, DesktopError> {
        let connecting = self
            .endpoint
            .accept()
            .await
            .ok_or_else(|| DesktopError::Connection("没有待接受的连接".into()))?;
        let conn = connecting
            .await
            .map_err(|e| DesktopError::Connection(format!("接受连接失败: {}", e)))?;
        log::info!("接受来自 {} 的连接", conn.remote_address());
        Ok(Arc::new(DesktopConnection::new(conn)))
    }
}

/// 封装的单条连接（多路复用 uni stream）
pub struct DesktopConnection {
    inner: Connection,
}

impl DesktopConnection {
    pub fn new(inner: Connection) -> Self {
        Self { inner }
    }

    /// 远端地址
    pub fn remote_address(&self) -> SocketAddr {
        self.inner.remote_address()
    }

    /// 发送控制消息
    pub async fn send_control(&self, msg: &ControlMsg) -> Result<(), DesktopError> {
        self.send_framed(TYPE_CONTROL, &msg).await
    }

    /// 发送输入事件
    pub async fn send_input(&self, event: &InputEvent) -> Result<(), DesktopError> {
        self.send_framed(TYPE_INPUT, event).await
    }

    /// 发送剪贴板文本
    pub async fn send_clipboard(&self, text: &str) -> Result<(), DesktopError> {
        self.send_framed(TYPE_CLIPBOARD, text).await
    }

    /// 发送视频帧（H.264 数据）
    pub async fn send_video(
        &self,
        header: &VideoFrameHeader,
        data: &[u8],
    ) -> Result<(), DesktopError> {
        if header.data_len as usize != data.len() {
            return Err(DesktopError::Protocol(format!(
                "视频帧长度不符: header={} actual={}",
                header.data_len,
                data.len()
            )));
        }
        let header_bytes =
            bincode::serialize(header).map_err(|e| DesktopError::Protocol(e.to_string()))?;
        debug_assert_eq!(header_bytes.len(), VIDEO_HEADER_LEN);
        let mut payload = Vec::with_capacity(header_bytes.len() + data.len());
        payload.extend_from_slice(&header_bytes);
        payload.extend_from_slice(data);
        self.send_framed_raw(TYPE_VIDEO, &payload).await
    }

    /// 接收下一条消息（阻塞直到收到；连接关闭返回 Err）
    pub async fn recv_next(&self) -> Result<RecvMessage, DesktopError> {
        let mut stream = self
            .inner
            .accept_uni()
            .await
            .map_err(|e| DesktopError::Connection(format!("接收流失败: {}", e)))?;

        // [类型 u8][长度 u32]
        let mut head = [0u8; 5];
        stream
            .read_exact(&mut head)
            .await
            .map_err(|e| DesktopError::Connection(format!("读取消息头失败: {}", e)))?;
        let ty = head[0];
        let len = u32::from_le_bytes([head[1], head[2], head[3], head[4]]) as usize;
        if len > MAX_FRAME_SIZE {
            return Err(DesktopError::Protocol(format!("消息过大: {} bytes", len)));
        }
        let mut payload = vec![0u8; len];
        stream
            .read_exact(&mut payload)
            .await
            .map_err(|e| DesktopError::Connection(format!("读取消息体失败: {}", e)))?;

        match ty {
            TYPE_CONTROL => Ok(RecvMessage::Control(
                decode(&payload).map_err(|e| DesktopError::Protocol(e.to_string()))?,
            )),
            TYPE_INPUT => Ok(RecvMessage::Input(
                decode(&payload).map_err(|e| DesktopError::Protocol(e.to_string()))?,
            )),
            TYPE_CLIPBOARD => Ok(RecvMessage::Clipboard(
                decode(&payload).map_err(|e| DesktopError::Protocol(e.to_string()))?,
            )),
            TYPE_VIDEO => {
                if payload.len() < VIDEO_HEADER_LEN {
                    return Err(DesktopError::Protocol("视频帧头不完整".into()));
                }
                let (header_bytes, data) = payload.split_at(VIDEO_HEADER_LEN);
                let header: VideoFrameHeader =
                    decode(header_bytes).map_err(|e| DesktopError::Protocol(e.to_string()))?;
                if header.data_len as usize != data.len() {
                    return Err(DesktopError::Protocol(format!(
                        "视频帧长度不符: header={} actual={}",
                        header.data_len,
                        data.len()
                    )));
                }
                Ok(RecvMessage::Video(header, data.to_vec()))
            }
            other => Err(DesktopError::Protocol(format!("未知消息类型: {}", other))),
        }
    }

    /// 关闭连接（尽力通知对端）
    pub fn close(&self, reason: &str) {
        self.inner.close(0u8.into(), reason.as_bytes());
    }

    async fn send_framed<T: Serialize + ?Sized>(&self, ty: u8, msg: &T) -> Result<(), DesktopError> {
        let payload =
            bincode::serialize(msg).map_err(|e| DesktopError::Protocol(e.to_string()))?;
        self.send_framed_raw(ty, &payload).await
    }

    async fn send_framed_raw(&self, ty: u8, payload: &[u8]) -> Result<(), DesktopError> {
        let mut stream = self
            .inner
            .open_uni()
            .await
            .map_err(|e| DesktopError::Network(format!("打开流失败: {}", e)))?;
        let len = (payload.len() as u32).to_le_bytes();
        let head = [ty, len[0], len[1], len[2], len[3]];
        stream
            .write_all(&head)
            .await
            .map_err(|e| DesktopError::Network(format!("写入消息头失败: {}", e)))?;
        stream
            .write_all(payload)
            .await
            .map_err(|e| DesktopError::Network(format!("写入消息体失败: {}", e)))?;
        stream
            .finish()
            .await
            .map_err(|e| DesktopError::Network(format!("结束流失败: {}", e)))?;
        Ok(())
    }
}

// ==================== 序列化辅助 ====================

fn decode<T: DeserializeOwned>(bytes: &[u8]) -> Result<T, bincode::Error> {
    bincode::deserialize(bytes)
}

// ==================== UDP 身份探测 ====================

/// 向对端固定探测端口发送请求，获取其 (NodeId, QUIC 端口)
/// 响应格式：VNTDS1:{node_id_hex}:{quic_port}
async fn probe_remote_on(remote_ip: IpAddr, probe_port: u16) -> Result<(NodeId, u16), DesktopError> {
    let local = UdpSocket::bind((IpAddr::from([0, 0, 0, 0]), 0))
        .await
        .map_err(|e| DesktopError::Network(format!("探测 socket 创建失败: {}", e)))?;
    local
        .send_to(PROBE_MAGIC.as_bytes(), (remote_ip, probe_port))
        .await
        .map_err(|e| DesktopError::Connection(format!("探测发送失败: {}", e)))?;

    let mut buf = [0u8; 512];
    let (n, _peer) = tokio::time::timeout(
        Duration::from_secs(2),
        local.recv_from(&mut buf),
    )
    .await
    .map_err(|_| {
        DesktopError::Connection(format!(
            "探测超时: {}:{}（被控端未运行桌面共享，或防火墙拦截了 UDP {}）",
            remote_ip, probe_port, probe_port
        ))
    })?
    .map_err(|e| DesktopError::Connection(format!("探测接收失败: {}", e)))?;

    let text = std::str::from_utf8(&buf[..n])
        .map_err(|_| DesktopError::Protocol("探测响应非 UTF-8".into()))?
        .trim();
    let body = text
        .strip_prefix(PROBE_PREFIX)
        .ok_or_else(|| DesktopError::Protocol(format!("探测响应格式错误: {}", text)))?;

    // 兼容旧格式（无端口）与新版（node_id:port）
    let (node_id_hex, quic_port) = match body.rsplit_once(':') {
        Some((hex, port_str)) => match port_str.parse::<u16>() {
            Ok(p) if p > 0 => (hex, p),
            _ => (body, PROBE_PORT),
        },
        None => (body, PROBE_PORT),
    };

    let node_id = NodeId::from_str(node_id_hex)
        .map_err(|e| DesktopError::Protocol(format!("NodeId 解析失败: {}", e)))?;
    Ok((node_id, quic_port))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::desktop_share::protocol::{ClientCapabilities, ScreenInfo};

    /// 双 endpoint 本地互连：探测 → QUIC 连接 → 控制消息往返 → 视频帧往返
    #[tokio::test]
    async fn two_endpoints_full_roundtrip() {
        // 用独立探测端口（23247），避免与运行中的 vnt-gui 抢占 PROBE_PORT
        const TEST_PROBE_PORT: u16 = 23247;
        let a = DesktopNetwork::new(43210).await.expect("A endpoint");
        let b = DesktopNetwork::new(44555).await.expect("B endpoint");

        // A 启动探测服务器（测试端口）
        let _probe_task = a
            .start_probe_server_on(TEST_PROBE_PORT)
            .await
            .expect("probe server");

        // B 探测发现 A 的 NodeId + QUIC 端口，建立 QUIC 连接
        let (node_id, quic_port) = probe_remote_on(IpAddr::from([127, 0, 0, 1]), TEST_PROBE_PORT)
            .await
            .expect("探测 A");
        assert_eq!(quic_port, a.bound_port(), "探测应返回 A 的实际 QUIC 端口");
        let conn_b = b
            .connect_with_node_id(IpAddr::from([127, 0, 0, 1]), quic_port, node_id)
            .await
            .expect("B→A 连接");

        // A 接受连接
        let conn_a = a.accept().await.expect("A 接受连接");

        // B → A：控制消息（ConnectRequest）
        conn_b
            .send_control(&ControlMsg::ConnectRequest {
                device_name: "test-b".into(),
                client_node_id: b.node_id(),
                capabilities: ClientCapabilities {
                    mouse: true,
                    keyboard: true,
                    clipboard: true,
                    view_only: false,
                },
            })
            .await
            .expect("发送 ConnectRequest");

        match conn_a.recv_next().await.expect("A 接收") {
            RecvMessage::Control(ControlMsg::ConnectRequest {
                device_name,
                client_node_id,
                capabilities,
            }) => {
                assert_eq!(device_name, "test-b");
                assert_eq!(client_node_id, b.node_id());
                assert!(capabilities.mouse);
            }
            other => panic!("期望 ConnectRequest，实际 {:?}", other),
        }

        // A → B：ConnectAccept
        conn_a
            .send_control(&ControlMsg::ConnectAccept {
                granted: crate::desktop_share::protocol::GrantedCapabilities::default(),
                screen: ScreenInfo {
                    width: 1920,
                    height: 1080,
                    dpi: 96,
                    monitor_count: 1,
                },
            })
            .await
            .expect("发送 ConnectAccept");

        match conn_b.recv_next().await.expect("B 接收") {
            RecvMessage::Control(ControlMsg::ConnectAccept { granted, screen }) => {
                assert!(granted.mouse);
                assert_eq!(screen.width, 1920);
            }
            other => panic!("期望 ConnectAccept，实际 {:?}", other),
        }

        // B → A：视频帧（伪造 NALU 数据）
        let fake_nalu = vec![0x65, 0x88, 0x84, 0x21, 0xff];
        conn_b
            .send_video(
                &VideoFrameHeader {
                    pts: 7,
                    is_keyframe: true,
                    width: 1920,
                    height: 1080,
                    data_len: fake_nalu.len() as u32,
                },
                &fake_nalu,
            )
            .await
            .expect("发送视频帧");

        match conn_a.recv_next().await.expect("A 接收视频") {
            RecvMessage::Video(header, data) => {
                assert_eq!(header.pts, 7);
                assert!(header.is_keyframe);
                assert_eq!(data, fake_nalu);
            }
            other => panic!("期望视频帧，实际 {:?}", other),
        }

        // B → A：剪贴板
        conn_b
            .send_clipboard("你好，VNT!")
            .await
            .expect("发送剪贴板");
        match conn_a.recv_next().await.expect("A 接收剪贴板") {
            RecvMessage::Clipboard(text) => assert_eq!(text, "你好，VNT!"),
            other => panic!("期望剪贴板，实际 {:?}", other),
        }

        conn_a.close("测试结束");
        conn_b.close("测试结束");
    }

    #[tokio::test]
    async fn probe_timeout_fails_cleanly() {
        let b = DesktopNetwork::new(0).await.expect("endpoint");
        // 探测 PROBE_PORT 无监听者时应快速失败（2s 超时）
        // 若本机恰好有桌面共享在运行（端口被占），本测试会连上——跳过该场景用未占端口：
        // 直接向 127.0.0.1 探测，若无响应则超时失败 ✓
        let r = b.connect(IpAddr::from([192, 0, 2, 1])).await;
        assert!(r.is_err(), "无探测服务器时应失败");
    }

    #[test]
    fn probe_response_parse() {
        // 新版响应（含 QUIC 端口）
        let sk = iroh_net::key::SecretKey::from_bytes(&[2u8; 32]);
        let node_id = sk.public();
        let reply = format!("{}{}:{}{}", PROBE_PREFIX, node_id, 45231, "\n");
        let text = reply.trim();
        let body = text.strip_prefix(PROBE_PREFIX).unwrap();
        let (hex, port) = body.rsplit_once(':').unwrap();
        let parsed_id = NodeId::from_str(hex).unwrap();
        let parsed_port: u16 = port.parse().unwrap();
        assert_eq!(parsed_id, node_id);
        assert_eq!(parsed_port, 45231);

        // 旧版响应（无端口）→ 默认 PROBE_PORT
        let old = format!("{}{}\n", PROBE_PREFIX, node_id);
        let body = old.trim().strip_prefix(PROBE_PREFIX).unwrap();
        let (hex2, port2) = match body.rsplit_once(':') {
            Some((h, p)) => match p.parse::<u16>() {
                Ok(p) if p > 0 => (h, p),
                _ => (body, PROBE_PORT),
            },
            None => (body, PROBE_PORT),
        };
        assert_eq!(NodeId::from_str(hex2).unwrap(), node_id);
        assert_eq!(port2, PROBE_PORT);

        // 错误格式
        assert!("VNTDS2:abc".strip_prefix(PROBE_PREFIX).is_none());
        assert!(NodeId::from_str("not-a-hex").is_err());
    }

    #[test]
    fn frame_header_constants() {
        // VideoFrameHeader bincode 长度 = 8(u64) + 1(bool) + 12(3×u32)
        let h = VideoFrameHeader {
            pts: 1,
            is_keyframe: true,
            width: 1920,
            height: 1080,
            data_len: 100,
        };
        let bytes = bincode::serialize(&h).unwrap();
        assert_eq!(bytes.len(), VIDEO_HEADER_LEN);
    }

    #[test]
    fn probe_protocol_parse() {
        // 模拟探测响应解析（与 probe_node_id 相同的解析逻辑）
        // 用 SecretKey::from_bytes 构造有效 ed25519 密钥（任意 32 字节私钥均有效）
        let sk = iroh_net::key::SecretKey::from_bytes(&[2u8; 32]);
        let node_id = sk.public();
        let reply = format!("{}{}\n", PROBE_PREFIX, node_id);
        let text = reply.trim();
        let hex = text.strip_prefix(PROBE_PREFIX).unwrap().trim();
        let parsed = NodeId::from_str(hex).unwrap();
        assert_eq!(parsed, node_id);

        // 错误格式
        assert!("VNTDS2:abc".strip_prefix(PROBE_PREFIX).is_none());
        assert!(NodeId::from_str("not-a-hex").is_err());
    }
}
