import { useState, useEffect, useCallback } from "react";
import { invoke } from "@/lib/tauri";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { Cloud, HardDrive, Plus } from "lucide-react";
import { CollapsibleSection } from "./CollapsibleSection";
import { ModelRow, CurrentBanner, type ModelRowData } from "./ModelRow";
import { CloudModelForm, type CloudModelData } from "./CloudModelForm";
import { Button } from "@/components/ui/button";
import { useT } from "@/lib/i18n";

interface EngineOption {
  id: number;
  name: string;
  provider: string;
  category: string;
  current: boolean;
  is_local: boolean;
  label: string;
  source: string;
  secret_key: string;
  is_streaming: boolean;
  is_thinking: boolean;
}

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
    cloudId: e.id,
  }));

  const onDeleteCloud = async (id: number) => {
    try { await invoke("remove_cloud_model", { id }); load(); }
    catch (e) { showToast(String(e)); }
  };

  const onEditCloud = (e: EngineOption) => {
    setEditTarget({
      id: e.id, domain: "asr", provider: e.provider, category: e.category,
      modelName: e.name, source: e.source, secretKey: e.secret_key,
      isStreaming: e.is_streaming, isThinking: e.is_thinking,
    });
    setShowForm(true);
  };

  return (
    <div className="space-y-0.5">
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
          <Button variant="voice-soft" size="sm"
            onClick={() => { setEditTarget(null); setShowForm(true); }}>
            <Plus /> {t("settings.models.addModel")}
          </Button>
        </div>
        {cloudRows.map((m) => {
          const engine = cloudEngines.find((e) => e.id === m.cloudId);
          return (
          <ModelRow key={m.provider + ":" + m.name} model={m} progress={null} busy={!!busyRepo}
            onActivate={() => onActivate(m.name)}
            onDownload={() => {}}
            onVerify={() => {}}
            onDelete={() => m.cloudId && onDeleteCloud(m.cloudId)}
            onEdit={engine ? () => onEditCloud(engine) : undefined}
          />
          );
        })}
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
