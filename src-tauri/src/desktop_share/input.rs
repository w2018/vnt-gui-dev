//! 输入模拟引擎
//!
//! 使用 enigo 0.6.1 模拟键盘鼠标事件（Windows 底层为 SendInput）
//! ⚠️ 必须在 GUI 进程运行（daemon 无前台会话，无法模拟输入）
//! ⚠️ Windows 下 UAC 弹窗 / 安全桌面会阻断输入模拟

use std::sync::Mutex;

use enigo::{Axis, Button, Coordinate, Direction, Enigo, Key, Keyboard, Mouse, Settings};

use crate::desktop_share::error::DesktopError;
use crate::desktop_share::protocol::{InputEvent, SpecialKeyKind};

/// 输入模拟器
pub struct InputSimulator {
    enigo: Mutex<Enigo>,
    /// 是否启用输入模拟（被控端可在设置中关闭）
    enabled: Mutex<bool>,
}

impl InputSimulator {
    pub fn new() -> Result<Self, DesktopError> {
        let enigo = Enigo::new(&Settings::default())
            .map_err(|e| DesktopError::Input(format!("初始化 enigo 失败: {}", e)))?;
        Ok(Self {
            enigo: Mutex::new(enigo),
            enabled: Mutex::new(true),
        })
    }

    /// 设置是否启用
    pub fn set_enabled(&self, enabled: bool) {
        *self
            .enabled
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = enabled;
    }

    /// 处理输入事件（失败仅记录日志，不中断）
    pub fn handle_event(&self, event: InputEvent) {
        if !*self
            .enabled
            .lock()
            .unwrap_or_else(|e| e.into_inner())
        {
            log::debug!("输入模拟已禁用，忽略事件");
            return;
        }

        let mut enigo = self.enigo.lock().unwrap_or_else(|e| e.into_inner());

        match event {
            InputEvent::MouseMove { x, y } => {
                if let Err(e) = enigo.move_mouse(x, y, Coordinate::Abs) {
                    log::warn!("鼠标移动失败: {}", e);
                }
            }
            InputEvent::MouseButton { button, pressed } => {
                let btn = match button {
                    1 => Button::Left,
                    2 => Button::Right,
                    3 => Button::Middle,
                    _ => {
                        log::warn!("未知鼠标按键: {}", button);
                        return;
                    }
                };
                let dir = if pressed {
                    Direction::Press
                } else {
                    Direction::Release
                };
                if let Err(e) = enigo.button(btn, dir) {
                    log::warn!("鼠标按键失败: {}", e);
                }
            }
            InputEvent::MouseScroll { delta_x, delta_y } => {
                // delta 为前端 wheel 事件原始值，除以 100 换算为格数
                if delta_x != 0 {
                    let len = delta_x / 100;
                    if len != 0 {
                        if let Err(e) = enigo.scroll(len, Axis::Horizontal) {
                            log::warn!("水平滚轮失败: {}", e);
                        }
                    }
                }
                if delta_y != 0 {
                    let len = delta_y / 100;
                    if len != 0 {
                        if let Err(e) = enigo.scroll(len, Axis::Vertical) {
                            log::warn!("垂直滚轮失败: {}", e);
                        }
                    }
                }
            }
            InputEvent::KeyDown { key } => {
                if let Some(k) = map_key(&key) {
                    if let Err(e) = enigo.key(k, Direction::Press) {
                        log::warn!("按键按下失败: {} ({})", key, e);
                    }
                } else {
                    log::debug!("忽略未知按键: {}", key);
                }
            }
            InputEvent::KeyUp { key } => {
                if let Some(k) = map_key(&key) {
                    if let Err(e) = enigo.key(k, Direction::Release) {
                        log::warn!("按键释放失败: {} ({})", key, e);
                    }
                } else {
                    log::debug!("忽略未知按键: {}", key);
                }
            }
            InputEvent::SpecialKey { kind } => handle_special(&mut enigo, kind),
            InputEvent::ClipboardText { .. } => {
                // 剪贴板由 clipboard 模块处理
            }
        }
    }
}

/// 特殊组合键
fn handle_special(enigo: &mut Enigo, kind: SpecialKeyKind) {
    match kind {
        SpecialKeyKind::CtrlAltDel => {
            // Windows 安全机制：普通应用无法模拟，需用户手动操作
            log::warn!("Ctrl+Alt+Del 受系统安全机制保护，无法模拟，请被控端用户手动操作");
        }
        SpecialKeyKind::AltTab => {
            let _ = enigo.key(Key::Alt, Direction::Press);
            let _ = enigo.key(Key::Tab, Direction::Press);
            let _ = enigo.key(Key::Tab, Direction::Release);
            let _ = enigo.key(Key::Alt, Direction::Release);
        }
        SpecialKeyKind::WinKey => {
            let _ = enigo.key(Key::Meta, Direction::Press);
            let _ = enigo.key(Key::Meta, Direction::Release);
        }
        SpecialKeyKind::TaskManager => {
            // Ctrl+Shift+Esc
            let _ = enigo.key(Key::Control, Direction::Press);
            let _ = enigo.key(Key::Shift, Direction::Press);
            let _ = enigo.key(Key::Escape, Direction::Press);
            let _ = enigo.key(Key::Escape, Direction::Release);
            let _ = enigo.key(Key::Shift, Direction::Release);
            let _ = enigo.key(Key::Control, Direction::Release);
        }
    }
}

/// 将 DOM KeyboardEvent.key 字符串映射为 enigo Key
/// 返回 None 表示未知按键（调用方忽略）
fn map_key(key: &str) -> Option<Key> {
    match key {
        "Enter" | "Return" => Some(Key::Return),
        "Tab" => Some(Key::Tab),
        "Escape" | "Esc" => Some(Key::Escape),
        "Backspace" => Some(Key::Backspace),
        "Delete" | "Del" => Some(Key::Delete),
        "ArrowUp" | "Up" => Some(Key::UpArrow),
        "ArrowDown" | "Down" => Some(Key::DownArrow),
        "ArrowLeft" | "Left" => Some(Key::LeftArrow),
        "ArrowRight" | "Right" => Some(Key::RightArrow),
        "Home" => Some(Key::Home),
        "End" => Some(Key::End),
        "PageUp" => Some(Key::PageUp),
        "PageDown" => Some(Key::PageDown),
        " " | "Spacebar" => Some(Key::Space),
        "CapsLock" => Some(Key::CapsLock),
        "Shift" => Some(Key::Shift),
        "Control" | "Ctrl" => Some(Key::Control),
        "Alt" => Some(Key::Alt),
        "Meta" | "Win" | "Super" => Some(Key::Meta),
        "F1" => Some(Key::F1),
        "F2" => Some(Key::F2),
        "F3" => Some(Key::F3),
        "F4" => Some(Key::F4),
        "F5" => Some(Key::F5),
        "F6" => Some(Key::F6),
        "F7" => Some(Key::F7),
        "F8" => Some(Key::F8),
        "F9" => Some(Key::F9),
        "F10" => Some(Key::F10),
        "F11" => Some(Key::F11),
        "F12" => Some(Key::F12),
        s if s.chars().count() == 1 => {
            let ch = s.chars().next()?;
            if ch.is_control() {
                None
            } else {
                Some(Key::Unicode(ch))
            }
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn map_named_keys() {
        assert_eq!(map_key("Enter"), Some(Key::Return));
        assert_eq!(map_key("Tab"), Some(Key::Tab));
        assert_eq!(map_key("Escape"), Some(Key::Escape));
        assert_eq!(map_key("Backspace"), Some(Key::Backspace));
        assert_eq!(map_key("Delete"), Some(Key::Delete));
        assert_eq!(map_key("ArrowUp"), Some(Key::UpArrow));
        assert_eq!(map_key("ArrowDown"), Some(Key::DownArrow));
        assert_eq!(map_key("ArrowLeft"), Some(Key::LeftArrow));
        assert_eq!(map_key("ArrowRight"), Some(Key::RightArrow));
        assert_eq!(map_key("Home"), Some(Key::Home));
        assert_eq!(map_key("End"), Some(Key::End));
        assert_eq!(map_key("PageUp"), Some(Key::PageUp));
        assert_eq!(map_key("PageDown"), Some(Key::PageDown));
        assert_eq!(map_key(" "), Some(Key::Space));
        assert_eq!(map_key("CapsLock"), Some(Key::CapsLock));
        assert_eq!(map_key("Shift"), Some(Key::Shift));
        assert_eq!(map_key("Control"), Some(Key::Control));
        assert_eq!(map_key("Alt"), Some(Key::Alt));
        assert_eq!(map_key("Meta"), Some(Key::Meta));
        assert_eq!(map_key("F1"), Some(Key::F1));
        assert_eq!(map_key("F12"), Some(Key::F12));
    }

    #[test]
    fn map_unicode_single_char() {
        assert_eq!(map_key("a"), Some(Key::Unicode('a')));
        assert_eq!(map_key("A"), Some(Key::Unicode('A')));
        assert_eq!(map_key("中"), Some(Key::Unicode('中')));
        assert_eq!(map_key("1"), Some(Key::Unicode('1')));
    }

    #[test]
    fn map_unknown_returns_none() {
        assert_eq!(map_key("F13"), None);
        assert_eq!(map_key("MediaPlayPause"), None);
        assert_eq!(map_key(""), None);
        assert_eq!(map_key("ab"), None);
        // 控制字符不映射
        assert_eq!(map_key("\u{1}"), None);
    }
}
