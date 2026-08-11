//! 文件传输 Iroh QUIC 网络层
//!
//! 独立 Endpoint（ALPN `vnt-file-transfer/1`），不修改 desktop_share/：
//! 其 Endpoint 的 accept() 已被桌面共享监听循环独占，无法插入新的文件流类型。
//!
//! 身份交换与桌面共享一致：QUIC 握手需要对端 NodeId（公钥），发送方先经
//! UDP 探测（PROBE_PORT）获取对端 NodeId 与实际 QUIC 端口，再建立连接。
//! 消息 framing：[u8 类型][u32 长度][bincode FileMsg 载荷]

use std::net::{IpAddr, SocketAddr};
use std::str::FromStr;
use std::sync::atomic::{AtomicU16, Ordering};
use std::sync::Arc;
use std::time::Duration;

use iroh_net::endpoint::Connection;
use iroh_net::relay::RelayMode;
use iroh_net::{Endpoint, NodeAddr, NodeId};
use tokio::net::UdpSocket;

use crate::file_transfer::protocol::{FileMsg, MAX_FRAME_SIZE};

/// 文件传输 QUIC 固定监听端口
pub const DEFAULT_QUIC_PORT: u16 = 34249;
/// ALPN 协议标识
pub const ALPN: &[u8] = b"vnt-file-transfer/1";
/// UDP 身份探测固定端口
pub const PROBE_PORT: u16 = 34249;
/// UDP 探测请求魔数
const PROBE_MAGIC: &str = "VNTFT1";
/// 探测响应前缀
const PROBE_PREFIX: &str = "VNTFT1:";
/// 探测超时
const PROBE_TIMEOUT: Duration = Duration::from_secs(2);
/// 帧类型（FileMsg 统一消息）
const TYPE_FILETRANSFER: u8 = 1;

/// 探测结果（NodeId + 实际 QUIC 端口 + TCP 大文件端口）
pub struct ProbeInfo {
    pub node_id: NodeId,
    pub quic_port: u16,
    /// 接收端 TCP 高速通道监听端口（0 = 未就绪）
    pub tcp_port: u16,
}

/// 文件传输网络管理器
pub struct FileNetwork {
    endpoint: Endpoint,
    /// 实际绑定端口
    bound_port: u16,
    /// TCP 大文件监听端口（由 daemon_listener 动态写入，探测响应公告）
    tcp_port_hint: Arc<AtomicU16>,
}

impl FileNetwork {
    /// 创建 Iroh Endpoint（绑定所有本机接口，含 VNT 虚拟 IP）
    /// port=0 时由系统分配随机端口；生产固定 DEFAULT_QUIC_PORT
    pub async fn new(port: u16) -> Result<Self, String> {
        let endpoint = Endpoint::builder()
            .relay_mode(RelayMode::Disabled) // 不用 Iroh Relay，走 VNT 虚拟网卡
            .alpns(vec![ALPN.to_vec()])
            .bind(port)
            .await
            .map_err(|e| format!("创建文件传输 Endpoint 失败: {}", e))?;

        let bound_port = endpoint.local_addr().0.port();
        log::info!("文件传输 Iroh Node: {} (端口 {})", endpoint.node_id(), bound_port);

        Ok(Self {
            endpoint,
            bound_port,
            tcp_port_hint: Arc::new(AtomicU16::new(0)),
        })
    }

    /// 本机 Node ID（hex 字符串）
    pub fn node_id(&self) -> String {
        self.endpoint.node_id().to_string()
    }

    /// 实际绑定端口
    pub fn bound_port(&self) -> u16 {
        self.bound_port
    }

    /// TCP 大文件监听端口共享句柄（daemon_listener 写入实际端口，探测响应读取公告）
    pub fn tcp_port_hint(&self) -> Arc<AtomicU16> {
        self.tcp_port_hint.clone()
    }

    /// 启动 UDP 身份探测服务器（固定 PROBE_PORT，响应本机 NodeId + QUIC 实际端口）
    pub async fn start_probe_server(&self) -> Result<tokio::task::JoinHandle<()>, String> {
        self.start_probe_server_on(PROBE_PORT).await
    }

    /// 指定端口启动探测服务器（生产用 [start_probe_server]；测试用独立端口）
    pub(crate) async fn start_probe_server_on(
        &self,
        port: u16,
    ) -> Result<tokio::task::JoinHandle<()>, String> {
        let socket = UdpSocket::bind((IpAddr::from([0, 0, 0, 0]), port))
            .await
            .map_err(|e| format!("绑定文件传输探测端口 {} 失败: {}", port, e))?;
        log::info!("文件传输身份探测监听端口 {}", port);

        let node_id = self.node_id();
        let quic_port = self.bound_port();
        let tcp_port_hint = self.tcp_port_hint.clone();
        Ok(tokio::spawn(async move {
            let mut buf = [0u8; 256];
            loop {
                match socket.recv_from(&mut buf).await {
                    Ok((n, peer)) => {
                        if &buf[..n] == PROBE_MAGIC.as_bytes() {
                            // 响应格式：VNTFT1:{node_id}:{quic_port}:{tcp_port}
                            // tcp_port 为接收端 TCP 高速通道实际监听端口（未就绪时为 0）
                            let tcp_port = tcp_port_hint.load(Ordering::Acquire);
                            let reply = format!(
                                "{}{}:{}:{}\n",
                                PROBE_PREFIX, node_id, quic_port, tcp_port
                            );
                            if let Err(e) = socket.send_to(reply.as_bytes(), peer).await {
                                log::warn!("文件传输探测响应发送失败: {}", e);
                            }
                        }
                    }
                    Err(e) => {
                        log::warn!("文件传输探测监听错误: {}", e);
                        break;
                    }
                }
            }
        }))
    }

    /// UDP 探测对端：获取 NodeId + 实际 QUIC 端口 + TCP 高速通道端口
    pub async fn probe(&self, remote_ip: IpAddr) -> Result<ProbeInfo, String> {
        let (node_id, quic_port, tcp_port) = probe_remote_on(remote_ip, PROBE_PORT).await?;
        Ok(ProbeInfo { node_id, quic_port, tcp_port })
    }

    /// 连接到对端：先经 PROBE_PORT UDP 探测发现其 NodeId 与 QUIC 端口，再建立 QUIC 连接
    pub async fn connect(&self, remote_ip: IpAddr) -> Result<Arc<FileConnection>, String> {
        let (node_id, quic_port, _tcp_port) = probe_remote_on(remote_ip, PROBE_PORT).await?;
        self.connect_with_node_id(remote_ip, quic_port, node_id).await
    }

    /// 直接以已知 NodeId 连接（供内部与测试使用）
    pub async fn connect_with_node_id(
        &self,
        remote_ip: IpAddr,
        port: u16,
        node_id: NodeId,
    ) -> Result<Arc<FileConnection>, String> {
        let addr = SocketAddr::new(remote_ip, port);
        let node_addr = NodeAddr::from_parts(node_id, None, vec![addr]);
        log::info!("文件传输正在连接 {} (node_id={})...", addr, node_id);
        let conn = self
            .endpoint
            .connect(node_addr, ALPN)
            .await
            .map_err(|e| format!("文件传输连接失败: {}", e))?;
        log::info!("文件传输已连接到 {}", conn.remote_address());
        Ok(Arc::new(FileConnection::new(conn)))
    }

    /// 接受对端连接（监听循环中调用）
    pub async fn accept(&self) -> Result<Arc<FileConnection>, String> {
        let connecting = self
            .endpoint
            .accept()
            .await
            .ok_or_else(|| "没有待接受的文件传输连接".to_string())?;
        let conn = connecting
            .await
            .map_err(|e| format!("接受文件传输连接失败: {}", e))?;
        log::info!("接受文件传输连接来自 {}", conn.remote_address());
        Ok(Arc::new(FileConnection::new(conn)))
    }
}

/// 封装的单条连接（多路复用 uni stream）
pub struct FileConnection {
    inner: Connection,
}

impl FileConnection {
    pub fn new(inner: Connection) -> Self {
        Self { inner }
    }

    /// 远端地址
    pub fn remote_address(&self) -> SocketAddr {
        self.inner.remote_address()
    }

    /// 发送一条 FileMsg（[type][len][bincode] 独立 uni stream）
    pub async fn send_msg(&self, msg: &FileMsg) -> Result<(), String> {
        let payload = bincode::serialize(msg).map_err(|e| format!("序列化失败: {}", e))?;
        self.send_raw(&payload).await
    }

    /// 接收下一条 FileMsg（阻塞直到收到；连接关闭返回 Err）
    pub async fn recv_msg(&self) -> Result<FileMsg, String> {
        let mut stream = self
            .inner
            .accept_uni()
            .await
            .map_err(|e| format!("接收文件流失败: {}", e))?;

        let mut head = [0u8; 5];
        stream
            .read_exact(&mut head)
            .await
            .map_err(|e| format!("读取文件消息头失败: {}", e))?;
        if head[0] != TYPE_FILETRANSFER {
            return Err(format!("未知文件消息类型: {}", head[0]));
        }
        let len = u32::from_le_bytes([head[1], head[2], head[3], head[4]]) as usize;
        if len > MAX_FRAME_SIZE {
            return Err(format!("文件消息过大: {} bytes", len));
        }
        let mut payload = vec![0u8; len];
        stream
            .read_exact(&mut payload)
            .await
            .map_err(|e| format!("读取文件消息体失败: {}", e))?;
        bincode::deserialize(&payload).map_err(|e| format!("解析文件消息失败: {}", e))
    }

    /// 关闭连接（尽力通知对端）
    pub fn close(&self, reason: &str) {
        self.inner.close(0u8.into(), reason.as_bytes());
    }

    async fn send_raw(&self, payload: &[u8]) -> Result<(), String> {
        let mut stream = self
            .inner
            .open_uni()
            .await
            .map_err(|e| format!("打开文件流失败: {}", e))?;
        let len = (payload.len() as u32).to_le_bytes();
        let head = [TYPE_FILETRANSFER, len[0], len[1], len[2], len[3]];
        stream
            .write_all(&head)
            .await
            .map_err(|e| format!("写入文件消息头失败: {}", e))?;
        stream
            .write_all(payload)
            .await
            .map_err(|e| format!("写入文件消息体失败: {}", e))?;
        stream.finish().await.map_err(|e| format!("结束文件流失败: {}", e))?;
        Ok(())
    }
}

// ==================== UDP 身份探测 ====================

/// 向对端固定探测端口发送请求，获取其 (NodeId, QUIC 端口, TCP 端口)
/// 响应格式：VNTFT1:{node_id}:{quic_port}[:{tcp_port}]（tcp_port 可选，兼容旧版本）
async fn probe_remote_on(
    remote_ip: IpAddr,
    probe_port: u16,
) -> Result<(NodeId, u16, u16), String> {
    let local = UdpSocket::bind((IpAddr::from([0, 0, 0, 0]), 0))
        .await
        .map_err(|e| format!("探测 socket 创建失败: {}", e))?;
    local
        .send_to(PROBE_MAGIC.as_bytes(), (remote_ip, probe_port))
        .await
        .map_err(|e| format!("探测发送失败: {}", e))?;

    let mut buf = [0u8; 512];
    let (n, _peer) = tokio::time::timeout(PROBE_TIMEOUT, local.recv_from(&mut buf))
        .await
        .map_err(|_| {
            format!(
                "文件传输探测超时: {}:{}（被控端未启动文件接收，或防火墙拦截了 UDP {}）",
                remote_ip, probe_port, probe_port
            )
        })?
        .map_err(|e| format!("探测接收失败: {}", e))?;

    let text = std::str::from_utf8(&buf[..n])
        .map_err(|_| "探测响应非 UTF-8".to_string())?
        .trim();
    let body = text
        .strip_prefix(PROBE_PREFIX)
        .ok_or_else(|| format!("探测响应格式错误: {}", text))?;

    // node_id 为 64 位 hex，不含冒号；格式 node_id[:quic_port[:tcp_port]]
    let mut parts = body.split(':');
    let node_id_hex = parts.next().ok_or_else(|| format!("探测响应格式错误: {}", text))?;
    let quic_port = parts
        .next()
        .and_then(|p| p.parse::<u16>().ok())
        .filter(|p| *p > 0)
        .unwrap_or(DEFAULT_QUIC_PORT);
    let tcp_port = parts
        .next()
        .and_then(|p| p.parse::<u16>().ok())
        .unwrap_or(0);

    let node_id = NodeId::from_str(node_id_hex)
        .map_err(|e| format!("NodeId 解析失败: {}", e))?;
    Ok((node_id, quic_port, tcp_port))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::file_transfer::protocol::{
        FileAccept, FileMsg, TextMessage,
    };

    /// 双 endpoint 本地互连：探测 → QUIC 连接 → FileMsg 往返
    #[tokio::test]
    async fn two_endpoints_msg_roundtrip() {
        // 独立探测/QUIC 端口，避免与运行中的 vnt-gui 抢占
        const TEST_PROBE_PORT: u16 = 23249;
        let a = FileNetwork::new(43211).await.expect("A endpoint");
        let b = FileNetwork::new(44556).await.expect("B endpoint");

        let _probe_task = a
            .start_probe_server_on(TEST_PROBE_PORT)
            .await
            .expect("probe server");

        // B 探测 A 的 NodeId + QUIC 端口（A 未启动 TCP listener → tcp_port=0），建立连接
        let (node_id, quic_port, tcp_port) =
            probe_remote_on(IpAddr::from([127, 0, 0, 1]), TEST_PROBE_PORT)
                .await
                .expect("探测 A");
        assert_eq!(quic_port, a.bound_port());
        assert_eq!(tcp_port, 0, "未启动 TCP listener 时 tcp_port 应为 0");
        let conn_b = b
            .connect_with_node_id(IpAddr::from([127, 0, 0, 1]), quic_port, node_id)
            .await
            .expect("B→A 连接");
        let conn_a = a.accept().await.expect("A 接受连接");

        // B → A：Offer（含中文文件名）
        let offer = crate::file_transfer::protocol::FileOffer {
            transfer_id: 42,
            filename: "测试文件.pdf".into(),
            file_size: 123456,
            file_hash_hex: "abc123".into(),
            chunk_size: 65536,
            channel: crate::file_transfer::protocol::TransferChannel::Quic,
            sender_device: "test-b".into(),
            sender_ip: "10.26.0.4".into(),
        };
        conn_b.send_msg(&FileMsg::Offer(offer.clone())).await.expect("发送 Offer");
        match conn_a.recv_msg().await.expect("A 接收") {
            FileMsg::Offer(o) => {
                assert_eq!(o.filename, "测试文件.pdf");
                assert_eq!(o.transfer_id, 42);
            }
            other => panic!("期望 Offer，实际 {:?}", other),
        }

        // A → B：Accept
        conn_a
            .send_msg(&FileMsg::Accept(FileAccept { transfer_id: 42, resume_offset: 0 }))
            .await
            .expect("发送 Accept");
        match conn_b.recv_msg().await.expect("B 接收") {
            FileMsg::Accept(a) => assert_eq!(a.resume_offset, 0),
            other => panic!("期望 Accept，实际 {:?}", other),
        }

        // B → A：Chunk（含数据）
        conn_b
            .send_msg(&FileMsg::Chunk {
                header: crate::file_transfer::protocol::FileChunk {
                    transfer_id: 42,
                    offset: 0,
                    data_len: 5,
                },
                data: vec![1, 2, 3, 4, 5],
            })
            .await
            .expect("发送 Chunk");
        match conn_a.recv_msg().await.expect("A 接收 Chunk") {
            FileMsg::Chunk { header, data } => {
                assert_eq!(header.offset, 0);
                assert_eq!(data, vec![1, 2, 3, 4, 5]);
            }
            other => panic!("期望 Chunk，实际 {:?}", other),
        }

        // A → B：Text
        conn_a
            .send_msg(&FileMsg::Text(TextMessage {
                msg_id: 1,
                timestamp: 1719820800,
                text: "你好，VNT！".into(),
                from: "test-a".into(),
            }))
            .await
            .expect("发送 Text");
        match conn_b.recv_msg().await.expect("B 接收 Text") {
            FileMsg::Text(t) => assert_eq!(t.text, "你好，VNT！"),
            other => panic!("期望 Text，实际 {:?}", other),
        }

        conn_a.close("测试结束");
        conn_b.close("测试结束");
    }

    #[test]
    fn probe_response_parse() {
        let sk = iroh_net::key::SecretKey::from_bytes(&[3u8; 32]);
        let node_id = sk.public();

        // 新版：node_id:quic_port:tcp_port
        let reply = format!("{}{}:45233:34248\n", PROBE_PREFIX, node_id);
        let text = reply.trim();
        let body = text.strip_prefix(PROBE_PREFIX).unwrap();
        let mut parts = body.split(':');
        let parsed_id = NodeId::from_str(parts.next().unwrap()).unwrap();
        let quic: u16 = parts.next().unwrap().parse().unwrap();
        let tcp: u16 = parts.next().unwrap().parse().unwrap();
        assert_eq!(parsed_id, node_id);
        assert_eq!(quic, 45233);
        assert_eq!(tcp, 34248);

        // 旧版：node_id:quic_port（无 tcp_port → 解析为 0）
        let old = format!("{}{}:45234\n", PROBE_PREFIX, node_id);
        let body = old.trim().strip_prefix(PROBE_PREFIX).unwrap();
        let mut parts = body.split(':');
        assert_eq!(
            NodeId::from_str(parts.next().unwrap()).unwrap(),
            node_id
        );
        let quic: u16 = parts.next().unwrap().parse().unwrap();
        assert_eq!(quic, 45234);
        assert!(parts.next().is_none(), "旧格式无 tcp_port 段");

        // 错误前缀
        assert!("VNTDS2:abc".strip_prefix(PROBE_PREFIX).is_none());
    }
}
