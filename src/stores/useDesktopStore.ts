// 桌面共享 Zustand store

import { create } from 'zustand';
import { desktopApi } from '../lib/desktopApi';
import type {
  ConnectRequest,
  DesktopLocalInfo,
  DesktopShareConfig,
  InputEvent,
  SessionInfo,
} from '../types/desktop';

interface DesktopStoreState {
  // 初始化状态
  initialized: boolean;
  initError: string | null;
  localInfo: DesktopLocalInfo | null;

  // 会话
  session: SessionInfo;

  // 配置
  config: DesktopShareConfig | null;
  encoderAvailable: boolean | null;

  // 待处理的连接请求（被控端）
  pendingRequest: ConnectRequest | null;

  // 连接目标（控制端）
  targetIp: string;
  viewOnly: boolean;

  // 最近收到的剪贴板内容（接收方向展示）
  lastClipboard: string;

  // actions
  init: () => Promise<void>;
  refreshSession: () => Promise<void>;
  setTargetIp: (ip: string) => void;
  setViewOnly: (v: boolean) => void;
  connect: () => Promise<void>;
  disconnect: (reason?: string) => Promise<void>;
  acceptRequest: (opts: {
    mouse: boolean;
    keyboard: boolean;
    clipboard: boolean;
    viewOnly: boolean;
  }) => Promise<void>;
  rejectRequest: (reason: string) => Promise<void>;
  startSharing: () => Promise<void>;
  stopSharing: () => Promise<void>;
  loadConfig: () => Promise<void>;
  saveConfig: (patch: Partial<DesktopShareConfig>) => Promise<void>;
  checkEncoder: () => Promise<void>;
  sendInput: (event: InputEvent) => Promise<void>;
  setupListeners: () => Promise<() => void>;
}

const defaultSession: SessionInfo = {
  role: 'idle',
  state: { type: 'idle' },
  remote_device: null,
  remote_address: null,
  capabilities: null,
  screen: null,
  stats: {
    fps: 0,
    bitrate_kbps: 0,
    latency_ms: 0,
    uptime_secs: 0,
    frames_sent: 0,
    frames_dropped: 0,
  },
};

export const useDesktopStore = create<DesktopStoreState>((set, get) => ({
  initialized: false,
  initError: null,
  localInfo: null,
  session: defaultSession,
  config: null,
  encoderAvailable: null,
  pendingRequest: null,
  targetIp: '',
  viewOnly: false,
  lastClipboard: '',

  init: async () => {
    if (get().initialized) return;
    try {
      await desktopApi.init();
      const info = await desktopApi.getLocalInfo();
      set({ initialized: true, localInfo: info, initError: null });
      await get().refreshSession();
      await get().loadConfig();
      await get().checkEncoder();
    } catch (e) {
      set({ initError: String(e) });
      throw e;
    }
  },

  refreshSession: async () => {
    try {
      const session = await desktopApi.getSession();
      set({ session });
    } catch {
      // 忽略瞬时错误
    }
  },

  setTargetIp: (ip) => set({ targetIp: ip }),
  setViewOnly: (v) => set({ viewOnly: v }),

  connect: async () => {
    const { targetIp, viewOnly, localInfo } = get();
    const ip = targetIp.trim();
    if (!ip) throw new Error('请输入目标 VNT IP 地址');
    const deviceName = localInfo?.node_id || 'unknown-device';
    await desktopApi.connect(ip, 0, deviceName, viewOnly);
    await get().refreshSession();
  },

  disconnect: async (reason = '用户断开') => {
    await desktopApi.disconnect(reason);
    await get().refreshSession();
  },

  acceptRequest: async (opts) => {
    await desktopApi.acceptRequest(opts.mouse, opts.keyboard, opts.clipboard, opts.viewOnly);
    set({ pendingRequest: null });
    await get().refreshSession();
  },

  rejectRequest: async (reason) => {
    await desktopApi.rejectRequest(reason);
    set({ pendingRequest: null });
    await get().refreshSession();
  },

  startSharing: async () => {
    await desktopApi.startSharing();
    await get().refreshSession();
  },

  stopSharing: async () => {
    await desktopApi.stopSharing();
    await get().refreshSession();
  },

  loadConfig: async () => {
    const cfg = await desktopApi.getConfig();
    set({ config: cfg });
  },

  saveConfig: async (patch) => {
    const current = get().config;
    if (!current) throw new Error('配置未加载');
    const next: DesktopShareConfig = { ...current, ...patch };
    await desktopApi.saveConfig(next);
    set({ config: next });
  },

  checkEncoder: async () => {
    try {
      const ok = await desktopApi.checkEncoder();
      set({ encoderAvailable: ok });
    } catch {
      set({ encoderAvailable: false });
    }
  },

  sendInput: async (event) => {
    await desktopApi.sendInput(event);
  },

  setupListeners: async () => {
    const unlisteners: Array<() => void> = [];

    unlisteners.push(
      await desktopApi.onSessionUpdate((info) => {
        set({ session: info });
      }),
    );

    unlisteners.push(
      await desktopApi.onConnectRequest((req) => {
        set({ pendingRequest: req });
      }),
    );

    unlisteners.push(
      await desktopApi.onClipboard((text) => {
        set({ lastClipboard: text });
      }),
    );

    return () => unlisteners.forEach((fn) => fn());
  },
}));
