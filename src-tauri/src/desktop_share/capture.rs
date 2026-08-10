//! 屏幕捕获引擎（零 ffmpeg 依赖）
//!
//! - 捕获：windows-capture 2.0（DXGI Desktop Duplication，事件驱动 + 33ms 节流 ≈ 30fps）
//! - 转换：BGRA8 → NV12（CPU，BT.601 limited range，2×2 色度下采样）
//! - 编码：Media Foundation H.264 MFT（系统自带，Main profile）
//! - 输出：Annex-B H.264 帧（含 SPS/PPS 注入），经 channel 交给网络层

use windows_capture::dxgi_duplication_api::{DxgiDuplicationApi, DxgiDuplicationFormat, Error as DupError};
use windows_capture::monitor::Monitor;

use tokio::sync::mpsc;

use crate::desktop_share::error::DesktopError;
use crate::desktop_share::mf_encoder::MfH264Encoder;
use crate::desktop_share::protocol::{ScreenInfo, VideoFrameHeader};

/// 捕获配置
#[derive(Debug, Clone)]
pub struct CaptureConfig {
    /// 显示器索引（0 = 主显示器）
    pub monitor: usize,
    /// 目标帧率
    pub fps: u32,
    /// 目标码率（bps）
    pub bitrate: u32,
    /// 输出分辨率宽度（暂不支持缩放，捕获原生分辨率）
    pub width: u32,
    /// 输出分辨率高度（暂不支持缩放）
    pub height: u32,
    /// 编码质量（MF AVEncCommonQuality，0-100，默认 50）
    pub quality: u32,
}

impl Default for CaptureConfig {
    fn default() -> Self {
        Self {
            monitor: 0,
            fps: 30,
            bitrate: 2_000_000,
            width: 0,
            height: 0,
            quality: 50,
        }
    }
}

/// 屏幕捕获器
pub struct ScreenCapturer {
    config: CaptureConfig,
}

impl ScreenCapturer {
    pub fn new(config: CaptureConfig) -> Self {
        Self { config }
    }

    /// 启动捕获 + 编码，H.264 帧通过 tx 发送；返回任务句柄（停止时 abort）
    pub fn start(
        self,
        tx: mpsc::Sender<(VideoFrameHeader, Vec<u8>)>,
    ) -> Result<std::thread::JoinHandle<()>, DesktopError> {
        let handle = std::thread::Builder::new()
            .name("desktop-capture".into())
            .spawn(move || {
                if let Err(e) = capture_loop(&self.config, tx) {
                    log::error!("屏幕捕获循环退出: {}", e);
                }
            })
            .map_err(|e| DesktopError::Capture(format!("创建捕获线程失败: {}", e)))?;
        Ok(handle)
    }
}

/// 捕获主循环（阻塞线程内运行）
fn capture_loop(
    cfg: &CaptureConfig,
    tx: mpsc::Sender<(VideoFrameHeader, Vec<u8>)>,
) -> Result<(), DesktopError> {
    let monitor = select_monitor(cfg.monitor)?;
    let mut encoder = create_encoder(&monitor, cfg)?;

    let mut nv12_buf: Vec<u8> = Vec::new();
    let mut bgra_buf: Vec<u8> = Vec::new();
    let mut scaled_bgra: Vec<u8> = Vec::new();
    let mut pts_counter: u64 = 0;
    let start = std::time::Instant::now();
    let mut frames_encoded: u64 = 0;

    // 帧率节流：DXGI 桌面变化时可达显示器刷新率（远超配置 fps），这里真正限帧
    let frame_interval = std::time::Duration::from_micros(1_000_000 / cfg.fps.max(1) as u64);
    let mut last_send = start.checked_sub(frame_interval).unwrap_or(start);

    loop {
        // 每帧重建 duplication（AccessLost 后自动恢复；开销可忽略）
        let mut dup = match DxgiDuplicationApi::new_options(
            monitor.clone(),
            &[DxgiDuplicationFormat::Bgra8],
        ) {
            Ok(d) => d,
            Err(e) => {
                log::warn!("创建屏幕捕获会话失败: {}", e);
                std::thread::sleep(std::time::Duration::from_millis(1000));
                continue;
            }
        };

        'frames: loop {
            // 等待新帧（33ms 上限 = ~30fps 节流）
            let mut frame = match dup.acquire_next_frame(33) {
                Ok(f) => f,
                Err(DupError::Timeout) => {
                    // 屏幕无变化：跳过本周期
                    continue;
                }
                Err(DupError::AccessLost) => {
                    log::info!("屏幕捕获会话丢失（分辨率变化/会话切换），重建中");
                    break 'frames;
                }
                Err(e) => {
                    log::warn!("获取帧失败: {}", e);
                    std::thread::sleep(std::time::Duration::from_millis(500));
                    break 'frames;
                }
            };

            // 空帧（桌面未变化的首帧）跳过
            if frame.frame_info().LastPresentTime == 0 {
                continue;
            }

            // 帧率节流：未到发送间隔则跳过本帧（继续等待下一帧变化）
            let now = std::time::Instant::now();
            if now.duration_since(last_send) < frame_interval {
                continue;
            }
            last_send = now;

            let width = frame.width();
            let height = frame.height();
            if width % 2 != 0 || height % 2 != 0 {
                log::warn!("捕获分辨率非偶数 ({x}x{y})，跳过本帧", x = width, y = height);
                continue;
            }

            // 目标输出分辨率：配置有效且小于物理时按比例缩小（不放大、保持宽高比）
            let (dst_w, dst_h) = target_size(width, height, cfg);

            // 分辨率变化 → 以目标分辨率重建编码器
            if encoder.width() != dst_w || encoder.height() != dst_h {
                log::info!(
                    "分辨率变化: {}x{} → 输出 {}x{}",
                    encoder.width(),
                    encoder.height(),
                    dst_w,
                    dst_h
                );
                encoder = create_encoder_resolved(dst_w, dst_h, cfg)?;
            }

            // CPU 读回 BGRA
            let buf = match frame.buffer() {
                Ok(b) => b,
                Err(e) => {
                    log::warn!("读取帧缓冲失败: {}", e);
                    continue;
                }
            };
            bgra_buf.clear();
            let bgra = buf.as_nopadding_buffer(&mut bgra_buf);
            if bgra.len() < (width as usize) * (height as usize) * 4 {
                continue;
            }

            // BGRA → NV12（物理分辨率 ≠ 目标时先双线性缩放到目标尺寸，再颜色转换）
            if dst_w == width && dst_h == height {
                bgra_to_nv12(bgra, width, height, &mut nv12_buf);
            } else {
                scale_bgra(bgra, width, height, dst_w, dst_h, &mut scaled_bgra);
                bgra_to_nv12(&scaled_bgra, dst_w, dst_h, &mut nv12_buf);
            }

            // H.264 编码
            let pts_ms = start.elapsed().as_millis() as u64;
            match encoder.encode(&nv12_buf, pts_ms) {
                Ok(Some(frame)) => {
                    frames_encoded += 1;
                    if tx
                        .blocking_send((
                            VideoFrameHeader {
                                pts: pts_counter,
                                is_keyframe: frame.is_keyframe,
                                width,
                                height,
                                data_len: frame.data.len() as u32,
                            },
                            frame.data,
                        ))
                        .is_err()
                    {
                        log::info!("捕获端接收者已关闭，退出");
                        return Ok(());
                    }
                    pts_counter += 1;
                }
                Ok(None) => {}
                Err(e) => {
                    log::warn!("编码失败: {}", e);
                }
            }
        }

        // 会话重建前小憩，避免空转
        std::thread::sleep(std::time::Duration::from_millis(100));
        let _ = frames_encoded;
    }
}

/// 选择显示器（按索引，0 = 主显示器）
fn select_monitor(index: usize) -> Result<Monitor, DesktopError> {
    if index == 0 {
        Monitor::primary()
            .map_err(|e| DesktopError::Capture(format!("获取主显示器失败: {}", e)))
    } else {
        Monitor::from_index(index)
            .map_err(|e| DesktopError::Capture(format!("获取显示器 {} 失败: {}", index, e)))
    }
}

/// 根据显示器分辨率创建编码器
fn create_encoder(monitor: &Monitor, cfg: &CaptureConfig) -> Result<MfH264Encoder, DesktopError> {
    let width = monitor
        .width()
        .map_err(|e| DesktopError::Capture(format!("获取显示器宽度失败: {}", e)))?;
    let height = monitor
        .height()
        .map_err(|e| DesktopError::Capture(format!("获取显示器高度失败: {}", e)))?;
    create_encoder_resolved(width, height, cfg)
}

/// 以指定分辨率创建编码器（偶数校验）
fn create_encoder_resolved(
    width: u32,
    height: u32,
    cfg: &CaptureConfig,
) -> Result<MfH264Encoder, DesktopError> {
    let w = if width % 2 == 0 { width } else { width - 1 };
    let h = if height % 2 == 0 { height } else { height - 1 };
    MfH264Encoder::new(w, h, cfg.bitrate, cfg.fps, cfg.quality)
}

/// BGRA8 → NV12（BT.601 limited range，2×2 色度平均）
/// 输出布局：Y 平面 w*h，随后 UV 交错平面（(w/2)*(h/2)*2，U 在前，行步幅 = 偶数化宽度）
/// 奇数尺寸输入自动偶数化（丢弃最右/最下行），不会 panic
pub fn bgra_to_nv12(bgra: &[u8], width: u32, height: u32, out: &mut Vec<u8>) {
    let ow = width as usize;
    let oh = height as usize;
    let w = (ow & !1).max(2); // 偶数化
    let h = (oh & !1).max(2);
    out.resize(w * h + (w / 2) * (h / 2) * 2, 0);

    let (y_plane, uv_plane) = out.split_at_mut(w * h);

    // Y 平面（每像素）
    for y in 0..h {
        let row = y * w;
        for x in 0..w {
            let py = y.min(oh - 1);
            let px = x.min(ow - 1);
            let i = (py * ow + px) * 4;
            let b = bgra[i] as i32;
            let g = bgra[i + 1] as i32;
            let r = bgra[i + 2] as i32;
            // BT.601: Y = 0.257R + 0.504G + 0.098B + 16（整数近似）
            let yv = ((66 * r + 129 * g + 25 * b) >> 8) + 16;
            y_plane[row + x] = yv.clamp(16, 235) as u8;
        }
    }

    // UV 平面（2×2 块平均，U/V 交错，行步幅 = w）
    for y in (0..h).step_by(2) {
        for x in (0..w).step_by(2) {
            let mut r = 0i32;
            let mut g = 0i32;
            let mut b = 0i32;
            for dy in 0..2 {
                let py = (y + dy).min(oh - 1);
                for dx in 0..2 {
                    let px = (x + dx).min(ow - 1);
                    let i = (py * ow + px) * 4;
                    b += bgra[i] as i32;
                    g += bgra[i + 1] as i32;
                    r += bgra[i + 2] as i32;
                }
            }
            r >>= 2;
            g >>= 2;
            b >>= 2;
            // BT.601: U = -0.148R - 0.291G + 0.439B + 128；V = 0.439R - 0.368G - 0.071B + 128
            let u = ((-38 * r - 74 * g + 112 * b) >> 8) + 128;
            let v = ((112 * r - 94 * g - 18 * b) >> 8) + 128;
            let uv_idx = (y / 2) * w + x;
            uv_plane[uv_idx] = u.clamp(16, 240) as u8;
            uv_plane[uv_idx + 1] = v.clamp(16, 240) as u8;
        }
    }
}

/// 计算输出目标分辨率：配置有效且小于物理时按比例缩小（保持宽高比、偶数化；不放大）
fn target_size(phys_w: u32, phys_h: u32, cfg: &CaptureConfig) -> (u32, u32) {
    let cfg_w = cfg.width;
    let cfg_h = cfg.height;
    if cfg_w == 0 || cfg_h == 0 {
        return (phys_w, phys_h);
    }
    if cfg_w >= phys_w && cfg_h >= phys_h {
        return (phys_w, phys_h); // 配置不小于物理 → 不放大
    }
    // 保持宽高比缩小（fit 到配置边界内）
    let scale = (cfg_w as f64 / phys_w as f64).min(cfg_h as f64 / phys_h as f64);
    let w = (((phys_w as f64 * scale).round() as u32).max(2)) & !1;
    let h = (((phys_h as f64 * scale).round() as u32).max(2)) & !1;
    (w, h)
}

/// BGRA 双线性缩放（纯 CPU，零新依赖；双线性插值保持清晰度）
fn scale_bgra(src: &[u8], src_w: u32, src_h: u32, dst_w: u32, dst_h: u32, out: &mut Vec<u8>) {
    let sw = src_w as usize;
    let sh = src_h as usize;
    let dw = dst_w as usize;
    let dh = dst_h as usize;
    out.resize(dw * dh * 4, 0);
    if dw == sw && dh == sh {
        out.copy_from_slice(src);
        return;
    }
    let sx = sw as f64 / dw as f64;
    let sy = sh as f64 / dh as f64;
    for dy in 0..dh {
        let fy = dy as f64 * sy;
        let y0 = (fy as usize).min(sh - 1);
        let y1 = (y0 + 1).min(sh - 1);
        let wy = fy - y0 as f64;
        for dx in 0..dw {
            let fx = dx as f64 * sx;
            let x0 = (fx as usize).min(sw - 1);
            let x1 = (x0 + 1).min(sw - 1);
            let wx = fx - x0 as f64;
            let r0 = (y0 * sw + x0) * 4;
            let r1 = (y0 * sw + x1) * 4;
            let r2 = (y1 * sw + x0) * 4;
            let r3 = (y1 * sw + x1) * 4;
            let o = (dy * dw + dx) * 4;
            for c in 0..4 {
                let v00 = src[r0 + c] as f64;
                let v10 = src[r1 + c] as f64;
                let v01 = src[r2 + c] as f64;
                let v11 = src[r3 + c] as f64;
                let top = v00 + (v10 - v00) * wx;
                let bot = v01 + (v11 - v01) * wx;
                out[o + c] = ((top + (bot - top) * wy).round()) as u8;
            }
        }
    }
}

/// 获取主显示器屏幕信息（Windows API）
pub async fn get_screen_info() -> ScreenInfo {
    #[cfg(windows)]
    {
        unsafe {
            use windows_sys::Win32::UI::WindowsAndMessaging::{
                GetSystemMetrics, SM_CMONITORS, SM_CXSCREEN, SM_CYSCREEN,
            };
            let w = GetSystemMetrics(SM_CXSCREEN) as u32;
            let h = GetSystemMetrics(SM_CYSCREEN) as u32;
            let monitors = GetSystemMetrics(SM_CMONITORS) as u32;
            if w > 0 && h > 0 {
                return ScreenInfo {
                    width: w,
                    height: h,
                    dpi: 96,
                    monitor_count: monitors.max(1),
                };
            }
        }
    }
    ScreenInfo {
        width: 1920,
        height: 1080,
        dpi: 96,
        monitor_count: 1,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_bgra(w: usize, h: usize, r: u8, g: u8, b: u8) -> Vec<u8> {
        let mut buf = Vec::with_capacity(w * h * 4);
        for _ in 0..w * h {
            buf.extend_from_slice(&[b, g, r, 255]);
        }
        buf
    }

    #[test]
    fn nv12_output_size() {
        // 4x4 纯红
        let bgra = make_bgra(4, 4, 255, 0, 0);
        let mut nv12 = Vec::new();
        bgra_to_nv12(&bgra, 4, 4, &mut nv12);
        assert_eq!(nv12.len(), 4 * 4 * 3 / 2); // 16 + 8
        // Y 平面全黑? 不：纯红 → Y ≈ 81（0.257*255+16）
        let y_expected = ((66 * 255) >> 8) + 16;
        assert_eq!(nv12[0], y_expected.clamp(16, 235) as u8);
    }

    #[test]
    fn nv12_uv_layout() {
        // 2x2 纯蓝 → U 高 V 低
        let bgra = make_bgra(2, 2, 0, 0, 255);
        let mut nv12 = Vec::new();
        bgra_to_nv12(&bgra, 2, 2, &mut nv12);
        let u = nv12[4]; // Y 平面 2*2=4 字节后
        let v = nv12[5];
        assert!(u > v, "蓝色应 U>V，实际 U={} V={}", u, v);
        assert!(u > 128);
        assert!(v < 128);
    }

    #[test]
    fn nv12_gray_same_uv() {
        // 灰色 → U≈V≈128
        let bgra = make_bgra(2, 2, 128, 128, 128);
        let mut nv12 = Vec::new();
        bgra_to_nv12(&bgra, 2, 2, &mut nv12);
        let u = nv12[4];
        let v = nv12[5];
        assert!((u as i32 - 128).abs() <= 2, "U={}", u);
        assert!((v as i32 - 128).abs() <= 2, "V={}", v);
    }

    #[test]
    fn nv12_odd_edges_clamped() {
        // 奇数尺寸自动偶数化（3x3 → 2x2），不应 panic
        let bgra = make_bgra(3, 3, 10, 200, 30);
        let mut nv12 = Vec::new();
        bgra_to_nv12(&bgra, 3, 3, &mut nv12);
        assert_eq!(nv12.len(), 2 * 2 + 1 * 1 * 2); // Y 4 + UV 2
    }
}
