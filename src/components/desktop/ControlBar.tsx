// 控制栏：全屏 / 特殊键 / 断开

import { Button, Divider, Space, Tooltip } from 'antd';
import { Maximize, Minimize } from 'lucide-react';
import { useState } from 'react';
import { useDesktopStore } from '../../stores/useDesktopStore';

export function ControlBar() {
  const { sendInput, disconnect, session } = useDesktopStore();
  const [fullscreen, setFullscreen] = useState(false);

  const isViewOnly = session.capabilities?.view_only ?? false;

  const handleFullscreen = () => {
    const canvas = document.querySelector('canvas[tabindex="0"]');
    if (!canvas) return;
    if (document.fullscreenElement) {
      document.exitFullscreen().catch(() => {});
      setFullscreen(false);
    } else {
      canvas.requestFullscreen().catch(() => {});
      setFullscreen(true);
    }
  };

  const sendSpecial = (kind: 'CtrlAltDel' | 'AltTab' | 'WinKey' | 'TaskManager') => {
    sendInput({ SpecialKey: { kind } }).catch(() => {});
  };

  return (
    <Space wrap>
      <Tooltip title={fullscreen ? '退出全屏' : '全屏'}>
        <Button
          icon={fullscreen ? <Minimize size={14} /> : <Maximize size={14} />}
          onClick={handleFullscreen}
        />
      </Tooltip>
      <Divider type="vertical" />
      {!isViewOnly && (
        <>
          <Tooltip title="受 Windows 安全机制保护，被控端需手动操作">
            <Button onClick={() => sendSpecial('CtrlAltDel')}>Ctrl+Alt+Del</Button>
          </Tooltip>
          <Tooltip title="Alt+Tab">
            <Button onClick={() => sendSpecial('AltTab')}>Alt+Tab</Button>
          </Tooltip>
          <Tooltip title="任务管理器 (Ctrl+Shift+Esc)">
            <Button onClick={() => sendSpecial('TaskManager')}>任务管理器</Button>
          </Tooltip>
          <Divider type="vertical" />
        </>
      )}
      <Tooltip title="断开连接">
        <Button danger onClick={() => disconnect('用户主动断开').catch(() => {})}>
          断开
        </Button>
      </Tooltip>
    </Space>
  );
}
