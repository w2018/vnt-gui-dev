// F5/F6 用户编辑弹窗：用户名、密码、权限勾选（只读优先）

import { useEffect, useState } from 'react';
import { Checkbox, Form, Input, Modal, Typography } from 'antd';
import { defaultFtpPermissions } from '../../types/ftp';
import type { FtpPermissions, FtpUser } from '../../types/ftp';

interface Props {
  open: boolean;
  /** null = 新增；非空 = 编辑 */
  user: FtpUser | null;
  existingNames: string[];
  onCancel: () => void;
  onOk: (user: FtpUser) => void;
}

export function FtpUserDialog({ open, user, existingNames, onCancel, onOk }: Props) {
  const [username, setUsername] = useState('');
  const [password, setPassword] = useState('');
  const [perms, setPerms] = useState<FtpPermissions>(defaultFtpPermissions());
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (open) {
      setUsername(user?.username ?? '');
      setPassword('');
      setPerms(user?.permissions ?? defaultFtpPermissions());
      setError(null);
    }
  }, [open, user]);

  const toggle = (key: keyof FtpPermissions, v: boolean) => {
    setPerms((prev) => {
      const next = { ...prev, [key]: v };
      if (key === 'readonly' && v) {
        // 只读 = 强制禁止上传 + 删除
        next.upload = false;
        next.delete = false;
      }
      return next;
    });
  };

  const handleOk = () => {
    const name = username.trim();
    if (!name) {
      setError('用户名不能为空');
      return;
    }
    if (!user && existingNames.includes(name)) {
      setError('用户名已存在');
      return;
    }
    if (!user && !password) {
      setError('新增用户必须设置密码');
      return;
    }
    if (user && password && password.length < 4) {
      setError('密码至少 4 位');
      return;
    }
    const permsFinal = { ...perms };
    if (permsFinal.readonly) {
      permsFinal.upload = false;
      permsFinal.delete = false;
    }
    if (user && !password) {
      // 🆕 编辑模式且密码为空 = 不改密码，传空 password（后端保留 keyring 旧密码；
      //   若 keyring 也没有 → 后端明确报错"凭据库中不存在"，绝不静默）
      onOk({
        username: name,
        password: '',
        permissions: permsFinal,
      });
      return;
    }
    onOk({
      username: name,
      password,
      permissions: permsFinal,
    });
  };

  return (
    <Modal
      title={user ? `编辑用户 ${user.username}` : '添加用户'}
      open={open}
      onCancel={onCancel}
      onOk={handleOk}
      okText="保存"
      cancelText="取消"
      destroyOnClose
    >
      <Form layout="vertical" style={{ marginTop: 8 }}>
        <Form.Item label="用户名" required>
          <Input
            value={username}
            disabled={user != null}
            placeholder="登录用户名"
            onChange={(e) => setUsername(e.target.value)}
          />
        </Form.Item>
        <Form.Item label={user ? '新密码（留空 = 不修改）' : '密码'} required={!user}>
          <Input.Password
            value={password}
            placeholder={user ? '留空保持原密码' : '设置密码'}
            onChange={(e) => setPassword(e.target.value)}
          />
        </Form.Item>
        <Form.Item label="权限">
          <div style={{ display: 'flex', gap: 24, flexWrap: 'wrap' }}>
            <Checkbox
              checked={perms.upload}
              disabled={perms.readonly}
              onChange={(e) => toggle('upload', e.target.checked)}
            >
              上传
            </Checkbox>
            <Checkbox checked={perms.download} onChange={(e) => toggle('download', e.target.checked)}>
              下载
            </Checkbox>
            <Checkbox
              checked={perms.delete}
              disabled={perms.readonly}
              onChange={(e) => toggle('delete', e.target.checked)}
            >
              删除
            </Checkbox>
            <Checkbox checked={perms.readonly} onChange={(e) => toggle('readonly', e.target.checked)}>
              只读
            </Checkbox>
          </div>
          {perms.readonly && (
            <Typography.Text type="secondary" style={{ fontSize: 12 }}>
              只读模式：禁止上传与删除（仅可下载/浏览）
            </Typography.Text>
          )}
        </Form.Item>
        {error && <Typography.Text type="danger">{error}</Typography.Text>}
      </Form>
    </Modal>
  );
}
