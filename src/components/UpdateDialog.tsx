// 更新页（文档 §3.9.3）：vnt-cli 与 GUI 分开检测与更新

import { useEffect, useState } from 'react';
import { Alert, Button, Card, Progress, Space, Typography, message } from 'antd';
import { Download, ExternalLink, RefreshCw } from 'lucide-react';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import { open } from '@tauri-apps/plugin-shell';
import { api } from '../lib/tauri';
import type { UpdateInfo } from '../lib/types';

export function UpdateDialog() {
  const [info, setInfo] = useState<UpdateInfo | null>(null);
  const [checking, setChecking] = useState(false);
  const [downloading, setDownloading] = useState(false);
  const [progress, setProgress] = useState<number | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let unlisteners: UnlistenFn[] = [];
    (async () => {
      unlisteners = [
        await listen<{ downloaded: number }>('update-progress', (e) => {
          setProgress(e.payload.downloaded);
        }),
        await listen<{ version: string }>('update-complete', () => {
          setDownloading(false);
          message.success('vnt-cli 更新完成，重启后生效');
        }),
      ];
    })();
    return () => unlisteners.forEach((fn) => fn());
  }, []);

  const handleCheck = async () => {
    setChecking(true);
    setError(null);
    try {
      const result = await api.checkUpdate();
      setInfo(result);
      if (!result.has_update && !result.app_has_update) {
        message.info('vnt-cli 与 GUI 均是最新版本');
      }
    } catch (e) {
      setError(String(e));
    } finally {
      setChecking(false);
    }
  };

  const handleDownloadCli = async () => {
    if (!info?.download_url) return;
    setDownloading(true);
    setProgress(0);
    setError(null);
    try {
      await api.downloadAndReplace(info.download_url);
    } catch (e) {
      setError(`下载失败: ${String(e)}`);
      setDownloading(false);
    }
  };

  const handleOpenGuiRelease = async () => {
    try {
      await open('https://github.com/w2018/vnt-gui-dev/releases/latest');
    } catch (e) {
      message.error(`打开失败: ${String(e)}`);
    }
  };

  return (
    <div style={{ maxWidth: 720 }}>
      <Card
        title="软件更新"
        extra={
          <Button icon={<RefreshCw size={14} />} loading={checking} onClick={handleCheck}>
            检查更新
          </Button>
        }
      >
        {error && (
          <Alert type="error" showIcon message={error} style={{ marginBottom: 16 }} />
        )}
      </Card>

      {/* vnt-cli 更新区 */}
      <Card title="vnt-cli 更新" style={{ marginTop: 16 }}>
        {!info ? (
          <Typography.Text type="secondary">尚未检测</Typography.Text>
        ) : (
          <Space direction="vertical" size="middle" style={{ width: '100%' }}>
            <Alert
              type={info.has_update ? 'warning' : 'success'}
              showIcon
              message={
                info.has_update
                  ? `发现新版 vnt-cli ${info.latest_version}（当前 ${info.current_version}）`
                  : `vnt-cli 已是最新版本 ${info.current_version}`
              }
              description="更新内容：下载官方编译的最新 vnt-cli 二进制并替换本地文件（不会修改配置）"
            />
            {info.has_update && info.download_url && (
              <Button
                type="primary"
                icon={<Download size={14} />}
                loading={downloading}
                onClick={handleDownloadCli}
              >
                {downloading ? '下载中...' : '更新 vnt-cli'}
              </Button>
            )}
            {downloading && progress !== null && (
              <Progress
                percent={Math.min(100, Math.round((progress / 10_000_000) * 100))}
                status="active"
                format={() => `${(progress / 1024 / 1024).toFixed(1)} MB`}
              />
            )}
          </Space>
        )}
      </Card>

      {/* GUI 更新区 */}
      <Card title="VNT GUI 更新" style={{ marginTop: 16 }}>
        {!info ? (
          <Typography.Text type="secondary">尚未检测</Typography.Text>
        ) : (
          <Space direction="vertical" size="middle" style={{ width: '100%' }}>
            <Alert
              type={info.app_has_update ? 'warning' : 'success'}
              showIcon
              message={
                info.app_has_update && info.app_latest_version
                  ? `发现新版 GUI ${info.app_latest_version}（当前 ${info.app_version}）`
                  : `GUI 已是最新版本 ${info.app_version}`
              }
              description="GUI 更新需下载安装包重新安装，点击按钮前往 GitHub Releases 页面。"
            />
            {info.app_has_update && (
              <Button
                type="primary"
                icon={<ExternalLink size={14} />}
                onClick={handleOpenGuiRelease}
              >
                前往下载新版 GUI
              </Button>
            )}
          </Space>
        )}
      </Card>
    </div>
  );
}
