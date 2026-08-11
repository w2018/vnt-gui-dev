//! 文件接收端
//!
//! - 规划：计算 .partial 断点偏移 + 默认保存路径
//! - QUIC：接收 Offer → 接收 Chunk → 写 .partial → 校验 → 重命名 → Verify
//! - TCP：读 JSON 握手 → 接收原始字节 + 尾部哈希 → 校验 → 重命名
//!
//! 校验统一在接收完成后对 .partial 全文重新计算 SHA-256（断点续传时
//! .partial 已含历史数据，从头计算才能得到完整文件哈希）。

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

use crate::file_transfer::filter::FileTypeFilter;
use crate::file_transfer::network::FileConnection;
use crate::file_transfer::protocol::{FileCancel, FileMsg, FileOffer, FileVerify};
use crate::file_transfer::resumable;
use crate::file_transfer::transfer_manager::CancelFlag;

/// 进度上报阈值（每 ~512KB）
const REPORT_BYTES: u64 = 512 * 1024;
/// 取消轮询间隔
const POLL_INTERVAL: Duration = Duration::from_millis(300);
/// TCP 读缓冲 256KB
const READ_BUFFER_SIZE: usize = 256 * 1024;

/// 接收规划（Offer 到达时计算）
#[derive(Debug, Clone)]
pub struct ReceivePlan {
    pub offer: FileOffer,
    /// 断点续传偏移量（.partial 已有字节数）
    pub resume_offset: u64,
    /// 默认保存路径下的 .partial 路径
    pub partial_path: PathBuf,
}

/// 检查文件类型是否被过滤规则允许
pub fn is_allowed(offer: &FileOffer, filter: &FileTypeFilter) -> bool {
    let ext = Path::new(&offer.filename)
        .extension()
        .unwrap_or_default()
        .to_string_lossy()
        .to_lowercase();
    filter.is_allowed(&ext)
}

/// 计算接收规划：最终保存路径 + 偏移量 = .partial 已有字节数
pub async fn plan_receive(offer: &FileOffer, save_path: &Path) -> Result<ReceivePlan, String> {
    let partial_path = partial_for(save_path);
    let resume_offset = resumable::validate_resume(&partial_path, offer.file_size).await?;
    Ok(ReceivePlan { offer: offer.clone(), resume_offset, partial_path })
}

/// 检查保存目录是否已有内容一致的同名文件（md5 匹配），命中返回该文件路径（可秒传）
/// 有 .partial 残留（本地版本不完整）时不判秒传，交给断点续传处理。
pub async fn find_duplicate(save_dir: &Path, offer: &FileOffer) -> Option<PathBuf> {
    let target = save_dir.join(&offer.filename);
    if !target.is_file() {
        return None;
    }
    if partial_for(&target).exists() {
        return None;
    }
    // 大小不一致 → 内容必不同，直接跳过全量哈希（避免大文件无谓计算）
    if let Ok(meta) = tokio::fs::metadata(&target).await {
        if meta.len() != offer.file_size {
            return None;
        }
    }
    match compute_file_hash(&target).await {
        Ok(hash) if hex::encode(hash) == offer.file_hash_hex => Some(target),
        _ => None,
    }
}

/// 接收执行前防覆盖：目标路径已存在同名文件且内容不同（非断点残留）→ 生成新文件名；
/// 内容一致（覆盖无损失）或存在 .partial 断点残留（走续传）→ 保持原路径。
pub async fn resolve_conflict(save_path: &Path, offer: &FileOffer) -> PathBuf {
    if !save_path.exists() || partial_for(save_path).exists() {
        return save_path.to_path_buf();
    }
    if let Ok(hash) = compute_file_hash(save_path).await {
        if hex::encode(hash) == offer.file_hash_hex {
            return save_path.to_path_buf(); // 内容一致，覆盖无损失
        }
    }
    let dir = save_path.parent().unwrap_or(Path::new("."));
    let name = save_path
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();
    resolve_save_path(dir, &name)
}

/// 解析最终保存路径：
/// - 目标不存在 → 原路径
/// - 已存在同名文件（内容不同，秒传未命中）→ 生成 原文件名(n).扩展名，避免覆盖原文件
pub fn resolve_save_path(save_dir: &Path, filename: &str) -> PathBuf {
    let target = save_dir.join(filename);
    if !target.exists() {
        return target;
    }
    let stem = Path::new(filename)
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| filename.to_string());
    let ext = Path::new(filename)
        .extension()
        .map(|e| format!(".{}", e.to_string_lossy()))
        .unwrap_or_default();
    let mut n = 1;
    loop {
        let candidate = save_dir.join(format!("{}({}){}", stem, n, ext));
        if !candidate.exists() {
            return candidate;
        }
        n += 1;
    }
}

/// 从最终保存路径派生 .partial 路径
pub fn partial_for(save_path: &Path) -> PathBuf {
    let mut s = save_path.as_os_str().to_os_string();
    s.push(".partial");
    PathBuf::from(s)
}

/// QUIC 接收循环：写数据 → Complete 后校验 → 重命名 → 发 Verify
pub async fn receive_quic(
    conn: &Arc<FileConnection>,
    plan: &ReceivePlan,
    save_path: &Path,
    cancel: CancelFlag,
    on_progress: &mut impl FnMut(u64),
) -> Result<u64, String> {
    let offer = &plan.offer;
    let partial = partial_for(save_path);
    // 若用户更改保存路径导致 .partial 不同，重新计算偏移量
    let resume_offset = if partial == plan.partial_path {
        plan.resume_offset
    } else {
        resumable::validate_resume(&partial, offer.file_size).await?
    };

    let mut file = resumable::open_partial(&partial).await?;
    let mut received = resume_offset;
    let mut since_report: u64 = 0;

    loop {
        // 循环顶部检查取消：数据持续到达时 select 的 sleep 分支会被 recv_msg 饿死，
        // 必须在循环顶部及时响应（否则接收端取消后发送端仍持续发送）
        if cancel.is_cancelled() {
            // 通知发送端立即停止（否则发送端继续发块直到连接超时）
            let _ = conn
                .send_msg(&FileMsg::Cancel(FileCancel {
                    transfer_id: offer.transfer_id,
                    reason: "接收端已取消".to_string(),
                }))
                .await;
            return Err("传输已取消".to_string());
        }
        tokio::select! {
            r = conn.recv_msg() => match r {
                Ok(FileMsg::Chunk { header, data }) if header.transfer_id == offer.transfer_id => {
                    if header.offset != received {
                        if header.offset < received {
                            continue; // 重复块（理论上不出现），跳过
                        }
                        return Err(format!(
                            "块偏移不连续: 期望 {} 实际 {}",
                            received, header.offset
                        ));
                    }
                    resumable::write_chunk(&mut file, &data, header.offset).await?;
                    received += data.len() as u64;
                    since_report += data.len() as u64;
                    if since_report >= REPORT_BYTES {
                        on_progress(received);
                        since_report = 0;
                    }
                }
                Ok(FileMsg::Complete(_)) => break,
                Ok(FileMsg::Cancel(c)) => {
                    // 回发确认：确保发送端在关闭连接前知道我们已收到取消/暂停消息，
                    // 避免发送端立即关闭连接导致"closed by peer"误删断点
                    let _ = conn
                        .send_msg(&FileMsg::Cancel(FileCancel {
                            transfer_id: offer.transfer_id,
                            reason: "ack".to_string(),
                        }))
                        .await;
                    return Err(format!("对方取消: {}", c.reason));
                }
                Ok(_) => continue,
                Err(e) => return Err(e),
            },
            _ = tokio::time::sleep(POLL_INTERVAL) => {
                if cancel.is_cancelled() {
                    // 通知发送端立即停止（否则发送端继续发块直到连接超时）
                    let _ = conn
                        .send_msg(&FileMsg::Cancel(FileCancel {
                            transfer_id: offer.transfer_id,
                            reason: "接收端已取消".to_string(),
                        }))
                        .await;
                    return Err("传输已取消".to_string());
                }
            }
        }
    }
    on_progress(received);
    drop(file);

    // 校验全文哈希
    let actual_hash = compute_file_hash(&partial).await?;
    let actual_hex = hex::encode(actual_hash);
    let ok = actual_hex == offer.file_hash_hex;
    let _ = conn
        .send_msg(&FileMsg::Verify(FileVerify {
            transfer_id: offer.transfer_id,
            ok,
            expected_hash_hex: offer.file_hash_hex.clone(),
            actual_hash_hex: actual_hex.clone(),
        }))
        .await;
    if !ok {
        return Err(format!(
            "文件校验失败: 期望 {} 实际 {}",
            offer.file_hash_hex, actual_hex
        ));
    }

    resumable::finalize(&partial, save_path).await?;
    Ok(received)
}

/// TCP 接收循环：写数据 → 尾部哈希 → 校验 → 重命名
pub async fn receive_tcp(
    stream: &mut TcpStream,
    offer: &FileOffer,
    save_path: &Path,
    cancel: CancelFlag,
    on_progress: &mut impl FnMut(u64),
) -> Result<u64, String> {
    let partial = partial_for(save_path);
    let resume_offset = resumable::validate_resume(&partial, offer.file_size).await?;

    let mut file = resumable::open_partial(&partial).await?;
    let mut buffer = vec![0u8; READ_BUFFER_SIZE];
    let mut received = resume_offset;
    let mut file_remaining = offer.file_size - resume_offset;
    let mut since_report: u64 = 0;

    while file_remaining > 0 {
        // 循环顶部检查取消：数据持续到达时 select 的 sleep 分支会被 read 饿死，
        // 必须在循环顶部及时响应（否则接收端取消后发送端仍持续发送）
        if cancel.is_cancelled() {
            // 关闭连接，让发送端立即感知（否则发送端继续写直到管道断开）
            let _ = stream.shutdown().await;
            return Err("传输已取消".to_string());
        }
        tokio::select! {
            n = stream.read(&mut buffer[..file_remaining.min(READ_BUFFER_SIZE as u64) as usize]) => {
                let n = n.map_err(|e| format!("读取数据失败: {}", e))?;
                if n == 0 {
                    return Err(format!(
                        "数据流提前结束（已收 {} 字节，期望 {} 字节）",
                        received, offer.file_size
                    ));
                }
                file.write_all(&buffer[..n])
                    .await
                    .map_err(|e| format!("写入文件失败: {}", e))?;
                received += n as u64;
                file_remaining -= n as u64;
                since_report += n as u64;
                if since_report >= REPORT_BYTES {
                    on_progress(received);
                    since_report = 0;
                }
            }
            _ = tokio::time::sleep(POLL_INTERVAL) => {
                if cancel.is_cancelled() {
                    // 关闭连接，让发送端立即感知（否则发送端继续写直到管道断开）
                    let _ = stream.shutdown().await;
                    return Err("传输已取消".to_string());
                }
            }
        }
    }
    file.flush().await.map_err(|e| format!("刷盘失败: {}", e))?;
    drop(file);
    on_progress(received);

    // 读尾部 32 字节 SHA-256 并校验
    let hash_bytes = crate::file_transfer::tcp_channel::TcpReceiver::read_exact_n(stream, 32)
        .await?;
    let actual_hash = compute_file_hash(&partial).await?;
    let expected_hex = hex::encode(hash_bytes);
    let actual_hex = hex::encode(actual_hash);
    if actual_hex != expected_hex {
        return Err(format!("文件校验失败: 期望 {} 实际 {}", expected_hex, actual_hex));
    }

    resumable::finalize(&partial, save_path).await?;
    Ok(received)
}

// ==================== 辅助 ====================

/// 计算文件全文 SHA-256
pub async fn compute_file_hash(path: &Path) -> Result<[u8; 32], String> {
    let mut file = tokio::fs::File::open(path)
        .await
        .map_err(|e| format!("打开文件失败: {}", e))?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; 256 * 1024];
    loop {
        let n = file
            .read(&mut buffer)
            .await
            .map_err(|e| format!("读取文件失败: {}", e))?;
        if n == 0 {
            break;
        }
        hasher.update(&buffer[..n]);
    }
    Ok(hasher.finalize().into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::file_transfer::filter::FilterMode;
    use crate::file_transfer::protocol::{
        FileAccept, FileCancel, FileChunk, FileMsg, FileOffer, TransferChannel,
    };
    use crate::file_transfer::sender::send_file_quic;
    use crate::file_transfer::sender::CHUNK_SIZE;
    use iroh_net::NodeId;
    use std::net::{IpAddr, Ipv4Addr};
    use std::str::FromStr;
    use std::time::Duration;

    fn make_offer(filename: &str, size: u64, hash: &str) -> FileOffer {
        FileOffer {
            transfer_id: 1,
            filename: filename.to_string(),
            file_size: size,
            file_hash_hex: hash.to_string(),
            chunk_size: CHUNK_SIZE as u32,
            channel: TransferChannel::Quic,
            sender_device: "test".into(),
            sender_ip: "10.26.0.9".into(),
        }
    }

    #[test]
    fn is_allowed_whitelist() {
        let filter = FileTypeFilter::default();
        let ok = make_offer("a.pdf", 10, "x");
        let bad = make_offer("b.exe", 10, "x");
        assert!(is_allowed(&ok, &filter));
        assert!(!is_allowed(&bad, &filter));
    }

    #[test]
    fn is_allowed_allow_all() {
        let filter = FileTypeFilter { mode: FilterMode::AllowAll, extensions: Default::default() };
        assert!(is_allowed(&make_offer("c.exe", 1, "x"), &filter));
    }

    #[tokio::test]
    async fn plan_receive_offsets() {
        let dir = tempfile::tempdir().expect("临时目录");
        let offer = make_offer("plan.bin", 1000, "deadbeef");

        // 无 .partial → 0
        let plan = plan_receive(&offer, &dir.path().join("plan.bin")).await.expect("plan");
        assert_eq!(plan.resume_offset, 0);
        assert_eq!(plan.partial_path, dir.path().join("plan.bin.partial"));

        // 已有 400 字节 .partial → 400
        tokio::fs::write(&plan.partial_path, vec![0u8; 400]).await.expect("写 partial");
        let plan2 = plan_receive(&offer, &dir.path().join("plan.bin")).await.expect("plan2");
        assert_eq!(plan2.resume_offset, 400);

        // 超限残留 → 删除归零
        tokio::fs::write(dir.path().join("plan.bin.partial"), vec![0u8; 2000]).await.unwrap();
        let plan3 = plan_receive(&offer, &dir.path().join("plan.bin")).await.expect("plan3");
        assert_eq!(plan3.resume_offset, 0);
    }

    #[tokio::test]
    async fn compute_file_hash_known() {
        let dir = tempfile::tempdir().expect("临时目录");
        let p = dir.path().join("h.txt");
        tokio::fs::write(&p, b"hello world").await.expect("写");
        let hash = compute_file_hash(&p).await.expect("哈希");
        assert_eq!(hex::encode(hash), "b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9");
    }

    /// QUIC 全链路端到端：发送 100KB 文件，接收校验一致
    #[tokio::test]
    async fn quic_full_transfer_end_to_end() {
        let dir = tempfile::tempdir().expect("临时目录");
        let src = dir.path().join("payload.bin");
        let payload: Vec<u8> = (0..100_000u32).map(|i| (i % 251) as u8).collect();
        tokio::fs::write(&src, &payload).await.expect("写源文件");

        let net_a = crate::file_transfer::network::FileNetwork::new(0).await.expect("A");
        let net_b = crate::file_transfer::network::FileNetwork::new(0).await.expect("B");
        let node_id = NodeId::from_str(&net_a.node_id()).expect("NodeId");
        let conn_b = net_b
            .connect_with_node_id(IpAddr::V4(Ipv4Addr::LOCALHOST), net_a.bound_port(), node_id)
            .await
            .expect("B→A");
        let conn_a = net_a.accept().await.expect("A 接受");

        // 接收端协程
        let recv_conn = conn_a.clone();
        let save = dir.path().join("out.bin");
        let save_clone = save.clone();
        let recv_task = tokio::spawn(async move {
            let offer = match recv_conn.recv_msg().await.expect("接收 Offer") {
                FileMsg::Offer(o) => o,
                other => panic!("期望 Offer，实际 {:?}", other),
            };
            let plan = plan_receive(&offer, &save_clone).await.expect("plan");
            assert_eq!(plan.resume_offset, 0);
            recv_conn
                .send_msg(&FileMsg::Accept(FileAccept {
                    transfer_id: offer.transfer_id,
                    resume_offset: plan.resume_offset,
                }))
                .await
                .expect("Accept");
            receive_quic(&recv_conn, &plan, &save_clone, CancelFlag::new(), &mut |_| {})
                .await
                .expect("接收")
        });

        // 发送端
        let result = send_file_quic(
            &conn_b,
            src.to_str().expect("路径"),
            "test-device",
            "10.26.0.9",
            CHUNK_SIZE,
            1,
            CancelFlag::new(),
            &mut |_| {},
        )
        .await
        .expect("发送");

        assert_eq!(result.bytes_sent, 100_000);
        let saved_bytes = recv_task.await.expect("接收任务");
        assert_eq!(saved_bytes, 100_000);
        let saved = tokio::fs::read(&save).await.expect("读结果");
        assert_eq!(saved, payload, "接收文件应与源一致");
    }

    /// 断点续传：已有 1000 字节 .partial → 发送方从偏移继续，最终文件完整
    #[tokio::test]
    async fn quic_resume_from_partial() {
        let dir = tempfile::tempdir().expect("临时目录");
        let src = dir.path().join("resume.bin");
        let payload: Vec<u8> = (0..50_000u32).map(|i| (i % 251) as u8).collect();
        tokio::fs::write(&src, &payload).await.expect("写源文件");

        // 预写 .partial（前 1000 字节），模拟中断残留
        let save = dir.path().join("resume.bin");
        tokio::fs::write(dir.path().join("resume.bin.partial"), &payload[..1000])
            .await
            .expect("预写 partial");

        let net_a = crate::file_transfer::network::FileNetwork::new(0).await.expect("A");
        let net_b = crate::file_transfer::network::FileNetwork::new(0).await.expect("B");
        let node_id = NodeId::from_str(&net_a.node_id()).expect("NodeId");
        let conn_b = net_b
            .connect_with_node_id(IpAddr::V4(Ipv4Addr::LOCALHOST), net_a.bound_port(), node_id)
            .await
            .expect("B→A");
        let conn_a = net_a.accept().await.expect("A 接受");

        let recv_conn = conn_a.clone();
        let save2 = save.clone();
        let recv_task = tokio::spawn(async move {
            let offer = match recv_conn.recv_msg().await.expect("接收 Offer") {
                FileMsg::Offer(o) => o,
                other => panic!("期望 Offer，实际 {:?}", other),
            };
            let plan = plan_receive(&offer, &save2).await.expect("plan");
            assert_eq!(plan.resume_offset, 1000, "应识别断点偏移 1000");
            recv_conn
                .send_msg(&FileMsg::Accept(FileAccept {
                    transfer_id: offer.transfer_id,
                    resume_offset: plan.resume_offset,
                }))
                .await
                .expect("Accept");
            receive_quic(&recv_conn, &plan, &save2, CancelFlag::new(), &mut |_| {})
                .await
                .expect("接收")
        });

        let result = send_file_quic(
            &conn_b,
            src.to_str().expect("路径"),
            "test-device",
            "10.26.0.9",
            CHUNK_SIZE,
            1,
            CancelFlag::new(),
            &mut |_| {},
        )
        .await
        .expect("发送");

        // 发送方应从 1000 开始（bytes_sent = 50000）
        assert_eq!(result.bytes_sent, 50_000);
        assert_eq!(recv_task.await.expect("接收任务"), 50_000);

        let saved = tokio::fs::read(&save).await.expect("读结果");
        assert_eq!(saved, payload, "断点续传后文件应完整");
    }

    /// TCP 全链路端到端：发送小文件走 TCP 通道，接收校验一致
    #[tokio::test]
    async fn tcp_full_transfer_end_to_end() {
        let dir = tempfile::tempdir().expect("临时目录");
        let src = dir.path().join("tcp.bin");
        let payload: Vec<u8> = (0..50_000u32).map(|i| (i % 253) as u8).collect();
        tokio::fs::write(&src, &payload).await.expect("写源文件");

        let receiver = crate::file_transfer::tcp_channel::TcpReceiver::bind(
            (IpAddr::V4(Ipv4Addr::LOCALHOST), 0).into(),
        )
        .await
        .expect("绑定");
        let port = receiver.local_addr().expect("地址").port();

        // 接收端协程
        let save = dir.path().join("tcp-out.bin");
        let save_clone = save.clone();
        let recv_task = tokio::spawn(async move {
            let mut stream = receiver.accept().await.expect("accept");
            let hs = crate::file_transfer::tcp_channel::TcpReceiver::read_handshake(&mut stream)
                .await
                .expect("握手");
            let offer = FileOffer {
                transfer_id: hs["transfer_id"].as_u64().unwrap_or(0),
                filename: hs["filename"].as_str().unwrap_or("unknown").to_string(),
                file_size: hs["file_size"].as_u64().unwrap_or(0),
                file_hash_hex: hs["file_hash"].as_str().unwrap_or("").to_string(),
                chunk_size: 0,
                channel: TransferChannel::Tcp { port },
                sender_device: hs["sender_device"].as_str().unwrap_or("").to_string(),
                sender_ip: hs["sender_ip"].as_str().unwrap_or("").to_string(),
            };
            let plan = plan_receive(&offer, &save_clone).await.expect("plan");
            crate::file_transfer::tcp_channel::TcpReceiver::send_response(
                &mut stream,
                &serde_json::json!({ "type": "accept", "resume_offset": plan.resume_offset }),
            )
            .await
            .expect("响应");
            receive_tcp(&mut stream, &offer, &save_clone, CancelFlag::new(), &mut |_| {})
                .await
                .expect("TCP 接收")
        });

        // 发送端
        let result = crate::file_transfer::sender::send_file_tcp(
            "127.0.0.1",
            port,
            src.to_str().expect("路径"),
            "test",
            "10.26.0.9",
            1,
            CancelFlag::new(),
            &mut |_| {},
        )
        .await
        .expect("TCP 发送");

        assert_eq!(result.bytes_sent, 50_000);
        assert_eq!(recv_task.await.expect("接收任务"), 50_000);
        let saved = tokio::fs::read(&save).await.expect("读结果");
        assert_eq!(saved, payload, "TCP 接收文件应与源一致");
    }

    /// partial_for 派生路径
    #[test]
    fn partial_path_derivation() {
        let p = Path::new("D:\\下载\\报告.pdf");
        let partial = partial_for(p);
        assert_eq!(partial.to_string_lossy(), "D:\\下载\\报告.pdf.partial");
    }

    /// FileChunk 往返（receiver 依赖的协议消息）
    #[test]
    fn chunk_msg_roundtrip() {
        let msg = FileMsg::Chunk {
            header: FileChunk { transfer_id: 9, offset: 0, data_len: 3 },
            data: vec![1, 2, 3],
        };
        let bytes = bincode::serialize(&msg).expect("序列化");
        let back: FileMsg = bincode::deserialize(&bytes).expect("反序列化");
        assert_eq!(msg, back);
    }

    /// 秒传检测：同名且内容一致 → 命中；内容不同/无文件 → 未命中
    #[tokio::test]
    async fn find_duplicate_matches_md5() {
        let dir = tempfile::tempdir().expect("临时目录");
        let content = b"hello world";
        let target = dir.path().join("dup.txt");
        tokio::fs::write(&target, content).await.expect("写文件");

        // 命中：内容一致
        let hash = hex::encode(compute_file_hash(&target).await.unwrap());
        let offer = make_offer("dup.txt", content.len() as u64, &hash);
        assert_eq!(find_duplicate(dir.path(), &offer).await, Some(target.clone()));

        // 内容不同 → 未命中
        let bad = make_offer("dup.txt", content.len() as u64, "deadbeef");
        assert_eq!(find_duplicate(dir.path(), &bad).await, None);

        // 大小不一致 → 未命中（无需哈希比对）
        let wrong_size = make_offer("dup.txt", content.len() as u64 + 1, &hash);
        assert_eq!(find_duplicate(dir.path(), &wrong_size).await, None);

        // 无同名文件 → None
        let other = make_offer("missing.txt", 1, "x");
        assert_eq!(find_duplicate(dir.path(), &other).await, None);

        // 存在 .partial 残留（不完整）→ 不判秒传
        tokio::fs::write(dir.path().join("dup.txt.partial"), b"partial").await.unwrap();
        assert_eq!(find_duplicate(dir.path(), &offer).await, None);
    }

    /// 同名文件解析：无冲突原路径；已存在 → 原文件名(1)(2)…递增
    #[test]
    fn resolve_save_path_renames() {
        let dir = tempfile::tempdir().expect("临时目录");
        // 目标不存在 → 原路径
        assert_eq!(resolve_save_path(dir.path(), "a.txt"), dir.path().join("a.txt"));

        // 已存在同名 → a(1).txt
        std::fs::write(dir.path().join("a.txt"), b"x").unwrap();
        assert_eq!(resolve_save_path(dir.path(), "a.txt"), dir.path().join("a(1).txt"));

        // 继续冲突 → a(2).txt
        std::fs::write(dir.path().join("a(1).txt"), b"x").unwrap();
        assert_eq!(resolve_save_path(dir.path(), "a.txt"), dir.path().join("a(2).txt"));

        // 无扩展名文件 → 名(1)
        std::fs::write(dir.path().join("README"), b"x").unwrap();
        assert_eq!(resolve_save_path(dir.path(), "README"), dir.path().join("README(1)"));
    }

    /// 接收前防覆盖：内容不同 → 新路径；内容一致 → 保持原路径
    #[tokio::test]
    async fn resolve_conflict_preserves_or_renames() {
        let dir = tempfile::tempdir().expect("临时目录");
        let content = b"same";
        let target = dir.path().join("c.bin");
        tokio::fs::write(&target, content).await.unwrap();

        // 内容不同 → 新路径 c(1).bin
        let bad = make_offer("c.bin", content.len() as u64, "deadbeef");
        assert_eq!(resolve_conflict(&target, &bad).await, dir.path().join("c(1).bin"));

        // 内容一致 → 保持原路径（覆盖无损失）
        let hash = hex::encode(compute_file_hash(&target).await.unwrap());
        let same = make_offer("c.bin", content.len() as u64, &hash);
        assert_eq!(resolve_conflict(&target, &same).await, target);
    }

    /// 问题①验证：接收端收到 Accept 后立即发 Cancel("已暂停")，
    /// 发送端（send_file_quic）必须在发送循环中监听到对端消息并立即停止，
    /// 而不是继续发完整个文件。
    #[tokio::test]
    async fn quic_sender_stops_on_peer_cancel() {
        let dir = tempfile::tempdir().expect("临时目录");
        let src = dir.path().join("payload.bin");
        // 1MB 文件：若发送端未监听对端，会持续发送直到完成（时间足够断言"未发完"）
        let payload: Vec<u8> = (0..1_000_000u32).map(|i| (i % 251) as u8).collect();
        tokio::fs::write(&src, &payload).await.expect("写源文件");

        let net_a = crate::file_transfer::network::FileNetwork::new(0).await.expect("A");
        let net_b = crate::file_transfer::network::FileNetwork::new(0).await.expect("B");
        let node_id = NodeId::from_str(&net_a.node_id()).expect("NodeId");
        let conn_b = net_b
            .connect_with_node_id(IpAddr::V4(Ipv4Addr::LOCALHOST), net_a.bound_port(), node_id)
            .await
            .expect("B→A");
        let conn_a = net_a.accept().await.expect("A 接受");

        // 接收端协程：收到 Offer → 发 Accept → 立即发 Cancel("已暂停")
        let recv_conn = conn_a.clone();
        let recv_task = tokio::spawn(async move {
            let offer = match recv_conn.recv_msg().await.expect("接收 Offer") {
                FileMsg::Offer(o) => o,
                other => panic!("期望 Offer，实际 {:?}", other),
            };
            recv_conn
                .send_msg(&FileMsg::Accept(FileAccept {
                    transfer_id: offer.transfer_id,
                    resume_offset: 0,
                }))
                .await
                .expect("Accept");
            // 收到第一个 chunk 后发 Cancel，模拟接收端终止
            match recv_conn.recv_msg().await.expect("接收 Chunk") {
                FileMsg::Chunk { .. } => {
                    recv_conn
                        .send_msg(&FileMsg::Cancel(FileCancel {
                            transfer_id: offer.transfer_id,
                            reason: "已暂停".to_string(),
                        }))
                        .await
                        .expect("发送 Cancel");
                }
                other => panic!("期望 Chunk，实际 {:?}", other),
            }
            // 再收一个消息：应为发送端发来的 ack（或超时）
            tokio::time::timeout(Duration::from_secs(2), recv_conn.recv_msg())
                .await
                .ok();
        });

        // 发送端：应在收到 Cancel 后立即返回 Err，而不是发完全部 1MB
        let start = std::time::Instant::now();
        let result = send_file_quic(
            &conn_b,
            src.to_str().expect("路径"),
            "test-device",
            "10.26.0.9",
            CHUNK_SIZE,
            1,
            CancelFlag::new(),
            &mut |_| {},
        )
        .await;

        let elapsed = start.elapsed();
        // 发送端应因"对方取消: 已暂停"返回错误
        match result {
            Err(e) => {
                assert!(
                    e.contains("对方取消"),
                    "发送端应收到对方取消，实际错误: {}",
                    e
                );
            }
            Ok(r) => panic!(
                "发送端不应成功发送（对方已暂停），实际 bytes_sent={} elapsed={:?}",
                r.bytes_sent, elapsed
            ),
        }
        // 应远小于完整发送 1MB 的时间（完整发送通常 <1s，取消应在几百 ms 内触发）
        assert!(
            elapsed < Duration::from_secs(10),
            "发送端应快速停止，实际耗时 {:?}",
            elapsed
        );
        let _ = recv_task.await.expect("接收任务");
    }

    /// 问题②验证：接收端 receive_quic 收到对端 Cancel("已暂停") 时，
    /// 返回 Err("对方取消: 已暂停") 且 .partial 文件保留（供断点续传）。
    #[tokio::test]
    async fn quic_receiver_preserves_partial_on_pause() {
        let dir = tempfile::tempdir().expect("临时目录");
        let src = dir.path().join("payload.bin");
        let payload: Vec<u8> = (0..200_000u32).map(|i| (i % 251) as u8).collect();
        tokio::fs::write(&src, &payload).await.expect("写源文件");

        let net_a = crate::file_transfer::network::FileNetwork::new(0).await.expect("A");
        let net_b = crate::file_transfer::network::FileNetwork::new(0).await.expect("B");
        let node_id = NodeId::from_str(&net_a.node_id()).expect("NodeId");
        let conn_b = net_b
            .connect_with_node_id(IpAddr::V4(Ipv4Addr::LOCALHOST), net_a.bound_port(), node_id)
            .await
            .expect("B→A");
        let conn_a = net_a.accept().await.expect("A 接受");

        // 接收端：收 Offer → Accept → receive_quic
        let save = dir.path().join("out.bin");
        let save_clone = save.clone();
        let recv_task = tokio::spawn(async move {
            let offer = match conn_a.recv_msg().await.expect("接收 Offer") {
                FileMsg::Offer(o) => o,
                other => panic!("期望 Offer，实际 {:?}", other),
            };
            let plan = plan_receive(&offer, &save_clone).await.expect("plan");
            conn_a
                .send_msg(&FileMsg::Accept(FileAccept {
                    transfer_id: offer.transfer_id,
                    resume_offset: plan.resume_offset,
                }))
                .await
                .expect("Accept");
            let r = receive_quic(&conn_a, &plan, &save_clone, CancelFlag::new(), &mut |_| {})
                .await;
            (r, plan.partial_path)
        });

        // 发送端方向：先发 Offer，等 Accept，然后发一个 chunk + Cancel("已暂停")
        let offer = FileOffer {
            transfer_id: 1,
            filename: "payload.bin".to_string(),
            file_size: payload.len() as u64,
            file_hash_hex: hex::encode({
                let mut h = Sha256::new();
                h.update(&payload);
                h.finalize()
            }),
            chunk_size: CHUNK_SIZE as u32,
            channel: TransferChannel::Quic,
            sender_device: "test-device".to_string(),
            sender_ip: "10.26.0.9".to_string(),
        };
        conn_b
            .send_msg(&FileMsg::Offer(offer.clone()))
            .await
            .expect("发 Offer");
        match conn_b.recv_msg().await.expect("等 Accept") {
            FileMsg::Accept(_) => {}
            other => panic!("期望 Accept，实际 {:?}", other),
        }
        // 发一个 chunk
        conn_b
            .send_msg(&FileMsg::Chunk {
                header: FileChunk {
                    transfer_id: 1,
                    offset: 0,
                    data_len: 1024,
                },
                data: vec![7u8; 1024],
            })
            .await
            .expect("发 Chunk");
        // 发 Cancel("已暂停") 模拟发送端暂停
        conn_b
            .send_msg(&FileMsg::Cancel(FileCancel {
                transfer_id: 1,
                reason: "已暂停".to_string(),
            }))
            .await
            .expect("发 Cancel");

        // 接收端应返回 Err("对方取消: 已暂停")，且 .partial 保留
        let (result, partial_path) = recv_task.await.expect("接收任务");
        let err = result.expect_err("应返回对方取消错误");
        assert!(
            err.contains("对方取消") && err.contains("暂停"),
            "错误应包含对方取消+暂停，实际: {}",
            err
        );
        assert!(
            partial_path.exists(),
            "暂停后 .partial 应保留（供断点续传）"
        );
    }

    /// 完整暂停链路验证：发送端 send_file_quic 发送大文件，中途 cancel（模拟 file_pause），
    /// 发送端返回 Err 后按 run_send 暂停分支逻辑发 Cancel("已暂停") 并等待接收端 ack，
    /// 接收端 receive_quic 应返回 Err("对方取消: 已暂停") 且 .partial 保留（供续传）。
    #[tokio::test]
    async fn quic_pause_full_flow_preserves_partial() {
        let dir = tempfile::tempdir().expect("临时目录");
        let src = dir.path().join("big.bin");
        // 大文件（8MB）确保发送需要时间，中途 cancel 能生效
        let payload: Vec<u8> = (0..8_000_000u32).map(|i| (i % 251) as u8).collect();
        tokio::fs::write(&src, &payload).await.expect("写源文件");

        let net_a = crate::file_transfer::network::FileNetwork::new(0).await.expect("A");
        let net_b = crate::file_transfer::network::FileNetwork::new(0).await.expect("B");
        let node_id = NodeId::from_str(&net_a.node_id()).expect("NodeId");
        let conn_b = net_b
            .connect_with_node_id(IpAddr::V4(Ipv4Addr::LOCALHOST), net_a.bound_port(), node_id)
            .await
            .expect("B→A");
        let conn_a = net_a.accept().await.expect("A 接受");

        // 接收端协程（模拟 daemon_listener 侧）：收 Offer → Accept → receive_quic
        let save = dir.path().join("out.bin");
        let save_clone = save.clone();
        let partial_check = dir.path().join("out.bin.partial");
        let recv_task = tokio::spawn(async move {
            let offer = match conn_a.recv_msg().await.expect("接收 Offer") {
                FileMsg::Offer(o) => o,
                other => panic!("期望 Offer，实际 {:?}", other),
            };
            let plan = plan_receive(&offer, &save_clone).await.expect("plan");
            conn_a
                .send_msg(&FileMsg::Accept(FileAccept {
                    transfer_id: offer.transfer_id,
                    resume_offset: plan.resume_offset,
                }))
                .await
                .expect("Accept");
            receive_quic(&conn_a, &plan, &save_clone, CancelFlag::new(), &mut |_| {})
                .await
        });

        // 发送端 cancel flag（模拟 file_pause 触发）
        let send_cancel = CancelFlag::new();
        let send_cancel2 = send_cancel.clone();
        let conn_b2 = conn_b.clone();
        let src2 = src.clone();
        let send_task = tokio::spawn(async move {
            // 发送协程（模拟 run_send 里 send_file_quic）：发送大文件
            send_file_quic(
                &conn_b2,
                src2.to_str().expect("路径"),
                "test-device",
                "10.26.0.9",
                CHUNK_SIZE,
                1,
                send_cancel2,
                &mut |_| {},
            )
            .await
        });

        // 短暂等待后触发暂停（模拟用户点击暂停 → file_pause 置 Paused + cancel flag）
        tokio::time::sleep(Duration::from_millis(150)).await;
        send_cancel.cancel();
        let send_result = send_task.await.expect("发送任务结束");
        // 发送端应返回 Err（传输已取消）
        assert!(send_result.is_err(), "发送端应因暂停返回 Err");

        // 模拟 run_send 暂停分支：任务已 Paused → 发 Cancel("已暂停") 给接收端并等待 ack
        conn_b
            .send_msg(&FileMsg::Cancel(FileCancel {
                transfer_id: 1,
                reason: "已暂停".to_string(),
            }))
            .await
            .expect("发暂停通知");
        // 等待接收端 ack（reason=ack）
        tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                match conn_b.recv_msg().await {
                    Ok(FileMsg::Cancel(c)) if c.transfer_id == 1 && c.reason == "ack" => break,
                    Ok(_) => continue,
                    Err(e) => panic!("等 ack 失败: {}", e),
                }
            }
        })
        .await
        .expect("应收到接收端 ack");

        // 接收端应返回 Err("对方取消: 已暂停")
        let recv_result = recv_task.await.expect("接收任务结束");
        let err = recv_result.expect_err("接收端应返回暂停错误");
        assert!(
            err.contains("对方取消") && err.contains("暂停"),
            "接收端错误应含对方取消+暂停，实际: {}",
            err
        );
        // .partial 必须保留（供断点续传）
        assert!(
            partial_check.exists(),
            "暂停后 .partial 必须保留，否则继续时从头重传"
        );
    }
}
