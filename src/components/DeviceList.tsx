// 设备列表（文档 §4.2.6）：连接时每 5 秒轮询

import { useEffect, useRef } from 'react';
import { Button, Card, Empty, Space, Table, Tag, Typography, message } from 'antd';
import { Copy, RefreshCw } from 'lucide-react';
import type { ColumnsType } from 'antd/es/table';
import { useDeviceStore } from '../stores/deviceStore';
import type { PeerInfo } from '../lib/types';

export function DeviceList() {
  const { devices, refresh } = useDeviceStore();
  const timerRef = useRef<number | null>(null);

  useEffect(() => {
    void refresh();
    timerRef.current = window.setInterval(() => {
      void refresh();
    }, 5000);
    return () => {
      if (timerRef.current) window.clearInterval(timerRef.current);
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
      title: '延迟',
      dataIndex: 'latency',
      key: 'latency',
      render: (ms: number) => (ms > 0 ? `${ms}ms` : '-'),
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
