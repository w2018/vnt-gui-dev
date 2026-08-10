// F9 连接日志：实时显示客户端连接记录（IP、用户、操作、时间）

import { useEffect } from 'react';
import { Card, Table, Tag, Typography } from 'antd';
import { useFtpStore } from '../../stores/useFtpStore';
import type { FtpLogEntry } from '../../types/ftp';

const ACTION_COLOR: Record<string, string> = {
  登录成功: 'green',
  登录失败: 'red',
  上传: 'purple',
  下载: 'blue',
  删除: 'red',
  删除目录: 'red',
  新建目录: 'cyan',
  重命名: 'orange',
};

export function FtpConnectionLogs() {
  const { logs, refreshLogs } = useFtpStore();

  // 3 秒轮询（与服务运行同步刷新）
  useEffect(() => {
    void refreshLogs();
    const timer = window.setInterval(() => void refreshLogs(), 3000);
    return () => window.clearInterval(timer);
  }, [refreshLogs]);

  return (
    <Card
      title="连接日志"
      extra={
        <Typography.Link onClick={() => void refreshLogs()}>刷新</Typography.Link>
      }
    >
      <Table<FtpLogEntry>
        rowKey={(r) => `${r.time}-${r.ip}-${r.user}-${r.action}-${r.detail}`}
        dataSource={logs}
        size="small"
        pagination={{ pageSize: 10, showSizeChanger: false }}
        locale={{ emptyText: '暂无连接记录' }}
        columns={[
          {
            title: '时间',
            dataIndex: 'time',
            width: 80,
          },
          {
            title: 'IP',
            dataIndex: 'ip',
            width: 130,
          },
          {
            title: '用户',
            dataIndex: 'user',
            width: 110,
          },
          {
            title: '操作',
            dataIndex: 'action',
            width: 100,
            render: (v: string) => <Tag color={ACTION_COLOR[v] ?? 'default'}>{v}</Tag>,
          },
          {
            title: '详情',
            dataIndex: 'detail',
            ellipsis: true,
          },
        ]}
      />
    </Card>
  );
}
