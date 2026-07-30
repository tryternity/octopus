// 删除确认弹框——单条/批量删除录屏，含「同时删磁盘文件」checkbox。
// 从 RecordingPanel.tsx 拆出（2026-07-30）。

import { useState } from "react";
import { Trash2 } from "lucide-react";

interface DeleteConfirmDialogProps {
  /** 删除目标描述（单条用文件名，批量用「N 个录屏」）。 */
  targetLabel: string;
  /** 删除数量（影响文案：单数/复数）。 */
  count: number;
  onCancel: () => void;
  onConfirm: (permanent: boolean) => void;
  t: (key: string, params?: Record<string, string | number>) => string;
}

export default function DeleteConfirmDialog({
  targetLabel,
  count,
  onCancel,
  onConfirm,
  t,
}: DeleteConfirmDialogProps) {
  // 默认不勾——删除是常见操作，误删磁盘文件不可逆；用户显式勾选才删文件。
  const [deleteFile, setDeleteFile] = useState(false);
  const isBatch = count > 1;

  return (
    <div
      className="fixed inset-0 z-50 flex items-center justify-center bg-black/40"
      onClick={onCancel}
    >
      <div
        className="relative w-full max-w-sm mx-4 rounded-lg border border-border bg-surface shadow-xl"
        onClick={(e) => e.stopPropagation()}
      >
        {/* 标题区：destructive 色调 + Trash2 图标 */}
        <div className="flex items-center gap-2 px-3 py-2.5 border-b border-border/60">
          <div className="flex items-center gap-1.5 text-destructive">
            <Trash2 className="w-3.5 h-3.5" />
            <span className="text-xs font-medium">
              {t("settings.recordings.delete")}
            </span>
          </div>
        </div>

        {/* 内容区：目标 + checkbox */}
        <div className="px-3 py-3 space-y-2.5">
          <p className="text-[11px] text-foreground/80 break-words">
            {isBatch
              ? t("settings.recordings.confirmDeleteN", { n: count })
              : t("settings.recordings.deleteConfirm")}
            {!isBatch && (
              <span className="block mt-0.5 font-mono-vault text-[10px] text-muted-foreground break-all">
                {targetLabel}
              </span>
            )}
          </p>
          <label className="flex items-start gap-2 cursor-pointer group">
            <input
              type="checkbox"
              className="mt-0.5 w-3.5 h-3.5 accent-destructive flex-shrink-0"
              checked={deleteFile}
              onChange={(e) => setDeleteFile(e.target.checked)}
            />
            <span className="text-[11px] leading-relaxed text-foreground/80 group-hover:text-foreground transition-colors">
              {t("settings.recordings.deleteAlsoFile")}
            </span>
          </label>
        </div>

        {/* 底部按钮：取消 + 删除（destructive） */}
        <div className="flex items-center justify-end gap-1.5 px-3 py-2 border-t border-border/60 bg-muted/40">
          <button
            onClick={onCancel}
            className="px-2.5 py-1 rounded text-[10px] font-medium text-muted-foreground hover:text-foreground hover:bg-accent transition-colors"
          >
            {t("settings.recordings.subtitlePolishCancel")}
          </button>
          <button
            onClick={() => onConfirm(deleteFile)}
            className="px-2.5 py-1 rounded text-[10px] font-medium bg-destructive text-white hover:bg-destructive/90 transition-colors"
          >
            {t("settings.recordings.delete")}
          </button>
        </div>
      </div>
    </div>
  );
}
