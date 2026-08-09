// 首页：软件介绍 + 图标（欢迎页，不含任何运行时连接数据）

import { useEffect, useState } from 'react';
import { Card, Space, Typography } from 'antd';
import { Activity, Network, RefreshCw, ShieldCheck } from 'lucide-react';
import { api } from '../lib/tauri';

const FEATURES = [
  { icon: <Network size={18} />, title: '一键组网', desc: '基于 vnt-cli 的虚拟局域网，配置即连' },
  { icon: <ShieldCheck size={18} />, title: '断线自动重连', desc: '指数退避策略，最多重试 10 次' },
  { icon: <Activity size={18} />, title: '实时监控', desc: '流量统计与在线设备一览无余' },
  { icon: <RefreshCw size={18} />, title: '自动更新', desc: 'vnt-cli 与 GUI 版本检测，一键升级' },
];

export function HomePage() {
  const [version, setVersion] = useState('');

  useEffect(() => {
    void api.getAppVersion().then(setVersion).catch(() => {});
  }, []);

  return (
    <div
      style={{
        display: 'flex',
        justifyContent: 'center',
        alignItems: 'center',
        minHeight: 'calc(100vh - 48px)',
      }}
    >
      <Card style={{ width: 560, textAlign: 'center', border: 'none', boxShadow: 'none' }}>
        <Space direction="vertical" size="middle" style={{ width: '100%' }}>
          <img
            src="/vnt-logo.png"
            alt="VNT GUI"
            style={{ width: 128, height: 128, margin: '0 auto' }}
          />
          <div>
            <Typography.Title level={2} style={{ margin: 0 }}>
              VNT GUI
            </Typography.Title>
            <Typography.Text type="secondary">
              vnt 虚拟局域网图形化管理工具
            </Typography.Text>
            <div style={{ marginTop: 8 }}>
              <Typography.Text type="secondary" style={{ fontSize: 13 }}>
                当前版本 v{version || '-'}
              </Typography.Text>
            </div>
          </div>
          <div
            style={{
              display: 'grid',
              gridTemplateColumns: '1fr 1fr',
              gap: 12,
              marginTop: 16,
            }}
          >
            {FEATURES.map((f) => (
              <div
                key={f.title}
                style={{
                  border: '1px solid #e5e7eb',
                  borderRadius: 8,
                  padding: '12px 16px',
                  textAlign: 'left',
                }}
              >
                <Space>
                  {f.icon}
                  <Typography.Text strong>{f.title}</Typography.Text>
                </Space>
                <div>
                  <Typography.Text type="secondary" style={{ fontSize: 12 }}>
                    {f.desc}
                  </Typography.Text>
                </div>
              </div>
            ))}
          </div>
        </Space>
      </Card>
    </div>
  );
}
