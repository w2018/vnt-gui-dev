//! 文件传输设置持久化（file_transfer_config.json）
//!
//! 持久化内容：过滤模式、扩展名列表、自动接收开关、通道阈值、默认保存目录。
//! 写入采用原子操作（先写 .tmp 再 rename）。

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::file_transfer::filter::{FileTypeFilter, FilterMode};
use crate::file_transfer::DEFAULT_THRESHOLD;

/// 文件传输设置
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FileTransferSettings {
    pub mode: FilterMode,
    /// 扩展名集合（不含点，小写）
    pub extensions: Vec<String>,
    pub auto_accept: bool,
    /// 通道阈值（字节，≥ 此值走 TCP）
    pub threshold: u64,
    /// 默认保存目录（空 = 使用默认接收目录）
    pub save_dir: String,
}

impl Default for FileTransferSettings {
    fn default() -> Self {
        let f = FileTypeFilter::default();
        let mut exts = f.extensions.into_iter().collect::<Vec<_>>();
        exts.sort();
        Self {
            mode: f.mode,
            extensions: exts,
            auto_accept: false,
            threshold: DEFAULT_THRESHOLD,
            save_dir: String::new(),
        }
    }
}

const CONFIG_FILE: &str = "file_transfer_config.json";

/// 加载设置（文件缺失/损坏返回默认值）
pub fn load(config_dir: &Path) -> FileTransferSettings {
    let path = config_dir.join(CONFIG_FILE);
    match std::fs::read_to_string(&path) {
        Ok(content) => serde_json::from_str(&content).unwrap_or_else(|e| {
            log::warn!("文件传输设置解析失败（使用默认值）: {}", e);
            FileTransferSettings::default()
        }),
        Err(_) => FileTransferSettings::default(),
    }
}

/// 保存设置（原子写：先 .tmp 再 rename）
pub fn save(config_dir: &Path, settings: &FileTransferSettings) -> Result<(), String> {
    let path = config_dir.join(CONFIG_FILE);
    let tmp = config_dir.join(format!("{}.tmp", CONFIG_FILE));
    let json =
        serde_json::to_string_pretty(settings).map_err(|e| format!("序列化失败: {}", e))?;
    std::fs::write(&tmp, json).map_err(|e| format!("写入失败: {}", e))?;
    std::fs::rename(&tmp, &path).map_err(|e| format!("落盘失败: {}", e))?;
    Ok(())
}

/// 由设置构造过滤器
pub fn to_filter(settings: &FileTransferSettings) -> FileTypeFilter {
    FileTypeFilter {
        mode: settings.mode,
        extensions: settings.extensions.iter().cloned().collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_settings() {
        let s = FileTransferSettings::default();
        assert_eq!(s.mode, FilterMode::Whitelist);
        assert!(!s.auto_accept);
        assert_eq!(s.threshold, DEFAULT_THRESHOLD);
        assert!(s.extensions.contains(&"txt".to_string()));
        assert!(s.save_dir.is_empty());
    }

    #[test]
    fn roundtrip() {
        let dir = tempfile::tempdir().expect("临时目录");
        let s = FileTransferSettings {
            mode: FilterMode::Blacklist,
            extensions: vec!["exe".to_string(), "dll".to_string()],
            auto_accept: true,
            threshold: 50 * 1024 * 1024,
            save_dir: "D:\\接收".to_string(),
        };
        save(dir.path(), &s).expect("保存");
        let loaded = load(dir.path());
        assert_eq!(loaded, s);
    }

    #[test]
    fn load_missing_returns_default() {
        let dir = tempfile::tempdir().expect("临时目录");
        assert_eq!(load(dir.path()), FileTransferSettings::default());
    }

    #[test]
    fn load_corrupt_returns_default() {
        let dir = tempfile::tempdir().expect("临时目录");
        std::fs::write(dir.path().join(CONFIG_FILE), "{bad").unwrap();
        assert_eq!(load(dir.path()), FileTransferSettings::default());
    }

    #[test]
    fn to_filter_converts() {
        let s = FileTransferSettings {
            mode: FilterMode::Whitelist,
            extensions: vec!["txt".to_string(), "pdf".to_string()],
            auto_accept: false,
            threshold: 100,
            save_dir: String::new(),
        };
        let f = to_filter(&s);
        assert!(f.is_allowed("txt"));
        assert!(f.is_allowed("PDF"));
        assert!(!f.is_allowed("exe"));
    }
}
