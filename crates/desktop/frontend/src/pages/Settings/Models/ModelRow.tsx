import { useState, useRef } from "react";
import { cn, fmtBytes } from "@/lib/utils";
import { CheckCircle2, Circle, Download, FileDown, Pencil, RefreshCw, Trash2 } from "lucide-react";
import { confirm } from "@tauri-apps/plugin-dialog";
import { useT } from "@/lib/i18n";
import { Button } from "@/components/ui/button";
import DownloadPopover from "@/components/DownloadPopover";

export interface ModelRowData {
  name: string;
  provider: string;
  category: string;
  description: string;
  isReady: boolean;
  isCurrent: boolean;
  /** 模型来源: 0=builtin 1=local 2=cloud（!== 2 即本地/builtin） */
  sourceType: number;
  repo: string;
  /** 云端模型 id（用于编辑/删除） */
  cloudId?: number;
}

interface DownloadProgress {
  repo: string;
  downloaded: number;
  total: number;
}

export function ModelRow({
  model,
  progress,
  busy,
  popoverOpen,
  onPopoverOpenChange,
  onActivate,
  onDownload,
  onVerify,
  onDelete,
  onEdit,
}: {
  model: ModelRowData;
  progress?: DownloadProgress | null;
  busy: boolean;
  popoverOpen?: boolean;
  onPopoverOpenChange?: (open: boolean) => void;
  onActivate: () => void;
  onDownload: () => void;
  onVerify: () => void;
  onDelete: () => void;
  onEdit?: () => void;
}) {
  const t = useT();
  const pct = progress && progress.total > 0 ? (progress.downloaded / progress.total) * 100 : 0;
  const showDownload = model.sourceType !== 2 && !model.isReady;
  // 内部 fallback 状态（无外部控制时用，如云端行不需互斥）
  const [internalOpen, setInternalOpen] = useState(false);
  const showPopover = onPopoverOpenChange ? (popoverOpen ?? false) : internalOpen;
  const setShowPopover = (open: boolean) => {
    if (onPopoverOpenChange) onPopoverOpenChange(open);
    else setInternalOpen(open);
  };
  const filesBtnRef = useRef<HTMLButtonElement>(null);

  return (
    <div
      className={cn(
        "group flex items-start justify-between py-2 px-3 rounded-md gap-3 transition-colors",
        "border-l-2",
        model.isCurrent
          ? "border-l-success bg-success/10"
          : model.isReady
            ? "border-l-voice/50"
            : "border-l-border/40",
      )}
    >
      {/* 左：状态 + 名称 + 描述 */}
      <div className="flex flex-col gap-0.5 flex-1 min-w-0">
        <div className="flex items-center gap-1.5">
          {model.isCurrent ? (
            <CheckCircle2 className="w-3 h-3 text-success shrink-0" />
          ) : model.isReady ? (
            <CheckCircle2 className="w-3 h-3 text-voice/70 shrink-0" />
          ) : (
            <Circle className="w-3 h-3 text-muted-foreground/30 shrink-0" />
          )}
          <span className="text-xs font-medium">{model.name}</span>
          <span className="text-[9px] text-muted-foreground/50 px-1 py-px rounded bg-muted">[{model.provider === "local" ? t("settings.models.local") : model.provider}]</span>
        </div>
        {model.description && (
          <span className="text-[11px] text-muted-foreground/60">{model.description}</span>
        )}
        {progress && (
          <div className="mt-1">
            <div className="h-1 bg-muted rounded-full overflow-hidden">
              <div className="h-full bg-voice transition-all" style={{ width: `${pct}%` }} />
            </div>
            <span className="text-[10px] text-muted-foreground/50">{fmtBytes(progress.downloaded)} / {fmtBytes(progress.total)}</span>
          </div>
        )}
      </div>

      {/* 右：操作按钮 */}
      <div className="flex items-center gap-1 flex-shrink-0">
        {showDownload ? (
          <Button
            variant="primary"
            size="sm"
            disabled={busy}
            onClick={onDownload}
          >
            <Download />
            {t("settings.models.download")}
          </Button>
        ) : (
          <>
            {/* 激活 / 已激活——配色对齐提示配方：已激活=绿色 success，未激活=黄/橙 warning-soft */}
            {model.isCurrent ? (
              <Button variant="success" size="sm" disabled className="cursor-default">
                {t("settings.models.activated")}
              </Button>
            ) : (
              <Button variant="warning-soft" size="sm" onClick={onActivate}>
                {t("settings.models.activate")}
              </Button>
            )}

            {/* 校验（本地已就绪） */}
            {model.sourceType !== 2 && model.isReady && (
              <Button
                variant="ghost"
                size="icon-sm"
                disabled={busy}
                onClick={onVerify}
              >
                <RefreshCw />
              </Button>
            )}

            {/* 删除（local 已就绪 或 云端模型；builtin 始终灰掉占位——文件损坏用校验+重新下载） */}
            {(model.sourceType !== 2 && model.isReady || model.sourceType === 2) && (
              <Button
                variant="destructive-ghost"
                size="icon-sm"
                disabled={busy || model.sourceType === 0}
                onClick={async () => {
                  if (model.sourceType === 0) return; // builtin 不可删
                  const ok = await confirm(t("settings.models.confirmDelete"), { title: "删除模型", kind: "warning" });
                  if (ok) onDelete();
                }}
              >
                <Trash2 />
              </Button>
            )}

            {/* 编辑（云端模型） */}
            {model.sourceType === 2 && onEdit && (
              <Button variant="ghost" size="icon-sm" onClick={onEdit}>
                <Pencil />
              </Button>
            )}
          </>
        )}

        {/* 文件列表浮层（本地+builtin，无论就绪/下载中/未下载）—— hover/click 展示文件级进度 */}
        {model.sourceType !== 2 && (
          <div className="relative">
            <Button
              ref={filesBtnRef}
              variant="ghost"
              size="icon-sm"
              onClick={() => setShowPopover(!showPopover)}
              onMouseEnter={() => setShowPopover(true)}
            >
              <FileDown />
            </Button>
            {showPopover && (
              <DownloadPopover
                repo={model.repo}
                modelName={model.name}
                triggerRef={filesBtnRef}
                onClose={() => setShowPopover(false)}
              />
            )}
          </div>
        )}
      </div>
    </div>
  );
}

export function CurrentBanner({ label }: { label: string }) {
  const t = useT();
  return (
    <div className="flex items-center gap-1.5 px-3 py-1.5 rounded-md border-l-2 border-success bg-success/10 text-[11px] mb-1">
      <CheckCircle2 className="w-3 h-3 text-success shrink-0" />
      <span className="text-muted-foreground">{t("settings.models.current")}</span>
      <span className="font-medium text-foreground">{label}</span>
    </div>
  );
}
