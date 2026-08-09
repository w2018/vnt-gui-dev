//! 日志环形缓冲与导出（文档 §3.6）

use std::collections::VecDeque;

use parking_lot::Mutex;

use crate::state::{LogEntry, LogLevel};

/// 最多保留的日志行数
pub const MAX_LOG_LINES: usize = 2000;

/// 线程安全的环形日志缓冲区
pub struct LogBuffer {
    buffer: Mutex<VecDeque<LogEntry>>,
}

impl LogBuffer {
    pub fn new() -> Self {
        Self {
            buffer: Mutex::new(VecDeque::with_capacity(MAX_LOG_LINES)),
        }
    }

    /// 追加一条日志（超出容量自动丢弃最旧）
    pub fn push(&self, entry: LogEntry) {
        let mut buf = self.buffer.lock();
        if buf.len() >= MAX_LOG_LINES {
            buf.pop_front();
        }
        buf.push_back(entry);
    }

    /// 便捷构造并追加
    pub fn append(&self, level: LogLevel, message: String) {
        let entry = LogEntry {
            timestamp: chrono::Utc::now().to_rfc3339(),
            level,
            message,
        };
        self.push(entry);
    }

    pub fn get_all(&self) -> Vec<LogEntry> {
        self.buffer.lock().iter().cloned().collect()
    }

    pub fn clear(&self) {
        self.buffer.lock().clear();
    }
}

impl Default for LogBuffer {
    fn default() -> Self {
        Self::new()
    }
}
