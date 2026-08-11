//! 后台监听 + GUI 唤起
//!
//! GUI 进程（常驻托盘）默认开启文件接收监听：
//! - QUIC accept 循环（Iroh Endpoint）
//! - TCP accept 循环（裸 TCP 大文件通道）
//!
//! 收到请求 → 校验文件类型 → 计算断点偏移 → 创建待确认任务 →
//! emit `file-transfer-offer`（任意页面/后台均可弹出确认弹窗）→
//! 用户确认后执行接收（QUIC / TCP），完成后写入历史记录。

use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

use tauri::{AppHandle, Emitter};

use crate::file_transfer::protocol::{
    FileAccept, FileMsg, FileOffer, FileReject, FileVerify, TransferChannel, TransferDirection,
    TransferRecord, TransferStatus,
};
use crate::file_transfer::receiver::{self, ReceivePlan};
use crate::file_transfer::transfer_manager::{CancelFlag, TransferTask};
use crate::file_transfer::{
    new_receive_task, FileOfferPayload, FileTransferState, PendingReceive, ReceiveDecision,
    CONFIRM_TIMEOUT_SECS,
};

/// 接收握手超时
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(30);
/// 确认等待超时
const CONFIRM_TIMEOUT: Duration = Duration::from_secs(CONFIRM_TIMEOUT_SECS);
/// 监听错误重试间隔
const RETRY_DELAY: Duration = Duration::from_millis(500);

/// 启动后台监听（QUIC + TCP 两条循环）
pub fn start_listeners(state: Arc<FileTransferState>, app: AppHandle) {
    start_quic_listener(state.clone(), app.clone());
    start_tcp_listener(state.clone(), app);
}

/// QUIC accept 循环
fn start_quic_listener(state: Arc<FileTransferState>, app: AppHandle) {
    tokio::spawn(async move {
        loop {
            match state.network.accept().await {
                Ok(conn) => {
                    let s = state.clone();
                    let a = app.clone();
                    tokio::spawn(async move {
                        handle_quic_connection(s, a, conn).await;
                    });
                }
                Err(e) => {
                    log::warn!("文件传输 QUIC 监听错误: {}", e);
                    tokio::time::sleep(RETRY_DELAY).await;
                }
            }
        }
    });
}

/// TCP accept 循环
/// 安全：仅绑定 VNT 虚拟 IP（未组网前不暴露端口；避免任意主机明文连入）。
/// 端口动态分配（端口 0 → 系统分配随机空闲端口，避开被占用端口），
/// 实际端口写入 state.tcp_port 并经 UDP 探测公告给发送端（对齐桌面共享逻辑）。
fn start_tcp_listener(state: Arc<FileTransferState>, app: AppHandle) {
    tokio::spawn(async move {
        // 等待 VNT 虚拟 IP（文件传输需组网；未连接前 TCP 接收不可用是合理行为）
        let mut vnt_ip = loop {
            if let Some(ip) = get_vnt_ip().await {
                if !ip.is_empty() {
                    break ip;
                }
            }
            tokio::time::sleep(Duration::from_secs(1)).await;
        };
        // 外层循环：绑定失败或 accept 异常（VNT IP 变化）时重新绑定
        loop {
            let bind_addr: std::net::SocketAddr = match format!("{}:0", vnt_ip).parse() {
                Ok(a) => a,
                Err(e) => {
                    log::error!("文件传输 TCP 监听地址无效 {}: {}", vnt_ip, e);
                    tokio::time::sleep(RETRY_DELAY).await;
                    continue;
                }
            };
            let listener = match crate::file_transfer::tcp_channel::TcpReceiver::bind(bind_addr).await
            {
                Ok(l) => l,
                Err(e) => {
                    log::error!("文件传输 TCP 监听绑定失败 {}: {}（重试）", bind_addr, e);
                    tokio::time::sleep(RETRY_DELAY).await;
                    continue;
                }
            };
            // 写入实际端口，供 UDP 探测公告
            let port = match listener.local_addr() {
                Ok(a) => a.port(),
                Err(e) => {
                    log::error!("文件传输获取 TCP 监听端口失败: {}", e);
                    tokio::time::sleep(RETRY_DELAY).await;
                    continue;
                }
            };
            state.tcp_port.store(port, Ordering::Release);
            log::info!("文件传输 TCP 监听端口 {}（VNT IP {}）", port, vnt_ip);

            // 内层循环：accept（异常时跳出重新绑定）
            loop {
                match listener.accept().await {
                    Ok(stream) => {
                        let s = state.clone();
                        let a = app.clone();
                        tokio::spawn(async move {
                            handle_tcp_connection(s, a, stream).await;
                        });
                    }
                    Err(e) => {
                        log::warn!("文件传输 TCP 监听错误: {}（重新绑定）", e);
                        state.tcp_port.store(0, Ordering::Release);
                        break;
                    }
                }
            }
            // 重新读取 VNT IP（可能已变化）
            if let Some(ip) = get_vnt_ip().await {
                if !ip.is_empty() {
                    vnt_ip = ip;
                }
            }
            tokio::time::sleep(RETRY_DELAY).await;
        }
    });
}

/// 从 daemon 获取 VNT 虚拟 IP
async fn get_vnt_ip() -> Option<String> {
    use crate::daemon::rpc_protocol::DaemonResponse;
    match crate::daemon::rpc_client::get_state().await {
        Ok(DaemonResponse::State { vnt_virtual_ip, .. }) => vnt_virtual_ip,
        _ => None,
    }
}

// ==================== QUIC 连接处理 ====================

async fn handle_quic_connection(
    state: Arc<FileTransferState>,
    app: AppHandle,
    conn: Arc<crate::file_transfer::network::FileConnection>,
) {
    // 1. 读首条消息（Offer 或 Text），带超时
    let msg = match tokio::time::timeout(HANDSHAKE_TIMEOUT, conn.recv_msg()).await {
        Ok(Ok(m)) => m,
        Ok(Err(e)) => {
            log::info!("文件传输连接读取失败: {}", e);
            return;
        }
        Err(_) => {
            log::info!("文件传输连接握手超时");
            return;
        }
    };

    match msg {
        FileMsg::Text(t) => {
            // 文本消息：emit 到前端，无需确认
            let _ = app.emit("file-text-message", t);
            return;
        }
        FileMsg::Offer(offer) => {
            handle_offer(state, app, conn, offer).await;
        }
        other => {
            log::warn!("文件传输连接首条消息非 Offer/Text: {:?}", other);
        }
    }
}

/// 处理 Offer（QUIC 通道）
async fn handle_offer(
    state: Arc<FileTransferState>,
    app: AppHandle,
    conn: Arc<crate::file_transfer::network::FileConnection>,
    offer: FileOffer,
) {
    let transfer_id = offer.transfer_id;
    // 远端 IP：优先用发送方通告的 VNT 虚拟 IP（QUIC 连接远端地址可能是 IPv6 链路本地）
    let remote_ip = if offer.sender_ip.trim().is_empty() {
        conn.remote_address().ip().to_string()
    } else {
        offer.sender_ip.clone()
    };
    let save_dir = state.save_dir.lock().await.clone();

    // 1. 文件类型过滤
    {
        let filter = state.filter.lock().await.clone();
        if !receiver::is_allowed(&offer, &filter) {
            let _ = conn
                .send_msg(&FileMsg::Reject(FileReject {
                    transfer_id,
                    reason: format!("文件类型 .{} 被过滤规则拒绝", ext(&offer.filename)),
                }))
                .await;
            log::info!("拒绝文件 {}（类型被过滤）", offer.filename);
            return;
        }
    }

    // 2. 同名文件 md5 一致 → 秒传（跳过实际传输，直接标记完成）
    if let Some(existing) = receiver::find_duplicate(&save_dir, &offer).await {
        log::info!("秒传命中: {}（内容与本地文件一致，跳过传输）", offer.filename);
        // resume_offset = file_size 通知发送方"对方已有完整文件"
        let _ = conn
            .send_msg(&FileMsg::Accept(FileAccept {
                transfer_id,
                resume_offset: offer.file_size,
            }))
            .await;
        let _ = conn
            .send_msg(&FileMsg::Verify(FileVerify {
                transfer_id,
                ok: true,
                expected_hash_hex: offer.file_hash_hex.clone(),
                actual_hash_hex: offer.file_hash_hex.clone(),
            }))
            .await;
        let task_id = state
            .transfer_mgr
            .enqueue(new_receive_task(&offer, &remote_ip, offer.file_size, &existing))
            .await;
        update_task(&state, &app, task_id, |t| {
            t.status = TransferStatus::Completed;
            t.bytes_done = offer.file_size;
            t.save_path = Some(existing.to_string_lossy().to_string());
            t.quick_sent = true;
        })
        .await;
        if let Some(task) = state.transfer_mgr.find(task_id).await {
            let _ = state
                .history
                .push(TransferRecord {
                    id: 0,
                    transfer_id: task.transfer_id,
                    direction: TransferDirection::Receive,
                    filename: task.filename.clone(),
                    file_size: offer.file_size,
                    remote_ip: task.remote_ip.clone(),
                    remote_device: task.remote_device.clone(),
                    channel: task.channel,
                    status: TransferStatus::Completed,
                    start_time: task.created_at,
                    end_time: Some(crate::file_transfer::transfer_manager::unix_now()),
                    bytes_transferred: offer.file_size,
                    file_hash: Some(offer.file_hash_hex.clone()),
                    error_message: None,
                    file_path: Some(existing.to_string_lossy().to_string()),
                    quick_sent: true,
                    avg_speed_kbps: None,
                })
                .await;
            let _ = app.emit("file-transfer-update", &task);
        }
        return;
    }

    // 3. 解析最终保存路径（同名不同内容 → 新文件名避免覆盖）
    let save_path = receiver::resolve_save_path(&save_dir, &offer.filename);

    // 4. 断点偏移 + 创建待确认任务
    let plan = match receiver::plan_receive(&offer, &save_path).await {
        Ok(p) => p,
        Err(e) => {
            let _ = conn
                .send_msg(&FileMsg::Reject(FileReject {
                    transfer_id,
                    reason: format!("接收准备失败: {}", e),
                }))
                .await;
            return;
        }
    };

    // 3. 创建任务
    let task_id = state.transfer_mgr.enqueue(new_receive_task(&offer, &remote_ip, plan.resume_offset, &plan.partial_path)).await;

    // 5.1 自动接收（开关开启且类型已通过过滤）→ 跳过弹窗直接接收
    if *state.auto_accept.lock().await {
        log::info!("自动接收文件: {}", offer.filename);
        let _ = conn
            .send_msg(&FileMsg::Accept(FileAccept {
                transfer_id,
                resume_offset: plan.resume_offset,
            }))
            .await;
        execute_receive(state.clone(), app, conn, offer, plan, task_id, save_path).await;
        state.cancel_flags.lock().await.remove(&task_id);
        return;
    }

    // 5.2 注册待确认
    let (tx, rx) = tokio::sync::oneshot::channel();
    {
        let mut pending = state.pending.lock().await;
        pending.insert(
            task_id,
            PendingReceive {
                offer: offer.clone(),
                resume_offset: plan.resume_offset,
                partial_path: plan.partial_path.clone(),
                default_save_path: save_path.clone(),
                confirm: tx,
            },
        );
    }

    // 6. emit 弹窗
    let _ = app.emit(
        "file-transfer-offer",
        FileOfferPayload {
            transfer_id,
            filename: offer.filename.clone(),
            file_size: offer.file_size,
            channel: offer.channel,
            remote_ip: remote_ip.clone(),
            remote_device: offer.sender_device.clone(),
            resume_offset: plan.resume_offset,
            default_save_path: save_path.to_string_lossy().to_string(),
        },
    );
    log::info!(
        "收到文件传输请求: {} ({} 字节, {}) 来自 {}",
        offer.filename,
        offer.file_size,
        if offer.channel.is_tcp() { "TCP" } else { "QUIC" },
        remote_ip
    );

    // 5. 等待用户确认（超时或连接关闭 → 自动拒绝）
    match tokio::time::timeout(CONFIRM_TIMEOUT, rx).await {
        Ok(Ok(ReceiveDecision::Accept { save_path })) => {
            // 基于用户最终保存路径重新规划（防覆盖 + 断点偏移同步；
            // 否则用户选择非默认路径时，发送端按旧偏移续传、接收端从头接收会错位）
            let save_path = receiver::resolve_conflict(&save_path, &offer).await;
            let plan = match receiver::plan_receive(&offer, &save_path).await {
                Ok(p) => p,
                Err(e) => {
                    let _ = conn
                        .send_msg(&FileMsg::Reject(FileReject {
                            transfer_id,
                            reason: format!("接收准备失败: {}", e),
                        }))
                        .await;
                    update_task(&state, &app, task_id, |t| {
                        t.status = TransferStatus::Failed;
                        t.error_message = Some(format!("接收准备失败: {}", e));
                    })
                    .await;
                    state.pending.lock().await.remove(&task_id);
                    state.cancel_flags.lock().await.remove(&task_id);
                    return;
                }
            };
            // 发 Accept（含与接收端一致的断点偏移），执行接收
            let _ = conn
                .send_msg(&FileMsg::Accept(FileAccept {
                    transfer_id,
                    resume_offset: plan.resume_offset,
                }))
                .await;
            execute_receive(state.clone(), app, conn, offer, plan, task_id, save_path).await;
        }
        Ok(Ok(ReceiveDecision::Reject { reason })) => {
            let _ = conn
                .send_msg(&FileMsg::Reject(FileReject {
                    transfer_id,
                    reason: reason.clone(),
                }))
                .await;
            update_task(&state, &app, task_id, |t| {
                t.status = TransferStatus::Rejected;
                t.error_message = Some(reason);
            })
            .await;
            log::info!("用户拒绝文件 {}", offer.filename);
        }
        _ => {
            // 超时或连接关闭
            let _ = conn
                .send_msg(&FileMsg::Reject(FileReject {
                    transfer_id,
                    reason: "等待确认超时".to_string(),
                }))
                .await;
            update_task(&state, &app, task_id, |t| {
                t.status = TransferStatus::Rejected;
                t.error_message = Some("等待确认超时".to_string());
            })
            .await;
        }
    }

    state.pending.lock().await.remove(&task_id);
    state.cancel_flags.lock().await.remove(&task_id);
}

/// 执行 QUIC 接收（用户已确认）
async fn execute_receive(
    state: Arc<FileTransferState>,
    app: AppHandle,
    conn: Arc<crate::file_transfer::network::FileConnection>,
    offer: FileOffer,
    plan: ReceivePlan,
    task_id: u64,
    save_path: std::path::PathBuf,
) {
    // 调用方已保证 save_path 与 plan 一致（确认分支/自动接收前已 resolve_conflict + plan_receive）
    let cancel = CancelFlag::new();
    state.cancel_flags.lock().await.insert(task_id, cancel.clone());

    update_task(&state, &app, task_id, |t| {
        t.status = TransferStatus::Transferring;
        t.save_path = Some(save_path.to_string_lossy().to_string());
    })
    .await;

    let result = receiver::receive_quic(
        &conn,
        &plan,
        &save_path,
        cancel,
        &mut |done| {
            progress_update(state.clone(), app.clone(), task_id, done);
        },
    )
    .await;

    finish_receive(&state, &app, task_id, result, save_path, offer.file_size).await;
}

// ==================== TCP 连接处理 ====================

async fn handle_tcp_connection(
    state: Arc<FileTransferState>,
    app: AppHandle,
    mut stream: tokio::net::TcpStream,
) {
    use crate::file_transfer::tcp_channel::TcpReceiver;

    // 1. 读握手 JSON（带超时）
    let hs = match tokio::time::timeout(HANDSHAKE_TIMEOUT, TcpReceiver::read_handshake(&mut stream))
        .await
    {
        Ok(Ok(h)) => h,
        Ok(Err(e)) => {
            log::info!("文件传输 TCP 握手读取失败: {}", e);
            return;
        }
        Err(_) => {
            log::info!("文件传输 TCP 握手超时");
            return;
        }
    };

    if hs["type"].as_str() != Some("offer") {
        log::warn!("文件传输 TCP 握手类型异常");
        return;
    }
    let offer = FileOffer {
        transfer_id: hs["transfer_id"].as_u64().unwrap_or(0),
        filename: hs["filename"].as_str().unwrap_or("unknown").to_string(),
        file_size: hs["file_size"].as_u64().unwrap_or(0),
        file_hash_hex: hs["file_hash"].as_str().unwrap_or("").to_string(),
        chunk_size: 0,
        channel: TransferChannel::Tcp {
            port: crate::file_transfer::tcp_channel::DEFAULT_PORT,
        },
        sender_device: hs["sender_device"].as_str().unwrap_or("").to_string(),
        sender_ip: hs["sender_ip"].as_str().unwrap_or("").to_string(),
    };
    let transfer_id = offer.transfer_id;
    // 远端 IP：优先用发送方通告的 VNT 虚拟 IP（TCP peer_addr 可能非 VNT 虚拟 IP）
    let remote_ip = if offer.sender_ip.trim().is_empty() {
        stream.peer_addr().map(|a| a.ip().to_string()).unwrap_or_default()
    } else {
        offer.sender_ip.clone()
    };
    let save_dir = state.save_dir.lock().await.clone();

    // 2. 文件类型过滤
    {
        let filter = state.filter.lock().await.clone();
        if !receiver::is_allowed(&offer, &filter) {
            let _ = TcpReceiver::send_response(
                &mut stream,
                &serde_json::json!({
                    "type": "reject",
                    "reason": format!("文件类型 .{} 被过滤规则拒绝", ext(&offer.filename)),
                }),
            )
            .await;
            return;
        }
    }

    // 3. 同名文件 md5 一致 → 秒传（跳过传输，直接标记完成）
    if let Some(existing) = receiver::find_duplicate(&save_dir, &offer).await {
        log::info!("秒传命中(TCP): {}（内容与本地文件一致，跳过传输）", offer.filename);
        let _ = TcpReceiver::send_response(
            &mut stream,
            &serde_json::json!({ "type": "accept", "resume_offset": offer.file_size }),
        )
        .await;
        let task_id = state
            .transfer_mgr
            .enqueue(new_receive_task(&offer, &remote_ip, offer.file_size, &existing))
            .await;
        update_task(&state, &app, task_id, |t| {
            t.status = TransferStatus::Completed;
            t.bytes_done = offer.file_size;
            t.save_path = Some(existing.to_string_lossy().to_string());
            t.quick_sent = true;
        })
        .await;
        if let Some(task) = state.transfer_mgr.find(task_id).await {
            let _ = state
                .history
                .push(TransferRecord {
                    id: 0,
                    transfer_id: task.transfer_id,
                    direction: TransferDirection::Receive,
                    filename: task.filename.clone(),
                    file_size: offer.file_size,
                    remote_ip: task.remote_ip.clone(),
                    remote_device: task.remote_device.clone(),
                    channel: task.channel,
                    status: TransferStatus::Completed,
                    start_time: task.created_at,
                    end_time: Some(crate::file_transfer::transfer_manager::unix_now()),
                    bytes_transferred: offer.file_size,
                    file_hash: Some(offer.file_hash_hex.clone()),
                    error_message: None,
                    file_path: Some(existing.to_string_lossy().to_string()),
                    quick_sent: true,
                    avg_speed_kbps: None,
                })
                .await;
            let _ = app.emit("file-transfer-update", &task);
        }
        return;
    }

    // 4. 解析最终保存路径（同名不同内容 → 新文件名避免覆盖）
    let save_path = receiver::resolve_save_path(&save_dir, &offer.filename);

    // 5. 断点偏移 + 待确认任务
    let plan = match receiver::plan_receive(&offer, &save_path).await {
        Ok(p) => p,
        Err(e) => {
            let _ = TcpReceiver::send_response(
                &mut stream,
                &serde_json::json!({ "type": "reject", "reason": e }),
            )
            .await;
            return;
        }
    };
    let task_id = state.transfer_mgr.enqueue(new_receive_task(&offer, &remote_ip, plan.resume_offset, &plan.partial_path)).await;

    // 自动接收（开关开启且类型已通过过滤）→ 跳过弹窗直接接收
    if *state.auto_accept.lock().await {
        log::info!("自动接收 TCP 文件: {}", offer.filename);
        // save_path 已由 resolve_save_path 处理同名（新文件名），与 plan 保持一致
        let _ = TcpReceiver::send_response(
            &mut stream,
            &serde_json::json!({ "type": "accept", "resume_offset": plan.resume_offset }),
        )
        .await;
        let cancel = CancelFlag::new();
        state.cancel_flags.lock().await.insert(task_id, cancel.clone());
        update_task(&state, &app, task_id, |t| {
            t.status = TransferStatus::Transferring;
            t.save_path = Some(save_path.to_string_lossy().to_string());
        })
        .await;
        let result = receiver::receive_tcp(
            &mut stream,
            &offer,
            &save_path,
            cancel,
            &mut |done| {
                progress_update(state.clone(), app.clone(), task_id, done);
            },
        )
        .await;
        finish_receive(&state, &app, task_id, result, save_path, offer.file_size).await;
        state.cancel_flags.lock().await.remove(&task_id);
        return;
    }

    let (tx, rx) = tokio::sync::oneshot::channel();
    {
        let mut pending = state.pending.lock().await;
        pending.insert(
            task_id,
            PendingReceive {
                offer: offer.clone(),
                resume_offset: plan.resume_offset,
                partial_path: plan.partial_path.clone(),
                default_save_path: save_path.clone(),
                confirm: tx,
            },
        );
    }

    let _ = app.emit(
        "file-transfer-offer",
        FileOfferPayload {
            transfer_id,
            filename: offer.filename.clone(),
            file_size: offer.file_size,
            channel: offer.channel,
            remote_ip: remote_ip.clone(),
            remote_device: offer.sender_device.clone(),
            resume_offset: plan.resume_offset,
            default_save_path: save_path.to_string_lossy().to_string(),
        },
    );
    log::info!("收到 TCP 文件传输请求: {} ({} 字节) 来自 {}", offer.filename, offer.file_size, remote_ip);

    match tokio::time::timeout(CONFIRM_TIMEOUT, rx).await {
        Ok(Ok(ReceiveDecision::Accept { save_path })) => {
            // 基于用户最终保存路径重新规划（防覆盖 + 断点偏移同步）
            let save_path = receiver::resolve_conflict(&save_path, &offer).await;
            let resume_offset = match receiver::plan_receive(&offer, &save_path).await {
                Ok(p) => p.resume_offset,
                Err(e) => {
                    let _ = TcpReceiver::send_response(
                        &mut stream,
                        &serde_json::json!({ "type": "reject", "reason": e }),
                    )
                    .await;
                    update_task(&state, &app, task_id, |t| {
                        t.status = TransferStatus::Failed;
                        t.error_message = Some(format!("接收准备失败: {}", e));
                    })
                    .await;
                    state.pending.lock().await.remove(&task_id);
                    state.cancel_flags.lock().await.remove(&task_id);
                    return;
                }
            };
            let _ = TcpReceiver::send_response(
                &mut stream,
                &serde_json::json!({ "type": "accept", "resume_offset": resume_offset }),
            )
            .await;

            let cancel = CancelFlag::new();
            state.cancel_flags.lock().await.insert(task_id, cancel.clone());
            update_task(&state, &app, task_id, |t| {
                t.status = TransferStatus::Transferring;
                t.save_path = Some(save_path.to_string_lossy().to_string());
            })
            .await;

            let result = receiver::receive_tcp(
                &mut stream,
                &offer,
                &save_path,
                cancel,
                &mut |done| {
                    progress_update(state.clone(), app.clone(), task_id, done);
                },
            )
            .await;

            finish_receive(&state, &app, task_id, result, save_path, offer.file_size).await;
        }
        Ok(Ok(ReceiveDecision::Reject { reason })) => {
            let _ = TcpReceiver::send_response(
                &mut stream,
                &serde_json::json!({ "type": "reject", "reason": reason.clone() }),
            )
            .await;
            update_task(&state, &app, task_id, |t| {
                t.status = TransferStatus::Rejected;
                t.error_message = Some(reason);
            })
            .await;
        }
        _ => {
            // 超时或连接关闭
            let _ = TcpReceiver::send_response(
                &mut stream,
                &serde_json::json!({ "type": "reject", "reason": "等待确认超时" }),
            )
            .await;
            update_task(&state, &app, task_id, |t| {
                t.status = TransferStatus::Rejected;
                t.error_message = Some("等待确认超时".to_string());
            })
            .await;
        }
    }

    state.pending.lock().await.remove(&task_id);
    state.cancel_flags.lock().await.remove(&task_id);
}

// ==================== 收尾 / 进度 ====================

/// 接收完成后统一收尾：更新任务状态 + 写入历史
async fn finish_receive(
    state: &Arc<FileTransferState>,
    app: &AppHandle,
    task_id: u64,
    result: Result<u64, String>,
    save_path: std::path::PathBuf,
    file_size: u64,
) {
    match result {
        Ok(bytes) => {
            update_task(state, app, task_id, |t| {
                t.status = TransferStatus::Completed;
                t.bytes_done = bytes;
                t.speed_kbps = None;
                t.eta_seconds = None;
            })
            .await;
            if let Some(task) = state.transfer_mgr.find(task_id).await {
                let end_time = crate::file_transfer::transfer_manager::unix_now();
                let duration = end_time.saturating_sub(task.created_at);
                // 平均速度：字节 / 秒 / 1024 = KB/s
                let avg = if duration > 0 { Some((bytes / duration) / 1024) } else { None };
                // 计算文件 md5（历史记录秒传验证用）
                let file_hash = match receiver::compute_file_hash(&save_path).await {
                    Ok(h) => Some(hex::encode(h)),
                    Err(_) => None,
                };
                let _ = state
                    .history
                    .push(crate::file_transfer::protocol::TransferRecord {
                        id: 0,
                        transfer_id: task.transfer_id,
                        direction: task.direction,
                        filename: task.filename.clone(),
                        file_size,
                        remote_ip: task.remote_ip.clone(),
                        remote_device: task.remote_device.clone(),
                        channel: task.channel,
                        status: TransferStatus::Completed,
                        start_time: task.created_at,
                        end_time: Some(end_time),
                        bytes_transferred: bytes,
                        file_hash,
                        error_message: None,
                        file_path: Some(save_path.to_string_lossy().to_string()),
                        quick_sent: task.quick_sent,
                        avg_speed_kbps: avg,
                    })
                    .await;
                let _ = app.emit("file-transfer-update", &task);
            }
            log::info!("文件接收完成: {} -> {} ({} 字节)", task_id, save_path.display(), bytes);
        }
        Err(e) => {
            // 发送端已暂停（Cancel reason=已暂停）→ 保留 .partial 断点，任务置 Paused
            // （不发历史、不移出传输中；接收端显示暂停，等待发送端继续）
            let paused = e.contains("暂停") && e.contains("对方取消");
            if paused {
                update_task(state, app, task_id, |t| {
                    t.status = TransferStatus::Paused;
                    t.error_message = Some("发送端已暂停，等待继续".to_string());
                    t.speed_kbps = None;
                    t.eta_seconds = None;
                })
                .await;
                log::info!("接收已暂停（发送端暂停）: task={} {}", task_id, e);
                return;
            }
            let cancelled = e.contains("取消");
            update_task(state, app, task_id, |t| {
                if cancelled {
                    t.status = TransferStatus::Cancelled;
                } else {
                    t.status = TransferStatus::Failed;
                }
                t.error_message = Some(e.clone());
                t.speed_kbps = None;
                t.eta_seconds = None;
            })
            .await;

            // 断点清理策略：
            // - 用户主动取消（cancelled）→ 删除 .partial（用户放弃，无需续传）
            // - 校验失败（文件内容已损坏）→ 删除 .partial（断点无效，续传也必失败）
            // - 其他连接异常（closed by peer / 对方中断）→ **保留 .partial**：
            //   可能是发送端暂停/临时中断，保留断点才能续传（否则重发从头开始）
            let verify_failed = e.contains("校验失败");
            if cancelled || verify_failed {
                let partial = receiver::partial_for(&save_path);
                if partial.exists() {
                    let _ = tokio::fs::remove_file(&partial).await;
                    log::info!(
                        "接收{}，已删除断点残留 {}",
                        if cancelled { "已取消" } else { "校验失败" },
                        partial.display()
                    );
                }
            } else {
                log::info!(
                    "接收失败但保留断点残留（供续传）: task={} err={}",
                    task_id,
                    e
                );
            }

            if let Some(task) = state.transfer_mgr.find(task_id).await {
                let status = task.status;
                let _ = state
                    .history
                    .push(crate::file_transfer::protocol::TransferRecord {
                        id: 0,
                        transfer_id: task.transfer_id,
                        direction: task.direction,
                        filename: task.filename.clone(),
                        file_size,
                        remote_ip: task.remote_ip.clone(),
                        remote_device: task.remote_device.clone(),
                        channel: task.channel,
                        status,
                        start_time: task.created_at,
                        end_time: Some(crate::file_transfer::transfer_manager::unix_now()),
                        bytes_transferred: task.bytes_done,
                        file_hash: None,
                        error_message: Some(e.clone()),
                        file_path: Some(save_path.to_string_lossy().to_string()),
                        quick_sent: task.quick_sent,
                        avg_speed_kbps: None,
                    })
                    .await;
                let _ = app.emit("file-transfer-update", &task);
            }
            log::warn!("文件接收失败 {}: {}", task_id, e);
        }
    }
}

/// 进度更新：更新任务（含实时速度/剩余时间）+ emit 事件
fn progress_update(state: Arc<FileTransferState>, app: AppHandle, task_id: u64, done: u64) {
    let s = state.clone();
    let a = app.clone();
    tokio::spawn(async move {
        s.transfer_mgr
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
        if let Some(task) = s.transfer_mgr.find(task_id).await {
            let _ = a.emit("file-transfer-update", &task);
        }
    });
}

/// 更新任务状态并 emit 前端事件
async fn update_task(
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

/// 提取文件扩展名（小写）
fn ext(filename: &str) -> String {
    std::path::Path::new(filename)
        .extension()
        .unwrap_or_default()
        .to_string_lossy()
        .to_lowercase()
}
