// 打开文件 / 定位所在目录（Rust 端 open_path / reveal_in_explorer，走系统 explorer）

import { invoke } from '@tauri-apps/api/core';

/** 提取文件所在目录（兼容 / 与 \） */
export function fileDir(filePath: string): string {
  const i = filePath.lastIndexOf('\\');
  const j = filePath.lastIndexOf('/');
  const k = Math.max(i, j);
  return k >= 0 ? filePath.slice(0, k) : filePath;
}

/** 用系统默认程序打开文件；成功返回 true */
export async function openFile(path?: string | null): Promise<boolean> {
  if (!path) return false;
  try {
    await invoke('open_path', { path });
    return true;
  } catch {
    return false;
  }
}

/** 打开文件所在目录（并选中该文件）；成功返回 true */
export async function openContainingDir(path?: string | null): Promise<boolean> {
  if (!path) return false;
  try {
    await invoke('reveal_in_explorer', { path });
    return true;
  } catch {
    return false;
  }
}
