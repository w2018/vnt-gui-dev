// F5/F6 用户管理：表格 + 添加/编辑/删除（权限独立设置）

import { useState } from 'react';
import { Button, Card, Space, Table, Tag, Typography, Popconfirm, message } from 'antd';
import { Plus, Pencil, Trash2 } from 'lucide-react';
import { useFtpStore } from '../../stores/useFtpStore';
import type { FtpUser } from '../../types/ftp';
import { FtpUserDialog } from './FtpUserDialog';

export function FtpUserManager() {
  const { config, addUser, removeUser, updateUser } = useFtpStore();
  const [editing, setEditing] = useState<FtpUser | null>(null);
  const [dialogOpen, setDialogOpen] = useState(false);

  const permissionTags = (u: FtpUser) => {
    const p = u.permissions;
    const tags: { text: string; color: string }[] = [];
    if (p.readonly) {
      tags.push({ text: '只读', color: 'blue' });
      tags.push({ text: '下载', color: 'green' });
    } else {
      if (p.upload) tags.push({ text: '上传', color: 'purple' });
      if (p.download) tags.push({ text: '下载', color: 'green' });
      if (p.delete) tags.push({ text: '删除', color: 'red' });
    }
    if (tags.length === 0) tags.push({ text: '无权限', color: 'default' });
    return tags.map((t) => (
      <Tag key={t.text} color={t.color}>
        {t.text}
      </Tag>
    ));
  };

  const handleSaveUser = async (user: FtpUser, isEdit: boolean) => {
    try {
      if (isEdit) {
        await updateUser(user.username, user);
        message.success('用户已更新');
      } else {
        await addUser(user);
        message.success('用户已添加');
      }
      setDialogOpen(false);
      setEditing(null);
    } catch (e) {
      message.error(String(e));
    }
  };

  return (
    <Card
      title="用户管理"
      extra={
        <Button
          type="primary"
          icon={<Plus size={14} />}
          onClick={() => {
            setEditing(null);
            setDialogOpen(true);
          }}
        >
          添加用户
        </Button>
      }
    >
      <Table<FtpUser>
        rowKey="username"
        dataSource={config.users}
        pagination={false}
        locale={{ emptyText: '暂无用户，点击右上角添加' }}
        columns={[
          {
            title: '用户',
            dataIndex: 'username',
            render: (v: string) => <Typography.Text strong>{v}</Typography.Text>,
          },
          {
            title: '权限',
            dataIndex: 'permissions',
            render: (_: unknown, record: FtpUser) => <Space size={4}>{permissionTags(record)}</Space>,
          },
          {
            title: '操作',
            width: 140,
            render: (_: unknown, record: FtpUser) => (
              <Space>
                <Button
                  size="small"
                  icon={<Pencil size={13} />}
                  onClick={() => {
                    setEditing(record);
                    setDialogOpen(true);
                  }}
                >
                  编辑
                </Button>
                <Popconfirm
                  title="删除用户"
                  description={`确定删除用户 ${record.username}？`}
                  okText="删除"
                  okButtonProps={{ danger: true }}
                  onConfirm={() =>
                    removeUser(record.username).catch((e) => message.error(String(e)))
                  }
                >
                  <Button size="small" danger icon={<Trash2 size={13} />} />
                </Popconfirm>
              </Space>
            ),
          },
        ]}
      />
      <FtpUserDialog
        open={dialogOpen}
        user={editing}
        existingNames={config.users.map((u) => u.username)}
        onCancel={() => {
          setDialogOpen(false);
          setEditing(null);
        }}
        onOk={(u) => handleSaveUser(u, editing != null)}
      />
    </Card>
  );
}
