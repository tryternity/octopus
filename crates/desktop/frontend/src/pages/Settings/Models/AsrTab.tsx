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
  source_type: number;
  label: string;
  source: string;
  secret_key: string;
  is_streaming: boolean;
  is_thinking: boolean;
}

interface DownloadableModel {
  id: number;
  name: string;
  repo: string;
  description: string;
  category: string;
  // Task 2 后：is_available=就绪（文件完备）；is_enabled=激活（每域仅 1 个=1）
  is_available: boolean;
  is_enabled: boolean;
  source_type: number;
}

/** verify_model 返回的校验结果。ok=false 时 broken_files 含损坏/缺失文件列表。 */
interface VerifyResult {
  ok: boolean;
  bootstrapped: boolean;
  broken_files: string[];
  message: string;
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

  // Task 2 后：currentLabel 仍从 get_config 的 asr_engines（带 current 标记）取。
  const currentLabel = engines.find((e) => e.current)?.label ?? "";

  // Task 2 后：统一走 switch_active_model(domain, id)。本地模型 id 来自 DownloadableModel，
  // 云端模型 id 来自 EngineOption。本地模型激活前先校验完整性——损坏/缺失自动下载修复。
  const onActivate = async (id: number, repo?: string, name?: string) => {
    // 云端模型（无 repo/name）直接激活
    if (!repo || !name) {
      invoke("switch_active_model", { domain: "asr", id }).then(load).catch((e) => showToast(t("settings.models.switchFailed") + e));
      return;
    }
    if (busyRepo) return;
    setBusyRepo(repo);
    try {
      // 1. 校验文件完整性（stat 快检，不强制 SHA256）
      const result = await invoke<VerifyResult>("verify_model", { repo, modelName: name, full: false });
      if (!result.ok) {
        // 2. 损坏/缺失 → 自动下载修复。下载失败则不激活（return 跳出）
        showToast(result.message || t("settings.models.verifyFailed"));
        try {
          await invoke("download_model", { repo });
        } catch (e) {
          showToast(t("settings.models.downloadStartFailed") + e);
          return; // 下载失败，不激活
        }
        setProgress((prev) => { const next = { ...prev }; delete next[repo]; return next; });
      }
      // 3. 校验通过（或下载修复成功）→ 激活
      invoke("switch_active_model", { domain: "asr", id }).then(load).catch((e) => showToast(t("settings.models.switchFailed") + e));
    } catch (e) {
      showToast(t("settings.models.verifyFailed") + e);
    } finally {
      setBusyRepo(null);
    }
  };

  const onDownload = (repo: string) => {
    if (busyRepo) return;
    onDownloadInternal(repo);
  };

  const onVerify = async (repo: string, name: string) => {
    if (busyRepo) return;
    setBusyRepo(repo);
    try {
      const result = await invoke<VerifyResult>("verify_model", { repo, modelName: name, full: true });
      if (result.ok) {
        showToast(result.message || t("settings.models.verifyComplete"));
        load();
      } else {
        // 校验失败（文件损坏/缺失）→ 自动重新下载修复
        showToast(result.message || t("settings.models.verifyFailed"));
        setBusyRepo(null); // 清校验 busy，让 onDownload 能设下载 busy
        await onDownloadInternal(repo);
      }
    }
    catch (e) { showToast(t("settings.models.verifyFailed") + e); }
    finally { setBusyRepo(null); }
  };

  /// 下载内部实现（onVerify 校验失败时复用，避免 busyRepo 互斥）。
  /// return invoke 让 caller 可 await（onVerify 的 finally 不会提前清 busyRepo）。
  const onDownloadInternal = async (repo: string) => {
    setBusyRepo(repo);
    return invoke("download_model", { repo })
      .then(() => {
        setBusyRepo(null);
        setProgress((prev) => { const next = { ...prev }; delete next[repo]; return next; });
        showToast(t("settings.models.downloadComplete"));
        load();
      })
      .catch((e) => {
        setBusyRepo(null);
        setProgress((prev) => { const next = { ...prev }; delete next[repo]; return next; });
        showToast(t("settings.models.downloadStartFailed") + e);
      });
  };

  const onDelete = (repo: string) => {
    invoke("delete_model", { repo }).then(load).catch((e) => showToast(e));
  };

  const readyCount = downloadable.filter((m) => m.is_available).length;

  const [showForm, setShowForm] = useState(false);
  const [editTarget, setEditTarget] = useState<CloudModelData | null>(null);

  // 合并本地 + 云端。Task 2 后：is_ready 用 is_available；is_current 用 is_enabled（激活）。
  const localRows: ModelRowData[] = downloadable.map((m) => ({
    name: m.name, provider: "local", category: m.category,
    description: m.description, is_ready: m.is_available,
    is_current: m.is_enabled, source_type: m.source_type, repo: m.repo,
    cloudId: m.id,
  }));

  const cloudEngines = engines.filter((e) => e.source_type === 2);
  const cloudRows: ModelRowData[] = cloudEngines.map((e) => ({
    name: e.name, provider: e.provider, category: e.category,
    description: e.label, is_ready: true,
    is_current: e.current, source_type: 2, repo: "",
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
          <ModelRow key={m.repo} model={m} progress={progress[m.repo]} busy={busyRepo === m.repo}
            onActivate={() => m.cloudId && onActivate(m.cloudId, m.repo, m.name)}
            onDownload={() => onDownload(m.repo)}
            onVerify={() => onVerify(m.repo, m.name)}
            onDelete={() => onDelete(m.repo)}
          />
        ))}
      </CollapsibleSection>

      <CollapsibleSection icon={Cloud} label={t("settings.models.cloudEngines")}
        action={
          <Button variant="voice-soft" size="sm"
            onClick={() => { setEditTarget(null); setShowForm(true); }}>
            <Plus /> {t("settings.models.addModel")}
          </Button>
        }
      >
        {cloudRows.map((m) => {
          const engine = cloudEngines.find((e) => e.id === m.cloudId);
          return (
          <ModelRow key={m.provider + ":" + m.name} model={m} progress={null} busy={busyRepo === m.repo}
            onActivate={() => m.cloudId && onActivate(m.cloudId)}
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
