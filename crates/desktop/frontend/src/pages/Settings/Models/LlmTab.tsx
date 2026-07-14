import { useState, useEffect, useCallback } from "react";
import { invoke } from "@/lib/tauri";
import { cn } from "@/lib/utils";
import { CheckCircle2, Cloud, HardDrive } from "lucide-react";
import { CollapsibleSection } from "./CollapsibleSection";
import { useT } from "@/lib/i18n";

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
  }, [showToast, t]);

  useEffect(() => { load(); }, [load]);

  const handleActivate = async (model: LlmOption) => {
    if (model.current) return;
    try { await invoke("switch_polish_llm", { modelName: model.name }); load(); }
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
        model.current ? "border-l-voice/40 bg-voice/5" : "border-l-border/40",
      )}
    >
      <div className="flex items-center gap-1.5">
        <span className={cn("text-xs font-medium", !model.is_enabled && "text-muted-foreground")}>{model.name}</span>
        {model.current && <CheckCircle2 className="w-3 h-3 text-voice" />}
      </div>
      <button
        className={cn(
          "px-2 py-0.5 text-[11px] rounded transition-colors",
          model.current
            ? "bg-muted text-muted-foreground/40 cursor-default"
            : "bg-voice/10 text-voice hover:bg-voice/20",
        )}
        disabled={model.current}
        onClick={() => handleActivate(model)}
      >
        {model.current ? t("settings.models.activated") : t("settings.models.activate")}
      </button>
    </div>
  );

  return (
    <div className="space-y-0.5 max-w-[560px]">
      {current && <CurrentBanner label={current.label} />}
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
