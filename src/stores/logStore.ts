// 日志 store：环形追加 + 过滤

import { create } from 'zustand';
import type { LogEntry, LogLevel } from '../lib/types';

interface LogStore {
  logs: LogEntry[];
  filterLevel: LogLevel | 'all';
  searchTerm: string;
  appendLog: (entry: LogEntry) => void;
  setLogs: (logs: LogEntry[]) => void;
  setFilterLevel: (level: LogLevel | 'all') => void;
  setSearchTerm: (term: string) => void;
}

export const useLogStore = create<LogStore>((set) => ({
  logs: [],
  filterLevel: 'all',
  searchTerm: '',
  appendLog: (entry) =>
    set((state) => {
      const logs = [...state.logs, entry];
      if (logs.length > 2000) logs.splice(0, logs.length - 2000);
      return { logs };
    }),
  setLogs: (logs) => set({ logs }),
  setFilterLevel: (filterLevel) => set({ filterLevel }),
  setSearchTerm: (searchTerm) => set({ searchTerm }),
}));
