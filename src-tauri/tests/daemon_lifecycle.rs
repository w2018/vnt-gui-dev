//! Daemon 生命周期验证（V2/V3）
//!
//! V2：启动 daemon（随机测试端口）→ RPC ping → Shutdown → 断言连接断开
//! V3：客户端断开（模拟 GUI 退出）→ daemon 仍存活 → 重连 ping 成功

use std::sync::Arc;

use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

use vnt_gui_lib::daemon::rpc_client;
use vnt_gui_lib::daemon::rpc_protocol::{DaemonRequest, DaemonResponse};
use vnt_gui_lib::daemon::rpc_server;
use vnt_gui_lib::daemon::state_store::RuntimeState;

/// 在随机端口启动 daemon RPC 服务（返回 addr + 停止句柄）
async fn start_test_daemon() -> (String, Arc<Mutex<RuntimeState>>, CancellationToken, tokio::task::JoinHandle<()>) {
    // 随机空闲端口（并行测试互不冲突）
    let probe = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = probe.local_addr().unwrap().port();
    drop(probe);
    let addr = format!("127.0.0.1:{}", port);

    let state = Arc::new(Mutex::new(RuntimeState::default()));
    let shutdown = CancellationToken::new();
    let handle = {
        let state = state.clone();
        let shutdown = shutdown.clone();
        let addr = addr.clone();
        tokio::spawn(async move {
            rpc_server::run(&addr, state, shutdown).await;
        })
    };
    // 等待监听就绪
    for _ in 0..50 {
        if rpc_client::send_request_to(&addr, DaemonRequest::Ping).await.is_ok() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    (addr, state, shutdown, handle)
}

#[tokio::test]
async fn test_daemon_lifecycle() {
    // V2 要求：启动 daemon → ping → Pong → Shutdown → 连接断开
    let (addr, _state, _shutdown, handle) = start_test_daemon().await;

    // Ping → Pong
    let uptime = rpc_client::ping_to(&addr).await.expect("Ping 应成功");
    assert!(uptime >= 0);

    // GetState → 初始状态
    match rpc_client::get_state_to(&addr).await {
        Ok(DaemonResponse::State { vnt_running, ftp_running, .. }) => {
            assert!(!vnt_running);
            assert!(!ftp_running);
        }
        other => panic!("GetState 响应异常: {:?}", other),
    }

    // Shutdown → daemon 退出
    rpc_client::send_request_to(&addr, DaemonRequest::Shutdown)
        .await
        .expect("Shutdown 应成功");
    // 等 daemon 任务结束
    let _ = tokio::time::timeout(std::time::Duration::from_secs(3), handle).await;

    // 再 ping → 必须失败（daemon 已退出）
    let ping_after = rpc_client::send_request_to(&addr, DaemonRequest::Ping).await;
    assert!(ping_after.is_err(), "daemon 退出后 ping 必须失败: {:?}", ping_after);
}

#[tokio::test]
async fn test_daemon_survives_client_disconnect() {
    // V3 要求：模拟 GUI 断开（连接后直接 drop）→ daemon 仍存活 → 重连成功
    let (addr, _state, shutdown, handle) = start_test_daemon().await;

    // 第一次连接（模拟 GUI）→ 立即断开
    {
        let resp = rpc_client::send_request_to(&addr, DaemonRequest::Ping).await;
        assert!(resp.is_ok(), "首次连接应成功");
    } // drop 连接

    // 等待 1 秒（模拟 GUI 退出后的间隔）
    tokio::time::sleep(std::time::Duration::from_secs(1)).await;

    // 重新连接（模拟 GUI 再次打开）→ daemon 还活着
    let resp = rpc_client::send_request_to(&addr, DaemonRequest::Ping).await;
    assert!(resp.is_ok(), "daemon 必须在 GUI 断开后仍存活: {:?}", resp);

    // 清理
    shutdown.cancel();
    let _ = tokio::time::timeout(std::time::Duration::from_secs(3), handle).await;
}

#[tokio::test]
async fn test_daemon_handles_parse_error() {
    // 非法 JSON → daemon 返回 parse_error 且不崩溃
    let (addr, _state, shutdown, handle) = start_test_daemon().await;

    let resp = rpc_client::send_raw_to(&addr, "{not-json").await;
    assert!(resp.is_ok(), "非法请求应得到错误响应而非断连");
    if let Ok(r) = resp {
        match r {
            DaemonResponse::Error { code, .. } => assert_eq!(code, "parse_error"),
            other => panic!("应返回 parse_error: {:?}", other),
        }
    }

    // daemon 仍存活
    let resp = rpc_client::send_request_to(&addr, DaemonRequest::Ping).await;
    assert!(resp.is_ok(), "parse_error 后 daemon 必须仍存活");

    shutdown.cancel();
    let _ = tokio::time::timeout(std::time::Duration::from_secs(3), handle).await;
}
