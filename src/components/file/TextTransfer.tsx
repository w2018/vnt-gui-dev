// 文本传输面板：输入文本直接发送（走 QUIC 控制流，实时到达）

import { useState } from 'react';
import { Button, Input, List, Space, Tag, Typography, message } from 'antd';
import { Send } from 'lucide-react';
import { useFileTransferStore } from '../../stores/useFileTransferStore';

const { Text } = Typography;

export function TextTransfer() {
  const { sendText, textMessages, targetIp } = useFileTransferStore();
  const [text, setText] = useState('');
  const [sending, setSending] = useState(false);

  const handleSend = async () => {
    const trimmed = text.trim();
    if (!trimmed) return;
    if (!targetIp) {
      message.warning('请先在左侧选择目标设备');
      return;
    }
    setSending(true);
    try {
      await sendText(trimmed);
      message.success('文本已发送');
      setText('');
    } catch (e) {
      message.error(`发送失败: ${String(e)}`);
    } finally {
      setSending(false);
    }
  };

  return (
    <Space direction="vertical" size="middle" style={{ width: '100%' }}>
      <div>
        <Text type="secondary" style={{ fontSize: 12 }}>
          文本消息走 QUIC 控制流，实时到达，无需建立额外连接
        </Text>
      </div>
      <Input.TextArea
        value={text}
        onChange={(e) => setText(e.target.value)}
        placeholder="输入要发送的文本（支持任意长度）"
        rows={4}
        maxLength={10000}
      />
      <Space>
        <Button
          type="primary"
          icon={<Send size={14} />}
          onClick={() => void handleSend()}
          loading={sending}
          disabled={!text.trim()}
        >
          发送文本
        </Button>
        {!targetIp && <Text type="warning">未选择目标设备</Text>}
      </Space>

      {textMessages.length > 0 && (
        <>
          <Text strong>消息记录</Text>
          <List
            dataSource={textMessages.slice().reverse()}
            rowKey={(m) => m.msg_id}
            renderItem={(msg) => (
              <List.Item>
                <List.Item.Meta
                  title={
                    <Space>
                      <Tag color="blue">{msg.from}</Tag>
                      <Text style={{ fontSize: 12 }}>
                        {new Date(msg.timestamp * 1000).toLocaleString('zh-CN')}
                      </Text>
                    </Space>
                  }
                  description={<Text style={{ whiteSpace: 'pre-wrap' }}>{msg.text}</Text>}
                />
              </List.Item>
            )}
          />
        </>
      )}
    </Space>
  );
}
