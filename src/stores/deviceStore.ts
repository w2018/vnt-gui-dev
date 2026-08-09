// 设备列表 store

import { create } from 'zustand';
import { api } from '../lib/tauri';
import type { PeerInfo } from '../lib/types';

interface DeviceStore {
  devices: PeerInfo[];
  setDevices: (devices: PeerInfo[]) => void;
  /** 更新单台设备延迟（-1 = 超时），用于 ping 结果绑定 */
  updateLatency: (ip: string, ms: number) => void;
  refresh: () => Promise<void>;
}

export const useDeviceStore = create<DeviceStore>((set) => ({
  devices: [],
  setDevices: (devices) => set({ devices }),
  updateLatency: (ip, ms) =>
    set((s) => ({
      devices: s.devices.map((d) => (d.virtual_ip === ip ? { ...d, latency: ms } : d)),
    })),
  refresh: async () => {
    try {
      const devices = await api.getDeviceList();
      set({ devices });
    } catch {
      // 未连接时忽略
    }
  },
}));
