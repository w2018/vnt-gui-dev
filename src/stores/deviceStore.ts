// 设备列表 store

import { create } from 'zustand';
import { api } from '../lib/tauri';
import type { PeerInfo } from '../lib/types';

interface DeviceStore {
  devices: PeerInfo[];
  /** 本机在组网中的设备信息（连接信息展示用） */
  localPeer: PeerInfo | null;
  setDevices: (devices: PeerInfo[]) => void;
  /** 更新单台设备延迟（-1 = 超时），用于 ping 结果绑定 */
  updateLatency: (ip: string, ms: number) => void;
  refresh: () => Promise<void>;
}

export const useDeviceStore = create<DeviceStore>((set) => ({
  devices: [],
  localPeer: null,
  setDevices: (devices) => set({ devices }),
  updateLatency: (ip, ms) =>
    set((s) => ({
      devices: s.devices.map((d) => (d.virtual_ip === ip ? { ...d, latency: ms } : d)),
    })),
  refresh: async () => {
    try {
      const r = await api.getDeviceList();
      // 合并刷新：保留已有 latency（ping 结果），避免每轮 --list 后延迟归零造成闪烁
      set((s) => {
        const old = new Map(s.devices.map((d) => [d.virtual_ip, d.latency]));
        const oldLocal = s.localPeer?.virtual_ip ? s.localPeer.latency : null;
        return {
          devices: r.devices.map((d) => ({
            ...d,
            latency: old.get(d.virtual_ip) ?? d.latency,
          })),
          localPeer: r.local
            ? { ...r.local, latency: oldLocal ?? r.local.latency }
            : null,
        };
      });
    } catch {
      // 未连接时忽略
    }
  },
}));
