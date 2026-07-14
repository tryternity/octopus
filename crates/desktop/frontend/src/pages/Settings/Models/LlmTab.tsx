import { useState, useEffect, useCallback } from "react";
import { invoke } from "@/lib/tauri";
import { Cloud, HardDrive } from "lucide-react";
import { CollapsibleSection } from "./CollapsibleSection";
import { ModelRow, CurrentBanner, type ModelRowData } from "./ModelRow";
import { useT } from "@/lib/i18n";

interface LlmOption {
  name: string;
  provider: string;
  category: string;
  current: boolean;
  is_local: boolean;
  label: string;
}

export default function LlmTab({ showToast }: { showToast: (msg: string) => void }) {
  const t = useT();
  const [models, setModels] = useState<LlmOption[]>([]);

  const load = useCallback(async () => {
    try {
      const resp = await invoke<{ llm_models: LlmOption[] }>("get_config");
      setModels(resp.llm_models ?? []);
    } catch (e) { showToast(t("settings.models.loadFailed") + e); }
  }, [showToast, t]);

  useEffect(() => { load(); }, [load]);

  const onActivate = async (name: string) => {
    try { await invoke("switch_polish_llm", { modelName: name }); load(); }
    catch (e) { showToast(t("settings.models.switchFailed") + e); }
  };

  const currentLabel = models.find((m) => m.current && m.name)?.label ?? "";
  const rows: ModelRowData[] = models
    .filter((m) => m.name)
    .map((m) => ({
      name: m.name, provider: m.provider, category: m.category,
      description: m.label, is_ready: true,
      is_current: m.current, is_local: m.is_local, repo: "",
    }));

  const localRows = rows.filter((r) => r.is_local);
  const cloudRows = rows.filter((r) => !r.is_local);

  return (
    <div className="space-y-0.5 max-w-[560px]">
      {currentLabel && <CurrentBanner label={currentLabel} />}
      {localRows.length > 0 && (
        <CollapsibleSection icon={HardDrive} label={t("settings.models.localModels")}>
          {localRows.map((m) => (
            <ModelRow key={m.provider + ":" + m.name} model={m} progress={null} busy={false}
              onActivate={() => onActivate(m.name)} onDownload={() => {}} onVerify={() => {}} onDelete={() => {}}
            />
          ))}
        </CollapsibleSection>
      )}
      {cloudRows.length > 0 && (
        <CollapsibleSection icon={Cloud} label={t("settings.models.cloudModels")}>
          {cloudRows.map((m) => (
            <ModelRow key={m.provider + ":" + m.name} model={m} progress={null} busy={false}
              onActivate={() => onActivate(m.name)} onDownload={() => {}} onVerify={() => {}} onDelete={() => {}}
            />
          ))}
        </CollapsibleSection>
      )}
    </div>
  );
}
