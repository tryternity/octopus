import { useState, useEffect, useCallback } from "react";
import { invoke } from "@/lib/tauri";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { Cloud, HardDrive, Plus } from "lucide-react";
import { CollapsibleSection } from "./CollapsibleSection";
import { ModelRow, CurrentBanner, type ModelRowData } from "./ModelRow";
import { CloudModelForm, type CloudModelData } from "./CloudModelForm";
import { useT } from "@/lib/i18n";

interface DownloadableModel {
  name: string;
  repo: string;
  description: string;
  category: string;
  is_enabled: boolean;
}

interface EngineOption {
  name: string;
  provider: string;
  category: string;
  current: boolean;
  is_local: boolean;
  label: string;
}

interface DownloadProgress {
  repo: string;
  downloaded: number;
  total: number;
}

export default function AsrTab({ showToast }: { showToast: (msg: string) => void }) {
  const t = useT();
  const [downloadable, setDownloadable] = useState<DownloadableModel[]>([]);
  const [engines, setEngines] = useState<EngineOption[]>([]);
  const [progress, setProgress] = useState<Record<string, DownloadProgress>>({});
  const [busyRepo, setBusyRepo] = useState<string | null>(null);

  const load = useCallback(async () => {
    try {
      const [data, resp] = await Promise.all([
        invoke<DownloadableModel[]>("list_downloadable_models", { domain: "asr" }),
        invoke<{ asr_engines: EngineOption[] }>("get_config"),
      ]);
      setDownloadable(data);
      setEngines(resp.asr_engines ?? []);
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

  const isCurrent = (name: string) => engines.some((e) => e.current && e.name === name);
  const currentLabel = engines.find((e) => e.current)?.label ?? "";

  const onActivate = (name: string) =>
    invoke("switch_asr_engine", { modelName: name }).then(load).catch((e) => showToast(t("settings.models.switchFailed") + e));

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

  const [showForm, setShowForm] = useState(false);
  const [editTarget, setEditTarget] = useState<CloudModelData | null>(null);

  // 合并本地 + 云端
  const localRows: ModelRowData[] = downloadable.map((m) => ({
    name: m.name, provider: "local", category: m.category,
    description: m.description, is_ready: m.is_enabled,
    is_current: isCurrent(m.name), is_local: true, repo: m.repo,
  }));

  const cloudEngines = engines.filter((e) => !e.is_local);
  const cloudRows: ModelRowData[] = cloudEngines.map((e) => ({
    name: e.name, provider: e.provider, category: e.category,
    description: e.label, is_ready: true,
    is_current: e.current, is_local: false, repo: "",
  }));

  const onDeleteCloud = async (_name: string, _provider: string) => {
    // 云端模型删除需要 id——从 engines 找不到 id，用 DB 查
    // 简化：通过 name 找 engines 中的匹配项再调 remove_cloud_model
    // 实际需要后端返回 id，暂时用 name+provider 查
    try {
      // 找到 engines 中的索引——但 EngineOption 没有 id
      // TODO: 后端 EngineOption 加 id 字段，或 remove_cloud_model 按 name+provider+domain 删
      showToast(t("settings.models.deleteCloudHint") || "请通过编辑修改");
    } catch (e) { showToast(String(e)); }
  };

  return (
    <div className="space-y-0.5 max-w-[560px]">
      {currentLabel && <CurrentBanner label={currentLabel} />}

      <CollapsibleSection icon={HardDrive} label={t("settings.models.localModels")} count={`${readyCount}/${downloadable.length}`}>
        {localRows.map((m) => (
          <ModelRow key={m.repo} model={m} progress={progress[m.repo]} busy={!!busyRepo}
            onActivate={() => onActivate(m.name)}
            onDownload={() => onDownload(m.repo)}
            onVerify={() => onVerify(m.repo, m.name)}
            onDelete={() => onDelete(m.repo)}
          />
        ))}
      </CollapsibleSection>

      <CollapsibleSection icon={Cloud} label={t("settings.models.cloudEngines")}>
        <div className="flex justify-end pb-1">
          <button className="flex items-center gap-1 px-2 py-0.5 text-[11px] rounded bg-voice/10 text-voice hover:bg-voice/20 transition-colors"
            onClick={() => { setEditTarget(null); setShowForm(true); }}>
            <Plus className="w-2.5 h-2.5" /> {t("settings.models.addModel")}
          </button>
        </div>
        {cloudRows.map((m) => (
          <ModelRow key={m.provider + ":" + m.name} model={m} progress={null} busy={!!busyRepo}
            onActivate={() => onActivate(m.name)}
            onDownload={() => {}}
            onVerify={() => {}}
            onDelete={() => onDeleteCloud(m.name, m.provider)}
          />
        ))}
        {cloudRows.length === 0 && (
          <div className="text-[11px] text-muted-foreground/50 py-3 text-center">
            {t("settings.models.noCloudModels")}
          </div>
        )}
        </CollapsibleSection>

      {showForm && (
        <CloudModelForm
          domain="asr"
          editModel={editTarget}
          onSaved={() => { setShowForm(false); setEditTarget(null); load(); }}
          onCancel={() => { setShowForm(false); setEditTarget(null); }}
        />
      )}
    </div>
  );
}
