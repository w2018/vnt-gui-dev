//! 历史记录持久化（transfer_history.json）
//!
//! 永久保存收发记录，支持查询（方向/关键字过滤、按开始时间倒序）、
//! 单条删除、批量删除、清空。写入采用原子操作（先写 .tmp 再 rename）。

use std::path::{Path, PathBuf};
use std::sync::Arc;

use tokio::sync::Mutex;

use crate::file_transfer::protocol::{TransferDirection, TransferRecord};

/// 历史记录管理器
pub struct HistoryStore {
    path: PathBuf,
    records: Arc<Mutex<Vec<TransferRecord>>>,
    /// 下一条记录 ID（单调递增，用于 UI 主键）
    next_id: Arc<Mutex<u64>>,
}

impl HistoryStore {
    /// 加载历史记录（文件缺失/损坏时返回空）
    pub fn load(history_dir: &Path) -> Self {
        let path = history_dir.join("transfer_history.json");
        let (records, max_id) = if path.exists() {
            match std::fs::read_to_string(&path) {
                Ok(content) => match serde_json::from_str::<Vec<TransferRecord>>(&content) {
                    Ok(recs) => {
                        let max_id = recs.iter().map(|r| r.id).max().unwrap_or(0);
                        (recs, max_id)
                    }
                    Err(e) => {
                        log::warn!("历史记录解析失败（忽略损坏文件）: {}", e);
                        (Vec::new(), 0)
                    }
                },
                Err(_) => (Vec::new(), 0),
            }
        } else {
            (Vec::new(), 0)
        };
        Self {
            path,
            records: Arc::new(Mutex::new(records)),
            next_id: Arc::new(Mutex::new(max_id + 1)),
        }
    }

    /// 新增一条记录（分配自增 id），返回分配的 id
    pub async fn push(&self, mut record: TransferRecord) -> Result<u64, String> {
        let mut next = self.next_id.lock().await;
        record.id = *next;
        *next += 1;
        drop(next);

        let mut records = self.records.lock().await;
        records.push(record);
        let id = records.last().map(|r| r.id).unwrap_or(0);
        self.persist(&records).await?;
        Ok(id)
    }

    /// 更新/追加（按 transfer_id 匹配）；不存在则追加
    pub async fn upsert(&self, record: TransferRecord) -> Result<(), String> {
        let mut records = self.records.lock().await;
        if let Some(existing) = records.iter_mut().find(|r| r.transfer_id == record.transfer_id) {
            *existing = record;
        } else {
            let mut next = self.next_id.lock().await;
            let mut rec = record;
            rec.id = *next;
            *next += 1;
            drop(next);
            records.push(rec);
        }
        self.persist(&records).await
    }

    /// 删除单条记录（按持久化自增 id，跨会话唯一）
    /// 注意：不能用 transfer_id —— 传输 ID 每次启动从 1 重置，
    /// 跨会话历史记录的 transfer_id 会重复，导致选择/删除错乱。
    pub async fn delete(&self, id: u64) -> Result<(), String> {
        let mut records = self.records.lock().await;
        records.retain(|r| r.id != id);
        self.persist(&records).await
    }

    /// 批量删除，返回删除条数（按持久化自增 id）
    pub async fn delete_many(&self, ids: &[u64]) -> Result<usize, String> {
        let mut records = self.records.lock().await;
        let before = records.len();
        records.retain(|r| !ids.contains(&r.id));
        let removed = before - records.len();
        self.persist(&records).await?;
        Ok(removed)
    }

    /// 清空全部
    pub async fn clear_all(&self) -> Result<(), String> {
        let mut records = self.records.lock().await;
        records.clear();
        self.persist(&records).await
    }

    /// 查询（按开始时间倒序，limit 截断）
    pub async fn query(
        &self,
        direction: Option<TransferDirection>,
        keyword: Option<&str>,
        limit: usize,
    ) -> Vec<TransferRecord> {
        let records = self.records.lock().await;
        let mut result: Vec<TransferRecord> = records
            .iter()
            .filter(|r| {
                if let Some(dir) = direction {
                    if r.direction != dir {
                        return false;
                    }
                }
                if let Some(kw) = keyword {
                    let kw = kw.to_lowercase();
                    if !r.filename.to_lowercase().contains(&kw)
                        && !r.remote_ip.to_lowercase().contains(&kw)
                    {
                        return false;
                    }
                }
                true
            })
            .cloned()
            .collect();
        result.sort_by(|a, b| b.start_time.cmp(&a.start_time));
        result.truncate(limit.max(1));
        result
    }

    /// 全部记录条数
    pub async fn count(&self) -> usize {
        self.records.lock().await.len()
    }

    /// 原子持久化：先写 .tmp 再 rename
    async fn persist(&self, records: &[TransferRecord]) -> Result<(), String> {
        let json = serde_json::to_string_pretty(records)
            .map_err(|e| format!("序列化失败: {}", e))?;
        let tmp = self.path.with_extension("json.tmp");
        tokio::fs::write(&tmp, json)
            .await
            .map_err(|e| format!("写入临时文件失败: {}", e))?;
        tokio::fs::rename(&tmp, &self.path)
            .await
            .map_err(|e| format!("落盘失败: {}", e))?;
        Ok(())
    }
}

// 供测试构造记录
#[cfg(test)]
fn make_record(transfer_id: u64, filename: &str, start: u64) -> TransferRecord {
    TransferRecord {
        id: 0, // push 时分配
        transfer_id,
        direction: TransferDirection::Send,
        filename: filename.to_string(),
        file_size: 1000,
        remote_ip: "10.26.0.4".to_string(),
        remote_device: "Office-PC".to_string(),
        channel: crate::file_transfer::protocol::TransferChannel::Quic,
        status: crate::file_transfer::protocol::TransferStatus::Completed,
        start_time: start,
        end_time: Some(start + 10),
        bytes_transferred: 1000,
        file_hash: None,
        error_message: None,
        file_path: None,
        quick_sent: false,
        avg_speed_kbps: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::file_transfer::protocol::TransferDirection;

    #[tokio::test]
    async fn push_and_query() {
        let dir = tempfile::tempdir().expect("临时目录");
        let store = HistoryStore::load(dir.path());

        let id1 = store.push(make_record(1001, "a.txt", 100)).await.expect("push");
        let id2 = store.push(make_record(1002, "b.pdf", 200)).await.expect("push");
        assert_eq!(id1, 1);
        assert_eq!(id2, 2);

        // 全部（按时间倒序）
        let all = store.query(None, None, 100).await;
        assert_eq!(all.len(), 2);
        assert_eq!(all[0].filename, "b.pdf"); // 较新的在前

        // 关键字
        let kw = store.query(None, Some("a.txt"), 100).await;
        assert_eq!(kw.len(), 1);
        assert_eq!(kw[0].filename, "a.txt");

        // 方向过滤（全部为 Send）
        let recv = store.query(Some(TransferDirection::Receive), None, 100).await;
        assert_eq!(recv.len(), 0);
    }

    #[tokio::test]
    async fn persist_and_reload() {
        let dir = tempfile::tempdir().expect("临时目录");
        {
            let store = HistoryStore::load(dir.path());
            store.push(make_record(1001, "persisted.bin", 500)).await.expect("push");
        }
        // 重新加载（模拟重启）→ 数据仍在
        let store2 = HistoryStore::load(dir.path());
        let all = store2.query(None, None, 100).await;
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].filename, "persisted.bin");
        // id 单调性：新 push 不重复
        let next = store2.push(make_record(1002, "next.bin", 600)).await.expect("push");
        assert_eq!(next, 2);
    }

    #[tokio::test]
    async fn delete_and_batch_delete() {
        let dir = tempfile::tempdir().expect("临时目录");
        let store = HistoryStore::load(dir.path());
        store.push(make_record(1, "a", 1)).await.expect("a");
        store.push(make_record(2, "b", 2)).await.expect("b");
        store.push(make_record(3, "c", 3)).await.expect("c");

        store.delete(2).await.expect("删除单条");
        assert_eq!(store.count().await, 2);

        let removed = store.delete_many(&[1, 3]).await.expect("批量删除");
        assert_eq!(removed, 2);
        assert_eq!(store.count().await, 0);

        // 已持久化
        let store2 = HistoryStore::load(dir.path());
        assert_eq!(store2.count().await, 0);
    }

    #[tokio::test]
    async fn upsert_updates_existing() {
        let dir = tempfile::tempdir().expect("临时目录");
        let store = HistoryStore::load(dir.path());
        store.push(make_record(1001, "first", 1)).await.expect("首次");

        let mut updated = make_record(1001, "first", 1);
        updated.status = crate::file_transfer::protocol::TransferStatus::Failed;
        updated.error_message = Some("网络中断".into());
        store.upsert(updated).await.expect("更新");

        let all = store.query(None, None, 100).await;
        assert_eq!(all.len(), 1, "upsert 不应新增");
        assert_eq!(
            all[0].status,
            crate::file_transfer::protocol::TransferStatus::Failed
        );
        assert_eq!(all[0].error_message.as_deref(), Some("网络中断"));
    }

    #[tokio::test]
    async fn clear_all() {
        let dir = tempfile::tempdir().expect("临时目录");
        let store = HistoryStore::load(dir.path());
        store.push(make_record(1, "a", 1)).await.expect("a");
        store.clear_all().await.expect("清空");
        assert_eq!(store.count().await, 0);
    }

    #[tokio::test]
    async fn corrupt_file_returns_empty() {
        let dir = tempfile::tempdir().expect("临时目录");
        std::fs::write(dir.path().join("transfer_history.json"), "{bad json").unwrap();
        let store = HistoryStore::load(dir.path());
        assert_eq!(store.count().await, 0);
        // 仍可正常写入
        store.push(make_record(1, "ok", 1)).await.expect("push");
        assert_eq!(store.count().await, 1);
    }

    #[test]
    fn json_serde_roundtrip() {
        let rec = make_record(1001, "serde.bin", 7);
        let json = serde_json::to_string(&rec).expect("序列化");
        let back: TransferRecord = serde_json::from_str(&json).expect("反序列化");
        assert_eq!(rec, back);
    }
}
