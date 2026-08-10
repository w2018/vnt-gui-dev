// FTP 服务类型定义（与后端 ftp/config.rs 对应）

/** 用户权限（F6） */
export interface FtpPermissions {
  upload: boolean;
  download: boolean;
  delete: boolean;
  readonly: boolean;
}

/** FTP 用户（password 仅编辑时填写，后端不回传） */
export interface FtpUser {
  username: string;
  /** 前端临时持有，保存时若为空 = 不改密码（后端保留 keyring 旧密码） */
  password: string;
  /** 🆕 后端返回的 keyring 状态：该用户是否已设置密码 */
  password_set?: boolean;
  permissions: FtpPermissions;
}

/** FTP 全局配置 */
export interface FtpConfig {
  enabled: boolean;
  auto_start_with_app: boolean;
  auto_start_with_system: boolean;
  root_dir: string;
  port: number;
  pasv_ports: [number, number] | null;
  users: FtpUser[];
}

/** 服务状态（F8） */
export interface FtpServerStatus {
  state: 'stopped' | 'running' | 'error';
  listen_addr: string | null;
  error: string | null;
}

/** 连接日志条目（F9） */
export interface FtpLogEntry {
  time: string;
  ip: string;
  user: string;
  action: string;
  detail: string;
}

export const defaultFtpConfig = (): FtpConfig => ({
  enabled: false,
  auto_start_with_app: false,
  auto_start_with_system: false,
  root_dir: '',
  port: 2121,
  pasv_ports: null,
  users: [],
});

export const defaultFtpPermissions = (): FtpPermissions => ({
  upload: true,
  download: true,
  delete: false,
  readonly: false,
});
