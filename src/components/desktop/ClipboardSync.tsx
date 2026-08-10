// 剪贴板同步面板

import { Button, Card, Input, Space, Typography, message } from 'antd';
import { useState } from 'react';
import { useDesktopStore } from '../../stores/useDesktopStore';

const { Text } = Typography;

export function ClipboardSync() {
  const [text, setText] = useState('');
  const { sendInput, lastClipboard, session } = useDesktopStore();
  const enabled = session.capabilities?.clipboard ?? true;

  const handleSend = () => {
    if (!text.trim()) {
      message.warning('请输入要发送的文本');
      return;
    }
    sendInput({ ClipboardText: { text } }).catch((e) => message.error(String(e)));
    message.success('已发送剪贴板内容');
  };

  return (
    <Card title="剪贴板同步" size="small" style={{ marginTop: 12 }}>
      <Space direction="vertical" style={{ width: '100%' }}>
        <Input.TextArea
          value={text}
          onChange={(e) => setText(e.target.value)}
          placeholder="输入文本后点击发送（对方将写入系统剪贴板）"
          rows={2}
          disabled={!enabled}
        />
        <Space style={{ width: '100%', justifyContent: 'space-between' }}>
          <Button type="primary" onClick={handleSend} size="small" disabled={!enabled}>
            发送剪贴板
          </Button>
          {lastClipboard && (
            <Text type="secondary" ellipsis style={{ maxWidth: 260, fontSize: 12 }}>
              最近接收: {lastClipboard}
            </Text>
          )}
        </Space>
      </Space>
    </Card>
  );
}
