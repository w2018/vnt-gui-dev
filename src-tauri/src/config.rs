//! 配置持久化（文档 §3.5）
//!
//! 存储路径：Windows 应用安装目录（可执行文件同目录）`config.json`
//! 说明：配置/日志统一存放于安装目录，卸载时由 NSIS 钩子按用户选择删除。

use std::path::{Path, PathBuf};

use chrono::Utc;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// VNT 连接配置（对应 vnt-cli 命令行参数）
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct VntConfig {
    /// UUID，唯一标识
    pub id: String,
    /// 配置名称（用户自定义，如"家庭网络"）
    pub name: String,
    /// 组网编号（必填，-k）
    pub token: String,
    /// 设备名称（-n）
    #[serde(default)]
    pub device_name: Option<String>,
    /// 设备 ID（-d）
    #[serde(default)]
    pub device_id: Option<String>,
    /// 虚拟 IP（--ip）
    #[serde(default)]
    pub virtual_ip: Option<String>,
    /// 服务器地址（-s）
    #[serde(default)]
    pub server_address: Option<String>,
    /// 组网密码（-w）
    #[serde(default)]
    pub password: Option<String>,
    /// 服务端加密（-W）
    #[serde(default)]
    pub server_encrypt: bool,
    /// 入站网段（-i）
    #[serde(default)]
    pub in_ips: Vec<String>,
    /// 出站网段（-o）
    #[serde(default)]
    pub out_ips: Vec<String>,
    /// 压缩算法（--compressor）
    #[serde(default)]
    pub compressor: Option<String>,
    /// MTU（--mtu）
    #[serde(default)]
    pub mtu: Option<u16>,
    /// 使用 TCP 协议（-t）
    #[serde(default)]
    pub use_tcp: bool,
    /// 使用 WebSocket 协议（--use-ws）
    #[serde(default)]
    pub use_ws: bool,
    /// 禁用代理（--no-proxy）
    #[serde(default)]
    pub no_proxy: bool,
    /// 创建时间 ISO 8601
    #[serde(default)]
    pub created_at: String,
    /// 更新时间 ISO 8601
    #[serde(default)]
    pub updated_at: String,
    /// 最后使用时间
    #[serde(default)]
    pub last_used: Option<String>,
}

impl VntConfig {
    /// 新建配置（生成 id 与时间戳）
    pub fn new(name: String, token: String) -> Self {
        let now = Utc::now().to_rfc3339();
        Self {
            id: Uuid::new_v4().to_string(),
            name,
            token,
            device_name: None,
            device_id: None,
            virtual_ip: None,
            server_address: None,
            password: None,
            server_encrypt: false,
            in_ips: Vec::new(),
            out_ips: Vec::new(),
            compressor: None,
            mtu: None,
            use_tcp: false,
            use_ws: false,
            no_proxy: false,
            created_at: now.clone(),
            updated_at: now,
            last_used: None,
        }
    }
}

/// 配置存储容器
#[derive(Debug, Serialize, Deserialize)]
pub struct ConfigStore {
    pub active_config_id: Option<String>,
    pub configs: Vec<VntConfig>,
}

impl Default for ConfigStore {
    fn default() -> Self {
        Self {
            active_config_id: None,
            configs: Vec::new(),
        }
    }
}

impl ConfigStore {
    pub fn new() -> Self {
        Self::default()
    }

    /// 新增或更新配置
    pub fn add_or_update(&mut self, mut config: VntConfig) {
        config.updated_at = Utc::now().to_rfc3339();
        if let Some(idx) = self.configs.iter().position(|c| c.id == config.id) {
            self.configs[idx] = config;
        } else {
            config.id = Uuid::new_v4().to_string();
            config.created_at = Utc::now().to_rfc3339();
            self.configs.push(config);
        }
    }

    /// 删除配置；删除活动配置时自动切到第一条
    pub fn delete(&mut self, id: &str) {
        self.configs.retain(|c| c.id != id);
        if self.active_config_id.as_deref() == Some(id) {
            self.active_config_id = self.configs.first().map(|c| c.id.clone());
        }
    }

    /// 切换活动配置并记录最后使用时间
    pub fn set_active(&mut self, id: &str) {
        if self.configs.iter().any(|c| c.id == id) {
            self.active_config_id = Some(id.to_string());
            if let Some(cfg) = self.configs.iter_mut().find(|c| c.id == id) {
                cfg.last_used = Some(Utc::now().to_rfc3339());
            }
        }
    }

    /// 获取活动配置
    pub fn get_active(&self) -> Option<&VntConfig> {
        let id = self.active_config_id.as_deref()?;
        self.configs.iter().find(|c| c.id == id)
    }

    /// 按 id 获取配置
    pub fn get(&self, id: &str) -> Option<&VntConfig> {
        self.configs.iter().find(|c| c.id == id)
    }
}

/// 可执行文件所在目录（即应用安装目录；dev 运行 = target 目录）
fn exe_dir() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.to_path_buf()))
        .unwrap_or_else(|| {
            dirs::config_dir()
                .unwrap_or_else(|| PathBuf::from("."))
                .join("vnt-gui")
        })
}

/// 应用数据目录：<安装目录>/data（配置文件统一存放，卸载可整体清除）
pub fn app_data_dir() -> PathBuf {
    let dir = exe_dir().join("data");
    std::fs::create_dir_all(&dir).ok();
    dir
}

/// 日志目录：<安装目录>/logs（GUI 日志与 daemon 日志统一存放）
pub fn log_dir() -> PathBuf {
    let dir = exe_dir().join("logs");
    std::fs::create_dir_all(&dir).ok();
    dir
}

/// 配置文件路径：<安装目录>/data/config.json
pub fn get_config_path() -> PathBuf {
    app_data_dir().join("config.json")
}

/// 迁移旧版数据到新目录布局：
/// 1. %APPDATA%/vnt-gui → <安装目录>/data（v2.0.4 之前的配置目录）
/// 2. <安装目录>根下的旧布局残留（v2.0.4 曾直接放安装目录）→ data/ 与 logs/
/// 目标文件已存在则跳过，不覆盖新数据。
pub fn migrate_legacy_data() {
    // 1. %APPDATA%\vnt-gui → data\
    let legacy = dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("vnt-gui");
    let data_dir = app_data_dir();
    if legacy.is_dir() && legacy != data_dir {
        copy_files(&legacy, &data_dir);
    }
    // 2. 安装目录根残留（v2.0.4 中间版本直接写安装目录）
    for name in ["config.json", "ftp_config.json", "runtime_state.json", "daemon.pid"] {
        let src = exe_dir().join(name);
        if src.is_file() {
            let dst = data_dir.join(name);
            if !dst.exists() {
                let _ = std::fs::copy(&src, &dst);
            }
        }
    }
    let src_log = exe_dir().join("vnt-daemon.log");
    if src_log.is_file() {
        let dst = log_dir().join("vnt-daemon.log");
        if !dst.exists() {
            let _ = std::fs::copy(&src_log, &dst);
        }
    }
}

/// 将 src 目录下的文件复制到 dst 目录（目标已存在跳过）
fn copy_files(src: &Path, dst: &Path) {
    let Ok(entries) = std::fs::read_dir(src) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let s = entry.path();
        let d = dst.join(&name);
        if s.is_file() && !d.exists() {
            let _ = std::fs::copy(&s, &d);
        }
    }
}

/// 加载配置存储（文件不存在或损坏时返回空存储）
pub fn load_config_store() -> ConfigStore {
    let path = get_config_path();
    match std::fs::read_to_string(&path) {
        Ok(content) => serde_json::from_str(&content).unwrap_or_else(|_| ConfigStore::new()),
        Err(_) => ConfigStore::new(),
    }
}

/// 保存配置存储
pub fn save_config_store(store: &ConfigStore) -> Result<(), String> {
    let path = get_config_path();
    let json = serde_json::to_string_pretty(store).map_err(|e| format!("序列化配置失败: {}", e))?;
    std::fs::write(&path, json).map_err(|e| format!("写入配置失败: {}", e))?;
    Ok(())
}
