//! 文件发送端
//!
//! 支持两条通道：
//! - QUIC：Offer → Accept(断点偏移) → 分块 Chunk → Complete → Verify
//! - TCP：JSON 握手 → Accept(断点偏移) → 原始字节流 → 尾部 32 字节 SHA-256
//!
//! 发送前全文计算 SHA-256（校验用），传输全程可协作取消。

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncSeekExt, SeekFrom};

use crate::file_transfer::network::FileConnection;
use crate::file_transfer::protocol::{
    FileChunk, FileComplete, FileMsg, FileOffer, FileVerify, TransferChannel,
};
use crate::file_transfer::transfer_manager::CancelFlag;
use crate::file_transfer::tcp_channel::TcpSender;

/// QUIC 默认块大小：64KB（QUIC 友好）
pub const CHUNK_SIZE: usize = 64 * 1024;
/// TCP 块大小：256KB（减少系统调用开销）
pub const TCP_CHUNK_SIZE: usize = 256 * 1024;
/// 取消/接收轮询间隔
const POLL_INTERVAL: Duration = Duration::from_millis(300);
/// 等待对方确认（Accept/Verify）超时
const CONFIRM_TIMEOUT: Duration = Duration::from_secs(30);
/// QUIC 进度上报阈值（每 ~512KB）
const QUIC_REPORT_BYTES: u64 = 512 * 1024;
/// TCP 进度上报阈值（每 ~1MB）
const TCP_REPORT_BYTES: u64 = 1024 * 1024;

/// 发送结果
#[derive(Debug, Clone)]
pub struct SendResult {
    pub bytes_sent: u64,
    pub file_hash_hex: String,
    pub resume_offset: u64,
    /// 是否为秒传（接收端已有相同 md5 文件，跳过实际传输）
    pub quick_sent: bool,
}

/// 发送文件（按通道分发）
pub async fn send_file(
    file_path: &str,
    remote_ip: &str,
    channel: TransferChannel,
    task_id: u64,
    cancel: CancelFlag,
    conn: Option<&Arc<FileConnection>>,
    mut on_progress: impl FnMut(u64) + Send,
) -> Result<SendResult, String> {
    // 发送者身份 = 本机设备名（不能传目标设备名，否则接收端显示的发送者名错误）
    let sender_device = crate::file_transfer::local_device_name();
    // 发送方本机 VNT 虚拟 IP（接收端展示用，避免 QUIC 连接远端地址为 IPv6 链路本地）
    let sender_ip = crate::file_transfer::local_vnt_ip().await.unwrap_or_default();
    match channel {
        TransferChannel::Quic => {
            let conn = conn.ok_or_else(|| "QUIC 连接未建立".to_string())?;
            send_file_quic(conn, file_path, &sender_device, &sender_ip, CHUNK_SIZE, task_id, cancel, &mut on_progress).await
        }
        TransferChannel::Tcp { port } => {
            send_file_tcp(remote_ip, port, file_path, &sender_device, &sender_ip, task_id, cancel, &mut on_progress).await
        }
    }
}

/// QUIC 通道发送
pub async fn send_file_quic(
    conn: &Arc<FileConnection>,
    file_path: &str,
    sender_device: &str,
    sender_ip: &str,
    chunk_size: usize,
    task_id: u64,
    cancel: CancelFlag,
    on_progress: &mut impl FnMut(u64),
) -> Result<SendResult, String> {
    // 1. 计算文件哈希与大小
    let (file_size, hash) = compute_hash(file_path).await?;
    let hash_hex = hex::encode(hash);
    let filename = basename(file_path);

    // 2. 发送 Offer
    let offer = FileOffer {
        transfer_id: task_id,
        filename,
        file_size,
        file_hash_hex: hash_hex.clone(),
        chunk_size: chunk_size as u32,
        channel: TransferChannel::Quic,
        sender_device: sender_device.to_string(),
        sender_ip: sender_ip.to_string(),
    };
    conn.send_msg(&FileMsg::Offer(offer)).await?;

    // 3. 等待 Accept（含断点偏移量，可取消）
    let accept = wait_for_accept(conn, task_id, &cancel).await?;
    let resume_offset = if accept.resume_offset > file_size {
        file_size
    } else {
        accept.resume_offset
    };
    // 秒传：接收端已有相同 md5 文件（resume_offset = file_size），跳过数据发送
    let quick_sent = resume_offset >= file_size;

    // 4. 打开文件，seek 到断点
    let mut file = tokio::fs::File::open(file_path)
        .await
        .map_err(|e| format!("打开文件失败: {}", e))?;
    if resume_offset > 0 {
        file.seek(SeekFrom::Start(resume_offset))
            .await
            .map_err(|e| format!("Seek 失败: {}", e))?;
    }

    // 5. 分块发送（发送每个块时同时监听对端 Cancel/Reject，收到立即停止）
    let mut buffer = vec![0u8; chunk_size.max(1)];
    let mut offset = resume_offset;
    let mut since_report: u64 = 0;

    while offset < file_size {
        if cancel.is_cancelled() {
            return Err("传输已取消".to_string());
        }
        let remaining = (file_size - offset) as usize;
        let to_read = remaining.min(chunk_size);
        let n = file
            .read(&mut buffer[..to_read])
            .await
            .map_err(|e| format!("读取文件失败: {}", e))?;
        if n == 0 {
            break;
        }
        // 发送本块；同时监听对端消息（接收端终止 → 立即停止发送）。
        // 内层循环保证本块发送成功（收到无关消息仅继续等待，不重发本块）
        let chunk = FileMsg::Chunk {
            header: FileChunk {
                transfer_id: task_id,
                offset,
                data_len: n as u32,
            },
            data: buffer[..n].to_vec(),
        };
        loop {
            tokio::select! {
                r = conn.send_msg(&chunk) => {
                    r.map_err(|e| format!("发送数据块失败: {}", e))?;
                    break;
                }
                r = conn.recv_msg() => match r {
                    Ok(FileMsg::Cancel(c)) => return Err(format!("对方取消: {}", c.reason)),
                    Ok(FileMsg::Reject(rj)) => return Err(format!("对方拒绝: {}", rj.reason)),
                    Ok(_) => continue,
                    Err(e) => return Err(e),
                },
            }
        }
        offset += n as u64;
        since_report += n as u64;
        if since_report >= QUIC_REPORT_BYTES {
            on_progress(offset);
            since_report = 0;
        }
    }
    on_progress(offset);

    // 6. 发送 Complete
    conn.send_msg(&FileMsg::Complete(FileComplete { transfer_id: task_id })).await?;

    // 7. 等待校验结果（可取消）
    let verify = wait_for_verify(conn, task_id, &cancel).await?;
    if !verify.ok {
        return Err(format!(
            "文件校验失败: 期望 {}，实际 {}",
            verify.expected_hash_hex, verify.actual_hash_hex
        ));
    }

    Ok(SendResult {
        bytes_sent: offset,
        file_hash_hex: hash_hex,
        resume_offset,
        quick_sent,
    })
}

/// TCP 通道发送
pub async fn send_file_tcp(
    remote_ip: &str,
    remote_port: u16,
    file_path: &str,
    sender_device: &str,
    sender_ip: &str,
    task_id: u64,
    cancel: CancelFlag,
    on_progress: &mut impl FnMut(u64),
) -> Result<SendResult, String> {
    // 1. 计算文件哈希与大小
    let (file_size, hash) = compute_hash(file_path).await?;
    let hash_hex = hex::encode(hash);
    let filename = basename(file_path);

    // 2. 建立 TCP 连接
    let mut sender = TcpSender::connect(remote_ip, remote_port).await?;

    // 3. 发送握手（JSON 文本行）
    let handshake = serde_json::json!({
        "type": "offer",
        "transfer_id": task_id,
        "filename": filename,
        "file_size": file_size,
        "file_hash": hash_hex,
        "sender_device": sender_device,
        "sender_ip": sender_ip,
        "channel": "tcp",
    });
    sender.send_line(&handshake.to_string()).await?;

    // 4. 等待响应（可取消）
    let response = loop {
        tokio::select! {
            r = sender.read_line() => match r {
                Ok(line) => break line,
                Err(e) => return Err(e),
            },
            _ = tokio::time::sleep(POLL_INTERVAL) => {
                if cancel.is_cancelled() {
                    return Err("传输已取消".to_string());
                }
            }
        }
    };
    let resp_json: serde_json::Value = serde_json::from_str(&response)
        .map_err(|e| format!("解析响应失败: {}", e))?;
    let ty = resp_json["type"].as_str().unwrap_or("");
    if ty == "reject" {
        return Err(format!(
            "对方拒绝: {}",
            resp_json["reason"].as_str().unwrap_or("未知原因")
        ));
    }
    if ty != "accept" {
        return Err(format!("响应格式错误: {}", response));
    }
    let resume_offset = resp_json["resume_offset"].as_u64().unwrap_or(0).min(file_size);

    // 5. 秒传：对方已有完整文件（resume_offset = file_size）→ 无需传输，直接完成
    if resume_offset >= file_size {
        log::info!("秒传命中: {} 已在对方存在，跳过传输", filename);
        let _ = sender.close().await;
        return Ok(SendResult {
            bytes_sent: file_size,
            file_hash_hex: hash_hex,
            resume_offset,
            quick_sent: true,
        });
    }

    // 6. 打开文件，seek 到断点
    let mut file = tokio::fs::File::open(file_path)
        .await
        .map_err(|e| format!("打开文件失败: {}", e))?;
    if resume_offset > 0 {
        file.seek(SeekFrom::Start(resume_offset))
            .await
            .map_err(|e| format!("Seek 失败: {}", e))?;
    }

    // 6. 原始字节流发送
    let mut buffer = vec![0u8; TCP_CHUNK_SIZE];
    let mut offset = resume_offset;
    let mut since_report: u64 = 0;

    while offset < file_size {
        if cancel.is_cancelled() {
            return Err("传输已取消".to_string());
        }
        let remaining = (file_size - offset) as usize;
        let to_read = remaining.min(TCP_CHUNK_SIZE);
        let n = file
            .read(&mut buffer[..to_read])
            .await
            .map_err(|e| format!("读取文件失败: {}", e))?;
        if n == 0 {
            break;
        }
        sender.send_data(&buffer[..n]).await?;
        offset += n as u64;
        since_report += n as u64;
        if since_report >= TCP_REPORT_BYTES {
            on_progress(offset);
            since_report = 0;
        }
    }
    on_progress(offset);

    // 7. 发送尾部 SHA-256 校验和并关闭
    sender.send_data(&hash).await?;
    sender.close().await?;

    Ok(SendResult {
        bytes_sent: offset,
        file_hash_hex: hash_hex,
        resume_offset,
        quick_sent: false,
    })
}

// ==================== 辅助 ====================

/// 计算文件 SHA-256 与大小（发送前全文读取）
async fn compute_hash(file_path: &str) -> Result<(u64, [u8; 32]), String> {
    let mut file = tokio::fs::File::open(file_path)
        .await
        .map_err(|e| format!("打开文件失败: {}", e))?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; 256 * 1024];
    let mut total_size = 0u64;

    loop {
        let n = file
            .read(&mut buffer)
            .await
            .map_err(|e| format!("读取文件失败: {}", e))?;
        if n == 0 {
            break;
        }
        hasher.update(&buffer[..n]);
        total_size += n as u64;
    }

    let hash: [u8; 32] = hasher.finalize().into();
    Ok((total_size, hash))
}

/// 从路径提取文件名（Unicode 安全）
fn basename(file_path: &str) -> String {
    Path::new(file_path)
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| file_path.to_string())
}

/// 等待对方 Accept（可取消，整体 30 秒超时）
async fn wait_for_accept(
    conn: &Arc<FileConnection>,
    task_id: u64,
    cancel: &CancelFlag,
) -> Result<crate::file_transfer::protocol::FileAccept, String> {
    let fut = async {
        loop {
            tokio::select! {
                r = conn.recv_msg() => match r {
                    Ok(FileMsg::Accept(a)) if a.transfer_id == task_id => return Ok(a),
                    Ok(FileMsg::Reject(rj)) => return Err(format!("对方拒绝: {}", rj.reason)),
                    Ok(FileMsg::Cancel(c)) => return Err(format!("对方取消: {}", c.reason)),
                    Ok(_) => continue,
                    Err(e) => return Err(e),
                },
                _ = tokio::time::sleep(POLL_INTERVAL) => {
                    if cancel.is_cancelled() {
                        return Err("传输已取消".to_string());
                    }
                }
            }
        }
    };
    match tokio::time::timeout(CONFIRM_TIMEOUT, fut).await {
        Ok(r) => r,
        Err(_) => Err("等待对方确认超时（对方未响应）".to_string()),
    }
}

/// 等待对方 Verify（可取消，整体 30 秒超时）
async fn wait_for_verify(
    conn: &Arc<FileConnection>,
    task_id: u64,
    cancel: &CancelFlag,
) -> Result<FileVerify, String> {
    let fut = async {
        loop {
            tokio::select! {
                r = conn.recv_msg() => match r {
                    Ok(FileMsg::Verify(v)) if v.transfer_id == task_id => return Ok(v),
                    Ok(FileMsg::Cancel(c)) => return Err(format!("对方取消: {}", c.reason)),
                    Ok(_) => continue,
                    Err(e) => return Err(e),
                },
                _ = tokio::time::sleep(POLL_INTERVAL) => {
                    if cancel.is_cancelled() {
                        return Err("传输已取消".to_string());
                    }
                }
            }
        }
    };
    match tokio::time::timeout(CONFIRM_TIMEOUT, fut).await {
        Ok(r) => r,
        Err(_) => Err("等待对方校验超时".to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn basename_extracts_filename() {
        assert_eq!(basename("D:\\下载\\报告.pdf"), "报告.pdf");
        assert_eq!(basename("C:/tmp/a b.bin"), "a b.bin");
        assert_eq!(basename("plain.txt"), "plain.txt");
    }

    /// compute_hash 对空文件返回 0 大小与合法哈希
    #[tokio::test]
    async fn hash_empty_file() {
        let dir = tempfile::tempdir().expect("临时目录");
        let path = dir.path().join("empty.bin");
        tokio::fs::write(&path, b"").await.expect("写空文件");
        let (size, hash) = compute_hash(path.to_str().unwrap()).await.expect("哈希");
        assert_eq!(size, 0);
        // 空文件 SHA-256 = e3b0c44298fc1c149afbf4c8996fb924...
        assert_eq!(hex::encode(hash), "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855");
    }

    /// compute_hash 已知内容
    #[tokio::test]
    async fn hash_known_content() {
        let dir = tempfile::tempdir().expect("临时目录");
        let path = dir.path().join("hello.txt");
        tokio::fs::write(&path, b"hello world").await.expect("写");
        let (size, hash) = compute_hash(path.to_str().unwrap()).await.expect("哈希");
        assert_eq!(size, 11);
        // sha256("hello world") = b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9
        assert_eq!(hex::encode(hash), "b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9");
    }

    #[test]
    fn cancel_flag_works() {
        let flag = CancelFlag::new();
        assert!(!flag.is_cancelled());
        flag.cancel();
        assert!(flag.is_cancelled());
    }

    #[test]
    fn default_port_constant() {
        assert_eq!(crate::file_transfer::tcp_channel::DEFAULT_PORT, 34248);
    }
}
