//! TCP JSON-RPC 客户端（GUI 侧使用）

use futures_util::{SinkExt, StreamExt};
use tokio::net::TcpStream;
use tokio_util::codec::{Framed, LinesCodec};

use super::rpc_protocol::{DaemonRequest, DaemonResponse, DAEMON_ADDR};

/// 发送请求并等待响应（每请求一个短连接）
async fn send_request(req: DaemonRequest) -> Result<DaemonResponse, String> {
    send_request_to(DAEMON_ADDR, req).await
}

/// 发送请求到指定地址（测试用）
pub async fn send_request_to(addr: &str, req: DaemonRequest) -> Result<DaemonResponse, String> {
    let stream = TcpStream::connect(addr)
        .await
        .map_err(|e| format!("连接 daemon 失败: {}", e))?;
    let mut framed = Framed::new(stream, LinesCodec::new());
    let json = serde_json::to_string(&req).map_err(|e| format!("序列化请求失败: {}", e))?;
    framed
        .send(json)
        .await
        .map_err(|e| format!("发送请求失败: {}", e))?;
    // 等待响应（超时保护）
    match tokio::time::timeout(std::time::Duration::from_secs(10), framed.next()).await {
        Ok(Some(Ok(line))) => serde_json::from_str(&line).map_err(|e| format!("解析响应失败: {}", e)),
        Ok(Some(Err(e))) => Err(format!("读取响应失败: {}", e)),
        Ok(None) => Err("daemon 未返回响应（连接关闭）".to_string()),
        Err(_) => Err("等待 daemon 响应超时".to_string()),
    }
}

/// 发送原始行（测试 parse_error 路径）
pub async fn send_raw_to(addr: &str, raw: &str) -> Result<DaemonResponse, String> {
    let stream = TcpStream::connect(addr)
        .await
        .map_err(|e| format!("连接 daemon 失败: {}", e))?;
    let mut framed = Framed::new(stream, LinesCodec::new());
    framed
        .send(raw.to_string())
        .await
        .map_err(|e| format!("发送失败: {}", e))?;
    match tokio::time::timeout(std::time::Duration::from_secs(5), framed.next()).await {
        Ok(Some(Ok(line))) => serde_json::from_str(&line).map_err(|e| format!("解析响应失败: {}", e)),
        _ => Err("无响应".to_string()),
    }
}

/// 健康检查（指定地址，测试用）
pub async fn ping_to(addr: &str) -> Result<u64, String> {
    match send_request_to(addr, DaemonRequest::Ping).await {
        Ok(DaemonResponse::Pong { uptime_secs }) => Ok(uptime_secs),
        Ok(other) => Err(format!("意外响应: {:?}", other)),
        Err(e) => Err(e),
    }
}

/// 获取状态快照（指定地址，测试用）
pub async fn get_state_to(addr: &str) -> Result<DaemonResponse, String> {
    send_request_to(addr, DaemonRequest::GetState).await
}

/// 健康检查
pub async fn ping() -> Result<u64, String> {
    match send_request(DaemonRequest::Ping).await {
        Ok(DaemonResponse::Pong { uptime_secs }) => Ok(uptime_secs),
        Ok(other) => Err(format!("意外响应: {:?}", other)),
        Err(e) => Err(e),
    }
}

/// 获取完整状态快照
pub async fn get_state() -> Result<DaemonResponse, String> {
    send_request(DaemonRequest::GetState).await
}

/// 启动 VNT
pub async fn vnt_start(config: crate::config::VntConfig) -> Result<(), String> {
    send_request(DaemonRequest::VntStart { config })
        .await?
        .into_result()
        .map(|_| ())
}

/// 停止 VNT
pub async fn vnt_stop() -> Result<(), String> {
    send_request(DaemonRequest::VntStop)
        .await?
        .into_result()
        .map(|_| ())
}

/// 重启 VNT
pub async fn vnt_restart(config: crate::config::VntConfig) -> Result<(), String> {
    send_request(DaemonRequest::VntRestart { config })
        .await?
        .into_result()
        .map(|_| ())
}

/// 启动 FTP（密码经 FtpConfigWithSecrets 传输，daemon 不跨进程读 keyring）
pub async fn ftp_start(config: crate::daemon::rpc_protocol::FtpConfigWithSecrets) -> Result<(), String> {
    send_request(DaemonRequest::FtpStart { config })
        .await?
        .into_result()
        .map(|_| ())
}

/// 停止 FTP
pub async fn ftp_stop() -> Result<(), String> {
    send_request(DaemonRequest::FtpStop)
        .await?
        .into_result()
        .map(|_| ())
}

/// 重启 FTP（密码经 FtpConfigWithSecrets 传输，daemon 不跨进程读 keyring）
pub async fn ftp_restart(config: crate::daemon::rpc_protocol::FtpConfigWithSecrets) -> Result<(), String> {
    send_request(DaemonRequest::FtpRestart { config })
        .await?
        .into_result()
        .map(|_| ())
}

/// 获取设备列表
pub async fn vnt_list_peers() -> Result<Vec<crate::state::PeerInfo>, String> {
    match send_request(DaemonRequest::VntListPeers).await? {
        DaemonResponse::State { peers, .. } => Ok(peers),
        other => Err(format!("意外响应: {:?}", other)),
    }
}

/// 获取 FTP 连接日志
pub async fn ftp_get_logs() -> Result<Vec<crate::ftp::log::FtpLogEntry>, String> {
    match send_request(DaemonRequest::FtpGetLogs).await? {
        DaemonResponse::Event { data, .. } => {
            serde_json::from_value(data).map_err(|e| format!("解析日志失败: {}", e))
        }
        other => Err(format!("意外响应: {:?}", other)),
    }
}

/// 获取 daemon 运行日志（VNT 实时日志，新→旧）
pub async fn vnt_get_logs() -> Result<Vec<crate::state::LogEntry>, String> {
    match send_request(DaemonRequest::VntGetLogs).await? {
        DaemonResponse::Logs { entries } => Ok(entries),
        other => Err(format!("意外响应: {:?}", other)),
    }
}

/// 清空 daemon 运行日志
pub async fn vnt_clear_logs() -> Result<(), String> {
    match send_request(DaemonRequest::VntClearLogs).await? {
        DaemonResponse::Ok => Ok(()),
        other => Err(format!("意外响应: {:?}", other)),
    }
}

/// 优雅关闭 daemon（完全退出）
pub async fn shutdown_daemon() -> Result<(), String> {
    let _ = send_request(DaemonRequest::Shutdown).await;
    Ok(())
}
