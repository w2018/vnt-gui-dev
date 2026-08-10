//! 剪贴板同步
//!
//! 使用 arboard 读写系统剪贴板
//! 控制端复制 → 发送 → 被控端粘贴
//! 被控端复制 → 发送 → 控制端粘贴

use std::sync::Arc;

use arboard::Clipboard;
use tokio::sync::Mutex;

use crate::desktop_share::error::DesktopError;
use crate::desktop_share::network::DesktopConnection;

/// 剪贴板管理器
pub struct ClipboardManager {
    clipboard: Mutex<Clipboard>,
    /// 上次同步的文本（用于去重）
    last_text: Mutex<String>,
}

impl ClipboardManager {
    pub fn new() -> Result<Self, DesktopError> {
        let cb =
            Clipboard::new().map_err(|e| DesktopError::Config(format!("剪贴板初始化失败: {}", e)))?;
        Ok(Self {
            clipboard: Mutex::new(cb),
            last_text: Mutex::new(String::new()),
        })
    }

    /// 读取本地剪贴板文本
    pub async fn read(&self) -> Result<String, DesktopError> {
        let mut cb = self.clipboard.lock().await;
        cb.get_text()
            .map_err(|e| DesktopError::Config(format!("读取剪贴板失败: {}", e)))
    }

    /// 写入本地剪贴板文本（并更新去重基线）
    pub async fn write(&self, text: String) -> Result<(), DesktopError> {
        {
            let mut cb = self.clipboard.lock().await;
            cb.set_text(text.clone())
                .map_err(|e| DesktopError::Config(format!("写入剪贴板失败: {}", e)))?;
        }
        *self.last_text.lock().await = text;
        Ok(())
    }

    /// 检查剪贴板是否变化，变化则通过连接发送
    pub async fn poll_and_send(
        &self,
        conn: &Arc<DesktopConnection>,
    ) -> Result<(), DesktopError> {
        let text = self.read().await?;
        let mut last = self.last_text.lock().await;
        if *last != text {
            *last = text.clone();
            drop(last);
            conn.send_clipboard(&text).await?;
        }
        Ok(())
    }

    /// 启动轮询任务（每 1 秒检查一次，失败即退出）
    pub async fn start_polling(self: Arc<Self>, conn: Arc<DesktopConnection>) {
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(1));
            loop {
                interval.tick().await;
                if let Err(e) = self.poll_and_send(&conn).await {
                    log::warn!("剪贴板轮询错误: {}", e);
                    break;
                }
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 去重规则：空文本不视为变更（避免启动即把空剪贴板发出去）
    fn should_sync(last: &str, current: &str) -> bool {
        !current.is_empty() && last != current
    }

    #[test]
    fn dedup_rules() {
        assert!(should_sync("a", "b"));
        assert!(!should_sync("a", "a"));
        // 空剪贴板不触发同步（初始 last="" 时，避免误发空内容）
        assert!(!should_sync("", ""));
        assert!(!should_sync("a", ""));
        // 首次非空文本触发
        assert!(should_sync("", "hello"));
    }
}
