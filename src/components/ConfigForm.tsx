// 配置表单（文档 §4.2.2）

import { useState } from 'react';
import {
  Button,
  Card,
  Form,
  Input,
  InputNumber,
  Select,
  Space,
  Switch,
  message,
} from 'antd';
import { defaultConfig, type VntConfig } from '../lib/types';
import { useConfigStore } from '../stores/configStore';

type Protocol = 'udp' | 'tcp' | 'ws' | 'wss';

function protocolOf(config?: VntConfig): Protocol {
  const server = config?.server_address ?? '';
  if (server.includes('tcp://')) return 'tcp';
  if (server.includes('wss://')) return 'wss';
  if (server.includes('ws://')) return 'ws';
  return 'udp';
}

/** 保存时按协议生成 server 前缀 */
function applyProtocol(server: string | undefined, protocol: Protocol): string | undefined {
  const s = (server ?? '').trim();
  if (!s) return undefined;
  // 已有前缀则保留
  if (s.includes('://')) return s;
  if (protocol === 'udp') return s;
  return `${protocol}://${s}`;
}

export function ConfigForm({ config }: { config?: VntConfig }) {
  const { save } = useConfigStore();
  const [saving, setSaving] = useState(false);
  const [form] = Form.useForm();

  const initialValues = {
    name: config?.name ?? '',
    token: config?.token ?? '',
    device_name: config?.device_name ?? '',
    device_id: config?.device_id ?? '',
    virtual_ip: config?.virtual_ip ?? '',
    server_address: config?.server_address ?? '',
    protocol: protocolOf(config),
    password: config?.password ?? '',
    server_encrypt: config?.server_encrypt ?? false,
    in_ips: config?.in_ips ?? [],
    out_ips: config?.out_ips ?? [],
    compressor: config?.compressor ?? '',
    mtu: config?.mtu ?? undefined,
    no_proxy: config?.no_proxy ?? false,
  };

  const handleFinish = async (values: Record<string, unknown>) => {
    const protocol = (values.protocol as Protocol) ?? 'udp';
    const merged: VntConfig = {
      ...defaultConfig(),
      ...(config ?? {}),
      id: config?.id ?? '',
      name: String(values.name ?? ''),
      token: String(values.token ?? ''),
      device_name: strOrUndef(values.device_name),
      device_id: strOrUndef(values.device_id),
      virtual_ip: strOrUndef(values.virtual_ip),
      server_address: applyProtocol(strOrUndef(values.server_address), protocol),
      password: strOrUndef(values.password),
      server_encrypt: Boolean(values.server_encrypt),
      in_ips: (values.in_ips as string[]) ?? [],
      out_ips: (values.out_ips as string[]) ?? [],
      compressor: strOrUndef(values.compressor),
      mtu: values.mtu as number | undefined,
      use_tcp: protocol === 'tcp',
      use_ws: protocol === 'ws' || protocol === 'wss',
      no_proxy: Boolean(values.no_proxy),
      created_at: config?.created_at ?? '',
      updated_at: config?.updated_at ?? '',
    };
    setSaving(true);
    try {
      await save(merged);
      message.success('配置已保存');
      form.resetFields();
    } catch (e) {
      message.error(`保存失败: ${String(e)}`);
    } finally {
      setSaving(false);
    }
  };

  return (
    <Card
      title={config ? `编辑配置：${config.name}` : '新建配置'}
      styles={{ body: { maxHeight: 'calc(100vh - 140px)', overflow: 'auto' } }}
    >
      <Form
        form={form}
        layout="vertical"
        initialValues={initialValues}
        onFinish={handleFinish}
        requiredMark="optional"
      >
        <Form.Item
          name="name"
          label="配置名称（仅本地标识）"
          rules={[{ required: true, message: '请输入配置名称' }]}
        >
          <Input placeholder="例：家庭网络" />
        </Form.Item>
        <Form.Item
          name="token"
          label="组网编号 (Token)"
          rules={[{ required: true, message: '请输入组网编号' }]}
        >
          <Input placeholder="vnt 组网编号（必填）" />
        </Form.Item>

        <Form.Item name="device_name" label="设备名称">
          <Input placeholder="留空自动生成" />
        </Form.Item>
        <Form.Item name="device_id" label="设备 ID">
          <Input placeholder="留空自动生成" />
        </Form.Item>
        <Form.Item name="virtual_ip" label="虚拟 IP">
          <Input placeholder="10.26.0.x（留空自动分配）" />
        </Form.Item>

        <Space.Compact style={{ width: '100%' }}>
          <Form.Item name="server_address" label="服务器地址" style={{ flex: 1 }}>
            <Input placeholder="默认官方服务器" />
          </Form.Item>
          <Form.Item name="protocol" label="协议">
            <Select
              style={{ width: 110 }}
              options={[
                { value: 'udp', label: 'UDP（默认）' },
                { value: 'tcp', label: 'TCP' },
                { value: 'ws', label: 'WebSocket' },
                { value: 'wss', label: 'WSS（加密）' },
              ]}
            />
          </Form.Item>
        </Space.Compact>

        <Form.Item name="password" label="组网密码">
          <Input.Password placeholder="留空则不使用密码" />
        </Form.Item>
        <Form.Item name="server_encrypt" label="服务端加密" valuePropName="checked">
          <Switch />
        </Form.Item>

        <Form.Item name="in_ips" label="入站网段 (-i)">
          <Select
            mode="tags"
            placeholder="输入后回车，例: 192.168.1.0/24"
            tokenSeparators={[',', '，', ' ']}
          />
        </Form.Item>
        <Form.Item name="out_ips" label="出站网段 (-o)">
          <Select
            mode="tags"
            placeholder="输入后回车，例: 192.168.0.0/24"
            tokenSeparators={[',', '，', ' ']}
          />
        </Form.Item>

        <Form.Item name="compressor" label="压缩算法">
          <Select
            allowClear
            placeholder="无"
            options={[{ value: 'lz4', label: 'LZ4（快速）' }]}
          />
        </Form.Item>

        <Form.Item name="mtu" label="MTU">
          <InputNumber min={576} max={1500} placeholder="默认" style={{ width: '100%' }} />
        </Form.Item>

        <Form.Item name="no_proxy" label="禁用系统代理" valuePropName="checked">
          <Switch />
        </Form.Item>

        <Button type="primary" htmlType="submit" loading={saving}>
          保存配置
        </Button>
      </Form>
    </Card>
  );
}

function strOrUndef(v: unknown): string | undefined {
  const s = String(v ?? '').trim();
  return s.length > 0 ? s : undefined;
}

// 供新建时使用（保持 defaultConfig 导出一致）
export { defaultConfig };
