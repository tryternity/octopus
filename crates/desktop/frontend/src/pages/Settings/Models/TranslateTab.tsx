import { useState, useEffect, useCallback } from "react";
import { invoke } from "@/lib/tauri";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { HardDrive } from "lucide-react";
import { CollapsibleSection } from "./CollapsibleSection";
import { ModelRow, CurrentBanner, type ModelRowData } from "./ModelRow";
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

export default function TranslateTab({ showToast }: { showToast: (msg: string) => void }) {
  const t = useT();
  const [downloadable, setDownloadable] = useState<DownloadableModel[]>([]);
  const [progress, setProgress] = useState<Record<string, DownloadProgress>>({});
  const [busyRepo, setBusyRepo] = useState<string | null>(null);

  const load = useCallback(async () => {
    try {
      const data = await invoke<DownloadableModel[]>("list_downloadable_models", { domain: "translate" });
      setDownloadable(data);
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

  const onActivate = async (name: string) => {
    try { await invoke("set_config", { key: "translate_engine", value: `local:${name}` }); load(); }
    catch (e) { showToast(t("settings.models.switchFailed") + e); }
  };

  const onDownload = (repo: string) => {
    if (busyRepo) return;
    setBusyRepo(repo);
    invoke("download_model", { repo }).catch((e) => { setBusyRepo(null); showToast(t("settings.models.downloadStartFailed") + e); });
  };

  const onVerify = async (repo: string, name: string) => {
    if (busyRepo) return;
    setBusyRepo(repo);
    try { await invoke("verify_model", { repo, modelName: name }); showToast(t("settings.models.verifyComplete")); load(); }
    catch (e) { showToast(t("settings.models.verifyFailed") + e); }
    finally { setBusyRepo(null); }
  };

  const onDelete = (repo: string) => {
    invoke("delete_model", { repo }).then(load).catch((e) => showToast(e));
  };

  const readyCount = downloadable.filter((m) => m.is_enabled).length;
  // translate 没有持久化 "current" 的概念——translate_engine 存的是引擎 spec
  // 简化：已下载的本地模型即 is_ready，current 通过配置判断
  const currentEngine = (() => {
    try {
      // 从 downloadable + translate_status 无法精确知道 current，用启发式：第一个 ready 的
      return "";
    } catch { return ""; }
  })();

  const rows: ModelRowData[] = downloadable.map((m) => ({
    name: m.name, provider: "local", category: m.category,
    description: m.description, is_ready: m.is_enabled,
    is_current: false, // translate 无 current 概念，不显示绿勾
    is_local: true, repo: m.repo,
  }));

  return (
    <div className="space-y-0.5 max-w-[560px]">
      {currentEngine && <CurrentBanner label={currentEngine} />}
      <CollapsibleSection icon={HardDrive} label={t("settings.models.localModels")} count={`${readyCount}/${downloadable.length}`}>
        {rows.map((m) => (
          <ModelRow key={m.repo} model={m} progress={progress[m.repo]} busy={!!busyRepo}
            onActivate={() => onActivate(m.name)}
            onDownload={() => onDownload(m.repo)}
            onVerify={() => onVerify(m.repo, m.name)}
            onDelete={() => onDelete(m.repo)}
          />
        ))}
      </CollapsibleSection>
    </div>
  );
}
