// 日志查看器（文档 §4.2.4）：过滤 / 搜索 / 导出 / 清空 / 自动滚动

import { useEffect, useMemo, useRef, useState } from 'react';
import { Button, Card, Input, Popconfirm, Select, Space, Tag, message } from 'antd';
import { Download, Eraser } from 'lucide-react';
import { save } from '@tauri-apps/plugin-dialog';
import { api } from '../lib/tauri';
import { useLogStore } from '../stores/logStore';
import type { LogLevel } from '../lib/types';

const levelColors: Record<LogLevel, string> = {
  info: 'blue',
  warn: 'orange',
  error: 'red',
  debug: 'default',
};

const levelFilterOptions = [
  { value: 'all', label: '全部' },
  { value: 'error', label: '仅错误' },
  { value: 'warn', label: '警告+' },
  { value: 'info', label: '信息+' },
  { value: 'debug', label: '调试' },
];

function levelRank(l: LogLevel): number {
  return l === 'error' ? 4 : l === 'warn' ? 3 : l === 'info' ? 2 : 1;
}

export function LogViewer() {
  const { logs, filterLevel, searchTerm, setFilterLevel, setSearchTerm, setLogs } =
    useLogStore();
  const scrollRef = useRef<HTMLDivElement>(null);
  const [exporting, setExporting] = useState(false);
  const [clearing, setClearing] = useState(false);

  // 过滤 + 搜索
  const filtered = useMemo(() => {
    const minRank = filterLevel === 'all' ? 0 : levelRank(filterLevel);
    return logs.filter((log) => {
      if (minRank > 0 && levelRank(log.level) < minRank) return false;
      if (searchTerm && !log.message.includes(searchTerm)) return false;
      return true;
    });
  }, [logs, filterLevel, searchTerm]);

  // 自动滚到底部
  useEffect(() => {
    const el = scrollRef.current;
    if (el) el.scrollTop = el.scrollHeight;
  }, [filtered.length]);

  const handleExport = async () => {
    try {
      const path = await save({
        title: '导出日志',
        defaultPath: `vnt-gui-logs-${new Date().toISOString().slice(0, 10)}.txt`,
        filters: [{ name: '文本文件', extensions: ['txt', 'log'] }],
      });
      if (!path) return;
      setExporting(true);
      await api.exportLogs(path);
      message.success('日志已导出');
    } catch (e) {
      message.error(`导出失败: ${String(e)}`);
    } finally {
      setExporting(false);
    }
  };

  const handleClear = async () => {
    try {
      setClearing(true);
      await api.clearLogs();
      setLogs([]);
      message.success('日志已清空');
    } catch (e) {
      message.error(`清空失败: ${String(e)}`);
    } finally {
      setClearing(false);
    }
  };

  return (
    <Card
      title="实时日志"
      extra={
        <Space>
          <Select
            size="small"
            value={filterLevel}
            onChange={setFilterLevel}
            options={levelFilterOptions}
            style={{ width: 100 }}
          />
          <Input.Search
            size="small"
            placeholder="搜索日志..."
            allowClear
            value={searchTerm}
            onChange={(e) => setSearchTerm(e.target.value)}
            style={{ width: 200 }}
          />
          <Button size="small" icon={<Download size={13} />} loading={exporting} onClick={handleExport}>
            导出
          </Button>
          <Popconfirm title="确定清空全部日志？" onConfirm={handleClear}>
            <Button size="small" danger icon={<Eraser size={13} />} loading={clearing}>
              清空
            </Button>
          </Popconfirm>
        </Space>
      }
    >
      <div
        ref={scrollRef}
        className="log-viewer"
        style={{
          height: 'calc(100vh - 220px)',
          overflow: 'auto',
          background: '#0f1115',
          color: '#d4d4d4',
          borderRadius: 8,
          padding: '8px 12px',
        }}
      >
        {filtered.length === 0 ? (
          <div style={{ color: '#666', padding: 12 }}>暂无日志</div>
        ) : (
          filtered.map((log, i) => (
            <div key={`${log.timestamp}-${i}`} style={{ whiteSpace: 'pre-wrap', wordBreak: 'break-all' }}>
              <Tag color={levelColors[log.level]} style={{ marginRight: 6 }}>
                {log.level.toUpperCase()}
              </Tag>
              <span style={{ color: '#888', marginRight: 8 }}>
                {formatTime(log.timestamp)}
              </span>
              <span
                style={{
                  color:
                    log.level === 'error'
                      ? '#ff7875'
                      : log.level === 'warn'
                        ? '#ffc53d'
                        : '#d4d4d4',
                }}
              >
                {log.message}
              </span>
            </div>
          ))
        )}
      </div>
    </Card>
  );
}

function formatTime(ts: string): string {
  try {
    const d = new Date(ts);
    return d.toLocaleTimeString('zh-CN', { hour12: false }) + '.' + String(d.getMilliseconds()).padStart(3, '0');
  } catch {
    return ts;
  }
}
