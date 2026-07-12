import { useState, useEffect, useCallback } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { Download, CheckCircle2, Loader2, Languages } from "lucide-react";
import CollapsibleSection from "./CollapsibleSection";

interface DownloadableModel {
  name: string;
  repo: string;
  description: string;
  sizeMb: number;
}

interface TranslationModelInfo {
  name: string;
  source: string;
  downloaded: boolean;
  sizeMb: number;
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
  const [models, setModels] = useState<TranslationModelInfo[]>([]);
  const [downloadable, setDownloadable] = useState<DownloadableModel[]>([]);
  const [status, setStatus] = useState<TranslateStatus | null>(null);
  const [engineConfig, setEngineConfig] = useState("");
  const [progress, setProgress] = useState<DownloadProgress | null>(null);
  const [busyRepo, setBusyRepo] = useState<string | null>(null);

  const loadData = useCallback(async () => {
    try {
      const [dl, disc, st, cfg] = await Promise.all([
        invoke<DownloadableModel[]>("list_downloadable_translation_models"),
        invoke<TranslationModelInfo[]>("discover_translation_models"),
        invoke<TranslateStatus>("translate_status"),
        invoke<{ config: Record<string, string | number | boolean> }>("get_config"),
      ]);
      setDownloadable(dl);
      setModels(disc);
      setStatus(st);
      setEngineConfig((cfg.config.translate_engine as string) || "");
    } catch (e) {
      showToast("加载翻译模型失败：" + e);
    }
  }, [showToast]);

  useEffect(() => {
    loadData();
    const unlistenProg = listen<DownloadProgress>("download-progress", (e) => {
      setProgress(e.payload);
    });
    const unlistenDone = listen<{ repo: string; error?: string }>("download-done", (e) => {
      setBusyRepo(null);
      setProgress(null);
      if (e.payload.error) {
        showToast("下载失败：" + e.payload.error);
      } else {
        showToast("下载完成");
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
      showToast("下载启动失败：" + e);
    }
  };

  const handleSetEngine = async (value: string) => {
    setEngineConfig(value);
    try {
      await invoke("set_config", { key: "translate_engine", value });
      showToast(value === "" ? "已切换为自动模式" : `已切换引擎：${value}`);
      loadData();
    } catch (e) {
      showToast("设置失败：" + e);
    }
  };

  const engineOptions: EngineOption[] = [
    { value: "", label: "自动（推荐）", isLocal: false, downloaded: true },
    ...models.map((m) => ({
      value: `local:${m.name.split(" ")[0].toLowerCase()}`,
      label: `${m.name}（本地）`,
      isLocal: true,
      downloaded: m.downloaded,
    })),
    { value: "llm", label: "LLM（远程）", isLocal: false, downloaded: true },
  ];

  return (
    <div className="max-w-[560px]">
      <CollapsibleSection icon={Languages} label="翻译引擎" count={status?.engineName || ""}>
        <div className="space-y-2 py-1">
          <select
            className="w-full bg-background border border-border rounded-md px-3 py-2 text-sm outline-none focus:border-voice/50 focus:ring-1 focus:ring-voice/20 transition-all"
            value={engineConfig}
            onChange={(e) => handleSetEngine(e.target.value)}
          >
            {engineOptions.map((opt) => (
              <option key={opt.value} value={opt.value} disabled={opt.isLocal && !opt.downloaded}>
                {opt.label}{opt.isLocal && !opt.downloaded ? "（未下载）" : ""}
              </option>
            ))}
          </select>
          {status?.strategy === "auto" && (
            <p className="text-[11px] text-muted-foreground">
              {models.some((m) => m.downloaded)
                ? `当前将使用本地引擎：${status.engineName}`
                : "未检测到本地翻译模型，将使用 LLM 翻译"}
            </p>
          )}
        </div>
      </CollapsibleSection>

      <CollapsibleSection
        icon={Download}
        label="翻译模型"
        count={`${models.filter((m) => m.downloaded).length}/${models.length}`}
      >
        {downloadable.map((model) => {
          const local = models.find((m) => m.source === model.repo);
          const downloaded = local?.downloaded ?? false;
          const isBusy = busyRepo === model.repo;
          return (
            <div key={model.repo} className="flex items-center gap-3 py-2 px-3 rounded-md hover:bg-muted/30">
              <div className="flex-1 min-w-0">
                <div className="flex items-center gap-2">
                  <span className="text-xs font-medium">{model.name}</span>
                  {downloaded && <CheckCircle2 className="w-3.5 h-3.5 text-emerald-500 shrink-0" />}
                </div>
                <span className="text-[11px] text-muted-foreground">{model.description} · {model.sizeMb}MB</span>
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
                <span className="text-[11px] text-emerald-600 shrink-0">已下载</span>
              ) : (
                <button
                  onClick={() => handleDownload(model)}
                  disabled={!!busyRepo}
                  className="shrink-0 rounded-md bg-voice/10 px-2.5 py-1 text-[11px] font-medium text-voice transition-colors hover:bg-voice/20 disabled:opacity-40"
                >
                  下载
                </button>
              )}
            </div>
          );
        })}
      </CollapsibleSection>
    </div>
  );
}
