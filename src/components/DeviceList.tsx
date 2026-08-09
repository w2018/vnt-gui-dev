// 设备列表（文档 §4.2.6）：每 5 秒刷新列表 + 逐一 ping 每台设备（错峰，避免洪泛）

import { useEffect } from 'react';
import { Button, Card, Empty, Space, Table, Tag, Typography, message } from 'antd';
import { Copy, RefreshCw } from 'lucide-react';
import type { ColumnsType } from 'antd/es/table';
import { useDeviceStore } from '../stores/deviceStore';
import { api } from '../lib/tauri';
import { PING_INTERVAL } from '../lib/constants';
import type { PeerInfo } from '../lib/types';

export function DeviceList() {
  const { devices, refresh } = useDeviceStore();

  // 每 5 秒：刷新设备列表 + 并发 ping 每台设备；组件卸载时清除定时器
  useEffect(() => {
    let cancelled = false;
    const tick = async () => {
      await refresh();
      const list = useDeviceStore.getState().devices;
      // 并发 ping 所有设备（设备数有限，5s 一轮；单设备失败不影响其他）
      await Promise.all(
        list.map(async (dev) => {
          try {
            const ms = await api.pingHost(dev.virtual_ip);
            if (!cancelled) useDeviceStore.getState().updateLatency(dev.virtual_ip, ms);
          } catch {
            // 离线/超时：标记 -1，不中断后续设备检测
            if (!cancelled) useDeviceStore.getState().updateLatency(dev.virtual_ip, -1);
          }
        }),
      );
    };
    void tick();
    const timer = window.setInterval(tick, PING_INTERVAL);
    return () => {
      cancelled = true;
      window.clearInterval(timer);
    };
  }, [refresh]);

  const copyIp = async (ip: string) => {
    try {
      await navigator.clipboard.writeText(ip);
      message.success(`已复制 ${ip}`);
    } catch {
      message.warning('复制失败');
    }
  };

  const columns: ColumnsType<PeerInfo> = [
    { title: '设备名称', dataIndex: 'name', key: 'name' },
    {
      title: '虚拟 IP',
      dataIndex: 'virtual_ip',
      key: 'virtual_ip',
      render: (ip: string) => <Typography.Text code>{ip}</Typography.Text>,
    },
    {
      title: '连接类型',
      dataIndex: 'connection_type',
      key: 'connection_type',
      render: (t: string) =>
        t === 'p2p' ? <Tag color="green">P2P 直连</Tag> : <Tag color="orange">中继</Tag>,
    },
    {
      title: '延时',
      dataIndex: 'latency',
      key: 'latency',
      render: (ms: number) => (ms > 0 ? `${ms}ms` : ms === 0 ? '--' : '超时'),
    },
    {
      title: '状态',
      dataIndex: 'status',
      key: 'status',
      render: (s: string) =>
        s === 'online' ? <Tag color="success">在线</Tag> : <Tag>离线</Tag>,
    },
    {
      title: '操作',
      key: 'action',
      render: (_, record) => (
        <Space>
          <Button size="small" icon={<Copy size={13} />} onClick={() => copyIp(record.virtual_ip)}>
            复制 IP
          </Button>
        </Space>
      ),
    },
  ];

  return (
    <Card
      title="设备列表"
      extra={
        <Button size="small" icon={<RefreshCw size={13} />} onClick={() => void refresh()}>
          刷新
        </Button>
      }
    >
      {devices.length === 0 ? (
        <Empty description="暂无在线设备（连接后自动刷新）" />
      ) : (
        <Table
          rowKey="virtual_ip"
          columns={columns}
          dataSource={devices}
          size="small"
          pagination={false}
        />
      )}
    </Card>
  );
}
