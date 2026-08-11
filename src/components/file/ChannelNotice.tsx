// 传输通道规则提醒横幅

import { Alert, Typography } from 'antd';
import { Info } from 'lucide-react';
import { useFileTransferStore } from '../../stores/useFileTransferStore';

const { Text } = Typography;

export function ChannelNotice() {
  const { thresholdMB } = useFileTransferStore();

  return (
    <Alert
      style={{ marginBottom: 12 }}
      icon={<Info size={16} />}
      message={
        <Text>
          <Text strong>传输通道规则：</Text>
          文件 &lt; {thresholdMB}MB 走 QUIC 加密流（低延迟），
          ≥ {thresholdMB}MB 自动切换为 TCP 高速通道（跑满带宽）。
          <Text type="warning"> 大文件传输时请确保双方网络稳定。</Text>
        </Text>
      }
      type="info"
      showIcon
    />
  );
}
