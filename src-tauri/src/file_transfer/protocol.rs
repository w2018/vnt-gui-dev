//! 文件传输协议定义
//!
//! QUIC 通道上所有消息以 `FileMsg` 枚举经 bincode 序列化写入独立 uni stream，
//! 传输格式（与 desktop_share/network.rs 保持一致）：[u8 类型][u32 长度][bincode 载荷]。
//! TCP 通道（≥阈值大文件）握手使用 JSON 文本行，数据为原始字节流 + 尾部 32 字节 SHA-256。

use serde::{Deserialize, Serialize};

// ==================== 文件消息类型字节 ====================

pub const TYPE_FILE_OFFER: u8 = 10;       // 发起方 → 接收方：文件元数据
pub const TYPE_FILE_ACCEPT: u8 = 11;      // 接收方 → 发起方：接受
pub const TYPE_FILE_REJECT: u8 = 12;      // 接收方 → 发起方：拒绝
pub const TYPE_FILE_CHUNK: u8 = 13;       // 发起方 → 接收方：数据块
pub const TYPE_FILE_COMPLETE: u8 = 14;    // 发起方 → 接收方：发送完毕
pub const TYPE_FILE_VERIFY: u8 = 15;      // 接收方 → 发起方：校验结果
pub const TYPE_FILE_CANCEL: u8 = 16;      // 任意方 → 对方：取消
pub const TYPE_FILE_RESUME_REQ: u8 = 17;  // 发起方 → 接收方：请求续传
pub const TYPE_FILE_RESUME_ACK: u8 = 18;  // 接收方 → 发起方：续传偏移量
pub const TYPE_TEXT_MESSAGE: u8 = 19;     // 任意方 → 对方：文本传输

/// 单条消息最大长度（8 MiB，防恶意超大帧；QUIC 通道 64KB 分块远小于此）
pub const MAX_FRAME_SIZE: usize = 8 * 1024 * 1024;

// ==================== 消息结构 ====================

/// 文件传输元数据（Offer）
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FileOffer {
    pub transfer_id: u64,
    pub filename: String,
    pub file_size: u64,
    /// SHA-256（hex 编码，便于 JSON/TCP 握手传输）
    pub file_hash_hex: String,
    /// 建议块大小（字节）
    pub chunk_size: u32,
    /// 传输通道（QUIC 或 TCP）
    pub channel: TransferChannel,
    /// 发送方设备名
    pub sender_device: String,
    /// 发送方本机 VNT 虚拟 IP（接收端展示/存储用；
    /// QUIC 连接的远端地址可能是 IPv6 链路本地，不代表 VNT 虚拟 IP）
    #[serde(default)]
    pub sender_ip: String,
}

/// 传输通道选择
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub enum TransferChannel {
    /// Iroh QUIC 流（小文件）
    Quic,
    /// 裸 TCP 通道（大文件，TCP 监听端口）
    Tcp {
        /// 接收方 TCP 监听端口
        port: u16,
    },
}

impl TransferChannel {
    /// 是否为 TCP 高速通道
    pub fn is_tcp(&self) -> bool {
        matches!(self, TransferChannel::Tcp { .. })
    }
}

/// 接收方接受
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FileAccept {
    pub transfer_id: u64,
    /// 已接收字节数（断点续传时 > 0）
    pub resume_offset: u64,
}

/// 接收方拒绝
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FileReject {
    pub transfer_id: u64,
    pub reason: String,
}

/// 数据块头（数据体紧随其后，不参与 bincode）
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FileChunk {
    pub transfer_id: u64,
    pub offset: u64,
    /// 数据体长度
    pub data_len: u32,
}

/// 发送完毕通知
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FileComplete {
    pub transfer_id: u64,
}

/// 校验结果
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FileVerify {
    pub transfer_id: u64,
    pub ok: bool,
    pub expected_hash_hex: String,
    pub actual_hash_hex: String,
}

/// 取消传输
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FileCancel {
    pub transfer_id: u64,
    pub reason: String,
}

/// 续传请求
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FileResumeRequest {
    pub transfer_id: u64,
    pub filename: String,
    pub file_size: u64,
    pub file_hash_hex: String,
}

/// 续传确认（携带已接收偏移量）
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FileResumeAck {
    pub transfer_id: u64,
    pub resume_offset: u64,
}

/// 文本消息
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TextMessage {
    pub msg_id: u64,
    pub timestamp: u64,
    pub text: String,
    /// 发送方设备名
    pub from: String,
}

/// QUIC 通道统一消息（bincode 序列化）
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum FileMsg {
    Offer(FileOffer),
    Accept(FileAccept),
    Reject(FileReject),
    /// 数据块（data 为原始字节）
    Chunk { header: FileChunk, data: Vec<u8> },
    Complete(FileComplete),
    Verify(FileVerify),
    Cancel(FileCancel),
    ResumeRequest(FileResumeRequest),
    ResumeAck(FileResumeAck),
    Text(TextMessage),
}

// ==================== 传输方向 / 状态 ====================

/// 传输方向
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum TransferDirection {
    Send,
    Receive,
}

/// 传输状态
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum TransferStatus {
    /// 等待对方确认 / 等待用户确认
    Pending,
    /// 传输中
    Transferring,
    /// 已暂停（保留在传输中列表，可点「继续」断点续传）
    Paused,
    /// 完成
    Completed,
    /// 失败
    Failed,
    /// 取消
    Cancelled,
    /// 对方拒绝
    Rejected,
}

// ==================== 历史记录 ====================

/// 历史记录条目（transfer_history.json 持久化）
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TransferRecord {
    pub id: u64,
    pub transfer_id: u64,
    pub direction: TransferDirection,
    pub filename: String,
    pub file_size: u64,
    pub remote_ip: String,
    pub remote_device: String,
    pub channel: TransferChannel,
    pub status: TransferStatus,
    /// Unix 时间戳（秒）
    pub start_time: u64,
    pub end_time: Option<u64>,
    pub bytes_transferred: u64,
    /// 文件 SHA-256（hex，可选）
    pub file_hash: Option<String>,
    pub error_message: Option<String>,
    /// 文件完整路径（接收方 = 保存路径；发送方 = 源文件路径）
    pub file_path: Option<String>,
    /// 是否为秒传（接收端已有相同 md5 文件，跳过实际传输）
    #[serde(default)]
    pub quick_sent: bool,
    /// 平均传输速度（KB/s，传输完成时由字节数/耗时计算）
    #[serde(default)]
    pub avg_speed_kbps: Option<u64>,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 所有消息类型的 bincode serde 往返
    #[test]
    fn msg_serde_roundtrip() {
        let msgs = vec![
            FileMsg::Offer(FileOffer {
                transfer_id: 1,
                filename: "报告.pdf".into(),
                file_size: 2048,
                file_hash_hex: "deadbeef".into(),
                chunk_size: 65536,
                channel: TransferChannel::Quic,
                sender_device: "PC-A".into(),
                sender_ip: "10.26.0.3".into(),
            }),
            FileMsg::Accept(FileAccept { transfer_id: 1, resume_offset: 65536 }),
            FileMsg::Reject(FileReject { transfer_id: 1, reason: "用户拒绝".into() }),
            FileMsg::Chunk {
                header: FileChunk { transfer_id: 1, offset: 0, data_len: 4 },
                data: vec![1, 2, 3, 4],
            },
            FileMsg::Complete(FileComplete { transfer_id: 1 }),
            FileMsg::Verify(FileVerify {
                transfer_id: 1,
                ok: true,
                expected_hash_hex: "abc".into(),
                actual_hash_hex: "abc".into(),
            }),
            FileMsg::Cancel(FileCancel { transfer_id: 1, reason: "取消".into() }),
            FileMsg::ResumeRequest(FileResumeRequest {
                transfer_id: 1,
                filename: "a.bin".into(),
                file_size: 100,
                file_hash_hex: "ff".into(),
            }),
            FileMsg::ResumeAck(FileResumeAck { transfer_id: 1, resume_offset: 50 }),
            FileMsg::Text(TextMessage {
                msg_id: 9,
                timestamp: 1719820800,
                text: "你好".into(),
                from: "PC-B".into(),
            }),
        ];
        for m in &msgs {
            let bytes = bincode::serialize(m).expect("序列化失败");
            let back: FileMsg = bincode::deserialize(&bytes).expect("反序列化失败");
            assert_eq!(*m, back, "消息往返不一致: {:?}", m);
        }
    }

    /// 消息类型字节唯一性
    #[test]
    fn type_bytes_unique() {
        let mut set = std::collections::HashSet::new();
        for t in [
            TYPE_FILE_OFFER,
            TYPE_FILE_ACCEPT,
            TYPE_FILE_REJECT,
            TYPE_FILE_CHUNK,
            TYPE_FILE_COMPLETE,
            TYPE_FILE_VERIFY,
            TYPE_FILE_CANCEL,
            TYPE_FILE_RESUME_REQ,
            TYPE_FILE_RESUME_ACK,
            TYPE_TEXT_MESSAGE,
        ] {
            assert!(set.insert(t), "类型字节 {} 重复", t);
        }
        assert_eq!(set.len(), 10);
    }

    /// 通道判别
    #[test]
    fn channel_discriminant() {
        assert!(!TransferChannel::Quic.is_tcp());
        assert!(TransferChannel::Tcp { port: 34248 }.is_tcp());
    }

    /// TransferRecord JSON 持久化往返（history.json 格式）
    #[test]
    fn record_json_roundtrip() {
        let rec = TransferRecord {
            id: 1,
            transfer_id: 1001,
            direction: TransferDirection::Send,
            filename: "report.pdf".into(),
            file_size: 2097152,
            remote_ip: "10.26.0.4".into(),
            remote_device: "Office-PC".into(),
            channel: TransferChannel::Quic,
            status: TransferStatus::Completed,
            start_time: 1719820800,
            end_time: Some(1719820810),
            bytes_transferred: 2097152,
            file_hash: Some("aabbcc".into()),
            error_message: None,
            file_path: Some("D:\\接收\\report.pdf".into()),
            quick_sent: false,
            avg_speed_kbps: Some(2048),
        };
        let json = serde_json::to_string_pretty(&rec).expect("JSON 序列化失败");
        let back: TransferRecord = serde_json::from_str(&json).expect("JSON 反序列化失败");
        assert_eq!(rec, back);
    }
}
