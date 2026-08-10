// 远程画布：WebCodecs 解码 + Canvas 渲染 + 输入事件捕获
// 降级链：codec（Main→High→Baseline）× 加速策略（硬解→自动→软解），全部失败才报错
// 说明：WebCodecs 的 prefer-hardware 在无 GPU（如虚拟机）时不会自动回退软解，必须显式重试

import { useEffect, useRef, useState } from 'react';
import { Channel } from '@tauri-apps/api/core';
import { desktopApi } from '../../lib/desktopApi';
import { useDesktopStore } from '../../stores/useDesktopStore';
import type { InputEvent, VideoFramePayload } from '../../types/desktop';

interface Size {
  width: number;
  height: number;
}

// H.264 codec 候选（按兼容性顺序，decode 失败自动切换下一个）
const CODEC_CANDIDATES = ['avc1.4D401F', 'avc1.640028', 'avc1.42E01F'];

// 解码加速策略回退链：VM/无 GPU 环境硬解不可用时依次回退软解
const ACCEL_CANDIDATES = ['prefer-hardware', 'no-preference', 'prefer-software'] as const;

// 远程鼠标光标：彩色箭头（SVG data URI），区别于本机系统默认光标
const REMOTE_CURSOR = `url("data:image/svg+xml,${encodeURIComponent(
  '<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24"><path d="M3 2 L9 21 L11.5 13.5 L19 11 Z" fill="#3b82f6" stroke="#ffffff" stroke-width="1.5" stroke-linejoin="round"/></svg>',
)}") 0 0, default`;

export function RemoteCanvas() {
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const decoderRef = useRef<VideoDecoder | null>(null);
  const drawnSizeRef = useRef<Size | null>(null);
  const lastMouseSendRef = useRef(0);
  const [remoteSize, setRemoteSize] = useState<Size>({ width: 1920, height: 1080 });
  const [unsupported, setUnsupported] = useState(false);
  const [decoderError, setDecoderError] = useState<string | null>(null);
  // 尝试组合索引：attempt = codecIndex × accelIndex（0..codecs*accels-1）
  const [attempt, setAttempt] = useState(0);
  // 当前 decoder 是否已触发 error（error 后 decoder 自动 closed，只推进一次组合）
  const erroredRef = useRef(false);
  // 新解码器是否已收到关键帧：必须从关键帧开始解码（delta 帧无参考帧会解码失败）
  const gotKeyframeRef = useRef(false);
  // 会话状态：disconnected → sharing 等变化时重建解码器（二次连接/重连黑屏防御）
  const sessionType = useDesktopStore((s) => s.session.state.type);

  // 创建/重建解码器（attempt 变化时重建；codec × 加速策略组合自动重试）
  useEffect(() => {
    if (!('VideoDecoder' in window)) {
      setUnsupported(true);
      return;
    }
    const totalAttempts = CODEC_CANDIDATES.length * ACCEL_CANDIDATES.length;
    if (attempt >= totalAttempts) {
      setDecoderError('H.264 解码器不可用（硬解/软解均尝试失败），请确认系统支持 H.264 解码');
      return;
    }

    let dec: VideoDecoder | null = null;
    // 尝试顺序：codec0+硬解 → codec0+自动 → codec0+软解 → codec1+硬解 → ...
    const codec = CODEC_CANDIDATES[Math.floor(attempt / ACCEL_CANDIDATES.length)];
    const accel = ACCEL_CANDIDATES[attempt % ACCEL_CANDIDATES.length];
    erroredRef.current = false;

    try {
      dec = new VideoDecoder({
        output: (frame: VideoFrame) => {
          const canvas = canvasRef.current;
          if (!canvas) {
            frame.close();
            return;
          }
          const ctx = canvas.getContext('2d');
          if (!ctx) {
            frame.close();
            return;
          }
          const dw = frame.displayWidth;
          const dh = frame.displayHeight;
          const last = drawnSizeRef.current;
          if (!last || last.width !== dw || last.height !== dh) {
            canvas.width = dw;
            canvas.height = dh;
            drawnSizeRef.current = { width: dw, height: dh };
            setRemoteSize({ width: dw, height: dh });
          }
          ctx.drawImage(frame, 0, 0, dw, dh);
          frame.close();
        },
        error: (e) => {
          console.error(`VideoDecoder error (codec=${codec}, accel=${accel}):`, e);
          // 解码阶段失败 → 先换加速策略，策略用尽再换 codec（同一 decoder 只推进一次）
          if (!erroredRef.current) {
            erroredRef.current = true;
            setAttempt((a) => a + 1);
          }
        },
      });
      dec.configure({
        codec,
        optimizeForLatency: true,
        hardwareAcceleration: accel,
      });
    } catch (e) {
      console.error(`解码器初始化失败 (codec=${codec}, accel=${accel}):`, e);
      setAttempt((a) => a + 1);
      return;
    }

    decoderRef.current = dec;
    setDecoderError(null);
    // 新解码器必须等待关键帧才能开始解码
    gotKeyframeRef.current = false;

    return () => {
      // error 后 decoder 已自动进入 closed 状态，close() 会抛 InvalidStateError → 必须先判状态
      try {
        if (dec && dec.state !== 'closed') {
          dec.close();
        }
      } catch (e) {
        console.warn('关闭解码器失败:', e);
      }
      if (decoderRef.current === dec) {
        decoderRef.current = null;
      }
    };
  }, [attempt, sessionType]);

  // 注册视频帧通道（一次）
  useEffect(() => {
    if (!('VideoDecoder' in window)) return;
    const channel = new Channel<VideoFramePayload>();
    channel.onmessage = (payload) => {
      const d = decoderRef.current;
      if (!d || d.state !== 'configured') return;
      // 新解码器必须从关键帧开始：跳过关键帧前的 delta，避免无参考帧解码失败触发错误回退
      if (!gotKeyframeRef.current) {
        if (!payload.header.is_keyframe) {
          return;
        }
        gotKeyframeRef.current = true;
      }
      try {
        const bytes =
          payload.data instanceof Uint8Array
            ? payload.data
            : new Uint8Array(payload.data as number[]);
        const chunk = new EncodedVideoChunk({
          type: payload.header.is_keyframe ? 'key' : 'delta',
          timestamp: payload.header.pts * 1000,
          data: bytes as BufferSource,
        });
        d.decode(chunk);
      } catch (e) {
        console.warn('解码错误:', e);
      }
    };
    desktopApi.setVideoChannel(channel).catch((e) => {
      console.error('注册视频通道失败:', e);
    });

    // 卸载时清空回调：避免切页后 channel 仍接收视频帧（Tauri Channel 无显式关闭 API）
    return () => {
      channel.onmessage = () => {};
    };
  }, []);

  // 会话结束/断开时清空画布：避免旧帧残留造成"卡住/黑屏"错觉（二次连接黑屏防御）
  useEffect(() => {
    if (sessionType !== 'sharing') {
      const canvas = canvasRef.current;
      if (canvas) {
        const ctx = canvas.getContext('2d');
        ctx?.clearRect(0, 0, canvas.width, canvas.height);
      }
    }
  }, [sessionType]);

  // 输入事件发送（view_only 时不发）
  const sendInput = (event: InputEvent) => {
    const session = useDesktopStore.getState().session;
    if (session.capabilities?.view_only) return;
    useDesktopStore
      .getState()
      .sendInput(event)
      .catch(() => {});
  };

  const handleMouseMove = (e: React.MouseEvent<HTMLCanvasElement>) => {
    const now = Date.now();
    if (now - lastMouseSendRef.current < 16) return;
    lastMouseSendRef.current = now;

    const canvas = canvasRef.current;
    if (!canvas) return;
    const rect = canvas.getBoundingClientRect();
    if (rect.width === 0 || rect.height === 0) return;
    // 目标坐标系 = 被控端屏幕逻辑分辨率（非视频帧分辨率）：
    // 被控端 SetCursorPos 使用其屏幕坐标，若按缩放后的视频帧尺寸映射会偏移
    const session = useDesktopStore.getState().session;
    const targetW = session.screen?.width ?? remoteSize.width;
    const targetH = session.screen?.height ?? remoteSize.height;
    const scaleX = targetW / rect.width;
    const scaleY = targetH / rect.height;
    const x = Math.round((e.clientX - rect.left) * scaleX);
    const y = Math.round((e.clientY - rect.top) * scaleY);
    sendInput({ MouseMove: { x, y } });
  };

  const buttonMap = (button: number): number => (button === 0 ? 1 : button === 2 ? 2 : 3);

  const handleMouseDown = (e: React.MouseEvent<HTMLCanvasElement>) => {
    e.preventDefault();
    // 点击画布后获取焦点：使 onKeyDown/onKeyUp 能拦截键盘输入并传给被控端
    // （否则 preventDefault 会阻止 canvas 获得焦点，键盘操作永远无法生效）
    canvasRef.current?.focus();
    sendInput({ MouseButton: { button: buttonMap(e.button), pressed: true } });
  };

  const handleMouseUp = (e: React.MouseEvent<HTMLCanvasElement>) => {
    e.preventDefault();
    sendInput({ MouseButton: { button: buttonMap(e.button), pressed: false } });
  };

  const handleWheel = (e: React.WheelEvent<HTMLCanvasElement>) => {
    e.preventDefault();
    sendInput({ MouseScroll: { delta_x: Math.round(e.deltaX), delta_y: Math.round(e.deltaY) } });
  };

  const handleKeyDown = (e: React.KeyboardEvent<HTMLCanvasElement>) => {
    e.preventDefault();
    sendInput({ KeyDown: { key: e.key } });
  };

  const handleKeyUp = (e: React.KeyboardEvent<HTMLCanvasElement>) => {
    e.preventDefault();
    sendInput({ KeyUp: { key: e.key } });
  };

  if (unsupported) {
    return (
      <div
        style={{
          height: 400,
          display: 'flex',
          alignItems: 'center',
          justifyContent: 'center',
          background: '#1a1a1a',
          borderRadius: 8,
          color: '#e6a23c',
        }}
      >
        当前 WebView 不支持 WebCodecs API，无法解码远程画面
      </div>
    );
  }

  if (decoderError) {
    return (
      <div
        style={{
          height: 400,
          display: 'flex',
          alignItems: 'center',
          justifyContent: 'center',
          background: '#1a1a1a',
          borderRadius: 8,
          color: '#ff4d4f',
          padding: 16,
          textAlign: 'center',
        }}
      >
        {decoderError}
      </div>
    );
  }

  return (
    <canvas
      ref={canvasRef}
      style={{
        maxWidth: '100%',
        maxHeight: '100%',
        width: 'auto',
        height: 'auto',
        background: '#000',
        borderRadius: 8,
        cursor: REMOTE_CURSOR,
        outline: 'none',
      }}
      onMouseMove={handleMouseMove}
      onMouseDown={handleMouseDown}
      onMouseUp={handleMouseUp}
      onContextMenu={(e) => e.preventDefault()}
      onWheel={handleWheel}
      onKeyDown={handleKeyDown}
      onKeyUp={handleKeyUp}
      tabIndex={0}
    />
  );
}
