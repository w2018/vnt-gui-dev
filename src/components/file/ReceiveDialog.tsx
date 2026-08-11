// 接收确认弹窗：收到文件请求时全局弹出（任意页面/后台可见）
// 使用系统保存对话框选择保存路径（替代 prompt）；30 秒确认超时 + 倒计时。
// 并发请求以队列形式逐个展示（pendingOffers[0]），确认/拒绝后自动轮到下一个。

import { useEffect, useState } from 'react';
import { Alert, Button, Modal, Space, Tag, Typography, message } from 'antd';
import { save } from '@tauri-apps/plugin-dialog';
import { useFileTransferStore } from '../../stores/useFileTransferStore';
import { channelLabel, formatSize, type FileOffer } from '../../types/file_transfer';

const { Text, Title } = Typography;

/** 确认超时（秒，与后端 CONFIRM_TIMEOUT_SECS 一致） */
const CONFIRM_TIMEOUT_SECS = 30;

export function ReceiveDialog() {
  const { pendingOffers, acceptOffer, rejectOffer, dismissOffer, filter, saveDir } =
    useFileTransferStore();
  const [handling, setHandling] = useState(false);
  const [remaining, setRemaining] = useState(CONFIRM_TIMEOUT_SECS);

  // 当前待确认的请求（队列头部）；无请求则不显示
  const offer: FileOffer | null = pendingOffers[0] ?? null;
  const offerId = offer?.transfer_id ?? null;

  // 收到新请求 → 重置倒计时，每秒递减
  useEffect(() => {
    if (offerId == null) return;
    setRemaining(CONFIRM_TIMEOUT_SECS);
    const timer = window.setInterval(() => {
      setRemaining((r) => {
        if (r <= 1) {
          window.clearInterval(timer);
          return 0;
        }
        return r - 1;
      });
    }, 1000);
    return () => window.clearInterval(timer);
  }, [offerId]);

  // 倒计时归零：关闭弹窗（后端已按超时自动拒绝）
  useEffect(() => {
    if (remaining <= 0 && offerId != null) {
      dismissOffer(offerId);
    }
  }, [remaining, offerId, dismissOffer]);

  if (!offer) return null;

  const isAutoAccept =
    filter?.mode === 'AllowAll' ||
    (filter?.mode === 'Whitelist' &&
      filter.extensions.includes(offer.filename.split('.').pop()?.toLowerCase() || ''));

  const channelType = channelLabel(offer.channel);
  const isTcp = offer.channel !== 'Quic';

  const handleAccept = async () => {
    // 系统保存对话框选择最终保存路径（默认在接收目录）
    const result = await save({
      title: '保存接收的文件',
      defaultPath: offer.default_save_path,
    });
    if (!result) return; // 用户取消保存对话框 → 保持请求
    setHandling(true);
    try {
      await acceptOffer(offer.transfer_id, result);
      message.success('已开始接收');
    } catch (e) {
      message.error(`接收失败: ${String(e)}`);
    } finally {
      setHandling(false);
    }
  };

  const handleReject = async (reason: string) => {
    setHandling(true);
    try {
      await rejectOffer(offer.transfer_id, reason);
    } catch (e) {
      message.error(`拒绝失败: ${String(e)}`);
    } finally {
      setHandling(false);
    }
  };

  /** 保存到默认位置（不弹保存对话框） */
  const handleAcceptDefault = async () => {
    const dir = saveDir.trim();
    const target =
      dir.length > 0
        ? dir.endsWith('\\') || dir.endsWith('/')
          ? `${dir}${offer.filename}`
          : `${dir}\\${offer.filename}`
        : offer.default_save_path;
    setHandling(true);
    try {
      await acceptOffer(offer.transfer_id, target);
      message.success('已开始接收（保存到默认位置）');
    } catch (e) {
      message.error(`接收失败: ${String(e)}`);
    } finally {
      setHandling(false);
    }
  };

  return (
    <Modal
      title={<Title level={5} style={{ margin: 0 }}>📥 收到文件传输请求</Title>}
      open
      onCancel={() => void handleReject('用户取消')}
      maskClosable={false}
      confirmLoading={handling}
      footer={[
        <Button key="reject" onClick={() => void handleReject('用户拒绝')} disabled={handling}>
          拒绝
        </Button>,
        <Button key="accept-default" onClick={() => void handleAcceptDefault()} disabled={handling}>
          保存到默认位置
        </Button>,
        <Button key="accept" type="primary" onClick={() => void handleAccept()} disabled={handling}>
          选择保存位置并接收
        </Button>,
      ]}
    >
      <Space direction="vertical" size="middle" style={{ width: '100%' }}>
        {pendingOffers.length > 1 && (
          <Alert
            type="info"
            showIcon
            message={`还有 ${pendingOffers.length - 1} 个请求等待确认，将逐个显示`}
          />
        )}
        <div>
          <Text strong style={{ fontSize: 16 }}>
            {offer.filename}
          </Text>
        </div>
        <Space size="large" wrap>
          <Text type="secondary">大小：{formatSize(offer.file_size)}</Text>
          <Tag color={isTcp ? 'orange' : 'blue'}>{channelType}</Tag>
          {offer.resume_offset > 0 && (
            <Tag color="green">
              检测到断点：已接收 {formatSize(offer.resume_offset)}，将续传
            </Tag>
          )}
          <Tag color={remaining <= 10 ? 'red' : 'default'}>
            ⏱ 剩余 {remaining} 秒后自动拒绝
          </Tag>
        </Space>
        <div>
          <Text type="secondary">
            来自：{offer.remote_device} ({offer.remote_ip})
          </Text>
        </div>
        <div>
          <Text type="secondary" style={{ fontSize: 12 }}>
            默认保存到：{offer.default_save_path}
          </Text>
        </div>
        {isTcp && (
          <Alert type="warning" message="大文件将使用 TCP 高速通道传输，请确保网络稳定" showIcon />
        )}
        {isAutoAccept && (
          <Alert type="info" message="此文件类型在自动接收白名单中，可自动接受" showIcon />
        )}
      </Space>
    </Modal>
  );
}
