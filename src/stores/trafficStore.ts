// 流量 store：60 秒滚动历史 + 总量

import { create } from 'zustand';
import type { TrafficSnapshot } from '../lib/types';

interface TrafficPoint {
  time: string;
  upload: number;
  download: number;
}

interface TrafficStore {
  current: TrafficSnapshot | null;
  points: TrafficPoint[];
  totalUpload: number;
  totalDownload: number;
  updateTraffic: (snap: TrafficSnapshot) => void;
  setAlertThreshold: (up: number, down: number) => void;
}

const MAX_POINTS = 60;

export const useTrafficStore = create<TrafficStore>((set) => ({
  current: null,
  points: [],
  totalUpload: 0,
  totalDownload: 0,

  updateTraffic: (snap) =>
    set((state) => {
      const time = new Date().toLocaleTimeString('zh-CN', { hour12: false });
      const point: TrafficPoint = {
        time,
        upload: Math.round(snap.upload_speed),
        download: Math.round(snap.download_speed),
      };
      const points = [...state.points, point];
      if (points.length > MAX_POINTS) points.splice(0, points.length - MAX_POINTS);
      return {
        current: snap,
        points,
        totalUpload: snap.upload_bytes,
        totalDownload: snap.download_bytes,
      };
    }),

  setAlertThreshold: () => {
    // 告警阈值由设置页持久化，Phase 4 实现
  },
}));
