// 桌面共享 Tauri API 封装

import { Channel, invoke } from '@tauri-apps/api/core';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import type {
  ConnectRequest,
  DesktopLocalInfo,
  DesktopShareConfig,
  InputEvent,
  SessionInfo,
  VideoFramePayload,
} from '../types/desktop';

export const desktopApi = {
  // 初始化
  init: () => invoke<void>('desktop_init'),

  // 本地信息
  getLocalInfo: () => invoke<DesktopLocalInfo>('desktop_get_local_info'),

  // 会话管理
  getSession: () => invoke<SessionInfo>('desktop_get_session'),
  connect: (remote_ip: string, remote_port: number, device_name: string, view_only: boolean) =>
    invoke<void>('desktop_connect', {
      remoteIp: remote_ip,
      remotePort: remote_port,
      deviceName: device_name,
      viewOnly: view_only,
    }),
  acceptRequest: (
    grant_mouse: boolean,
    grant_keyboard: boolean,
    grant_clipboard: boolean,
    view_only: boolean,
  ) =>
    invoke<void>('desktop_accept_request', {
      grantMouse: grant_mouse,
      grantKeyboard: grant_keyboard,
      grantClipboard: grant_clipboard,
      viewOnly: view_only,
    }),
  rejectRequest: (reason: string) => invoke<void>('desktop_reject_request', { reason }),
  disconnect: (reason: string) => invoke<void>('desktop_disconnect', { reason }),

  // 屏幕共享
  startSharing: () => invoke<void>('desktop_start_sharing'),
  stopSharing: () => invoke<void>('desktop_stop_sharing'),

  // 视频帧通道（Channel 二进制传输）
  setVideoChannel: (channel: Channel<VideoFramePayload>) =>
    invoke<void>('desktop_set_video_channel', { channel }),

  // 输入
  sendInput: (event: InputEvent) => invoke<void>('desktop_send_input', { event }),

  // 配置
  getConfig: () => invoke<DesktopShareConfig>('desktop_get_config'),
  saveConfig: (cfg: DesktopShareConfig) => invoke<void>('desktop_save_config', { cfg }),

  // 编码器检查（Media Foundation H.264）
  checkEncoder: () => invoke<boolean>('desktop_check_encoder'),

  // 事件监听
  onSessionUpdate: (cb: (info: SessionInfo) => void): Promise<UnlistenFn> =>
    listen<SessionInfo>('desktop-session-update', (e) => cb(e.payload)),
  onConnectRequest: (cb: (req: ConnectRequest) => void): Promise<UnlistenFn> =>
    listen<ConnectRequest>('desktop-connect-request', (e) => cb(e.payload)),
  onClipboard: (cb: (text: string) => void): Promise<UnlistenFn> =>
    listen<string>('desktop-clipboard', (e) => cb(e.payload)),
};
