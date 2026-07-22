import { useEffect, useState, useRef } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { Download, CheckCircle2, AlertCircle, Loader2 } from "lucide-react";
import { Button } from "@/components/ui/button";
import { cn } from "@/lib/utils";

/** 后端 BuiltinModelInfo（builtin_models.rs::BuiltinModelInfo）。 */
interface BuiltinModelInfo {
  name: string;
  source: string;
  description: string;
  is_streaming: boolean;
}

/** download-progress 事件 payload。 */
interface DownloadProgress {
  repo: string;
  downloaded: number;
  total: number;
  speed?: number;
}

/** download-done 事件 payload。 */
interface DownloadDone {
  repo: string;
  already_ready?: boolean;
  error?: string;
}

/** 单模型下载状态。 */
type ModelStatus = "pending" | "downloading" | "done" | "error";

interface ModelState {
  info: BuiltinModelInfo;
  status: ModelStatus;
  progress: { downloaded: number; total: number } | null;
  error: string | null;
}

function fmtBytes(n: number): string {
  if (n < 1024) return n + " B";
  if (n < 1048576) return (n / 1024).toFixed(1) + " KB";
  if (n < 1073741824) return (n / 1048576).toFixed(1) + " MB";
  return (n / 1073741824).toFixed(2) + " GB";
}

export default function DownloadPage() {
  const [models, setModels] = useState<ModelState[]>([]);
  const [loading, setLoading] = useState(true);
  const [downloading, setDownloading] = useState(false);
  const unlistenRef = useRef<UnlistenFn[]>([]);

  // 加载缺失的 builtin 模型列表
  useEffect(() => {
    invoke<BuiltinModelInfo[]>("check_builtin_models")
      .then((list) => {
        setModels(
          list.map((info) => ({
            info,
            status: "pending" as ModelStatus,
            progress: null,
            error: null,
          })),
        );
        setLoading(false);
      })
      .catch((e) => {
        console.error("check_builtin_models failed", e);
        setLoading(false);
      });
  }, []);

  // 监听下载事件
  useEffect(() => {
    const setup = async () => {
      const onProgress = await listen<DownloadProgress>("download-progress", (e) => {
        const { repo, downloaded, total } = e.payload;
        setModels((prev) =>
          prev.map((m) =>
            m.info.source === repo
              ? {
                  ...m,
                  status: "downloading",
                  progress: { downloaded, total },
                }
              : m,
          ),
        );
      });
      const onDone = await listen<DownloadDone>("download-done", (e) => {
        const { repo, error } = e.payload;
        setModels((prev) =>
          prev.map((m) =>
            m.info.source === repo
              ? {
                  ...m,
                  status: error ? "error" : "done",
                  progress: error ? m.progress : null,
                  error: error ?? null,
                }
              : m,
          ),
        );
      });
      unlistenRef.current = [onProgress, onDone];
    };
    setup();
    return () => {
      unlistenRef.current.forEach((fn) => fn());
    };
  }, []);

  const allDone = models.length > 0 && models.every((m) => m.status === "done");
  const hasError = models.some((m) => m.status === "error");

  // 下载所有缺失模型（串行，复用 model_commands::download_model）
  // 全部完成后自动进入系统（关闭下载窗）。
  // 全部完成后自动进入系统（关闭下载窗）。
  const handleDownloadAll = async () => {
    setDownloading(true);
    let allSuccess = true;
    for (const m of models) {
      if (m.status === "done") continue;
      setModels((prev) =>
        prev.map((x) =>
          x.info.source === m.info.source ? { ...x, status: "downloading", error: null } : x,
        ),
      );
      try {
        await invoke("download_model", { repo: m.info.source });
        // invoke 成功返回 = 下载完成，直接置 done（不等 download-done 事件，避免竞态）
        setModels((prev) =>
          prev.map((x) =>
            x.info.source === m.info.source
              ? { ...x, status: "done", progress: null, error: null }
              : x,
          ),
        );
      } catch (e) {
        allSuccess = false;
        setModels((prev) =>
          prev.map((x) =>
            x.info.source === m.info.source
              ? { ...x, status: "error", error: String(e) }
              : x,
          ),
        );
      }
    }
    setDownloading(false);
    // 全部成功 → 自动进入系统
    if (allSuccess) {
      handleEnter();
    }
  };

  // 进入系统（关闭下载窗）
  const handleEnter = () => {
    invoke("close_download_window").catch(() => {
      window.close();
    });
  };

  if (loading) {
    return (
      <div className="flex items-center justify-center h-screen text-muted-foreground text-sm">
        <Loader2 className="w-4 h-4 animate-spin mr-2" />
        正在检查…
      </div>
    );
  }

  // 无缺失（理论上不会到这——setup 仅在有缺失时建窗，但防御性处理）
  if (models.length === 0) {
    return (
      <div className="flex flex-col items-center justify-center h-screen gap-3">
        <CheckCircle2 className="w-8 h-8 text-success" />
        <span className="text-sm">内置模型已就绪</span>
        <Button variant="primary" size="sm" onClick={handleEnter}>
          进入系统
        </Button>
      </div>
    );
  }

  return (
    <div className="flex flex-col h-screen">
      {/* 标题区 */}
      <header className="px-6 pt-6 pb-3 border-b border-border/40">
        <h1 className="text-base font-semibold tracking-tight">需要下载内置模型</h1>
        <p className="text-xs text-muted-foreground/70 mt-1 leading-relaxed">
          语音识别的兜底引擎（zipformer-small）未随应用打包，需联网下载约 27 MB。
          下载完成后即可离线使用。下载期间无法使用语音识别，完成后自动进入系统。
        </p>
      </header>

      {/* 模型卡片列表 */}
      <main className="flex-1 overflow-y-auto px-6 py-4">
        <div className="flex flex-col gap-2">
          {models.map((m) => (
            <ModelCard key={m.info.source} model={m} />
          ))}
        </div>
        {hasError && (
          <div className="mt-4 text-xs text-destructive/80 flex items-start gap-1.5">
            <AlertCircle className="w-3.5 h-3.5 mt-px shrink-0" />
            <span>
              部分模型下载失败，可稍后在「系统设置 → 模型管理」重试，或检查网络连接。
            </span>
          </div>
        )}
      </main>

      {/* 底部操作栏 */}
      <footer className="flex items-center justify-between gap-3 px-6 py-4 border-t border-border/40">
        <Button variant="ghost" size="sm" onClick={handleEnter} disabled={downloading}>
          稍后下载
        </Button>
        <div className="flex items-center gap-2">
          {allDone && (
            <span className="text-xs text-success flex items-center gap-1">
              <CheckCircle2 className="w-3.5 h-3.5" />
              全部完成
            </span>
          )}
          <Button
            variant="primary"
            size="sm"
            onClick={handleDownloadAll}
            disabled={downloading || allDone}
          >
            {downloading ? (
              <>
                <Loader2 className="w-3.5 h-3.5 animate-spin" />
                下载中…
              </>
            ) : allDone ? (
              "已就绪"
            ) : (
              <>
                <Download />
                下载并进入系统
              </>
            )}
          </Button>
          {(downloading || allDone) && (
            <Button variant="success" size="sm" onClick={handleEnter}>
              进入系统
            </Button>
          )}
        </div>
      </footer>
    </div>
  );
}

function ModelCard({ model }: { model: ModelState }) {
  const { info, status, progress, error } = model;
  const pct =
    progress && progress.total > 0 ? (progress.downloaded / progress.total) * 100 : 0;

  // 左侧状态色条：pending 灰、downloading 蓝、done 绿、error 红
  const borderColor =
    status === "done"
      ? "border-l-success"
      : status === "error"
        ? "border-l-destructive"
        : status === "downloading"
          ? "border-l-voice"
          : "border-l-border/40";

  return (
    <div
      className={cn(
        "border-l-2 rounded-md py-2.5 px-3 flex items-center justify-between gap-3",
        "bg-muted/30",
        borderColor,
      )}
    >
      {/* 左：名称 + 描述 */}
      <div className="flex flex-col gap-0.5 flex-1 min-w-0">
        <div className="flex items-center gap-1.5">
          {status === "done" ? (
            <CheckCircle2 className="w-3.5 h-3.5 text-success shrink-0" />
          ) : status === "error" ? (
            <AlertCircle className="w-3.5 h-3.5 text-destructive shrink-0" />
          ) : status === "downloading" ? (
            <Loader2 className="w-3.5 h-3.5 text-voice shrink-0 animate-spin" />
          ) : (
            <Download className="w-3.5 h-3.5 text-muted-foreground/50 shrink-0" />
          )}
          <span className="text-xs font-medium">{info.name}</span>
          {info.is_streaming && (
            <span className="text-[9px] text-muted-foreground/50 px-1 py-px rounded bg-muted">
              流式
            </span>
          )}
        </div>
        <span className="text-[11px] text-muted-foreground/60 truncate">
          {error ?? info.description}
        </span>
        {progress && (
          <div className="mt-1 flex items-center gap-2">
            <div className="flex-1 h-1 bg-muted rounded-full overflow-hidden">
              <div className="h-full bg-voice transition-all" style={{ width: `${pct}%` }} />
            </div>
            <span className="text-[10px] text-muted-foreground/50 tabular-nums">
              {fmtBytes(progress.downloaded)} / {fmtBytes(progress.total)}
            </span>
          </div>
        )}
      </div>
      {/* 右：状态标签 */}
      <div className="flex-shrink-0">
        {status === "pending" && (
          <span className="text-[10px] text-muted-foreground/50">待下载</span>
        )}
        {status === "downloading" && pct > 0 && (
          <span className="text-[10px] text-voice tabular-nums">{Math.round(pct)}%</span>
        )}
        {status === "done" && (
          <span className="text-[10px] text-success">就绪</span>
        )}
      </div>
    </div>
  );
}
