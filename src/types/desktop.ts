// 桌面共享类型定义（与 Rust 后端 serde 对应）

// 会话角色
export type SessionRole = 'idle' | 'controller' | 'host' | 'both';

// 会话状态（Rust 端 serde tag="type"）
export type SessionState =
  | { type: 'idle' }
  | { type: 'waiting_confirm' }
  | { type: 'connecting' }
  | { type: 'sharing' }
  | { type: 'disconnected'; reason: string }
  | { type: 'error'; message: string };

// 能力配置
export interface ClientCapabilities {
  mouse: boolean;
  keyboard: boolean;
  clipboard: boolean;
  view_only: boolean;
}

export interface GrantedCapabilities {
  mouse: boolean;
  keyboard: boolean;
  clipboard: boolean;
  view_only: boolean;
}

// 屏幕信息
export interface ScreenInfo {
  width: number;
  height: number;
  dpi: number;
  monitor_count: number;
}

// 会话统计
export interface SessionStats {
  fps: number;
  bitrate_kbps: number;
  latency_ms: number;
  uptime_secs: number;
  frames_sent: number;
  frames_dropped: number;
}

// 会话信息
export interface SessionInfo {
  role: SessionRole;
  state: SessionState;
  remote_device: string | null;
  remote_address: string | null;
  capabilities: GrantedCapabilities | null;
  screen: ScreenInfo | null;
  stats: SessionStats;
}

// 捕获设置
export interface CaptureSettings {
  fps: number;
  bitrate_kbps: number;
  width: number;
  height: number;
  monitor: number;
  quality: number;
}

// 桌面共享配置
export interface DesktopShareConfig {
  allow_be_controlled: boolean;
  default_grant: GrantedCapabilities;
  capture: CaptureSettings;
  confirm_timeout_secs: number;
  ffmpeg_path: string;
  clipboard_sync: boolean;
  listen_port: number;
}

// 本机连接信息
export interface DesktopLocalInfo {
  node_id: string;
  listen_addr: string;
  vnt_ip: string;
}

// 输入事件（Rust 侧 enum，前端构造后 invoke）
export type InputEvent =
  | { MouseMove: { x: number; y: number } }
  | { MouseButton: { button: number; pressed: boolean } }
  | { MouseScroll: { delta_x: number; delta_y: number } }
  | { KeyDown: { key: string } }
  | { KeyUp: { key: string } }
  | { SpecialKey: { kind: 'CtrlAltDel' | 'AltTab' | 'WinKey' | 'TaskManager' } }
  | { ClipboardText: { text: string } };

// 连接请求（被控端收到）
export interface ConnectRequest {
  device_name: string;
  client_node_id: string;
  capabilities: ClientCapabilities;
}

// 视频帧头
export interface VideoFrameHeader {
  pts: number;
  is_keyframe: boolean;
  width: number;
  height: number;
  data_len: number;
}

// 视频帧载荷（Channel 二进制传输，data 为 H.264 字节）
export interface VideoFramePayload {
  header: VideoFrameHeader;
  data: number[] | Uint8Array;
}
