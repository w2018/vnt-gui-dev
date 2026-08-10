// 桌面共享设置面板

import { Card, Col, InputNumber, Row, Slider, Space, Switch, Typography, message } from 'antd';
import { useState } from 'react';
import { useDesktopStore } from '../../stores/useDesktopStore';

const { Text } = Typography;

export function DesktopSettings() {
  const { config, saveConfig, loadConfig, encoderAvailable } = useDesktopStore();
  // hooks 必须在任何条件 return 之前（否则 config 加载完成后 hooks 数量变化 → React #310 崩溃）
  const [collapsed, setCollapsed] = useState(true);

  if (!config) {
    return (
      <Card title="设置" size="small">
        <a onClick={() => loadConfig().catch(() => {})}>加载设置</a>
      </Card>
    );
  }

  const updateCapture = (patch: Partial<typeof config.capture>) => {
    saveConfig({ capture: { ...config.capture, ...patch } }).catch((e) =>
      message.error(String(e)),
    );
  };

  return (
    <Card
      size="small"
      title={
        <a onClick={() => setCollapsed(!collapsed)} style={{ fontSize: 14 }}>
          ⚙ 设置 {collapsed ? '▸' : '▾'}
        </a>
      }
    >
      {!collapsed && (
      <Row gutter={[12, 12]}>
        <Col span={24}>
          <Space direction="vertical" size="small" style={{ width: '100%' }}>
            <Space>
              <Text strong>允许被控制</Text>
              <Switch
                checked={config.allow_be_controlled}
                onChange={(v) =>
                  saveConfig({ allow_be_controlled: v }).catch((e) => message.error(String(e)))
                }
              />
            </Space>
            <Text type="secondary" style={{ fontSize: 12 }}>
              关闭后其他设备无法请求连接你的桌面
            </Text>
          </Space>
        </Col>

        <Col span={12}>
          <Text strong>帧率 (FPS)</Text>
          <Slider
            min={10}
            max={60}
            value={config.capture.fps}
            onChange={(v) => updateCapture({ fps: v })}
            marks={{ 10: '10', 30: '30', 60: '60' }}
          />
        </Col>

        <Col span={12}>
          <Text strong>码率 (Kbps)</Text>
          <InputNumber
            min={500}
            max={10000}
            step={500}
            value={config.capture.bitrate_kbps}
            onChange={(v) => updateCapture({ bitrate_kbps: v ?? 2000 })}
            style={{ width: '100%' }}
            addonAfter="Kbps"
          />
        </Col>

        <Col span={12}>
          <Text strong>输出宽度</Text>
          <InputNumber
            min={640}
            max={3840}
            step={160}
            value={config.capture.width}
            onChange={(v) => updateCapture({ width: v ?? 1920 })}
            style={{ width: '100%' }}
          />
        </Col>

        <Col span={12}>
          <Text strong>输出高度</Text>
          <InputNumber
            min={480}
            max={2160}
            step={90}
            value={config.capture.height}
            onChange={(v) => updateCapture({ height: v ?? 1080 })}
            style={{ width: '100%' }}
          />
        </Col>

        <Col span={24}>
          <Text strong>画质 (CRF)</Text>
          <Slider
            min={10}
            max={35}
            value={config.capture.quality}
            onChange={(v) => updateCapture({ quality: v })}
            marks={{ 10: '高', 23: '中', 35: '低' }}
          />
        </Col>

        <Col span={24}>
          <Space>
            <Text strong>剪贴板同步</Text>
            <Switch
              checked={config.clipboard_sync}
              onChange={(v) =>
                saveConfig({ clipboard_sync: v }).catch((e) => message.error(String(e)))
              }
            />
          </Space>
        </Col>

        <Col span={24}>
          <Text
            type={encoderAvailable === false ? 'danger' : 'secondary'}
            style={{ fontSize: 12 }}
          >
            {encoderAvailable === null
              ? '正在检查系统编码器...'
              : encoderAvailable
                ? '✅ 系统 H.264 编码器可用（Windows Media Foundation）'
                : '⚠️ 系统缺少 H.264 编码器（Windows N 版需安装媒体功能包）'}
          </Text>
        </Col>
      </Row>
      )}
    </Card>
  );
}
