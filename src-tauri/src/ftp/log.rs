//! FTP 连接日志（F9）：实时收集客户端连接/操作记录
//!
//! 环形缓冲上限 500 条；由 auth（登录事件）与 storage（操作事件）写入，
//! 前端通过 `ftp_get_logs` 命令读取。

use std::collections::VecDeque;
use std::net::IpAddr;
use std::sync::OnceLock;

use parking_lot::Mutex;

/// 单条连接日志
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct FtpLogEntry {
    /// 事件时间（本地时间 HH:MM:SS）
    pub time: String,
    /// 客户端 IP
    pub ip: String,
    /// 用户名（登录失败时可能为空）
    pub user: String,
    /// 动作：登录成功/登录失败/上传/下载/删除/新建目录/重命名/列表
    pub action: String,
    /// 详情（文件路径或失败原因）
    pub detail: String,
}

const MAX_LOGS: usize = 500;

static LOG_BUFFER: OnceLock<Mutex<VecDeque<FtpLogEntry>>> = OnceLock::new();

fn buffer() -> &'static Mutex<VecDeque<FtpLogEntry>> {
    LOG_BUFFER.get_or_init(|| Mutex::new(VecDeque::with_capacity(MAX_LOGS)))
}

/// 写入一条日志
pub fn push_log(ip: IpAddr, user: &str, action: &str, detail: &str) {
    let mut buf = buffer().lock();
    if buf.len() >= MAX_LOGS {
        buf.pop_front();
    }
    buf.push_back(FtpLogEntry {
        time: chrono::Local::now().format("%H:%M:%S").to_string(),
        ip: ip.to_string(),
        user: user.to_string(),
        action: action.to_string(),
        detail: detail.to_string(),
    });
}

/// 存储层操作日志（user 携带会话客户端 IP 与用户名）
pub fn push_log_anon(user: &crate::ftp::auth::FtpUserDetail, action: &str, detail: &str) {
    push_log(user.client_ip, &user.username, action, detail);
}

/// 读取全部日志（最新在前）
pub fn get_logs() -> Vec<FtpLogEntry> {
    let buf = buffer().lock();
    buf.iter().rev().cloned().collect()
}

/// 清空日志
pub fn clear_logs() {
    buffer().lock().clear();
}
