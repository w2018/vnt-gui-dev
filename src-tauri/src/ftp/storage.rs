//! 自定义 StorageBackend：限制 ROOT 目录 + 用户权限拦截（需求 F6）
//!
//! 安全设计：
//! - 所有路径先经 `resolve()` 规范化：拒绝绝对路径（盘符/UNC/POSIX）、
//!   `..` 越界直接拒绝 —— 路径穿越（path traversal）在入口处被拦截；
//! - 写操作（put/mkd/rmd/del/rename）按用户权限校验；
//! - readonly=true 用户所有写操作一律 PermissionDenied。

use std::io;
use std::path::{Component, Path, PathBuf};

use async_trait::async_trait;
use tokio::io::AsyncWriteExt;
use unftp_core::storage::{Error, ErrorKind, Fileinfo, Metadata, Result, StorageBackend};

use crate::ftp::auth::FtpUserDetail;
use crate::ftp::log;

/// 文件元数据包装（unftp Metadata trait 适配 std::fs::Metadata）
#[derive(Debug)]
pub struct FsMeta(pub std::fs::Metadata);

impl Metadata for FsMeta {
    fn len(&self) -> u64 {
        self.0.len()
    }
    fn is_empty(&self) -> bool {
        self.0.len() == 0
    }
    fn is_dir(&self) -> bool {
        self.0.is_dir()
    }
    fn is_file(&self) -> bool {
        self.0.is_file()
    }
    fn is_symlink(&self) -> bool {
        self.0.file_type().is_symlink()
    }
    fn modified(&self) -> Result<std::time::SystemTime> {
        self.0
            .modified()
            .map_err(|e| Error::new(ErrorKind::TransientFileNotAvailable, e))
    }
    fn gid(&self) -> u32 {
        0
    }
    fn uid(&self) -> u32 {
        0
    }
    fn links(&self) -> u64 {
        1
    }
    fn permissions(&self) -> unftp_core::storage::Permissions {
        unftp_core::storage::Permissions(if self.0.is_dir() { 0o755 } else { 0o644 })
    }
}

/// FTP 存储后端：限制在根目录内 + 权限拦截
#[derive(Debug)]
pub struct FtpStorage {
    root: PathBuf,
}

impl FtpStorage {
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }

    /// 路径规范化 + 前缀校验（穿越防护核心）
    ///
    /// FTP 语义：`/xxx` 表示 FTP 根（root_dir）下的路径 → 前导 RootDir 放行；
    /// 拒绝：盘符（`C:`）/ UNC（`\\`）前缀、路径中间出现根分隔符、`..` 越界。
    /// 返回 root 内的绝对路径。
    fn resolve(&self, user: &FtpUserDetail, path: &Path) -> Result<PathBuf> {
        // 客户端传入路径为 session.cwd.join(path)，可能形如 "/sub/dir"、"/../x"、"C:\x"
        let mut parts: Vec<std::ffi::OsString> = Vec::new();
        for comp in path.components() {
            match comp {
                Component::Normal(c) => parts.push(c.to_os_string()),
                Component::CurDir => {}
                Component::ParentDir => {
                    // 越界（试图逃出根目录）→ 拒绝
                    if parts.pop().is_none() {
                        return Err(Error::new(ErrorKind::PermissionDenied, "path traversal blocked"));
                    }
                }
                // 前导 "/"（FTP 根）放行；中间的 RootDir 或任何 Prefix（盘符/UNC）→ 拒绝
                Component::RootDir => {
                    if !parts.is_empty() {
                        return Err(Error::new(
                            ErrorKind::PermissionDenied,
                            format!("root separator in path not allowed: {:?}", path),
                        ));
                    }
                }
                _ => {
                    return Err(Error::new(
                        ErrorKind::PermissionDenied,
                        format!("absolute path not allowed: {:?}", path),
                    ));
                }
            }
        }
        let mut full = user.root_dir.clone();
        for p in parts {
            full.push(p);
        }
        Ok(full)
    }

    /// 权限检查：写操作统一入口
    fn check_write(&self, user: &FtpUserDetail, action: &str) -> Result<()> {
        let perms = &user.permissions;
        if perms.readonly {
            log::push_log_anon(user, action, "拒绝：只读用户");
            return Err(Error::new(ErrorKind::PermissionDenied, "readonly user"));
        }
        let allowed = match action {
            "上传" => perms.upload,
            "删除" => perms.delete,
            "重命名" => perms.upload,
            _ => false,
        };
        if !allowed {
            log::push_log_anon(user, action, "拒绝：无权限");
            return Err(Error::new(ErrorKind::PermissionDenied, format!("no {} permission", action)));
        }
        Ok(())
    }

    /// 读权限检查
    fn check_read(&self, user: &FtpUserDetail) -> Result<()> {
        if !user.permissions.download {
            return Err(Error::new(ErrorKind::PermissionDenied, "no download permission"));
        }
        Ok(())
    }

    /// 客户端可见路径（root 内相对路径）
    fn display_path(&self, full: &Path) -> String {
        full.strip_prefix(&self.root)
            .map(|p| p.to_string_lossy().replace('\\', "/"))
            .unwrap_or_else(|_| full.to_string_lossy().to_string())
    }
}

#[async_trait]
impl StorageBackend<FtpUserDetail> for FtpStorage {
    type Metadata = FsMeta;

    async fn metadata<P: AsRef<Path> + Send + std::fmt::Debug>(
        &self,
        user: &FtpUserDetail,
        path: P,
    ) -> Result<Self::Metadata> {
        let full = self.resolve(user, path.as_ref())?;
        let meta = tokio::fs::metadata(&full)
            .await
            .map_err(|e| Error::new(ErrorKind::TransientFileNotAvailable, e))?;
        Ok(FsMeta(meta))
    }

    async fn list<P: AsRef<Path> + Send + std::fmt::Debug>(
        &self,
        user: &FtpUserDetail,
        path: P,
    ) -> Result<Vec<Fileinfo<PathBuf, Self::Metadata>>> {
        let full = self.resolve(user, path.as_ref())?;
        let mut entries = Vec::new();
        let mut rd = tokio::fs::read_dir(&full)
            .await
            .map_err(|e| Error::new(ErrorKind::TransientFileNotAvailable, e))?;
        while let Some(entry) = rd
            .next_entry()
            .await
            .map_err(|e| Error::new(ErrorKind::TransientFileNotAvailable, e))?
        {
            let meta = entry
                .metadata()
                .await
                .map_err(|e| Error::new(ErrorKind::TransientFileNotAvailable, e))?;
            entries.push(Fileinfo {
                path: PathBuf::from(entry.file_name()),
                metadata: FsMeta(meta),
            });
        }
        Ok(entries)
    }

    async fn get<P: AsRef<Path> + Send + std::fmt::Debug>(
        &self,
        user: &FtpUserDetail,
        path: P,
        start_pos: u64,
    ) -> Result<Box<dyn tokio::io::AsyncRead + Send + Sync + Unpin>> {
        self.check_read(user)?;
        let full = self.resolve(user, path.as_ref())?;
        use tokio::io::AsyncSeekExt;
        let mut file = tokio::fs::File::open(&full)
            .await
            .map_err(|e| Error::new(ErrorKind::TransientFileNotAvailable, e))?;
        if start_pos > 0 {
            file.seek(io::SeekFrom::Start(start_pos))
                .await
                .map_err(|e| Error::new(ErrorKind::TransientFileNotAvailable, e))?;
        }
        log::push_log_anon(user, "下载", &self.display_path(&full));
        Ok(Box::new(file))
    }

    async fn put<P: AsRef<Path> + Send + std::fmt::Debug, R: tokio::io::AsyncRead + Send + Sync + Unpin + 'static>(
        &self,
        user: &FtpUserDetail,
        input: R,
        path: P,
        start_pos: u64,
    ) -> Result<u64> {
        self.check_write(user, "上传")?;
        let full = self.resolve(user, path.as_ref())?;
        use tokio::io::AsyncSeekExt;
        let mut opts = tokio::fs::OpenOptions::new();
        opts.write(true).create(true);
        if start_pos == 0 {
            opts.truncate(true);
        }
        let mut file = opts
            .open(&full)
            .await
            .map_err(|e| Error::new(ErrorKind::TransientFileNotAvailable, e))?;
        if start_pos > 0 {
            file.seek(io::SeekFrom::Start(start_pos))
                .await
                .map_err(|e| Error::new(ErrorKind::TransientFileNotAvailable, e))?;
        }
        let mut input = input;
        let n = tokio::io::copy(&mut input, &mut file)
            .await
            .map_err(|e| Error::new(ErrorKind::TransientFileNotAvailable, e))?;
        file.flush()
            .await
            .map_err(|e| Error::new(ErrorKind::TransientFileNotAvailable, e))?;
        log::push_log_anon(user, "上传", &self.display_path(&full));
        Ok(n)
    }

    async fn del<P: AsRef<Path> + Send + std::fmt::Debug>(&self, user: &FtpUserDetail, path: P) -> Result<()> {
        self.check_write(user, "删除")?;
        let full = self.resolve(user, path.as_ref())?;
        tokio::fs::remove_file(&full)
            .await
            .map_err(|e| Error::new(ErrorKind::TransientFileNotAvailable, e))?;
        log::push_log_anon(user, "删除", &self.display_path(&full));
        Ok(())
    }

    async fn mkd<P: AsRef<Path> + Send + std::fmt::Debug>(&self, user: &FtpUserDetail, path: P) -> Result<()> {
        self.check_write(user, "上传")?;
        let full = self.resolve(user, path.as_ref())?;
        tokio::fs::create_dir(&full)
            .await
            .map_err(|e| Error::new(ErrorKind::TransientFileNotAvailable, e))?;
        log::push_log_anon(user, "新建目录", &self.display_path(&full));
        Ok(())
    }

    async fn rmd<P: AsRef<Path> + Send + std::fmt::Debug>(&self, user: &FtpUserDetail, path: P) -> Result<()> {
        self.check_write(user, "删除")?;
        let full = self.resolve(user, path.as_ref())?;
        tokio::fs::remove_dir(&full)
            .await
            .map_err(|e| Error::new(ErrorKind::TransientFileNotAvailable, e))?;
        log::push_log_anon(user, "删除目录", &self.display_path(&full));
        Ok(())
    }

    async fn rename<P: AsRef<Path> + Send + std::fmt::Debug>(
        &self,
        user: &FtpUserDetail,
        from: P,
        to: P,
    ) -> Result<()> {
        self.check_write(user, "重命名")?;
        let from_full = self.resolve(user, from.as_ref())?;
        let to_full = self.resolve(user, to.as_ref())?;
        tokio::fs::rename(&from_full, &to_full)
            .await
            .map_err(|e| Error::new(ErrorKind::TransientFileNotAvailable, e))?;
        log::push_log_anon(
            user,
            "重命名",
            &format!("{} → {}", self.display_path(&from_full), self.display_path(&to_full)),
        );
        Ok(())
    }

    async fn cwd<P: AsRef<Path> + Send + std::fmt::Debug>(&self, user: &FtpUserDetail, path: P) -> Result<()> {
        let full = self.resolve(user, path.as_ref())?;
        let meta = tokio::fs::metadata(&full)
            .await
            .map_err(|e| Error::new(ErrorKind::TransientFileNotAvailable, e))?;
        if !meta.is_dir() {
            return Err(Error::new(
                ErrorKind::TransientFileNotAvailable,
                io::Error::new(io::ErrorKind::NotADirectory, "not a dir"),
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ftp::config::FtpPermissions;
    use crate::ftp::auth::FtpUserDetail;
    use tokio::io::AsyncReadExt;

    fn user(perms: FtpPermissions, root: PathBuf) -> FtpUserDetail {
        FtpUserDetail {
            username: "tester".into(),
            permissions: perms,
            root_dir: root,
            client_ip: std::net::IpAddr::from([127, 0, 0, 1]),
        }
    }

    fn full_perms() -> FtpPermissions {
        FtpPermissions {
            upload: true,
            download: true,
            delete: true,
            readonly: false,
        }
    }

    fn readonly_perms() -> FtpPermissions {
        FtpPermissions {
            upload: true,
            download: true,
            delete: true,
            readonly: true,
        }
    }

    #[test]
    fn test_path_traversal_blocked() {
        // V2 要求：传入 ../../etc/passwd 类路径，验证被拦截
        let root = PathBuf::from("C:\\ftp_root");
        let u = user(full_perms(), root.clone());
        let storage = FtpStorage::new(root);

        // 越界 ..（根目录 pop 空 → 拒绝）
        let p = storage.resolve(&u, Path::new("/../../etc/passwd"));
        assert!(p.is_err(), "越界 .. 必须被拦截: {:?}", p);
        let p = storage.resolve(&u, Path::new("sub/../../../etc/passwd"));
        assert!(p.is_err());

        // 绝对路径（Windows 盘符）
        let p = storage.resolve(&u, Path::new("C:\\windows\\system32"));
        assert!(p.is_err());

        // FTP 语义：前导 "/" 是 FTP 根（root_dir），放行
        let p = storage.resolve(&u, Path::new("/etc/passwd")).unwrap();
        assert_eq!(p, PathBuf::from("C:\\ftp_root\\etc\\passwd"));

        // 正常相对路径放行
        let p = storage.resolve(&u, Path::new("sub/dir/file.txt")).unwrap();
        assert_eq!(p, PathBuf::from("C:\\ftp_root\\sub\\dir\\file.txt"));
    }

    #[test]
    fn test_permission_denied_returns_error() {
        // V2 要求：无 delete 权限的用户执行 del → PermissionDenied
        let root = tempfile::tempdir().unwrap();
        let perms = FtpPermissions {
            upload: true,
            download: true,
            delete: false,
            readonly: false,
        };
        let u = user(perms, root.path().to_path_buf());
        let storage = FtpStorage::new(root.path().to_path_buf());
        let rt = tokio::runtime::Runtime::new().unwrap();
        let err = rt.block_on(storage.del(&u, Path::new("somefile.txt")));
        let err = err.unwrap_err();
        assert_eq!(err.kind(), ErrorKind::PermissionDenied);
    }

    #[tokio::test]
    async fn test_readonly_user_cannot_upload() {
        // V2 要求：readonly=true 时 put 被拒
        let root = tempfile::tempdir().unwrap();
        let u = user(readonly_perms(), root.path().to_path_buf());
        let storage = FtpStorage::new(root.path().to_path_buf());
        let data: &[u8] = b"evil payload";
        let err = storage
            .put(&u, data, Path::new("upload.txt"), 0)
            .await
            .unwrap_err();
        assert_eq!(err.kind(), ErrorKind::PermissionDenied);
        // 文件确实未被创建
        assert!(!root.path().join("upload.txt").exists());
    }

    #[tokio::test]
    async fn test_delete_file_succeeds() {
        // 有删除权限用户：上传 → 删除 → 文件消失（正向删除验证）
        let root = tempfile::tempdir().unwrap();
        let u = user(full_perms(), root.path().to_path_buf());
        let storage = FtpStorage::new(root.path().to_path_buf());

        storage
            .put(&u, std::io::Cursor::new(b"to-delete".to_vec()), Path::new("f.txt"), 0)
            .await
            .unwrap();
        assert!(root.path().join("f.txt").exists());
        storage.del(&u, Path::new("f.txt")).await.unwrap();
        assert!(!root.path().join("f.txt").exists());
    }

    #[tokio::test]
    async fn test_upload_download_roundtrip() {
        // 有权限用户：上传 → 下载 → 内容一致
        let root = tempfile::tempdir().unwrap();
        let u = user(full_perms(), root.path().to_path_buf());
        let storage = FtpStorage::new(root.path().to_path_buf());

        let content = b"hello ftp world".to_vec();
        // 先建子目录
        storage.mkd(&u, Path::new("a")).await.unwrap();
        let n = storage
            .put(&u, std::io::Cursor::new(content.clone()), Path::new("a/b.txt"), 0)
            .await
            .unwrap();
        assert_eq!(n as usize, content.len());

        let mut reader = storage.get(&u, Path::new("a/b.txt"), 0).await.unwrap();
        let mut out = Vec::new();
        reader.read_to_end(&mut out).await.unwrap();
        assert_eq!(out, content);
    }
}
