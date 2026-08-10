// F8 状态卡片：已停止 / 运行中 / 异常 + 监听地址（全部网卡 IPv4 + 端口，Bug 3）

import { useEffect, useState } from 'react';
import { Badge, Button, Card, Space, Tag, Typography, message } from 'antd';
import { Play, Square } from 'lucide-react';
import { useFtpStore } from '../../stores/useFtpStore';
import { api } from '../../lib/tauri';

const STATE_META: Record<string, { color: string; text: string }> = {
  running: { color: 'green', text: '运行中' },
  stopped: { color: 'default', text: '已停止' },
  error: { color: 'red', text: '异常' },
};

export function FtpStatusCard() {
  const { status, start, stop, refreshStatus } = useFtpStore();
  const meta = STATE_META[status.state] ?? STATE_META.stopped;
  const running = status.state === 'running';
  // 全部监听地址（运行中轮询 5 秒刷新）
  const [addresses, setAddresses] = useState<string[]>([]);

  useEffect(() => {
    if (status.state !== 'running') {
      setAddresses([]);
      return;
    }
    let cancelled = false;
    const tick = async () => {
      try {
        const list = await api.ftpGetListenAddresses();
        if (!cancelled) setAddresses(list);
      } catch {
        /* 忽略：下次轮询重试 */
      }
    };
    void tick();
    const timer = window.setInterval(tick, 5000);
    return () => {
      cancelled = true;
      window.clearInterval(timer);
    };
  }, [status.state]);

  const handleToggle = async () => {
    try {
      if (running) {
        await stop();
        message.success('FTP 服务已停止');
      } else {
        await start();
        message.success('FTP 服务已启动');
      }
    } catch (e) {
      message.error(`操作失败: ${String(e)}`);
    }
  };

  return (
    <Card
      title="运行状态"
      extra={
        <Button type="primary" icon={running ? <Square size={14} /> : <Play size={14} />} onClick={handleToggle}>
          {running ? '停止' : '启动'}
        </Button>
      }
    >
      <Space direction="vertical" size="small">
        <Space>
          <Badge status={meta.color as 'success'} text={<Typography.Text strong>{meta.text}</Typography.Text>} />
        </Space>
        <div>
          <Typography.Text type="secondary">监听地址：</Typography.Text>
          {running ? (
            <div style={{ marginTop: 4 }}>
              <Space size={4} wrap>
                {addresses.map((addr) => (
                  <Tag key={addr} color="blue" style={{ fontFamily: 'Consolas, monospace' }}>
                    {addr}
                  </Tag>
                ))}
              </Space>
              <div>
                <Typography.Text type="secondary" style={{ fontSize: 12 }}>
                  控制通道：{status.listen_addr ?? '-'}
                </Typography.Text>
              </div>
            </div>
          ) : (
            <Typography.Text type="secondary">未监听</Typography.Text>
          )}
        </div>
        {status.state === 'error' && status.error && (
          <Typography.Text type="danger">{status.error}</Typography.Text>
        )}
        <Typography.Link onClick={() => void refreshStatus()}>刷新状态</Typography.Link>
      </Space>
    </Card>
  );
}
