// 桌面共享主页面：标签页收纳式布局
// 顶栏常驻连接操作 + 远程画面最大化 + 底部功能标签页（连接/被控端/设备/设置）

import {
  Button,
  Card,
  Checkbox,
  Col,
  Descriptions,
  Input,
  Row,
  Space,
  Tag,
  Tabs,
  Typography,
  message,
} from 'antd';
import { useEffect } from 'react';
import { MonitorPlay } from 'lucide-react';
import { ControlBar } from '../components/desktop/ControlBar';
import { DesktopSettings } from '../components/desktop/DesktopSettings';
import { DeviceList } from '../components/desktop/DeviceList';
import { RemoteCanvas } from '../components/desktop/RemoteCanvas';
import { useDesktopStore } from '../stores/useDesktopStore';
import { useDeviceStore } from '../stores/deviceStore';
import type { DesktopLocalInfo, SessionInfo } from '../types/desktop';

const { Title, Text } = Typography;

export function DesktopShare() {
  const {
    initError,
    localInfo,
    session,
    targetIp,
    viewOnly,
    setTargetIp,
    setViewOnly,
    init,
    connect,
    disconnect,
    startSharing,
    stopSharing,
  } = useDesktopStore();

  // 初始化 + 状态轮询 + 设备列表轮询（事件监听已由 App 全局注册，跨页面/后台接收连接请求）
  useEffect(() => {
    let timer: number | undefined;
    let deviceTimer: number | undefined;

    (async () => {
      try {
        if (!useDesktopStore.getState().initialized) {
          await init();
        }
      } catch (e) {
        message.error(`桌面共享初始化失败: ${String(e)}`);
      }
    })();

    timer = window.setInterval(() => {
      useDesktopStore.getState().refreshSession();
    }, 3000);

    // 设备列表及时刷新（VNT peers）
    deviceTimer = window.setInterval(() => {
      useDeviceStore.getState().refresh();
    }, 5000);
    useDeviceStore.getState().refresh();

    return () => {
      window.clearInterval(timer);
      window.clearInterval(deviceTimer);
    };
  }, [init]);

  const stateType = session.state.type;
  const isSharing = stateType === 'sharing';
  const isBusy = stateType === 'connecting' || stateType === 'waiting_confirm';
  const isHost = session.role === 'host';

  const handleConnect = () => connect().catch((e) => message.error(String(e)));
  const handleDisconnect = () =>
    disconnect('用户主动断开').catch((e) => message.error(String(e)));

  return (
    <div
      style={{
        height: 'calc(100vh - 48px)',
        display: 'flex',
        flexDirection: 'column',
        gap: 12,
      }}
    >
      {/* 顶部：标题 + 状态 + 常驻连接操作 */}
      <Card style={{ flexShrink: 0 }}>
        <Row align="middle" justify="space-between" gutter={[16, 8]} wrap>
          <Col flex="auto">
            <Space size="middle" wrap>
              <Title level={4} style={{ margin: 0 }}>
                桌面共享
              </Title>
              <SessionStatusBadge session={session} />
              {isSharing && session.remote_device && (
                <Tag color="blue">{session.remote_device}</Tag>
              )}
            </Space>
          </Col>
          <Col>
            <Space size="middle" wrap>
              <Checkbox
                checked={viewOnly}
                onChange={(e) => setViewOnly(e.target.checked)}
                disabled={isSharing || isBusy}
              >
                仅查看
              </Checkbox>
              <Input
                placeholder="目标 VNT IP，如 10.26.0.4"
                value={targetIp}
                onChange={(e) => setTargetIp(e.target.value)}
                disabled={isSharing || isBusy}
                onPressEnter={() => {
                  if (!isSharing && !isBusy) {
                    handleConnect();
                  }
                }}
                allowClear
                style={{ width: 220 }}
              />
              <Button
                type="primary"
                loading={stateType === 'connecting'}
                disabled={isSharing || isBusy || !localInfo}
                onClick={handleConnect}
              >
                连接
              </Button>
              <Button danger disabled={!isSharing && !isBusy} onClick={handleDisconnect}>
                断开
              </Button>
            </Space>
          </Col>
        </Row>
      </Card>

      {/* 主体：远程画面最大化 */}
      <Card
        style={{ flex: 1, minHeight: 0, display: 'flex', flexDirection: 'column' }}
        styles={{
          body: {
            flex: 1,
            display: 'flex',
            alignItems: 'center',
            justifyContent: 'center',
            overflow: 'hidden',
            padding: 12,
          },
        }}
        title={
          <Space>
            <span>远程桌面</span>
            {isSharing && session.screen && (
              <Tag color="green">
                {session.screen.width}×{session.screen.height}
              </Tag>
            )}
          </Space>
        }
        extra={
          isSharing && session.stats.uptime_secs > 0 ? (
            <Text type="secondary" style={{ fontSize: 12 }}>
              已运行 {Math.floor(session.stats.uptime_secs / 60)}m
              {session.stats.uptime_secs % 60}s
            </Text>
          ) : undefined
        }
      >
        {isSharing ? (
          isHost ? (
            // 被控端：本机屏幕正在共享，无需渲染远程画面解码器
            <div
              style={{
                width: '100%',
                height: '100%',
                display: 'flex',
                alignItems: 'center',
                justifyContent: 'center',
                flexDirection: 'column',
                gap: 12,
                background: '#1a1a1a',
                borderRadius: 8,
              }}
            >
              <MonitorPlay size={48} color="#666" />
              <Text style={{ color: '#999' }}>
                本机屏幕正在共享给 {session.remote_device ?? '对方设备'}
              </Text>
              <Text type="secondary" style={{ fontSize: 12 }}>
                被控端无需显示远程画面，切换页面共享不中断
              </Text>
            </div>
          ) : (
            <RemoteCanvas />
          )
        ) : (
          <div
            style={{
              width: '100%',
              height: '100%',
              display: 'flex',
              alignItems: 'center',
              justifyContent: 'center',
              flexDirection: 'column',
              gap: 8,
              background: '#1a1a1a',
              borderRadius: 8,
              color: '#666',
            }}
          >
            {stateType === 'connecting'
              ? '正在连接...'
              : stateType === 'waiting_confirm'
                ? '等待对方确认...'
                : stateType === 'error'
                  ? '会话错误'
                  : '未连接，输入目标 IP 或从「设备」页签选择设备'}
          </div>
        )}
      </Card>

      {/* 控制栏（共享时） */}
      {isSharing && (
        <div style={{ flexShrink: 0 }}>
          <ControlBar />
        </div>
      )}

      {/* 功能标签页：连接 / 被控端 / 设备 / 设置 */}
      <div style={{ flexShrink: 0 }}>
        <Tabs
          defaultActiveKey="info"
          items={[
            {
              key: 'info',
              label: '连接',
              children: (
                <ConnectionInfoPanel
                  localInfo={localInfo}
                  session={session}
                  initError={initError}
                  isHost={isHost}
                />
              ),
            },
            {
              key: 'host',
              label: '被控端',
              children: (
                <Space direction="vertical" size="middle" style={{ width: '100%' }}>
                  <Text type="secondary">
                    其他设备经 VNT IP 请求连接本机
                    {isHost && isSharing ? '（共享中）' : ''}
                  </Text>
                  {isHost ? (
                    isSharing ? (
                      <Button
                        danger
                        block
                        onClick={() =>
                          stopSharing().catch((e) => message.error(String(e)))
                        }
                      >
                        停止共享
                      </Button>
                    ) : (
                      <Button
                        type="primary"
                        block
                        onClick={() =>
                          startSharing().catch((e) => message.error(String(e)))
                        }
                      >
                        开始共享
                      </Button>
                    )
                  ) : (
                    <Text type="secondary">接受连接请求后自动开始共享</Text>
                  )}
                </Space>
              ),
            },
            { key: 'devices', label: '设备', children: <DeviceList /> },
            { key: 'settings', label: '设置', children: <DesktopSettings /> },
          ]}
        />
      </div>

    </div>
  );
}

// 连接信息面板：本机信息 + 会话状态
function ConnectionInfoPanel({
  localInfo,
  session,
  initError,
  isHost,
}: {
  localInfo: DesktopLocalInfo | null;
  session: SessionInfo;
  initError: string | null;
  isHost: boolean;
}) {
  const stateText = (() => {
    switch (session.state.type) {
      case 'sharing':
        return '共享中';
      case 'connecting':
        return '连接中';
      case 'waiting_confirm':
        return '等待对方确认';
      case 'idle':
        return '空闲';
      case 'disconnected':
        return session.state.reason
          ? `已断开（${session.state.reason}）`
          : '已断开';
      case 'error':
        return session.state.message ? `错误（${session.state.message}）` : '错误';
      default:
        return '未知';
    }
  })();

  return (
    <Row gutter={[48, 16]}>
      <Col xs={24} sm={12}>
        <Text strong>本机信息</Text>
        <div style={{ marginTop: 8 }}>
          <Descriptions
            column={1}
            size="small"
            colon={false}
            items={[
              {
                key: 'vnt_ip',
                label: 'VNT IP',
                children: localInfo?.vnt_ip ?? (initError ? '初始化失败' : '-'),
              },
              {
                key: 'port',
                label: '监听端口',
                children: localInfo
                  ? localInfo.listen_addr.split(':')[1] ?? '-'
                  : '-',
              },
              {
                key: 'node',
                label: 'Node ID',
                children: localInfo
                  ? `${localInfo.node_id.slice(0, 16)}...`
                  : '-',
              },
            ]}
          />
        </div>
      </Col>
      <Col xs={24} sm={12}>
        <Text strong>会话状态</Text>
        <div style={{ marginTop: 8 }}>
          <Descriptions
            column={1}
            size="small"
            colon={false}
            items={[
              { key: 'role', label: '当前角色', children: isHost ? '被控端' : '控制端' },
              { key: 'state', label: '连接状态', children: stateText },
              {
                key: 'remote',
                label: '远程设备',
                children: session.remote_device ?? '-',
              },
              {
                key: 'screen',
                label: '画面',
                children: session.screen
                  ? `${session.screen.width}×${session.screen.height}`
                  : '-',
              },
              {
                key: 'stats',
                label: '帧率 / 延时',
                children:
                  session.stats.fps > 0
                    ? `${session.stats.fps}fps / ${session.stats.latency_ms}ms`
                    : '-',
              },
            ]}
          />
        </div>
      </Col>
    </Row>
  );
}

// 会话状态徽章
function SessionStatusBadge({ session }: { session: SessionInfo }) {
  switch (session.state.type) {
    case 'sharing':
      return <Tag color="green">● 共享中</Tag>;
    case 'connecting':
      return <Tag color="blue">● 连接中</Tag>;
    case 'waiting_confirm':
      return <Tag color="orange">● 等待确认</Tag>;
    case 'idle':
      return <Tag>○ 空闲</Tag>;
    case 'disconnected':
      return <Tag color="default">● 已断开</Tag>;
    case 'error':
      return <Tag color="red">● 错误</Tag>;
    default:
      return <Tag>未知</Tag>;
  }
}
