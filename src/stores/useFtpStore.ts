// FTP 服务 Zustand store（需求 F1-F9 状态管理）

import { create } from 'zustand';
import { api } from '../lib/tauri';
import { defaultFtpConfig } from '../types/ftp';
import type { FtpConfig, FtpLogEntry, FtpServerStatus, FtpUser } from '../types/ftp';

interface FtpState {
  config: FtpConfig;
  status: FtpServerStatus;
  logs: FtpLogEntry[];
  loading: boolean;
  saving: boolean;

  /** 加载配置 + 状态 + 日志 */
  loadAll: () => Promise<void>;
  /** 保存配置（密码由后端处理，不回传） */
  saveConfig: (patch: Partial<FtpConfig>) => Promise<void>;
  /** 直接保存完整配置 */
  persist: (cfg: FtpConfig) => Promise<void>;
  start: () => Promise<void>;
  stop: () => Promise<void>;
  pickRootDir: () => Promise<string | null>;
  addUser: (user: FtpUser) => Promise<void>;
  removeUser: (username: string) => Promise<void>;
  updateUser: (username: string, patch: Partial<FtpUser>) => Promise<void>;
  refreshStatus: () => Promise<void>;
  refreshLogs: () => Promise<void>;
}

export const useFtpStore = create<FtpState>((set, get) => ({
  config: defaultFtpConfig(),
  status: { state: 'stopped', listen_addr: null, error: null },
  logs: [],
  loading: false,
  saving: false,

  loadAll: async () => {
    set({ loading: true });
    try {
      const [config, status, logs] = await Promise.all([
        api.ftpGetConfig(),
        api.ftpStatus(),
        api.ftpGetLogs(),
      ]);
      set({ config, status, logs });
    } catch (e) {
      console.error('FTP 数据加载失败', e);
    } finally {
      set({ loading: false });
    }
  },

  saveConfig: async (patch) => {
    const next = { ...get().config, ...patch };
    await get().persist(next);
  },

  persist: async (cfg) => {
    set({ saving: true });
    try {
      await api.ftpSaveConfig(cfg);
      set({ config: { ...cfg, users: cfg.users.map((u) => ({ ...u, password: '' })) } });
      // 保存后状态可能因自动重启变化
      await get().refreshStatus();
    } catch (e) {
      console.error('FTP 配置保存失败', e);
      throw e;
    } finally {
      set({ saving: false });
    }
  },

  start: async () => {
    await api.ftpStart();
    await get().refreshStatus();
  },

  stop: async () => {
    await api.ftpStop();
    await get().refreshStatus();
  },

  pickRootDir: async () => {
    const p = await api.ftpPickRootDir();
    return p || null;
  },

  addUser: async (user) => {
    const cfg = get().config;
    if (cfg.users.some((u) => u.username === user.username)) {
      throw new Error('用户名已存在');
    }
    const users = [...cfg.users, user];
    await get().persist({ ...cfg, users });
  },

  removeUser: async (username) => {
    const cfg = get().config;
    const users = cfg.users.filter((u) => u.username !== username);
    await get().persist({ ...cfg, users });
  },

  updateUser: async (username, patch) => {
    const cfg = get().config;
    const users = cfg.users.map((u) => (u.username === username ? { ...u, ...patch } : u));
    await get().persist({ ...cfg, users });
  },

  refreshStatus: async () => {
    const status = await api.ftpStatus();
    set({ status });
  },

  refreshLogs: async () => {
    const logs = await api.ftpGetLogs();
    set({ logs });
  },
}));
