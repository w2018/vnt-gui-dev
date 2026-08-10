// 可连接的设备列表（复用 VNT peers）—— 桌面共享页"设备"标签页内容

import { List, Tag, Button, Space, Typography } from 'antd';
import { useDesktopStore } from '../../stores/useDesktopStore';
import { useDeviceStore } from '../../stores/deviceStore';

const { Text } = Typography;

export function DeviceList() {
  const { setTargetIp, targetIp, session } = useDesktopStore();
  const devices = useDeviceStore((s) => s.devices);

  const online = devices.filter((d) => d.status === 'online');

  const isBusy = session.state.type === 'sharing' || session.state.type === 'connecting';

  if (online.length === 0) {
    return (
      <div style={{ padding: '16px 4px' }}>
        <Text type="secondary">暂无在线设备。确保 VNT 已连接且其他设备在线。</Text>
      </div>
    );
  }

  return (
    <List
      dataSource={online}
      renderItem={(peer) => (
        <List.Item
          actions={[
            <Button
              key="connect"
              type={targetIp === peer.virtual_ip ? 'primary' : 'link'}
              size="small"
              disabled={isBusy}
              onClick={() => setTargetIp(peer.virtual_ip)}
            >
              {targetIp === peer.virtual_ip ? '已选择' : '选择'}
            </Button>,
          ]}
        >
          <List.Item.Meta
            title={
              <Space size="middle">
                <span>{peer.name}</span>
                {peer.connection_type === 'p2p' ? (
                  <Tag color="green" style={{ marginInlineEnd: 0 }}>
                    P2P
                  </Tag>
                ) : (
                  <Tag color="orange" style={{ marginInlineEnd: 0 }}>
                    Relay
                  </Tag>
                )}
              </Space>
            }
            description={`${peer.virtual_ip} | ${peer.latency}ms`}
          />
        </List.Item>
      )}
    />
  );
}
