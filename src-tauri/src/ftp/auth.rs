//! FTP 认证：自定义 Authenticator + UserDetail + UserDetailProvider
//!
//! 密码校验使用 argon2（与存储哈希匹配）；内存中的用户表与 config/storage 共享。
//! 登录成功/失败同时写入连接日志（F9）。

use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use argon2::password_hash::{PasswordHash, PasswordHasher, SaltString};
use argon2::{Argon2, PasswordVerifier};
use parking_lot::RwLock;
use rand_core::OsRng;
use unftp_core::auth::{AuthenticationError, Authenticator, Principal, UserDetail, UserDetailError, UserDetailProvider};
use unftp_core::auth::Credentials;

use crate::ftp::config::{FtpConfig, FtpPermissions, FtpUser};
use crate::ftp::log;

/// 会话用户：认证通过后的完整用户信息（权限 + 根目录）
#[derive(Debug, Clone)]
pub struct FtpUserDetail {
    pub username: String,
    pub permissions: FtpPermissions,
    pub root_dir: PathBuf,
}

impl UserDetail for FtpUserDetail {
    fn account_enabled(&self) -> bool {
        true
    }

    /// 返回根目录：libunftp 会话将受限于此目录
    fn home(&self) -> Option<&Path> {
        Some(&self.root_dir)
    }
}

impl fmt::Display for FtpUserDetail {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.username)
    }
}

/// 内存用户表（authenticator / user_detail_provider / storage 共享）
/// password 字段为 argon2 哈希（来源 keyring）
#[derive(Debug, Default)]
pub struct UserStore {
    pub users: Vec<FtpUser>,
    pub root_dir: PathBuf,
}

impl UserStore {
    pub fn from_config(cfg: &FtpConfig) -> Self {
        Self {
            users: cfg.users.clone(),
            root_dir: PathBuf::from(&cfg.root_dir),
        }
    }

    pub fn find(&self, username: &str) -> Option<&FtpUser> {
        self.users.iter().find(|u| u.username == username)
    }
}

/// 自定义 Authenticator：argon2 校验密码
#[derive(Debug)]
pub struct FtpAuthenticator {
    store: Arc<RwLock<UserStore>>,
}

impl FtpAuthenticator {
    pub fn new(store: Arc<RwLock<UserStore>>) -> Self {
        Self { store }
    }
}

#[async_trait::async_trait]
impl Authenticator for FtpAuthenticator {
    async fn authenticate(&self, username: &str, creds: &Credentials) -> Result<Principal, AuthenticationError> {
        // 诊断日志：认证尝试（用户名 + 来源 IP）
        ::log::info!("FTP 认证尝试: user={}, ip={}", username, creds.source_ip);

        let store = self.store.read();
        let Some(user) = store.find(username) else {
            log::push_log(creds.source_ip, username, "登录失败", "用户不存在");
            ::log::info!("FTP 认证失败: user={}, ip={}, 原因=用户不存在", username, creds.source_ip);
            return Err(AuthenticationError::new("用户名或密码错误"));
        };
        // 无哈希（keyring 缺失）→ 拒绝
        if user.password.is_empty() {
            log::push_log(creds.source_ip, username, "登录失败", "凭据缺失");
            ::log::info!("FTP 认证失败: user={}, ip={}, 原因=凭据缺失(keyring 无记录)", username, creds.source_ip);
            return Err(AuthenticationError::new("用户名或密码错误"));
        }
        // argon2 校验（keyring 中存储的是 argon2 哈希，非明文）
        let password = creds.password.as_deref().unwrap_or("");
        let parsed = PasswordHash::new(&user.password).map_err(|_| AuthenticationError::new("凭据损坏"))?;
        let ok = Argon2::default()
            .verify_password(password.as_bytes(), &parsed)
            .is_ok();
        if ok {
            log::push_log(creds.source_ip, username, "登录成功", "CONNECT");
            ::log::info!("FTP 认证成功: user={}, ip={}", username, creds.source_ip);
            Ok(Principal {
                username: username.to_string(),
            })
        } else {
            log::push_log(creds.source_ip, username, "登录失败", "密码错误");
            ::log::info!("FTP 认证失败: user={}, ip={}, 原因=密码错误", username, creds.source_ip);
            Err(AuthenticationError::new("用户名或密码错误"))
        }
    }
}

/// UserDetailProvider：Principal → FtpUserDetail（携带权限与根目录）
#[derive(Debug)]
pub struct FtpUserDetailProvider {
    store: Arc<RwLock<UserStore>>,
}

impl FtpUserDetailProvider {
    pub fn new(store: Arc<RwLock<UserStore>>) -> Self {
        Self { store }
    }
}

#[async_trait::async_trait]
impl UserDetailProvider for FtpUserDetailProvider {
    type User = FtpUserDetail;

    async fn provide_user_detail(&self, principal: &Principal) -> Result<FtpUserDetail, UserDetailError> {
        let store = self.store.read();
        let user = store
            .find(&principal.username)
            .ok_or_else(|| UserDetailError::new("用户不存在"))?;
        Ok(FtpUserDetail {
            username: user.username.clone(),
            permissions: user.permissions.clone(),
            root_dir: store.root_dir.clone(),
        })
    }
}

/// 对明文密码做 argon2 哈希（保存用户时调用）
pub fn hash_password(plain: &str) -> Result<String, String> {
    let salt = SaltString::generate(&mut OsRng);
    Argon2::default()
        .hash_password(plain.as_bytes(), &salt)
        .map(|h| h.to_string())
        .map_err(|e| format!("密码哈希失败: {}", e))
}

/// 校验明文密码是否匹配哈希（供测试与鉴权复用）
pub fn verify_password(plain: &str, hash: &str) -> bool {
    PasswordHash::new(hash)
        .map(|parsed| Argon2::default().verify_password(plain.as_bytes(), &parsed).is_ok())
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hash_and_verify_roundtrip() {
        let hash = hash_password("correct-horse").unwrap();
        assert!(verify_password("correct-horse", &hash));
        assert!(!verify_password("wrong", &hash));
    }

    #[test]
    fn test_user_store_find() {
        let mut cfg = FtpConfig::default();
        cfg.users.push(FtpUser {
            username: "admin".into(),
            password: String::new(),
            permissions: FtpPermissions::default(),
        });
        let store = UserStore::from_config(&cfg);
        assert!(store.find("admin").is_some());
        assert!(store.find("nobody").is_none());
    }
}
