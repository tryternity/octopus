import { useState, useEffect, useCallback } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { cn } from "@/lib/utils";
import { Server, CheckCircle2, Download, RefreshCw, Cloud } from "lucide-react";

interface DownloadableModel {
  name: string;
  repo: string;
  description: string;
  category: string;
  is_enabled: boolean;
}

interface DownloadProgress {
  repo: string;
  downloaded: number;
  total: number;
}

function fmtBytes(n: number | null | undefined): string {
  if (n == null) return "?";
  if (n < 1024) return n + " B";
  if (n < 1048576) return (n / 1024).toFixed(1) + " KB";
  if (n < 1073741824) return (n / 1048576).toFixed(1) + " MB";
  return (n / 1073741824).toFixed(2) + " GB";
}

export default function ModelsPanel({ showToast }: { showToast: (msg: string) => void }) {
  const [models, setModels] = useState<DownloadableModel[]>([]);
  const [mirror, setMirror] = useState("");
  const [progress, setProgress] = useState<Record<string, DownloadProgress>>({});
  const [busyRepo, setBusyRepo] = useState<string | null>(null);

  const loadModels = useCallback(async () => {
    try {
      const data = await invoke<DownloadableModel[]>("list_downloadable_models");
      setModels(data);
    } catch (e) { showToast("加载模型列表失败：" + e); }
  }, [showToast]);

  useEffect(() => {
    loadModels();
    invoke<string>("get_config").then((resp: any) => {
      setMirror(resp.config?.download_mirror ?? "");
    }).catch(() => {});

    let unlistens: UnlistenFn[] = [];
    let cancelled = false;
    (async () => {
      const subs: [string, (p: unknown) => void][] = [
        ["download-progress", (p) => {
          const prog = p as DownloadProgress;
          setProgress((prev) => ({ ...prev, [prog.repo]: prog }));
        }],
        ["download-file", (p) => {
          const data = p as { repo: string; file: string; downloaded: number; total: number };
          setProgress((prev) => ({ ...prev, [data.repo]: { repo: data.repo, downloaded: data.downloaded, total: data.total } }));
        }],
        ["download-done", (p) => {
          const data = p as { repo: string; already_ready?: boolean; error?: string };
          setBusyRepo(null);
          setProgress((prev) => { const next = { ...prev }; delete next[data.repo]; return next; });
          if (data.error) showToast("下载失败：" + data.error);
          else if (data.already_ready) showToast("模型已就绪，无需重新下载");
          else showToast("下载完成");
          loadModels();
        }],
      ];
      for (const [event, handler] of subs) {
        const fn = await listen(event, (e) => handler(e.payload));
        if (cancelled) { fn(); return; }
        unlistens.push(fn);
      }
    })();
    return () => { cancelled = true; unlistens.forEach((fn) => fn()); };
  }, [loadModels, showToast]);

  const handleDownload = async (model: DownloadableModel) => {
    if (busyRepo) return;
    setBusyRepo(model.repo);
    try { await invoke("download_model", { repo: model.repo }); }
    catch (e) { setBusyRepo(null); showToast("下载启动失败：" + e); }
  };

  const handleVerify = async (model: DownloadableModel) => {
    if (busyRepo) return;
    setBusyRepo(model.repo);
    try { await invoke("verify_model", { repo: model.repo }); showToast("校验完成"); loadModels(); }
    catch (e) { showToast("校验失败：" + e); }
    finally { setBusyRepo(null); }
  };

  const handleSetMirror = async () => {
    try { await invoke("set_download_mirror", { mirror }); showToast("镜像已设置"); }
    catch (e) { showToast("设置失败：" + e); }
  };

  const readyCount = models.filter((m) => m.is_enabled).length;

  return (
    <div className="max-w-[640px]">
      {/* 镜像设置 */}
      <div className="mb-3 border border-border rounded-lg overflow-hidden bg-background">
        <div className="flex items-center gap-2 px-4 py-2.5 bg-muted/40 border-b border-border">
          <Cloud className="w-4 h-4 text-muted-foreground" />
          <h3 className="text-sm font-semibold">下载源</h3>
        </div>
        <div className="px-4 py-3 flex items-center justify-between gap-3">
          <div className="flex flex-col gap-0.5 flex-1 min-w-0">
            <span className="text-sm">HF 镜像</span>
            <span className="text-xs text-muted-foreground/60">国内推荐 https://hf-mirror.com；留空用官方源</span>
          </div>
          <div className="flex gap-2 flex-shrink-0">
            <input
              type="text"
              className="px-2.5 py-1.5 border border-border rounded-md text-sm bg-background outline-none focus:border-voice/40 transition-colors min-w-[180px]"
              placeholder="https://hf-mirror.com"
              value={mirror}
              onChange={(e) => setMirror(e.target.value)}
            />
            <button
              className="px-3 py-1.5 border border-border rounded-md text-sm hover:border-foreground/30 transition-colors"
              onClick={handleSetMirror}
            >
              设置
            </button>
          </div>
        </div>
      </div>

      {/* 模型列表 */}
      <div className="border border-border rounded-lg overflow-hidden bg-background">
        <div className="flex items-center gap-2 px-4 py-2.5 bg-muted/40 border-b border-border">
          <Server className="w-4 h-4 text-muted-foreground" />
          <h3 className="text-sm font-semibold">ASR 模型</h3>
          <span className="ml-auto text-[10px] text-muted-foreground/60">{readyCount}/{models.length} 就绪</span>
        </div>
        <div className="px-4 py-1">
          {models.map((model) => {
            const prog = progress[model.repo];
            const pct = prog && prog.total > 0 ? (prog.downloaded / prog.total) * 100 : 0;
            return (
              <div key={model.repo} className="flex items-start justify-between py-2.5 border-b border-border/40 last:border-0 gap-3">
                <div className="flex flex-col gap-0.5 flex-1 min-w-0">
                  <div className="flex items-center gap-1.5">
                    <span className="text-sm font-medium">{model.name}</span>
                    <span className="text-[10px] text-muted-foreground/60 px-1.5 py-0.5 rounded bg-muted">{model.category}</span>
                    {model.is_enabled && <CheckCircle2 className="w-3.5 h-3.5 text-voice" />}
                  </div>
                  <span className="text-xs text-muted-foreground/70">{model.description}</span>
                  {prog && (
                    <div className="mt-1">
                      <div className="h-1 bg-muted rounded-full overflow-hidden">
                        <div className="h-full bg-voice transition-all" style={{ width: `${pct}%` }} />
                      </div>
                      <span className="text-[10px] text-muted-foreground/60">{fmtBytes(prog.downloaded)} / {fmtBytes(prog.total)}</span>
                    </div>
                  )}
                </div>
                <div className="flex flex-col items-end gap-1 flex-shrink-0">
                  {model.is_enabled ? (
                    <button
                      className="flex items-center gap-1 px-2.5 py-1 text-xs text-muted-foreground hover:text-foreground transition-colors"
                      disabled={!!busyRepo}
                      onClick={() => handleVerify(model)}
                    >
                      <RefreshCw className="w-3 h-3" /> 校验
                    </button>
                  ) : (
                    <button
                      className={cn(
                        "flex items-center gap-1 px-3 py-1.5 rounded-md text-xs transition-colors",
                        "bg-foreground text-background hover:opacity-85",
                        busyRepo && "opacity-40 cursor-not-allowed",
                      )}
                      disabled={!!busyRepo}
                      onClick={() => handleDownload(model)}
                    >
                      <Download className="w-3 h-3" />
                      {busyRepo === model.repo ? "下载中…" : "下载"}
                    </button>
                  )}
                </div>
              </div>
            );
          })}
        </div>
      </div>
    </div>
  );
}
