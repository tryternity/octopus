// 字幕 LLM 润色对话框——选择是否润色 + 用哪个 LLM。
// 从 RecordingPanel.tsx 拆出（2026-07-30）。

import { Sparkles, X } from "lucide-react";
import { cn } from "@/lib/utils";
import type { RecordingMeta, LlmOption, PolishOption } from "./types";
import { formatDuration } from "./format";

interface SubtitlePolishDialogProps {
  /** 弹框关联的 recording（找不到时退化为只显示标题文案）。 */
  rec?: RecordingMeta;
  llmOptions: LlmOption[];
  polishEnabled: boolean;
  llmKey: string;
  onPolishEnabledChange: (next: boolean) => void;
  onLlmKeyChange: (key: string) => void;
  onCancel: () => void;
  /** 确认：返回 polish 参数（null=不润色，{llmKey}=润色），由父 invoke generate_subtitle。 */
  onConfirm: (polish: PolishOption | null) => void;
  t: (key: string, params?: Record<string, string | number>) => string;
}

export default function SubtitlePolishDialog({
  rec,
  llmOptions,
  polishEnabled,
  llmKey,
  onPolishEnabledChange,
  onLlmKeyChange,
  onCancel,
  onConfirm,
  t,
}: SubtitlePolishDialogProps) {
  const title = rec
    ? rec.title || rec.filePath.split("/").pop() || `#${rec.id}`
    : t("settings.recordings.subtitlePolishDialogTitle");
  const durationLabel = rec && rec.durationMs > 0 ? formatDuration(rec.durationMs) : null;

  return (
    <div
      className="fixed inset-0 z-50 flex items-center justify-center bg-black/40"
      onClick={onCancel}
    >
      <div
        className="w-[340px] max-w-[90vw] bg-surface border border-border rounded-lg shadow-lg overflow-hidden border-l-2 border-l-primary"
        onClick={(e) => e.stopPropagation()}
      >
        {/* 标题行：Sparkles + 文案 + 关闭 */}
        <div className="flex items-center gap-1.5 px-3 py-2 border-b border-border/60">
          <Sparkles className="w-3.5 h-3.5 text-warning flex-shrink-0" />
          <span className="text-xs font-semibold text-foreground flex-1 min-w-0 truncate">
            {t("settings.recordings.subtitlePolishDialogTitle")}
          </span>
          <button
            onClick={onCancel}
            className="p-0.5 rounded text-muted-foreground hover:text-foreground hover:bg-accent transition-colors flex-shrink-0"
            title={t("settings.recordings.subtitlePolishCancel")}
          >
            <X className="w-3.5 h-3.5" />
          </button>
        </div>

        {/* 录屏标题 + meta（让用户确认是对哪个录屏操作） */}
        <div className="px-3 py-2 border-b border-border/60">
          <p className="text-xs text-foreground truncate" title={title}>
            {title}
          </p>
          <div className="flex items-center gap-1.5 mt-0.5 text-[10px] text-muted-foreground">
            {durationLabel && (
              <span className="tabular-nums px-1 rounded bg-muted">{durationLabel}</span>
            )}
            {rec && rec.width > 0 && rec.height > 0 && (
              <span className="tabular-nums">
                {rec.width}×{rec.height}
              </span>
            )}
            <span
              className={cn(
                "px-1.5 py-0.5 rounded font-medium",
                rec?.hasMicrophone
                  ? "bg-voice/10 text-voice"
                  : "text-muted-foreground/60",
              )}
            >
              {rec?.sourceType || ""}
            </span>
          </div>
        </div>

        {/* 润色选项：checkbox + 下拉（垂直堆叠） */}
        <div className="px-3 py-2.5 space-y-2">
          <label className="flex items-start gap-2 cursor-pointer">
            <input
              type="checkbox"
              className="w-3.5 h-3.5 mt-0.5 accent-primary flex-shrink-0"
              checked={polishEnabled}
              onChange={(e) => onPolishEnabledChange(e.target.checked)}
            />
            <div className="flex flex-col min-w-0">
              <span className="text-xs text-foreground">
                {t("settings.recordings.subtitlePolishCheckbox")}
              </span>
              <span className="text-[10px] text-muted-foreground leading-relaxed mt-0.5">
                {t("settings.recordings.subtitlePolishHint")}
              </span>
            </div>
          </label>

          {/* LLM 下拉：checkbox off 时灰禁 + 折叠（避免占空间）。
              pl-[22px] 对齐 checkbox 宽度（w-3.5=14px + gap-2=8px），视觉缩进与 checkbox 文字齐。 */}
          {polishEnabled && (
            <div className="flex items-center gap-1.5 pl-[22px]">
              <span className="text-[10px] text-muted-foreground whitespace-nowrap flex-shrink-0">
                {t("settings.recordings.subtitlePolishLlm")}
              </span>
              <select
                value={llmKey}
                onChange={(e) => onLlmKeyChange(e.target.value)}
                disabled={!polishEnabled}
                className="flex-1 min-w-0 bg-background border border-border rounded text-[10px] px-1.5 py-1 text-foreground outline-none focus:border-primary truncate"
              >
                {llmOptions.length === 0 ? (
                  <option value="">
                    {t("settings.recordings.subtitlePolishNoLlm")}
                  </option>
                ) : (
                  llmOptions.map((opt) => (
                    <option key={opt.key} value={opt.key}>
                      {opt.label}
                    </option>
                  ))
                )}
              </select>
            </div>
          )}
        </div>

        {/* 底部按钮：取消（ghost）+ 确认（primary）。动作名随润色开关变。 */}
        <div className="flex items-center justify-end gap-1.5 px-3 py-2 border-t border-border/60 bg-muted/40">
          <button
            onClick={onCancel}
            className="px-2.5 py-1 rounded text-[10px] font-medium text-muted-foreground hover:text-foreground hover:bg-accent transition-colors"
          >
            {t("settings.recordings.subtitlePolishCancel")}
          </button>
          <button
            onClick={() =>
              onConfirm(polishEnabled ? { llmKey: llmKey || null } : null)
            }
            className="flex items-center gap-1 px-2.5 py-1 rounded text-[10px] font-medium bg-primary text-primary-foreground hover:bg-primary/90 transition-colors"
          >
            {polishEnabled && <Sparkles className="w-3 h-3" />}
            {polishEnabled
              ? t("settings.recordings.subtitlePolishConfirm")
              : t("settings.recordings.subtitlePolishConfirmPlain")}
          </button>
        </div>
      </div>
    </div>
  );
}
