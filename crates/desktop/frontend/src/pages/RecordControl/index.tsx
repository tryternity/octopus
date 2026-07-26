/**
 * RecordControl —— display/window 录制时的桌面控制 pill。
 *
 * 屏幕右下角 fixed 位置（位置由后端 record_control_window.rs 算好）。
 * 紧凑布局：红点 + 时长 + 暂停/恢复按钮 + 停止按钮。
 *
 * **duration 初始化**：浮窗创建晚于 recording-started 事件，useRecordSession 的
 * 监听器收不到该事件 → duration 永远 0。mount 时主动 invoke get_record_status
 * 拿当前 state + elapsed_secs，如果在录制中就设 duration 初值 + 启动 setInterval。
 *
 * 停止路径：emit `record://stop-requested`（与 RecordAnnotation 同），main.rs 监听后
 * 调 stop_and_store → close_control_window + emit record://stopped。
 * 暂停/恢复：直接 invoke `record_pause` / `record_resume`（via useRecordSession hook）。
 */
import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { emit } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { useRecordSession } from "@/hooks/useRecordSession";
import { useT } from "@/lib/i18n";

function formatDuration(secs: number): string {
  const totalSec = Math.max(0, Math.floor(secs));
  const h = Math.floor(totalSec / 3600);
  const m = Math.floor((totalSec % 3600) / 60);
  const s = totalSec % 60;
  const pad = (n: number) => n.toString().padStart(2, "0");
  if (h > 0) return `${h}:${pad(m)}:${pad(s)}`;
  return `${pad(m)}:${pad(s)}`;
}

export default function RecordControl() {
  const t = useT();
  const session = useRecordSession();
  const { state } = session;
  const isRecording = state === "recording";
  const isPaused = state === "paused";

  // mount 时主动查当前录制状态——浮窗创建晚于 recording-started 事件，
  // useRecordSession 的监听收不到，duration 会一直 0。这里 invoke 拿真实状态 + elapsed。
  const [synced, setSynced] = useState(false);
  useEffect(() => {
    let cancelled = false;
    invoke<{ state: string; elapsed_secs: number }>("get_record_status")
      .then((status) => {
        if (cancelled) return;
        // 如果正在录制（state=recording），直接用后端的 elapsed 作为初始 duration。
        // useRecordSession 内部的 setInterval 还没启动（recording-started 事件已过），
        // 这里手动启一个补上——但 setInterval 在 hook 内部，外部无法直接触发。
        // 折中：直接展示后端 elapsed + 本地 setInterval 继续累加。
        // 但 hook 的 duration 是内部 state，外部无法设——所以本组件维护自己的 displayDuration。
        if (status.state === "recording" || status.state === "paused") {
          setDisplayDuration(status.elapsed_secs);
        }
        setSynced(true);
      })
      .catch(() => setSynced(true));
    return () => {
      cancelled = true;
    };
  }, []);

  // 自己维护 displayDuration（hook 的 duration 收不到 recording-started 不会启动）
  const [displayDuration, setDisplayDuration] = useState(0);
  useEffect(() => {
    if (!synced) return;
    // recording 时启动本地计时器；paused/idle 时停
    if (state === "recording") {
      const timer = setInterval(() => {
        setDisplayDuration((d) => d + 1);
      }, 1000);
      return () => clearInterval(timer);
    }
  }, [synced, state]);

  // 监听停止失败（停止失败时也 hide 浮窗，避免残留）
  useEffect(() => {
    let unlisten: (() => void) | undefined;
    let cancelled = false;
    import("@tauri-apps/api/event").then(({ listen }) => {
      if (cancelled) return;
      listen("record://stop-failed", () => {
        getCurrentWindow().hide().catch(() => {});
      }).then((fn) => {
        if (cancelled) fn();
        else unlisten = fn;
      });
    });
    return () => {
      cancelled = true;
      unlisten?.();
    };
  }, []);

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
          <svg width="8" height="10" viewBox="0 0 10 12" fill="currentColor">
            <rect x="0" y="0" width="3" height="12" rx="1" />
            <rect x="7" y="0" width="3" height="12" rx="1" />
          </svg>
        ) : (
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
