import { cn } from "@/lib/utils";
import { CheckCircle2, Circle, Download, Pencil, RefreshCw, Trash2 } from "lucide-react";
import { confirm } from "@tauri-apps/plugin-dialog";
import { useT } from "@/lib/i18n";
import { Button } from "@/components/ui/button";

export interface ModelRowData {
  name: string;
  provider: string;
  category: string;
  description: string;
  is_ready: boolean;
  is_current: boolean;
  is_local: boolean;
  repo: string;
  /** 云端模型 id（用于编辑/删除） */
  cloudId?: number;
}

interface DownloadProgress {
  repo: string;
  downloaded: number;
  total: number;
}

function fmtBytes(n: number): string {
  if (n < 1024) return n + " B";
  if (n < 1048576) return (n / 1024).toFixed(1) + " KB";
  if (n < 1073741824) return (n / 1048576).toFixed(1) + " MB";
  return (n / 1073741824).toFixed(2) + " GB";
}

export function ModelRow({
  model,
  progress,
  busy,
  onActivate,
  onDownload,
  onVerify,
  onDelete,
  onEdit,
}: {
  model: ModelRowData;
  progress?: DownloadProgress | null;
  busy: boolean;
  onActivate: () => void;
  onDownload: () => void;
  onVerify: () => void;
  onDelete: () => void;
  onEdit?: () => void;
}) {
  const t = useT();
  const pct = progress && progress.total > 0 ? (progress.downloaded / progress.total) * 100 : 0;
  const showDownload = model.is_local && !model.is_ready;

  return (
    <div
      className={cn(
        "group flex items-start justify-between py-2 px-3 rounded-md gap-3 transition-colors",
        "border-l-2",
        model.is_current
          ? "border-l-success bg-success/10"
          : model.is_ready
            ? "border-l-voice/50"
            : "border-l-border/40",
      )}
    >
      {/* 左：状态 + 名称 + 描述 */}
      <div className="flex flex-col gap-0.5 flex-1 min-w-0">
        <div className="flex items-center gap-1.5">
          {model.is_current ? (
            <CheckCircle2 className="w-3 h-3 text-success shrink-0" />
          ) : model.is_ready ? (
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
            {/* 激活 / 已激活 */}
            {model.is_current ? (
              <Button variant="ghost" size="sm" disabled className="cursor-default">
                {t("settings.models.activated")}
              </Button>
            ) : (
              <Button variant="success" size="sm" onClick={onActivate}>
                {t("settings.models.activate")}
              </Button>
            )}

            {/* 校验（本地已就绪） */}
            {model.is_local && model.is_ready && (
              <Button
                variant="ghost"
                size="icon-sm"
                disabled={busy}
                onClick={onVerify}
              >
                <RefreshCw />
              </Button>
            )}

            {/* 删除（本地已就绪 或 云端模型） */}
            {(model.is_local && model.is_ready || !model.is_local) && (
              <Button
                variant="destructive-ghost"
                size="icon-sm"
                disabled={busy}
                onClick={async () => {
                  const ok = await confirm(t("settings.models.confirmDelete"), { title: "删除模型", kind: "warning" });
                  if (ok) onDelete();
                }}
              >
                <Trash2 />
              </Button>
            )}

            {/* 编辑（云端模型） */}
            {!model.is_local && onEdit && (
              <Button variant="ghost" size="icon-sm" onClick={onEdit}>
                <Pencil />
              </Button>
            )}
          </>
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
