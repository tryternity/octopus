/**
 * RecordControl —— display/window 录制时的桌面控制 pill。
 *
 * 屏幕右下角 fixed 位置（位置由后端 record_control_window.rs 算好）。
 * 显示：红点 + 时长 + 暂停/恢复按钮 + 停止按钮。
 *
 * 与 RecordAnnotation 互斥（Area 录制用 RecordAnnotation，display/window 用本浮窗）。
 *
 * 停止路径：emit `record://stop-requested`（与 RecordAnnotation 同），main.rs 监听后
 * 调 stop_and_store → close_control_window + emit record://stopped。
 * 暂停/恢复：直接 invoke `record_pause` / `record_resume`（via useRecordSession hook）。
 *
 * 监听 `record://stop-failed`：停止失败时也 hide 浮窗（避免残留），用户从 tray 重试。
 */
import { useEffect } from "react";
import { emit } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { useRecordSession } from "@/hooks/useRecordSession";
import { useT } from "@/lib/i18n";

function formatDuration(ms: number): string {
  const totalSec = Math.max(0, Math.floor(ms / 1000));
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
  const { state, duration } = session;
  const isRecording = state === "recording";
  const isPaused = state === "paused";

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
    // 不立即 hide——main.rs handler 成功后会 close_control_window（destroy）。
    // 失败时由 record://stop-failed listener 处理。
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
        gap: 10,
        padding: "8px 12px",
        width: "100vw",
        height: "100vh",
        boxSizing: "border-box",
        background: "rgba(15,15,17,0.92)",
        backdropFilter: "blur(16px)",
        WebkitBackdropFilter: "blur(16px)",
        borderRadius: 12,
        border: "1px solid rgba(255,255,255,0.08)",
        boxShadow: "0 8px 32px rgba(0,0,0,0.4)",
        color: "#fff",
        fontFamily: "-apple-system, BlinkMacSystemFont, sans-serif",
        userSelect: "none",
      }}
    >
      {/* 红点 + 时长 */}
      <div style={{ display: "flex", alignItems: "center", gap: 8, flex: 1 }}>
        <div
          style={{
            width: 8,
            height: 8,
            borderRadius: "50%",
            background: isPaused ? "rgba(255,255,255,0.4)" : "#ef4444",
            boxShadow: isPaused ? "none" : "0 0 6px #ef4444",
            animation: isRecording ? "pulse 1.5s ease-in-out infinite" : "none",
            flexShrink: 0,
          }}
        />
        <span
          style={{
            fontSize: 13,
            fontWeight: 600,
            fontVariantNumeric: "tabular-nums",
            fontFamily: "SF Mono, Menlo, monospace",
            letterSpacing: 0.3,
          }}
        >
          {formatDuration(duration * 1000)}
        </span>
        {isPaused && (
          <span style={{ fontSize: 11, color: "rgba(255,255,255,0.5)" }}>
            {t("settings.recordings.paused")}
          </span>
        )}
      </div>

      {/* 暂停/恢复按钮 */}
      <button
        onClick={onTogglePause}
        title={isRecording ? t("settings.recordings.pauseBtn") : t("settings.recordings.resumeBtn")}
        style={{
          width: 28,
          height: 28,
          display: "flex",
          alignItems: "center",
          justifyContent: "center",
          borderRadius: 6,
          border: "1px solid rgba(255,255,255,0.15)",
          background: "transparent",
          color: "rgba(255,255,255,0.8)",
          cursor: "pointer",
          padding: 0,
          flexShrink: 0,
        }}
      >
        {isRecording ? (
          // 暂停图标（两竖）
          <svg width="10" height="12" viewBox="0 0 10 12" fill="currentColor">
            <rect x="0" y="0" width="3" height="12" rx="1" />
            <rect x="7" y="0" width="3" height="12" rx="1" />
          </svg>
        ) : (
          // 播放图标（三角）
          <svg width="10" height="12" viewBox="0 0 10 12" fill="currentColor">
            <path d="M0 0 L10 6 L0 12 Z" />
          </svg>
        )}
      </button>

      {/* 停止按钮（红方块） */}
      <button
        onClick={onStop}
        title={t("tray.recordStop")}
        style={{
          width: 28,
          height: 28,
          display: "flex",
          alignItems: "center",
          justifyContent: "center",
          borderRadius: 6,
          border: "none",
          background: "#ef4444",
          color: "#fff",
          cursor: "pointer",
          padding: 0,
          flexShrink: 0,
        }}
      >
        <svg width="10" height="10" viewBox="0 0 10 10" fill="currentColor">
          <rect x="0" y="0" width="10" height="10" rx="1.5" />
        </svg>
      </button>

      {/* pulse 动画 keyframes（注入到 document，仅一次） */}
      <style>{`
        @keyframes pulse {
          0%, 100% { opacity: 1; transform: scale(1); }
          50% { opacity: 0.5; transform: scale(0.85); }
        }
      `}</style>
    </div>
  );
}
