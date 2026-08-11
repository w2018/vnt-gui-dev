//! 裸 TCP 大文件通道
//!
//! ≥ 阈值（默认 100MB）的文件走此通道，直接绑定 VNT 虚拟 IP 建立 TCP 连接，
//! 绕过 QUIC 拥塞控制开销以跑满带宽。
//! 协议：
//!   1. 握手（JSON 文本行）：offer → accept{resume_offset} / reject{reason}
//!   2. 数据：原始字节流（发送方从 resume_offset 开始发送）
//!   3. 尾部：32 字节 SHA-256 校验和

use std::net::SocketAddr;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

/// TCP 大文件监听端口（可配置）
pub const DEFAULT_PORT: u16 = 34248;
/// 接收缓冲区大小（256KB）
const READ_BUFFER_SIZE: usize = 256 * 1024;

// ==================== 发送端 ====================

/// TCP 发送端
pub struct TcpSender {
    stream: TcpStream,
}

impl TcpSender {
    /// 连接到接收方（VNT 虚拟 IP:端口）
    pub async fn connect(remote_ip: &str, remote_port: u16) -> Result<Self, String> {
        let addr: SocketAddr = format!("{}:{}", remote_ip, remote_port)
            .parse()
            .map_err(|e| format!("无效地址 {}:{}: {}", remote_ip, remote_port, e))?;
        let stream = TcpStream::connect(addr)
            .await
            .map_err(|e| format!("TCP 连接失败 {}: {}", addr, e))?;
        Ok(Self { stream })
    }

    /// 发送一行文本（握手 JSON）
    pub async fn send_line(&mut self, line: &str) -> Result<(), String> {
        self.stream
            .write_all(line.as_bytes())
            .await
            .map_err(|e| format!("发送握手失败: {}", e))?;
        self.stream
            .write_all(b"\n")
            .await
            .map_err(|e| format!("发送握手失败: {}", e))?;
        self.stream.flush().await.map_err(|e| format!("刷新失败: {}", e))?;
        Ok(())
    }

    /// 读取一行文本（响应 JSON）
    pub async fn read_line(&mut self) -> Result<String, String> {
        let mut buf = Vec::new();
        let mut byte = [0u8; 1];
        loop {
            let n = self
                .stream
                .read(&mut byte)
                .await
                .map_err(|e| format!("读取失败: {}", e))?;
            if n == 0 {
                return Err("连接被对端关闭".to_string());
            }
            if byte[0] == b'\n' {
                break;
            }
            buf.push(byte[0]);
        }
        String::from_utf8(buf).map_err(|e| format!("无效 UTF-8: {}", e))
    }

    /// 发送原始数据
    pub async fn send_data(&mut self, data: &[u8]) -> Result<(), String> {
        self.stream
            .write_all(data)
            .await
            .map_err(|e| format!("发送数据失败: {}", e))?;
        Ok(())
    }

    /// 关闭连接（发送 FIN）
    pub async fn close(&mut self) -> Result<(), String> {
        self.stream.shutdown().await.map_err(|e| format!("关闭失败: {}", e))?;
        Ok(())
    }
}

// ==================== 接收端 ====================

/// TCP 接收端（监听 + 单连接处理）
pub struct TcpReceiver {
    listener: TcpListener,
}

impl TcpReceiver {
    /// 绑定到指定地址（VNT 虚拟 IP 或 0.0.0.0）
    pub async fn bind(addr: SocketAddr) -> Result<Self, String> {
        let listener = TcpListener::bind(addr)
            .await
            .map_err(|e| format!("TCP 绑定失败 {}: {}", addr, e))?;
        Ok(Self { listener })
    }

    /// 监听地址
    pub fn local_addr(&self) -> Result<SocketAddr, String> {
        self.listener.local_addr().map_err(|e| format!("获取监听地址失败: {}", e))
    }

    /// 接受下一个连接
    pub async fn accept(&self) -> Result<TcpStream, String> {
        let (stream, _) = self
            .listener
            .accept()
            .await
            .map_err(|e| format!("TCP accept 失败: {}", e))?;
        Ok(stream)
    }

    /// 读取握手（JSON 文本行）
    pub async fn read_handshake(
        stream: &mut TcpStream,
    ) -> Result<serde_json::Value, String> {
        let mut buf = Vec::new();
        let mut byte = [0u8; 1];
        loop {
            let n = stream
                .read(&mut byte)
                .await
                .map_err(|e| format!("读取失败: {}", e))?;
            if n == 0 {
                return Err("连接在握手前关闭".to_string());
            }
            if byte[0] == b'\n' {
                break;
            }
            buf.push(byte[0]);
        }
        serde_json::from_slice(&buf).map_err(|e| format!("解析握手 JSON 失败: {}", e))
    }

    /// 发送响应（JSON 文本行）
    pub async fn send_response(
        stream: &mut TcpStream,
        json: &serde_json::Value,
    ) -> Result<(), String> {
        let line = format!("{}\n", serde_json::to_string(json).map_err(|e| e.to_string())?);
        stream
            .write_all(line.as_bytes())
            .await
            .map_err(|e| format!("发送响应失败: {}", e))?;
        stream.flush().await.map_err(|e| format!("刷新失败: {}", e))?;
        Ok(())
    }

    /// 读取恰好 N 字节（流提前关闭返回 Err）
    pub async fn read_exact_n(stream: &mut TcpStream, n: usize) -> Result<Vec<u8>, String> {
        let mut out = vec![0u8; n];
        let mut read = 0;
        while read < n {
            let k = stream
                .read(&mut out[read..])
                .await
                .map_err(|e| format!("读取数据失败: {}", e))?;
            if k == 0 {
                return Err(format!("连接提前关闭（已读 {} 字节，期望 {} 字节）", read, n));
            }
            read += k;
        }
        Ok(out)
    }

    /// 读取数据并写入文件，直到写满 expected_len 字节；随后读取尾部 32 字节哈希
    /// 返回 (已写入文件的实际字节数, 尾部哈希)
    pub async fn receive_file_and_hash(
        stream: &mut TcpStream,
        file: &mut tokio::fs::File,
        expected_len: u64,
    ) -> Result<(u64, [u8; 32]), String> {
        let mut buffer = vec![0u8; READ_BUFFER_SIZE];
        let mut received: u64 = 0;

        while received < expected_len {
            let remaining = (expected_len - received) as usize;
            let to_read = remaining.min(READ_BUFFER_SIZE);
            let n = stream
                .read(&mut buffer[..to_read])
                .await
                .map_err(|e| format!("读取数据失败: {}", e))?;
            if n == 0 {
                return Err(format!(
                    "数据流提前结束（已收 {} 字节，期望 {} 字节）",
                    received, expected_len
                ));
            }
            file.write_all(&buffer[..n])
                .await
                .map_err(|e| format!("写入文件失败: {}", e))?;
            received += n as u64;
        }
        file.flush().await.map_err(|e| format!("刷盘失败: {}", e))?;

        // 读取尾部 32 字节 SHA-256
        let hash_bytes = Self::read_exact_n(stream, 32).await?;
        let mut hash = [0u8; 32];
        hash.copy_from_slice(&hash_bytes);
        Ok((received, hash))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 本机回环：TCP 握手 → 数据 → 尾部哈希完整往返
    #[tokio::test]
    async fn tcp_handshake_data_hash_roundtrip() {
        use std::net::{IpAddr, Ipv4Addr};

        let receiver = TcpReceiver::bind((IpAddr::V4(Ipv4Addr::LOCALHOST), 0).into())
            .await
            .expect("绑定");
        let listen_addr = receiver.local_addr().expect("监听地址");

        // 接收端协程
        let recv_task = tokio::spawn(async move {
            let mut stream = receiver.accept().await.expect("accept");

            // 读 offer 握手
            let offer = TcpReceiver::read_handshake(&mut stream).await.expect("握手");
            assert_eq!(offer["type"], "offer");
            assert_eq!(offer["filename"], "big.bin");
            assert_eq!(offer["file_size"], 100);

            // 响应 accept（resume_offset=0）
            TcpReceiver::send_response(
                &mut stream,
                &serde_json::json!({ "type": "accept", "resume_offset": 0 }),
            )
            .await
            .expect("响应");

            // 接收 100 字节数据 + 尾部哈希
            let tmp = tempfile::tempdir().expect("临时目录");
            let path = tmp.path().join("out.bin");
            let mut file = tokio::fs::File::create(&path).await.expect("创建");
            let (received, hash) =
                TcpReceiver::receive_file_and_hash(&mut stream, &mut file, 100)
                    .await
                    .expect("接收");
            assert_eq!(received, 100);
            assert_eq!(hash, [7u8; 32]);
        });

        // 发送端
        let mut sender = TcpSender::connect("127.0.0.1", listen_addr.port()).await.expect("连接");
        sender
            .send_line(&serde_json::json!({
                "type": "offer",
                "filename": "big.bin",
                "file_size": 100,
                "file_hash": "07".repeat(32),
            })
            .to_string())
            .await
            .expect("发送 offer");

        let resp = sender.read_line().await.expect("读响应");
        let resp_json: serde_json::Value = serde_json::from_str(&resp).expect("解析");
        assert_eq!(resp_json["resume_offset"], 0);

        // 发送 100 字节 + 32 字节哈希
        let payload = vec![1u8; 100];
        sender.send_data(&payload).await.expect("数据");
        sender.send_data(&[7u8; 32]).await.expect("哈希");
        sender.close().await.expect("关闭");

        recv_task.await.expect("接收端任务");
    }

    /// 发送方读到 reject 响应
    #[tokio::test]
    async fn tcp_reject_response() {
        use std::net::{IpAddr, Ipv4Addr};
        let receiver = TcpReceiver::bind((IpAddr::V4(Ipv4Addr::LOCALHOST), 0).into())
            .await
            .expect("绑定");
        let port = receiver.local_addr().expect("地址").port();

        let recv_task = tokio::spawn(async move {
            let mut stream = receiver.accept().await.expect("accept");
            let _offer = TcpReceiver::read_handshake(&mut stream).await.expect("握手");
            TcpReceiver::send_response(
                &mut stream,
                &serde_json::json!({ "type": "reject", "reason": "用户拒绝" }),
            )
            .await
            .expect("响应");
        });

        let mut sender = TcpSender::connect("127.0.0.1", port).await.expect("连接");
        sender
            .send_line(&serde_json::json!({ "type": "offer" }).to_string())
            .await
            .expect("offer");
        let resp = sender.read_line().await.expect("读响应");
        let v: serde_json::Value = serde_json::from_str(&resp).expect("解析");
        assert_eq!(v["type"], "reject");
        assert_eq!(v["reason"], "用户拒绝");

        recv_task.await.expect("接收端任务");
    }
}
