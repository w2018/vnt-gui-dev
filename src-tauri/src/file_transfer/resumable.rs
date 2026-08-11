//! 断点续传支持
//!
//! 传输中的文件保存为 `.partial` 临时文件，顺序写入，**不预分配空间**，
//! 因此文件实际字节数即已接收偏移量（`resume_offset = metadata.len()`）。
//! 传输完成后重命名为正式文件名，中断时保留 `.partial` 供下次续传。

use std::path::{Path, PathBuf};

use tokio::fs::OpenOptions;
use tokio::io::AsyncWriteExt;

/// 打开/创建 .partial 文件（不预分配、不截断，保留已有数据用于续传）
pub async fn open_partial(partial_path: &Path) -> Result<tokio::fs::File, String> {
    OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(partial_path)
        .await
        .map_err(|e| format!("打开 .partial 失败: {}", e))
}

/// 获取已接收偏移量 = .partial 文件当前大小（不存在返回 0）
pub async fn resume_offset(partial_path: &Path) -> Result<u64, String> {
    if !partial_path.exists() {
        return Ok(0);
    }
    let len = tokio::fs::metadata(partial_path)
        .await
        .map_err(|e| format!("获取 .partial 元数据失败: {}", e))?
        .len();
    Ok(len)
}

/// 校验 .partial 现有数据量不超过期望大小（防御异常残留）
pub async fn validate_resume(partial_path: &Path, expected_size: u64) -> Result<u64, String> {
    let offset = resume_offset(partial_path).await?;
    if offset > expected_size {
        // 残留比目标还大（异常），删除重传
        let _ = tokio::fs::remove_file(partial_path).await;
        return Ok(0);
    }
    Ok(offset)
}

/// 将数据追加写入文件并刷盘（每块写入后 flush，保证中断后字节数准确）
pub async fn write_chunk(
    file: &mut tokio::fs::File,
    data: &[u8],
    write_offset: u64,
) -> Result<(), String> {
    tokio::io::AsyncSeekExt::seek(file, std::io::SeekFrom::Start(write_offset))
        .await
        .map_err(|e| format!("定位写入位置失败: {}", e))?;
    file.write_all(data)
        .await
        .map_err(|e| format!("写入 .partial 失败: {}", e))?;
    file.flush().await.map_err(|e| format!("刷盘失败: {}", e))?;
    Ok(())
}

/// 传输完成后重命名 .partial → 正式文件（若目标已存在先删除）
pub async fn finalize(partial_path: &Path, final_path: &Path) -> Result<(), String> {
    if final_path.exists() {
        tokio::fs::remove_file(final_path)
            .await
            .map_err(|e| format!("删除旧文件失败: {}", e))?;
    }
    tokio::fs::rename(partial_path, final_path)
        .await
        .map_err(|e| format!("重命名失败: {}", e))?;
    Ok(())
}

/// 清理残留的 .partial 文件（返回被清理的文件名列表）
pub async fn cleanup_orphans(dir: &Path) -> Result<Vec<String>, String> {
    let mut orphans = Vec::new();
    let mut entries = tokio::fs::read_dir(dir)
        .await
        .map_err(|e| format!("读取目录失败: {}", e))?;

    while let Some(entry) = entries
        .next_entry()
        .await
        .map_err(|e| format!("读取目录项失败: {}", e))?
    {
        let name = entry.file_name().to_string_lossy().to_string();
        if name.ends_with(".partial") {
            orphans.push(name);
        }
    }
    Ok(orphans)
}

/// 列出 .partial 文件的完整路径（用于续传检测）
pub async fn partial_paths(dir: &Path) -> Result<Vec<PathBuf>, String> {
    let mut out = Vec::new();
    let mut entries = tokio::fs::read_dir(dir)
        .await
        .map_err(|e| format!("读取目录失败: {}", e))?;
    while let Some(entry) = entries
        .next_entry()
        .await
        .map_err(|e| format!("读取目录项失败: {}", e))?
    {
        if entry.file_name().to_string_lossy().ends_with(".partial") {
            out.push(entry.path());
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::AsyncWriteExt;

    #[tokio::test]
    async fn resume_offset_zero_when_missing() {
        let dir = tempfile::tempdir().expect("临时目录");
        let p = dir.path().join("none.partial");
        assert_eq!(resume_offset(&p).await.expect("查询"), 0);
    }

    #[tokio::test]
    async fn open_write_resume_cycle() {
        let dir = tempfile::tempdir().expect("临时目录");
        let p = dir.path().join("test.partial");

        // 第一段：写 100 字节
        let mut f = open_partial(&p).await.expect("打开");
        let first = vec![0xAB; 100];
        write_chunk(&mut f, &first, 0).await.expect("写入第一段");
        assert_eq!(resume_offset(&p).await.expect("偏移量"), 100);

        // 模拟中断：重新打开（保留数据），续写第二段
        let mut f2 = open_partial(&p).await.expect("重开");
        let second = vec![0xCD; 50];
        write_chunk(&mut f2, &second, 100).await.expect("写入第二段");
        assert_eq!(resume_offset(&p).await.expect("偏移量"), 150);

        // 读取全文校验内容
        let content = tokio::fs::read(&p).await.expect("读取");
        assert_eq!(content.len(), 150);
        assert_eq!(&content[..100], &first[..]);
        assert_eq!(&content[100..], &second[..]);
    }

    #[tokio::test]
    async fn validate_resume_rejects_oversized() {
        let dir = tempfile::tempdir().expect("临时目录");
        let p = dir.path().join("oversized.partial");
        // 写入超出期望大小的残留
        let mut f = open_partial(&p).await.expect("打开");
        f.write_all(&vec![0u8; 500]).await.expect("写入");
        f.flush().await.expect("flush");

        let offset = validate_resume(&p, 100).await.expect("校验");
        assert_eq!(offset, 0, "超限残留应删除并归零");
        assert!(!p.exists(), "超限残留应被删除");
    }

    #[tokio::test]
    async fn finalize_renames_and_overwrites() {
        let dir = tempfile::tempdir().expect("临时目录");
        let partial = dir.path().join("x.partial");
        let final_path = dir.path().join("x.txt");

        let mut f = open_partial(&partial).await.expect("打开");
        f.write_all(b"hello").await.expect("写入");
        f.flush().await.expect("flush");

        // 目标已存在 → 覆盖
        tokio::fs::write(&final_path, b"old").await.expect("旧文件");
        finalize(&partial, &final_path).await.expect("finalize");

        assert!(!partial.exists(), ".partial 应被移除");
        let content = tokio::fs::read_to_string(&final_path).await.expect("读取");
        assert_eq!(content, "hello");
    }

    #[tokio::test]
    async fn cleanup_and_list_orphans() {
        let dir = tempfile::tempdir().expect("临时目录");
        tokio::fs::write(dir.path().join("a.partial"), b"a").await.expect("a");
        tokio::fs::write(dir.path().join("b.partial"), b"b").await.expect("b");
        tokio::fs::write(dir.path().join("c.txt"), b"c").await.expect("c");

        let orphans = cleanup_orphans(dir.path()).await.expect("清理");
        assert_eq!(orphans.len(), 2);
        assert!(orphans.contains(&"a.partial".to_string()));
        assert!(orphans.contains(&"b.partial".to_string()));

        let paths = partial_paths(dir.path()).await.expect("列出");
        assert_eq!(paths.len(), 2);
    }
}
