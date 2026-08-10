//! 运行时状态持久化（daemon 重启后恢复服务）
//!
//! 数据落盘：%APPDATA%\vnt-gui\runtime_state.json
//! 恢复语义：磁盘加载后 running 标记重置（进程已重启），仅保留
//! `*_was_running`（上次是否在运行）供 auto_start 决策。
//!
//! 实现：内存结构含 `Instant`（不可序列化），通过 StoredState 视图读写磁盘。

use std::path::PathBuf;
use std::time::Instant;

use serde::{Deserialize, Serialize};

use crate::config::VntConfig;
use crate::ftp::config::FtpConfig;
use crate::state::PeerInfo;

/// Daemon 运行时状态（进程内存）
#[derive(Debug, Clone)]
pub struct RuntimeState {
    /// VNT 进程是否在运行
    pub vnt_running: bool,
    /// VNT 是否已连接（组网注册成功）
    pub vnt_connected: bool,
    /// 上次运行时 VNT 在跑（用于重启后恢复）
    pub vnt_was_running: bool,
    /// 当前 VNT 配置
    pub vnt_config: Option<VntConfig>,
    /// 真实连接服务器（IP:端口，连接信息展示）
    pub vnt_server_host: Option<String>,
    /// 本机虚拟 IP
    pub vnt_virtual_ip: Option<String>,
    /// NAT 类型（尽力解析）
    pub vnt_nat_type: Option<String>,
    /// FTP 是否在运行
    pub ftp_running: bool,
    /// 上次运行时 FTP 在跑（用于重启后恢复）
    pub ftp_was_running: bool,
    /// 当前 FTP 配置
    pub ftp_config: Option<FtpConfig>,
    /// 设备列表（--list 定期解析）
    pub peers: Vec<PeerInfo>,
    /// daemon 启动时间（不持久化）
    pub started_at: Instant,
}

impl Default for RuntimeState {
    fn default() -> Self {
        Self {
            vnt_running: false,
            vnt_connected: false,
            vnt_was_running: false,
            vnt_config: None,
            vnt_server_host: None,
            vnt_virtual_ip: None,
            vnt_nat_type: None,
            ftp_running: false,
            ftp_was_running: false,
            ftp_config: None,
            peers: Vec::new(),
            started_at: Instant::now(),
        }
    }
}

impl RuntimeState {
    /// daemon 运行时长（秒）
    pub fn uptime_secs(&self) -> u64 {
        self.started_at.elapsed().as_secs()
    }
}

/// 磁盘存储视图（可序列化；无 Instant）
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct StoredState {
    vnt_running: bool,
    vnt_connected: bool,
    vnt_was_running: bool,
    vnt_config: Option<VntConfig>,
    vnt_server_host: Option<String>,
    vnt_virtual_ip: Option<String>,
    vnt_nat_type: Option<String>,
    ftp_running: bool,
    ftp_was_running: bool,
    ftp_config: Option<FtpConfig>,
    peers: Vec<PeerInfo>,
}

impl From<&RuntimeState> for StoredState {
    fn from(s: &RuntimeState) -> Self {
        Self {
            vnt_running: s.vnt_running,
            vnt_connected: s.vnt_connected,
            vnt_was_running: s.vnt_was_running,
            vnt_config: s.vnt_config.clone(),
            vnt_server_host: s.vnt_server_host.clone(),
            vnt_virtual_ip: s.vnt_virtual_ip.clone(),
            vnt_nat_type: s.vnt_nat_type.clone(),
            ftp_running: s.ftp_running,
            ftp_was_running: s.ftp_was_running,
            ftp_config: s.ftp_config.clone(),
            peers: s.peers.clone(),
        }
    }
}

/// 状态文件路径
fn state_path() -> PathBuf {
    crate::daemon::pid_file::daemon_data_dir().join("runtime_state.json")
}

/// 从磁盘加载（损坏/缺失 → 默认）；运行标记重置
pub async fn load_or_init() -> RuntimeState {
    let path = state_path();
    if let Ok(content) = std::fs::read_to_string(&path) {
        if let Ok(stored) = serde_json::from_str::<StoredState>(&content) {
            // 进程已重启：running 标记必须重置，仅保留 was_running 供恢复决策
            return RuntimeState {
                vnt_running: false,
                vnt_connected: false,
                vnt_was_running: stored.vnt_was_running,
                vnt_config: stored.vnt_config,
                vnt_server_host: stored.vnt_server_host,
                vnt_virtual_ip: stored.vnt_virtual_ip,
                vnt_nat_type: stored.vnt_nat_type,
                ftp_running: false,
                ftp_was_running: stored.ftp_was_running,
                ftp_config: stored.ftp_config,
                peers: stored.peers,
                started_at: Instant::now(),
            };
        }
    }
    RuntimeState::default()
}

/// 保存状态：记录 was_running = 当前 running
pub async fn save(state: &RuntimeState) {
    let path = state_path();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let mut stored = StoredState::from(state);
    stored.vnt_was_running = state.vnt_running;
    stored.ftp_was_running = state.ftp_running;
    // 原子写盘
    let tmp = path.with_extension("json.tmp");
    if let Ok(json) = serde_json::to_string_pretty(&stored) {
        if std::fs::write(&tmp, json).is_ok() {
            let _ = std::fs::rename(&tmp, &path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ftp::config::FtpUser;

    #[tokio::test]
    async fn test_state_persistence_roundtrip() {
        // V4 要求：save → load → was_running 正确恢复、running 重置为 false
        // set_data_dir 全局竞争 → 串行锁
        let _g = crate::daemon::pid_file::DATA_DIR_LOCK.lock().await;
        let _dir = tempfile::tempdir().unwrap();
        crate::daemon::pid_file::set_data_dir(_dir.path().to_path_buf());

        let mut state = RuntimeState::default();
        state.vnt_running = true;
        state.vnt_connected = true;
        state.vnt_was_running = false;
        state.vnt_config = Some(VntConfig {
            id: "c1".into(),
            name: "测试组网".into(),
            token: "tok123".into(),
            ..Default::default()
        });
        state.ftp_running = true;
        state.ftp_config = Some(crate::ftp::config::FtpConfig {
            port: 2121,
            users: vec![FtpUser {
                username: "admin".into(),
                ..Default::default()
            }],
            ..Default::default()
        });
        state.peers = vec![PeerInfo {
            name: "Z".into(),
            virtual_ip: "10.26.0.2".into(),
            connection_type: "p2p".into(),
            latency: 12,
            status: "online".into(),
        }];

        save(&state).await;

        let loaded = load_or_init().await;
        // 运行标记重置
        assert!(!loaded.vnt_running, "重启后 running 必须为 false");
        assert!(!loaded.ftp_running, "重启后 running 必须为 false");
        assert!(!loaded.vnt_connected);
        // was_running 恢复
        assert!(loaded.vnt_was_running, "was_running 应恢复为 true");
        assert!(loaded.ftp_was_running, "was_running 应恢复为 true");
        // 配置与 peers 恢复
        assert_eq!(loaded.vnt_config.as_ref().map(|c| c.token.as_str()), Some("tok123"));
        assert_eq!(loaded.ftp_config.as_ref().map(|c| c.port), Some(2121));
        assert_eq!(loaded.peers.len(), 1);
        assert_eq!(loaded.peers[0].virtual_ip, "10.26.0.2");
    }

    #[tokio::test]
    async fn test_state_load_missing_file() {
        // set_data_dir 全局竞争 → 串行锁
        let _g = crate::daemon::pid_file::DATA_DIR_LOCK.lock().await;
        let _dir = tempfile::tempdir().unwrap();
        crate::daemon::pid_file::set_data_dir(_dir.path().to_path_buf());
        let state = load_or_init().await;
        assert!(!state.vnt_running);
        assert!(!state.vnt_was_running);
        assert!(state.vnt_config.is_none());
    }

    #[tokio::test]
    async fn test_state_load_corrupt_file() {
        // set_data_dir 全局竞争 → 串行锁
        let _g = crate::daemon::pid_file::DATA_DIR_LOCK.lock().await;
        let _dir = tempfile::tempdir().unwrap();
        crate::daemon::pid_file::set_data_dir(_dir.path().to_path_buf());
        let path = crate::daemon::pid_file::daemon_data_dir().join("runtime_state.json");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "{corrupt json").unwrap();
        let state = load_or_init().await;
        assert!(!state.vnt_running);
        assert!(state.vnt_config.is_none());
    }
}
