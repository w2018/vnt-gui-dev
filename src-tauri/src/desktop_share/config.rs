//! 桌面共享配置持久化
//!
//! 存于 数据目录/desktop_share_config.json（与项目其他配置同目录）

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::desktop_share::protocol::GrantedCapabilities;

/// 桌面共享配置
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DesktopShareConfig {
    /// 是否允许被控（接受连接）
    pub allow_be_controlled: bool,
    /// 默认授予的能力
    pub default_grant: GrantedCapabilities,
    /// 捕获配置
    pub capture: CaptureSettings,
    /// 连接确认超时（秒）
    pub confirm_timeout_secs: u64,
    /// ffmpeg 路径（空 = 使用系统 PATH 或安装资源目录）
    pub ffmpeg_path: String,
    /// 是否启用剪贴板同步
    pub clipboard_sync: bool,
    /// 桌面共享监听端口（Iroh QUIC 固定端口，两端需一致）
    pub listen_port: u16,
}

/// 捕获设置
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CaptureSettings {
    /// 目标帧率
    pub fps: u32,
    /// 目标码率（Kbps）
    pub bitrate_kbps: u32,
    /// 输出宽度（0 = 原始分辨率）
    pub width: u32,
    /// 输出高度（0 = 原始分辨率）
    pub height: u32,
    /// 显示器索引
    pub monitor: usize,
    /// 画质 CRF（0-51，越低越好）
    pub quality: u32,
}

impl Default for DesktopShareConfig {
    fn default() -> Self {
        Self {
            allow_be_controlled: true,
            default_grant: GrantedCapabilities::default(),
            capture: CaptureSettings {
                fps: 30,
                bitrate_kbps: 2000,
                width: 1920,
                height: 1080,
                monitor: 0,
                quality: 23,
            },
            confirm_timeout_secs: 30,
            ffmpeg_path: String::new(),
            clipboard_sync: true,
            listen_port: crate::desktop_share::network::DEFAULT_PORT,
        }
    }
}

impl Default for CaptureSettings {
    fn default() -> Self {
        Self {
            fps: 30,
            bitrate_kbps: 2000,
            width: 1920,
            height: 1080,
            monitor: 0,
            quality: 23,
        }
    }
}

const CONFIG_FILE: &str = "desktop_share_config.json";

/// 数据目录（与项目其他配置同目录）
pub fn config_dir() -> PathBuf {
    crate::config::get_config_path()
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .to_path_buf()
}

/// 加载配置（不存在或解析失败返回默认值）
pub fn load(config_dir: &Path) -> DesktopShareConfig {
    let path = config_dir.join(CONFIG_FILE);
    match std::fs::read_to_string(&path) {
        Ok(content) => serde_json::from_str(&content).unwrap_or_else(|e| {
            log::warn!("桌面共享配置解析失败（使用默认值）: {}", e);
            DesktopShareConfig::default()
        }),
        Err(_) => DesktopShareConfig::default(),
    }
}

/// 保存配置（原子写：先写临时文件再 rename）
pub fn save(config_dir: &Path, cfg: &DesktopShareConfig) -> Result<(), String> {
    let path = config_dir.join(CONFIG_FILE);
    let tmp = config_dir.join(format!("{}.tmp", CONFIG_FILE));
    let json =
        serde_json::to_string_pretty(cfg).map_err(|e| format!("序列化失败: {}", e))?;
    std::fs::write(&tmp, json).map_err(|e| format!("写入失败: {}", e))?;
    std::fs::rename(&tmp, &path).map_err(|e| format!("落盘失败: {}", e))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let cfg = DesktopShareConfig::default();
        assert!(cfg.allow_be_controlled);
        assert_eq!(cfg.capture.fps, 30);
        assert_eq!(cfg.capture.bitrate_kbps, 2000);
        assert!(cfg.default_grant.mouse);
        assert!(cfg.default_grant.keyboard);
        assert!(cfg.clipboard_sync);
        assert!(cfg.listen_port > 0);
    }

    #[test]
    fn test_config_roundtrip() {
        let cfg = DesktopShareConfig {
            allow_be_controlled: false,
            capture: CaptureSettings {
                fps: 60,
                bitrate_kbps: 5000,
                width: 2560,
                height: 1440,
                monitor: 1,
                quality: 18,
            },
            clipboard_sync: false,
            listen_port: 40000,
            ..Default::default()
        };
        let dir = tempfile::tempdir().expect("创建临时目录失败");
        save(dir.path(), &cfg).expect("保存失败");
        let loaded = load(dir.path());
        assert_eq!(loaded.capture.fps, 60);
        assert_eq!(loaded.capture.bitrate_kbps, 5000);
        assert_eq!(loaded.capture.width, 2560);
        assert_eq!(loaded.capture.monitor, 1);
        assert_eq!(loaded.listen_port, 40000);
        assert!(!loaded.allow_be_controlled);
        assert!(!loaded.clipboard_sync);
    }

    #[test]
    fn test_load_missing_returns_default() {
        let dir = tempfile::tempdir().expect("创建临时目录失败");
        let cfg = load(dir.path());
        assert!(cfg.allow_be_controlled);
    }

    #[test]
    fn test_load_corrupt_returns_default() {
        let dir = tempfile::tempdir().expect("创建临时目录失败");
        std::fs::write(dir.path().join(CONFIG_FILE), "{invalid json").unwrap();
        let cfg = load(dir.path());
        assert!(cfg.allow_be_controlled);
    }

    #[test]
    fn test_save_overwrites() {
        let dir = tempfile::tempdir().expect("创建临时目录失败");
        let a = DesktopShareConfig {
            allow_be_controlled: false,
            ..Default::default()
        };
        save(dir.path(), &a).unwrap();
        let b = DesktopShareConfig::default();
        save(dir.path(), &b).unwrap();
        let loaded = load(dir.path());
        assert!(loaded.allow_be_controlled);
    }
}
