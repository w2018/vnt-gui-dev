// 传输队列：任务列表（含进度与操作，双击终态项打开文件）
// 全部为终态时（已完成/终止 Tab）提供全选 / 反选 / 批量移除

import { useEffect, useState } from 'react';
import { Button, Checkbox, Empty, List, Popconfirm, Space, Typography, message } from 'antd';
import { TransferItem } from './TransferItem';
import { openFile } from '../../lib/fileOpen';
import { useFileTransferStore } from '../../stores/useFileTransferStore';
import type { TransferTask } from '../../types/file_transfer';

const { Text } = Typography;

interface Props {
  transfers: TransferTask[];
}

/** 是否为终态（已完成/失败/已取消/已拒绝） */
function isFinal(t: TransferTask): boolean {
  return t.status === 'Completed' || t.status === 'Failed' || t.status === 'Cancelled' || t.status === 'Rejected';
}

export function TransferQueue({ transfers }: Props) {
  const removeTaskBatch = useFileTransferStore((s) => s.removeTaskBatch);
  const [selected, setSelected] = useState<Set<number>>(new Set());

  const allFinal = transfers.length > 0 && transfers.every(isFinal);

  // transfers 变化时清理已不存在任务的选中项
  useEffect(() => {
    setSelected((prev) => {
      const ids = new Set(transfers.map((t) => t.transfer_id));
      const next = new Set([...prev].filter((id) => ids.has(id)));
      return next.size === prev.size ? prev : next;
    });
  }, [transfers]);

  const toggleSelect = (id: number) => {
    setSelected((prev) => {
      const next = new Set(prev);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      return next;
    });
  };

  const handleBatchRemove = async () => {
    if (selected.size === 0) return;
    try {
      await removeTaskBatch(Array.from(selected));
      message.success(`已移除 ${selected.size} 条记录`);
      setSelected(new Set());
    } catch (e) {
      message.error(`移除失败: ${String(e)}`);
    }
  };

  if (transfers.length === 0) {
    return (
      <div style={{ padding: 40, textAlign: 'center' }}>
        <Empty description={<Text type="secondary">暂无传输任务</Text>} />
      </div>
    );
  }

  return (
    <div>
      {allFinal && (
        <Space style={{ marginBottom: 12 }} wrap>
          <Checkbox
            indeterminate={selected.size > 0 && selected.size < transfers.length}
            checked={selected.size === transfers.length}
            onChange={(e) => {
              if (e.target.checked) {
                setSelected(new Set(transfers.map((t) => t.transfer_id)));
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
                transfers
                  .filter((t) => !selected.has(t.transfer_id))
                  .map((t) => t.transfer_id),
              );
              setSelected(next);
            }}
          >
            反选
          </Button>
          <Popconfirm
            title={`移除选中的 ${selected.size} 条记录？（不会删除文件）`}
            onConfirm={() => void handleBatchRemove()}
            disabled={selected.size === 0}
          >
            <Button danger size="small" disabled={selected.size === 0}>
              批量移除 ({selected.size})
            </Button>
          </Popconfirm>
        </Space>
      )}

      <List
        dataSource={transfers}
        rowKey={(t) => t.transfer_id}
        renderItem={(task) => (
          <List.Item
            style={{ padding: '12px 0' }}
            onDoubleClick={
              isFinal(task)
                ? () => {
                    void openFile(task.save_path ?? task.file_path).then((ok) => {
                      if (!ok) message.warning('无法打开文件（可能已被移动）');
                    });
                  }
                : undefined
            }
          >
            <div style={{ display: 'flex', alignItems: 'flex-start', width: '100%' }}>
              {allFinal && (
                <Checkbox
                  checked={selected.has(task.transfer_id)}
                  onChange={() => toggleSelect(task.transfer_id)}
                  style={{ marginRight: 12, marginTop: 4 }}
                />
              )}
              <div style={{ flex: 1, minWidth: 0 }}>
                <TransferItem task={task} />
              </div>
            </div>
          </List.Item>
        )}
      />
    </div>
  );
}
