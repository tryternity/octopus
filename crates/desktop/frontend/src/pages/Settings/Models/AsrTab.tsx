import { useState, useEffect, useCallback } from "react";
import { invoke } from "@/lib/tauri";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { cn } from "@/lib/utils";
import { CheckCircle2, Download, RefreshCw, Cloud, HardDrive } from "lucide-react";
import { CollapsibleSection } from "./CollapsibleSection";
import { useT } from "@/lib/i18n";

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

const selectClass = "px-2.5 py-1.5 border border-border rounded-md text-sm bg-background min-w-[160px] max-w-[220px] cursor-pointer hover:border-foreground/30 transition-colors outline-none focus:border-voice/40";

function fmtBytes(n: number | null | undefined): string {
  if (n == null) return "?";
  if (n < 1024) return n + " B";
  if (n < 1048576) return (n / 1024).toFixed(1) + " KB";
  if (n < 1073741824) return (n / 1048576).toFixed(1) + " MB";
  return (n / 1073741824).toFixed(2) + " GB";
}

export default function AsrTab({ showToast }: { showToast: (msg: string) => void }) {
  const t = useT();
  const [models, setModels] = useState<DownloadableModel[]>([]);
  const [progress, setProgress] = useState<Record<string, DownloadProgress>>({});
  const [busyRepo, setBusyRepo] = useState<string | null>(null);
  const [allEngines, setAllEngines] = useState<EngineOption[]>([]);
  const [cloudEngines, setCloudEngines] = useState<EngineOption[]>([]);

  const loadModels = useCallback(async () => {
    try {
      const data = await invoke<DownloadableModel[]>("list_downloadable_models", { domain: "asr" });
      setModels(data);
    } catch (e) { showToast(t("settings.models.loadFailed") + e); }
  }, [showToast]);

  useEffect(() => {
    loadModels();
    invoke<{ asr_engines: EngineOption[] }>("get_config").then((resp) => {
      setAllEngines(resp.asr_engines ?? []);
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
          if (data.error) showToast(t("settings.models.downloadFailed") + data.error);
          else if (data.already_ready) showToast(t("settings.models.alreadyReady"));
          else showToast(t("settings.models.downloadComplete"));
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
    catch (e) { setBusyRepo(null); showToast(t("settings.models.downloadStartFailed") + e); }
  };

  const handleVerify = async (model: DownloadableModel) => {
    if (busyRepo) return;
    setBusyRepo(model.repo);
    try { await invoke("verify_model", { repo: model.repo, modelName: model.name }); showToast(t("settings.models.verifyComplete")); loadModels(); }
    catch (e) { showToast(t("settings.models.verifyFailed") + e); }
    finally { setBusyRepo(null); }
  };

  const handleSwitchEngine = async (name: string) => {
    try { await invoke("switch_asr_engine", { modelName: name }); }
    catch (e) { showToast(t("settings.models.switchFailed") + e); }
  };

  const readyCount = models.filter((m) => m.is_enabled).length;

  return (
    <div className="space-y-0.5 max-w-[560px]">
      {/* ASR 引擎选择 */}
      <div className="flex items-center justify-between py-2 px-3 rounded-md border border-border/60 bg-surface">
        <span className="text-xs text-muted-foreground">{t("settings.general.asrModel")}</span>
        <select className={selectClass}
          value={allEngines.find((e) => e.current)?.name ?? ""}
          onChange={(e) => handleSwitchEngine(e.target.value)}>
          {allEngines.map((e) => <option key={e.name} value={e.name}>{e.label}</option>)}
        </select>
      </div>

      <CollapsibleSection icon={HardDrive} label={t("settings.models.localModels")} count={`${readyCount}/${models.length}`}>
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
                  <RefreshCw className="w-2.5 h-2.5" /> {t("settings.models.verify")}
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
                  {busyRepo === model.repo ? t("settings.models.downloading") : t("settings.models.download")}
                </button>
              )}
            </div>
          </div>
        );
      })}
      </CollapsibleSection>

      {cloudEngines.length > 0 && (
        <CollapsibleSection icon={Cloud} label={t("settings.models.cloudEngines")}>
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
        </CollapsibleSection>
      )}
    </div>
  );
}
