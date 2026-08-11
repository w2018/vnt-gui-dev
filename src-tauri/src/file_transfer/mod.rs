//! 文件传输模块入口
//!
//! 按开发文档新增：文件流（QUIC < 阈值）+ 裸 TCP 大文件通道（≥ 阈值，断点续传）。
//! 架构决策：
//! - Iroh Endpoint 独立创建（不修改 desktop_share/：其 accept() 已被桌面共享独占）
//! - 后台监听在 GUI 进程（daemon 无 AppHandle 无法 emit；GUI 常驻托盘即"后台"）

use std::collections::HashMap;
use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicU16;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use tauri::{AppHandle, Emitter, Manager};
use tokio::sync::Mutex;

pub mod config;
pub mod daemon_listener;
pub mod filter;
pub mod history;
pub mod network;
pub mod protocol;
pub mod receiver;
pub mod resumable;
pub mod sender;
pub mod tcp_channel;
pub mod transfer_manager;

use network::FileNetwork;
use protocol::{FileCancel, FileMsg, TransferChannel, TransferDirection, TransferStatus};
use transfer_manager::{CancelFlag, TransferManager, TransferTask};

/// 默认保存目录子目录名（数据目录下）
pub const SAVE_DIR_NAME: &str = "file_transfers";
/// 通道阈值（字节，≥ 此值走 TCP）：100MB
pub const DEFAULT_THRESHOLD: u64 = 100 * 1024 * 1024;
/// 默认最大并发传输数
pub const DEFAULT_MAX_CONCURRENT: usize = 3;
/// 接收确认超时（秒，弹窗倒计时展示）
pub const CONFIRM_TIMEOUT_SECS: u64 = 30;

/// 接收确认决策（弹窗 → 用户操作）
#[derive(Debug, Clone)]
pub enum ReceiveDecision {
    Accept { save_path: PathBuf },
    Reject { reason: String },
}

/// 待确认的接收请求
pub struct PendingReceive {
    pub offer: protocol::FileOffer,
    pub resume_offset: u64,
    pub partial_path: PathBuf,
    pub default_save_path: PathBuf,
    /// 确认通道（file_accept / file_reject 发送决策）
    pub confirm: tokio::sync::oneshot::Sender<ReceiveDecision>,
}

/// 弹窗载荷（前端 file-transfer-offer 事件）
#[derive(Debug, Clone, serde::Serialize)]
pub struct FileOfferPayload {
    pub transfer_id: u64,
    pub filename: String,
    pub file_size: u64,
    pub channel: TransferChannel,
    pub remote_ip: String,
    pub remote_device: String,
    pub resume_offset: u64,
    pub default_save_path: String,
}

/// 文件传输全局状态
pub struct FileTransferState {
    /// 文件传输专用 Iroh Endpoint
    pub network: Arc<FileNetwork>,
    pub transfer_mgr: Arc<TransferManager>,
    pub history: Arc<history::HistoryStore>,
    pub filter: Mutex<filter::FileTypeFilter>,
    pub auto_accept: Mutex<bool>,
    pub threshold: Mutex<u64>,
    pub config_dir: PathBuf,
    /// 默认保存目录（用户可改，持久化）
    pub save_dir: Mutex<PathBuf>,
    /// 待确认接收请求
    pub pending: Mutex<HashMap<u64, PendingReceive>>,
    /// 传输取消标记
    pub cancel_flags: Mutex<HashMap<u64, CancelFlag>>,
    /// TCP 高速通道实际监听端口（daemon_listener 动态写入，UDP 探测公告）
    pub tcp_port: Arc<AtomicU16>,
}

/// 数据目录（与项目其他配置同目录）
pub fn config_dir() -> PathBuf {
    crate::config::get_config_path()
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .to_path_buf()
}

/// 新建接收方向的任务（监听循环用）
pub fn new_receive_task(
    offer: &protocol::FileOffer,
    remote_ip: &str,
    resume_offset: u64,
    save_path: &Path,
) -> TransferTask {
    TransferTask {
        transfer_id: 0, // enqueue 时分配
        filename: offer.filename.clone(),
        file_size: offer.file_size,
        bytes_done: resume_offset,
        direction: TransferDirection::Receive,
        channel: offer.channel,
        status: TransferStatus::Pending,
        remote_ip: remote_ip.to_string(),
        remote_device: offer.sender_device.clone(),
        error_message: None,
        speed_kbps: None,
        eta_seconds: None,
        created_at: 0, // enqueue 时填充
        file_path: None,
        save_path: Some(save_path.to_string_lossy().to_string()),
        resume_offset,
        quick_sent: false,
        last_progress_time: None,
    }
}

/// 新建发送方向的任务（发送命令用）
pub fn new_send_task(
    filename: String,
    file_size: u64,
    remote_ip: &str,
    remote_device: &str,
    channel: TransferChannel,
    file_path: &str,
) -> TransferTask {
    TransferTask {
        transfer_id: 0,
        filename,
        file_size,
        bytes_done: 0,
        direction: TransferDirection::Send,
        channel,
        status: TransferStatus::Pending,
        remote_ip: remote_ip.to_string(),
        remote_device: remote_device.to_string(),
        error_message: None,
        speed_kbps: None,
        eta_seconds: None,
        created_at: 0,
        file_path: Some(file_path.to_string()),
        save_path: None,
        resume_offset: 0,
        quick_sent: false,
        last_progress_time: None,
    }
}

// ==================== Tauri 命令 ====================

/// 初始化文件传输模块（幂等）
#[tauri::command]
pub async fn file_init(app: AppHandle) -> Result<(), String> {
    if app.try_state::<Arc<FileTransferState>>().is_some() {
        return Ok(());
    }
    let cfg_dir = config_dir();

    // 加载持久化设置（过滤/自动接收/阈值/默认保存目录）
    let settings = config::load(&cfg_dir);
    let save_dir = if settings.save_dir.trim().is_empty() {
        cfg_dir.join(SAVE_DIR_NAME)
    } else {
        PathBuf::from(&settings.save_dir)
    };
    std::fs::create_dir_all(&save_dir).map_err(|e| format!("创建接收目录失败: {}", e))?;

    // 独立 Iroh Endpoint（随机端口，经 UDP 探测公告；他人可探测发现实际端口）
    let network = Arc::new(FileNetwork::new(0).await?);
    // TCP 大文件监听端口共享句柄（daemon_listener 写入实际端口，探测响应公告）
    let tcp_port = network.tcp_port_hint();
    if let Err(e) = network.start_probe_server().await {
        log::warn!("文件传输探测服务器启动失败（他人将无法连接本机）: {}", e);
    }

    let transfer_mgr = Arc::new(TransferManager::new(DEFAULT_MAX_CONCURRENT, settings.threshold));
    let history = Arc::new(history::HistoryStore::load(&cfg_dir));

    let state = Arc::new(FileTransferState {
        network,
        transfer_mgr,
        history,
        filter: Mutex::new(config::to_filter(&settings)),
        auto_accept: Mutex::new(settings.auto_accept),
        threshold: Mutex::new(settings.threshold),
        config_dir: cfg_dir,
        save_dir: Mutex::new(save_dir),
        pending: Mutex::new(HashMap::new()),
        cancel_flags: Mutex::new(HashMap::new()),
        tcp_port,
    });
    app.manage(state.clone());

    // 启动后台监听（QUIC + TCP 大文件通道）
    daemon_listener::start_listeners(state.clone(), app.clone());

    log::info!("文件传输模块已初始化");
    Ok(())
}

/// 发送文件（加入队列后台传输，自动选择通道）
#[tauri::command]
pub async fn file_send(
    app: AppHandle,
    file_path: String,
    remote_ip: String,
) -> Result<(), String> {
    let state = app
        .try_state::<Arc<FileTransferState>>()
        .map(|s| (*s).clone())
        .ok_or_else(|| "文件传输未初始化".to_string())?;

    // 1. 校验文件并读取大小
    let meta = tokio::fs::metadata(&file_path)
        .await
        .map_err(|e| format!("文件不存在或不可读: {}", e))?;
    if !meta.is_file() {
        return Err("路径不是文件".to_string());
    }
    let file_size = meta.len();
    let filename = Path::new(&file_path)
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| file_path.clone());

    // 2. 选择通道 + 创建设备名
    let mut channel = state.transfer_mgr.select_channel(file_size);
    // TCP 高速通道：端口动态随机（接收端每次启动不同），发送前经 UDP 探测获取实际端口
    if channel.is_tcp() {
        let ip: IpAddr = remote_ip
            .trim()
            .parse()
            .map_err(|e| format!("无效 IP 地址: {}", e))?;
        let info = state.network.probe(ip).await.map_err(|e| format!("探测对方 TCP 通道失败: {}", e))?;
        if info.tcp_port == 0 {
            return Err(
                "对方文件接收的 TCP 高速通道尚未就绪（接收端未连接虚拟局域网或服务未启动），请稍后重试"
                    .to_string(),
            );
        }
        channel = TransferChannel::Tcp { port: info.tcp_port };
        log::info!(
            "大文件 {} 使用 TCP 高速通道，对方端口 {}",
            filename,
            info.tcp_port
        );
    }
    let remote_device = lookup_device_name(&remote_ip).await;

    // 3. 入队 + emit（filename/remote_ip 保留副本用于日志）
    let log_filename = filename.clone();
    let log_remote_ip = remote_ip.clone();
    let task_id = state
        .transfer_mgr
        .enqueue(new_send_task(
            filename,
            file_size,
            &remote_ip,
            &remote_device,
            channel,
            &file_path,
        ))
        .await;
    emit_task(&app, &state, task_id).await;

    log::info!(
        "已加入发送队列: {} ({} 字节, {}) -> {}",
        log_filename,
        file_size,
        if channel.is_tcp() { "TCP" } else { "QUIC" },
        log_remote_ip
    );

    // 4. 后台发送协程（move 各参数）
    let cancel = CancelFlag::new();
    state.cancel_flags.lock().await.insert(task_id, cancel.clone());
    let s = state.clone();
    let a = app.clone();
    let run_file_path = file_path.clone();
    let run_remote_ip = remote_ip.clone();
    tokio::spawn(async move {
        run_send(
            &s,
            &a,
            &run_file_path,
            &run_remote_ip,
            channel,
            task_id,
            cancel,
        )
        .await;
    });
    Ok(())
}

/// 批量发送文件（列队串行）
#[tauri::command]
pub async fn file_send_batch(
    app: AppHandle,
    file_paths: Vec<String>,
    remote_ip: String,
) -> Result<(), String> {
    for path in &file_paths {
        file_send(app.clone(), path.clone(), remote_ip.clone()).await?;
    }
    Ok(())
}

/// 发送文本（走 QUIC 控制流，即时到达）
#[tauri::command]
pub async fn file_send_text(
    app: AppHandle,
    text: String,
    remote_ip: String,
) -> Result<(), String> {
    let state = app
        .try_state::<Arc<FileTransferState>>()
        .ok_or_else(|| "文件传输未初始化".to_string())?;
    let text = text.trim().to_string();
    if text.is_empty() {
        return Err("文本为空".to_string());
    }
    let ip: IpAddr = remote_ip
        .trim()
        .parse()
        .map_err(|e| format!("无效 IP 地址: {}", e))?;
    let conn = state.network.connect(ip).await?;
    let msg = protocol::TextMessage {
        msg_id: unix_now_millis(),
        timestamp: transfer_manager::unix_now(),
        text,
        from: local_device_name(),
    };
    conn.send_msg(&protocol::FileMsg::Text(msg)).await?;
    conn.close("text sent");
    Ok(())
}

/// 接受文件（用户确认后发送决策，指定保存路径）
#[tauri::command]
pub async fn file_accept(
    app: AppHandle,
    transfer_id: u64,
    save_path: String,
) -> Result<(), String> {
    let state = app
        .try_state::<Arc<FileTransferState>>()
        .ok_or_else(|| "文件传输未初始化".to_string())?;
    let entry = state
        .pending
        .lock()
        .await
        .remove(&transfer_id)
        .ok_or_else(|| "请求不存在或已过期".to_string())?;
    entry
        .confirm
        .send(ReceiveDecision::Accept { save_path: PathBuf::from(save_path) })
        .map_err(|_| "确认通道已关闭".to_string())?;
    Ok(())
}

/// 拒绝文件
#[tauri::command]
pub async fn file_reject(
    app: AppHandle,
    transfer_id: u64,
    reason: String,
) -> Result<(), String> {
    let state = app
        .try_state::<Arc<FileTransferState>>()
        .ok_or_else(|| "文件传输未初始化".to_string())?;
    let entry = state
        .pending
        .lock()
        .await
        .remove(&transfer_id)
        .ok_or_else(|| "请求不存在或已过期".to_string())?;
    entry
        .confirm
        .send(ReceiveDecision::Reject { reason })
        .map_err(|_| "确认通道已关闭".to_string())?;
    Ok(())
}

/// 暂停传输（不终止、不写历史）：中断连接，接收端保留 .partial 断点，
/// 任务保持「已暂停」状态留在传输中列表，点「继续」可断点续传。
#[tauri::command]
pub async fn file_pause(
    app: AppHandle,
    transfer_id: u64,
) -> Result<(), String> {
    let state = app
        .try_state::<Arc<FileTransferState>>()
        .ok_or_else(|| "文件传输未初始化".to_string())?;

    // 1. 先置为暂停状态（保留在传输中，不写历史）。
    //    ⚠️ 必须先置状态再 cancel：run_send 协程在 send_file 返回后依据任务状态
    //    判断是否暂停，若先 cancel 后置状态，run_send 可能读到未置 Paused 的旧状态，
    //    误走失败分支（finish_send），导致接收端收到 closed by peer 而非 Cancel("已暂停")，
    //    误删 .partial 断点，继续时变成重新发送。
    state
        .transfer_mgr
        .update(transfer_id, |t| {
            t.status = TransferStatus::Paused;
            t.error_message = Some("已暂停，点「继续」可断点续传".to_string());
            t.speed_kbps = None;
            t.eta_seconds = None;
        })
        .await;
    // 2. 触发进行中传输的取消标记（断开连接，接收端 .partial 保留）
    if let Some(flag) = state.cancel_flags.lock().await.get(&transfer_id) {
        flag.cancel();
    }
    // 3. 未确认的接收请求 → 拒绝（断开）
    if let Some(entry) = state.pending.lock().await.remove(&transfer_id) {
        let _ = entry
            .confirm
            .send(ReceiveDecision::Reject { reason: "发送端已暂停".to_string() });
    }
    if let Some(task) = state.transfer_mgr.find(transfer_id).await {
        let _ = app.emit("file-transfer-update", &task);
    }
    Ok(())
}

/// 取消传输
#[tauri::command]
pub async fn file_cancel(
    app: AppHandle,
    transfer_id: u64,
    reason: String,
) -> Result<(), String> {
    let state = app
        .try_state::<Arc<FileTransferState>>()
        .ok_or_else(|| "文件传输未初始化".to_string())?;

    // 1. 触发进行中传输的取消标记
    if let Some(flag) = state.cancel_flags.lock().await.get(&transfer_id) {
        flag.cancel();
    }
    // 2. 未确认的接收请求 → 拒绝
    if let Some(entry) = state.pending.lock().await.remove(&transfer_id) {
        let _ = entry.confirm.send(ReceiveDecision::Reject { reason: reason.clone() });
    }
    // 3. 更新任务状态
    state
        .transfer_mgr
        .update(transfer_id, |t| {
            t.status = TransferStatus::Cancelled;
            t.error_message = Some(reason.clone());
        })
        .await;
    if let Some(task) = state.transfer_mgr.find(transfer_id).await {
        let _ = app.emit("file-transfer-update", &task);
    }
    Ok(())
}

/// 从传输列表移除任务（仅移除列表记录，不删除文件、不影响历史记录）
#[tauri::command]
pub async fn file_remove_task(
    app: AppHandle,
    transfer_id: u64,
) -> Result<(), String> {
    let state = app
        .try_state::<Arc<FileTransferState>>()
        .ok_or_else(|| "文件传输未初始化".to_string())?;
    // 清理该任务的取消标记与待确认项（若仍在进行中）
    state.cancel_flags.lock().await.remove(&transfer_id);
    state.pending.lock().await.remove(&transfer_id);
    state.transfer_mgr.remove(transfer_id).await;
    Ok(())
}

/// 获取传输任务列表
#[tauri::command]
pub async fn file_get_transfers(app: AppHandle) -> Result<Vec<TransferTask>, String> {
    let state = app
        .try_state::<Arc<FileTransferState>>()
        .ok_or_else(|| "文件传输未初始化".to_string())?;
    Ok(state.transfer_mgr.snapshot().await)
}

/// 获取历史记录
#[tauri::command]
pub async fn file_get_history(
    app: AppHandle,
    direction: Option<TransferDirection>,
    keyword: Option<String>,
    limit: Option<usize>,
) -> Result<Vec<protocol::TransferRecord>, String> {
    let state = app
        .try_state::<Arc<FileTransferState>>()
        .ok_or_else(|| "文件传输未初始化".to_string())?;
    Ok(state
        .history
        .query(direction, keyword.as_deref(), limit.unwrap_or(100))
        .await)
}

/// 删除单条历史（按持久化自增 id）
#[tauri::command]
pub async fn file_delete_history(
    app: AppHandle,
    id: u64,
) -> Result<(), String> {
    let state = app
        .try_state::<Arc<FileTransferState>>()
        .ok_or_else(|| "文件传输未初始化".to_string())?;
    state.history.delete(id).await
}

/// 批量删除历史（按持久化自增 id）
#[tauri::command]
pub async fn file_delete_history_batch(
    app: AppHandle,
    ids: Vec<u64>,
) -> Result<usize, String> {
    let state = app
        .try_state::<Arc<FileTransferState>>()
        .ok_or_else(|| "文件传输未初始化".to_string())?;
    state.history.delete_many(&ids).await
}

/// 清空历史
#[tauri::command]
pub async fn file_clear_history(app: AppHandle) -> Result<(), String> {
    let state = app
        .try_state::<Arc<FileTransferState>>()
        .ok_or_else(|| "文件传输未初始化".to_string())?;
    state.history.clear_all().await
}

/// 获取过滤器
#[tauri::command]
pub async fn file_get_filter(app: AppHandle) -> Result<filter::FileTypeFilter, String> {
    let state = app
        .try_state::<Arc<FileTransferState>>()
        .ok_or_else(|| "文件传输未初始化".to_string())?;
    let filter = state.filter.lock().await.clone();
    Ok(filter)
}

/// 保存过滤器
#[tauri::command]
pub async fn file_save_filter(
    app: AppHandle,
    filter: filter::FileTypeFilter,
) -> Result<(), String> {
    let state = app
        .try_state::<Arc<FileTransferState>>()
        .ok_or_else(|| "文件传输未初始化".to_string())?;
    *state.filter.lock().await = filter;
    persist_settings(&**state).await
}

/// 获取通道阈值（字节）
#[tauri::command]
pub async fn file_get_threshold(app: AppHandle) -> Result<u64, String> {
    let state = app
        .try_state::<Arc<FileTransferState>>()
        .ok_or_else(|| "文件传输未初始化".to_string())?;
    let threshold = *state.threshold.lock().await;
    Ok(threshold)
}

/// 设置通道阈值（字节，立即生效）
#[tauri::command]
pub async fn file_set_threshold(
    app: AppHandle,
    bytes: u64,
) -> Result<(), String> {
    let state = app
        .try_state::<Arc<FileTransferState>>()
        .ok_or_else(|| "文件传输未初始化".to_string())?;
    *state.threshold.lock().await = bytes;
    state.transfer_mgr.set_size_threshold(bytes);
    persist_settings(&**state).await
}

/// 设置自动接收
#[tauri::command]
pub async fn file_set_auto_accept(
    app: AppHandle,
    enabled: bool,
) -> Result<(), String> {
    let state = app
        .try_state::<Arc<FileTransferState>>()
        .ok_or_else(|| "文件传输未初始化".to_string())?;
    *state.auto_accept.lock().await = enabled;
    log::info!("文件自动接收已{}", if enabled { "开启" } else { "关闭" });
    persist_settings(&**state).await
}

/// 获取全部文件传输设置（过滤/自动接收/阈值/默认保存目录）
#[tauri::command]
pub async fn file_get_settings(app: AppHandle) -> Result<config::FileTransferSettings, String> {
    let state = app
        .try_state::<Arc<FileTransferState>>()
        .ok_or_else(|| "文件传输未初始化".to_string())?;
    let filter = state.filter.lock().await.clone();
    let auto_accept = *state.auto_accept.lock().await;
    let threshold = *state.threshold.lock().await;
    let save_dir = state.save_dir.lock().await.clone();
    Ok(config::FileTransferSettings {
        mode: filter.mode,
        extensions: filter.extensions.into_iter().collect(),
        auto_accept,
        threshold,
        save_dir: save_dir.to_string_lossy().to_string(),
    })
}

/// 设置默认保存目录（接收文件存放位置）
#[tauri::command]
pub async fn file_set_save_dir(app: AppHandle, path: String) -> Result<(), String> {
    let state = app
        .try_state::<Arc<FileTransferState>>()
        .ok_or_else(|| "文件传输未初始化".to_string())?;
    let path = path.trim();
    if path.is_empty() {
        return Err("保存路径为空".to_string());
    }
    let dir = PathBuf::from(path);
    std::fs::create_dir_all(&dir).map_err(|e| format!("创建目录失败: {}", e))?;
    *state.save_dir.lock().await = dir;
    log::info!("文件传输默认保存目录已设为: {}", path);
    persist_settings(&**state).await
}

/// 文件信息（待发送列表元数据展示）
#[derive(Debug, Clone, serde::Serialize)]
pub struct FileInfo {
    pub name: String,
    pub path: String,
    pub size: u64,
    /// 文件类型（扩展名，不含点，小写）
    pub file_type: String,
    /// 修改时间（Unix 毫秒）
    pub modified: u64,
}

/// 读取文件元信息（发送端待发送列表展示用）
#[tauri::command]
pub async fn file_get_file_info(file_path: String) -> Result<FileInfo, String> {
    let meta = tokio::fs::metadata(&file_path)
        .await
        .map_err(|e| format!("读取文件信息失败: {}", e))?;
    if !meta.is_file() {
        return Err("路径不是文件".to_string());
    }
    let path = PathBuf::from(&file_path);
    let name = path
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| file_path.clone());
    let file_type = path
        .extension()
        .unwrap_or_default()
        .to_string_lossy()
        .to_lowercase();
    let modified = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    Ok(FileInfo {
        name,
        path: file_path,
        size: meta.len(),
        file_type,
        modified,
    })
}

// ==================== 后台发送协程 ====================

/// 发送协程：并发许可 → 建立连接 → 传输 → 记录历史
async fn run_send(
    state: &Arc<FileTransferState>,
    app: &AppHandle,
    file_path: &str,
    remote_ip: &str,
    channel: TransferChannel,
    task_id: u64,
    cancel: CancelFlag,
) {
    // 并发许可（最多 max_concurrent 个并发，其余排队；保持持有直到本任务完成）
    let _permit = match state.transfer_mgr.acquire_permit().acquire_owned().await {
        Ok(p) => p,
        Err(_) => return,
    };

    // QUIC 通道：先建立连接（经 UDP 探测发现对端）
    let conn = match channel {
        TransferChannel::Quic => {
            let ip: IpAddr = match remote_ip.trim().parse() {
                Ok(i) => i,
                Err(e) => {
                    finish_send(state, app, task_id, Err(format!("无效 IP: {}", e))).await;
                    state.cancel_flags.lock().await.remove(&task_id);
                    return;
                }
            };
            match state.network.connect(ip).await {
                Ok(c) => Some(c),
                Err(e) => {
                    finish_send(state, app, task_id, Err(format!("连接对方失败: {}", e))).await;
                    state.cancel_flags.lock().await.remove(&task_id);
                    return;
                }
            }
        }
        TransferChannel::Tcp { .. } => None,
    };

    let result = sender::send_file(
        file_path,
        remote_ip,
        channel,
        task_id,
        cancel,
        conn.as_ref(),
        &mut |done| {
            progress_update(state.clone(), app.clone(), task_id, done);
        },
    )
    .await;

    // 不主动 close：让 conn 随本协程结束自然释放，避免接收端仍在收流时
    // 因"closed by peer: send done"而报错（接收端此时可能正在处理 Complete/Verify）

    // 暂停检测：任务已被 file_pause 置为 Paused → 保留状态，不写历史（非终止）
    let paused = state
        .transfer_mgr
        .find(task_id)
        .await
        .map(|t| t.status == TransferStatus::Paused)
        .unwrap_or(false);
    if paused {
        // 通过 QUIC 控制流通知接收端"已暂停"（接收端据此保留 .partial 并显示 Paused）
        if let Some(conn) = &conn {
            let _ = conn
                .send_msg(&FileMsg::Cancel(FileCancel {
                    transfer_id: task_id,
                    reason: "已暂停".to_string(),
                }))
                .await;
            // 等待接收端 ack 确认（最多 2 秒），避免发送端立即关闭连接导致
            // 接收端收到"closed by peer"而非 Cancel 消息 → 误删断点
            wait_pause_ack(conn, task_id).await;
        }
    } else {
        finish_send(state, app, task_id, result).await;
    }
    state.cancel_flags.lock().await.remove(&task_id);
}

/// 等待接收端确认收到暂停消息（reason=ack），超时 2 秒
async fn wait_pause_ack(conn: &Arc<network::FileConnection>, task_id: u64) {
    use tokio::time::{timeout, Duration};
    let deadline = Duration::from_secs(2);
    let _ = timeout(deadline, async {
        loop {
            match conn.recv_msg().await {
                Ok(FileMsg::Cancel(c)) if c.transfer_id == task_id && c.reason == "ack" => break,
                Ok(FileMsg::Cancel(c)) if c.transfer_id == task_id => {
                    // 接收端返回了其它取消（如对方主动取消）→ 不再等待
                    break;
                }
                Ok(_) => continue,
                Err(_) => break,
            }
        }
    })
    .await;
}

/// 发送完成收尾：更新任务 + 历史 + emit
async fn finish_send(
    state: &Arc<FileTransferState>,
    app: &AppHandle,
    task_id: u64,
    result: Result<sender::SendResult, String>,
) {
    match result {
        Ok(res) => {
            update_task_transfer(state, app, task_id, |t| {
                t.status = TransferStatus::Completed;
                t.bytes_done = res.bytes_sent;
                t.speed_kbps = None;
                t.eta_seconds = None;
                t.quick_sent = res.quick_sent;
            })
            .await;
            if let Some(task) = state.transfer_mgr.find(task_id).await {
                push_send_record(
                    state,
                    &task,
                    TransferStatus::Completed,
                    res.bytes_sent,
                    None,
                    Some(res.file_hash_hex.clone()),
                )
                .await;
                let _ = app.emit("file-transfer-update", &task);
            }
            log::info!("文件发送完成: {} ({} 字节)", task_id, res.bytes_sent);
        }
        Err(e) => {
            let cancelled = e.contains("取消");
            let status = if cancelled {
                TransferStatus::Cancelled
            } else {
                TransferStatus::Failed
            };
            update_task_transfer(state, app, task_id, |t| {
                t.status = status;
                t.error_message = Some(e.clone());
                t.speed_kbps = None;
                t.eta_seconds = None;
            })
            .await;
            if let Some(task) = state.transfer_mgr.find(task_id).await {
                push_send_record(state, &task, status, task.bytes_done, Some(e.clone()), None)
                    .await;
                let _ = app.emit("file-transfer-update", &task);
            }
            log::warn!("文件发送失败 {}: {}", task_id, e);
        }
    }
}

/// 写入发送方向历史记录
async fn push_send_record(
    state: &Arc<FileTransferState>,
    task: &TransferTask,
    status: TransferStatus,
    bytes: u64,
    error: Option<String>,
    file_hash: Option<String>,
) {
    let end_time = transfer_manager::unix_now();
    let duration = end_time.saturating_sub(task.created_at);
    // 平均速度：字节 / 秒 / 1024 = KB/s
    let avg = if duration > 0 { Some((bytes / duration) / 1024) } else { None };
    let _ = state
        .history
        .push(protocol::TransferRecord {
            id: 0,
            transfer_id: task.transfer_id,
            direction: task.direction,
            filename: task.filename.clone(),
            file_size: task.file_size,
            remote_ip: task.remote_ip.clone(),
            remote_device: task.remote_device.clone(),
            channel: task.channel,
            status,
            start_time: task.created_at,
            end_time: Some(end_time),
            bytes_transferred: bytes,
            file_hash,
            error_message: error,
            file_path: task.file_path.clone(),
            quick_sent: task.quick_sent,
            avg_speed_kbps: avg,
        })
        .await;
}

// ==================== 通用辅助 ====================

/// 更新任务状态并 emit 前端事件
async fn update_task_transfer(
    state: &Arc<FileTransferState>,
    app: &AppHandle,
    task_id: u64,
    f: impl FnOnce(&mut TransferTask),
) {
    state.transfer_mgr.update(task_id, f).await;
    if let Some(task) = state.transfer_mgr.find(task_id).await {
        let _ = app.emit("file-transfer-update", &task);
    }
}

/// 发送进度更新（后台 spawn 更新速度 + emit）
fn progress_update(state: Arc<FileTransferState>, app: AppHandle, task_id: u64, done: u64) {
    tokio::spawn(async move {
        state
            .transfer_mgr
            .update(task_id, |t| {
                // 终态/暂停任务不再被进度事件覆盖（避免传输完成后残留为"传输中"）
                if matches!(
                    t.status,
                    TransferStatus::Completed
                        | TransferStatus::Failed
                        | TransferStatus::Cancelled
                        | TransferStatus::Rejected
                        | TransferStatus::Paused
                ) {
                    return;
                }
                crate::file_transfer::transfer_manager::update_speed(t, done);
            })
            .await;
        if let Some(task) = state.transfer_mgr.find(task_id).await {
            let _ = app.emit("file-transfer-update", &task);
        }
    });
}

/// emit 任务事件
async fn emit_task(app: &AppHandle, state: &Arc<FileTransferState>, task_id: u64) {
    if let Some(task) = state.transfer_mgr.find(task_id).await {
        let _ = app.emit("file-transfer-update", &task);
    }
}

/// 从 daemon 在线设备列表查询设备名（查不到用 IP）
async fn lookup_device_name(remote_ip: &str) -> String {
    use crate::daemon::rpc_protocol::DaemonResponse;
    match crate::daemon::rpc_client::get_state().await {
        Ok(DaemonResponse::State { peers, .. }) => {
            if let Some(p) = peers.iter().find(|p| p.virtual_ip == remote_ip) {
                return p.name.clone();
            }
        }
        _ => {}
    }
    remote_ip.to_string()
}

/// 本机 VNT 虚拟 IP（daemon 状态；未连接返回 None）
pub(crate) async fn local_vnt_ip() -> Option<String> {
    use crate::daemon::rpc_protocol::DaemonResponse;
    match crate::daemon::rpc_client::get_state().await {
        Ok(DaemonResponse::State { vnt_virtual_ip, .. }) => vnt_virtual_ip,
        _ => None,
    }
}

/// 本机设备名（配置优先，其次主机名）
fn local_device_name() -> String {
    if let Some(cfg) = crate::config::load_config_store().get_active() {
        if let Some(n) = &cfg.device_name {
            if !n.is_empty() {
                return n.clone();
            }
        }
    }
    std::env::var("COMPUTERNAME").unwrap_or_else(|_| "未知设备".to_string())
}

/// 当前 Unix 毫秒时间戳
fn unix_now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// 将当前内存设置持久化到 file_transfer_config.json
async fn persist_settings(state: &FileTransferState) -> Result<(), String> {
    let filter = state.filter.lock().await.clone();
    let auto_accept = *state.auto_accept.lock().await;
    let threshold = *state.threshold.lock().await;
    let save_dir = state.save_dir.lock().await.clone();
    let settings = config::FileTransferSettings {
        mode: filter.mode,
        extensions: filter.extensions.into_iter().collect(),
        auto_accept,
        threshold,
        save_dir: save_dir.to_string_lossy().to_string(),
    };
    config::save(&state.config_dir, &settings)
}
