// 更新页（文档 §3.9.3）：检查更新 → 下载进度 → 完成提示

import { useEffect, useState } from 'react';
import { Alert, Button, Card, Progress, Space, Typography, message } from 'antd';
import { RefreshCw } from 'lucide-react';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';
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
          message.success('更新完成，重启后生效');
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
      if (!result.has_update) {
        message.info('当前已是最新版本');
      }
    } catch (e) {
      setError(String(e));
    } finally {
      setChecking(false);
    }
  };

  const handleDownload = async () => {
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

  return (
    <Card
      title="软件更新"
      extra={
        <Button icon={<RefreshCw size={14} />} loading={checking} onClick={handleCheck}>
          检查更新
        </Button>
      }
      style={{ maxWidth: 640 }}
    >
      <Space direction="vertical" size="middle" style={{ width: '100%' }}>
        <Typography.Text type="secondary">
          更新内容：下载官方编译的最新 vnt-cli 二进制并替换本地文件（不会修改配置）
        </Typography.Text>

        {info && (
          <Alert
            type={info.has_update ? 'warning' : 'success'}
            showIcon
            message={
              info.has_update
                ? `发现新版 vnt-cli ${info.latest_version}（当前 ${info.current_version}）`
                : `vnt-cli 已是最新版本 ${info.current_version}`
            }
            description={`GUI 版本：${info.app_version}${info.app_has_update && info.app_latest_version ? `（发现新版 GUI ${info.app_latest_version}，请前往项目主页下载）` : ''}`}
            action={
              info.has_update &&
              info.download_url && (
                <Button size="small" type="primary" loading={downloading} onClick={handleDownload}>
                  {downloading ? '下载中...' : '立即更新'}
                </Button>
              )
            }
          />
        )}

        {downloading && progress !== null && (
          <Progress
            percent={Math.min(100, Math.round((progress / 10_000_000) * 100))}
            status="active"
            format={() => `${(progress / 1024 / 1024).toFixed(1)} MB`}
          />
        )}

        {error && <Alert type="error" showIcon message={error} />}
      </Space>
    </Card>
  );
}
