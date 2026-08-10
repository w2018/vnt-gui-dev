//! TCP JSON-RPC 服务端（一行一个 JSON，\n 分隔）

use std::sync::Arc;

use futures_util::{SinkExt, StreamExt};
use tokio::net::TcpListener;
use tokio::sync::Mutex;
use tokio_util::codec::{Framed, LinesCodec};
use tokio_util::sync::CancellationToken;

use super::rpc_protocol::{DaemonRequest, DaemonResponse};
use super::state_store::RuntimeState;
use super::{ftp_manager, vnt_manager};

/// 运行 RPC 服务（阻塞直到 shutdown 触发）
pub async fn run(addr: &str, state: Arc<Mutex<RuntimeState>>, shutdown: CancellationToken) {
    let listener = match TcpListener::bind(addr).await {
        Ok(l) => l,
        Err(e) => {
            tracing::error!("Daemon 绑定 RPC 端口失败 {}: {}", addr, e);
            return;
        }
    };
    tracing::info!("Daemon RPC listening on {}", addr);

    loop {
        tokio::select! {
            _ = shutdown.cancelled() => {
                tracing::info!("RPC 服务关闭（shutdown 触发）");
                break;
            }
            accepted = listener.accept() => {
                let (socket, _peer) = match accepted {
                    Ok(v) => v,
                    Err(e) => {
                        tracing::warn!("接受连接失败: {}", e);
                        continue;
                    }
                };
                let state = state.clone();
                let shutdown = shutdown.clone();
                tokio::spawn(async move {
                    handle_connection(socket, state, shutdown).await;
                });
            }
        }
    }
}

/// 处理单个连接（多个请求复用连接；断开不影响 daemon）
async fn handle_connection(
    socket: tokio::net::TcpStream,
    state: Arc<Mutex<RuntimeState>>,
    shutdown: CancellationToken,
) {
    let mut framed = Framed::new(socket, LinesCodec::new());
    while let Some(line) = framed.next().await {
        let line = match line {
            Ok(l) => l,
            Err(_) => break,
        };
        if line.trim().is_empty() {
            continue;
        }
        let req: DaemonRequest = match serde_json::from_str(&line) {
            Ok(r) => r,
            Err(e) => {
                let resp = serde_json::to_string(&DaemonResponse::Error {
                    code: "parse_error".into(),
                    message: e.to_string(),
                })
                .unwrap_or_default();
                if framed.send(resp).await.is_err() {
                    break;
                }
                continue;
            }
        };
        let (resp, should_shutdown) = handle_request(req, state.clone()).await;
        if let Ok(json) = serde_json::to_string(&resp) {
            if framed.send(json).await.is_err() {
                break;
            }
        }
        if should_shutdown {
            shutdown.cancel();
            break;
        }
    }
}

/// 处理请求：返回（响应，是否触发 daemon 退出）
async fn handle_request(
    req: DaemonRequest,
    state: Arc<Mutex<RuntimeState>>,
) -> (DaemonResponse, bool) {
    match req {
        DaemonRequest::Ping => {
            let uptime = state.lock().await.uptime_secs();
            (DaemonResponse::Pong { uptime_secs: uptime }, false)
        }
        DaemonRequest::GetState => {
            let s = state.lock().await;
            (
                DaemonResponse::State {
                    vnt_running: s.vnt_running,
                    vnt_connected: s.vnt_connected,
                    vnt_config: s.vnt_config.clone(),
                    ftp_running: s.ftp_running,
                    ftp_config: s.ftp_config.clone(),
                    peers: s.peers.clone(),
                    vnt_server_host: s.vnt_server_host.clone(),
                    vnt_virtual_ip: s.vnt_virtual_ip.clone(),
                    vnt_nat_type: s.vnt_nat_type.clone(),
                    uptime_secs: s.uptime_secs(),
                },
                false,
            )
        }
        DaemonRequest::VntStart { config } => {
            let resp = match vnt_manager::start(state.clone(), config).await {
                Ok(()) => DaemonResponse::Ok,
                Err(e) => DaemonResponse::Error {
                    code: "vnt_start_failed".into(),
                    message: e,
                },
            };
            (resp, false)
        }
        DaemonRequest::VntStop => {
            let resp = match vnt_manager::stop(state.clone()).await {
                Ok(()) => DaemonResponse::Ok,
                Err(e) => DaemonResponse::Error {
                    code: "vnt_stop_failed".into(),
                    message: e,
                },
            };
            (resp, false)
        }
        DaemonRequest::VntRestart { config } => {
            let resp = match vnt_manager::restart(state.clone(), config).await {
                Ok(()) => DaemonResponse::Ok,
                Err(e) => DaemonResponse::Error {
                    code: "vnt_restart_failed".into(),
                    message: e,
                },
            };
            (resp, false)
        }
        DaemonRequest::FtpStart { config } => {
            let resp = match ftp_manager::start(state.clone(), config).await {
                Ok(()) => DaemonResponse::Ok,
                Err(e) => DaemonResponse::Error {
                    code: "ftp_start_failed".into(),
                    message: e,
                },
            };
            (resp, false)
        }
        DaemonRequest::FtpStop => {
            let resp = match ftp_manager::stop(state.clone()).await {
                Ok(()) => DaemonResponse::Ok,
                Err(e) => DaemonResponse::Error {
                    code: "ftp_stop_failed".into(),
                    message: e,
                },
            };
            (resp, false)
        }
        DaemonRequest::FtpRestart { config } => {
            let resp = match ftp_manager::restart(state.clone(), config).await {
                Ok(()) => DaemonResponse::Ok,
                Err(e) => DaemonResponse::Error {
                    code: "ftp_restart_failed".into(),
                    message: e,
                },
            };
            (resp, false)
        }
        DaemonRequest::VntListPeers => {
            let peers = state.lock().await.peers.clone();
            (DaemonResponse::State {
                vnt_running: false,
                vnt_connected: false,
                vnt_config: None,
                ftp_running: false,
                ftp_config: None,
                peers,
                vnt_server_host: None,
                vnt_virtual_ip: None,
                vnt_nat_type: None,
                uptime_secs: 0,
            }, false)
        }
        DaemonRequest::FtpGetLogs => {
            let logs = ftp_manager::get_logs();
            match serde_json::to_value(&logs) {
                Ok(data) => (DaemonResponse::Event { event: "ftp_logs".into(), data }, false),
                Err(e) => (
                    DaemonResponse::Error {
                        code: "serialize_error".into(),
                        message: e.to_string(),
                    },
                    false,
                ),
            }
        }
        DaemonRequest::Shutdown => {
            // 先停服务再退出
            let _ = vnt_manager::stop(state.clone()).await;
            let _ = ftp_manager::stop(state.clone()).await;
            (DaemonResponse::Ok, true)
        }
    }
}
