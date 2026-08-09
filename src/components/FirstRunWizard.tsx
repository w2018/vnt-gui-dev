// 首次启动向导（文档 F10）：权限检查 → 环境检测 → 基础配置引导

import { useEffect, useState } from 'react';
import { Button, Modal, Space, Steps, Typography, message } from 'antd';
import { api } from '../lib/tauri';

const WIZARD_KEY = 'vnt-wizard-done';

export function FirstRunWizard() {
  const [open, setOpen] = useState(false);
  const [step, setStep] = useState(0);
  const [vntVersion, setVntVersion] = useState<string | null>(null);
  const [sidecarOk, setSidecarOk] = useState(false);

  useEffect(() => {
    if (localStorage.getItem(WIZARD_KEY) === '1') return;
    setOpen(true);
  }, []);

  const checkEnv = async () => {
    try {
      const v = await api.getVntVersion();
      setVntVersion(v);
      setSidecarOk(true);
      message.success('环境检测通过');
    } catch (e) {
      setVntVersion(String(e));
      setSidecarOk(false);
      message.warning('vnt-cli 不可用，请检查程序目录');
    }
  };

  const finish = () => {
    localStorage.setItem(WIZARD_KEY, '1');
    setOpen(false);
  };

  return (
    <Modal
      open={open}
      title="欢迎使用 VNT GUI"
      closable={false}
      maskClosable={false}
      footer={null}
      width={560}
    >
      <Steps
        current={step}
        size="small"
        style={{ marginBottom: 24 }}
        items={[{ title: '权限' }, { title: '环境检测' }, { title: '开始使用' }]}
      />

      {step === 0 && (
        <div>
          <Typography.Paragraph>
            本应用通过 <Typography.Text code>vnt-cli</Typography.Text>{' '}
            创建虚拟局域网（TUN 虚拟网卡）。首次使用时：
          </Typography.Paragraph>
          <ul>
            <li>若创建虚拟网卡失败，请以管理员身份运行本程序</li>
            <li>关闭窗口会最小化到系统托盘，程序继续运行</li>
          </ul>
          <Button type="primary" onClick={() => setStep(1)}>
            下一步
          </Button>
        </div>
      )}

      {step === 1 && (
        <div>
          <Typography.Paragraph>检测 vnt-cli 运行环境...</Typography.Paragraph>
          {vntVersion === null ? (
            <Button onClick={checkEnv}>开始检测</Button>
          ) : (
            <div>
              <Typography.Paragraph>
                {sidecarOk ? (
                  <Typography.Text type="success">
                    检测通过：vnt-cli {vntVersion}
                  </Typography.Text>
                ) : (
                  <Typography.Text type="danger">{vntVersion}</Typography.Text>
                )}
              </Typography.Paragraph>
              <Space>
                {!sidecarOk && (
                  <Button onClick={checkEnv}>重新检测</Button>
                )}
                <Button
                  type="primary"
                  disabled={!sidecarOk}
                  onClick={() => setStep(2)}
                >
                  下一步
                </Button>
              </Space>
            </div>
          )}
        </div>
      )}

      {step === 2 && (
        <div>
          <Typography.Paragraph>
            在「配置」页创建你的第一个组网配置（组网编号 Token 必填），
            然后回到首页点击「连接」即可组网。
          </Typography.Paragraph>
          <Button type="primary" onClick={finish}>
            开始使用
          </Button>
        </div>
      )}
    </Modal>
  );
}
