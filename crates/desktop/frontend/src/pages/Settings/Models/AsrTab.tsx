import { useState, useEffect, useCallback } from "react";
import { invoke } from "@/lib/tauri";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { cn } from "@/lib/utils";
import { CheckCircle2, Download, RefreshCw, Cloud, HardDrive } from "lucide-react";

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

interface EngineOption {
  name: string;
  label: string;
  current: boolean;
  is_local: boolean;
}

function fmtBytes(n: number | null | undefined): string {
  if (n == null) return "?";
  if (n < 1024) return n + " B";
  if (n < 1048576) return (n / 1024).toFixed(1) + " KB";
  if (n < 1073741824) return (n / 1048576).toFixed(1) + " MB";
  return (n / 1073741824).toFixed(2) + " GB";
}

function SectionHeader({ icon: Icon, label, count }: { icon: React.ElementType; label: string; count?: string }) {
  return (
    <div className="flex items-center gap-1.5 pt-3 pb-1 first:pt-0">
      <Icon className="w-3 h-3 text-muted-foreground/60" />
      <span className="text-[11px] font-medium text-muted-foreground">{label}</span>
      {count && <span className="text-[10px] text-muted-foreground/40">{count}</span>}
    </div>
  );
}

function CurrentBanner({ label }: { label: string }) {
  return (
    <div className="flex items-center gap-1.5 px-3 py-1.5 rounded-md border-l-2 border-voice bg-voice/5 text-[11px] mb-1">
      <CheckCircle2 className="w-3 h-3 text-voice shrink-0" />
      <span className="text-muted-foreground">当前使用</span>
      <span className="font-medium text-foreground">{label}</span>
    </div>
  );
}

export default function AsrTab({ showToast }: { showToast: (msg: string) => void }) {
  const [models, setModels] = useState<DownloadableModel[]>([]);
  const [progress, setProgress] = useState<Record<string, DownloadProgress>>({});
  const [busyRepo, setBusyRepo] = useState<string | null>(null);
  const [currentLabel, setCurrentLabel] = useState("");
  const [cloudEngines, setCloudEngines] = useState<EngineOption[]>([]);

  const loadModels = useCallback(async () => {
    try {
      const data = await invoke<DownloadableModel[]>("list_downloadable_models");
      setModels(data);
    } catch (e) { showToast("加载模型列表失败：" + e); }
  }, [showToast]);

  useEffect(() => {
    loadModels();
    invoke<{ asr_engines: EngineOption[] }>("get_config").then((resp) => {
      const cur = resp.asr_engines?.find((e) => e.current);
      setCurrentLabel(cur?.label ?? "");
      setCloudEngines(resp.asr_engines?.filter((e) => !e.is_local) ?? []);
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

  const readyCount = models.filter((m) => m.is_enabled).length;

  return (
    <div className="space-y-0.5 max-w-[560px]">
      {currentLabel && <CurrentBanner label={currentLabel} />}

      <SectionHeader icon={HardDrive} label="本地模型" count={`${readyCount}/${models.length}`} />
      {models.map((model) => {
        const prog = progress[model.repo];
        const pct = prog && prog.total > 0 ? (prog.downloaded / prog.total) * 100 : 0;
        return (
          <div
            key={model.repo}
            className={cn(
              "group flex items-start justify-between py-2 px-3 rounded-md gap-3 transition-colors",
              "border-l-2 border-border/40 hover:border-border hover:bg-accent/30",
              model.is_enabled && "border-l-voice/40",
            )}
          >
            <div className="flex flex-col gap-0.5 flex-1 min-w-0">
              <div className="flex items-center gap-1.5">
                <span className="text-xs font-medium">{model.name}</span>
                <span className="text-[9px] text-muted-foreground/50 px-1 py-px rounded bg-muted">{model.category}</span>
                {model.is_enabled && <CheckCircle2 className="w-3 h-3 text-voice" />}
              </div>
              <span className="text-[11px] text-muted-foreground/60">{model.description}</span>
              {prog && (
                <div className="mt-1">
                  <div className="h-1 bg-muted rounded-full overflow-hidden">
                    <div className="h-full bg-voice transition-all" style={{ width: `${pct}%` }} />
                  </div>
                  <span className="text-[10px] text-muted-foreground/50">{fmtBytes(prog.downloaded)} / {fmtBytes(prog.total)}</span>
                </div>
              )}
            </div>
            <div className="flex flex-col items-end gap-1 flex-shrink-0">
              {model.is_enabled ? (
                <button
                  className="flex items-center gap-1 px-2 py-0.5 text-[11px] text-muted-foreground hover:text-foreground transition-colors rounded hover:bg-accent"
                  disabled={!!busyRepo}
                  onClick={() => handleVerify(model)}
                >
                  <RefreshCw className="w-2.5 h-2.5" /> 校验
                </button>
              ) : (
                <button
                  className={cn(
                    "flex items-center gap-1 px-2.5 py-1 rounded-md text-[11px] transition-all",
                    "bg-foreground text-background hover:opacity-85",
                    busyRepo && "opacity-40 cursor-not-allowed",
                  )}
                  disabled={!!busyRepo}
                  onClick={() => handleDownload(model)}
                >
                  <Download className="w-2.5 h-2.5" />
                  {busyRepo === model.repo ? "下载中…" : "下载"}
                </button>
              )}
            </div>
          </div>
        );
      })}

      {cloudEngines.length > 0 && (
        <>
          <SectionHeader icon={Cloud} label="云端引擎" />
          {cloudEngines.map((engine) => (
            <div
              key={engine.name}
              className={cn(
                "flex items-center justify-between py-2 px-3 rounded-md transition-colors",
                "border-l-2 border-border/40",
                engine.current && "border-l-voice/40 bg-voice/5",
              )}
            >
              <div className="flex items-center gap-1.5">
                <span className="text-xs font-medium">{engine.label}</span>
                {engine.current && <CheckCircle2 className="w-3 h-3 text-voice" />}
              </div>
              <span className="text-[9px] text-muted-foreground/40 px-1 py-px rounded bg-muted font-mono">{engine.name}</span>
            </div>
          ))}
        </>
      )}
    </div>
  );
}
