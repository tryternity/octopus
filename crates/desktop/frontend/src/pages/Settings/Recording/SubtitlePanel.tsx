// 字幕预览浮层——cue 列表 + 单击复制 + 复制全部 + 在 Finder 显示。
// 从 RecordingPanel.tsx 拆出（2026-07-30）。

import { useState, useEffect, useRef } from "react";
import { Copy, CopyCheck, Download, Info, X } from "lucide-react";
import { cn } from "@/lib/utils";
import type { SubtitleResult, SubtitleCue } from "./types";
import { formatMs } from "./format";

interface SubtitlePanelProps {
  result: SubtitleResult;
  /** 行内错误文案（生成失败留存）。面板展开时也展示，方便用户看完整上下文。 */
  error?: string;
  onExport: () => void;
  onCopyCue: (cue: SubtitleCue) => void;
  onCopyAll: () => void;
  /** 关闭浮层（点遮罩/Esc/关闭按钮触发）。 */
  onClose: () => void;
  t: (key: string, params?: Record<string, string | number>) => string;
}

export default function SubtitlePanel({
  result,
  error,
  onExport,
  onCopyCue,
  onCopyAll,
  onClose,
  t,
}: SubtitlePanelProps) {
  // 最近一次成功复制的 cue 文本（用于行内 CopyCheck 反馈）。1.2s 后清。
  const [copiedText, setCopiedText] = useState<string | null>(null);
  const copiedTimer = useRef<ReturnType<typeof setTimeout> | null>(null);

  useEffect(() => {
    return () => {
      if (copiedTimer.current) clearTimeout(copiedTimer.current);
    };
  }, []);

  // Esc 关闭浮层。
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") onClose();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onClose]);

  const handleCopyCue = (cue: SubtitleCue) => {
    onCopyCue(cue);
    setCopiedText(cue.text);
    if (copiedTimer.current) clearTimeout(copiedTimer.current);
    copiedTimer.current = setTimeout(() => setCopiedText(null), 1200);
  };

  const isFallback = result.trackUsed !== "microphone";
  const cueCount = result.cues.length;

  const trackLabel =
    result.trackUsed === "microphone"
      ? t("settings.recordings.subtitleTrackMic")
      : result.trackUsed === "system"
        ? t("settings.recordings.subtitleTrackSystem")
        : result.trackUsed === "merged"
          ? t("settings.recordings.subtitleTrackMerged")
          : t("settings.recordings.subtitleTrackUnknown");

  return (
    <div
      className="fixed inset-0 z-50 flex items-center justify-center bg-black/40"
      onClick={onClose}
    >
      <div
        className="relative w-full max-w-2xl max-h-[80vh] mx-4 rounded-lg border border-border bg-surface shadow-xl flex flex-col"
        onClick={(e) => e.stopPropagation()}
      >
        {/* 标题栏：cue 计数 · 模型 · track 来源 · 关闭按钮 */}
        <div className="flex items-center gap-1.5 px-4 py-2.5 border-b border-border/60 text-[10px] text-muted-foreground flex-wrap">
          <span className="tabular-nums">
            {t("settings.recordings.subtitleCount", { count: cueCount })}
          </span>
          <span className="text-muted-foreground/40">·</span>
          <span className="font-mono-vault text-muted-foreground/80">{result.model}</span>
          <span className="text-muted-foreground/40">·</span>
          <span
            className={cn(
              "px-1.5 py-0.5 rounded font-medium",
              result.trackUsed === "microphone"
                ? "bg-success/10 text-success"
                : "bg-warning/10 text-warning",
            )}
          >
            {trackLabel}
          </span>
          <button
            onClick={onClose}
            className="ml-auto p-1 rounded hover:bg-accent text-muted-foreground hover:text-foreground transition-colors"
            title={t("settings.recordings.subtitleCollapse")}
          >
            <X className="w-3.5 h-3.5" />
          </button>
        </div>

        {/* 提示区（fallback / error） */}
        {(isFallback || (error && !isFallback)) && (
          <div className="px-4 pt-2">
            {isFallback && (
              <div className="flex items-start gap-1.5 mb-1 px-2 py-1.5 rounded border-l-2 border-warning bg-warning/10 text-[10px] leading-relaxed text-foreground/80">
                <Info className="w-3 h-3 mt-px flex-shrink-0 text-warning" />
                <span>{t("settings.recordings.subtitleFallbackSystem")}</span>
              </div>
            )}
            {error && !isFallback && (
              <div className="flex items-start gap-1.5 mb-1 px-2 py-1.5 rounded border-l-2 border-destructive bg-destructive/10 text-[10px] leading-relaxed text-destructive">
                <Info className="w-3 h-3 mt-px flex-shrink-0" />
                <span className="break-all">{error}</span>
              </div>
            )}
          </div>
        )}

        {/* cue 列表（flex-1 占满浮层中间，可滚，单击复制） */}
        <div className="flex-1 overflow-y-auto thin-scrollbar px-4 py-2">
          {cueCount === 0 ? (
            <div className="py-6 text-center text-[10px] text-muted-foreground">
              {t("settings.recordings.subtitleEmpty")}
            </div>
          ) : (
            <div className="space-y-px">
              {result.cues.map((cue, i) => {
                const isCopied = copiedText === cue.text;
                return (
                  <button
                    key={i}
                    onClick={(e) => {
                      e.stopPropagation();
                      handleCopyCue(cue);
                    }}
                    className={cn(
                      "group/cue w-full flex items-start gap-2 px-1.5 py-1 rounded text-left transition-colors",
                      "hover:bg-accent",
                    )}
                    title={t("settings.recordings.subtitleCopyCueHint")}
                  >
                    <span className="flex-shrink-0 font-mono-vault text-[10px] tabular-nums text-muted-foreground/80 pt-px">
                      <span>{formatMs(cue.startMs)}</span>
                      <span className="mx-0.5 text-muted-foreground/40">→</span>
                      <span>{formatMs(cue.endMs)}</span>
                    </span>
                    <span className="flex-1 min-w-0 text-xs leading-relaxed text-foreground/90 break-words">
                      {cue.text}
                    </span>
                    <span className="flex-shrink-0 pt-px">
                      {isCopied ? (
                        <CopyCheck className="w-3 h-3 text-success" />
                      ) : (
                        <Copy className="w-3 h-3 text-muted-foreground/40 opacity-0 group-hover/cue:opacity-100 transition-opacity" />
                      )}
                    </span>
                  </button>
                );
              })}
            </div>
          )}
        </div>

        {/* 底部操作条：复制全部 + 在 Finder 显示 */}
        {cueCount > 0 && (
          <div className="flex items-center gap-1 px-4 py-2 border-t border-border/40">
            <button
              onClick={(e) => {
                e.stopPropagation();
                onCopyAll();
              }}
              className="flex items-center gap-1 px-2 py-1 rounded text-[10px] font-medium text-muted-foreground hover:text-foreground hover:bg-accent transition-colors"
            >
              <Copy className="w-3 h-3" />
              {t("settings.recordings.subtitleCopyAll")}
            </button>
            <button
              onClick={(e) => {
                e.stopPropagation();
                onExport();
              }}
              className="flex items-center gap-1 px-2 py-1 rounded text-[10px] font-medium text-muted-foreground hover:text-foreground hover:bg-accent transition-colors"
            >
              <Download className="w-3 h-3" />
              {t("settings.recordings.subtitleExport")}
            </button>
          </div>
        )}
      </div>
    </div>
  );
}
