// 流量图表（文档 §4.2.5）：Recharts 折线图 + 统计卡片

import { Card, Col, Row, Statistic, Empty } from 'antd';
import {
  Line,
  LineChart,
  ResponsiveContainer,
  Tooltip,
  XAxis,
  YAxis,
  CartesianGrid,
} from 'recharts';
import { useTrafficStore } from '../stores/trafficStore';

export function TrafficChart() {
  const { current, points, totalUpload, totalDownload } = useTrafficStore();

  return (
    <Card title="流量统计">
      <Row gutter={16} style={{ marginBottom: 16 }}>
        <Col span={6}>
          <Statistic
            title="上传速率"
            value={formatSpeed(current?.upload_speed ?? 0)}
          />
        </Col>
        <Col span={6}>
          <Statistic
            title="下载速率"
            value={formatSpeed(current?.download_speed ?? 0)}
          />
        </Col>
        <Col span={6}>
          <Statistic title="累计上传" value={formatBytes(totalUpload)} />
        </Col>
        <Col span={6}>
          <Statistic title="累计下载" value={formatBytes(totalDownload)} />
        </Col>
      </Row>

      {points.length < 2 ? (
        <Empty description="暂无流量数据（连接后每秒刷新）" style={{ padding: '48px 0' }} />
      ) : (
        <ResponsiveContainer width="100%" height={360}>
          <LineChart data={points} margin={{ top: 8, right: 16, bottom: 8, left: 8 }}>
            <CartesianGrid strokeDasharray="3 3" stroke="#eee" />
            <XAxis dataKey="time" tick={{ fontSize: 12 }} />
            <YAxis tickFormatter={(v: number) => formatSpeed(v)} tick={{ fontSize: 12 }} />
            <Tooltip formatter={(v: number | string) => formatSpeed(Number(v))} />
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
