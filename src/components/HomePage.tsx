// 首页：软件介绍 + 连接状态 + FTP 服务状态（实时）

import { useEffect, useState } from 'react';
import { Card, Col, Row, Space, Tag, Typography } from 'antd';
import { Activity, Network, RefreshCw, Server, ShieldCheck, Wifi } from 'lucide-react';
import { api } from '../lib/tauri';
import { useConnectionStore } from '../stores/connectionStore';
import type { FtpServerStatus } from '../types/ftp';

const FEATURES = [
  { icon: <Network size={18} />, title: '一键组网', desc: '基于 vnt-cli 的虚拟局域网，配置即连' },
  { icon: <ShieldCheck size={18} />, title: '断线自动重连', desc: '指数退避策略，最多重试 10 次' },
  { icon: <Activity size={18} />, title: '实时监控', desc: '流量统计与在线设备一览无余' },
  { icon: <RefreshCw size={18} />, title: '自动更新', desc: 'vnt-cli 与 GUI 版本检测，一键升级' },
];

const CONN_STATE_MAP: Record<string, { text: string; color: string }> = {
  stopped: { text: '未连接', color: 'default' },
  starting: { text: '连接中…', color: 'processing' },
  connected: { text: '已连接', color: 'success' },
  reconnecting: { text: '重连中…', color: 'warning' },
  error: { text: '连接异常', color: 'error' },
};

/** 延迟分级着色：<50ms 绿、<150ms 橙、其余红；未测出灰 */
function latencyColor(ms: number | null): string {
  if (ms == null) return '#8c8c8c';
  if (ms < 50) return '#22c55e';
  if (ms < 150) return '#faad14';
  return '#ff4d4f';
}

/** 单行省略文本（防长文本溢出遮挡相邻列） */
function EllipsisText({ text, fontSize = 13 }: { text: string; fontSize?: number }) {
  return (
    <div
      title={text}
      style={{
        fontSize,
        overflow: 'hidden',
        textOverflow: 'ellipsis',
        whiteSpace: 'nowrap',
        lineHeight: '24px',
        color: '#262626',
      }}
    >
      {text}
    </div>
  );
}

export function HomePage() {
  const [version, setVersion] = useState('');
  const conn = useConnectionStore();
  const [ftp, setFtp] = useState<FtpServerStatus>({ state: 'stopped', listen_addr: null, error: null });

  useEffect(() => {
    void api.getAppVersion().then(setVersion).catch(() => {});
  }, []);

  // 🆕 连接状态及时刷新（3s 轮询 getStatus → store，事件驱动 + 轮询双保险）
  useEffect(() => {
    let alive = true;
    const load = async () => {
      try {
        const s = await api.getStatus();
        if (alive) useConnectionStore.getState().setStatus(s.status);
      } catch {
        /* 忽略瞬时错误 */
      }
    };
    void load();
    const timer = window.setInterval(load, 3000);
    return () => {
      alive = false;
      window.clearInterval(timer);
    };
  }, []);

  // FTP 服务状态轮询（5s）
  useEffect(() => {
    let alive = true;
    const load = async () => {
      try {
        const s = await api.ftpStatus();
        if (alive) setFtp(s);
      } catch {
        /* 忽略 */
      }
    };
    void load();
    const timer = window.setInterval(load, 5000);
    return () => {
      alive = false;
      window.clearInterval(timer);
    };
  }, []);

  const connState = CONN_STATE_MAP[conn.status] ?? CONN_STATE_MAP.stopped;
  const ftpState =
    ftp.state === 'running'
      ? { text: '运行中', color: 'success' }
      : ftp.state === 'error'
        ? { text: '异常', color: 'error' }
        : { text: '已停止', color: 'default' };

  return (
    <div style={{ maxWidth: 980, margin: '0 auto' }}>
      {/* 介绍 + 版本（含 logo） */}
      <Card style={{ textAlign: 'center', marginBottom: 16 }}>
        <Space direction="vertical" size="small" style={{ width: '100%' }}>
          <img
            src="/vnt-logo.png"
            alt="VNT GUI"
            style={{ width: 96, height: 96, margin: '0 auto' }}
          />
          <div>
            <Typography.Title level={3} style={{ margin: 0 }}>
              VNT GUI
            </Typography.Title>
            <Typography.Text type="secondary">vnt 虚拟局域网图形化管理工具</Typography.Text>
            <div style={{ marginTop: 4 }}>
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
              marginTop: 8,
            }}
          >
            {FEATURES.map((f) => (
              <div
                key={f.title}
                style={{
                  border: '1px solid #e5e7eb',
                  borderRadius: 8,
                  padding: '10px 14px',
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

      {/* 状态总览：连接状态 + FTP 服务 */}
      <Row gutter={16}>
        <Col span={12}>
          <Card
            title={
              <Space>
                <Wifi size={15} />
                连接状态
                <Tag color={connState.color}>{connState.text}</Tag>
              </Space>
            }
            size="small"
          >
            <Row gutter={16} align="middle">
              <Col span={10}>
                <div style={{ fontSize: 12, color: '#8c8c8c', marginBottom: 2 }}>虚拟 IP</div>
                <div style={{ fontSize: 16, fontWeight: 500, lineHeight: '24px', color: '#262626' }}>
                  {conn.virtualIp ?? '-'}
                </div>
              </Col>
              <Col span={8}>
                <div style={{ fontSize: 12, color: '#8c8c8c', marginBottom: 2 }}>服务器</div>
                {/* 单行省略：长地址不遮挡延迟列；字号与虚拟 IP/延迟统一 16px */}
                <EllipsisText text={conn.serverAddress ?? '-'} fontSize={16} />
              </Col>
              <Col span={6}>
                <div style={{ fontSize: 12, color: '#8c8c8c', marginBottom: 2 }}>延迟</div>
                <div style={{ fontSize: 16, fontWeight: 500, lineHeight: '24px', color: latencyColor(conn.latency) }}>
                  {conn.latency == null ? '-' : `${conn.latency} ms`}
                </div>
              </Col>
            </Row>
            {conn.status === 'error' && conn.errorMessage && (
              <Typography.Text type="danger" style={{ fontSize: 12 }}>
                错误：{conn.errorMessage}
              </Typography.Text>
            )}
          </Card>
        </Col>
        <Col span={12}>
          <Card
            title={
              <Space>
                <Server size={15} />
                FTP 服务
                <Tag color={ftpState.color}>{ftpState.text}</Tag>
              </Space>
            }
            size="small"
          >
            <div style={{ fontSize: 12, color: '#8c8c8c', marginBottom: 2 }}>监听地址</div>
            <EllipsisText text={ftp.listen_addr ?? '未监听'} fontSize={16} />
            {ftp.state === 'error' && ftp.error && (
              <Typography.Text type="danger" style={{ fontSize: 12 }}>
                错误：{ftp.error}
              </Typography.Text>
            )}
          </Card>
        </Col>
      </Row>
    </div>
  );
}
