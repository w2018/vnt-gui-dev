//! 文件类型过滤
//!
//! 决定哪些文件类型可自动接收（白名单/黑名单），不匹配的文件类型将自动拒绝。

use serde::{Deserialize, Serialize};
use std::collections::HashSet;

/// 过滤模式
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum FilterMode {
    /// 白名单：仅允许列表中的类型自动接收
    Whitelist,
    /// 黑名单：列表中的类型拒绝
    Blacklist,
    /// 全部允许
    AllowAll,
    /// 全部拒绝（仅手动确认）
    DenyAll,
}

/// 文件类型过滤器
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FileTypeFilter {
    pub mode: FilterMode,
    /// 扩展名集合（不含点，小写）
    pub extensions: HashSet<String>,
}

impl Default for FileTypeFilter {
    fn default() -> Self {
        // 默认白名单：常见安全文件类型
        let mut exts = HashSet::new();
        for ext in [
            "txt", "pdf", "doc", "docx", "xls", "xlsx", "ppt", "pptx",
            "png", "jpg", "jpeg", "gif", "bmp", "webp",
            "mp3", "mp4", "wav", "flac",
            "zip", "rar", "7z", "tar", "gz",
            "json", "xml", "csv", "md",
            "rs", "ts", "tsx", "js", "py", "go", "java", "c", "cpp", "h",
        ] {
            exts.insert(ext.to_string());
        }
        Self { mode: FilterMode::Whitelist, extensions: exts }
    }
}

impl FileTypeFilter {
    /// 检查文件类型是否允许（扩展名不含点）
    pub fn is_allowed(&self, extension: &str) -> bool {
        let ext = extension.trim_start_matches('.').to_lowercase();
        if ext.is_empty() {
            // 无扩展名文件：白名单模式下视为不允许，其余模式走模式判定
            return matches!(self.mode, FilterMode::AllowAll)
                || matches!(self.mode, FilterMode::Blacklist);
        }
        match self.mode {
            FilterMode::AllowAll => true,
            FilterMode::DenyAll => false,
            FilterMode::Whitelist => self.extensions.contains(&ext),
            FilterMode::Blacklist => !self.extensions.contains(&ext),
        }
    }

    /// 添加扩展名
    pub fn add_extension(&mut self, ext: &str) {
        self.extensions.insert(ext.trim_start_matches('.').to_lowercase());
    }

    /// 移除扩展名
    pub fn remove_extension(&mut self, ext: &str) {
        self.extensions.remove(&ext.trim_start_matches('.').to_lowercase());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn default_filter() -> FileTypeFilter {
        FileTypeFilter::default()
    }

    #[test]
    fn whitelist_allows_known_types() {
        let f = default_filter();
        assert!(f.is_allowed("txt"));
        assert!(f.is_allowed("PDF")); // 大小写不敏感
        assert!(f.is_allowed(".png")); // 容忍点前缀
        assert!(f.is_allowed("md"));
        assert!(f.is_allowed("mp4"));
    }

    #[test]
    fn whitelist_rejects_unknown_types() {
        let f = default_filter();
        assert!(!f.is_allowed("exe"));
        assert!(!f.is_allowed("dll"));
        assert!(!f.is_allowed("bat"));
        assert!(!f.is_allowed("")); // 无扩展名文件在白名单下拒绝
    }

    #[test]
    fn blacklist_rejects_listed_types() {
        let mut f = FileTypeFilter { mode: FilterMode::Blacklist, extensions: HashSet::new() };
        f.add_extension("exe");
        f.add_extension("dll");
        assert!(!f.is_allowed("exe"));
        assert!(!f.is_allowed("dll"));
        assert!(f.is_allowed("txt"));
        assert!(f.is_allowed(""));
    }

    #[test]
    fn allow_all_and_deny_all() {
        let f = FileTypeFilter { mode: FilterMode::AllowAll, extensions: HashSet::new() };
        assert!(f.is_allowed("exe"));
        assert!(f.is_allowed(""));
        assert!(f.is_allowed("anything"));

        let g = FileTypeFilter { mode: FilterMode::DenyAll, extensions: HashSet::new() };
        assert!(!g.is_allowed("txt"));
        assert!(!g.is_allowed(""));
    }

    #[test]
    fn add_and_remove_extension() {
        let mut f = default_filter();
        f.add_extension("EXE"); // 应转为小写
        assert!(f.is_allowed("exe"));
        f.remove_extension("exe");
        assert!(!f.is_allowed("exe"));
    }

    #[test]
    fn filter_json_roundtrip() {
        let f = default_filter();
        let json = serde_json::to_string(&f).expect("序列化失败");
        let back: FileTypeFilter = serde_json::from_str(&json).expect("反序列化失败");
        assert_eq!(f, back);
    }
}
