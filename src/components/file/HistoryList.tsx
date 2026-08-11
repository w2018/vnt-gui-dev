// 历史记录列表：持久化收发记录，支持单选/批量删除、双击打开文件、定位目录、耗时展示

import { useState } from 'react';
import { Button, Checkbox, Empty, List, Popconfirm, Space, Tag, Tooltip, Typography, message } from 'antd';
import { DeleteOutlined, FolderOpenOutlined, RedoOutlined } from '@ant-design/icons';
import { useFileTransferStore } from '../../stores/useFileTransferStore';
import { fileTransferApi } from '../../lib/fileTransferApi';
import { openContainingDir, openFile } from '../../lib/fileOpen';
import { channelLabel, formatSize, formatSpeed, statusTag, type TransferRecord } from '../../types/file_transfer';

const { Text } = Typography;

function formatTime(ts: number): string {
  if (!ts) return '-';
  return new Date(ts * 1000).toLocaleString('zh-CN');
}

/** 传输耗时（秒 → 人可读） */
function formatDuration(start: number, end?: number | null): string {
  if (!start || !end || end < start) return '-';
  const secs = Math.max(0, end - start);
  if (secs < 60) return `${secs}s`;
  const m = Math.floor(secs / 60);
  const s = secs % 60;
  return `${m}m${s}s`;
}

export function HistoryList() {
  const { history, deleteHistory, deleteHistoryBatch } = useFileTransferStore();
  const [selected, setSelected] = useState<Set<number>>(new Set());

  const toggleSelect = (id: number) => {
    const next = new Set(selected);
    if (next.has(id)) next.delete(id);
    else next.add(id);
    setSelected(next);
  };

  const handleBatchDelete = async () => {
    if (selected.size === 0) return;
    await deleteHistoryBatch(Array.from(selected));
    setSelected(new Set());
  };

  /** 重发（仅发送方向）：重新将源文件发给原目标 */
  const handleResend = async (record: TransferRecord) => {
    if (!record.file_path || !record.remote_ip) {
      message.warning('源文件或目标设备缺失，无法重发');
      return;
    }
    try {
      useFileTransferStore.getState().setActiveTab('active'); // 重发 → 跳"传输中"
      await fileTransferApi.sendFile(record.file_path, record.remote_ip);
      message.success('已重新加入发送队列');
      await useFileTransferStore.getState().refreshTransfers();
    } catch (e) {
      message.error(`重发失败: ${String(e)}`);
    }
  };

  if (history.length === 0) {
    return (
      <div style={{ padding: 40, textAlign: 'center' }}>
        <Empty description={<Text type="secondary">暂无传输记录</Text>} />
      </div>
    );
  }

  return (
    <div>
      <Space style={{ marginBottom: 12 }}>
        <Checkbox
          indeterminate={selected.size > 0 && selected.size < history.length}
          checked={selected.size === history.length}
          onChange={(e) => {
            if (e.target.checked) {
              setSelected(new Set(history.map((h) => h.id)));
            } else {
              setSelected(new Set());
            }
          }}
        >
          全选
        </Checkbox>
        <Button
          size="small"
          onClick={() => {
            const next = new Set(
              history
                .filter((h) => !selected.has(h.id))
                .map((h) => h.id),
            );
            setSelected(next);
          }}
        >
          反选
        </Button>
        <Popconfirm
          title={`确定删除选中的 ${selected.size} 条记录？（不会删除文件）`}
          onConfirm={() => void handleBatchDelete()}
          disabled={selected.size === 0}
        >
          <Button
            danger
            size="small"
            icon={<DeleteOutlined />}
            disabled={selected.size === 0}
          >
            批量删除 ({selected.size})
          </Button>
        </Popconfirm>
      </Space>

      <List
        dataSource={history}
        rowKey={(r) => r.id}
        renderItem={(record) => {
          const isSelected = selected.has(record.id);
          const st = statusTag(record.status);
          return (
            <List.Item
              onDoubleClick={() => {
                void openFile(record.file_path).then((ok) => {
                  if (!ok) message.warning('无法打开文件（可能已被移动）');
                });
              }}
              actions={[
                record.direction === 'Send' && record.file_path ? (
                  <Tooltip key="resend" title="重发（断点续传）">
                    <Button
                      size="small"
                      type="text"
                      icon={<RedoOutlined />}
                      onClick={() => void handleResend(record)}
                    />
                  </Tooltip>
                ) : null,
                <Tooltip key="dir" title="打开保存位置">
                  <Button
                    size="small"
                    type="text"
                    icon={<FolderOpenOutlined />}
                    onClick={() => {
                      void openContainingDir(record.file_path).then((ok) => {
                        if (!ok) message.warning('无法打开保存位置');
                      });
                    }}
                  />
                </Tooltip>,
                <Popconfirm
                  key="del"
                  title="删除此记录？（不会删除文件）"
                  onConfirm={() => void deleteHistory(record.id)}
                >
                  <Button size="small" danger type="text" icon={<DeleteOutlined />} />
                </Popconfirm>,
              ]}
            >
              <List.Item.Meta
                title={
                  <Space wrap>
                    <Checkbox
                      checked={isSelected}
                      onChange={() => toggleSelect(record.id)}
                    />
                    <Text strong>{record.filename}</Text>
                    <Tag color={record.direction === 'Send' ? 'blue' : 'green'}>
                      {record.direction === 'Send' ? '↑ 发送' : '↓ 接收'}
                    </Tag>
                    <Tag color={st.color}>{st.text}</Tag>
                    {record.quick_sent && <Tag color="cyan">⚡ 秒传</Tag>}
                    <Tag color={record.channel === 'Quic' ? 'blue' : 'orange'}>
                      {channelLabel(record.channel)}
                    </Tag>
                  </Space>
                }
                description={
                  <Space size="middle" wrap>
                    <Text type="secondary" style={{ fontSize: 12 }}>
                      {formatSize(record.file_size)}
                    </Text>
                    <Text type="secondary" style={{ fontSize: 12 }}>
                      {record.remote_device} ({record.remote_ip})
                    </Text>
                    <Text type="secondary" style={{ fontSize: 12 }}>
                      {formatTime(record.start_time)}
                    </Text>
                    <Text type="secondary" style={{ fontSize: 12 }}>
                      耗时 {formatDuration(record.start_time, record.end_time)}
                    </Text>
                    {record.avg_speed_kbps != null && record.avg_speed_kbps > 0 && (
                      <Text type="secondary" style={{ fontSize: 12 }}>
                        平均速度 {formatSpeed(record.avg_speed_kbps)}
                      </Text>
                    )}
                    {record.error_message && (
                      <Text type="danger" style={{ fontSize: 12 }}>
                        {record.error_message}
                      </Text>
                    )}
                  </Space>
                }
              />
            </List.Item>
          );
        }}
      />
    </div>
  );
}
