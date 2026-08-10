// 可连接的设备列表（复用 VNT peers）

import { Card, List, Tag, Button, Space, Typography } from 'antd';
import { useDesktopStore } from '../../stores/useDesktopStore';
import { useDeviceStore } from '../../stores/deviceStore';

const { Text } = Typography;

export function DeviceList() {
  const { setTargetIp, targetIp, session } = useDesktopStore();
  const devices = useDeviceStore((s) => s.devices);

  const online = devices.filter((d) => d.status === 'online');

  const isBusy = session.state.type === 'sharing' || session.state.type === 'connecting';

  return (
    <Card title="可连接的设备" size="small">
      {online.length === 0 ? (
        <Space direction="vertical" size="small" style={{ width: '100%' }}>
          <Text type="secondary" style={{ fontSize: 12 }}>
            暂无在线设备。确保 VNT 已连接且其他设备在线。
          </Text>
        </Space>
      ) : (
        <List
          size="small"
          dataSource={online}
          renderItem={(peer) => (
            <List.Item
              actions={[
                <Button
                  key="connect"
                  type="link"
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
                  <Space>
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
      )}
    </Card>
  );
}
