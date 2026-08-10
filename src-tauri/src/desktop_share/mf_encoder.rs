//! Media Foundation H.264 编码器（零 ffmpeg 依赖）
//!
//! 使用 Windows 系统自带的 H.264 MFT 编码器（Mfh264enc.dll，Windows 8+ 预装），
//! 输入 NV12 帧，输出 H.264 裸流（Annex-B，SPS/PPS 注入到关键帧前）。
//!
//! 要点：
//! - MFT 原生输出 AVCC 格式（4 字节大端长度前缀），转换为 Annex-B 起始码格式
//! - SPS/PPS（nal type 7/8）从输出流提取缓存，每个 IDR 关键帧前注入
//!   （WebCodecs 解码关键帧必须携带 SPS/PPS）
//! - Windows N 版（无媒体功能包）无 H.264 编码器，先调用 [is_encoder_available]
//! - 输出 Main profile（前端 WebCodecs 使用 avc1.4D401F 匹配）

use windows::core::Interface;
use windows::Win32::Media::MediaFoundation::*;
use windows::Win32::System::Com::*;
use windows::Win32::System::Variant::VARIANT;

use crate::desktop_share::error::DesktopError;

/// 编码后的单帧
#[derive(Debug)]
pub struct EncodedFrame {
    pub pts_ms: u64,
    pub is_keyframe: bool,
    /// Annex-B H.264 数据（关键帧含 SPS/PPS 前缀）
    pub data: Vec<u8>,
}

/// Media Foundation H.264 编码器
pub struct MfH264Encoder {
    mft: IMFTransform,
    width: u32,
    height: u32,
    fps: u32,
    /// 预分配输出 sample 的最大字节数（同步 MFT 要求调用者提供）
    max_out_size: usize,
    /// 缓存的 SPS+PPS（Annex-B 格式，含起始码）
    sps_pps: Vec<u8>,
}

/// 检查系统是否提供 H.264 编码器（Windows N 版可能缺失）
pub fn is_encoder_available() -> bool {
    #[cfg(windows)]
    {
        unsafe {
            // 必须先初始化 COM（未初始化时 CoCreateInstance 返回 CO_E_NOTINITIALIZED）
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
            CoCreateInstance::<_, IMFTransform>(&CLSID_MSH264EncoderMFT, None, CLSCTX_INPROC_SERVER)
                .is_ok()
        }
    }
    #[cfg(not(windows))]
    {
        false
    }
}

impl MfH264Encoder {
    /// 创建编码器并协商输入（NV12）/输出（H.264）类型
    pub fn new(
        width: u32,
        height: u32,
        bitrate: u32,
        fps: u32,
        quality: u32,
    ) -> Result<Self, DesktopError> {
        if width == 0 || height == 0 {
            return Err(DesktopError::Capture("分辨率无效".into()));
        }
        if width % 2 != 0 || height % 2 != 0 {
            return Err(DesktopError::Capture("分辨率必须是偶数（NV12 要求）".into()));
        }

        unsafe {
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        }

        let mft: IMFTransform = unsafe {
            CoCreateInstance(&CLSID_MSH264EncoderMFT, None, CLSCTX_INPROC_SERVER)
                .map_err(|e| DesktopError::Capture(format!("创建 H.264 编码器失败: {}", e)))?
        };

        // ---- 输出类型：H.264 Main Profile（必须先于输入类型：MS 编码器要求） ----
        let output_type: IMFMediaType = unsafe { MFCreateMediaType() }
            .map_err(|e| DesktopError::Capture(format!("创建输出媒体类型失败: {}", e)))?;
        unsafe {
            output_type.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
            output_type.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_H264)?;
            output_type.SetUINT64(&MF_MT_FRAME_SIZE, pack_size(width, height))?;
            output_type.SetUINT64(&MF_MT_FRAME_RATE, pack_rate(fps))?;
            output_type.SetUINT32(&MF_MT_AVG_BITRATE, bitrate)?;
            output_type.SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32)?;
            output_type.SetUINT32(&MF_MT_MPEG2_PROFILE, eAVEncH264VProfile_Main.0 as u32)?;
            // GOP 长度（帧数）：作为输出媒体类型属性设置（Chromium 实践）
            output_type.SetUINT32(&CODECAPI_AVEncMPVGOPSInSeq, fps * 2)?;
            mft.SetOutputType(0, &output_type, 0)
                .map_err(|e| DesktopError::Capture(format!("设置输出类型失败: {}", e)))?;
        }

        // ---- 输入类型：NV12（输出类型就绪后才能设置） ----
        let input_type: IMFMediaType = unsafe { MFCreateMediaType() }
            .map_err(|e| DesktopError::Capture(format!("创建输入媒体类型失败: {}", e)))?;
        unsafe {
            input_type.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
            input_type.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_NV12)?;
            input_type.SetUINT64(&MF_MT_FRAME_SIZE, pack_size(width, height))?;
            input_type.SetUINT64(&MF_MT_FRAME_RATE, pack_rate(fps))?;
            input_type.SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32)?;
            mft.SetInputType(0, &input_type, 0)
                .map_err(|e| DesktopError::Capture(format!("设置输入类型失败: {}", e)))?;
        }

        // ---- 低延迟 / 质量 / GOP 设置（ICodecAPI） ----
        unsafe {
            if let Ok(codec) = mft.cast::<ICodecAPI>() {
                let _ = codec.SetValue(&CODECAPI_AVLowLatencyMode, &VARIANT::from(true));
                let _ = codec.SetValue(
                    &CODECAPI_AVEncCommonRateControlMode,
                    &VARIANT::from(eAVEncCommonRateControlMode_Quality.0 as u32),
                );
                let _ = codec.SetValue(&CODECAPI_AVEncCommonQuality, &VARIANT::from(quality));
                // GOP = 2 秒（保证新连接者快速出图）；ICodecAPI 属性为 GOPSize
                let _ = codec.SetValue(&CODECAPI_AVEncMPVGOPSize, &VARIANT::from(fps * 2));
            }
        }

        unsafe {
            let _ = mft.ProcessMessage(MFT_MESSAGE_NOTIFY_BEGIN_STREAMING, 0);
        }

        log::info!(
            "MF H.264 编码器就绪: {}x{} @{}fps {}bps (Main)",
            width,
            height,
            fps,
            bitrate
        );

        Ok(Self {
            mft,
            width,
            height,
            fps,
            max_out_size: (width * height * 4) as usize,
            sps_pps: Vec::new(),
        })
    }

    /// 编码一帧 NV12 数据，返回编码帧（可能为 None：MFT 缓冲中暂未产出输出）
    pub fn encode(
        &mut self,
        nv12: &[u8],
        pts_ms: u64,
    ) -> Result<Option<EncodedFrame>, DesktopError> {
        let expected = (self.width * self.height * 3 / 2) as usize;
        if nv12.len() != expected {
            return Err(DesktopError::Capture(format!(
                "NV12 帧大小不符: 期望 {} 实际 {}",
                expected,
                nv12.len()
            )));
        }

        let mut out = Vec::with_capacity(64 * 1024);
        let mut saw_idr = false;

        unsafe {
            // 创建输入 sample
            let sample: IMFSample = MFCreateSample()
                .map_err(|e| DesktopError::Capture(format!("创建 sample 失败: {}", e)))?;
            let buffer: IMFMediaBuffer = MFCreateMemoryBuffer(nv12.len() as u32)
                .map_err(|e| DesktopError::Capture(format!("创建缓冲区失败: {}", e)))?;

            let mut ptr: *mut u8 = std::ptr::null_mut();
            let _ = buffer.Lock(&mut ptr, None, None);
            if ptr.is_null() {
                return Err(DesktopError::Capture("锁定输入缓冲区失败".into()));
            }
            std::ptr::copy_nonoverlapping(nv12.as_ptr(), ptr, nv12.len());
            let _ = buffer.Unlock();
            buffer.SetCurrentLength(nv12.len() as u32)?;
            sample.AddBuffer(&buffer)?;
            sample.SetSampleTime((pts_ms * 10_000) as i64)?; // 100ns 单位
            sample.SetSampleDuration((10_000_000 / self.fps) as i64)?; // 100ns 单位，编码器要求带持续时间

            // 送入编码器；输入被拒绝时先排空输出再重试
            let mut hr = self.mft.ProcessInput(0, &sample, 0);
            if hr.is_err() {
                drain_output(
                    &self.mft,
                    &mut self.sps_pps,
                    &mut out,
                    &mut saw_idr,
                    self.max_out_size,
                )?;
                hr = self.mft.ProcessInput(0, &sample, 0);
            }
            hr.map_err(|e| DesktopError::Capture(format!("编码输入失败: {}", e)))?;

            // 收集输出（0..n 个 sample）
            drain_output(
                &self.mft,
                &mut self.sps_pps,
                &mut out,
                &mut saw_idr,
                self.max_out_size,
            )?;
        }

        if out.is_empty() {
            return Ok(None);
        }

        Ok(Some(EncodedFrame {
            pts_ms,
            is_keyframe: saw_idr,
            data: out,
        }))
    }

    /// 冲刷编码器残留输出
    pub fn drain(&mut self) -> Result<(), DesktopError> {
        let mut out = Vec::new();
        let mut saw_idr = false;
        unsafe {
            let _ = self.mft.ProcessMessage(MFT_MESSAGE_COMMAND_DRAIN, 0);
            drain_output(
                &self.mft,
                &mut self.sps_pps,
                &mut out,
                &mut saw_idr,
                self.max_out_size,
            )?;
        }
        Ok(())
    }

    pub fn width(&self) -> u32 {
        self.width
    }

    pub fn height(&self) -> u32 {
        self.height
    }
}

/// 循环排空 MFT 输出：AVCC → Annex-B，SPS/PPS 缓存并注入 IDR 前
/// 同步 MFT 要求调用者预分配输出 sample（pSample 为 NULL 时 ProcessOutput 返回 E_INVALIDARG）
unsafe fn drain_output(
    mft: &IMFTransform,
    sps_pps: &mut Vec<u8>,
    out: &mut Vec<u8>,
    saw_idr: &mut bool,
    max_out_size: usize,
) -> Result<(), DesktopError> {
    loop {
        let mut output = MFT_OUTPUT_DATA_BUFFER::default();
        output.dwStreamID = 0;
        let sample: IMFSample = MFCreateSample()
            .map_err(|e| DesktopError::Capture(format!("创建输出 sample 失败: {}", e)))?;
        let buffer: IMFMediaBuffer = MFCreateMemoryBuffer(max_out_size as u32)
            .map_err(|e| DesktopError::Capture(format!("创建输出缓冲区失败: {}", e)))?;
        sample
            .AddBuffer(&buffer)
            .map_err(|e| DesktopError::Capture(format!("输出 sample 加缓冲区失败: {}", e)))?;
        *output.pSample = Some(sample);

        let mut status = 0u32;
        let hr = mft.ProcessOutput(0, std::slice::from_mut(&mut output), &mut status);

        if hr.is_ok() {
            let sample = std::mem::replace(&mut *output.pSample, None);
            if let Some(sample) = sample {
                let data = sample_to_bytes(&sample)?;
                if !data.is_empty() {
                    convert_avcc_to_annexb(&data, sps_pps, out, saw_idr);
                }
            }
        } else if let Err(e) = hr {
            let code = e.code();
            if code == MF_E_TRANSFORM_NEED_MORE_INPUT {
                break;
            }
            if code == MF_E_TRANSFORM_STREAM_CHANGE {
                // 输出流变更：清空输出类型重新协商
                let _ = mft.SetOutputType(0, None, 0);
                break;
            }
            log::warn!("ProcessOutput 错误: {}", e);
            break;
        }
    }
    Ok(())
}

/// 提取 IMFSample 的字节数据（遍历 sample 内全部 buffer——MFT 可能按 NAL 分 buffer）
unsafe fn sample_to_bytes(sample: &IMFSample) -> Result<Vec<u8>, DesktopError> {
    let count = sample
        .GetBufferCount()
        .map_err(|e| DesktopError::Capture(format!("获取输出 buffer 数失败: {}", e)))?;
    let mut all = Vec::with_capacity(64 * 1024);
    for i in 0..count {
        let buffer = sample
            .GetBufferByIndex(i)
            .map_err(|e| DesktopError::Capture(format!("获取输出缓冲区失败: {}", e)))?;
        let mut ptr: *mut u8 = std::ptr::null_mut();
        let mut current_len = 0u32;
        let _ = buffer.Lock(&mut ptr, None, Some(&mut current_len));
        if ptr.is_null() {
            return Err(DesktopError::Capture("锁定输出缓冲区失败".into()));
        }
        all.extend_from_slice(std::slice::from_raw_parts(ptr, current_len as usize));
        let _ = buffer.Unlock();
    }
    Ok(all)
}

/// AVCC/Annex-B → 统一 Annex-B 输出：SPS/PPS 去重缓存入 sps_pps，slice 追加到 out；IDR 前注入 SPS/PPS
/// 实测 MS H.264 编码器直接输出 Annex-B（起始码分隔），同时兼容 AVCC（4 字节长度前缀）输入
fn convert_avcc_to_annexb(
    avcc: &[u8],
    sps_pps: &mut Vec<u8>,
    out: &mut Vec<u8>,
    saw_idr: &mut bool,
) {
    // 格式检测：以 00 00 00 01 起始码开头 → Annex-B
    let is_annexb =
        avcc.len() >= 4 && avcc[0] == 0 && avcc[1] == 0 && avcc[2] == 0 && avcc[3] == 1;
    if is_annexb {
        convert_annexb_to_annexb(avcc, sps_pps, out, saw_idr);
        return;
    }
    let mut i = 0usize;
    while i + 4 <= avcc.len() {
        let len = u32::from_be_bytes([avcc[i], avcc[i + 1], avcc[i + 2], avcc[i + 3]]) as usize;
        if i + 4 + len > avcc.len() {
            break;
        }
        let nal = &avcc[i + 4..i + 4 + len];
        if nal.is_empty() {
            i += 4 + len;
            continue;
        }
        push_nal(nal, sps_pps, out, saw_idr);
        i += 4 + len;
    }
}

/// 起始码判断（4 字节或 3 字节；不足长度返回 false，不越界）
fn is_start_code(data: &[u8], pos: usize) -> bool {
    data[pos..].starts_with(&[0, 0, 0, 1]) || data[pos..].starts_with(&[0, 0, 1])
}

/// Annex-B → Annex-B：按起始码切分 NAL，SPS/PPS 缓存、IDR 注入
fn convert_annexb_to_annexb(
    data: &[u8],
    sps_pps: &mut Vec<u8>,
    out: &mut Vec<u8>,
    saw_idr: &mut bool,
) {
    let mut i = 0usize;
    while i < data.len() {
        if !is_start_code(data, i) {
            i += 1;
            continue;
        }
        let nal_start = if data[i..].starts_with(&[0, 0, 0, 1]) {
            i + 4
        } else {
            i + 3
        };
        // 找 NAL 结束（下一个起始码或末尾）
        let mut j = nal_start;
        while j < data.len() && !is_start_code(data, j) {
            j += 1;
        }
        if j == nal_start {
            // 保护：无法推进则退出，避免死循环
            break;
        }
        let nal = &data[nal_start..j];
        if !nal.is_empty() {
            push_nal(nal, sps_pps, out, saw_idr);
        }
        i = j;
    }
}

/// 单个 NAL 分发：SPS/PPS 缓存去重；IDR 前注入；其余直接追加
fn push_nal(nal: &[u8], sps_pps: &mut Vec<u8>, out: &mut Vec<u8>, saw_idr: &mut bool) {
    let nal_type = nal[0] & 0x1F;
    match nal_type {
        7 | 8 => {
            // SPS / PPS：全量去重后追加到缓存（Annex-B 形式）
            let mut needle = Vec::with_capacity(4 + nal.len());
            needle.extend_from_slice(&[0, 0, 0, 1]);
            needle.extend_from_slice(nal);
            let is_dup = sps_pps.windows(needle.len()).any(|w| w == &needle[..]);
            if !is_dup {
                sps_pps.extend_from_slice(&needle);
            }
        }
        5 => {
            // IDR 关键帧：先注入缓存的 SPS/PPS
            *saw_idr = true;
            out.extend_from_slice(sps_pps);
            out.extend_from_slice(&[0, 0, 0, 1]);
            out.extend_from_slice(nal);
        }
        _ => {
            out.extend_from_slice(&[0, 0, 0, 1]);
            out.extend_from_slice(nal);
        }
    }
}

/// 打包宽高为 MF_MT_FRAME_SIZE 的 64 位值（高 32 位宽，低 32 位高）
fn pack_size(width: u32, height: u32) -> u64 {
    ((width as u64) << 32) | (height as u64)
}

/// 打包帧率为 MF_MT_FRAME_RATE 的 64 位值（高 32 位分子，低 32 位分母）
fn pack_rate(fps: u32) -> u64 {
    ((fps as u64) << 32) | 1
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pack_values() {
        assert_eq!(pack_size(1920, 1080), (1920u64 << 32) | 1080);
        assert_eq!(pack_rate(30), (30u64 << 32) | 1);
    }

    #[test]
    fn avcc_to_annexb_with_sps_pps_injection() {
        // 构造 AVCC：SPS(7) + PPS(8) + IDR(5) + 非关键帧(1)
        let mut avcc = Vec::new();
        for nal in [
            vec![0x67, 0x64, 0x00], // SPS
            vec![0x68, 0xEB],       // PPS
            vec![0x65, 0x88, 0x84], // IDR
            vec![0x41, 0x9A],       // non-IDR
        ] {
            avcc.extend_from_slice(&(nal.len() as u32).to_be_bytes());
            avcc.extend_from_slice(&nal);
        }

        let mut sps_pps = Vec::new();
        let mut out = Vec::new();
        let mut saw_idr = false;
        convert_avcc_to_annexb(&avcc, &mut sps_pps, &mut out, &mut saw_idr);

        assert!(saw_idr);
        // SPS/PPS 缓存
        assert_eq!(
            sps_pps,
            vec![0, 0, 0, 1, 0x67, 0x64, 0x00, 0, 0, 0, 1, 0x68, 0xEB]
        );
        // 输出：IDR 前注入 SPS/PPS，non-IDR 正常
        let expected = vec![
            0, 0, 0, 1, 0x67, 0x64, 0x00, 0, 0, 0, 1, 0x68, 0xEB, // SPS/PPS
            0, 0, 0, 1, 0x65, 0x88, 0x84, // IDR
            0, 0, 0, 1, 0x41, 0x9A, // non-IDR
        ];
        assert_eq!(out, expected);
    }

    #[test]
    fn avcc_truncated_tail_ignored() {
        let avcc = vec![0, 0, 0, 3, 0x67, 0x64, 0x00, 0, 0, 0, 9, 0x65]; // 最后一个 NAL 长度不足
        let mut sps_pps = Vec::new();
        let mut out = Vec::new();
        let mut saw_idr = false;
        convert_avcc_to_annexb(&avcc, &mut sps_pps, &mut out, &mut saw_idr);
        // SPS 仅进入缓存（无 IDR 触发注入），out 为空
        assert_eq!(sps_pps, vec![0, 0, 0, 1, 0x67, 0x64, 0x00]);
        assert!(out.is_empty());
        assert!(!saw_idr);
    }

    #[test]
    fn sps_pps_dedup() {
        // 编码器可能重复输出 SPS/PPS，缓存不应膨胀
        let mut avcc = Vec::new();
        for nal in [vec![0x67, 0x64], vec![0x68, 0xEB], vec![0x67, 0x64], vec![0x68, 0xEB]] {
            avcc.extend_from_slice(&(nal.len() as u32).to_be_bytes());
            avcc.extend_from_slice(&nal);
        }
        let mut sps_pps = Vec::new();
        let mut out = Vec::new();
        let mut saw_idr = false;
        convert_avcc_to_annexb(&avcc, &mut sps_pps, &mut out, &mut saw_idr);
        assert_eq!(sps_pps.len(), 4 + 2 + 4 + 2, "SPS/PPS 应各缓存一次");
    }

    /// 真实 MF 编码器集成测试：编码合成 NV12 黑帧，验证 Annex-B 输出与关键帧 SPS/PPS
    #[test]
    fn mf_encoder_real_encode() {
        if !is_encoder_available() {
            eprintln!("系统无 H.264 编码器（Windows N 版？），跳过");
            return;
        }
        let mut enc =
            MfH264Encoder::new(640, 480, 1_000_000, 30, 50).expect("创建编码器失败");
        // 合成 NV12：Y=16（黑），UV=128
        let mut nv12 = vec![16u8; 640 * 480];
        nv12.extend_from_slice(&vec![128u8; 640 * 480 / 2]);

        let mut got_keyframe = false;
        let mut sps_profile: Option<u8> = None;
        let mut frames = 0u32;
        // 40 帧 < GOP(60)，整个 GOP 内应只有首帧 IDR
        for pts in 0..40u64 {
            if let Some(frame) = enc.encode(&nv12, pts * 33).expect("编码失败") {
                frames += 1;
                assert!(frame.data.len() >= 5, "输出帧过短");
                assert_eq!(&frame.data[0..4], &[0, 0, 0, 1], "必须为 Annex-B 起始码");
                if frame.is_keyframe {
                    if !got_keyframe {
                        // 验证 SPS 注入在 IDR 之前（编码器输出以 AUD 开头，不要求 SPS 是首个 NAL）
                        let mut saw_sps_pos: Option<usize> = None;
                        let mut idr_pos: Option<usize> = None;
                        let mut i = 0usize;
                        while i + 4 < frame.data.len() {
                            if frame.data[i..].starts_with(&[0, 0, 0, 1]) {
                                let t = frame.data[i + 4] & 0x1F;
                                if t == 7 && saw_sps_pos.is_none() {
                                    saw_sps_pos = Some(i);
                                }
                                if t == 5 {
                                    idr_pos = Some(i);
                                }
                                i += 4;
                            }
                            i += 1;
                        }
                        assert!(saw_sps_pos.is_some(), "关键帧应包含 SPS");
                        assert!(idr_pos.is_some(), "关键帧应包含 IDR");
                        assert!(
                            saw_sps_pos < idr_pos,
                            "SPS 应注入在 IDR 之前"
                        );
                        // profile_idc：SPS NAL 头之后第 1 字节
                        sps_profile = Some(frame.data[saw_sps_pos.unwrap() + 5]);
                    }
                    got_keyframe = true;
                }
            }
        }
        enc.drain().expect("drain 失败");
        eprintln!(
            "MF 编码器输出: {} 帧, SPS profile_idc = {:?}（66=Baseline 77=Main 100=High）",
            frames, sps_profile
        );
        assert!(frames > 0, "编码器应产生输出帧");
        assert!(got_keyframe, "编码 40 帧内应出现关键帧");
        assert_eq!(sps_profile, Some(77), "应为 Main profile（avc1.4D401F 匹配）");
    }
}
