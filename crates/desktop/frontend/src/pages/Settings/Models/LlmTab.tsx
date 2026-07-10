import { useState, useEffect, useCallback } from "react";
import { invoke } from "@/lib/tauri";
import { CheckCircle2, Cloud, HardDrive } from "lucide-react";

interface LlmOption {
  id: number;
  name: string;
  label: string;
  current: boolean;
  is_enabled: boolean;
  is_local: boolean;
}

export default function LlmTab({ showToast }: { showToast: (msg: string) => void }) {
  const [models, setModels] = useState<LlmOption[]>([]);

  const load = useCallback(async () => {
    try {
      const resp = await invoke<{ llm_models: LlmOption[] }>("get_config");
      setModels(resp.llm_models ?? []);
    } catch (e) { showToast("加载模型列表失败：" + e); }
  }, [showToast]);

  useEffect(() => { load(); }, [load]);

  const handleToggle = async (model: LlmOption) => {
    try {
      await invoke("set_model_enabled", { id: model.id, enabled: !model.is_enabled });
      load();
    } catch (e) { showToast("切换失败：" + e); }
  };

  const current = models.find((m) => m.current);
  const localModels = models.filter((m) => m.is_local);
  const cloudModels = models.filter((m) => !m.is_local);

  const renderModel = (model: LlmOption) => (
    <div key={model.id} className="flex items-center justify-between py-2 border-b border-border/40 last:border-0">
      <div className="flex items-center gap-1.5">
        <span className="text-sm font-medium">{model.name}</span>
        {model.current && <CheckCircle2 className="w-3.5 h-3.5 text-voice" />}
      </div>
      <button
        className={`px-2.5 py-1 text-xs rounded transition-colors ${
          model.is_enabled
            ? "bg-voice/10 text-voice hover:bg-voice/20"
            : "bg-muted text-muted-foreground hover:bg-muted/80"
        }`}
        onClick={() => handleToggle(model)}
      >
        {model.is_enabled ? "已启用" : "已禁用"}
      </button>
    </div>
  );

  return (
    <div className="space-y-2">
      {current && (
        <div className="flex items-center gap-1.5 px-3 py-1.5 rounded bg-voice/8 text-xs">
          <CheckCircle2 className="w-3 h-3 text-voice" />
          当前使用：<span className="font-medium">{current.label}</span>
        </div>
      )}
      {localModels.length > 0 && (
        <>
          <div className="flex items-center gap-1.5 pt-1 pb-1">
            <HardDrive className="w-3.5 h-3.5 text-muted-foreground" />
            <span className="text-xs font-medium text-muted-foreground">本地模型</span>
          </div>
          {localModels.map(renderModel)}
        </>
      )}
      {cloudModels.length > 0 && (
        <>
          <div className="flex items-center gap-1.5 pt-3 pb-1">
            <Cloud className="w-3.5 h-3.5 text-muted-foreground" />
            <span className="text-xs font-medium text-muted-foreground">云端模型</span>
          </div>
          {cloudModels.map(renderModel)}
        </>
      )}
    </div>
  );
}
