import { useState, useEffect, useCallback } from "react";
import { invoke } from "@/lib/tauri";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { Cloud, HardDrive, Plus } from "lucide-react";
import { CollapsibleSection } from "./CollapsibleSection";
import { ModelRow, CurrentBanner, type ModelRowData } from "./ModelRow";
import { CloudModelForm, type CloudModelData } from "./CloudModelForm";
import { Button } from "@/components/ui/button";
import { useT } from "@/lib/i18n";

interface DownloadableModel {
  id: number;
  name: string;
  repo: string;
  description: string;
  category: string;
  // Task 2 后：is_available=就绪；is_enabled=激活
  is_available: boolean;
  is_enabled: boolean;
}

interface TranslateCloudModel {
  id: number;
  provider: string;
  category: string;
  modelName: string;
  source: string;
  secretKey: string;
  isStreaming: boolean;
  isThinking: boolean;
}

interface TranslateStatus {
  strategy: string;
  engineName: string;
  available: boolean;
}

interface DownloadProgress {
  repo: string;
  downloaded: number;
  total: number;
}

export default function TranslateTab({ showToast }: { showToast: (msg: string) => void }) {
  const t = useT();
  const [downloadable, setDownloadable] = useState<DownloadableModel[]>([]);
  const [cloudModels, setCloudModels] = useState<TranslateCloudModel[]>([]);
  const [status, setStatus] = useState<TranslateStatus | null>(null);
  const [progress, setProgress] = useState<Record<string, DownloadProgress>>({});
  const [busyRepo, setBusyRepo] = useState<string | null>(null);
  const [showForm, setShowForm] = useState(false);
  const [editTarget, setEditTarget] = useState<CloudModelData | null>(null);

  const load = useCallback(async () => {
    try {
      const [data, cloud, st] = await Promise.all([
        invoke<DownloadableModel[]>("list_downloadable_models", { domain: "translate" }),
        invoke<TranslateCloudModel[]>("list_translate_cloud_models"),
        invoke<TranslateStatus>("translate_status"),
      ]);
      setDownloadable(data);
      setCloudModels(cloud);
      setStatus(st);
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

  // Task 2 后：统一走 switch_active_model(domain, id)。
  const onActivate = async (id: number) => {
    try { await invoke("switch_active_model", { domain: "translate", id }); load(); }
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

  const onDeleteCloud = async (id: number) => {
    try { await invoke("remove_cloud_model", { id }); load(); }
    catch (e) { showToast(String(e)); }
  };

  const onEditCloud = (m: TranslateCloudModel) => {
    setEditTarget({
      id: m.id, domain: "translate", provider: m.provider, category: m.category,
      modelName: m.modelName, source: m.source, secretKey: m.secretKey,
      isStreaming: m.isStreaming, isThinking: m.isThinking,
    });
    setShowForm(true);
  };

  const readyCount = downloadable.filter((m) => m.is_available).length;
  // translate_status 返回 engineName 供 CurrentBanner 显示
  const currentEngineName = status?.engineName ?? "";

  // review fix 问题 3+6：local/cloud is_current 都用 DB is_enabled；local 补 cloudId: m.id
  const localRows: ModelRowData[] = downloadable.map((m) => ({
    name: m.name, provider: "local", category: m.category,
    description: m.description, is_ready: m.is_available,
    is_current: m.is_enabled,
    is_local: true, repo: m.repo,
    cloudId: m.id,
  }));

  const cloudRows: ModelRowData[] = cloudModels.map((m) => ({
    name: m.modelName, provider: m.provider, category: m.category,
    description: "", is_ready: true,
    is_current: m.isEnabled,
    is_local: false, repo: "",
    cloudId: m.id,
  }));

  return (
    <div className="space-y-0.5">
      {currentEngineName && <CurrentBanner label={currentEngineName} />}
      <CollapsibleSection icon={HardDrive} label={t("settings.models.localModels")} count={`${readyCount}/${downloadable.length}`}>
        {localRows.map((m) => (
          <ModelRow key={m.repo} model={m} progress={progress[m.repo]} busy={!!busyRepo}
            onActivate={() => m.cloudId && onActivate(m.cloudId)}
            onDownload={() => onDownload(m.repo)}
            onVerify={() => onVerify(m.repo, m.name)}
            onDelete={() => onDelete(m.repo)}
          />
        ))}
      </CollapsibleSection>

      <CollapsibleSection icon={Cloud} label={t("settings.models.translate.cloud.title")}>
        <div className="flex justify-end pb-1">
          <Button variant="voice-soft" size="sm"
            onClick={() => { setEditTarget(null); setShowForm(true); }}>
            <Plus /> {t("settings.models.addModel")}
          </Button>
        </div>
        {cloudRows.map((m) => (
          <ModelRow key={m.provider + ":" + m.name} model={m} progress={null} busy={false}
            onActivate={() => m.cloudId && onActivate(m.cloudId)}
            onDownload={() => {}}
            onVerify={() => {}}
            onDelete={() => m.cloudId && onDeleteCloud(m.cloudId)}
            onEdit={() => {
              const cm = cloudModels.find((o) => o.id === m.cloudId);
              if (cm) onEditCloud(cm);
            }}
          />
        ))}
        {cloudRows.length === 0 && (
          <div className="text-[11px] text-muted-foreground/50 py-3 text-center">
            {t("settings.models.translate.cloud.empty")}
          </div>
        )}
      </CollapsibleSection>

      {showForm && (
        <CloudModelForm
          domain="translate"
          editModel={editTarget}
          onSaved={() => { setShowForm(false); setEditTarget(null); load(); }}
          onCancel={() => { setShowForm(false); setEditTarget(null); }}
        />
      )}
    </div>
  );
}
