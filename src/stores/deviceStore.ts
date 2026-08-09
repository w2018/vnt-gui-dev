// 设备列表 store

import { create } from 'zustand';
import { api } from '../lib/tauri';
import type { PeerInfo } from '../lib/types';

interface DeviceStore {
  devices: PeerInfo[];
  setDevices: (devices: PeerInfo[]) => void;
  refresh: () => Promise<void>;
}

export const useDeviceStore = create<DeviceStore>((set) => ({
  devices: [],
  setDevices: (devices) => set({ devices }),
  refresh: async () => {
    try {
      const devices = await api.getDeviceList();
      set({ devices });
    } catch {
      // 未连接时忽略
    }
  },
}));
