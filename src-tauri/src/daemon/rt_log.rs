//! Daemon 运行日志收集：tracing Layer → 内存环形缓冲（GUI 经 RPC 拉取）
//!
//! GUI 的"实时日志"页同时展示 GUI 进程日志与 daemon 运行日志；
//! daemon 为独立进程，日志通过 RPC `VntGetLogs` / `VntClearLogs` 获取。

use std::sync::OnceLock;

use tracing::Subscriber;
use tracing_subscriber::Layer;

use crate::logger::LogBuffer;
use crate::state::{LogEntry, LogLevel};

/// daemon 日志缓冲（与 GUI 的全局缓冲同构，独立实例）
pub fn daemon_log_buffer() -> &'static LogBuffer {
    static BUF: OnceLock<LogBuffer> = OnceLock::new();
    BUF.get_or_init(LogBuffer::new)
}

/// tracing Layer：把 daemon 内 tracing/log 事件写入内存缓冲
pub struct RtLogLayer;

impl<S: Subscriber> Layer<S> for RtLogLayer {
    fn on_event(&self, event: &tracing::Event<'_>, _ctx: tracing_subscriber::layer::Context<'_, S>) {
        let level = match *event.metadata().level() {
            tracing::Level::ERROR => LogLevel::Error,
            tracing::Level::WARN => LogLevel::Warn,
            tracing::Level::DEBUG => LogLevel::Debug,
            tracing::Level::TRACE | tracing::Level::INFO => LogLevel::Info,
        };
        // 收集事件所有字段（含 message）
        let mut message = String::new();
        let mut visitor = MessageVisitor(&mut message);
        event.record(&mut visitor);
        if message.is_empty() {
            message = event.metadata().target().to_string();
        }
        daemon_log_buffer().append(level, message);
    }
}

/// 字段访问器：拼接 message 字段（tracing 的 fmt 风格）
struct MessageVisitor<'a>(&'a mut String);

impl<'a> tracing::field::Visit for MessageVisitor<'a> {
    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        if field.name() == "message" {
            self.0.push_str(&format!("{:?}", value));
        }
    }

    fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
        if field.name() == "message" {
            self.0.push_str(value);
        }
    }
}

/// 读取全部 daemon 日志（最新在前）
pub fn get_logs() -> Vec<LogEntry> {
    let mut all: Vec<LogEntry> = daemon_log_buffer().get_all();
    all.reverse(); // LogBuffer 旧→新，前端展示新→旧统一
    all
}

/// 清空 daemon 日志
pub fn clear_logs() {
    daemon_log_buffer().clear();
}

/// 与 LogBuffer 内部一致性的辅助（测试用）
#[allow(dead_code)]
fn _buffer_len() -> usize {
    let buf = daemon_log_buffer();
    let _ = buf;
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_layer_collects_log() {
        let buf = daemon_log_buffer();
        buf.clear();
        // 直接模拟 Layer 行为（Layer 需注册到 subscriber 才能接事件，这里验证 append 路径）
        buf.append(LogLevel::Info, "test message".into());
        let logs = get_logs();
        assert_eq!(logs.len(), 1);
        assert_eq!(logs[0].message, "test message");
        buf.clear();
        assert!(get_logs().is_empty());
    }
}
