//! VNT 进程生命周期管理（daemon 侧）
//!
//! - 启动 vnt-cli（与 daemon 同目录 / PATH），参数构建与 GUI 版一致
//! - 管道监听输出 → 解析状态（connected / 虚拟 IP / 真实服务器 / NAT）
//! - 进程意外退出时守护重启（防抖退避），主动停止不重启
//! - 定期 `--list` 维护设备列表、`--info` 尽力解析 NAT

use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

// Bug 1：spawn 子进程时隐藏控制台窗口（否则 daemon 每次 spawn 都弹黑框）
// 注：tokio::process::Command 的 creation_flags 是固有方法，无需导入 CommandExt
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x08000000;

use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

use crate::config::VntConfig;
use crate::daemon::state_store::{self, RuntimeState};

/// VNT 运行句柄
struct VntRuntime {
    /// 取消令牌：主动停止时触发，进程退出后不再守护重启
    cancel: CancellationToken,
    /// 输出监听任务（child.wait 也由它处理）
    task: tokio::task::JoinHandle<()>,
    /// peers / nat 轮询任务
    poller: tokio::task::JoinHandle<()>,
}

static VNT_RUNTIME: Mutex<Option<VntRuntime>> = Mutex::const_new(None);

/// 定位 vnt-cli 可执行文件：优先 daemon 同目录（sidecar 解包目录），其次 PATH
fn vnt_cli_path() -> Result<PathBuf, String> {
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let candidate = dir.join("vnt-cli.exe");
            if candidate.exists() {
                return Ok(candidate);
            }
        }
    }
    // PATH 回退
    Ok(PathBuf::from("vnt-cli.exe"))
}

/// 构建 vnt-cli 启动参数（与 GUI 版一致：-k token / -s server / -n name / -u mtu；协议由 server 前缀决定）
pub fn build_args(config: &VntConfig) -> Vec<String> {
    let mut args: Vec<String> = vec!["-k".into(), config.token.clone()];
    if !config.device_name.as_deref().unwrap_or("").trim().is_empty() {
        args.push("-n".into());
        args.push(config.device_name.as_deref().unwrap_or("").trim().to_string());
    }
    if !config.server_address.as_deref().unwrap_or("").trim().is_empty() {
        args.push("-s".into());
        args.push(config.server_address.as_deref().unwrap_or("").trim().to_string());
    }
    if let Some(mtu) = config.mtu {
        if mtu > 0 {
            args.push("-u".into());
            args.push(mtu.to_string());
        }
    }
    if let Some(compressor) = &config.compressor {
        if !compressor.is_empty() {
            args.push("--compressor".into());
            args.push(compressor.clone());
        }
    }
    args
}

/// 启动 VNT（已运行则忽略）
pub async fn start(state: Arc<Mutex<RuntimeState>>, config: VntConfig) -> Result<(), String> {
    let mut guard = VNT_RUNTIME.lock().await;
    if guard.is_some() {
        return Err("VNT 已在运行".to_string());
    }
    let cancel = CancellationToken::new();
    let task = {
        let state = state.clone();
        let config = config.clone();
        let cancel = cancel.clone();
        tokio::spawn(async move {
            run_vnt_with_supervision(state, config, cancel).await;
        })
    };
    let poller = {
        let state = state.clone();
        let config = config.clone();
        tokio::spawn(async move {
            poll_loop(state, config).await;
        })
    };
    *guard = Some(VntRuntime { cancel, task, poller });

    // 更新状态并持久化
    {
        let mut s = state.lock().await;
        s.vnt_running = true;
        s.vnt_connected = false;
        s.vnt_config = Some(config.clone());
        s.vnt_was_running = true;
        s.vnt_server_host = None;
        s.vnt_virtual_ip = None;
        s.vnt_nat_type = None;
    }
    state_store::save(&*state.lock().await).await;
    tracing::info!("VNT 启动中: name={}, server={}", config.name, config.server_address.as_deref().unwrap_or(""));
    Ok(())
}

/// 停止 VNT（主动停止 → 不守护重启）
pub async fn stop(state: Arc<Mutex<RuntimeState>>) -> Result<(), String> {
    let runtime = VNT_RUNTIME.lock().await.take();
    if let Some(runtime) = runtime {
        runtime.cancel.cancel();
        let _ = tokio::time::timeout(Duration::from_secs(3), runtime.task).await;
        runtime.poller.abort();
        // 兜底：确保进程结束
        let _ = kill_vnt_cli_process();
        tracing::info!("VNT 已停止");
    }
    {
        let mut s = state.lock().await;
        s.vnt_running = false;
        s.vnt_connected = false;
        s.vnt_was_running = false;
        s.peers.clear();
    }
    state_store::save(&*state.lock().await).await;
    Ok(())
}

/// 重启 VNT（先停后启）
pub async fn restart(state: Arc<Mutex<RuntimeState>>, config: VntConfig) -> Result<(), String> {
    stop(state.clone()).await?;
    start(state, config).await
}

/// 主循环：spawn 进程 + 监听输出 + 意外退出守护重启
async fn run_vnt_with_supervision(
    state: Arc<Mutex<RuntimeState>>,
    config: VntConfig,
    cancel: CancellationToken,
) {
    let mut restart_delay = Duration::from_secs(3);
    loop {
        // 主动停止 → 退出守护
        if cancel.is_cancelled() {
            break;
        }
        let path = match vnt_cli_path() {
            Ok(p) => p,
            Err(e) => {
                tracing::error!("定位 vnt-cli 失败: {}", e);
                break;
            }
        };
        let mut cmd = Command::new(&path);
        cmd.args(build_args(&config));
        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::piped());
        #[cfg(windows)]
        cmd.creation_flags(CREATE_NO_WINDOW);

        let mut child = match cmd.spawn() {
            Ok(c) => c,
            Err(e) => {
                tracing::error!("启动 vnt-cli 失败: {} ({})", e, path.display());
                break;
            }
        };
        tracing::info!("vnt-cli 已启动: {}", path.display());

        // 输出监听
        let stdout = child.stdout.take();
        let stderr = child.stderr.take();
        let state_stdout = state.clone();
        let state_stderr = state.clone();
        let out_task = tokio::spawn(async move {
            if let Some(out) = stdout {
                let mut lines = BufReader::new(out).lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    handle_output_line(&state_stdout, &line);
                }
            }
        });
        let err_task = tokio::spawn(async move {
            if let Some(err) = stderr {
                let mut lines = BufReader::new(err).lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    handle_output_line(&state_stderr, &line);
                }
            }
        });

        // 等待进程退出（主动停止 → kill 后 wait 立即返回）
        let status = tokio::select! {
            _ = cancel.cancelled() => {
                let _ = child.kill().await;
                child.wait().await
            }
            r = child.wait() => r,
        };
        let _ = status;
        out_task.abort();
        err_task.abort();

        // 主动停止 → 退出
        if cancel.is_cancelled() {
            tracing::info!("vnt-cli 已退出（主动停止）");
            break;
        }
        // 意外退出 → 标记断开 + 守护重启（防抖退避）
        {
            let mut s = state.lock().await;
            s.vnt_connected = false;
        }
        tracing::warn!("vnt-cli 意外退出，{} 秒后自动重启", restart_delay.as_secs());
        tokio::select! {
            _ = cancel.cancelled() => break,
            _ = tokio::time::sleep(restart_delay) => {}
        }
        if restart_delay < Duration::from_secs(30) {
            restart_delay *= 2;
        }
    }
}

/// 处理 vnt-cli 输出行：状态机（与 GUI 版解析一致）
fn handle_output_line(state: &Arc<Mutex<RuntimeState>>, line: &str) {
    let line = line.trim();
    if line.is_empty() {
        return;
    }
    let mut changed = false;
    if line.contains("Connect Successfully") || line.contains("Connection status: Connected") {
        if let Ok(mut s) = state.try_lock() {
            if !s.vnt_connected {
                s.vnt_connected = true;
                changed = true;
            }
        }
        tracing::info!("VNT 已连接: {}", line);
    } else if line.contains("register ip=") {
        if let Some(ip) = parse_virtual_ip(line) {
            if let Ok(mut s) = state.try_lock() {
                if s.vnt_virtual_ip.as_deref() != Some(ip.as_str()) {
                    s.vnt_virtual_ip = Some(ip.clone());
                    changed = true;
                }
            }
            tracing::info!("VNT 虚拟 IP: {}", ip);
        }
    } else if let Some(addr) = extract_server_addr(line) {
        if let Ok(mut s) = state.try_lock() {
            if s.vnt_server_host.as_deref() != Some(addr.as_str()) {
                s.vnt_server_host = Some(addr);
                changed = true;
            }
        }
    } else if line.contains("Connection closed") || line.contains("Disconnected") {
        if let Ok(mut s) = state.try_lock() {
            if s.vnt_connected {
                s.vnt_connected = false;
                changed = true;
            }
        }
        tracing::warn!("VNT 连接断开");
    }
    if changed {
        // 异步保存（不阻塞解析）
        let state = state.clone();
        tokio::spawn(async move {
            state_store::save(&*state.lock().await).await;
        });
    }
}

/// 轮询：设备列表（10s）+ NAT 信息（30s）
async fn poll_loop(state: Arc<Mutex<RuntimeState>>, config: VntConfig) {
    let mut tick: u32 = 0;
    loop {
        tokio::time::sleep(Duration::from_secs(10)).await;
        tick += 1;
        let running = state.lock().await.vnt_running;
        if !running {
            continue;
        }
        // --list
        if let Ok(output) = run_cli(&["--list", "-k", &config.token]).await {
            let mut peers = Vec::new();
            let text = output;
            for line in text.lines() {
                if let Some(peer) = crate::parse_list_line(line) {
                    peers.push(peer);
                }
            }
            if !peers.is_empty() {
                let mut s = state.lock().await;
                if s.peers != peers {
                    s.peers = peers;
                }
            }
        }
        // --info（每 3 次 = 30s）
        if tick % 3 == 0 {
            if let Ok(text) = run_cli(&["--info", "-k", &config.token]).await {
                let mut s = state.lock().await;
                if let Some(addr) = crate::parse_relay_addr(&text) {
                    if s.vnt_server_host.as_deref() != Some(addr.as_str()) {
                        s.vnt_server_host = Some(addr);
                    }
                }
                let (nat, _) = crate::parse_info(&text);
                if nat.is_some() {
                    s.vnt_nat_type = nat;
                }
            }
        }
    }
}

/// 运行一次性 vnt-cli 命令（--list / --info），返回输出文本
/// Bug 1：CREATE_NO_WINDOW 隐藏控制台（轮询每 10s spawn，不能弹框）
async fn run_cli(args: &[&str]) -> Result<String, String> {
    let path = vnt_cli_path()?;
    let mut cmd = Command::new(&path);
    cmd.args(args);
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());
    #[cfg(windows)]
    cmd.creation_flags(CREATE_NO_WINDOW);
    let output = tokio::time::timeout(Duration::from_secs(8), cmd.output())
        .await
        .map_err(|_| "vnt-cli 命令超时".to_string())?
        .map_err(|e| format!("vnt-cli 执行失败: {}", e))?;
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    if stdout.trim().is_empty() {
        Ok(stderr)
    } else {
        Ok(stdout)
    }
}

/// 解析 register ip= 行
fn parse_virtual_ip(line: &str) -> Option<String> {
    line.split("register ip=")
        .nth(1)
        .map(|s| s.split_whitespace().next().unwrap_or("").to_string())
        .filter(|s| !s.is_empty())
}

/// 解析真实服务器地址（connect count=1 ,address=8.134.66.150:29872）
fn extract_server_addr(line: &str) -> Option<String> {
    let lower = line.to_lowercase();
    let idx = lower.find("address=")?;
    let rest = &line[idx + "address=".len()..];
    let addr = rest
        .split(|c: char| c.is_whitespace() || c == ',')
        .next()
        .unwrap_or("")
        .trim();
    if addr.is_empty() {
        None
    } else {
        Some(addr.to_string())
    }
}

/// 兜底：杀掉所有 vnt-cli 进程（主动停止时确保清理）
#[cfg(windows)]
async fn kill_vnt_cli_process() -> Result<(), String> {
    let mut cmd = tokio::process::Command::new("taskkill");
    cmd.args(["/F", "/IM", "vnt-cli.exe"]);
    cmd.stdout(Stdio::null());
    cmd.stderr(Stdio::null());
    cmd.creation_flags(CREATE_NO_WINDOW);
    cmd.status()
        .await
        .map(|_| ())
        .map_err(|e| format!("taskkill 失败: {}", e))
}

#[cfg(not(windows))]
async fn kill_vnt_cli_process() -> Result<(), String> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::VntConfig;

    #[test]
    fn build_args_maps_config() {
        // 参数映射：-k token / -n 设备名 / -s 服务器（含协议前缀）/ -u mtu / --compressor
        let cfg = VntConfig {
            id: "1".into(),
            name: "测试".into(),
            token: "tok123".into(),
            device_name: Some("PC-Z".into()),
            device_id: Some("abc".into()),
            virtual_ip: Some("10.26.0.9".into()),
            server_address: Some("tcp://8.134.66.150:29872".into()),
            password: None,
            server_encrypt: true,
            in_ips: vec![],
            out_ips: vec![],
            compressor: Some("lz4".into()),
            mtu: Some(1400),
            use_tcp: true,
            use_ws: false,
            no_proxy: false,
            created_at: String::new(),
            updated_at: String::new(),
            last_used: None,
        };
        let args = build_args(&cfg);
        assert_eq!(
            args,
            vec![
                "-k", "tok123", "-n", "PC-Z", "-s", "tcp://8.134.66.150:29872", "-u", "1400",
                "--compressor", "lz4",
            ]
        );
    }

    #[test]
    fn build_args_minimal() {
        // 空设备名/服务器 → 只带 -k
        let cfg = VntConfig {
            token: "t".into(),
            ..Default::default()
        };
        assert_eq!(build_args(&cfg), vec!["-k", "t"]);
    }

    #[test]
    fn server_addr_parses_connect_log() {
        // 用户日志格式：connect count=1 ,address=8.134.66.150:29872
        assert_eq!(
            extract_server_addr("connect count=1 ,address=8.134.66.150:29872"),
            Some("8.134.66.150:29872".to_string())
        );
        assert_eq!(extract_server_addr("no address here"), None);
    }

    #[test]
    fn virtual_ip_parses_register_log() {
        assert_eq!(
            parse_virtual_ip("register ip=10.26.0.3 success"),
            Some("10.26.0.3".to_string())
        );
        assert_eq!(parse_virtual_ip("nothing"), None);
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn test_vnt_list_no_window() {
        // Bug 1 验证：CREATE_NO_WINDOW spawn 正常执行（无控制台窗口）
        let mut cmd = tokio::process::Command::new("cmd");
        cmd.args(["/c", "echo hello"]);
        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::piped());
        cmd.creation_flags(CREATE_NO_WINDOW);
        let output = cmd.output().await.expect("failed to spawn");
        assert!(output.status.success());
        assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "hello");
    }
}
