// 录屏格式化工具 + polish_outcome 降级提示。
// 从 RecordingPanel.tsx 拆出（2026-07-30）。

import type { ToastVariant } from "@/lib/useToast";

/** 格式化时长 ms → "MM:SS"（<1h）或 "H:MM:SS"（≥1h）。 */
export function formatDuration(ms: number): string {
  const totalSec = Math.max(0, Math.floor(ms / 1000));
  const h = Math.floor(totalSec / 3600);
  const m = Math.floor((totalSec % 3600) / 60);
  const s = totalSec % 60;
  const pad = (n: number) => n.toString().padStart(2, "0");
  if (h > 0) return `${h}:${pad(m)}:${pad(s)}`;
  return `${pad(m)}:${pad(s)}`;
}

/** 把 fileSize bytes 格式化为 KB/MB/GB。 */
export function formatSize(bytes: number): string {
  if (bytes < 1024) return `${bytes}B`;
  const units = ["KB", "MB", "GB"];
  let v = bytes / 1024;
  let i = 0;
  while (v >= 1024 && i < units.length - 1) {
    v /= 1024;
    i++;
  }
  return `${v.toFixed(v >= 10 ? 0 : 1)}${units[i]}`;
}

/** 把 ISO8601 createdAt 格式化为本地短日期（YYYY-MM-DD HH:MM）。 */
export function formatCreatedAt(iso: string): string {
  if (!iso) return "";
  const d = new Date(iso);
  if (Number.isNaN(d.getTime())) return iso;
  const pad = (n: number) => n.toString().padStart(2, "0");
  return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())} ${pad(
    d.getHours(),
  )}:${pad(d.getMinutes())}`;
}

/**
 * 把 cue 的 ms 时间戳格式化为紧凑时间码：
 * <1h → "MM:SS"，≥1h → "H:MM:SS"。字幕面板专用。
 */
export function formatMs(ms: number): string {
  const totalSec = Math.max(0, Math.floor(ms / 1000));
  const h = Math.floor(totalSec / 3600);
  const m = Math.floor((totalSec % 3600) / 60);
  const s = totalSec % 60;
  const pad = (n: number) => n.toString().padStart(2, "0");
  return h > 0 ? `${h}:${pad(m)}:${pad(s)}` : `${pad(m)}:${pad(s)}`;
}

/**
 * polish_outcome 降级提示（Phase 4，Task 4.2）。
 * 提示只在润色「降级」时出现——让用户知道字幕可能质量打折，但流程仍完成了。
 */
export function showPolishOutcomeToast(
  outcome: string | undefined,
  showToast: (msg: string, variant?: ToastVariant) => void,
  t: (key: string, params?: Record<string, string | number>) => string,
) {
  if (!outcome || outcome === "polished") return;
  if (outcome === "fallbackRatio") {
    showToast(t("settings.recordings.subtitlePolishOutcomeFallbackRatio"), "warning");
  } else if (outcome === "noLlmConfig") {
    showToast(t("settings.recordings.subtitlePolishOutcomeNoLlmConfig"), "error");
  } else if (outcome.startsWith("failed:")) {
    const msg = outcome.slice("failed:".length);
    showToast(
      t("settings.recordings.subtitlePolishOutcomeFailed", { msg }),
      "error",
    );
  }
}
