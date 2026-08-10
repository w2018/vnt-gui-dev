// FTP 服务主页面（需求 F1-F9）

import { useEffect } from 'react';
import { Row, Col } from 'antd';
import { useFtpStore } from '../stores/useFtpStore';
import { FtpStatusCard } from '../components/ftp/FtpStatusCard';
import { FtpGeneralSettings } from '../components/ftp/FtpGeneralSettings';
import { FtpUserManager } from '../components/ftp/FtpUserManager';
import { FtpConnectionLogs } from '../components/ftp/FtpConnectionLogs';

export function FtpService() {
  const loadAll = useFtpStore((s) => s.loadAll);

  // 进入页面加载配置/状态/日志
  useEffect(() => {
    void loadAll();
  }, [loadAll]);

  return (
    <div>
      <Row gutter={16}>
        <Col span={8}>
          <FtpStatusCard />
        </Col>
        <Col span={16}>
          <FtpGeneralSettings />
        </Col>
      </Row>
      <div style={{ marginTop: 16 }}>
        <FtpUserManager />
      </div>
      <div style={{ marginTop: 16 }}>
        <FtpConnectionLogs />
      </div>
    </div>
  );
}
