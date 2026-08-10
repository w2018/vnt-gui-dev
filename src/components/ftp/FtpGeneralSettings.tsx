// F2/F3/F4/F7 常规设置：ROOT 目录 + 端口 + PASV 范围 + 随应用/系统自启

import { Button, Card, Col, Input, InputNumber, Row, Space, Switch, Typography, message } from 'antd';
import { FolderOpen } from 'lucide-react';
import { useEffect, useState } from 'react';
import { useFtpStore } from '../../stores/useFtpStore';

export function FtpGeneralSettings() {
  const { config, saveConfig, pickRootDir, saving } = useFtpStore();
  const [rootDir, setRootDir] = useState(config.root_dir);
  const [port, setPort] = useState(config.port);
  const [pasv, setPasv] = useState<[number, number] | null>(config.pasv_ports);
  const [busy, setBusy] = useState(false);

  // 🆕 修复：store 异步加载完成/保存后同步本地编辑态（否则重启后 ROOT 目录/端口不显示）
  useEffect(() => {
    setRootDir(config.root_dir);
  }, [config.root_dir]);
  useEffect(() => {
    setPort(config.port);
  }, [config.port]);
  useEffect(() => {
    setPasv(config.pasv_ports);
  }, [config.pasv_ports]);

  const handlePick = async () => {
    setBusy(true);
    try {
      const dir = await pickRootDir();
      if (dir) {
        setRootDir(dir);
        await saveConfig({ root_dir: dir });
        message.success('ROOT 目录已更新');
      }
    } catch (e) {
      message.error(`选择失败: ${String(e)}`);
    } finally {
      setBusy(false);
    }
  };

  const handleSave = async () => {
    try {
      await saveConfig({ root_dir: rootDir, port, pasv_ports: pasv });
      message.success('常规设置已保存');
    } catch (e) {
      message.error(`保存失败: ${String(e)}`);
    }
  };

  return (
    <Card title="常规设置">
      <Space direction="vertical" size="middle" style={{ width: '100%' }}>
        {/* F4 ROOT 目录 */}
        <div>
          <Typography.Text strong>ROOT 目录</Typography.Text>
          <Row gutter={8} style={{ marginTop: 8 }}>
            <Col flex="auto">
              <Input
                value={rootDir}
                onChange={(e) => setRootDir(e.target.value)}
                placeholder="选择 FTP 根目录（默认空 = 未配置）"
              />
            </Col>
            <Col>
              <Button icon={<FolderOpen size={14} />} onClick={handlePick} loading={busy}>
                浏览
              </Button>
            </Col>
          </Row>
        </div>

        {/* F7 端口 */}
        <Row gutter={16}>
          <Col span={8}>
            <Typography.Text strong>控制端口</Typography.Text>
            <InputNumber
              min={1}
              max={65535}
              value={port}
              onChange={(v) => setPort(v ?? 2121)}
              style={{ width: '100%', marginTop: 8 }}
            />
            <Typography.Text type="secondary" style={{ fontSize: 12 }}>
              默认 2121（避免占用系统 FTP 21）
            </Typography.Text>
          </Col>
          <Col span={8}>
            <Typography.Text strong>PASV 端口范围</Typography.Text>
            <Row gutter={8} style={{ marginTop: 8 }}>
              <Col span={11}>
                <InputNumber
                  min={1024}
                  max={65535}
                  placeholder="起始"
                  value={pasv?.[0]}
                  onChange={(v) => setPasv(v == null ? null : [v, pasv?.[1] ?? v + 100])}
                  style={{ width: '100%' }}
                />
              </Col>
              <Col span={2} style={{ textAlign: 'center', lineHeight: '32px' }}>
                -
              </Col>
              <Col span={11}>
                <InputNumber
                  min={1024}
                  max={65535}
                  placeholder="结束"
                  value={pasv?.[1]}
                  onChange={(v) => setPasv(v == null ? null : [pasv?.[0] ?? 30000, v])}
                  style={{ width: '100%' }}
                />
              </Col>
            </Row>
            <Typography.Text type="secondary" style={{ fontSize: 12 }}>
              留空 = 自动分配
            </Typography.Text>
          </Col>
        </Row>

        {/* F2 随应用自启 */}
        <Row align="middle" justify="space-between">
          <Col>
            <Typography.Text strong>随应用自启</Typography.Text>
            <div>
              <Typography.Text type="secondary" style={{ fontSize: 12 }}>
                打开 VNT GUI 时自动启动 FTP 服务
              </Typography.Text>
            </div>
          </Col>
          <Col>
            <Switch
              checked={config.auto_start_with_app}
              disabled={saving}
              onChange={(v) => void saveConfig({ auto_start_with_app: v }).catch((e) => message.error(String(e)))}
            />
          </Col>
        </Row>

        {/* F3 随系统开机自启 */}
        <Row align="middle" justify="space-between">
          <Col>
            <Typography.Text strong>随系统开机自启</Typography.Text>
            <div>
              <Typography.Text type="secondary" style={{ fontSize: 12 }}>
                开机静默启动 VNT GUI（不弹窗口，daemon 按持久化状态自动恢复 VNT/FTP 服务）
              </Typography.Text>
            </div>
          </Col>
          <Col>
            <Switch
              checked={config.auto_start_with_system}
              disabled={saving}
              onChange={(v) => void saveConfig({ auto_start_with_system: v }).catch((e) => message.error(String(e)))}
            />
          </Col>
        </Row>

        {/* 总开关 + 保存 */}
        <Row align="middle" justify="space-between">
          <Col>
            <Typography.Text strong>启用 FTP 服务（总开关）</Typography.Text>
            <div>
              <Typography.Text type="secondary" style={{ fontSize: 12 }}>
                关闭后不可启动服务
              </Typography.Text>
            </div>
          </Col>
          <Col>
            <Switch
              checked={config.enabled}
              disabled={saving}
              onChange={(v) => void saveConfig({ enabled: v }).catch((e) => message.error(String(e)))}
            />
          </Col>
        </Row>

        <Button type="primary" onClick={handleSave} loading={saving}>
          保存常规设置
        </Button>
      </Space>
    </Card>
  );
}
