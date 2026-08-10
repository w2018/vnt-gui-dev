//! 日志环形缓冲与导出（文档 §3.6）
//!
//! `AppLogger` 实现 `log::Log`：每条日志同时
//! 1. 写入全局环形缓冲（`get_logs` 命令读取 / 前端日志页展示）
//! 2. 追加到 <安装目录>/logs/app.log（问题 3：日志统一存安装目录）
//! 3. emit `log-line` 事件（前端实时追加，不依赖轮询）

use std::collections::VecDeque;
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::sync::OnceLock;

use parking_lot::Mutex;
use tauri::{AppHandle, Emitter};

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

/// 全局日志缓冲（AppState 不再持有，log 系统静态写入）
fn global_buffer() -> &'static LogBuffer {
    static BUF: OnceLock<LogBuffer> = OnceLock::new();
    BUF.get_or_init(LogBuffer::new)
}

/// 全局日志器（log crate 静态 logger）
pub struct AppLogger {
    /// 落盘文件句柄（<安装目录>/logs/app.log；打开失败则仅内存）
    file: Mutex<Option<File>>,
}

/// 前端事件推送句柄（setup 后注入）
static APP_HANDLE: OnceLock<AppHandle> = OnceLock::new();

/// 初始化：设置全局 logger（只能调用一次）；file_path 为日志文件路径
pub fn init(file_path: std::path::PathBuf) {
    let file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&file_path)
        .ok();
    let logger: &'static AppLogger = Box::leak(Box::new(AppLogger {
        file: Mutex::new(file),
    }));
    let _ = log::set_logger(logger);
    log::set_max_level(log::LevelFilter::Info);
}

/// setup 阶段注入 AppHandle（用于 emit log-line）
pub fn attach(app: AppHandle) {
    let _ = APP_HANDLE.set(app);
}

impl AppLogger {
    fn write_entry(&self, entry: &LogEntry) {
        // 1. 内存缓冲（日志页历史）
        global_buffer().push(entry.clone());
        // 2. 落盘
        if let Some(f) = self.file.lock().as_mut() {
            let _ = writeln!(
                f,
                "[{}] {} {}",
                entry.timestamp,
                match entry.level {
                    LogLevel::Info => "INFO",
                    LogLevel::Warn => "WARN",
                    LogLevel::Error => "ERROR",
                    LogLevel::Debug => "DEBUG",
                },
                entry.message
            );
        }
        // 3. 前端实时事件
        if let Some(app) = APP_HANDLE.get() {
            let _ = app.emit("log-line", entry);
        }
    }
}

impl log::Log for AppLogger {
    fn enabled(&self, _metadata: &log::Metadata) -> bool {
        true
    }

    fn log(&self, record: &log::Record) {
        let level = match record.level() {
            log::Level::Error => LogLevel::Error,
            log::Level::Warn => LogLevel::Warn,
            log::Level::Debug => LogLevel::Debug,
            log::Level::Info | log::Level::Trace => LogLevel::Info,
        };
        let entry = LogEntry {
            timestamp: chrono::Utc::now().to_rfc3339(),
            level,
            message: record.args().to_string(),
        };
        self.write_entry(&entry);
    }

    fn flush(&self) {
        if let Some(f) = self.file.lock().as_mut() {
            let _ = f.flush();
        }
    }
}

/// 读取全局日志（日志页历史加载）
pub fn get_global_logs() -> Vec<LogEntry> {
    global_buffer().get_all()
}

/// 清空全局日志
pub fn clear_global_logs() {
    global_buffer().clear();
}
