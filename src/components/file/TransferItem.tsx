// 单个传输项：进度条 + 状态标签 + 操作

import { Button, Popconfirm, Progress, Space, Tag, Tooltip, Typography, message } from 'antd';
import { FolderOpen, PauseCircle, PlayCircle, Redo2, Trash2, X } from 'lucide-react';
import { useFileTransferStore } from '../../stores/useFileTransferStore';
import { fileTransferApi } from '../../lib/fileTransferApi';
import { openContainingDir } from '../../lib/fileOpen';
import {
  channelLabel,
  formatSize,
  formatSpeed,
  statusTag,
  type TransferTask,
} from '../../types/file_transfer';

const { Text } = Typography;

export function TransferItem({ task }: { task: TransferTask }) {
  const cancelTransfer = useFileTransferStore((s) => s.cancelTransfer);
  const removeTask = useFileTransferStore((s) => s.removeTask);
  const percent =
    task.file_size > 0 ? Math.min(100, Math.round((task.bytes_done / task.file_size) * 100)) : 0;
  const st = statusTag(task.status);

  // 进行中/等待确认 → 可取消；终态 → 可重发（发送方向）/移除；暂停 → 可继续
  const actionable = task.status === 'Pending' || task.status === 'Transferring';
  const isPaused = task.status === 'Paused';

  const channelColor = task.channel === 'Quic' ? 'blue' : 'orange';

  /** 重发/继续（仅发送方向终态）：重新将源文件发给原目标，接收端将断点续传 */
  const handleResend = async () => {
    if (!task.file_path) {
      message.warning('源文件路径缺失，无法重发');
      return;
    }
    if (!task.remote_ip) {
      message.warning('原目标设备缺失，无法重发');
      return;
    }
    try {
      useFileTransferStore.getState().setActiveTab('active'); // 重发 → 跳"传输中"
      await fileTransferApi.sendFile(task.file_path, task.remote_ip);
      message.success('已重新加入发送队列（接收端将断点续传）');
      await useFileTransferStore.getState().refreshTransfers();
    } catch (e) {
      message.error(`重发失败: ${String(e)}`);
    }
  };

  /** 暂停：中断传输，接收端保留 .partial 断点，之后点「继续」可续传 */
  const handlePause = async () => {
    try {
      await useFileTransferStore.getState().pauseTransfer(task.transfer_id);
      message.info('已暂停，接收端保留断点；点「继续」可断点续传');
    } catch (e) {
      message.error(`暂停失败: ${String(e)}`);
    }
  };

  /** 继续（暂停任务）：移除暂停任务并重新发送，接收端断点续传 */
  const handleContinue = async () => {
    if (!task.file_path || !task.remote_ip) {
      message.warning('源文件或目标缺失，无法继续');
      return;
    }
    try {
      useFileTransferStore.getState().setActiveTab('active'); // 继续 → 跳"传输中"
      await useFileTransferStore.getState().removeTask(task.transfer_id);
      await fileTransferApi.sendFile(task.file_path, task.remote_ip);
      message.success('已继续传输（断点续传）');
      await useFileTransferStore.getState().refreshTransfers();
    } catch (e) {
      message.error(`继续失败: ${String(e)}`);
    }
  };

  return (
    <Space direction="vertical" size={4} style={{ width: '100%' }}>
      <Space wrap>
        <Text strong>{task.filename}</Text>
        <Tag color={st.color}>{st.text}</Tag>
        <Tag color={channelColor}>{channelLabel(task.channel)}</Tag>
        <Tag color={task.direction === 'Send' ? 'blue' : 'green'}>
          {task.direction === 'Send' ? '↑ 发送' : '↓ 接收'}
        </Tag>
        {task.quick_sent && <Tag color="cyan">⚡ 秒传</Tag>}
        {task.remote_device && (
          <Text type="secondary" style={{ fontSize: 12 }}>
            {task.remote_device} ({task.remote_ip})
          </Text>
        )}
        {actionable ? (
          <>
            {task.direction === 'Send' && (
              <Tooltip title="暂停（中断后点「重发」可断点续传）">
                <Button
                  size="small"
                  type="text"
                  icon={<PauseCircle size={14} />}
                  onClick={() => void handlePause()}
                />
              </Tooltip>
            )}
            <Tooltip title="终止传输">
              <Button
                size="small"
                danger
                type="text"
                icon={<X size={14} />}
                onClick={() => void cancelTransfer(task.transfer_id)}
              />
            </Tooltip>
          </>
        ) : isPaused ? (
          <Tooltip title="继续（断点续传）">
            <Button
              size="small"
              type="primary"
              ghost
              icon={<PlayCircle size={14} />}
              onClick={() => void handleContinue()}
            />
          </Tooltip>
        ) : (
          <>
            <Tooltip title="打开所在目录">
              <Button
                size="small"
                type="text"
                icon={<FolderOpen size={14} />}
                onClick={() => {
                  void openContainingDir(task.save_path ?? task.file_path).then((ok) => {
                    if (!ok) message.warning('无法打开所在目录（文件或目录可能已被移动/删除）');
                  });
                }}
              />
            </Tooltip>
            {task.direction === 'Send' && task.file_path && (
              <Tooltip title="重发（断点续传）">
                <Button
                  size="small"
                  type="text"
                  icon={<Redo2 size={14} />}
                  onClick={() => void handleResend()}
                />
              </Tooltip>
            )}
            <Popconfirm
              title="从列表移除该项？（不会删除文件）"
              onConfirm={() => void removeTask(task.transfer_id)}
            >
              <Tooltip title="移除记录">
                <Button size="small" type="text" icon={<Trash2 size={14} />} />
              </Tooltip>
            </Popconfirm>
          </>
        )}
      </Space>
      <Space size="middle">
        <Text type="secondary" style={{ fontSize: 12 }}>
          {formatSize(task.bytes_done)} / {formatSize(task.file_size)}
        </Text>
        {task.speed_kbps != null && task.speed_kbps > 0 && (
          <Text type="secondary" style={{ fontSize: 12 }}>
            {formatSpeed(task.speed_kbps)}
          </Text>
        )}
        {task.eta_seconds != null && task.eta_seconds > 0 && (
          <Text type="secondary" style={{ fontSize: 12 }}>
            剩余 {Math.ceil(task.eta_seconds)}s
          </Text>
        )}
        {task.error_message && (
          <Text type="danger" style={{ fontSize: 12 }}>
            {task.error_message}
          </Text>
        )}
      </Space>
      <Progress
        percent={percent}
        size="small"
        status={
          task.status === 'Failed'
            ? 'exception'
            : task.status === 'Completed'
              ? 'success'
              : 'active'
        }
      />
    </Space>
  );
}
