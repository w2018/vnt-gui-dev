// F8 状态卡片：已停止 / 运行中 / 异常 + 监听地址

import { Badge, Button, Card, Space, Typography, message } from 'antd';
import { Play, Square } from 'lucide-react';
import { useFtpStore } from '../../stores/useFtpStore';

const STATE_META: Record<string, { color: string; text: string }> = {
  running: { color: 'green', text: '运行中' },
  stopped: { color: 'default', text: '已停止' },
  error: { color: 'red', text: '异常' },
};

export function FtpStatusCard() {
  const { status, start, stop, refreshStatus } = useFtpStore();
  const meta = STATE_META[status.state] ?? STATE_META.stopped;
  const running = status.state === 'running';

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
        <Typography.Text type="secondary">
          监听地址：{status.listen_addr ?? '未监听'}
        </Typography.Text>
        {status.state === 'error' && status.error && (
          <Typography.Text type="danger">{status.error}</Typography.Text>
        )}
        <Typography.Link onClick={() => void refreshStatus()}>刷新状态</Typography.Link>
      </Space>
    </Card>
  );
}
