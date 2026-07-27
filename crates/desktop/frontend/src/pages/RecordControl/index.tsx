/**
 * RecordControl —— display/window 录制时的桌面控制 pill。
 *
 * 屏幕右下角 fixed 位置（位置由后端 record_control_window.rs 算好）。
 * 紧凑布局：红点 + 时长 + 暂停/恢复按钮 + 停止按钮。
 *
 * **关键设计：不依赖 useRecordSession 的 state**——浮窗创建晚于 recording-started
 * 事件，hook 的监听器收不到该事件，state 会一直 idle。改用：
 * - mount 时 invoke get_record_status 拿真实 state + elapsed_secs
 * - 本地 currentState state（初始从 get_record_status 来）
 * - 监听 record://event 的 recording-paused/resumed/stopped 更新本地 state
 * - 本地 setInterval 在 currentState === "recording" 时累加 displayDuration
 */
import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { emit, listen as rawListen, type UnlistenFn, type Event } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { useRecordSession } from "@/hooks/useRecordSession";
import { useT } from "@/lib/i18n";

/** helper event payload（与后端 HelperEvent 对齐，只取关心的字段） */
interface HelperEventLite {
  event: "recording-started" | "recording-paused" | "recording-resumed" | "recording-stopped" | "ready" | "warning" | "error";
}

function formatDuration(secs: number): string {
  const totalSec = Math.max(0, Math.floor(secs));
  const h = Math.floor(totalSec / 3600);
  const m = Math.floor((totalSec % 3600) / 60);
  const s = totalSec % 60;
  const pad = (n: number) => n.toString().padStart(2, "0");
  if (h > 0) return `${h}:${pad(m)}:${pad(s)}`;
  return `${pad(m)}:${pad(s)}`;
}

type RecState = "idle" | "recording" | "paused";

export default function RecordControl() {
  const t = useT();
  // 只用 useRecordSession 的 pause/resume actions，不用它的 state（mount 时是错的 idle）
  const session = useRecordSession();

  // 本地录制状态——从 get_record_status 初始化，监听 record://event 更新
  const [currentState, setCurrentState] = useState<RecState>("recording"); // 默认 recording（浮窗只在录制中创建）
  const [displayDuration, setDisplayDuration] = useState(0);
  const [synced, setSynced] = useState(false);

  // mount 时拿真实状态 + 已录秒数
  useEffect(() => {
    let cancelled = false;
    invoke<{ state: string; elapsedSecs: number }>("get_record_status")
      .then((status) => {
        if (cancelled) return;
        const s = status.state;
        setCurrentState(s === "paused" ? "paused" : s === "recording" ? "recording" : "idle");
        setDisplayDuration(status.elapsedSecs);
        setSynced(true);
      })
      .catch(() => {
        // 失败时假定 recording（浮窗只在录制中创建）
        setSynced(true);
      });
    return () => {
      cancelled = true;
    };
  }, []);

  // 监听 record://event 更新本地 state（pause/resume/stop）
  useEffect(() => {
    let unlisten: UnlistenFn | undefined;
    let cancelled = false;
    rawListen<HelperEventLite>("record://event", (e: Event<HelperEventLite>) => {
      const evt = e.payload.event;
      if (evt === "recording-paused") setCurrentState("paused");
      else if (evt === "recording-resumed") setCurrentState("recording");
      else if (evt === "recording-stopped") setCurrentState("idle");
    }).then((fn) => {
      if (cancelled) fn();
      else unlisten = fn;
    });
    return () => {
      cancelled = true;
      unlisten?.();
    };
  }, []);

  // recording 时累加 displayDuration；paused/idle 停
  useEffect(() => {
    if (!synced || currentState !== "recording") return;
    const timer = setInterval(() => {
      setDisplayDuration((d) => d + 1);
    }, 1000);
    return () => clearInterval(timer);
  }, [synced, currentState]);

  // 监听停止失败（hide 浮窗避免残留）
  useEffect(() => {
    let unlisten: UnlistenFn | undefined;
    let cancelled = false;
    rawListen("record://stop-failed", () => {
      getCurrentWindow().hide().catch(() => {});
    }).then((fn) => {
      if (cancelled) fn();
      else unlisten = fn;
    });
    return () => {
      cancelled = true;
      unlisten?.();
    };
  }, []);

  const isRecording = currentState === "recording";
  const isPaused = currentState === "paused";

  const onStop = async () => {
    await emit("record://stop-requested", { from: "control" });
  };

  const onTogglePause = () => {
    if (isRecording) session.pause();
    else if (isPaused) session.resume();
  };

  return (
    <div
      data-tauri-drag-region
      style={{
        display: "flex",
        alignItems: "center",
        gap: 6,
        padding: "6px 8px",
        width: "100vw",
        height: "100vh",
        boxSizing: "border-box",
        background: "rgba(15,15,17,0.92)",
        backdropFilter: "blur(16px)",
        WebkitBackdropFilter: "blur(16px)",
        borderRadius: 10,
        border: "1px solid rgba(255,255,255,0.08)",
        boxShadow: "0 8px 32px rgba(0,0,0,0.4)",
        color: "#fff",
        fontFamily: "-apple-system, BlinkMacSystemFont, sans-serif",
        userSelect: "none",
      }}
    >
      {/* 红点 + 时长 */}
      <div style={{ display: "flex", alignItems: "center", gap: 6, flex: 1, minWidth: 0 }}>
        <div
          style={{
            width: 7,
            height: 7,
            borderRadius: "50%",
            background: isPaused ? "rgba(255,255,255,0.4)" : "#ef4444",
            boxShadow: isPaused ? "none" : "0 0 6px #ef4444",
            animation: isRecording ? "pulse 1.5s ease-in-out infinite" : "none",
            flexShrink: 0,
          }}
        />
        <span
          style={{
            fontSize: 12,
            fontWeight: 600,
            fontVariantNumeric: "tabular-nums",
            fontFamily: "SF Mono, Menlo, monospace",
            letterSpacing: 0.3,
          }}
        >
          {formatDuration(displayDuration)}
        </span>
      </div>

      {/* 暂停/恢复按钮 */}
      <button
        onClick={onTogglePause}
        title={isRecording ? t("settings.recordings.pauseBtn") : t("settings.recordings.resumeBtn")}
        style={{
          width: 24,
          height: 24,
          display: "flex",
          alignItems: "center",
          justifyContent: "center",
          borderRadius: 5,
          border: "1px solid rgba(255,255,255,0.15)",
          background: "transparent",
          color: "rgba(255,255,255,0.8)",
          cursor: "pointer",
          padding: 0,
          flexShrink: 0,
        }}
      >
        {isRecording ? (
          // 暂停图标（两竖）——录制中显示「暂停」
          <svg width="8" height="10" viewBox="0 0 10 12" fill="currentColor">
            <rect x="0" y="0" width="3" height="12" rx="1" />
            <rect x="7" y="0" width="3" height="12" rx="1" />
          </svg>
        ) : (
          // 播放图标（三角）——暂停中显示「继续」
          <svg width="8" height="10" viewBox="0 0 10 12" fill="currentColor">
            <path d="M0 0 L10 6 L0 12 Z" />
          </svg>
        )}
      </button>

      {/* 停止按钮（红方块） */}
      <button
        onClick={onStop}
        title={t("tray.recordStop")}
        style={{
          width: 24,
          height: 24,
          display: "flex",
          alignItems: "center",
          justifyContent: "center",
          borderRadius: 5,
          border: "none",
          background: "#ef4444",
          color: "#fff",
          cursor: "pointer",
          padding: 0,
          flexShrink: 0,
        }}
      >
        <svg width="8" height="8" viewBox="0 0 10 10" fill="currentColor">
          <rect x="0" y="0" width="10" height="10" rx="1.5" />
        </svg>
      </button>

      {/* pulse 动画 keyframes */}
      <style>{`
        @keyframes pulse {
          0%, 100% { opacity: 1; transform: scale(1); }
          50% { opacity: 0.5; transform: scale(0.85); }
        }
      `}</style>
    </div>
  );
}
