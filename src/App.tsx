// 应用主布局：侧边栏 + 主内容区（文档 §4.1）

import { useEffect, useState } from 'react';
import { Layout, Menu, Typography, message } from 'antd';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import { sendNotification } from '@tauri-apps/plugin-notification';
import {
  Activity,
  Database,
  FileText,
  Home,
  PlugZap,
  RefreshCw,
  Settings,
} from 'lucide-react';
import { initTauriListeners } from './lib/tauri';
import { HomePage } from './components/HomePage';
import { StatusPanel } from './components/StatusPanel';
import { ConfigForm } from './components/ConfigForm';
import { ConfigHistory } from './components/ConfigHistory';
import { LogViewer } from './components/LogViewer';
import { TrafficChart } from './components/TrafficChart';
import { DeviceList } from './components/DeviceList';
import { UpdateDialog } from './components/UpdateDialog';
import { FirstRunWizard } from './components/FirstRunWizard';
import { SettingsPanel } from './components/SettingsPanel';
import { useConfigStore } from './stores/configStore';
import { useConnectionStore } from './stores/connectionStore';
import { useTrafficStore } from './stores/trafficStore';
import { api } from './lib/tauri';
import type { VntConfig } from './lib/types';

type PageKey = 'home' | 'connect' | 'config' | 'traffic' | 'log' | 'settings' | 'update';

const menuItems = [
  { key: 'home', icon: <Home size={16} />, label: '首页' },
  { key: 'connect', icon: <PlugZap size={16} />, label: '连接' },
  { key: 'config', icon: <Database size={16} />, label: '配置' },
  { key: 'traffic', icon: <Activity size={16} />, label: '流量' },
  { key: 'log', icon: <FileText size={16} />, label: '日志' },
  { key: 'settings', icon: <Settings size={16} />, label: '设置' },
  { key: 'update', icon: <RefreshCw size={16} />, label: '更新' },
];

// 流量告警节流（每分钟最多通知一次）
let lastAlertAt = 0;

export default function App() {
  const [page, setPage] = useState<PageKey>('home');
  const [editing, setEditing] = useState<VntConfig | null>(null);
  const refreshConfigs = useConfigStore((s) => s.refresh);
  const setStatus = useConnectionStore((s) => s.setStatus);

  useEffect(() => {
    let unlisteners: UnlistenFn[] = [];
    (async () => {
      try {
        unlisteners = await initTauriListeners();
        // 托盘菜单跳转
        unlisteners.push(
          await listen<string>('navigate', (event) => {
            const route = event.payload;
            if (route === '/settings') setPage('settings');
            else if (route === '/traffic') setPage('traffic');
            else setPage('home');
          }),
        );
        await refreshConfigs();
        const status = await api.getStatus();
        setStatus(status.status);
      } catch (e) {
        message.error(`初始化失败: ${String(e)}`);
      }
    })();
    return () => unlisteners.forEach((fn) => fn());
  }, [refreshConfigs, setStatus]);

  // 连接状态兜底轮询：每 3 秒检测一次（事件即时更新，轮询保证可靠）
  useEffect(() => {
    const timer = window.setInterval(async () => {
      try {
        const status = await api.getStatus();
        const current = useConnectionStore.getState().status;
        if (status.status !== current) {
          useConnectionStore.getState().setStatus(status.status);
        }
      } catch {
        /* 忽略瞬时错误 */
      }
    }, 3000);
    return () => window.clearInterval(timer);
  }, []);

  // 流量告警（订阅 store，阈值存 localStorage，由设置页配置）
  useEffect(() => {
    return useTrafficStore.subscribe((state) => {
      const snap = state.current;
      if (!snap) return;
      let cfg: { enabled: boolean; up: number; down: number } | null = null;
      try {
        const raw = localStorage.getItem('vnt-alert-threshold');
        if (raw) cfg = JSON.parse(raw) as { enabled: boolean; up: number; down: number };
      } catch {
        return;
      }
      if (!cfg?.enabled) return;
      const up = snap.upload_speed / 1024 / 1024;
      const down = snap.download_speed / 1024 / 1024;
      if (up > cfg.up || down > cfg.down) {
        const now = Date.now();
        if (now - lastAlertAt < 60_000) return;
        lastAlertAt = now;
        void sendNotification({
          title: 'VNT GUI 流量告警',
          body: `上传 ${up.toFixed(1)} MB/s，下载 ${down.toFixed(1)} MB/s`,
        });
      }
    });
  }, []);

  return (
    <Layout style={{ minHeight: '100vh' }}>
      <Layout.Sider theme="light" width={200}>
        <div style={{ padding: '16px', textAlign: 'center' }}>
          <Typography.Title level={4} style={{ margin: 0 }}>
            VNT GUI
          </Typography.Title>
        </div>
        <Menu
          mode="inline"
          selectedKeys={[page]}
          items={menuItems}
          onClick={(e) => setPage(e.key as PageKey)}
          style={{ borderInlineEnd: 'none' }}
        />
      </Layout.Sider>
      <Layout.Content style={{ padding: 24, overflow: 'auto' }}>
        {page === 'home' && <HomePage />}
        {page === 'connect' && (
          <div style={{ display: 'grid', gridTemplateColumns: '1fr', gap: 16, maxWidth: 860 }}>
            <StatusPanel />
            <DeviceList />
          </div>
        )}
        {page === 'config' && (
          <div style={{ display: 'grid', gridTemplateColumns: '1fr', gap: 16 }}>
            <ConfigHistory onEdit={setEditing} />
            <ConfigForm config={editing ?? undefined} key={editing?.id ?? 'new'} />
          </div>
        )}
        {page === 'traffic' && <TrafficChart />}
        {page === 'log' && <LogViewer />}
        {page === 'settings' && <SettingsPanel />}
        {page === 'update' && <UpdateDialog />}
        <FirstRunWizard />
      </Layout.Content>
    </Layout>
  );
}
