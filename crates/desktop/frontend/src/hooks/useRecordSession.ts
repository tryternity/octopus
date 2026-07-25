/**
 * useRecordSession —— 录屏会话状态机（前端订阅 helper 事件流）。
 *
 * 与后端 `crates/record/src/protocol.rs::HelperEvent` 对齐：
 *   { event: "ready" | "recording-started" | "recording-paused" |
 *     "recording-resumed" | "recording-stopped" | "warning" | "error" }
 *   tag 用 kebab-case（serde rename_all = "kebab-case"）。
 *
 * 协议层：record_start 命令注入一个 callback，把 helper 进程 stdout 的事件
 * 经 Tauri emit("record://event", &e) 推给前端。本 hook 订阅该事件，
 * 根据事件 tag 切换 SessionState。
 *
 * duration 在 state==="recording" 时每秒 +1（前端计时器近似；spec §9.2 F19
 * 可改成由 helper 回报真实 timestamp_ms 差值）。
 *
 * 注意：本 hook 主要服务于未来的配置浮窗 / tray 菜单（控制 start/pause/stop），
 * RecordingPanel 也可以读取 state 在顶部显示「正在录制中」状态。当前 Task 13 MVP
 * 仅完成订阅与控制 API；具体触发由调用方决定。
 */
import { useCallback, useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import {
  listen as rawListen,
  type Event,
  type UnlistenFn,
} from "@tauri-apps/api/event";

// ── 后端协议类型（前端镜像，与 crates/record/src/protocol.rs 对齐）──────────────

/** HelperEvent：helper 子进程 → 主进程 → 前端 的事件流。tag 用 kebab-case。 */
export type HelperEvent =
  | { event: "ready"; schema_version: number }
  | {
      event: "recording-started";
      timestamp_ms: number;
      width: number;
      height: number;
    }
  | { event: "recording-paused"; timestamp_ms: number }
  | { event: "recording-resumed"; timestamp_ms: number }
  | {
      event: "recording-stopped";
      screen_path: string;
      duration_ms: number;
      file_size: number;
    }
  | { event: "warning"; code: string; message: string }
  | { event: "error"; code: string; message: string };

/** SessionState：本 hook 暴露的会话状态机。 */
export type SessionState = "idle" | "starting" | "recording" | "paused";

// ── RecordConfig（与 desktop/src/record_commands.rs::RecordConfig 对齐）─────────

/** Source：录制源。tag 用 lowercase（serde rename_all = "lowercase"）。 */
export type RecordSource =
  | { type: "display"; display_id: number }
  | { type: "window"; window_id: number }
  | {
      type: "area";
      display_id: number;
      x: number;
      y: number;
      width: number;
      height: number;
    };

export type VideoCodec = "h264" | "hevc";

export interface VideoConfig {
  fps: number;
  width: number;
  height: number;
  codec: VideoCodec;
  bitrate: number | null;
  hide_system_cursor: boolean;
}

export interface AudioConfig {
  system: { enabled: boolean; excludes_current_process: boolean };
  microphone: {
    enabled: boolean;
    device_id: string | null;
    device_name: string | null;
  };
}

export interface RecordConfig {
  source: RecordSource;
  video: VideoConfig;
  audio: AudioConfig;
}

// ── Hook 实现 ─────────────────────────────────────────────────

export interface UseRecordSessionApi {
  /** 当前会话状态。 */
  state: SessionState;
  /** 当前已录制秒数（recording 状态下每秒 +1）。 */
  duration: number;
  /** 最近一次 warning / error 事件（如有）。 */
  lastWarning: { code: string; message: string } | null;
  /** 启动录制。state 会经过 starting → recording。 */
  start: (config: RecordConfig) => Promise<void>;
  /** 暂停录制。 */
  pause: () => Promise<void>;
  /** 恢复录制。 */
  resume: () => Promise<void>;
  /** 停止录制（外部 record_stop 命令的入参由调用方持有，本 hook 只切回 idle）。 */
  stop: () => Promise<void>;
}

export function useRecordSession(): UseRecordSessionApi {
  const [state, setState] = useState<SessionState>("idle");
  const [duration, setDuration] = useState(0);
  const [lastWarning, setLastWarning] = useState<
    { code: string; message: string } | null
  >(null);

  // setInterval 句柄——用 ref 持有以便 unmount / state 切换时清理。
  const timerRef = useRef<ReturnType<typeof setInterval> | null>(null);

  const clearTimer = useCallback(() => {
    if (timerRef.current !== null) {
      clearInterval(timerRef.current);
      timerRef.current = null;
    }
  }, []);

  const startTimer = useCallback(() => {
    clearTimer();
    setDuration(0);
    timerRef.current = setInterval(() => {
      setDuration((d) => d + 1);
    }, 1000);
  }, [clearTimer]);

  // ── 订阅 helper 事件流 ──
  useEffect(() => {
    let unlisten: UnlistenFn | undefined;
    let cancelled = false;
    rawListen<HelperEvent>("record://event", (e: Event<HelperEvent>) => {
      const evt = e.payload;
      switch (evt.event) {
        case "recording-started":
          setState("recording");
          startTimer();
          break;
        case "recording-paused":
          setState("paused");
          clearTimer();
          break;
        case "recording-resumed":
          setState("recording");
          startTimer();
          break;
        case "recording-stopped":
          setState("idle");
          clearTimer();
          setDuration(0);
          break;
        case "warning":
          setLastWarning({ code: evt.code, message: evt.message });
          break;
        case "error":
          setLastWarning({ code: evt.code, message: evt.message });
          setState("idle");
          clearTimer();
          break;
        case "ready":
          // helper ready 仅是进程启动信号，不改 state（record_start 返回才转 starting）
          break;
      }
    }).then((fn) => {
      if (cancelled) fn();
      else unlisten = fn;
    });
    return () => {
      cancelled = true;
      unlisten?.();
      clearTimer();
    };
  }, [startTimer, clearTimer]);

  const start = useCallback(async (config: RecordConfig) => {
    setState("starting");
    try {
      // StartedInfo 字段见 record session.rs；本 hook MVP 不消费返回值。
      await invoke("record_start", { config });
    } catch (e) {
      setState("idle");
      throw e;
    }
  }, []);

  const pause = useCallback(async () => {
    await invoke("record_pause");
    setState("paused");
    clearTimer();
  }, [clearTimer]);

  const resume = useCallback(async () => {
    await invoke("record_resume");
    setState("recording");
    startTimer();
  }, [startTimer]);

  const stop = useCallback(async () => {
    // record_stop 入参（recording_id / dimensions / audio flags）由调用方持有
    // 的完整 RecordConfig 决定，本 hook MVP 不持有这些上下文——调用方应直接
    // invoke("record_stop", {...})，本方法仅作为状态清理的便捷入口。
    setState("idle");
    clearTimer();
    setDuration(0);
  }, [clearTimer]);

  return { state, duration, lastWarning, start, pause, resume, stop };
}
