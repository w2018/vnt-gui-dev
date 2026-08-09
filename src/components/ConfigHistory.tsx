// 历史配置列表（文档 §4.2.3）

import { useEffect } from 'react';
import {
  Button,
  Card,
  Empty,
  List,
  Popconfirm,
  Space,
  Tag,
  Tooltip,
  Typography,
  message,
} from 'antd';
import { Pencil, Plus, Trash2 } from 'lucide-react';
import { useConfigStore } from '../stores/configStore';
import { useConnectionStore } from '../stores/connectionStore';
import { api } from '../lib/tauri';
import { defaultConfig, type VntConfig } from '../lib/types';

export function ConfigHistory({ onEdit }: { onEdit: (c: VntConfig) => void }) {
  const { configs, activeConfigId, loading, refresh, remove, setActive } = useConfigStore();

  useEffect(() => {
    void refresh();
  }, [refresh]);

  const handleActivate = async (id: string) => {
    try {
      await setActive(id);
      // 若正在连接，按新配置重新连接
      const st = useConnectionStore.getState().status;
      if (st === 'connected' || st === 'starting' || st === 'reconnecting') {
        await api.stopConnection();
        await api.startConnection(id);
        message.success('已切换配置并按新配置重新连接');
      } else {
        message.success('已切换活动配置');
      }
    } catch (e) {
      message.error(`切换失败: ${String(e)}`);
    }
  };

  const handleDelete = async (id: string, name: string) => {
    try {
      await remove(id);
      message.success(`已删除配置「${name}」`);
    } catch (e) {
      message.error(`删除失败: ${String(e)}`);
    }
  };

  return (
    <Card
      title="配置列表"
      extra={
        <Button
          type="primary"
          size="small"
          icon={<Plus size={14} />}
          onClick={() => onEdit({ ...defaultConfig(), name: `配置 ${configs.length + 1}` })}
        >
          新建
        </Button>
      }
      styles={{ body: { maxHeight: 'calc(100vh - 140px)', overflow: 'auto' } }}
    >
      {configs.length === 0 && !loading ? (
        <Empty description="暂无配置，点击右上角新建" />
      ) : (
        <List
          loading={loading}
          dataSource={configs}
          renderItem={(cfg) => {
            const active = cfg.id === activeConfigId;
            return (
              <List.Item
                actions={[
                  <Tooltip key="activate" title={active ? '当前活动配置' : '设为活动配置'}>
                    <Button
                      type={active ? 'primary' : 'default'}
                      size="small"
                      disabled={active}
                      onClick={() => handleActivate(cfg.id)}
                    >
                      {active ? '使用中' : '激活'}
                    </Button>
                  </Tooltip>,
                  <Tooltip key="edit" title="编辑">
                    <Button
                      size="small"
                      icon={<Pencil size={13} />}
                      onClick={() => onEdit(cfg)}
                    />
                  </Tooltip>,
                  <Popconfirm
                    key="del"
                    title={`确定删除配置「${cfg.name}」？`}
                    onConfirm={() => handleDelete(cfg.id, cfg.name)}
                  >
                    <Button size="small" danger icon={<Trash2 size={13} />} />
                  </Popconfirm>,
                ]}
              >
                <List.Item.Meta
                  title={
                    <Space>
                      <Typography.Text strong>{cfg.name}</Typography.Text>
                      {active && <Tag color="blue">活动</Tag>}
                    </Space>
                  }
                  description={
                    <Space direction="vertical" size={0}>
                      <Typography.Text type="secondary" style={{ fontSize: 12 }}>
                        服务器: {cfg.server_address || '默认官方服务器'}
                      </Typography.Text>
                      <Typography.Text type="secondary" style={{ fontSize: 12 }}>
                        Token: {maskToken(cfg.token)}
                      </Typography.Text>
                      <Typography.Text type="secondary" style={{ fontSize: 12 }}>
                        虚拟 IP: {cfg.virtual_ip || '自动分配'}
                      </Typography.Text>
                    </Space>
                  }
                />
              </List.Item>
            );
          }}
        />
      )}
    </Card>
  );
}

function maskToken(token: string): string {
  return token;
}
