// 文件传输类型定义（与 Rust 端 serde 序列化格式一致）

/** 传输通道（Rust enum TransferChannel 的 JSON 形态） */
export type TransferChannel = 'Quic' | { Tcp: { port: number } };

/** 传输方向（Rust enum TransferDirection） */
export type TransferDirection = 'Send' | 'Receive';

/** 传输状态（Rust enum TransferStatus） */
export type TransferStatus =
  | 'Pending'
  | 'Transferring'
  | 'Paused'
  | 'Completed'
  | 'Failed'
  | 'Cancelled'
  | 'Rejected';

/** 传输任务（transfer_manager.rs TransferTask） */
export interface TransferTask {
  transfer_id: number;
  filename: string;
  file_size: number;
  bytes_done: number;
  direction: TransferDirection;
  channel: TransferChannel;
  status: TransferStatus;
  remote_ip: string;
  remote_device: string;
  error_message?: string | null;
  speed_kbps?: number | null;
  eta_seconds?: number | null;
  created_at: number;
  file_path?: string | null;
  save_path?: string | null;
  resume_offset: number;
  /** 是否为秒传（接收端已有相同 md5 文件，跳过实际传输） */
  quick_sent: boolean;
}

/** 历史记录（history.rs TransferRecord） */
export interface TransferRecord {
  id: number;
  transfer_id: number;
  direction: TransferDirection;
  filename: string;
  file_size: number;
  remote_ip: string;
  remote_device: string;
  channel: TransferChannel;
  status: TransferStatus;
  start_time: number;
  end_time?: number | null;
  bytes_transferred: number;
  file_hash?: string | null;
  error_message?: string | null;
  /** 文件完整路径（接收=保存路径；发送=源文件路径） */
  file_path?: string | null;
  /** 是否为秒传（接收端已有相同 md5 文件，跳过实际传输） */
  quick_sent: boolean;
  /** 平均传输速度（KB/s） */
  avg_speed_kbps?: number | null;
}

/** 文件接收请求弹窗载荷（daemon_listener 事件 file-transfer-offer） */
export interface FileOffer {
  transfer_id: number;
  filename: string;
  file_size: number;
  channel: TransferChannel;
  remote_ip: string;
  remote_device: string;
  resume_offset: number;
  default_save_path: string;
}

/** 文件类型过滤配置 */
export interface FileTypeFilter {
  mode: 'Whitelist' | 'Blacklist' | 'AllowAll' | 'DenyAll';
  extensions: string[];
}

/** 文本消息 */
export interface TextMessage {
  msg_id: number;
  timestamp: number;
  text: string;
  from: string;
}

/** 待发送文件（拖拽/选择后加入发送列表，手动发送） */
export interface PendingFile {
  /** 源文件完整路径 */
  path: string;
  /** 文件名 */
  name: string;
  /** 文件大小（字节） */
  size: number;
  /** 文件类型（扩展名，不含点，小写） */
  file_type: string;
  /** 修改时间（Unix 毫秒） */
  modified: number;
}

/** 文件元信息（后端 file_get_file_info 返回） */
export interface FileInfo {
  name: string;
  path: string;
  size: number;
  file_type: string;
  modified: number;
}

// ==================== 展示辅助 ====================

/** 通道标签文案 */
export function channelLabel(channel: TransferChannel): string {
  return channel === 'Quic' ? 'QUIC 流' : 'TCP 高速通道';
}

/** 方向文案 */
export function directionLabel(dir: TransferDirection): string {
  return dir === 'Send' ? '发送' : '接收';
}

/** 状态标签（antd Tag 颜色 + 文案） */
export function statusTag(status: TransferStatus): { color: string; text: string } {
  const map: Record<TransferStatus, { color: string; text: string }> = {
    Pending: { color: 'gold', text: '等待确认' },
    Transferring: { color: 'processing', text: '传输中' },
    Paused: { color: 'purple', text: '已暂停' },
    Completed: { color: 'success', text: '已完成' },
    Failed: { color: 'error', text: '失败' },
    Cancelled: { color: 'default', text: '已取消' },
    Rejected: { color: 'warning', text: '已拒绝' },
  };
  return map[status];
}

/** 文件大小格式化 */
export function formatSize(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  if (bytes < 1024 * 1024 * 1024) return `${(bytes / 1048576).toFixed(1)} MB`;
  return `${(bytes / 1073741824).toFixed(2)} GB`;
}

/** 速度格式化（KB/s → 自适应 KB/s / MB/s） */
export function formatSpeed(kbps: number): string {
  if (kbps < 1024) return `${Math.round(kbps)} KB/s`;
  return `${(kbps / 1024).toFixed(2)} MB/s`;
}
