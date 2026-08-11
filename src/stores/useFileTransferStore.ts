// 文件传输 Zustand Store

import { create } from 'zustand';
import { fileTransferApi } from '../lib/fileTransferApi';
import type {
  FileOffer,
  FileTypeFilter,
  PendingFile,
  TextMessage,
  TransferRecord,
  TransferTask,
} from '../types/file_transfer';

/** 监听注册幂等标志（App.tsx 全局注册一次，页面内不再重复） */
let listenersActive = false;

/** 终态（用于触发历史刷新） */
const FINAL_STATUS = ['Completed', 'Failed', 'Cancelled', 'Rejected'];

interface FileTransferStore {
  // ========== 状态 ==========
  transfers: TransferTask[];
  history: TransferRecord[];
  /** 当前激活标签（active=传输中 / finished=已完成终止，供 FileTransfer 页受控 Tabs） */
  activeTab: string;
  /** 跳转激活标签（点击发送 → active；传输中无任务 → finished） */
  setActiveTab: (tab: string) => void;
  /** 待确认接收请求队列（并发 offer 逐个展示，避免弹窗互相覆盖） */
  pendingOffers: FileOffer[];
  filter: FileTypeFilter | null;
  autoAccept: boolean;
  /** 通道阈值（MB） */
  thresholdMB: number;
  textMessages: TextMessage[];
  /** 目标设备虚拟 IP */
  targetIp: string | null;
  /** 待发送文件列表（拖拽/选择加入，手动发送） */
  pendingFiles: PendingFile[];
  /** 默认保存目录 */
  saveDir: string;

  // ========== Actions ==========
  setTargetIp: (ip: string | null) => void;
  init: () => Promise<void>;
  refreshTransfers: () => Promise<void>;
  refreshHistory: () => Promise<void>;
  sendFile: (filePath: string) => Promise<void>;
  sendFiles: (filePaths: string[]) => Promise<void>;
  sendText: (text: string) => Promise<void>;
  acceptOffer: (transferId: number, savePath: string) => Promise<void>;
  rejectOffer: (transferId: number, reason: string) => Promise<void>;
  /** 从待确认队列移除（确认/拒绝/超时关闭时调用） */
  dismissOffer: (transferId: number) => void;
  cancelTransfer: (transferId: number) => Promise<void>;
  pauseTransfer: (transferId: number) => Promise<void>;
  removeTask: (transferId: number) => Promise<void>;
  removeTaskBatch: (transferIds: number[]) => Promise<void>;
  deleteHistory: (transferId: number) => Promise<void>;
  deleteHistoryBatch: (transferIds: number[]) => Promise<void>;
  clearHistory: () => Promise<void>;
  updateFilter: (patch: Partial<FileTypeFilter>) => Promise<void>;
  setThreshold: (mb: number) => Promise<void>;
  setAutoAccept: (enabled: boolean) => Promise<void>;
  setSaveDir: (path: string) => Promise<void>;
  addPendingFiles: (files: PendingFile[]) => void;
  removePendingFile: (path: string) => void;
  clearPending: () => void;
  /** 发送全部待发送文件，返回失败数 */
  sendAllPending: () => Promise<number>;
  /** 发送单个待发送文件 */
  sendOnePending: (path: string) => Promise<void>;
  setupListeners: () => Promise<() => void>;
}

export const useFileTransferStore = create<FileTransferStore>((set, get) => ({
  transfers: [],
  history: [],
  activeTab: 'active',
  setActiveTab: (tab) => set({ activeTab: tab }),
  pendingOffers: [],
  filter: null,
  autoAccept: false,
  thresholdMB: 100,
  textMessages: [],
  targetIp: null,
  pendingFiles: [],
  saveDir: '',

  setTargetIp: (ip) => set({ targetIp: ip }),

  init: async () => {
    try {
      const [transfers, history, filter, settings] = await Promise.all([
        fileTransferApi.getTransfers(),
        fileTransferApi.getHistory(null, null, 200),
        fileTransferApi.getFilter(),
        fileTransferApi.getSettings().catch(() => null),
      ]);
      set({
        transfers,
        history,
        filter,
        autoAccept: settings?.auto_accept ?? false,
        thresholdMB: settings ? Math.max(1, Math.round(settings.threshold / 1024 / 1024)) : 100,
        saveDir: settings?.save_dir ?? '',
      });
    } catch (e) {
      console.error('初始化文件传输失败:', e);
    }
  },

  refreshTransfers: async () => {
    try {
      const transfers = await fileTransferApi.getTransfers();
      set({ transfers });
    } catch {
      /* 忽略瞬时错误 */
    }
  },

  refreshHistory: async () => {
    try {
      const history = await fileTransferApi.getHistory(null, null, 200);
      set({ history });
    } catch {
      /* 忽略瞬时错误 */
    }
  },

  sendFile: async (filePath) => {
    const ip = get().targetIp;
    if (!ip) throw new Error('请先选择目标设备');
    set({ activeTab: 'active' }); // 点击发送 → 自动跳转"传输中"
    await fileTransferApi.sendFile(filePath, ip);
    await get().refreshTransfers();
  },

  sendFiles: async (filePaths) => {
    const ip = get().targetIp;
    if (!ip) throw new Error('请先选择目标设备');
    set({ activeTab: 'active' }); // 点击发送 → 自动跳转"传输中"
    await fileTransferApi.sendFiles(filePaths, ip);
    await get().refreshTransfers();
  },

  sendText: async (text) => {
    const ip = get().targetIp;
    if (!ip) throw new Error('请先选择目标设备');
    await fileTransferApi.sendText(text, ip);
  },

  acceptOffer: async (transferId, savePath) => {
    try {
      await fileTransferApi.acceptFile(transferId, savePath);
    } catch (e) {
      // 后端可能已超时处理（请求不存在）→ 从队列移除并向上抛出（组件提示原因）
      set((s) => ({
        pendingOffers: s.pendingOffers.filter((o) => o.transfer_id !== transferId),
      }));
      throw e;
    } finally {
      set((s) => ({
        pendingOffers: s.pendingOffers.filter((o) => o.transfer_id !== transferId),
      }));
      await Promise.all([get().refreshTransfers(), get().refreshHistory()]);
    }
  },

  rejectOffer: async (transferId, reason) => {
    try {
      await fileTransferApi.rejectFile(transferId, reason);
    } catch {
      // 后端可能已超时处理（请求不存在）→ 从队列移除
    } finally {
      set((s) => ({
        pendingOffers: s.pendingOffers.filter((o) => o.transfer_id !== transferId),
      }));
      await get().refreshTransfers();
    }
  },

  dismissOffer: (transferId) => {
    set((s) => ({
      pendingOffers: s.pendingOffers.filter((o) => o.transfer_id !== transferId),
    }));
  },

  cancelTransfer: async (transferId) => {
    await fileTransferApi.cancelTransfer(transferId, '用户取消');
    await get().refreshTransfers();
  },

  pauseTransfer: async (transferId) => {
    await fileTransferApi.pauseTransfer(transferId);
    await get().refreshTransfers();
  },

  removeTask: async (transferId) => {
    await fileTransferApi.removeTask(transferId);
    await get().refreshTransfers();
  },

  removeTaskBatch: async (transferIds) => {
    for (const id of transferIds) {
      await fileTransferApi.removeTask(id);
    }
    await get().refreshTransfers();
  },

  deleteHistory: async (transferId) => {
    await fileTransferApi.deleteHistory(transferId);
    await get().refreshHistory();
  },

  deleteHistoryBatch: async (transferIds) => {
    await fileTransferApi.deleteHistoryBatch(transferIds);
    await get().refreshHistory();
  },

  clearHistory: async () => {
    await fileTransferApi.clearHistory();
    set({ history: [] });
  },

  updateFilter: async (patch) => {
    const current = get().filter;
    if (!current) return;
    const next: FileTypeFilter = { ...current, ...patch } as FileTypeFilter;
    await fileTransferApi.saveFilter(next);
    set({ filter: next });
  },

  setThreshold: async (mb) => {
    await fileTransferApi.setThreshold(mb * 1024 * 1024);
    set({ thresholdMB: mb });
  },

  setAutoAccept: async (enabled) => {
    await fileTransferApi.setAutoAccept(enabled);
    set({ autoAccept: enabled });
  },

  setSaveDir: async (path) => {
    await fileTransferApi.setSaveDir(path);
    set({ saveDir: path });
  },

  addPendingFiles: (files) => {
    set((s) => {
      const existing = new Set(s.pendingFiles.map((p) => p.path));
      const fresh = files.filter((f) => !existing.has(f.path));
      return { pendingFiles: [...s.pendingFiles, ...fresh] };
    });
  },

  removePendingFile: (path) => {
    set((s) => ({ pendingFiles: s.pendingFiles.filter((p) => p.path !== path) }));
  },

  clearPending: () => set({ pendingFiles: [] }),

  sendAllPending: async () => {
    const ip = get().targetIp;
    if (!ip) throw new Error('请先选择目标设备');
    const files = get().pendingFiles;
    if (files.length === 0) return 0;
    set({ activeTab: 'active' }); // 点击发送 → 自动跳转"传输中"
    const succeeded: string[] = [];
    let failed = 0;
    for (const f of files) {
      try {
        await fileTransferApi.sendFile(f.path, ip);
        succeeded.push(f.path);
      } catch {
        failed++;
      }
    }
    if (succeeded.length > 0) {
      set((s) => ({
        pendingFiles: s.pendingFiles.filter((p) => !succeeded.includes(p.path)),
      }));
    }
    await get().refreshTransfers();
    return failed;
  },

  sendOnePending: async (path) => {
    const ip = get().targetIp;
    if (!ip) throw new Error('请先选择目标设备');
    set({ activeTab: 'active' }); // 点击发送 → 自动跳转"传输中"
    await fileTransferApi.sendFile(path, ip);
    set((s) => ({ pendingFiles: s.pendingFiles.filter((p) => p.path !== path) }));
    await get().refreshTransfers();
  },

  setupListeners: async () => {
    if (listenersActive) {
      return () => undefined;
    }
    listenersActive = true;

    const unlisteners: Array<() => void> = [];

    // 接收请求 → 加入待确认队列（并发 offer 逐个弹窗展示）
    unlisteners.push(
      await fileTransferApi.onFileOffer((offer) => {
        set((s) => ({
          pendingOffers: s.pendingOffers.some((o) => o.transfer_id === offer.transfer_id)
            ? s.pendingOffers
            : [...s.pendingOffers, offer],
        }));
      }),
    );

    // 传输进度/状态更新；终态时自动刷新历史并移除对应待确认请求
    unlisteners.push(
      await fileTransferApi.onTransferUpdate((task) => {
        set((state) => {
          const idx = state.transfers.findIndex((t) => t.transfer_id === task.transfer_id);
          const next = [...state.transfers];
          if (idx >= 0) {
            next[idx] = task;
          } else {
            next.push(task);
          }
          return { transfers: next };
        });
        if (FINAL_STATUS.includes(task.status)) {
          void get().refreshHistory();
          // 后端超时自动拒绝（任务变 Rejected）→ 从待确认队列移除对应请求
          set((s) => ({
            pendingOffers: s.pendingOffers.filter((o) => o.transfer_id !== task.transfer_id),
          }));
        }
      }),
    );

    // 文本消息
    unlisteners.push(
      await fileTransferApi.onTextMessage((msg) => {
        set((state) => ({
          textMessages: [...state.textMessages, msg].slice(-100),
        }));
      }),
    );

    return () => {
      unlisteners.forEach((fn) => fn());
      listenersActive = false;
    };
  },
}));
