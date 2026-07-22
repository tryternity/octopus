import { useState, useEffect, useRef, type RefObject } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { CheckCircle2, Download, Loader2, AlertCircle, FileDown } from "lucide-react";
import { fmtBytes } from "@/lib/utils";
import { Button } from "@/components/ui/button";

/** 后端 list_model_files 返回的文件信息。 */
interface ModelFile {
  path: string;
  size: number;
  exists: boolean;
}

/** download-progress 事件 payload（文件级）。 */
interface FileProgress {
  repo: string;
  file: string;
  downloaded: number;
  total: number;
  speed?: number;
}

/** download-file 事件 payload（文件状态变更）。 */
interface FileStatus {
  repo: string;
  file: string;
  status: "start" | "done" | "error" | "skip";
}

/** download-done 事件 payload。 */
interface DownloadDone {
  repo: string;
  already_ready?: boolean;
  error?: string;
}

/** 单文件状态。 */
type FileState = "pending" | "exists" | "downloading" | "done" | "error";

interface FileRow {
  info: ModelFile;
  state: FileState;
  downloaded: number;
}

/**
 * 下载浮层：展示模型所有文件的列表 + 文件级进度。
 * 挂在 ModelRow 行级 hover/click 触发。
 */
export default function DownloadPopover({
  repo,
  modelName,
  triggerRef,
  onClose,
}: {
  repo: string;
  modelName: string;
  triggerRef: RefObject<HTMLElement | null>;
  onClose: () => void;
}) {
  const [files, setFiles] = useState<FileRow[]>([]);
  const [loading, setLoading] = useState(true);
  const [downloading, setDownloading] = useState(false);
  const [loadError, setLoadError] = useState<string | null>(null);
  const ref = useRef<HTMLDivElement>(null);
  const unlistenRef = useRef<UnlistenFn[]>([]);

  // 加载文件列表
  useEffect(() => {
    invoke<ModelFile[]>("list_model_files", { repo })
      .then((list) => {
        setFiles(
          list.map((f) => ({
            info: f,
            state: f.exists ? ("exists" as FileState) : ("pending" as FileState),
            downloaded: f.exists ? f.size : 0,
          })),
        );
        setLoading(false);
      })
      .catch((e) => {
        setLoadError(String(e));
        setLoading(false);
      });
  }, [repo]);

  // outside-click 关闭
  useEffect(() => {
    const handler = (e: MouseEvent) => {
      const target = e.target as Node;
      if (triggerRef.current && triggerRef.current.contains(target)) return;
      if (ref.current && !ref.current.contains(target)) onClose();
    };
    document.addEventListener("mousedown", handler);
    return () => document.removeEventListener("mousedown", handler);
  }, [onClose, triggerRef]);

  // 监听下载事件
  useEffect(() => {
    const setup = async () => {
      const onProgress = await listen<FileProgress>("download-progress", (e) => {
        const { repo: r, file, downloaded, total } = e.payload;
        if (r !== repo) return;
        setFiles((prev) =>
          prev.map((f) =>
            f.info.path === file
              ? { ...f, state: "downloading", downloaded, info: { ...f.info, size: total || f.info.size } }
              : f,
          ),
        );
      });
      const onFile = await listen<FileStatus>("download-file", (e) => {
        const { repo: r, file, status } = e.payload;
        if (r !== repo) return;
        setFiles((prev) =>
          prev.map((f) => {
            if (f.info.path !== file) return f;
            const nextState: FileState =
              status === "done" ? "done" :
              status === "error" ? "error" :
              status === "skip" ? "exists" :
              "downloading";
            return { ...f, state: nextState, downloaded: status === "done" || status === "skip" ? f.info.size : f.downloaded };
          }),
        );
      });
      const onDone = await listen<DownloadDone>("download-done", (e) => {
        const { repo: r, error } = e.payload;
        if (r !== repo) return;
        setDownloading(false);
        if (error) {
          setFiles((prev) => prev.map((f) => (f.state === "downloading" ? { ...f, state: "error" } : f)));
        } else {
          setFiles((prev) => prev.map((f) => ({ ...f, state: "exists", downloaded: f.info.size })));
        }
      });
      unlistenRef.current = [onProgress, onFile, onDone];
    };
    setup();
    return () => { unlistenRef.current.forEach((fn) => fn()); };
  }, [repo]);

  const handleDownload = async () => {
    setDownloading(true);
    // 标记 pending 文件为 downloading
    setFiles((prev) => prev.map((f) => (f.state === "pending" ? { ...f, state: "downloading" } : f)));
    try {
      await invoke("download_model", { repo });
    } catch (e) {
      setDownloading(false);
      setFiles((prev) => prev.map((f) => (f.state === "downloading" ? { ...f, state: "error" } : f)));
      console.error("download failed", e);
    }
  };

  const doneCount = files.filter((f) => f.state === "exists" || f.state === "done").length;
  const allDone = files.length > 0 && doneCount === files.length;

  if (loading) {
    return (
      <div ref={ref} className="absolute right-0 top-full mt-1.5 z-50 w-80 bg-background rounded-xl border shadow-xl p-3">
        <Loader2 className="w-4 h-4 animate-spin text-muted-foreground" />
      </div>
    );
  }

  return (
    <div
      ref={ref}
      className="absolute right-0 top-full mt-1.5 z-50 w-80 bg-background rounded-xl border shadow-xl"
    >
      {/* 标题 */}
      <div className="px-3 py-2 border-b">
        <div className="flex items-center gap-1.5">
          <FileDown className="w-3.5 h-3.5 text-muted-foreground" />
          <span className="text-xs font-medium">{modelName}</span>
          <span className="text-[10px] text-muted-foreground/50 ml-auto">{doneCount}/{files.length} 就绪</span>
        </div>
      </div>

      {/* 文件列表 */}
      <div className="px-3 py-2 space-y-1.5 max-h-48 overflow-y-auto">
        {loadError && (
          <div className="text-xs text-destructive/80">{loadError}</div>
        )}
        {files.map((f) => {
          const pct = f.info.size > 0 ? (f.downloaded / f.info.size) * 100 : 0;
          return (
            <div key={f.info.path} className="flex flex-col gap-0.5">
              <div className="flex items-center gap-1.5">
                {f.state === "exists" || f.state === "done" ? (
                  <CheckCircle2 className="w-3 h-3 text-success shrink-0" />
                ) : f.state === "error" ? (
                  <AlertCircle className="w-3 h-3 text-destructive shrink-0" />
                ) : f.state === "downloading" ? (
                  <Loader2 className="w-3 h-3 text-voice shrink-0 animate-spin" />
                ) : (
                  <Download className="w-3 h-3 text-muted-foreground/40 shrink-0" />
                )}
                <span className="text-[11px] truncate flex-1">{f.info.path}</span>
                <span className="text-[9px] text-muted-foreground/50 tabular-nums">
                  {f.state === "pending" ? fmtBytes(f.info.size) : `${fmtBytes(f.downloaded)} / ${fmtBytes(f.info.size)}`}
                </span>
              </div>
              {f.state === "downloading" && (
                <div className="h-0.5 bg-muted rounded-full overflow-hidden ml-4">
                  <div className="h-full bg-voice transition-all" style={{ width: `${pct}%` }} />
                </div>
              )}
            </div>
          );
        })}
      </div>

      {/* 操作 */}
      <div className="px-3 py-2 border-t flex justify-end gap-2">
        <Button variant="ghost" size="sm" onClick={onClose}>关闭</Button>
        <Button
          variant={allDone ? "ghost" : "primary"}
          size="sm"
          onClick={handleDownload}
          disabled={downloading || allDone}
        >
          {downloading ? (
            <><Loader2 className="w-3 h-3 animate-spin" /> 下载中…</>
          ) : allDone ? (
            "已就绪"
          ) : (
            <><Download /> {doneCount > 0 ? "继续下载" : "下载"}</>
          )}
        </Button>
      </div>
    </div>
  );
}
