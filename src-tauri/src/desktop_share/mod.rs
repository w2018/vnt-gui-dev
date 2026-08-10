//! 桌面共享模块入口
//!
//! 注册 Tauri 命令，管理全局状态与后台任务（接受循环 / 接收循环 / 捕获 / 心跳）

pub mod capture;
pub mod clipboard;
pub mod config;
pub mod error;
pub mod input;
pub mod mf_encoder;
pub mod network;
pub mod protocol;
pub mod session;

use std::net::IpAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use tokio::sync::mpsc;
use tokio::sync::Mutex;
use tauri::{AppHandle, Emitter, Manager, State};

use capture::{CaptureConfig, ScreenCapturer};
use config::DesktopShareConfig;
use network::{DesktopNetwork, RecvMessage};
use protocol::{ControlMsg, GrantedCapabilities, InputEvent, VideoFrameHeader};
use session::{SessionManager, SessionRole};

/// 视频帧通道载荷（前端 Channel 接收）
#[derive(Clone, serde::Serialize)]
pub struct VideoFramePayload {
    pub header: VideoFrameHeader,
    pub data: Vec<u8>,
}

/// 全局桌面共享状态
pub struct DesktopState {
    pub network: Arc<DesktopNetwork>,
    pub session: Arc<SessionManager>,
    pub input_simulator: Arc<input::InputSimulator>,
    pub clipboard: Arc<clipboard::ClipboardManager>,
    pub config: Mutex<DesktopShareConfig>,
    pub config_dir: PathBuf,
    /// 捕获线程（DXGI + MF 编码）
    pub capture_task: Mutex<Option<std::thread::JoinHandle<()>>>,
    /// 视频发送任务
    pub send_task: Mutex<Option<tokio::task::JoinHandle<()>>>,
    /// 接收循环任务
    pub recv_task: Mutex<Option<tokio::task::JoinHandle<()>>>,
    /// 心跳任务（控制端）
    pub heartbeat_task: Mutex<Option<tokio::task::JoinHandle<()>>>,
    /// 前端注册的视频帧通道
    pub video_channel: Mutex<Option<tauri::ipc::Channel<VideoFramePayload>>>,
    /// UDP 身份探测服务器任务
    pub probe_task: Mutex<Option<tokio::task::JoinHandle<()>>>,
    /// 接受循环任务
    pub accept_task: Mutex<Option<tokio::task::JoinHandle<()>>>,
}

// ==================== Tauri 命令 ====================

/// 初始化桌面共享（获取 VNT 虚拟 IP、绑定 Iroh、启动后台任务）；幂等
#[tauri::command]
pub async fn desktop_init(app: AppHandle) -> Result<(), String> {
    if app.try_state::<Arc<DesktopState>>().is_some() {
        return Ok(());
    }

    let vnt_ip = get_vnt_virtual_ip()
        .await
        .ok_or_else(|| "VNT 未连接，无法初始化桌面共享".to_string())?;

    let config_dir = config::config_dir();
    let cfg = config::load(&config_dir);

    let network = Arc::new(
        DesktopNetwork::new(0) // 0 = 随机空闲端口；QUIC 端口经探测公告
            .await
            .map_err(|e| e.to_string())?,
    );
    let session = Arc::new(SessionManager::new(network.clone()));
    let input_sim = Arc::new(input::InputSimulator::new().map_err(|e| e.to_string())?);
    let clipboard = Arc::new(clipboard::ClipboardManager::new().map_err(|e| e.to_string())?);

    let state = Arc::new(DesktopState {
        network,
        session,
        input_simulator: input_sim,
        clipboard,
        config: Mutex::new(cfg),
        config_dir,
        capture_task: Mutex::new(None),
        send_task: Mutex::new(None),
        recv_task: Mutex::new(None),
        heartbeat_task: Mutex::new(None),
        video_channel: Mutex::new(None),
        probe_task: Mutex::new(None),
        accept_task: Mutex::new(None),
    });
    // 注册为 Tauri 全局状态（缺失会导致后续命令 app.state() panic）
    app.manage(state.clone());

    // 启动 UDP 身份探测服务器（失败仅降级：他人无法探测连接本机，本机仍可作控制端）
    match state.network.start_probe_server().await {
        Ok(probe) => {
            *state.probe_task.lock().await = Some(probe);
        }
        Err(e) => {
            log::warn!("身份探测服务器启动失败（他人将无法连接本机）: {}", e);
        }
    }

    // 启动接受循环（被控端监听连接请求）
    let accept_state = state.clone();
    let accept_app = app.clone();
    let accept_task = tokio::spawn(async move {
        accept_loop(accept_state, accept_app).await;
    });
    *state.accept_task.lock().await = Some(accept_task);

    log::info!("桌面共享模块已初始化 (VNT IP: {})", vnt_ip);
    Ok(())
}

/// 本机连接信息（Node ID + 监听地址 + VNT IP）
#[derive(serde::Serialize)]
pub struct DesktopLocalInfo {
    pub node_id: String,
    pub listen_addr: String,
    pub vnt_ip: String,
}

#[tauri::command]
pub async fn desktop_get_local_info(app: AppHandle) -> Result<DesktopLocalInfo, String> {
    let state = app
        .try_state::<Arc<DesktopState>>()
        .ok_or_else(|| "桌面共享未初始化，请确认 VNT 已连接后重试".to_string())?;
    let vnt_ip = get_vnt_virtual_ip().await;
    let vnt_ip_str = vnt_ip.map(|i| i.to_string()).unwrap_or_default();
    Ok(DesktopLocalInfo {
        node_id: state.network.node_id(),
        listen_addr: format!("{}:{}", vnt_ip_str, state.network.bound_port()),
        vnt_ip: vnt_ip_str,
    })
}

/// 获取当前会话状态
#[tauri::command]
pub async fn desktop_get_session(app: AppHandle) -> Result<session::SessionInfo, String> {
    let state = app
        .try_state::<Arc<DesktopState>>()
        .ok_or_else(|| "桌面共享未初始化，请确认 VNT 已连接后重试".to_string())?;
    Ok(state.session.get_state().await)
}

/// 作为控制端：请求连接（remote_port 参数保留兼容，实际端口经 UDP 探测自动发现）
#[tauri::command]
pub async fn desktop_connect(
    app: AppHandle,
    remote_ip: String,
    _remote_port: u16,
    device_name: String,
    view_only: bool,
) -> Result<(), String> {
    let state = app
        .try_state::<Arc<DesktopState>>()
        .ok_or_else(|| "桌面共享未初始化，请确认 VNT 已连接后重试".to_string())?;
    let ip: IpAddr = remote_ip
        .trim()
        .parse()
        .map_err(|e| format!("无效 IP 地址: {}", e))?;

    let capabilities = protocol::ClientCapabilities {
        mouse: !view_only,
        keyboard: !view_only,
        clipboard: true,
        view_only,
    };

    let conn = state
        .session
        .request_connect(ip, device_name, capabilities)
        .await
        .map_err(|e| e.to_string())?;

    // 启动接收循环（视频/剪贴板/控制消息）与心跳
    spawn_controller_loops(state.inner(), &app, conn).await;
    emit_session(&app, &state).await;
    Ok(())
}

/// 作为被控端：接受连接请求
#[tauri::command]
pub async fn desktop_accept_request(
    app: AppHandle,
    grant_mouse: bool,
    grant_keyboard: bool,
    grant_clipboard: bool,
    view_only: bool,
) -> Result<(), String> {
    let state = app
        .try_state::<Arc<DesktopState>>()
        .ok_or_else(|| "桌面共享未初始化，请确认 VNT 已连接后重试".to_string())?;
    let granted = GrantedCapabilities {
        mouse: grant_mouse && !view_only,
        keyboard: grant_keyboard && !view_only,
        clipboard: grant_clipboard,
        view_only,
    };
    let screen = capture::get_screen_info().await;

    let conn = state
        .session
        .accept_pending(granted, screen)
        .await
        .map_err(|e| e.to_string())?;

    // 启动被控端接收循环（输入模拟 / 剪贴板 / 控制消息）
    spawn_host_loops(state.inner(), &app, conn, granted).await;
    // 自动开始屏幕共享：先清理可能残留的旧采集任务（对方断开未及时清理的兜底），再启动新采集
    stop_capture(state.inner()).await;
    start_capture(state.inner()).await?;
    emit_session(&app, &state).await;
    Ok(())
}

/// 作为被控端：拒绝连接请求
#[tauri::command]
pub async fn desktop_reject_request(app: AppHandle, reason: String) -> Result<(), String> {
    let state = app
        .try_state::<Arc<DesktopState>>()
        .ok_or_else(|| "桌面共享未初始化，请确认 VNT 已连接后重试".to_string())?;
    state
        .session
        .reject_pending(&reason)
        .await
        .map_err(|e| e.to_string())?;
    emit_session(&app, &state).await;
    Ok(())
}

/// 断开当前会话（并停止捕获）
#[tauri::command]
pub async fn desktop_disconnect(app: AppHandle, reason: String) -> Result<(), String> {
    let state = app
        .try_state::<Arc<DesktopState>>()
        .ok_or_else(|| "桌面共享未初始化，请确认 VNT 已连接后重试".to_string())?;
    stop_capture(state.inner()).await;
    // 清空视频通道引用：避免旧会话 channel 残留影响下次连接（二次连接黑屏防御）
    *state.video_channel.lock().await = None;
    state.session.disconnect(&reason).await;
    abort_loops(&state).await;
    emit_session(&app, &state).await;
    Ok(())
}

/// 开始共享屏幕（被控端；连接建立后通常已自动开始）
#[tauri::command]
pub async fn desktop_start_sharing(app: AppHandle) -> Result<(), String> {
    let state = app
        .try_state::<Arc<DesktopState>>()
        .ok_or_else(|| "桌面共享未初始化，请确认 VNT 已连接后重试".to_string())?;
    // 手动开始共享：先清理可能残留的旧采集任务，再启动
    stop_capture(state.inner()).await;
    start_capture(state.inner()).await?;
    emit_session(&app, &state).await;
    Ok(())
}

/// 停止共享屏幕：结束本次共享会话（停止采集 + 断开连接 + 中止后台循环）
/// 若只停采集，session.state 仍为 sharing，前端"停止共享"看起来无效
#[tauri::command]
pub async fn desktop_stop_sharing(app: AppHandle) -> Result<(), String> {
    let state = app
        .try_state::<Arc<DesktopState>>()
        .ok_or_else(|| "桌面共享未初始化，请确认 VNT 已连接后重试".to_string())?;
    stop_capture(state.inner()).await;
    state.session.disconnect("用户停止共享").await;
    abort_loops(&state).await;
    *state.video_channel.lock().await = None;
    emit_session(&app, &state).await;
    Ok(())
}

/// 注册视频帧通道（前端通过 invoke 传入 Channel）
#[tauri::command]
pub async fn desktop_set_video_channel(
    app: AppHandle,
    channel: tauri::ipc::Channel<VideoFramePayload>,
) -> Result<(), String> {
    let state = app
        .try_state::<Arc<DesktopState>>()
        .ok_or_else(|| "桌面共享未初始化，请确认 VNT 已连接后重试".to_string())?;
    *state.video_channel.lock().await = Some(channel);
    Ok(())
}

/// 发送输入事件（控制端 → 被控端；被控端角色则本地模拟）
#[tauri::command]
pub async fn desktop_send_input(app: AppHandle, event: InputEvent) -> Result<(), String> {
    let state = app
        .try_state::<Arc<DesktopState>>()
        .ok_or_else(|| "桌面共享未初始化，请确认 VNT 已连接后重试".to_string())?;
    let role = state.session.get_state().await.role;
    if role == SessionRole::Host {
        // 被控端：本地模拟（用于调试/本机操作）
        state.input_simulator.handle_event(event);
        Ok(())
    } else {
        let conn = state
            .session
            .get_connection()
            .await
            .ok_or_else(|| "没有活跃连接".to_string())?;
        conn.send_input(&event).await.map_err(|e| e.to_string())
    }
}

/// 保存配置
#[tauri::command]
pub async fn desktop_save_config(app: AppHandle, cfg: DesktopShareConfig) -> Result<(), String> {
    let state = app
        .try_state::<Arc<DesktopState>>()
        .ok_or_else(|| "桌面共享未初始化，请确认 VNT 已连接后重试".to_string())?;
    config::save(&state.config_dir, &cfg).map_err(|e| e.to_string())?;
    *state.config.lock().await = cfg;
    // 共享运行中：一次性重启采集，使新配置（fps/码率/分辨率/画质）立即生效
    // （不做每帧读配置，避免采集热路径开销）
    if state.capture_task.lock().await.is_some() {
        stop_capture(state.inner()).await;
        if let Err(e) = start_capture(state.inner()).await {
            log::error!("应用新采集配置失败: {}", e);
        }
    }
    Ok(())
}

/// 获取配置
#[tauri::command]
pub async fn desktop_get_config(app: AppHandle) -> Result<DesktopShareConfig, String> {
    let state = app
        .try_state::<Arc<DesktopState>>()
        .ok_or_else(|| "桌面共享未初始化，请确认 VNT 已连接后重试".to_string())?;
    let cfg = state.config.lock().await.clone();
    Ok(cfg)
}

/// 检查系统 H.264 编码器是否可用（Media Foundation，Windows 8+；N 版可能缺失）
#[tauri::command]
pub async fn desktop_check_encoder() -> Result<bool, String> {
    Ok(mf_encoder::is_encoder_available())
}

// ==================== 后台任务 ====================

/// 接受循环：监听连接请求 → emit 弹窗事件 → 超时自动拒绝
async fn accept_loop(state: Arc<DesktopState>, app: AppHandle) {
    loop {
        match state.session.handle_incoming().await {
            Ok(Some(info)) => {
                // 被控端关闭"允许被控制" → 直接拒绝，不打扰用户（弹窗仅当允许被控时出现）
                if !state.config.lock().await.allow_be_controlled {
                    log::info!(
                        "被控端已关闭允许被控制，拒绝 {} 的连接请求",
                        info.device_name
                    );
                    let _ = state.session.reject_pending("被控端已关闭允许被控制").await;
                    emit_session(&app, &state).await;
                    continue;
                }
                let timeout_secs = state.config.lock().await.confirm_timeout_secs;
                let _ = app.emit("desktop-connect-request", &info);
                log::info!("收到连接请求: {} ({})", info.device_name, info.client_node_id);
                // 超时自动拒绝
                let s = state.clone();
                let a = app.clone();
                tokio::spawn(async move {
                    tokio::time::sleep(Duration::from_secs(timeout_secs.max(5))).await;
                    if s.session.peek_pending().await.is_some() {
                        let _ = s.session.reject_pending("等待确认超时，已自动拒绝").await;
                        emit_session(&a, &s).await;
                    }
                });
            }
            Ok(None) => {}
            Err(e) => {
                log::warn!("接受循环错误: {}", e);
                tokio::time::sleep(Duration::from_millis(500)).await;
            }
        }
    }
}

/// 控制端接收循环：视频帧 → Channel；剪贴板 → 本地写入 + emit；控制消息
async fn spawn_controller_loops(
    state: &Arc<DesktopState>,
    app: &AppHandle,
    conn: Arc<network::DesktopConnection>,
) {
    let s = state.clone();
    let a = app.clone();
    let recv_conn = conn.clone();
    let recv_task = tokio::spawn(async move {
        loop {
            match recv_conn.recv_next().await {
                Ok(RecvMessage::Video(header, data)) => {
                    let channel = s.video_channel.lock().await.clone();
                    if let Some(ch) = channel {
                        let _ = ch.send(VideoFramePayload { header, data });
                    }
                }
                Ok(RecvMessage::Clipboard(text)) => {
                    if s.config.lock().await.clipboard_sync {
                        let _ = s.clipboard.write(text.clone()).await;
                    }
                    let _ = a.emit("desktop-clipboard", text);
                }
                Ok(RecvMessage::Control(ControlMsg::Ping { ts })) => {
                    let _ = recv_conn.send_control(&ControlMsg::Pong { ts }).await;
                }
                Ok(RecvMessage::Control(ControlMsg::Disconnect { reason })) => {
                    log::info!("对方断开: {}", reason);
                    s.session
                        .disconnect(&format!("对方已断开: {}", reason))
                        .await;
                    // 对方主动断开：清理本机采集/发送任务，否则重连时 start_capture 跳过导致黑屏
                    stop_capture(&s).await;
                    break;
                }
                Ok(_) => {}
                Err(e) => {
                    log::info!("接收循环结束: {}", e);
                    // 连接异常结束：同样清理采集/发送任务
                    stop_capture(&s).await;
                    break;
                }
            }
        }
        emit_session(&a, &s).await;
    });
    *state.recv_task.lock().await = Some(recv_task);

    // 心跳（每 5 秒）
    let hb_conn = conn.clone();
    let heartbeat = tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(5));
        interval.tick().await; // 跳过立即触发
        loop {
            interval.tick().await;
            let ts = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_millis() as u64)
                .unwrap_or(0);
            if hb_conn
                .send_control(&ControlMsg::Ping { ts })
                .await
                .is_err()
            {
                break;
            }
        }
    });
    *state.heartbeat_task.lock().await = Some(heartbeat);
}

/// 被控端接收循环：输入模拟 / 剪贴板写入 / 控制消息
async fn spawn_host_loops(
    state: &Arc<DesktopState>,
    app: &AppHandle,
    conn: Arc<network::DesktopConnection>,
    granted: GrantedCapabilities,
) {
    let s = state.clone();
    let a = app.clone();
    let recv_task = tokio::spawn(async move {
        loop {
            match conn.recv_next().await {
                Ok(RecvMessage::Input(event)) => {
                    // 按授予的权限过滤
                    let allowed = match &event {
                        InputEvent::MouseMove { .. }
                        | InputEvent::MouseButton { .. }
                        | InputEvent::MouseScroll { .. } => granted.mouse,
                        InputEvent::KeyDown { .. }
                        | InputEvent::KeyUp { .. }
                        | InputEvent::SpecialKey { .. } => granted.keyboard,
                        InputEvent::ClipboardText { .. } => granted.clipboard,
                    };
                    if allowed {
                        s.input_simulator.handle_event(event);
                    }
                }
                Ok(RecvMessage::Clipboard(text)) => {
                    if s.config.lock().await.clipboard_sync {
                        let _ = s.clipboard.write(text.clone()).await;
                    }
                    let _ = a.emit("desktop-clipboard", text);
                }
                Ok(RecvMessage::Control(ControlMsg::Ping { ts })) => {
                    let _ = conn.send_control(&ControlMsg::Pong { ts }).await;
                }
                Ok(RecvMessage::Control(ControlMsg::Disconnect { reason })) => {
                    log::info!("对方断开: {}", reason);
                    s.session
                        .disconnect(&format!("对方已断开: {}", reason))
                        .await;
                    // 对方主动断开：清理本机采集/发送任务，否则重连时 start_capture 跳过导致黑屏
                    stop_capture(&s).await;
                    break;
                }
                Ok(_) => {}
                Err(e) => {
                    log::info!("接收循环结束: {}", e);
                    // 连接异常结束：同样清理采集/发送任务
                    stop_capture(&s).await;
                    break;
                }
            }
        }
        emit_session(&a, &s).await;
    });
    *state.recv_task.lock().await = Some(recv_task);
}

/// 启动屏幕捕获 + 视频发送（DXGI 捕获 + MF 编码，零 ffmpeg 依赖）
async fn start_capture(state: &Arc<DesktopState>) -> Result<(), String> {
    if state.capture_task.lock().await.is_some() {
        return Ok(()); // 已在捕获
    }
    let conn = state
        .session
        .get_connection()
        .await
        .ok_or_else(|| "没有活跃连接".to_string())?;

    let cfg = state.config.lock().await.clone();
    // 编码器可用性检查（Windows N 版无 H.264 编码器）
    if !mf_encoder::is_encoder_available() {
        return Err("系统缺少 H.264 编码器（Windows N 版需安装媒体功能包）".to_string());
    }
    // 画质映射：原 CRF 语义（0-51，低=高质量）→ MF AVEncCommonQuality（0-100，高=高质量）
    let mf_quality = (51u32.saturating_sub(cfg.capture.quality.min(51)) * 100 / 51).clamp(1, 100);
    let capture_cfg = CaptureConfig {
        monitor: cfg.capture.monitor,
        fps: cfg.capture.fps,
        bitrate: cfg.capture.bitrate_kbps * 1000,
        width: cfg.capture.width,   // 目标输出宽度（0 = 原分辨率；缩放由 capture_loop 处理）
        height: cfg.capture.height, // 目标输出高度
        quality: mf_quality,
    };

    let (tx, mut rx) = mpsc::channel::<(VideoFrameHeader, Vec<u8>)>(60);
    let capturer = ScreenCapturer::new(capture_cfg);
    let task = capturer
        .start(tx)
        .map_err(|e| format!("启动屏幕捕获失败: {}", e))?;

    let send_conn = conn.clone();
    let send_task = tokio::spawn(async move {
        while let Some((header, data)) = rx.recv().await {
            if let Err(e) = send_conn.send_video(&header, &data).await {
                log::warn!("发送视频帧失败: {}", e);
                break;
            }
        }
    });

    *state.capture_task.lock().await = Some(task);
    *state.send_task.lock().await = Some(send_task);
    log::info!("屏幕共享已启动 (DXGI + Media Foundation)");
    Ok(())
}

/// 停止捕获与发送（先断开 rx 使捕获线程自然退出，再等待退出）
async fn stop_capture(state: &Arc<DesktopState>) {
    if let Some(t) = state.send_task.lock().await.take() {
        t.abort();
    }
    if let Some(t) = state.capture_task.lock().await.take() {
        // 捕获线程是独立 std::thread：spawn_blocking + 超时等待，避免同步 join 阻塞 tokio worker
        // （编码器缓冲期不触发 blocking_send 时，线程无法感知 rx 关闭，join 会挂起 command）
        // 超时后不再等待：线程在下次 blocking_send 检测到 channel 关闭后自行退出
        let _ = tokio::time::timeout(
            Duration::from_millis(1500),
            tokio::task::spawn_blocking(move || t.join()),
        )
        .await;
    }
}

/// 中止接收/心跳任务（断开时）
async fn abort_loops(state: &State<'_, Arc<DesktopState>>) {
    if let Some(t) = state.recv_task.lock().await.take() {
        t.abort();
    }
    if let Some(t) = state.heartbeat_task.lock().await.take() {
        t.abort();
    }
}

/// 向前端发射会话状态更新事件
async fn emit_session(app: &AppHandle, state: &Arc<DesktopState>) {
    let info = state.session.get_state().await;
    let _ = app.emit("desktop-session-update", info);
}

// ==================== VNT 虚拟 IP ====================

/// 从 daemon RPC 获取 VNT 虚拟 IP
async fn get_vnt_virtual_ip() -> Option<IpAddr> {
    use crate::daemon::rpc_protocol::DaemonResponse;
    match crate::daemon::rpc_client::get_state().await {
        Ok(DaemonResponse::State { vnt_virtual_ip, .. }) => vnt_virtual_ip?.parse().ok(),
        _ => None,
    }
}
