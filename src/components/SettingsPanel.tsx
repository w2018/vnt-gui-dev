// 设置面板：自启 / 主题 / 流量告警 / 配置导入导出 / 快捷键 / 版本信息

import { useEffect, useState } from 'react';
import {
  Button,
  Card,
  Col,
  Divider,
  InputNumber,
  Row,
  Space,
  Switch,
  Typography,
  message,
} from 'antd';
import { Moon, Sun, Upload, Download } from 'lucide-react';
import { open as openDialog, save as saveDialog } from '@tauri-apps/plugin-dialog';
import {
  isRegistered,
  register,
  unregister,
} from '@tauri-apps/plugin-global-shortcut';
import { getCurrentWindow } from '@tauri-apps/api/window';
import { api } from '../lib/tauri';
import { isDark, onThemeChange, setDark } from '../lib/theme';
import { useConfigStore } from '../stores/configStore';

const ALERT_KEY = 'vnt-alert-threshold';

export function SettingsPanel() {
  const [autostart, setAutostart] = useState(false);
  const [dark, setDarkState] = useState(isDark());
  const [shortcut, setShortcut] = useState(false);
  const [alertEnabled, setAlertEnabled] = useState(false);
  const [alertUp, setAlertUp] = useState<number>(10);
  const [alertDown, setAlertDown] = useState<number>(10);
  const [version, setVersion] = useState('');
  const [vntVersion, setVntVersion] = useState('');
  // 托盘可见性设置（1a/1b）
  const [hideTrayAutostart, setHideTrayAutostart] = useState(false);
  const [hideTrayBackground, setHideTrayBackground] = useState(false);
  // 初始状态加载完成前禁用交互，避免异步结果覆盖用户操作
  const [initializing, setInitializing] = useState(true);
  const { save } = useConfigStore();

  // 加载初始状态
  useEffect(() => {
    const unsub = onThemeChange(setDarkState);
    (async () => {
      try {
        setAutostart(await api.isAutostartEnabled());
      } catch {
        /* 忽略 */
      }
      try {
        setVersion(await api.getAppVersion());
      } catch {
        /* 忽略 */
      }
      try {
        setVntVersion(await api.getVntVersion());
      } catch {
        setVntVersion('未知');
      }
      try {
        setShortcut(await isRegistered('Ctrl+Shift+V'));
      } catch {
        /* 忽略 */
      }
      try {
        const raw = localStorage.getItem(ALERT_KEY);
        if (raw) {
          const cfg = JSON.parse(raw) as {
            enabled: boolean;
            up: number;
            down: number;
          };
          setAlertEnabled(cfg.enabled);
          setAlertUp(cfg.up);
          setAlertDown(cfg.down);
        }
      } catch {
        /* 忽略 */
      }
      try {
        const s = await api.getSettings();
        setHideTrayAutostart(s.hide_tray_on_autostart);
        setHideTrayBackground(s.hide_tray_on_background);
      } catch {
        /* 忽略 */
      }
      setInitializing(false);
    })();
    return unsub;
  }, []);

  // 托盘可见性设置变更时自动持久化（消除闭包旧值问题）
  useEffect(() => {
    if (initializing) return;
    void api
      .saveSettings({
        hide_tray_on_autostart: hideTrayAutostart,
        hide_tray_on_background: hideTrayBackground,
      })
      .catch((e) => message.error(`保存设置失败: ${String(e)}`));
  }, [hideTrayAutostart, hideTrayBackground, initializing]);

  const handleAutostart = async (v: boolean) => {
    try {
      await api.setAutostart(v);
      setAutostart(v);
      message.success(v ? '已开启开机自启' : '已关闭开机自启');
    } catch (e) {
      message.error(String(e));
    }
  };

  const handleTheme = (v: boolean) => {
    setDark(v);
  };

  const handleShortcut = async (v: boolean) => {
    try {
      if (v) {
        await register('Ctrl+Shift+V', () => {
          const win = getCurrentWindow();
          void win.isVisible().then((visible) => {
            if (visible) void win.hide();
            else {
              void win.show();
              void win.setFocus();
            }
          });
        });
        setShortcut(true);
        message.success('快捷键已注册：Ctrl+Shift+V（显示/隐藏窗口）');
      } else {
        await unregister('Ctrl+Shift+V');
        setShortcut(false);
        message.success('快捷键已注销');
      }
    } catch (e) {
      message.error(String(e));
    }
  };

  const saveAlert = () => {
    const cfg = { enabled: alertEnabled, up: alertUp, down: alertDown };
    localStorage.setItem(ALERT_KEY, JSON.stringify(cfg));
    message.success('告警阈值已保存');
  };

  const exportConfigs = async () => {
    try {
      const path = await saveDialog({
        title: '导出配置',
        defaultPath: 'vnt-gui-configs.json',
        filters: [{ name: 'JSON', extensions: ['json'] }],
      });
      if (!path) return;
      // 后端直接写文件（不受 fs 插件路径 scope 限制）
      await api.exportConfigs(path);
      message.success(`配置已导出：${path}`);
    } catch (e) {
      message.error(`导出失败: ${String(e)}`);
    }
  };

  const importConfigs = async () => {
    try {
      const path = await openDialog({
        title: '导入配置',
        filters: [{ name: 'JSON', extensions: ['json'] }],
      });
      if (!path || Array.isArray(path)) return;
      // 后端读取解析（不受 fs 插件路径 scope 限制）
      const imported = await api.importConfigs(path);
      if (!imported.length) {
        message.warning('文件中没有配置');
        return;
      }
      for (const cfg of imported) {
        await save({ ...cfg, id: '' }); // 重新生成 id，避免覆盖现有配置
      }
      message.success(`已导入 ${imported.length} 条配置`);
    } catch (e) {
      message.error(`导入失败: ${String(e)}`);
    }
  };

  return (
    <div style={{ maxWidth: 720 }}>
      <Card title="常规">
        <Space direction="vertical" size="middle" style={{ width: '100%' }}>
          <Row align="middle" justify="space-between">
            <Col>
              <Typography.Text strong>开机自启</Typography.Text>
              <div>
                <Typography.Text type="secondary" style={{ fontSize: 12 }}>
                  登录 Windows 后静默启动（无 UAC 弹窗）
                </Typography.Text>
              </div>
            </Col>
            <Col>
              <Switch checked={autostart} disabled={initializing} onChange={handleAutostart} />
            </Col>
          </Row>
          <Divider style={{ margin: '4px 0' }} />
          <Row align="middle" justify="space-between">
            <Col>
              <Typography.Text strong>深色模式</Typography.Text>
            </Col>
            <Col>
              <Space>
                {dark ? <Moon size={14} /> : <Sun size={14} />}
                <Switch checked={dark} disabled={initializing} onChange={handleTheme} />
              </Space>
            </Col>
          </Row>
          <Divider style={{ margin: '4px 0' }} />
          <Row align="middle" justify="space-between">
            <Col>
              <Typography.Text strong>全局快捷键</Typography.Text>
              <div>
                <Typography.Text type="secondary" style={{ fontSize: 12 }}>
                  Ctrl+Shift+V 显示/隐藏主窗口
                </Typography.Text>
              </div>
            </Col>
            <Col>
              <Switch checked={shortcut} disabled={initializing} onChange={handleShortcut} />
            </Col>
          </Row>
          <Divider style={{ margin: '4px 0' }} />
          <Row align="middle" justify="space-between">
            <Col>
              <Typography.Text strong>开机自启时不显示托盘</Typography.Text>
              <div>
                <Typography.Text type="secondary" style={{ fontSize: 12 }}>
                  通过开机自启启动时静默后台运行（不显示托盘图标）
                </Typography.Text>
              </div>
            </Col>
            <Col>
              <Switch
                checked={hideTrayAutostart}
                disabled={initializing}
                onChange={setHideTrayAutostart}
              />
            </Col>
          </Row>
          <Divider style={{ margin: '4px 0' }} />
          <Row align="middle" justify="space-between">
            <Col>
              <Typography.Text strong>后台运行时隐藏托盘</Typography.Text>
              <div>
                <Typography.Text type="secondary" style={{ fontSize: 12 }}>
                  关闭窗口进入后台时隐藏托盘图标（无托盘入口，需再次启动或快捷键唤出）
                </Typography.Text>
              </div>
            </Col>
            <Col>
              <Switch
                checked={hideTrayBackground}
                disabled={initializing}
                onChange={setHideTrayBackground}
              />
            </Col>
          </Row>
        </Space>
      </Card>

      <Card title="流量告警" style={{ marginTop: 16 }}>
        <Space direction="vertical" size="middle" style={{ width: '100%' }}>
          <Row align="middle" justify="space-between">
            <Col>
              <Typography.Text strong>启用告警</Typography.Text>
              <div>
                <Typography.Text type="secondary" style={{ fontSize: 12 }}>
                  速率超过阈值时弹出系统通知
                </Typography.Text>
              </div>
            </Col>
            <Col>
              <Switch
                disabled={initializing}
                checked={alertEnabled}
                onChange={(v) => {
                  // 直接用参数 v 构造持久化（避免闭包旧 state 导致保存相反值）
                  setAlertEnabled(v);
                  const cfg = { enabled: v, up: alertUp, down: alertDown };
                  localStorage.setItem(ALERT_KEY, JSON.stringify(cfg));
                  message.success(v ? '流量告警已启用' : '流量告警已关闭');
                }}
              />
            </Col>
          </Row>
          {alertEnabled && (
            <>
              <Divider style={{ margin: '4px 0' }} />
              <Row gutter={16}>
                <Col span={10}>
                  <Typography.Text>上传阈值 (MB/s)</Typography.Text>
                  <InputNumber
                    min={0.1}
                    step={0.1}
                    value={alertUp}
                    onChange={(v) => setAlertUp(v ?? 10)}
                    style={{ width: '100%' }}
                  />
                </Col>
                <Col span={10}>
                  <Typography.Text>下载阈值 (MB/s)</Typography.Text>
                  <InputNumber
                    min={0.1}
                    step={0.1}
                    value={alertDown}
                    onChange={(v) => setAlertDown(v ?? 10)}
                    style={{ width: '100%' }}
                  />
                </Col>
                <Col span={4} style={{ display: 'flex', alignItems: 'flex-end' }}>
                  <Button type="primary" onClick={saveAlert}>
                    保存
                  </Button>
                </Col>
              </Row>
            </>
          )}
        </Space>
      </Card>

      <Card title="配置管理" style={{ marginTop: 16 }}>
        <Space>
          <Button icon={<Download size={14} />} onClick={exportConfigs}>
            导出全部配置
          </Button>
          <Button icon={<Upload size={14} />} onClick={importConfigs}>
            导入配置
          </Button>
        </Space>
      </Card>

      <Card title="关于" style={{ marginTop: 16 }}>
        <Row gutter={16}>
          <Col span={12}>
            <Typography.Text type="secondary">VNT GUI 版本</Typography.Text>
            <div>
              <Typography.Text strong>{version || '-'}</Typography.Text>
            </div>
          </Col>
          <Col span={12}>
            <Typography.Text type="secondary">vnt-cli 版本</Typography.Text>
            <div>
              <Typography.Text strong>{vntVersion || '-'}</Typography.Text>
            </div>
          </Col>
        </Row>
      </Card>
    </div>
  );
}
