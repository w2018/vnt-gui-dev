// 连接状态面板（文档 §4.2.1）

import { useState } from 'react';
import { Button, Card, Descriptions, Space, Tag, Typography, message } from 'antd';
import { api } from '../lib/tauri';
import { useConnectionStore } from '../stores/connectionStore';
import { useConfigStore } from '../stores/configStore';

const statusMap: Record<string, { color: string; text: string }> = {
  stopped: { color: 'default', text: '未连接' },
  starting: { color: 'processing', text: '连接中...' },
  connected: { color: 'success', text: '已连接' },
  reconnecting: { color: 'warning', text: '重连中...' },
  error: { color: 'error', text: '错误' },
};

export function StatusPanel() {
  const { status, virtualIp, serverAddress, latency, errorMessage } = useConnectionStore();
  const { activeConfigId, configs } = useConfigStore();
  const [busy, setBusy] = useState(false);

  const active = configs.find((c) => c.id === activeConfigId);
  const running = status === 'connected' || status === 'starting' || status === 'reconnecting';
  const st = statusMap[status] ?? statusMap.stopped;

  const handleToggle = async () => {
    setBusy(true);
    try {
      if (running) {
        await api.stopConnection();
      } else if (active) {
        await api.startConnection(active.id);
      } else {
        message.warning('请先在"配置"页创建并激活一个配置');
      }
    } catch (e) {
      message.error(String(e));
    } finally {
      setBusy(false);
    }
  };

  return (
    <Card title="连接状态">
      <Space direction="vertical" size="middle" style={{ width: '100%' }}>
        <Tag color={st.color} style={{ fontSize: 14, padding: '4px 12px' }}>
          {st.text}
        </Tag>

        {status === 'error' && errorMessage && (
          <Typography.Text type="danger">{errorMessage}</Typography.Text>
        )}

        <Descriptions column={1} size="small" bordered>
          <Descriptions.Item label="虚拟 IP">{virtualIp ?? '未分配'}</Descriptions.Item>
          <Descriptions.Item label="服务器">
            {serverAddress ?? active?.server_address ?? '默认官方服务器'}
          </Descriptions.Item>
          <Descriptions.Item label="延迟">{latency != null ? `${latency}ms` : '-'}</Descriptions.Item>
          <Descriptions.Item label="活动配置">{active?.name ?? '未选择'}</Descriptions.Item>
        </Descriptions>

        <Button type="primary" loading={busy} onClick={handleToggle}>
          {running ? '断开连接' : '连接'}
        </Button>
      </Space>
    </Card>
  );
}
