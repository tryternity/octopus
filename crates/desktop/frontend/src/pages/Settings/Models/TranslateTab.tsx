import { useState, useEffect, useCallback } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { Download, CheckCircle2, Loader2, Languages } from "lucide-react";
import { useT } from "@/lib/i18n";
import { CollapsibleSection } from "./CollapsibleSection";

interface DownloadableModel {
  name: string;
  repo: string;
  description: string;
  category: string;
  is_enabled: boolean;
}

interface TranslationModelInfo {
  name: string;
  source: string;
  downloaded: boolean;
  path: string;
}

interface TranslateStatus {
  strategy: string;
  engineName: string;
  available: boolean;
}

interface EngineOption {
  value: string;
  label: string;
  isLocal: boolean;
  downloaded: boolean;
}

interface DownloadProgress {
  repo: string;
  downloaded: number;
  total: number;
}

export default function TranslateTab({ showToast }: { showToast: (msg: string) => void }) {
  const t = useT();
  const [models, setModels] = useState<TranslationModelInfo[]>([]);
  const [downloadable, setDownloadable] = useState<DownloadableModel[]>([]);
  const [status, setStatus] = useState<TranslateStatus | null>(null);
  const [engineConfig, setEngineConfig] = useState("");
  const [progress, setProgress] = useState<DownloadProgress | null>(null);
  const [busyRepo, setBusyRepo] = useState<string | null>(null);

  const loadData = useCallback(async () => {
    try {
      const [dl, disc, st, cfg] = await Promise.all([
        invoke<DownloadableModel[]>("list_downloadable_models", { domain: "translate" }),
        invoke<TranslationModelInfo[]>("discover_translation_models"),
        invoke<TranslateStatus>("translate_status"),
        invoke<{ config: Record<string, string | number | boolean> }>("get_config"),
      ]);
      setDownloadable(dl);
      setModels(disc);
      setStatus(st);
      setEngineConfig((cfg.config.translate_engine as string) || "");
    } catch (e) {
      showToast(t("settings.models.translate.loadFailed") + e);
    }
  }, [showToast, t]);

  useEffect(() => {
    loadData();
    const unlistenProg = listen<DownloadProgress>("download-progress", (e) => {
      setProgress(e.payload);
    });
    const unlistenDone = listen<{ repo: string; error?: string }>("download-done", (e) => {
      setBusyRepo(null);
      setProgress(null);
      if (e.payload.error) {
        showToast(t("settings.models.translate.downloadFailed") + e.payload.error);
      } else {
        showToast(t("settings.models.translate.downloadComplete"));
        loadData();
      }
    });
    return () => {
      unlistenProg.then((fn) => fn());
      unlistenDone.then((fn) => fn());
    };
  }, [loadData]);

  const handleDownload = async (model: DownloadableModel) => {
    if (busyRepo) return;
    setBusyRepo(model.repo);
    try {
      await invoke("download_model", { repo: model.repo });
    } catch (e) {
      setBusyRepo(null);
      showToast(t("settings.models.translate.downloadStartFailed") + e);
    }
  };

  const handleSetEngine = async (value: string) => {
    setEngineConfig(value);
    try {
      await invoke("set_config", { key: "translate_engine", value });
      showToast(value === ""
        ? t("settings.models.translate.switchAuto")
        : t("settings.models.translate.switchEngine", { value }));
      loadData();
    } catch (e) {
      showToast(t("settings.models.translate.switchFailed") + e);
    }
  };

  const engineOptions: EngineOption[] = [
    { value: "", label: t("settings.models.translate.engineAuto"), isLocal: false, downloaded: true },
    ...models.map((m) => ({
      value: `local:${m.name.split(" ")[0]}`,
      label: `${m.name}${t("settings.models.translate.engineLocal")}`,
      isLocal: true,
      downloaded: m.downloaded,
    })),
    { value: "llm", label: t("settings.models.translate.engineLlm"), isLocal: false, downloaded: true },
  ];

  return (
    <div className="max-w-[560px]">
      <CollapsibleSection icon={Languages} label={t("settings.models.translate.engineTitle")} count={status?.engineName || ""}>
        <div className="space-y-2 py-1">
          <select
            className="w-full bg-background border border-border rounded-md px-3 py-2 text-sm outline-none focus:border-voice/50 focus:ring-1 focus:ring-voice/20 transition-all"
            value={engineConfig}
            onChange={(e) => handleSetEngine(e.target.value)}
          >
            {engineOptions.map((opt) => (
              <option key={opt.value} value={opt.value} disabled={opt.isLocal && !opt.downloaded}>
                {opt.label}{opt.isLocal && !opt.downloaded ? t("settings.models.translate.engineNotDownloaded") : ""}
              </option>
            ))}
          </select>
          {status?.strategy === "auto" && (
            <p className="text-[11px] text-muted-foreground">
              {models.some((m) => m.downloaded)
                ? t("settings.models.translate.autoUsingLocal", { name: status.engineName })
                : t("settings.models.translate.autoUsingLlm")}
            </p>
          )}
        </div>
      </CollapsibleSection>

      <CollapsibleSection
        icon={Download}
        label={t("settings.models.translate.modelTitle")}
        count={`${models.filter((m) => m.downloaded).length}/${models.length}`}
      >
        {downloadable.map((model) => {
          const local = models.find((m) => m.name === model.name);
          const downloaded = local?.downloaded ?? false;
          const isBusy = busyRepo === model.repo;
          return (
            <div key={model.repo} className="flex items-center gap-3 py-2 px-3 rounded-md hover:bg-muted/30">
              <div className="flex-1 min-w-0">
                <div className="flex items-center gap-2">
                  <span className="text-xs font-medium">{model.name}</span>
                  {downloaded && <CheckCircle2 className="w-3.5 h-3.5 text-emerald-500 shrink-0" />}
                </div>
                <span className="text-[11px] text-muted-foreground">{model.description}</span>
              </div>
              {isBusy && progress ? (
                <div className="flex items-center gap-2 shrink-0">
                  <div className="w-20 h-1.5 bg-muted rounded-full overflow-hidden">
                    <div
                      className="h-full bg-voice transition-all"
                      style={{ width: `${(progress.downloaded / progress.total) * 100}%` }}
                    />
                  </div>
                  <Loader2 className="w-3.5 h-3.5 animate-spin text-voice shrink-0" />
                </div>
              ) : downloaded ? (
                <span className="text-[11px] text-emerald-600 shrink-0">{t("settings.models.translate.downloaded")}</span>
              ) : (
                <button
                  onClick={() => handleDownload(model)}
                  disabled={!!busyRepo}
                  className="shrink-0 rounded-md bg-voice/10 px-2.5 py-1 text-[11px] font-medium text-voice transition-colors hover:bg-voice/20 disabled:opacity-40"
                >
                  {t("settings.models.translate.download")}
                </button>
              )}
            </div>
          );
        })}
      </CollapsibleSection>
    </div>
  );
}
