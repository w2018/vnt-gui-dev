// 共享 TS 类型（与 Rust 后端 serde 序列化对应）

/** 连接状态（5 态，Rust 侧 serde tag="status"） */
export type ConnectionStatus =
  | { status: 'stopped' }
  | { status: 'starting' }
  | { status: 'connected' }
  | { status: 'reconnecting'; attempt: number }
  | { status: 'error'; message: string };

export type LogLevel = 'info' | 'warn' | 'error' | 'debug';

export interface LogEntry {
  timestamp: string;
  level: LogLevel;
  message: string;
}

/** VNT 连接配置（对应 vnt-cli 参数） */
export interface VntConfig {
  id: string;
  name: string;
  token: string;
  device_name?: string;
  device_id?: string;
  virtual_ip?: string;
  server_address?: string;
  password?: string;
  server_encrypt: boolean;
  in_ips: string[];
  out_ips: string[];
  compressor?: string;
  mtu?: number;
  use_tcp: boolean;
  use_ws: boolean;
  no_proxy: boolean;
  created_at: string;
  updated_at: string;
  last_used?: string;
}

export interface ConfigStore {
  active_config_id: string | null;
  configs: VntConfig[];
}

export interface TrafficSnapshot {
  upload_bytes: number;
  download_bytes: number;
  upload_speed: number;
  download_speed: number;
  peers: PeerTraffic[];
}

export interface PeerTraffic {
  ip: string;
  upload_bytes: number;
  download_bytes: number;
}

/** 设备列表条目 */
export interface PeerInfo {
  name: string;
  virtual_ip: string;
  connection_type: 'p2p' | 'relay' | 'client-relay';
  latency: number;
  status: 'online' | 'offline';
}

/** 应用行为设置（托盘可见性等） */
export interface AppSettings {
  hide_tray_on_autostart: boolean;
  hide_tray_on_background: boolean;
}

export interface UpdateInfo {
  has_update: boolean;
  latest_version: string;
  current_version: string;
  download_url: string | null;
  app_version: string;
  app_has_update: boolean;
  app_latest_version: string | null;
}

/** 新建配置默认值 */
export function defaultConfig(): VntConfig {
  return {
    id: '',
    name: '',
    token: '',
    device_name: undefined,
    device_id: undefined,
    virtual_ip: undefined,
    server_address: undefined,
    password: undefined,
    server_encrypt: false,
    in_ips: [],
    out_ips: [],
    compressor: undefined,
    mtu: undefined,
    use_tcp: false,
    use_ws: false,
    no_proxy: false,
    created_at: '',
    updated_at: '',
    last_used: undefined,
  };
}
