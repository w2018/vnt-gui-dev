//! FTP 服务配置：结构体 + JSON 持久化（%APPDATA%/vnt-gui/ftp_config.json）
//!
//! 安全约束：密码**禁止明文落盘**——`FtpUser.password` 标记 `#[serde(skip)]`，
//! 实际以 argon2 哈希存入系统凭据库（keyring，Windows = DPAPI）。

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// FTP 用户权限（独立设置，见需求 F6）
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct FtpPermissions {
    /// 上传（put / mkd）
    pub upload: bool,
    /// 下载（get / list / metadata）
    pub download: bool,
    /// 删除（del / rmd）
    pub delete: bool,
    /// 只读：强制禁止上传 + 删除（优先于其他勾选）
    pub readonly: bool,
}

impl FtpPermissions {
    /// 归一化：readonly 时强制关闭 upload/delete
    pub fn normalize(&mut self) {
        if self.readonly {
            self.upload = false;
            self.delete = false;
        }
    }
}

/// FTP 用户（username + 权限）
///
/// `password` 字段仅存在于内存（argon2 哈希，来源 keyring），
/// 序列化时被 `#[serde(skip)]` 跳过 —— JSON 中绝不出现密码。
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct FtpUser {
    pub username: String,
    /// 内存中的 argon2 密码哈希（不落盘）；编辑用户不改密码时为空 → 保留旧哈希
    #[serde(skip)]
    pub password: String,
    pub permissions: FtpPermissions,
}

/// FTP 服务全局配置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FtpConfig {
    /// 总开关
    pub enabled: bool,
    /// 随应用启动（F2）：打开 VNT GUI 时自动启动 FTP
    pub auto_start_with_app: bool,
    /// 随系统开机自启（F3）：勾选后同步 Windows 开机自启 VNT GUI
    pub auto_start_with_system: bool,
    /// FTP 根目录（F4）
    pub root_dir: String,
    /// 控制端口（F7，默认 2121 避免占用系统 21）
    pub port: u16,
    /// 监听 socket 开启 SO_REUSEADDR（默认 true，停止后端口立即可复用）
    #[serde(default = "default_true")]
    pub so_reuseaddr: bool,
    /// PASV 被动端口范围（F7，可选）
    pub pasv_ports: Option<(u16, u16)>,
    /// 用户列表（F5）
    pub users: Vec<FtpUser>,
}

fn default_true() -> bool {
    true
}

impl Default for FtpConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            auto_start_with_app: false,
            auto_start_with_system: false,
            root_dir: String::new(),
            port: 2121,
            so_reuseaddr: true,
            pasv_ports: None,
            users: Vec::new(),
        }
    }
}

const FTP_CONFIG_FILE: &str = "ftp_config.json";
/// keyring 服务名（Windows 凭据管理器中可见）
pub const KEYRING_SERVICE: &str = "vnt-gui-ftp";

/// 加载 FTP 配置（文件不存在返回默认）
pub fn load_ftp_config(config_dir: &Path) -> FtpConfig {
    let path = config_dir.join(FTP_CONFIG_FILE);
    match std::fs::read_to_string(&path) {
        Ok(content) => serde_json::from_str(&content).unwrap_or_default(),
        Err(_) => FtpConfig::default(),
    }
}

/// 原子写盘
pub fn save_ftp_config(config_dir: &Path, cfg: &FtpConfig) -> Result<(), String> {
    let path = config_dir.join(FTP_CONFIG_FILE);
    let tmp = config_dir.join(format!("{}.tmp", FTP_CONFIG_FILE));
    let json = serde_json::to_string_pretty(cfg).map_err(|e| format!("序列化失败: {}", e))?;
    std::fs::write(&tmp, json).map_err(|e| format!("写入失败: {}", e))?;
    std::fs::rename(&tmp, &path).map_err(|e| format!("落盘失败: {}", e))?;
    Ok(())
}

/// 读取/创建 keyring 条目（service="vnt-gui-ftp", account=username）
fn keyring_entry(username: &str) -> Result<keyring::Entry, String> {
    keyring::Entry::new(KEYRING_SERVICE, username).map_err(|e| format!("keyring 初始化失败: {}", e))
}

/// 将密码哈希写入系统凭据库
pub fn set_password_hash(username: &str, hash: &str) -> Result<(), String> {
    keyring_entry(username)?
        .set_password(hash)
        .map_err(|e| format!("密码存储失败: {}", e))
}

/// 从系统凭据库读取密码哈希（无条目返回空 → 该用户无法登录）
pub fn get_password_hash(username: &str) -> Option<String> {
    keyring_entry(username)
        .ok()
        .and_then(|e| e.get_password().ok())
}

/// 删除用户时同时清理凭据库条目
pub fn delete_password(username: &str) {
    if let Ok(entry) = keyring_entry(username) {
        let _ = entry.delete_credential();
    }
}

/// 配置目录（由 tauri setup 注入）
pub fn config_dir_of() -> PathBuf {
    PathBuf::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_port_is_2121() {
        // V2 要求：验证 Default 实现端口为 2121
        let cfg = FtpConfig::default();
        assert_eq!(cfg.port, 2121);
        assert!(cfg.so_reuseaddr, "so_reuseaddr 默认应为 true");
        assert!(!cfg.enabled);
        assert!(cfg.users.is_empty());
        assert_eq!(cfg.pasv_ports, None);
    }

    #[test]
    fn test_password_not_in_json() {
        // V2 要求：序列化 FtpConfig 为 JSON，断言不含 password 字段
        let mut cfg = FtpConfig::default();
        cfg.port = 2122;
        cfg.users.push(FtpUser {
            username: "admin".into(),
            password: "supersecret-hash-must-not-leak".into(),
            permissions: FtpPermissions {
                upload: true,
                download: true,
                delete: true,
                readonly: false,
            },
        });
        let json = serde_json::to_string(&cfg).unwrap();
        assert!(!json.contains("password"), "JSON 中不得出现 password 字段: {}", json);
        assert!(!json.contains("supersecret"), "JSON 中不得出现密码内容: {}", json);
        assert!(json.contains("\"username\":\"admin\""));
        assert!(json.contains("\"upload\":true"));
    }

    #[test]
    fn test_readonly_normalizes_upload_delete() {
        // readonly=true 时强制 upload=false, delete=false
        let mut p = FtpPermissions {
            upload: true,
            download: true,
            delete: true,
            readonly: true,
        };
        p.normalize();
        assert!(!p.upload);
        assert!(!p.delete);
        assert!(p.download);
    }

    #[test]
    fn test_config_roundtrip() {
        let mut cfg = FtpConfig::default();
        cfg.port = 3000;
        cfg.pasv_ports = Some((30001, 30010));
        cfg.users.push(FtpUser {
            username: "u1".into(),
            password: "".into(),
            permissions: FtpPermissions::default(),
        });
        let dir = tempfile::tempdir().unwrap();
        save_ftp_config(dir.path(), &cfg).unwrap();
        let loaded = load_ftp_config(dir.path());
        assert_eq!(loaded.port, 3000);
        assert_eq!(loaded.pasv_ports, Some((30001, 30010)));
        assert_eq!(loaded.users.len(), 1);
        assert_eq!(loaded.users[0].username, "u1");
        // 密码字段被 skip，roundtrip 后为空
        assert_eq!(loaded.users[0].password, "");
    }
}
