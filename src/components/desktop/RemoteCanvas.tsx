// 远程画布：WebCodecs 硬件解码 + Canvas 渲染 + 输入事件捕获
// codec 降级链：Main → High → Baseline（与 MF 编码器实际输出匹配，自动重试）

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

export function RemoteCanvas() {
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const decoderRef = useRef<VideoDecoder | null>(null);
  const drawnSizeRef = useRef<Size | null>(null);
  const lastMouseSendRef = useRef(0);
  const [remoteSize, setRemoteSize] = useState<Size>({ width: 1920, height: 1080 });
  const [unsupported, setUnsupported] = useState(false);
  const [decoderError, setDecoderError] = useState<string | null>(null);
  const [codecIndex, setCodecIndex] = useState(0);
  // 当前 decoder 是否已触发 error（error 后 decoder 自动 closed，只切一次 codec）
  const erroredRef = useRef(false);

  // 创建/重建解码器（codecIndex 变化时重建；decode 错误自动切换 codec）
  useEffect(() => {
    if (!('VideoDecoder' in window)) {
      setUnsupported(true);
      return;
    }
    if (codecIndex >= CODEC_CANDIDATES.length) {
      setDecoderError('H.264 解码器尝试全部失败（当前 WebView 不支持 avc1 硬解）');
      return;
    }

    let dec: VideoDecoder | null = null;
    const codec = CODEC_CANDIDATES[codecIndex];
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
          console.error(`VideoDecoder error (codec=${codec}):`, e);
          // 解码阶段失败 → 自动切换下一个 codec 重试（同一 decoder 只切一次）
          if (!erroredRef.current) {
            erroredRef.current = true;
            setCodecIndex((i) => i + 1);
          }
        },
      });
      dec.configure({
        codec,
        optimizeForLatency: true,
        hardwareAcceleration: 'prefer-hardware',
      });
    } catch (e) {
      console.error(`codec ${codec} 初始化失败:`, e);
      setCodecIndex((i) => i + 1);
      return;
    }

    decoderRef.current = dec;
    setDecoderError(null);

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
  }, [codecIndex]);

  // 注册视频帧通道（一次）
  useEffect(() => {
    if (!('VideoDecoder' in window)) return;
    const channel = new Channel<VideoFramePayload>();
    channel.onmessage = (payload) => {
      const d = decoderRef.current;
      if (!d || d.state !== 'configured') return;
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
  }, []);

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
    const scaleX = remoteSize.width / rect.width;
    const scaleY = remoteSize.height / rect.height;
    const x = Math.round((e.clientX - rect.left) * scaleX);
    const y = Math.round((e.clientY - rect.top) * scaleY);
    sendInput({ MouseMove: { x, y } });
  };

  const buttonMap = (button: number): number => (button === 0 ? 1 : button === 2 ? 2 : 3);

  const handleMouseDown = (e: React.MouseEvent<HTMLCanvasElement>) => {
    e.preventDefault();
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
        width: '100%',
        height: 'auto',
        minHeight: 400,
        background: '#000',
        borderRadius: 8,
        cursor: 'crosshair',
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
