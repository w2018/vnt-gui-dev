//! 应用设置持久化（%APPDATA%/vnt-gui/settings.json）
//! 与连接配置（config.json）分离，存放 UI/行为开关。

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// 应用行为设置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppSettings {
    /// 开机自启时不显示托盘（静默后台运行）
    #[serde(default)]
    pub hide_tray_on_autostart: bool,
    /// 后台运行时隐藏托盘（关闭窗口后无托盘入口）
    #[serde(default)]
    pub hide_tray_on_background: bool,
    /// 启动应用时自动连接默认配置（vnt-daemon 模式下；默认开启）
    #[serde(default = "default_true")]
    pub auto_connect_on_startup: bool,
}

/// 布尔默认值 true（serde 兼容：老配置缺少该字段时默认开启）
fn default_true() -> bool {
    true
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            hide_tray_on_autostart: false,
            hide_tray_on_background: false,
            auto_connect_on_startup: true,
        }
    }
}

/// 设置文件路径：%APPDATA%/vnt-gui/settings.json
pub fn settings_path() -> PathBuf {
    let appdata = dirs::config_dir().unwrap_or_else(|| PathBuf::from("."));
    let dir = appdata.join("vnt-gui");
    std::fs::create_dir_all(&dir).ok();
    dir.join("settings.json")
}

/// 加载设置（文件不存在/损坏时返回默认）
pub fn load_settings() -> AppSettings {
    match std::fs::read_to_string(settings_path()) {
        Ok(content) => serde_json::from_str(&content).unwrap_or_default(),
        Err(_) => AppSettings::default(),
    }
}

/// 保存设置
pub fn save_settings(settings: &AppSettings) -> Result<(), String> {
    let json =
        serde_json::to_string_pretty(settings).map_err(|e| format!("序列化设置失败: {}", e))?;
    std::fs::write(settings_path(), json).map_err(|e| format!("写入设置失败: {}", e))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_settings_default_auto_connect_true() {
        // 老版本配置缺少 auto_connect_on_startup 字段 → 应默认开启
        let raw = r#"{ "hide_tray_on_autostart": true, "hide_tray_on_background": false }"#;
        let s: AppSettings = serde_json::from_str(raw).unwrap();
        assert!(s.auto_connect_on_startup, "老配置缺字段应默认开启");
        assert!(s.hide_tray_on_autostart);
        assert!(!s.hide_tray_on_background);
    }

    #[test]
    fn settings_roundtrip_preserves_auto_connect() {
        let s = AppSettings {
            hide_tray_on_autostart: false,
            hide_tray_on_background: true,
            auto_connect_on_startup: false,
        };
        let json = serde_json::to_string(&s).unwrap();
        let back: AppSettings = serde_json::from_str(&json).unwrap();
        assert_eq!(back.auto_connect_on_startup, false);
        assert_eq!(back.hide_tray_on_background, true);
    }

    #[test]
    fn default_settings_auto_connect_enabled() {
        assert!(AppSettings::default().auto_connect_on_startup);
    }
}
