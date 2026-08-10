// 连接确认弹窗（被控端收到请求时）

import { Button, Checkbox, Modal, Space, Typography } from 'antd';
import { useState } from 'react';
import { useDesktopStore } from '../../stores/useDesktopStore';

const { Text, Title } = Typography;

export function ConnectionRequest() {
  const { pendingRequest, acceptRequest, rejectRequest } = useDesktopStore();
  const [grantMouse, setGrantMouse] = useState(true);
  const [grantKeyboard, setGrantKeyboard] = useState(true);
  const [grantClipboard, setGrantClipboard] = useState(true);
  const [viewOnly, setViewOnly] = useState(false);

  const visible = !!pendingRequest;

  const handleAccept = () => {
    acceptRequest({
      mouse: grantMouse && !viewOnly,
      keyboard: grantKeyboard && !viewOnly,
      clipboard: grantClipboard,
      viewOnly,
    }).catch((e) => console.error(e));
  };

  const handleReject = () => {
    rejectRequest('用户拒绝连接请求').catch((e) => console.error(e));
  };

  return (
    <Modal
      title="🔐 桌面共享请求"
      open={visible}
      onCancel={handleReject}
      footer={[
        <Button key="reject" danger onClick={handleReject}>
          拒绝
        </Button>,
        <Button key="accept" type="primary" onClick={handleAccept}>
          接受
        </Button>,
      ]}
      maskClosable={false}
    >
      {pendingRequest && (
        <Space direction="vertical" size="middle" style={{ width: '100%' }}>
          <div>
            <Text strong>{pendingRequest.device_name}</Text>
            <Text type="secondary"> 请求连接你的桌面</Text>
          </div>
          <div>
            <Text type="secondary" style={{ fontSize: 12 }}>
              请求的能力：
              {pendingRequest.capabilities.mouse ? ' 鼠标' : ''}
              {pendingRequest.capabilities.keyboard ? ' 键盘' : ''}
              {pendingRequest.capabilities.clipboard ? ' 剪贴板' : ''}
              {pendingRequest.capabilities.view_only ? ' (仅查看)' : ''}
            </Text>
          </div>
          <div>
            <Title level={5} style={{ marginBottom: 8 }}>
              授予权限
            </Title>
            <Space direction="vertical">
              <Checkbox
                checked={grantMouse}
                onChange={(e) => setGrantMouse(e.target.checked)}
                disabled={viewOnly}
              >
                允许鼠标控制
              </Checkbox>
              <Checkbox
                checked={grantKeyboard}
                onChange={(e) => setGrantKeyboard(e.target.checked)}
                disabled={viewOnly}
              >
                允许键盘控制
              </Checkbox>
              <Checkbox
                checked={grantClipboard}
                onChange={(e) => setGrantClipboard(e.target.checked)}
              >
                允许剪贴板同步
              </Checkbox>
              <Checkbox
                checked={viewOnly}
                onChange={(e) => {
                  setViewOnly(e.target.checked);
                  if (e.target.checked) {
                    setGrantMouse(false);
                    setGrantKeyboard(false);
                  }
                }}
              >
                仅查看模式（禁止对方控制）
              </Checkbox>
            </Space>
          </div>
        </Space>
      )}
    </Modal>
  );
}
