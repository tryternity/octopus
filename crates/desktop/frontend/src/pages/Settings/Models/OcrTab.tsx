import { useState, useEffect, useCallback, useRef } from "react";
import { invoke } from "@/lib/tauri";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { HardDrive } from "lucide-react";
import { CollapsibleSection } from "./CollapsibleSection";
import { ModelRow, CurrentBanner, type ModelRowData } from "./ModelRow";
import { useT } from "@/lib/i18n";

interface DownloadableModel {
  id: number;
  name: string;
  repo: string;
  description: string;
  category: string;
  // Task 2 后：isAvailable=就绪；isEnabled=激活
  isAvailable: boolean;
  isEnabled: boolean;
  sourceType: number;
}

interface OcrOption {
  name: string;
  provider: string;
  label: string;
  current: boolean;
  sourceType: number;
}

interface DownloadProgress {
  repo: string;
  downloaded: number;
  total: number;
}

export default function OcrTab({ showToast }: { showToast: (msg: string) => void }) {
  const t = useT();
  const [downloadable, setDownloadable] = useState<DownloadableModel[]>([]);
  const [ocrModels, setOcrModels] = useState<OcrOption[]>([]);
  const [progress, setProgress] = useState<Record<string, DownloadProgress>>({});
  const [busyRepo, setBusyRepo] = useState<string | null>(null);
  const [activePopoverRepo, setActivePopoverRepo] = useState<string | null>(null);

  const load = useCallback(async () => {
    try {
      const [resp, dl] = await Promise.all([
        invoke<{ ocrModels: OcrOption[] }>("get_config"),
        invoke<DownloadableModel[]>("list_downloadable_models", { domain: "ocr" }),
      ]);
      setOcrModels(resp.ocrModels ?? []);
      setDownloadable(dl);
    } catch (e) { showToast(t("settings.models.loadFailed") + e); }
  }, [showToast, t]);

  // listener 稳定化 ref（effect 依赖 []，回调读 ref 拿最新——AGENTS.md gotcha）
  const loadRef = useRef(load); loadRef.current = load;
  const tRef = useRef(t); tRef.current = t;
  const showToastRef = useRef(showToast); showToastRef.current = showToast;

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
          if (data.error) showToastRef.current(tRef.current("settings.models.downloadFailed") + data.error);
          else if (data.already_ready) showToastRef.current(tRef.current("settings.models.alreadyReady"));
          else showToastRef.current(tRef.current("settings.models.downloadComplete"));
          loadRef.current();
        }],
      ];
      for (const [event, handler] of subs) {
        const fn = await listen(event, (e) => handler(e.payload));
        if (cancelled) { fn(); return; }
        unlistens.push(fn);
      }
    })();
    return () => { cancelled = true; unlistens.forEach((fn) => fn()); };
  }, []);

  // Task 2 后：currentLabel 仍从 get_config 的 ocrModels（带 current 标记）取。
  const currentLabel = ocrModels.find((m) => m.current)?.label ?? "";
  const readyCount = downloadable.filter((m) => m.isAvailable).length;

  // Task 2 后：统一走 switch_active_model(domain, id)。
  const onActivate = async (id: number) => {
    try {
      await invoke("switch_active_model", { domain: "ocr", id });
      showToast(t("settings.models.ocrRestartHint"));
      load();
    } catch (e) { showToast(t("settings.models.switchFailed") + e); }
  };

  const onDownload = (repo: string) => {
    if (busyRepo) return;
    setBusyRepo(repo);
    invoke("download_model", { repo })
      .then(() => {
        setBusyRepo(null);
        setProgress((prev) => { const next = { ...prev }; delete next[repo]; return next; });
        load();
      })
      .catch((e) => {
        setBusyRepo(null);
        setProgress((prev) => { const next = { ...prev }; delete next[repo]; return next; });
        showToast(t("settings.models.downloadStartFailed") + e);
      });
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

  // Task 2 后：isReady 用 isAvailable；isCurrent 用 isEnabled（激活）。
  const rows: ModelRowData[] = downloadable.map((m) => ({
    name: m.name, provider: "local", category: m.category,
    description: m.description, isReady: m.isAvailable,
    isCurrent: m.isEnabled, sourceType: m.sourceType, repo: m.repo,
    cloudId: m.id,
  }));

  return (
    <div className="space-y-0.5">
      {currentLabel && <CurrentBanner label={currentLabel} />}
      <CollapsibleSection icon={HardDrive} label={t("settings.models.localModels")} count={`${readyCount}/${downloadable.length}`}>
        {rows.map((m) => (
          <ModelRow key={m.repo} model={m} progress={progress[m.repo]} busy={busyRepo === m.repo}
            popoverOpen={activePopoverRepo === m.repo}
            onPopoverOpenChange={(open) => setActivePopoverRepo(open ? m.repo : null)}
            onActivate={() => m.cloudId && onActivate(m.cloudId)}
            onDownload={() => onDownload(m.repo)}
            onVerify={() => onVerify(m.repo, m.name)}
            onDelete={() => onDelete(m.repo)}
          />
        ))}
      </CollapsibleSection>
    </div>
  );
}
