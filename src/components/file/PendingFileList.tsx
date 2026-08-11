// 待发送文件列表：拖拽/选择后加入，手动发送单个或全部

import { useState, type Key } from 'react';
import { Button, Empty, Popconfirm, Space, Table, Tag, Tooltip, Typography, message } from 'antd';
import { DeleteOutlined, FolderOpenOutlined, SendOutlined } from '@ant-design/icons';
import type { ColumnsType } from 'antd/es/table';
import { useFileTransferStore } from '../../stores/useFileTransferStore';
import { formatSize } from '../../types/file_transfer';
import { openContainingDir, openFile } from '../../lib/fileOpen';
import type { PendingFile } from '../../types/file_transfer';

const { Text } = Typography;

function formatTime(ms: number): string {
  if (!ms) return '-';
  return new Date(ms).toLocaleString('zh-CN');
}

export function PendingFileList() {
  const {
    pendingFiles,
    removePendingFile,
    clearPending,
    sendAllPending,
    sendOnePending,
    targetIp,
  } = useFileTransferStore();
  const [sending, setSending] = useState(false);
  const [selectedRowKeys, setSelectedRowKeys] = useState<Key[]>([]);

  /** 批量移除选中项（仅移除列表，不删除磁盘文件） */
  const handleBatchRemove = () => {
    if (selectedRowKeys.length === 0) return;
    for (const p of pendingFiles) {
      if (selectedRowKeys.includes(p.path)) removePendingFile(p.path);
    }
    setSelectedRowKeys([]);
  };

  /** 反选：选中与未选中互换 */
  const invertSelection = () => {
    const next = pendingFiles
      .filter((p) => !selectedRowKeys.includes(p.path))
      .map((p) => p.path);
    setSelectedRowKeys(next);
  };

  const handleSendAll = async () => {
    if (!targetIp) {
      message.warning('请先选择目标设备');
      return;
    }
    if (pendingFiles.length === 0) return;
    setSending(true);
    const total = pendingFiles.length;
    try {
      const failed = await sendAllPending();
      if (failed > 0) {
        message.warning(`${total - failed} 个已加入队列，${failed} 个失败`);
      } else {
        message.success(`已加入发送队列（${total} 个文件）`);
      }
    } catch (e) {
      message.error(`发送失败: ${String(e)}`);
    } finally {
      setSending(false);
    }
  };

  const handleSendOne = async (path: string) => {
    setSending(true);
    try {
      await sendOnePending(path);
      message.success('已加入发送队列');
    } catch (e) {
      message.error(`发送失败: ${String(e)}`);
    } finally {
      setSending(false);
    }
  };

  const handleDoubleClick = async (path: string) => {
    const ok = await openFile(path);
    if (!ok) message.warning('无法打开文件（可能已被移动）');
  };

  const columns: ColumnsType<PendingFile> = [
    {
      title: '文件名',
      dataIndex: 'name',
      key: 'name',
      ellipsis: true,
      render: (name: string) => <Text strong>{name}</Text>,
    },
    {
      title: '大小',
      dataIndex: 'size',
      key: 'size',
      width: 110,
      render: (s: number) => <Text style={{ fontSize: 12 }}>{formatSize(s)}</Text>,
    },
    {
      title: '类型',
      dataIndex: 'file_type',
      key: 'file_type',
      width: 100,
      render: (t: string) => (t ? <Tag>{t}</Tag> : <Tag>未知</Tag>),
    },
    {
      title: '保存位置',
      dataIndex: 'path',
      key: 'path',
      ellipsis: true,
      render: (p: string) => (
        <Tooltip title={p}>
          <Text style={{ fontSize: 12 }}>{p}</Text>
        </Tooltip>
      ),
    },
    {
      title: '修改时间',
      dataIndex: 'modified',
      key: 'modified',
      width: 170,
      render: (m: number) => <Text style={{ fontSize: 12 }}>{formatTime(m)}</Text>,
    },
    {
      title: '操作',
      key: 'action',
      width: 130,
      render: (_, record) => (
        <Space size={4}>
          <Tooltip title="发送">
            <Button
              size="small"
              type="primary"
              ghost
              icon={<SendOutlined />}
              loading={sending}
              onClick={() => void handleSendOne(record.path)}
            />
          </Tooltip>
          <Tooltip title="打开所在目录">
            <Button
              size="small"
              icon={<FolderOpenOutlined />}
              onClick={() => {
                void openContainingDir(record.path).then((ok) => {
                  if (!ok) message.warning('无法打开所在目录');
                });
              }}
            />
          </Tooltip>
          <Tooltip title="移除">
            <Button
              size="small"
              danger
              type="text"
              icon={<DeleteOutlined />}
              onClick={() => removePendingFile(record.path)}
            />
          </Tooltip>
        </Space>
      ),
    },
  ];

  if (pendingFiles.length === 0) {
    return (
      <div style={{ padding: 20, textAlign: 'center' }}>
        <Empty description={<Text type="secondary">暂无待发送文件（拖拽文件加入）</Text>} />
      </div>
    );
  }

  return (
    <div>
      <Space style={{ marginBottom: 12 }} wrap>
        <Button type="primary" icon={<SendOutlined />} loading={sending} onClick={() => void handleSendAll()}>
          发送全部 ({pendingFiles.length})
        </Button>
        <Popconfirm
          title={`移除选中的 ${selectedRowKeys.length} 个文件？（仅移除列表，不删除磁盘文件）`}
          onConfirm={handleBatchRemove}
          disabled={selectedRowKeys.length === 0}
        >
          <Button danger disabled={selectedRowKeys.length === 0}>
            批量移除 ({selectedRowKeys.length})
          </Button>
        </Popconfirm>
        <Button onClick={invertSelection} disabled={pendingFiles.length === 0}>
          反选
        </Button>
        <Button onClick={() => void clearPending()}>清空列表</Button>
        {!targetIp && <Text type="warning">请先选择目标设备</Text>}
        <Text type="secondary" style={{ fontSize: 12 }}>
          双击文件名可预览文件
        </Text>
      </Space>
      <Table
        rowKey="path"
        columns={columns}
        dataSource={pendingFiles}
        size="small"
        pagination={false}
        rowSelection={{
          selectedRowKeys,
          onChange: (keys) => setSelectedRowKeys(keys),
        }}
        onRow={(record) => ({
          onDoubleClick: () => void handleDoubleClick(record.path),
        })}
      />
    </div>
  );
}
