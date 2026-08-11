//! 传输管理器：队列管理、并发控制、传输 ID 分配、状态快照
//!
//! 任务统一存放于一个 Vec（前端按状态自行分组展示），支持按 transfer_id 更新/移除。

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use tokio::sync::{Mutex, Semaphore};

use crate::file_transfer::protocol::{
    TransferChannel, TransferDirection, TransferStatus,
};

/// 取消标记（传输协程协作式取消）
#[derive(Clone, Default)]
pub struct CancelFlag(Arc<AtomicBool>);

impl CancelFlag {
    pub fn new() -> Self {
        Self::default()
    }

    /// 触发取消
    pub fn cancel(&self) {
        self.0.store(true, Ordering::SeqCst);
    }

    /// 是否已取消
    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }
}

/// 单个传输任务的状态
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransferTask {
    pub transfer_id: u64,
    pub filename: String,
    pub file_size: u64,
    pub bytes_done: u64,
    pub direction: TransferDirection,
    pub channel: TransferChannel,
    pub status: TransferStatus,
    pub remote_ip: String,
    pub remote_device: String,
    pub error_message: Option<String>,
    /// 实时速率（KB/s，前端展示）
    pub speed_kbps: Option<u64>,
    /// 预计剩余秒数（前端展示）
    pub eta_seconds: Option<u64>,
    /// 创建时间（Unix 秒）
    pub created_at: u64,
    /// 发送方：源文件路径
    pub file_path: Option<String>,
    /// 接收方：目标保存路径
    pub save_path: Option<String>,
    /// 断点续传偏移量（接收方确认时写入）
    pub resume_offset: u64,
    /// 是否为秒传（接收端已有相同 md5 文件，跳过实际传输）
    pub quick_sent: bool,
    /// 上次进度更新时间（不序列化，用于实时速度/剩余时间计算）
    #[serde(skip)]
    pub last_progress_time: Option<Instant>,
}

/// 传输管理器
pub struct TransferManager {
    /// 全部任务（pending/transferring/completed/...）
    tasks: Mutex<Vec<TransferTask>>,
    /// 下一个传输 ID
    next_id: Mutex<u64>,
    /// 最大并发传输数（信号量）
    permits: Arc<Semaphore>,
    /// 通道大小阈值（字节，≥ 此值走 TCP；原子，运行时可调）
    size_threshold: AtomicU64,
}

impl TransferManager {
    pub fn new(max_concurrent: usize, size_threshold: u64) -> Self {
        Self {
            tasks: Mutex::new(Vec::new()),
            next_id: Mutex::new(1),
            permits: Arc::new(Semaphore::new(max_concurrent.max(1))),
            size_threshold: AtomicU64::new(size_threshold),
        }
    }

    /// 更新通道阈值（运行即时生效）
    pub fn set_size_threshold(&self, bytes: u64) {
        self.size_threshold.store(bytes, Ordering::Relaxed);
    }

    /// 分配新传输 ID
    pub async fn next_transfer_id(&self) -> u64 {
        let mut id = self.next_id.lock().await;
        let ret = *id;
        *id += 1;
        ret
    }

    /// 添加任务（返回分配的 transfer_id）
    pub async fn enqueue(&self, mut task: TransferTask) -> u64 {
        let id = self.next_transfer_id().await;
        task.transfer_id = id;
        if task.created_at == 0 {
            task.created_at = unix_now();
        }
        self.tasks.lock().await.push(task);
        id
    }

    /// 获取并发许可（队列串行，最多 max_concurrent 个并发）
    pub fn acquire_permit(&self) -> Arc<Semaphore> {
        self.permits.clone()
    }

    /// 获取当前所有任务快照（UI 展示）
    pub async fn snapshot(&self) -> Vec<TransferTask> {
        self.tasks.lock().await.clone()
    }

    /// 按 transfer_id 查找任务
    pub async fn find(&self, transfer_id: u64) -> Option<TransferTask> {
        self.tasks
            .lock()
            .await
            .iter()
            .find(|t| t.transfer_id == transfer_id)
            .cloned()
    }

    /// 更新任务状态（仅在存在时应用）
    pub async fn update(
        &self,
        transfer_id: u64,
        updater: impl FnOnce(&mut TransferTask),
    ) -> bool {
        let mut tasks = self.tasks.lock().await;
        if let Some(task) = tasks.iter_mut().find(|t| t.transfer_id == transfer_id) {
            updater(task);
            true
        } else {
            false
        }
    }

    /// 移除任务
    pub async fn remove(&self, transfer_id: u64) {
        let mut tasks = self.tasks.lock().await;
        tasks.retain(|t| t.transfer_id != transfer_id);
    }

    /// 自动选择通道
    pub fn select_channel(&self, file_size: u64) -> TransferChannel {
        if file_size >= self.size_threshold.load(Ordering::Relaxed) {
            TransferChannel::Tcp {
                port: crate::file_transfer::tcp_channel::DEFAULT_PORT,
            }
        } else {
            TransferChannel::Quic
        }
    }

    /// 通道阈值（字节）
    pub fn size_threshold(&self) -> u64 {
        self.size_threshold.load(Ordering::Relaxed)
    }
}

/// 当前 Unix 时间戳（秒）
pub fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// 基于上次进度更新时间计算实时速度（KB/s）与剩余时间，并推进进度
pub fn update_speed(task: &mut TransferTask, done: u64) {
    let now = Instant::now();
    if let Some(prev) = task.last_progress_time {
        let dt = now.duration_since(prev).as_secs_f64();
        // 间隔过短（<100ms）不做速率计算，避免抖动；进度仍推进
        if dt >= 0.1 {
            let dbytes = (done as i64 - task.bytes_done as i64).max(0) as u64;
            let kbps = if dt > 0.0 { (dbytes as f64 / 1024.0 / dt) as u64 } else { 0 };
            if kbps > 0 {
                task.speed_kbps = Some(kbps);
                let remaining = task.file_size.saturating_sub(done);
                task.eta_seconds = Some((remaining as f64 / (kbps as f64 * 1024.0)).ceil() as u64);
            }
        }
    }
    task.last_progress_time = Some(now);
    task.bytes_done = done;
    task.status = TransferStatus::Transferring;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_task() -> TransferTask {
        TransferTask {
            transfer_id: 0,
            filename: "a.bin".into(),
            file_size: 1000,
            bytes_done: 0,
            direction: TransferDirection::Send,
            channel: TransferChannel::Quic,
            status: TransferStatus::Pending,
            remote_ip: "10.26.0.4".into(),
            remote_device: "PC".into(),
            error_message: None,
            speed_kbps: None,
            eta_seconds: None,
            created_at: 0,
            file_path: None,
            save_path: None,
            resume_offset: 0,
            quick_sent: false,
            last_progress_time: None,
        }
    }

    #[tokio::test]
    async fn transfer_id_monotonic() {
        let mgr = TransferManager::new(3, 100);
        let a = mgr.next_transfer_id().await;
        let b = mgr.next_transfer_id().await;
        let c = mgr.next_transfer_id().await;
        assert_eq!(a, 1);
        assert_eq!(b, 2);
        assert_eq!(c, 3);
    }

    #[tokio::test]
    async fn enqueue_snapshot_update_remove() {
        let mgr = TransferManager::new(3, 100);
        let id = mgr.enqueue(make_task()).await;
        assert_eq!(id, 1);

        let snap = mgr.snapshot().await;
        assert_eq!(snap.len(), 1);
        assert_eq!(snap[0].status, TransferStatus::Pending);

        // 更新
        assert!(mgr.update(id, |t| { t.status = TransferStatus::Transferring; }).await);
        assert_eq!(mgr.snapshot().await[0].status, TransferStatus::Transferring);

        // 更新不存在的 id 返回 false
        assert!(!mgr.update(999, |_| {}).await);

        // 移除
        mgr.remove(id).await;
        assert!(mgr.snapshot().await.is_empty());
    }

    #[tokio::test]
    async fn enqueue_assigns_created_at() {
        let mgr = TransferManager::new(3, 100);
        let id = mgr.enqueue(make_task()).await;
        let task = mgr.find(id).await.expect("任务存在");
        assert!(task.created_at > 0);
    }

    #[tokio::test]
    async fn select_channel_by_threshold() {
        // 阈值 100 字节
        let mgr = TransferManager::new(3, 100);
        assert!(matches!(mgr.select_channel(99), TransferChannel::Quic));
        match mgr.select_channel(100) {
            TransferChannel::Tcp { port } => {
                assert_eq!(port, crate::file_transfer::tcp_channel::DEFAULT_PORT);
            }
            _ => panic!("100 字节应走 TCP"),
        }
        assert!(matches!(mgr.select_channel(1_000_000), TransferChannel::Tcp { .. }));
    }
}
