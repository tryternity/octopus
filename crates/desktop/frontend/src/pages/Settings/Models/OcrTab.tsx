import { useState, useEffect, useCallback } from "react";
import { invoke } from "@/lib/tauri";
import { cn } from "@/lib/utils";
import { CheckCircle2, Cloud, HardDrive } from "lucide-react";

interface OcrOption {
  id: number;
  name: string;
  label: string;
  current: boolean;
  is_enabled: boolean;
  is_local: boolean;
}

function SectionHeader({ icon: Icon, label }: { icon: React.ElementType; label: string }) {
  return (
    <div className="flex items-center gap-1.5 pt-3 pb-1 first:pt-0">
      <Icon className="w-3 h-3 text-muted-foreground/60" />
      <span className="text-[11px] font-medium text-muted-foreground">{label}</span>
    </div>
  );
}

function CurrentBanner({ label }: { label: string }) {
  return (
    <div className="flex items-center gap-1.5 px-3 py-1.5 rounded-md border-l-2 border-voice bg-voice/5 text-[11px] mb-1">
      <CheckCircle2 className="w-3 h-3 text-voice shrink-0" />
      <span className="text-muted-foreground">当前使用</span>
      <span className="font-medium text-foreground">{label}</span>
    </div>
  );
}

export default function OcrTab({ showToast }: { showToast: (msg: string) => void }) {
  const [models, setModels] = useState<OcrOption[]>([]);

  const load = useCallback(async () => {
    try {
      const resp = await invoke<{ ocr_models: OcrOption[] }>("get_config");
      setModels(resp.ocr_models ?? []);
    } catch (e) { showToast("加载模型列表失败：" + e); }
  }, [showToast]);

  useEffect(() => { load(); }, [load]);

  const handleToggle = async (model: OcrOption) => {
    try {
      await invoke("set_model_enabled", { id: model.id, enabled: !model.is_enabled });
      load();
    } catch (e) { showToast("切换失败：" + e); }
  };

  const current = models.find((m) => m.current);
  const localModels = models.filter((m) => m.is_local);
  const cloudModels = models.filter((m) => !m.is_local);

  const renderModel = (model: OcrOption) => (
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
        {model.is_enabled ? "启用" : "禁用"}
      </button>
    </div>
  );

  return (
    <div className="space-y-0.5 max-w-[560px]">
      {current && <CurrentBanner label={current.label} />}
      {localModels.length > 0 && <SectionHeader icon={HardDrive} label="本地模型" />}
      {localModels.map(renderModel)}
      {cloudModels.length > 0 && <SectionHeader icon={Cloud} label="云端模型" />}
      {cloudModels.map(renderModel)}
    </div>
  );
}
