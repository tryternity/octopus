import { useState, useEffect, useCallback } from "react";
import { invoke } from "@/lib/tauri";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { cn } from "@/lib/utils";
import { CheckCircle2, HardDrive, Download, RefreshCw } from "lucide-react";
import { CollapsibleSection } from "./CollapsibleSection";
import { useT } from "@/lib/i18n";

interface DownloadableModel {
  name: string;
  repo: string;
  description: string;
  category: string;
  is_enabled: boolean;
}

interface OcrOption {
  id: number;
  name: string;
  label: string;
  current: boolean;
  is_enabled: boolean;
  is_local: boolean;
}

interface DownloadProgress {
  repo: string;
  downloaded: number;
  total: number;
}

function CurrentBanner({ label }: { label: string }) {
  const t = useT();
  return (
    <div className="flex items-center gap-1.5 px-3 py-1.5 rounded-md border-l-2 border-voice bg-voice/5 text-[11px] mb-1">
      <CheckCircle2 className="w-3 h-3 text-voice shrink-0" />
      <span className="text-muted-foreground">{t("settings.models.current")}</span>
      <span className="font-medium text-foreground">{label}</span>
    </div>
  );
}

export default function OcrTab({ showToast }: { showToast: (msg: string) => void }) {
  const t = useT();
  const [models, setModels] = useState<OcrOption[]>([]);
  const [downloadable, setDownloadable] = useState<DownloadableModel[]>([]);
  const [progress, setProgress] = useState<Record<string, DownloadProgress>>({});
  const [busyRepo, setBusyRepo] = useState<string | null>(null);

  const load = useCallback(async () => {
    try {
      const resp = await invoke<{ ocr_models: OcrOption[] }>("get_config");
      setModels(resp.ocr_models ?? []);
      const dl = await invoke<DownloadableModel[]>("list_downloadable_models", { domain: "ocr" });
      setDownloadable(dl);
    } catch (e) { showToast(t("settings.models.loadFailed") + e); }
  }, [showToast, t]);

  useEffect(() => {
    load();
    let unlistens: UnlistenFn[] = [];
    let cancelled = false;
    (async () => {
      const subs: [string, (p: unknown) => void][] = [
        ["download-progress", (p) => {
          const prog = p as DownloadProgress;
          setProgress((prev) => ({ ...prev, [prog.repo]: prog }));
        }],
        ["download-done", (p) => {
          const data = p as { repo: string; already_ready?: boolean; error?: string };
          setBusyRepo(null);
          setProgress((prev) => { const next = { ...prev }; delete next[data.repo]; return next; });
          if (data.error) showToast(t("settings.models.downloadFailed") + data.error);
          else if (data.already_ready) showToast(t("settings.models.alreadyReady"));
          else showToast(t("settings.models.downloadComplete"));
          load();
        }],
      ];
      for (const [event, handler] of subs) {
        const fn = await listen(event, (e) => handler(e.payload));
        if (cancelled) { fn(); return; }
        unlistens.push(fn);
      }
    })();
    return () => { cancelled = true; unlistens.forEach((fn) => fn()); };
  }, [load, showToast, t]);

  const handleDownload = async (model: DownloadableModel) => {
    if (busyRepo) return;
    setBusyRepo(model.repo);
    try { await invoke("download_model", { repo: model.repo }); }
    catch (e) { setBusyRepo(null); showToast(t("settings.models.downloadStartFailed") + e); }
  };

  const handleVerify = async (model: DownloadableModel) => {
    if (busyRepo) return;
    setBusyRepo(model.repo);
    try { await invoke("verify_model", { repo: model.repo, modelName: model.name }); showToast(t("settings.models.verifyComplete")); load(); }
    catch (e) { showToast(t("settings.models.verifyFailed") + e); }
    finally { setBusyRepo(null); }
  };

  const current = models.find((m) => m.current);

  const handleSetOcrModel = async (name: string) => {
    try { await invoke("set_config", { key: "ocr_model", value: name }); load(); }
    catch (e) { showToast(t("settings.models.switchFailed") + e); }
  };

  const selectClass = "px-2.5 py-1.5 border border-border rounded-md text-sm bg-background min-w-[160px] max-w-[220px] cursor-pointer hover:border-foreground/30 transition-colors outline-none focus:border-voice/40";

  return (
    <div className="space-y-0.5 max-w-[560px]">
      {current && <CurrentBanner label={current.label} />}

      <div className="flex items-center justify-between py-2 px-3 rounded-md border border-border/60 bg-surface">
        <span className="text-xs text-muted-foreground">{t("settings.general.ocrModel")}</span>
        <select className={selectClass}
          value={models.find((m) => m.current)?.name ?? ""}
          onChange={(e) => handleSetOcrModel(e.target.value)}>
          {models.map((m) => <option key={m.name} value={m.name}>{m.label}</option>)}
        </select>
      </div>
      <CollapsibleSection icon={HardDrive} label={t("settings.models.localModels")} count={`${downloadable.filter(m => m.is_enabled).length}/${downloadable.length}`}>
        {downloadable.map((model) => {
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
                  {model.is_enabled && <CheckCircle2 className="w-3 h-3 text-voice" />}
                </div>
                <span className="text-[11px] text-muted-foreground/60">{model.description}</span>
                {prog && (
                  <div className="mt-1">
                    <div className="h-1 bg-muted rounded-full overflow-hidden">
                      <div className="h-full bg-voice transition-all" style={{ width: `${pct}%` }} />
                    </div>
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
    </div>
  );
}
