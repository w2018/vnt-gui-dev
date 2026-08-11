// 拖拽/选择文件 → 加入待发送列表（不自动发送，需手动发送）

import { useEffect, useState } from 'react';
import { Button, Typography, message } from 'antd';
import { FileUp, Inbox } from 'lucide-react';
import { open } from '@tauri-apps/plugin-dialog';
import { getCurrentWebview } from '@tauri-apps/api/webview';
import { useFileTransferStore } from '../../stores/useFileTransferStore';
import { fileTransferApi } from '../../lib/fileTransferApi';
import type { PendingFile } from '../../types/file_transfer';

const { Text } = Typography;

export function FileDropZone() {
  const addPendingFiles = useFileTransferStore((s) => s.addPendingFiles);
  const pendingCount = useFileTransferStore((s) => s.pendingFiles.length);
  const [dragOver, setDragOver] = useState(false);
  const [adding, setAdding] = useState(false);

  const handleNewFiles = (files: PendingFile[]) => {
    if (files.length === 0) return;
    addPendingFiles(files);
    message.success(`已加入待发送列表（${files.length} 个文件）`);
  };

  /** 拖拽/选择得到的路径列表 → 读取元数据加入待发送列表 */
  const addPaths = async (paths: string[]) => {
    if (paths.length === 0) return;
    setAdding(true);
    try {
      const pending: PendingFile[] = [];
      for (const p of paths) {
        try {
          pending.push(await fileTransferApi.getFileInfo(p));
        } catch (e) {
          message.error(`读取文件信息失败: ${p}（${String(e)}）`);
        }
      }
      handleNewFiles(pending);
    } finally {
      setAdding(false);
    }
  };

  // Tauri 原生拖拽事件（HTML5 onDrop 拿不到文件真实路径，必须用此事件）
  useEffect(() => {
    let unlisten: (() => void) | undefined;
    void getCurrentWebview()
      .onDragDropEvent((event) => {
        if (event.payload.type === 'over') {
          setDragOver(true);
        } else if (event.payload.type === 'drop') {
          setDragOver(false);
          void addPaths(event.payload.paths);
        } else {
          // 'leave' 或 'enter' 之外的状态 → 复位高亮
          setDragOver(false);
        }
      })
      .then((fn) => {
        unlisten = fn;
      })
      .catch((e) => {
        console.error('注册文件拖拽事件失败:', e);
      });
    return () => {
      unlisten?.();
    };
  }, []);

  const pickFiles = async () => {
    const picked = await open({
      multiple: true,
      title: '选择要发送的文件',
    });
    if (!picked) return;
    const paths = (Array.isArray(picked) ? picked : [picked]) as string[];
    await addPaths(paths);
  };

  return (
    <div
      style={{
        border: `2px dashed ${dragOver ? '#1677ff' : '#d9d9d9'}`,
        borderRadius: 8,
        padding: '24px 16px',
        textAlign: 'center',
        marginBottom: 12,
        background: dragOver ? '#e6f4ff' : 'transparent',
        transition: 'all 0.2s',
        cursor: 'pointer',
      }}
      onClick={pickFiles}
    >
      <Inbox size={34} color={dragOver ? '#1677ff' : '#999'} />
      <div style={{ marginTop: 8 }}>
        <Text strong>拖拽文件到此处加入发送列表，或点击选择文件</Text>
      </div>
      <div style={{ marginTop: 4 }}>
        <Text type="secondary" style={{ fontSize: 12 }}>
          支持单/多文件 · 加入后需手动发送 · 自动选择通道（≥100MB 走 TCP 高速通道）
          {pendingCount > 0 && ` · 当前待发送 ${pendingCount} 个`}
        </Text>
      </div>
      <Button
        size="small"
        icon={<FileUp size={14} />}
        loading={adding}
        style={{ marginTop: 10 }}
        onClick={(e) => {
          e.stopPropagation();
          void pickFiles();
        }}
      >
        选择文件
      </Button>
    </div>
  );
}
