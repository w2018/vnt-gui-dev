// 文件传输 Tauri API 封装

import { invoke } from '@tauri-apps/api/core';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import type {
  FileInfo,
  FileOffer,
  FileTypeFilter,
  TextMessage,
  TransferDirection,
  TransferRecord,
  TransferTask,
} from '../types/file_transfer';

/** 文件传输设置（后端 file_transfer_config.json） */
export interface FileTransferSettings {
  mode: FileTypeFilter['mode'];
  extensions: string[];
  auto_accept: boolean;
  threshold: number;
  save_dir: string;
}

export const fileTransferApi = {
  // ========== 发送 ==========
  /** 发送文件（自动选择通道） */
  sendFile: (filePath: string, remoteIp: string) =>
    invoke<void>('file_send', { filePath, remoteIp }),

  /** 批量发送文件（列队串行） */
  sendFiles: (filePaths: string[], remoteIp: string) =>
    invoke<void>('file_send_batch', { filePaths, remoteIp }),

  /** 发送文本 */
  sendText: (text: string, remoteIp: string) =>
    invoke<void>('file_send_text', { text, remoteIp }),

  // ========== 接收 ==========
  /** 接受文件（savePath 为最终保存路径） */
  acceptFile: (transferId: number, savePath: string) =>
    invoke<void>('file_accept', { transferId, savePath }),

  /** 拒绝文件 */
  rejectFile: (transferId: number, reason: string) =>
    invoke<void>('file_reject', { transferId, reason }),

  /** 设置自动接收 */
  setAutoAccept: (enabled: boolean) => invoke<void>('file_set_auto_accept', { enabled }),

  // ========== 管理 ==========
  /** 取消传输 */
  cancelTransfer: (transferId: number, reason: string) =>
    invoke<void>('file_cancel', { transferId, reason }),

  /** 暂停传输（保留在传输中列表，接收端保留断点，可继续） */
  pauseTransfer: (transferId: number) => invoke<void>('file_pause', { transferId }),

  /** 从传输列表移除任务（仅移除记录，不删除文件、不影响历史） */
  removeTask: (transferId: number) => invoke<void>('file_remove_task', { transferId }),

  /** 获取传输任务列表 */
  getTransfers: () => invoke<TransferTask[]>('file_get_transfers'),

  /** 获取历史记录 */
  getHistory: (
    direction?: TransferDirection | null,
    keyword?: string | null,
    limit?: number,
  ) =>
    invoke<TransferRecord[]>('file_get_history', {
      direction: direction ?? null,
      keyword: keyword ?? null,
      limit: limit ?? 100,
    }),

  /** 删除单条历史（按持久化自增 id） */
  deleteHistory: (id: number) => invoke<void>('file_delete_history', { id }),

  /** 批量删除历史（按持久化自增 id） */
  deleteHistoryBatch: (ids: number[]) =>
    invoke<void>('file_delete_history_batch', { ids }),

  /** 清空历史 */
  clearHistory: () => invoke<void>('file_clear_history'),

  // ========== 配置 ==========
  /** 获取过滤器配置 */
  getFilter: () => invoke<FileTypeFilter>('file_get_filter'),

  /** 保存过滤器配置 */
  saveFilter: (filter: FileTypeFilter) => invoke<void>('file_save_filter', { filter }),

  /** 获取通道阈值（字节） */
  getThreshold: () => invoke<number>('file_get_threshold'),

  /** 设置通道阈值（字节） */
  setThreshold: (bytes: number) => invoke<void>('file_set_threshold', { bytes }),

  /** 获取全部文件传输设置 */
  getSettings: () => invoke<FileTransferSettings>('file_get_settings'),

  /** 设置默认保存目录 */
  setSaveDir: (path: string) => invoke<void>('file_set_save_dir', { path }),

  /** 读取文件元信息（待发送列表展示） */
  getFileInfo: (filePath: string) => invoke<FileInfo>('file_get_file_info', { filePath }),

  // ========== 事件监听 ==========
  /** 新文件接收请求 */
  onFileOffer: (cb: (offer: FileOffer) => void): Promise<UnlistenFn> =>
    listen<FileOffer>('file-transfer-offer', (e) => cb(e.payload)),

  /** 传输进度/状态更新 */
  onTransferUpdate: (cb: (task: TransferTask) => void): Promise<UnlistenFn> =>
    listen<TransferTask>('file-transfer-update', (e) => cb(e.payload)),

  /** 文本消息接收 */
  onTextMessage: (cb: (msg: TextMessage) => void): Promise<UnlistenFn> =>
    listen<TextMessage>('file-text-message', (e) => cb(e.payload)),
};
