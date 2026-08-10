// 桌面共享设置—— 桌面共享页"设置"标签页内容（标签页本身已收纳，直接展开显示全部设置项）

import { Button, Col, InputNumber, Row, Slider, Space, Switch, Typography, message } from 'antd';
import { useDesktopStore } from '../../stores/useDesktopStore';

const { Text } = Typography;

// 采集预设档位（被控端生效；保存后采集线程立即重建编码器）
const CAPTURE_PRESETS = [
  {
    key: 'smooth',
    label: '流畅',
    desc: '720p / 10fps / 0.8Mbps（低内存推荐）',
    capture: { fps: 10, bitrate_kbps: 800, width: 1280, height: 720, quality: 30 },
  },
  {
    key: 'balanced',
    label: '均衡',
    desc: '1080p / 30fps / 2Mbps',
    capture: { fps: 30, bitrate_kbps: 2000, width: 1920, height: 1080, quality: 23 },
  },
  {
    key: 'hd',
    label: '高清',
    desc: '1080p / 30fps / 6Mbps',
    capture: { fps: 30, bitrate_kbps: 6000, width: 1920, height: 1080, quality: 15 },
  },
] as const;

export function DesktopSettings() {
  const { config, saveConfig, loadConfig, encoderAvailable } = useDesktopStore();

  if (!config) {
    return <a onClick={() => loadConfig().catch(() => {})}>加载设置</a>;
  }

  const updateCapture = (patch: Partial<typeof config.capture>) => {
    saveConfig({ capture: { ...config.capture, ...patch } }).catch((e) =>
      message.error(String(e)),
    );
  };

  // 当前配置是否匹配某预设
  const isActivePreset = (c: {
    fps: number;
    bitrate_kbps: number;
    width: number;
    height: number;
    quality: number;
  }) =>
    config.capture.fps === c.fps &&
    config.capture.bitrate_kbps === c.bitrate_kbps &&
    config.capture.width === c.width &&
    config.capture.height === c.height &&
    config.capture.quality === c.quality;

  return (
    <Row gutter={[24, 20]}>
      <Col span={24}>
        <Space size="middle">
          <Text strong>允许被控制</Text>
          <Switch
            checked={config.allow_be_controlled}
            onChange={(v) =>
              saveConfig({ allow_be_controlled: v }).catch((e) => message.error(String(e)))
            }
          />
        </Space>
        <div style={{ marginTop: 4 }}>
          <Text type="secondary">关闭后其他设备无法请求连接你的桌面</Text>
        </div>
      </Col>

      <Col span={24}>
        <Space direction="vertical" size="small" style={{ width: '100%' }}>
          <Text strong>采集预设（被控端生效，一键应用）</Text>
          <Space wrap>
            {CAPTURE_PRESETS.map((p) => (
              <Button
                key={p.key}
                type={isActivePreset(p.capture) ? 'primary' : 'default'}
                onClick={() =>
                  saveConfig({ capture: { ...config.capture, ...p.capture } })
                    .then(() => message.success(`已应用「${p.label}」预设（${p.desc}）`))
                    .catch((e) => message.error(String(e)))
                }
              >
                {p.label} · {p.desc}
              </Button>
            ))}
          </Space>
          <Text type="secondary" style={{ fontSize: 12 }}>
            共享运行中修改也会立即生效；虚拟机/低内存环境建议「流畅」档
          </Text>
        </Space>
      </Col>

      <Col xs={24} md={12}>
        <Space direction="vertical" size="small" style={{ width: '100%' }}>
          <Text strong>帧率 (FPS)</Text>
          <Slider
            min={10}
            max={60}
            value={config.capture.fps}
            onChange={(v) => updateCapture({ fps: v })}
            marks={{ 10: '10', 30: '30', 60: '60' }}
          />
        </Space>
      </Col>

      <Col xs={24} md={12}>
        <Space direction="vertical" size="small" style={{ width: '100%' }}>
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
        </Space>
      </Col>

      <Col xs={24} md={12}>
        <Space direction="vertical" size="small" style={{ width: '100%' }}>
          <Text strong>输出宽度</Text>
          <InputNumber
            min={640}
            max={3840}
            step={160}
            value={config.capture.width}
            onChange={(v) => updateCapture({ width: v ?? 1920 })}
            style={{ width: '100%' }}
          />
        </Space>
      </Col>

      <Col xs={24} md={12}>
        <Space direction="vertical" size="small" style={{ width: '100%' }}>
          <Text strong>输出高度</Text>
          <InputNumber
            min={480}
            max={2160}
            step={90}
            value={config.capture.height}
            onChange={(v) => updateCapture({ height: v ?? 1080 })}
            style={{ width: '100%' }}
          />
        </Space>
      </Col>

      <Col span={24}>
        <Space direction="vertical" size="small" style={{ width: '100%' }}>
          <Text strong>画质 (CRF)</Text>
          <Slider
            min={10}
            max={35}
            value={config.capture.quality}
            onChange={(v) => updateCapture({ quality: v })}
            marks={{ 10: '高', 23: '中', 35: '低' }}
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
  );
}
