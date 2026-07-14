import { useState, useEffect, useCallback } from "react";
import { invoke } from "@/lib/tauri";
import { cn } from "@/lib/utils";
import { CheckCircle2, Cloud, HardDrive } from "lucide-react";
import { CollapsibleSection } from "./CollapsibleSection";
import { useT } from "@/lib/i18n";

const selectClass = "px-2.5 py-1.5 border border-border rounded-md text-sm bg-background min-w-[160px] max-w-[220px] cursor-pointer hover:border-foreground/30 transition-colors outline-none focus:border-voice/40";

interface LlmOption {
  id: number;
  name: string;
  label: string;
  current: boolean;
  is_enabled: boolean;
  is_local: boolean;
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

export default function LlmTab({ showToast }: { showToast: (msg: string) => void }) {
  const t = useT();
  const [models, setModels] = useState<LlmOption[]>([]);

  const load = useCallback(async () => {
    try {
      const resp = await invoke<{ llm_models: LlmOption[] }>("get_config");
      setModels(resp.llm_models ?? []);
    } catch (e) { showToast(t("settings.models.loadFailed") + e); }
  }, [showToast]);

  useEffect(() => { load(); }, [load]);

  const handleToggle = async (model: LlmOption) => {
    try {
      await invoke("set_model_enabled", { id: model.id, enabled: !model.is_enabled });
      load();
    } catch (e) { showToast(t("settings.models.switchFailed") + e); }
  };

  const handleSwitchPolish = async (name: string) => {
    try { await invoke("switch_polish_llm", { modelName: name }); load(); }
    catch (e) { showToast(t("settings.models.switchFailed") + e); }
  };

  const current = models.find((m) => m.current);
  const localModels = models.filter((m) => m.is_local);
  const cloudModels = models.filter((m) => !m.is_local);

  const renderModel = (model: LlmOption) => (
    <div
      key={model.id}
      className={cn(
        "group flex items-center justify-between py-2 px-3 rounded-md transition-colors",
        "border-l-2 hover:bg-accent/30",
        model.is_enabled ? "border-l-voice/40" : "border-l-border/40",
      )}
    >
      <div className="flex items-center gap-1.5">
        <span className={cn("text-xs font-medium", !model.is_enabled && "text-muted-foreground")}>{model.name}</span>
        {model.current && <CheckCircle2 className="w-3 h-3 text-voice" />}
      </div>
      <button
        className={cn(
          "px-2 py-0.5 text-[11px] rounded transition-colors",
          model.is_enabled
            ? "bg-voice/10 text-voice hover:bg-voice/20"
            : "bg-muted text-muted-foreground hover:bg-accent",
        )}
        onClick={() => handleToggle(model)}
      >
        {model.is_enabled ? t("settings.models.enable") : t("settings.models.disable")}
      </button>
    </div>
  );

  return (
    <div className="space-y-0.5 max-w-[560px]">
      {current && <CurrentBanner label={current.label} />}

      <div className="flex items-center justify-between py-2 px-3 rounded-md border border-border/60 bg-surface">
        <span className="text-xs text-muted-foreground">{t("settings.general.polishModel")}</span>
        <select className={selectClass}
          value={models.find((m) => m.current)?.name ?? ""}
          onChange={(e) => handleSwitchPolish(e.target.value)}>
          {models.map((m) => <option key={m.name} value={m.name}>{m.label}</option>)}
        </select>
      </div>

      {localModels.length > 0 && (
        <CollapsibleSection icon={HardDrive} label={t("settings.models.localModels")}>
          {localModels.map(renderModel)}
        </CollapsibleSection>
      )}
      {cloudModels.length > 0 && (
        <CollapsibleSection icon={Cloud} label={t("settings.models.cloudModels")}>
          {cloudModels.map(renderModel)}
        </CollapsibleSection>
      )}
    </div>
  );
}
