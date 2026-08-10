// 桌面共享主页面：控制端 + 被控端双模（左侧窄栏 + 大画面）

import { Button, Card, Checkbox, Col, Input, Row, Space, Tag, Typography, message } from 'antd';
import { useEffect } from 'react';
import { ClipboardSync } from '../components/desktop/ClipboardSync';
import { ConnectionRequest } from '../components/desktop/ConnectionRequest';
import { ControlBar } from '../components/desktop/ControlBar';
import { DesktopSettings } from '../components/desktop/DesktopSettings';
import { DeviceList } from '../components/desktop/DeviceList';
import { RemoteCanvas } from '../components/desktop/RemoteCanvas';
import { useDesktopStore } from '../stores/useDesktopStore';
import { useDeviceStore } from '../stores/deviceStore';
import type { SessionInfo } from '../types/desktop';

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
    setupListeners,
    connect,
    disconnect,
    startSharing,
    stopSharing,
  } = useDesktopStore();

  // 初始化 + 事件监听 + 状态轮询 + 设备列表轮询
  useEffect(() => {
    let unlisten: (() => void) | undefined;
    let timer: number | undefined;
    let deviceTimer: number | undefined;

    (async () => {
      try {
        if (!useDesktopStore.getState().initialized) {
          await init();
        }
        unlisten = await setupListeners();
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
      unlisten?.();
      window.clearInterval(timer);
      window.clearInterval(deviceTimer);
    };
  }, [init, setupListeners]);

  const stateType = session.state.type;
  const isSharing = stateType === 'sharing';
  const isBusy = stateType === 'connecting' || stateType === 'waiting_confirm';
  const isHost = session.role === 'host';

  return (
    <div style={{ padding: 12, maxWidth: 1500 }}>
      {/* 顶部状态条 */}
      <Card size="small" style={{ marginBottom: 12 }}>
        <Row align="middle" justify="space-between">
          <Col>
            <Space>
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
            <Space direction="vertical" size={0} style={{ textAlign: 'right' }}>
              {localInfo ? (
                <>
                  <Text type="secondary" style={{ fontSize: 12 }}>
                    本机 {localInfo.vnt_ip} | 端口 {localInfo.listen_addr.split(':')[1] ?? '-'}
                  </Text>
                  <Text type="secondary" style={{ fontSize: 12 }}>
                    Node {localInfo.node_id.slice(0, 16)}...
                  </Text>
                </>
              ) : (
                <Text type="secondary" style={{ fontSize: 12 }}>
                  {initError ? `初始化失败: ${initError}` : '未初始化'}
                </Text>
              )}
            </Space>
          </Col>
        </Row>
      </Card>

      <Row gutter={12}>
        {/* 左侧窄栏：连接 / 被控端 / 设备 / 设置 */}
        <Col span={7}>
          <Space direction="vertical" size="small" style={{ width: '100%' }}>
            {/* 连接面板 */}
            <Card size="small" title="连接到设备">
              <Space direction="vertical" size="small" style={{ width: '100%' }}>
                <Input
                  placeholder="目标 VNT IP（如 10.26.0.4）"
                  value={targetIp}
                  onChange={(e) => setTargetIp(e.target.value)}
                  disabled={isSharing || isBusy}
                  onPressEnter={() => {
                    if (!isSharing && !isBusy) {
                      connect().catch((e) => message.error(String(e)));
                    }
                  }}
                />
                <Space style={{ width: '100%', justifyContent: 'space-between' }}>
                  <Checkbox
                    checked={viewOnly}
                    onChange={(e) => setViewOnly(e.target.checked)}
                    disabled={isSharing || isBusy}
                  >
                    仅查看
                  </Checkbox>
                  <Space>
                    <Button
                      type="primary"
                      size="small"
                      loading={stateType === 'connecting'}
                      disabled={isSharing || isBusy || !localInfo}
                      onClick={() => connect().catch((e) => message.error(String(e)))}
                    >
                      连接
                    </Button>
                    <Button
                      danger
                      size="small"
                      disabled={!isSharing && !isBusy}
                      onClick={() =>
                        disconnect('用户主动断开').catch((e) => message.error(String(e)))
                      }
                    >
                      断开
                    </Button>
                  </Space>
                </Space>
              </Space>
            </Card>

            {/* 被控端 */}
            <Card size="small" title="被控端">
              <Space direction="vertical" size="small" style={{ width: '100%' }}>
                <Text type="secondary" style={{ fontSize: 12 }}>
                  其他设备经 VNT IP 请求连接本机
                  {isHost && isSharing ? '（共享中）' : ''}
                </Text>
                {isHost ? (
                  isSharing ? (
                    <Button
                      size="small"
                      block
                      onClick={() => stopSharing().catch((e) => message.error(String(e)))}
                    >
                      停止共享
                    </Button>
                  ) : (
                    <Button
                      size="small"
                      block
                      onClick={() => startSharing().catch((e) => message.error(String(e)))}
                    >
                      开始共享
                    </Button>
                  )
                ) : (
                  <Text type="secondary" style={{ fontSize: 12 }}>
                    接受连接请求后自动开始共享
                  </Text>
                )}
              </Space>
            </Card>

            <DeviceList />

            <DesktopSettings />
          </Space>
        </Col>

        {/* 右侧：大画面 */}
        <Col span={17}>
          <Card
            size="small"
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
              isSharing && (
                <Text type="secondary" style={{ fontSize: 12 }}>
                  {session.stats.uptime_secs > 0
                    ? `${Math.floor(session.stats.uptime_secs / 60)}m${session.stats.uptime_secs % 60}s`
                    : ''}
                </Text>
              )
            }
          >
            {isSharing ? (
              <RemoteCanvas />
            ) : (
              <div
                style={{
                  height: 480,
                  display: 'flex',
                  alignItems: 'center',
                  justifyContent: 'center',
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
                      : '未连接'}
              </div>
            )}
          </Card>

          {isSharing && (
            <div style={{ marginTop: 8 }}>
              <ControlBar />
            </div>
          )}

          {isSharing && <ClipboardSync />}
        </Col>
      </Row>

      {/* 连接请求弹窗（被控端） */}
      <ConnectionRequest />
    </div>
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
