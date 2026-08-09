// 流量图表（文档 §4.2.5）：分时间段统计卡（今日/昨日/本月/累计）+ Recharts 折线图

import { useEffect, useState } from 'react';
import { Card, Col, Row, Statistic, Empty, Tooltip } from 'antd';
import {
  Line,
  LineChart,
  ResponsiveContainer,
  Tooltip as RechartsTooltip,
  XAxis,
  YAxis,
  CartesianGrid,
} from 'recharts';
import { useTrafficStore } from '../stores/trafficStore';
import { api } from '../lib/tauri';
import type { PeriodTraffic } from '../lib/types';

export function TrafficChart() {
  const { current, points, totalUpload, totalDownload } = useTrafficStore();
  // 分时间段统计（每 10 秒刷新）
  const [period, setPeriod] = useState<PeriodTraffic | null>(null);

  useEffect(() => {
    let cancelled = false;
    const tick = async () => {
      try {
        const p = await api.getTrafficPeriod();
        if (!cancelled) setPeriod(p);
      } catch {
        /* 忽略 */
      }
    };
    void tick();
    const timer = window.setInterval(tick, 10_000);
    return () => {
      cancelled = true;
      window.clearInterval(timer);
    };
  }, []);

  return (
    <Card title="流量统计">
      {/* 实时速率 + 会话累计 */}
      <Row gutter={16} style={{ marginBottom: 8 }}>
        <Col span={6}>
          <Statistic title="上传速率" value={formatSpeed(current?.upload_speed ?? 0)} />
        </Col>
        <Col span={6}>
          <Statistic title="下载速率" value={formatSpeed(current?.download_speed ?? 0)} />
        </Col>
        <Col span={6}>
          <Statistic title="会话上传" value={formatBytes(totalUpload)} />
        </Col>
        <Col span={6}>
          <Statistic title="会话下载" value={formatBytes(totalDownload)} />
        </Col>
      </Row>

      {/* 分时间段统计：今日 / 昨日 / 本月 / 累计 */}
      <Row gutter={16} style={{ marginBottom: 16 }}>
        {[
          { key: 'today', title: '今日流量' },
          { key: 'yesterday', title: '昨日流量' },
          { key: 'month', title: '本月流量' },
          { key: 'total', title: '累计流量' },
        ].map(({ key, title }) => {
          const d = period?.[key as keyof PeriodTraffic];
          return (
            <Col span={6} key={key}>
              <Card size="small" style={{ background: 'rgba(0,0,0,0.02)' }}>
                <Statistic
                  title={title}
                  value={formatBytes(d?.sent ?? 0)}
                  prefix={<Tooltip title="上传">↑</Tooltip>}
                />
                <Statistic
                  title=" "
                  value={formatBytes(d?.recv ?? 0)}
                  prefix={<Tooltip title="下载">↓</Tooltip>}
                  valueStyle={{ color: '#22c55e' }}
                />
                <Statistic
                  title="合计"
                  value={formatBytes(d ? d.sent + d.recv : 0)}
                  valueStyle={{ fontWeight: 600 }}
                />
              </Card>
            </Col>
          );
        })}
      </Row>

      {points.length < 2 ? (
        <Empty description="暂无流量数据（连接后每秒刷新）" style={{ padding: '48px 0' }} />
      ) : (
        <ResponsiveContainer width="100%" height={360}>
          <LineChart data={points} margin={{ top: 8, right: 16, bottom: 8, left: 8 }}>
            <CartesianGrid strokeDasharray="3 3" stroke="#eee" />
            <XAxis dataKey="time" tick={{ fontSize: 12 }} />
            <YAxis tickFormatter={(v: number) => formatSpeed(v)} tick={{ fontSize: 12 }} />
            <RechartsTooltip formatter={(v: number | string) => formatSpeed(Number(v))} />
            <Line
              type="monotone"
              dataKey="upload"
              stroke="#3b82f6"
              name="上传"
              dot={false}
              strokeWidth={2}
            />
            <Line
              type="monotone"
              dataKey="download"
              stroke="#22c55e"
              name="下载"
              dot={false}
              strokeWidth={2}
            />
          </LineChart>
        </ResponsiveContainer>
      )}
    </Card>
  );
}

function formatSpeed(bytesPerSec: number): string {
  if (bytesPerSec >= 1024 * 1024) return `${(bytesPerSec / 1024 / 1024).toFixed(2)} MB/s`;
  if (bytesPerSec >= 1024) return `${(bytesPerSec / 1024).toFixed(1)} KB/s`;
  return `${bytesPerSec.toFixed(0)} B/s`;
}

function formatBytes(bytes: number): string {
  if (bytes >= 1024 * 1024 * 1024) return `${(bytes / 1024 / 1024 / 1024).toFixed(2)} GB`;
  if (bytes >= 1024 * 1024) return `${(bytes / 1024 / 1024).toFixed(2)} MB`;
  if (bytes >= 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  return `${bytes} B`;
}
