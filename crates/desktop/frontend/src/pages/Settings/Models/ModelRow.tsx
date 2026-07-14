import { cn } from "@/lib/utils";
import { CheckCircle2, Circle, Download, RefreshCw, Trash2 } from "lucide-react";
import { useT } from "@/lib/i18n";

export interface ModelRowData {
  /** model_name */
  name: string;
  /** provider: local / aliyun / deepseek / bigmodel 等 */
  provider: string;
  /** 引擎族 / 类别 */
  category: string;
  /** 描述 */
  description: string;
  /** 本地模型是否已下载（is_enabled） */
  is_ready: boolean;
  /** 是否为当前激活模型 */
  is_current: boolean;
  /** 是否为本地模型（可下载/删除） */
  is_local: boolean;
  /** 下载路径标识（source），用于 download/delete */
  repo: string;
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

const btnBase = "text-[11px] rounded transition-colors flex items-center gap-1";

export function ModelRow({
  model,
  progress,
  busy,
  onActivate,
  onDownload,
  onVerify,
  onDelete,
}: {
  model: ModelRowData;
  progress?: DownloadProgress | null;
  busy: boolean;
  onActivate: () => void;
  onDownload: () => void;
  onVerify: () => void;
  onDelete: () => void;
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
          ? "border-l-emerald-500 bg-emerald-50/40"
          : model.is_ready
            ? "border-l-rose-300"
            : "border-l-border/40",
      )}
    >
      {/* 左：状态 + 名称 + 描述 */}
      <div className="flex flex-col gap-0.5 flex-1 min-w-0">
        <div className="flex items-center gap-1.5">
          {model.is_current ? (
            <CheckCircle2 className="w-3 h-3 text-emerald-500 shrink-0" />
          ) : model.is_ready ? (
            <CheckCircle2 className="w-3 h-3 text-rose-400 shrink-0" />
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
          <button
            className={cn(
              btnBase, "px-2.5 py-1",
              "bg-foreground text-background hover:opacity-85",
              busy && "opacity-40 cursor-not-allowed",
            )}
            disabled={busy}
            onClick={onDownload}
          >
            <Download className="w-2.5 h-2.5" />
            {t("settings.models.download")}
          </button>
        ) : (
          <>
            {/* 激活 / 已激活 */}
            <button
              className={cn(
                btnBase, "px-2 py-0.5",
                model.is_current
                  ? "bg-muted text-muted-foreground/40 cursor-default"
                  : "bg-emerald-500/10 text-emerald-600 hover:bg-emerald-500/20",
              )}
              disabled={model.is_current}
              onClick={onActivate}
            >
              {model.is_current ? t("settings.models.activated") : t("settings.models.activate")}
            </button>

            {/* 校验（本地已就绪） */}
            {model.is_local && model.is_ready && (
              <button
                className={cn(btnBase, "px-1.5 py-0.5 text-muted-foreground hover:text-foreground hover:bg-accent")}
                disabled={busy}
                onClick={onVerify}
              >
                <RefreshCw className="w-2.5 h-2.5" />
              </button>
            )}

            {/* 删除（本地已就绪） */}
            {model.is_local && model.is_ready && (
              <button
                className={cn(btnBase, "px-1.5 py-0.5 text-muted-foreground hover:text-destructive hover:bg-destructive/10")}
                disabled={busy}
                onClick={() => {
                  if (confirm(t("settings.models.confirmDelete"))) onDelete();
                }}
              >
                <Trash2 className="w-2.5 h-2.5" />
              </button>
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
    <div className="flex items-center gap-1.5 px-3 py-1.5 rounded-md border-l-2 border-emerald-500 bg-emerald-50/40 text-[11px] mb-1">
      <CheckCircle2 className="w-3 h-3 text-emerald-500 shrink-0" />
      <span className="text-muted-foreground">{t("settings.models.current")}</span>
      <span className="font-medium text-foreground">{label}</span>
    </div>
  );
}
