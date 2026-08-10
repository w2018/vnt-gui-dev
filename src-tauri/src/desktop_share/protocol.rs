//! 桌面共享协议：控制端 ↔ 被控端 消息定义
//!
//! 所有消息通过 bincode 序列化后写入 Iroh QUIC stream
//! 帧格式：[u32 长度前缀][bincode 载荷]，视频帧载荷 = bincode(VideoFrameHeader) + 原始 H.264 数据

use serde::{Deserialize, Serialize};

// ==================== 输入事件（控制端 → 被控端） ====================

/// 控制端 → 被控端：鼠标/键盘/剪贴板事件
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum InputEvent {
    /// 绝对坐标移动（控制端 canvas 坐标已映射到被控端分辨率）
    MouseMove { x: i32, y: i32 },
    /// 鼠标按键（1=左 2=右 3=中）
    MouseButton { button: u8, pressed: bool },
    /// 鼠标滚轮
    MouseScroll { delta_x: i32, delta_y: i32 },
    /// 键盘按键（key 为 DOM KeyboardEvent.key 值，如 "a"、"Enter"、"F1"）
    KeyDown { key: String },
    /// 键盘按键释放
    KeyUp { key: String },
    /// Ctrl+Alt+Del 等特殊组合
    SpecialKey { kind: SpecialKeyKind },
    /// 剪贴板文本同步
    ClipboardText { text: String },
}

/// 特殊按键组合
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum SpecialKeyKind {
    CtrlAltDel,
    AltTab,
    WinKey,
    TaskManager,
}

// ==================== 控制消息（双向） ====================

/// 控制端 ↔ 被控端：会话控制消息
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum ControlMsg {
    /// 控制端 → 被控端：请求连接
    ConnectRequest {
        device_name: String,
        client_node_id: String,
        /// 请求的能力
        capabilities: ClientCapabilities,
    },
    /// 被控端 → 控制端：接受连接
    ConnectAccept {
        /// 被控端授予的能力
        granted: GrantedCapabilities,
        /// 被控端屏幕信息
        screen: ScreenInfo,
    },
    /// 被控端 → 控制端：拒绝连接
    ConnectReject { reason: String },
    /// 任意方向：断开连接
    Disconnect { reason: String },
    /// 控制端 → 被控端：配置变更
    ConfigUpdate {
        fps: u32,
        bitrate: u32,
        width: u32,
        height: u32,
    },
    /// 心跳（双向）
    Ping { ts: u64 },
    /// 心跳响应
    Pong { ts: u64 },
    /// 被控端 → 控制端：屏幕分辨率变更通知
    ScreenChanged { width: u32, height: u32 },
}

/// 控制端请求的能力
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub struct ClientCapabilities {
    pub mouse: bool,
    pub keyboard: bool,
    pub clipboard: bool,
    /// true = 只看不控
    pub view_only: bool,
}

/// 被控端授予的能力
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub struct GrantedCapabilities {
    pub mouse: bool,
    pub keyboard: bool,
    pub clipboard: bool,
    pub view_only: bool,
}

impl Default for GrantedCapabilities {
    fn default() -> Self {
        Self {
            mouse: true,
            keyboard: true,
            clipboard: true,
            view_only: false,
        }
    }
}

/// 屏幕信息
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub struct ScreenInfo {
    pub width: u32,
    pub height: u32,
    pub dpi: u32,
    pub monitor_count: u32,
}

// ==================== 视频帧 ====================

/// 视频帧头（与原始 H.264 数据一同写入 stream）
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub struct VideoFrameHeader {
    pub pts: u64,
    pub is_keyframe: bool,
    pub width: u32,
    pub height: u32,
    pub data_len: u32,
}

// ==================== 单测 ====================

#[cfg(test)]
mod tests {
    use super::*;

    fn roundtrip<T: Serialize + for<'de> Deserialize<'de> + PartialEq + std::fmt::Debug>(v: &T) {
        let bytes = bincode::serialize(v).expect("序列化失败");
        let back: T = bincode::deserialize(&bytes).expect("反序列化失败");
        assert_eq!(*v, back);
    }

    #[test]
    fn input_event_roundtrip() {
        roundtrip(&InputEvent::MouseMove { x: 100, y: -50 });
        roundtrip(&InputEvent::MouseButton { button: 2, pressed: true });
        roundtrip(&InputEvent::MouseScroll { delta_x: 0, delta_y: -3 });
        roundtrip(&InputEvent::KeyDown { key: "F5".into() });
        roundtrip(&InputEvent::KeyUp { key: "a".into() });
        roundtrip(&InputEvent::SpecialKey { kind: SpecialKeyKind::AltTab });
        roundtrip(&InputEvent::ClipboardText { text: "你好 VNT".into() });
    }

    #[test]
    fn control_msg_roundtrip() {
        roundtrip(&ControlMsg::ConnectRequest {
            device_name: "PC-01".into(),
            client_node_id: "abc123".into(),
            capabilities: ClientCapabilities {
                mouse: true,
                keyboard: true,
                clipboard: true,
                view_only: false,
            },
        });
        roundtrip(&ControlMsg::ConnectAccept {
            granted: GrantedCapabilities::default(),
            screen: ScreenInfo {
                width: 1920,
                height: 1080,
                dpi: 96,
                monitor_count: 1,
            },
        });
        roundtrip(&ControlMsg::ConnectReject { reason: "繁忙".into() });
        roundtrip(&ControlMsg::Disconnect { reason: "再见".into() });
        roundtrip(&ControlMsg::Ping { ts: 12345 });
        roundtrip(&ControlMsg::Pong { ts: 12345 });
        roundtrip(&ControlMsg::ScreenChanged { width: 2560, height: 1440 });
    }

    #[test]
    fn video_frame_header_roundtrip() {
        roundtrip(&VideoFrameHeader {
            pts: 42,
            is_keyframe: true,
            width: 1920,
            height: 1080,
            data_len: 1024 * 1024,
        });
    }

    #[test]
    fn capabilities_defaults() {
        let g = GrantedCapabilities::default();
        assert!(g.mouse && g.keyboard && g.clipboard && !g.view_only);
    }
}
