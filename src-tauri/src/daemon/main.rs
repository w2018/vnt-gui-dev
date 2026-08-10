//! vnt-daemon 独立入口（纯 Rust 后台服务，不依赖 Tauri）
//!
//! 职责：管理 vnt-cli 与 FTP 服务生命周期，提供 TCP JSON-RPC；
//! GUI 退出不影响服务运行；重启后按持久化状态自动恢复。

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::sync::Arc;

use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

use vnt_gui_lib::daemon::{
    pid_file, rpc_server, state_store, vnt_manager,
};

#[tokio::main]
async fn main() {
    // 日志写入文件（<安装目录>/logs/vnt-daemon.log），不占用控制台
    let log_path = vnt_gui_lib::config::log_dir().join("vnt-daemon.log");
    if let Ok(file) = std::fs::File::create(&log_path) {
        use tracing_subscriber::prelude::*;
        // EnvFilter 作为独立过滤层 + 文件层 + 内存缓冲层（GUI 经 RPC 拉取实时日志）
        let filter =
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| {
                    // 关闭 iroh-net 内部高频日志（upnp/actor/rtt 端口映射探测等），保留其他 info
                    tracing_subscriber::EnvFilter::new("info,iroh_net=warn")
                });
        let file_layer = tracing_subscriber::fmt::layer().with_writer(file);
        let _ = tracing_subscriber::registry()
            .with(filter)
            .with(file_layer)
            .with(vnt_gui_lib::daemon::rt_log::RtLogLayer)
            .try_init();
        // 桥接 log crate（ftp/server.rs 等使用 log::info!）
        let _ = tracing_log::LogTracer::init();
    }
    tracing::info!("VNT Daemon starting (pid={})", std::process::id());

    // 写 PID 文件（失败不阻塞：仅影响 GUI 的存活检测）
    if let Err(e) = pid_file::write_current_pid() {
        tracing::warn!("写 PID 文件失败: {}", e);
    }

    // 加载持久化状态
    let state = Arc::new(Mutex::new(state_store::load_or_init().await));

    // 恢复上次运行的服务（服务不能断）
    restore_services(state.clone()).await;

    // 启动 RPC 服务（阻塞至 Shutdown）
    let shutdown = CancellationToken::new();
    let addr = vnt_gui_lib::daemon::rpc_protocol::DAEMON_ADDR;
    rpc_server::run(addr, state.clone(), shutdown.clone()).await;

    // 清理
    let _ = pid_file::remove();
    tracing::info!("VNT Daemon shutting down");
}

/// 按持久化状态恢复服务：VNT（was_running）+ FTP（was_running 且自启/启用）
async fn restore_services(state: Arc<Mutex<state_store::RuntimeState>>) {
    // VNT：上次在运行 → 自动拉起
    {
        let s = state.lock().await;
        if s.vnt_was_running {
            if let Some(cfg) = s.vnt_config.clone() {
                drop(s);
                if let Err(e) = vnt_manager::start(state.clone(), cfg).await {
                    tracing::error!("恢复 VNT 失败: {}", e);
                } else {
                    tracing::info!("已恢复 VNT 服务");
                }
            }
        }
    }
    // FTP：上次在运行且随应用自启（或总开关开启）→ 自动拉起
    // 密码来源：daemon 侧 keyring（start 时已写入）；读不到 → 明确报错不静默
    {
        let s = state.lock().await;
        if s.ftp_was_running {
            if let Some(cfg) = s.ftp_config.clone() {
                if cfg.auto_start_with_app || cfg.enabled {
                    drop(s);
                    if let Err(e) = vnt_gui_lib::daemon::ftp_manager::start_restored(state.clone(), cfg).await {
                        tracing::error!("恢复 FTP 失败: {}", e);
                    } else {
                        tracing::info!("已恢复 FTP 服务");
                    }
                }
            }
        }
    }
}
