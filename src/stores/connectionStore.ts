// 连接状态 store（文档 §4.3）

import { create } from 'zustand';

type StatusKey = 'stopped' | 'starting' | 'connected' | 'reconnecting' | 'error';

interface ConnectionStore {
  status: StatusKey;
  virtualIp: string | null;
  serverAddress: string | null;
  latency: number | null;
  errorMessage: string | null;
  setStatus: (s: StatusKey) => void;
  setVirtualIp: (ip: string | null) => void;
  setServerAddress: (addr: string | null) => void;
  setLatency: (ms: number | null) => void;
  setError: (msg: string | null) => void;
}

export const useConnectionStore = create<ConnectionStore>((set) => ({
  status: 'stopped',
  virtualIp: null,
  serverAddress: null,
  latency: null,
  errorMessage: null,
  setStatus: (status) => set({ status }),
  setVirtualIp: (virtualIp) => set({ virtualIp }),
  setServerAddress: (serverAddress) => set({ serverAddress }),
  setLatency: (latency) => set({ latency }),
  setError: (errorMessage) => set({ errorMessage }),
}));
