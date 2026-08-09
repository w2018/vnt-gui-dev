// Tauri invoke 封装与事件监听（文档 §3.10 / §3.11 / §4.4）

import { invoke } from '@tauri-apps/api/core';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import { useConnectionStore } from '../stores/connectionStore';
import { useLogStore } from '../stores/logStore';
import { useTrafficStore } from '../stores/trafficStore';
import type {
  AppSettings,
  ConfigStore,
  ConnectionStatus,
  DeviceListResult,
  LocalInfo,
  LogEntry,
  PeriodTraffic,
  TrafficSnapshot,
  UpdateInfo,
  VntConfig,
} from './types';

/** 全部 tauri::command 的封装（Rust 参数自动 camelCase → snake_case） */
export const api = {
  startConnection: (configId: string) => invoke<void>('start_connection', { configId }),
  stopConnection: () => invoke<void>('stop_connection'),
  getStatus: () => invoke<ConnectionStatus>('get_status'),
  getConfigs: () => invoke<ConfigStore>('get_configs'),
  saveConfig: (config: VntConfig) => invoke<void>('save_config', { config }),
  deleteConfig: (id: string) => invoke<void>('delete_config', { id }),
  setActiveConfig: (id: string) => invoke<void>('set_active_config', { id }),
  exportConfigs: (path: string) => invoke<void>('export_configs', { path }),
  importConfigs: (path: string) => invoke<VntConfig[]>('import_configs', { path }),
  getSettings: () => invoke<AppSettings>('get_settings'),
  saveSettings: (settings: AppSettings) => invoke<void>('save_settings', { settings }),
  getLogs: () => invoke<LogEntry[]>('get_logs'),
  clearLogs: () => invoke<void>('clear_logs'),
  exportLogs: (path: string) => invoke<void>('export_logs', { path }),
  checkUpdate: () => invoke<UpdateInfo>('check_update'),
  downloadAndReplace: (url: string) => invoke<void>('download_and_replace', { url }),
  setAutostart: (enabled: boolean) => invoke<void>('set_autostart', { enabled }),
  isAutostartEnabled: () => invoke<boolean>('is_autostart_enabled'),
  getAppVersion: () => invoke<string>('get_app_version'),
  getVntVersion: () => invoke<string>('get_vnt_version'),
  getDeviceList: () => invoke<DeviceListResult>('get_device_list'),
  getTrafficStats: () => invoke<TrafficSnapshot>('get_traffic_stats'),
  pingHost: (host: string) => invoke<number>('ping_host', { host }),
  pingTest: (host: string) => invoke<string>('ping_test', { host }),
  getPingHost: () => invoke<string | null>('get_ping_host'),
  getLocalInfo: () => invoke<LocalInfo>('get_local_info'),
  getTrafficPeriod: () => invoke<PeriodTraffic>('get_traffic_period'),
};

/** 初始化 Rust → 前端事件监听，返回取消函数列表 */
export async function initTauriListeners(): Promise<UnlistenFn[]> {
  const unlisteners: UnlistenFn[] = [];

  // 连接状态
  unlisteners.push(
    await listen<ConnectionStatus>('status-change', (event) => {
      const { setStatus, setError, setVirtualIp } = useConnectionStore.getState();
      const payload = event.payload;
      setStatus(payload.status);
      if (payload.status === 'error') {
        setError(payload.message ?? '未知错误');
      }
      if (payload.status === 'connected') {
        setError(null);
      }
      void setVirtualIp; // 虚拟 IP 由 virtual-ip-assigned 事件单独设置
    }),
  );

  // 虚拟 IP 分配
  unlisteners.push(
    await listen<string>('virtual-ip-assigned', (event) => {
      useConnectionStore.getState().setVirtualIp(event.payload);
    }),
  );

  // 延迟更新（由 vnt-cli 输出解析，实时推送）
  unlisteners.push(
    await listen<number>('latency-update', (event) => {
      useConnectionStore.getState().setLatency(event.payload);
    }),
  );

  // 日志
  unlisteners.push(
    await listen<LogEntry>('log-line', (event) => {
      useLogStore.getState().appendLog(event.payload);
    }),
  );

  // 流量
  unlisteners.push(
    await listen<TrafficSnapshot>('traffic-update', (event) => {
      useTrafficStore.getState().updateTraffic(event.payload);
    }),
  );

  return unlisteners;
}
