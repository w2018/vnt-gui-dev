// 连接状态面板（文档 §4.2.1）：延时 = 每 5 秒 ping 服务器

import { useEffect, useState } from 'react';
import { Button, Card, Descriptions, Space, Tag, Tooltip, Typography, message } from 'antd';
import { api } from '../lib/tauri';
import { PING_INTERVAL } from '../lib/constants';
import { latencyColor } from '../lib/latency';
import { useConnectionStore } from '../stores/connectionStore';
import { useConfigStore } from '../stores/configStore';
import { useDeviceStore } from '../stores/deviceStore';

const statusMap: Record<string, { color: string; text: string }> = {
  stopped: { color: 'default', text: '未连接' },
  starting: { color: 'processing', text: '连接中...' },
  connected: { color: 'success', text: '已连接' },
  reconnecting: { color: 'warning', text: '重连中...' },
  error: { color: 'error', text: '错误' },
};

function maskToken(token: string): string {
  return token;
}

export function StatusPanel() {
  const { status, virtualIp, serverAddress, latency, errorMessage, setLatency } =
    useConnectionStore();
  const { activeConfigId, configs } = useConfigStore();
  const [busy, setBusy] = useState(false);
  // 延时检测状态：latency=null 且 pingFail=true → 真实超时；pingError 记录失败详情（tooltip 展示）
  const [pingFail, setPingFail] = useState(false);
  const [pingError, setPingError] = useState<string | null>(null);

  const active = configs.find((c) => c.id === activeConfigId);
  const localPeer = useDeviceStore((s) => s.localPeer);
  const running = status === 'connected' || status === 'starting' || status === 'reconnecting';
  const st = statusMap[status] ?? statusMap.stopped;

  // 延时 = ping 服务器（每 5 秒）：ping 目标 = 配置地址或日志提取的实际服务器；组件卸载时清除定时器
  useEffect(() => {
    let cancelled = false;
    const tick = async () => {
      let host: string | null = null;
      try {
        host = await api.getPingHost();
      } catch {
        /* 忽略 */
      }
      if (!host) {
        // 无任何可 ping 目标（未配置且未连接过）：显示 "-"
        if (!cancelled) {
          setLatency(null);
          setPingFail(false);
          setPingError('未配置服务器地址且未从日志获取到连接服务器');
        }
        return;
      }
      try {
        const ms = await api.pingHost(host);
        if (!cancelled) {
          setLatency(ms);
          setPingFail(false);
          setPingError(null);
        }
      } catch (e) {
        if (!cancelled) {
          setLatency(null);
          setPingFail(true);
          setPingError(String(e));
        }
      }
    };
    void tick();
    const timer = window.setInterval(tick, PING_INTERVAL);
    return () => {
      cancelled = true;
      window.clearInterval(timer);
    };
  }, [setLatency]);

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
    <Card title="连接信息">
      <Space direction="vertical" size="middle" style={{ width: '100%' }}>
        <Tag color={st.color} style={{ fontSize: 14, padding: '4px 12px' }}>
          {st.text}
        </Tag>

        {status === 'error' && errorMessage && (
          <Typography.Text type="danger">{errorMessage}</Typography.Text>
        )}

        <Descriptions column={1} size="small" bordered>
          <Descriptions.Item label="设备名">
            {localPeer?.name ?? active?.device_name ?? '-'}
          </Descriptions.Item>
          <Descriptions.Item label="虚拟 IP">{virtualIp ?? '未分配'}</Descriptions.Item>
          <Descriptions.Item label="服务器">
            {serverAddress ?? active?.server_address ?? '默认官方服务器'}
          </Descriptions.Item>
          <Descriptions.Item label="组网编号">
            {active?.token ? maskToken(active.token) : '-'}
          </Descriptions.Item>
          <Descriptions.Item
            label={
              <Tooltip title={pingError ?? '每 5 秒检测'}>延时（ping 服务器）</Tooltip>
            }
          >
            <Typography.Text
              style={{
                color: latencyColor(latency != null ? latency : pingFail ? -1 : null),
                fontWeight: 600,
              }}
            >
              {latency != null ? `${latency}ms` : pingFail ? '超时' : '-'}
            </Typography.Text>
          </Descriptions.Item>
          <Descriptions.Item label="活动配置">{active?.name ?? '未选择'}</Descriptions.Item>
        </Descriptions>

        <Button type="primary" loading={busy} onClick={handleToggle}>
          {running ? '断开连接' : '连接'}
        </Button>
      </Space>
    </Card>
  );
}
