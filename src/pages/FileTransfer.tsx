// 文件传输主页面：目标设备选择 + 拖拽发送 + 队列/历史/文本/设置

import { useEffect, useState } from 'react';
import { Button, Card, Checkbox, Input, InputNumber, Select, Space, Switch, Tabs, Tag, Typography, message } from 'antd';
import { open } from '@tauri-apps/plugin-dialog';
import { useFileTransferStore } from '../stores/useFileTransferStore';
import { useDeviceStore } from '../stores/deviceStore';
import { ChannelNotice } from '../components/file/ChannelNotice';
import { FileDropZone } from '../components/file/FileDropZone';
import { PendingFileList } from '../components/file/PendingFileList';
import { TransferQueue } from '../components/file/TransferQueue';
import { HistoryList } from '../components/file/HistoryList';
import { TextTransfer } from '../components/file/TextTransfer';
import type { FileTypeFilter } from '../types/file_transfer';

const { Title, Text } = Typography;

// ==================== 设置区块 ====================

/** 预设扩展名分组（勾选即加入过滤列表） */
const PRESET_EXT_GROUPS: { label: string; exts: string[] }[] = [
  { label: '文档', exts: ['txt', 'pdf', 'doc', 'docx', 'xls', 'xlsx', 'ppt', 'pptx'] },
  { label: '图片', exts: ['png', 'jpg', 'jpeg', 'gif', 'bmp', 'webp'] },
  { label: '音视频', exts: ['mp3', 'mp4', 'wav', 'flac'] },
  { label: '压缩包', exts: ['zip', 'rar', '7z', 'tar', 'gz'] },
  { label: '数据文本', exts: ['json', 'xml', 'csv', 'md'] },
  { label: '代码', exts: ['rs', 'ts', 'tsx', 'js', 'py', 'go', 'java', 'c', 'cpp', 'h'] },
];

function TransferSettings() {
  const { filter, updateFilter, thresholdMB, setThreshold, autoAccept, setAutoAccept, saveDir, setSaveDir } =
    useFileTransferStore();
  const [newExt, setNewExt] = useState('');

  /** 选择默认保存目录（接收文件存放位置） */
  const changeSaveDir = async () => {
    const dir = await open({
      directory: true,
      title: '选择默认保存目录',
    });
    if (!dir) return;
    try {
      await setSaveDir(dir);
      message.success('默认保存位置已更新');
    } catch (e) {
      message.error(`设置失败: ${String(e)}`);
    }
  };

  const extensions = filter?.extensions ?? [];

  const saveFilter = async (patch: Partial<FileTypeFilter>) => {
    try {
      await updateFilter(patch);
      message.success('过滤规则已保存');
    } catch (e) {
      message.error(`保存失败: ${String(e)}`);
    }
  };

  /** 勾选/取消扩展名 */
  const toggleExt = async (ext: string, checked: boolean) => {
    const next = new Set(extensions);
    if (checked) {
      next.add(ext);
    } else {
      next.delete(ext);
    }
    await saveFilter({ extensions: Array.from(next) });
  };

  /** 手动添加新扩展名 */
  const addExt = async () => {
    const ext = newExt.trim().toLowerCase().replace(/^\./, '');
    if (!ext) return;
    if (extensions.includes(ext)) {
      message.info('该扩展名已在列表中');
      setNewExt('');
      return;
    }
    await saveFilter({ extensions: [...extensions, ext] });
    setNewExt('');
  };

  // 预设分组中已有的扩展名（用于勾选高亮）+ 用户自定义的扩展名
  const presetExts = new Set(PRESET_EXT_GROUPS.flatMap((g) => g.exts));
  const customExts = extensions.filter((e) => !presetExts.has(e));

  const mode = filter?.mode;
  const listInactive = mode === 'AllowAll' || mode === 'DenyAll';

  return (
    <Space direction="vertical" size="middle" style={{ width: '100%' }}>
      <Card size="small" title="接收规则">
        <Space direction="vertical" size="middle" style={{ width: '100%' }}>
          <Space wrap>
            <Text>过滤模式：</Text>
            <Select
              style={{ width: 220 }}
              value={mode}
              onChange={(m) => void saveFilter({ mode: m })}
              options={[
                { value: 'Whitelist', label: '白名单（仅允许列表内类型）' },
                { value: 'Blacklist', label: '黑名单（列表内类型拒绝）' },
                { value: 'AllowAll', label: '全部允许' },
                { value: 'DenyAll', label: '全部拒绝（仅手动确认）' },
              ]}
            />
            {listInactive && (
              <Text type="warning" style={{ fontSize: 12 }}>
                当前模式下扩展名列表不参与过滤
              </Text>
            )}
          </Space>

          <div>
            <Text strong style={{ display: 'block', marginBottom: 8 }}>
              扩展名列表（勾选即加入过滤）
            </Text>
            <Space direction="vertical" size={10} style={{ width: '100%' }}>
              {PRESET_EXT_GROUPS.map((g) => (
                <div key={g.label}>
                  <Text type="secondary" style={{ fontSize: 12, marginRight: 8 }}>
                    {g.label}
                  </Text>
                  <Space wrap size={[8, 4]}>
                    {g.exts.map((ext) => (
                      <Checkbox
                        key={ext}
                        checked={extensions.includes(ext)}
                        disabled={listInactive}
                        onChange={(e) => void toggleExt(ext, e.target.checked)}
                      >
                        {ext}
                      </Checkbox>
                    ))}
                  </Space>
                </div>
              ))}

              {customExts.length > 0 && (
                <div>
                  <Text type="secondary" style={{ fontSize: 12, marginRight: 8 }}>
                    自定义
                  </Text>
                  <Space wrap size={[8, 4]}>
                    {customExts.map((ext) => (
                      <Checkbox
                        key={ext}
                        checked
                        disabled={listInactive}
                        onChange={() => void toggleExt(ext, false)}
                      >
                        {ext}
                      </Checkbox>
                    ))}
                  </Space>
                </div>
              )}

              <Space>
                <Input
                  style={{ width: 180 }}
                  placeholder="手动添加，如：apk"
                  value={newExt}
                  maxLength={20}
                  onChange={(e) => setNewExt(e.target.value)}
                  onPressEnter={() => void addExt()}
                  disabled={listInactive}
                />
                <Button onClick={() => void addExt()} disabled={listInactive}>
                  添加扩展名
                </Button>
              </Space>
            </Space>
          </div>

          <Space wrap>
            <Text>自动接收白名单文件：</Text>
            <Switch
              checked={autoAccept}
              onChange={(v) => {
                void setAutoAccept(v).then(() =>
                  message.success(v ? '自动接收已开启' : '自动接收已关闭'),
                );
              }}
            />
            <Text type="secondary" style={{ fontSize: 12 }}>
              开启后，匹配过滤规则的文件将自动接收（不弹确认框）
            </Text>
          </Space>

          <Space wrap>
            <Text>默认保存位置：</Text>
            <Text type="secondary" style={{ fontSize: 12 }}>
              {saveDir || '（应用数据目录/file_transfers）'}
            </Text>
            <Button size="small" onClick={() => void changeSaveDir()}>
              更改
            </Button>
          </Space>
        </Space>
      </Card>

      <Card size="small" title="通道阈值">
        <Space wrap>
          <Text>文件大小超过（MB）走 TCP 高速通道：</Text>
          <InputNumber
            min={1}
            max={10000}
            value={thresholdMB}
            onChange={(v) => {
              if (v) {
                void setThreshold(v).then(() => message.success(`通道阈值已设为 ${v} MB`));
              }
            }}
          />
          <Text type="secondary" style={{ fontSize: 12 }}>
            当前值：{thresholdMB} MB
          </Text>
        </Space>
      </Card>
    </Space>
  );
}

// ==================== 主页面 ====================

export function FileTransfer() {
  const { init, setupListeners, transfers, targetIp, setTargetIp } = useFileTransferStore();
  const historyCount = useFileTransferStore((s) => s.history.length);
  const activeTab = useFileTransferStore((s) => s.activeTab);
  const setActiveTab = useFileTransferStore((s) => s.setActiveTab);
  const devices = useDeviceStore((s) => s.devices);

  useEffect(() => {
    init().catch(() => message.error('初始化文件传输失败'));
    // 刷新在线设备列表（目标设备下拉）
    void useDeviceStore.getState().refresh();
    let cleanup: (() => void) | undefined;
    void setupListeners().then((fn) => {
      cleanup = fn;
    });
    return () => cleanup?.();
  }, [init, setupListeners]);

  // 目标设备实时更新：每 5 秒刷新在线设备
  useEffect(() => {
    const timer = window.setInterval(() => {
      void useDeviceStore.getState().refresh();
    }, 5000);
    return () => window.clearInterval(timer);
  }, []);

  const onlineDevices = devices.filter((d) => d.status === 'online');
  const activeTransfers = transfers.filter(
    (t) => t.status === 'Pending' || t.status === 'Transferring' || t.status === 'Paused',
  );

  // 传输中列表无任务 → 自动跳转到"已完成/终止"（发送时已由 store 跳"传输中"）
  useEffect(() => {
    if (activeTransfers.length === 0) {
      setActiveTab('finished');
    }
  }, [activeTransfers.length, setActiveTab]);

  return (
    <div style={{ padding: 16, height: '100%', display: 'flex', flexDirection: 'column' }}>
      <div style={{ display: 'flex', justifyContent: 'space-between', alignItems: 'center', marginBottom: 12 }}>
        <Title level={4} style={{ margin: 0 }}>
          文件传输
        </Title>
        <Space wrap>
          <Tag color={onlineDevices.length > 0 ? 'success' : 'default'}>
            可用设备 {onlineDevices.length}
          </Tag>
          <Text type="secondary">目标设备：</Text>
          <Select
            style={{ width: 280 }}
            placeholder="选择要发送到的设备"
            value={targetIp ?? undefined}
            onChange={(ip) => setTargetIp(ip)}
            options={onlineDevices.map((d) => ({
              value: d.virtual_ip,
              label: `${d.name} (${d.virtual_ip} · ${d.latency}ms)`,
            }))}
          />
        </Space>
      </div>

      <ChannelNotice />
      <FileDropZone />
      <PendingFileList />

      <Tabs
        activeKey={activeTab}
        onChange={setActiveTab}
        style={{ flex: 1, minHeight: 0, overflow: 'auto' }}
        items={[
          {
            key: 'active',
            label: (
              <span>
                传输中
                {activeTransfers.length > 0 && (
                  <Tag color="processing" style={{ marginLeft: 4 }}>
                    {activeTransfers.length}
                  </Tag>
                )}
              </span>
            ),
            children: <TransferQueue transfers={activeTransfers} />,
          },
          {
            key: 'finished',
            label: (
              <span>
                已完成/终止
                {historyCount > 0 && (
                  <Tag style={{ marginLeft: 4 }}>{historyCount}</Tag>
                )}
              </span>
            ),
            children: <HistoryList />,
          },
          {
            key: 'text',
            label: '文本传输',
            children: <TextTransfer />,
          },
          {
            key: 'settings',
            label: '设置',
            children: <TransferSettings />,
          },
        ]}
      />
    </div>
  );
}
