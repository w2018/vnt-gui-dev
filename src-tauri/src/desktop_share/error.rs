//! 桌面共享错误类型

use thiserror::Error;

/// 桌面共享模块错误
#[derive(Debug, Error)]
pub enum DesktopError {
    #[error("网络错误: {0}")]
    Network(String),
    #[error("连接错误: {0}")]
    Connection(String),
    #[error("协议错误: {0}")]
    Protocol(String),
    #[error("捕获错误: {0}")]
    Capture(String),
    #[error("输入模拟错误: {0}")]
    Input(String),
    #[error("配置错误: {0}")]
    Config(String),
}

impl From<DesktopError> for String {
    fn from(e: DesktopError) -> Self {
        e.to_string()
    }
}

/// Windows API 错误转 DesktopError（仅 Windows 目标）
#[cfg(windows)]
impl From<windows::core::Error> for DesktopError {
    fn from(e: windows::core::Error) -> Self {
        DesktopError::Capture(format!("Windows API 错误: {}", e))
    }
}
