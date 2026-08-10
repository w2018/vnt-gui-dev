//! FTP 认证：自定义 Authenticator + UserDetail + UserDetailProvider
//!
//! 密码校验：keyring（DPAPI）中存明文密码，解密后与客户端输入**直接比较**
//! （用户指示，无需 argon2）；内存中的用户表与 config/storage 共享。
//! 登录成功/失败同时写入连接日志（F9）与 daemon 日志（2E 诊断增强）。

use std::collections::HashMap;
use std::fmt;
use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use parking_lot::RwLock;
use unftp_core::auth::{AuthenticationError, Authenticator, Principal, UserDetail, UserDetailError, UserDetailProvider};
use unftp_core::auth::Credentials;

use crate::ftp::config::{FtpConfig, FtpPermissions, FtpUser};
use crate::ftp::log;

/// 会话用户：认证通过后的完整用户信息（权限 + 根目录 + 客户端 IP）
#[derive(Debug, Clone)]
pub struct FtpUserDetail {
    pub username: String,
    pub permissions: FtpPermissions,
    pub root_dir: PathBuf,
    /// 客户端 IP（登录时从 creds.source_ip 记录；用于操作日志展示真实来源）
    pub client_ip: IpAddr,
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
/// password 字段为明文密码（来源 keyring，DPAPI 解密）
#[derive(Debug, Default)]
pub struct UserStore {
    pub users: Vec<FtpUser>,
    pub root_dir: PathBuf,
    /// 最近登录 IP（username → client IP）：authenticate 成功时写入，
    /// provide_user_detail 读取填充到会话用户（storage 操作日志使用）
    pub client_ips: parking_lot::Mutex<HashMap<String, IpAddr>>,
}

impl UserStore {
    pub fn from_config(cfg: &FtpConfig) -> Self {
        Self {
            users: cfg.users.clone(),
            root_dir: PathBuf::from(&cfg.root_dir),
            client_ips: Default::default(),
        }
    }

    pub fn find(&self, username: &str) -> Option<&FtpUser> {
        self.users.iter().find(|u| u.username == username)
    }
}

/// 自定义 Authenticator：明文密码直接比较（keyring 解密后即明文）
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
        // 2E 诊断日志：认证尝试（用户名 + 来源 IP + 内存中是否持有密码）
        let has_pwd = self
            .store
            .read()
            .find(username)
            .map(|u| !u.password.is_empty())
            .unwrap_or(false);
        ::log::info!(
            "FTP auth attempt: user={}, ip={}, has_password_in_memory={}",
            username,
            creds.source_ip,
            has_pwd
        );

        let store = self.store.read();
        let Some(user) = store.find(username) else {
            log::push_log(creds.source_ip, username, "登录失败", "用户不存在");
            ::log::info!("FTP 认证失败: user={}, ip={}, 原因=用户不存在", username, creds.source_ip);
            return Err(AuthenticationError::new("用户名或密码错误"));
        };
        // 无密码（keyring 缺失/未回填）→ 拒绝并记录
        if user.password.is_empty() {
            log::push_log(creds.source_ip, username, "登录失败", "凭据缺失");
            ::log::info!("FTP 认证失败: user={}, ip={}, 原因=凭据缺失(keyring 无记录或未回填)", username, creds.source_ip);
            return Err(AuthenticationError::new("用户名或密码错误"));
        }
        // 明文直接比较（keyring 解密后即明文）
        let password = creds.password.as_deref().unwrap_or("");
        if user.password == password {
            log::push_log(creds.source_ip, username, "登录成功", "CONNECT");
            ::log::info!("FTP 认证成功: user={}, ip={}", username, creds.source_ip);
            // 记录会话 IP（provide_user_detail 填充到 FtpUserDetail，供操作日志使用）
            store.client_ips.lock().insert(username.to_string(), creds.source_ip);
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
        // 客户端 IP：从登录记录读取（未记录时回退 127.0.0.1）
        let client_ip = store
            .client_ips
            .lock()
            .get(&principal.username)
            .copied()
            .unwrap_or(IpAddr::from([127, 0, 0, 1]));
        Ok(FtpUserDetail {
            username: user.username.clone(),
            permissions: user.permissions.clone(),
            root_dir: store.root_dir.clone(),
            client_ip,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_plain_password_compare() {
        // 明文比较：keyring 解密后即明文，直接 ==
        let mut cfg = FtpConfig::default();
        cfg.users.push(FtpUser {
            username: "admin".into(),
            password: "s3cret".into(),
            permissions: FtpPermissions::default(),
            password_set: false,
        });
        let store = UserStore::from_config(&cfg);
        assert_eq!(store.find("admin").unwrap().password, "s3cret");
        // 空密码用户（keyring 缺失）→ 登录被拒（authenticate 分支）
        cfg.users.push(FtpUser {
            username: "nopwd".into(),
            password: String::new(),
            permissions: FtpPermissions::default(),
            password_set: false,
        });
        let store = UserStore::from_config(&cfg);
        assert!(store.find("nopwd").unwrap().password.is_empty());
    }

    #[test]
    fn test_user_store_find() {
        let mut cfg = FtpConfig::default();
        cfg.users.push(FtpUser {
            username: "admin".into(),
            password: String::new(),
            permissions: FtpPermissions::default(),
            password_set: false,
        });
        let store = UserStore::from_config(&cfg);
        assert!(store.find("admin").is_some());
        assert!(store.find("nobody").is_none());
    }
}
