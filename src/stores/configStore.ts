// 配置 store：CRUD + 历史切换（全部走后端持久化）

import { create } from 'zustand';
import { api } from '../lib/tauri';
import type { VntConfig } from '../lib/types';

interface ConfigStoreState {
  configs: VntConfig[];
  activeConfigId: string | null;
  loading: boolean;
  refresh: () => Promise<void>;
  save: (config: VntConfig) => Promise<void>;
  remove: (id: string) => Promise<void>;
  setActive: (id: string) => Promise<void>;
}

export const useConfigStore = create<ConfigStoreState>((set, get) => ({
  configs: [],
  activeConfigId: null,
  loading: false,

  refresh: async () => {
    set({ loading: true });
    try {
      const store = await api.getConfigs();
      set({ configs: store.configs, activeConfigId: store.active_config_id });
    } finally {
      set({ loading: false });
    }
  },

  save: async (config) => {
    await api.saveConfig(config);
    await get().refresh();
  },

  remove: async (id) => {
    await api.deleteConfig(id);
    await get().refresh();
  },

  setActive: async (id) => {
    await api.setActiveConfig(id);
    await get().refresh();
  },
}));
