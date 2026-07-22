import { useState, useEffect, useCallback } from "react";
import { invoke } from "@/lib/tauri";
import { Cloud, HardDrive, Plus } from "lucide-react";
import { CollapsibleSection } from "./CollapsibleSection";
import { ModelRow, CurrentBanner, type ModelRowData } from "./ModelRow";
import { CloudModelForm, type CloudModelData } from "./CloudModelForm";
import { Button } from "@/components/ui/button";
import { useT } from "@/lib/i18n";

interface LlmOption {
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

export default function LlmTab({ showToast }: { showToast: (msg: string) => void }) {
  const t = useT();
  const [models, setModels] = useState<LlmOption[]>([]);
  const [showForm, setShowForm] = useState(false);
  const [editTarget, setEditTarget] = useState<CloudModelData | null>(null);

  const load = useCallback(async () => {
    try {
      const resp = await invoke<{ llm_models: LlmOption[] }>("get_config");
      setModels(resp.llm_models ?? []);
    } catch (e) { showToast(t("settings.models.loadFailed") + e); }
  }, [showToast, t]);

  useEffect(() => { load(); }, [load]);

  // Task 2 后：统一走 switch_active_model(domain, id)。空 id 取消激活（LLM 域允许无激活）。
  const onActivate = async (id: number | null) => {
    try {
      // id=null 表示「不选择模型」——传 -1 让后端 switch_active_model 用 IIF(id=-1,1,0) 清空
      const targetId = id ?? -1;
      await invoke("switch_active_model", { domain: "llm", id: targetId });
      load();
    }
    catch (e) { showToast(t("settings.models.switchFailed") + e); }
  };

  const onDelete = async (id: number) => {
    try { await invoke("remove_cloud_model", { id }); load(); }
    catch (e) { showToast(String(e)); }
  };

  const onEdit = (m: LlmOption) => {
    setEditTarget({
      id: m.id, domain: "llm", provider: m.provider, category: m.category,
      modelName: m.name, source: m.source, secretKey: m.secret_key,
      isStreaming: m.is_streaming, isThinking: m.is_thinking,
    });
    setShowForm(true);
  };

  const current = models.find((m) => m.current && m.name);
  const currentLabel = current?.label ?? "";

  const rows: ModelRowData[] = models
    .filter((m) => m.name)
    .map((m) => ({
      name: m.name, provider: m.provider, category: m.category,
      description: m.label, is_ready: true,
      is_current: m.current, source_type: m.source_type, repo: "",
      cloudId: m.id,
    }));

  const localRows = rows.filter((r) => r.source_type !== 2);
  const cloudRows = rows.filter((r) => r.source_type === 2);

  return (
    <div className="space-y-0.5">
      {currentLabel && <CurrentBanner label={currentLabel} />}
      {/* review fix 问题 4：「不选择模型」入口——用户可关闭 polish（取消 LLM 域激活） */}
      {models.some((m) => m.current && m.name) && (
        <div className="flex justify-end pb-0.5">
          <Button variant="voice-soft" size="sm"
            onClick={() => onActivate(null)}>
            {t("settings.models.deselectModel")}
          </Button>
        </div>
      )}

      {localRows.length > 0 && (
        <CollapsibleSection icon={HardDrive} label={t("settings.models.localModels")}>
          {localRows.map((m) => (
            <ModelRow key={m.provider + ":" + m.name} model={m} progress={null} busy={false}
              onActivate={() => m.cloudId && onActivate(m.cloudId)} onDownload={() => {}} onVerify={() => {}} onDelete={() => {}}
            />
          ))}
        </CollapsibleSection>
      )}

      <CollapsibleSection icon={Cloud} label={t("settings.models.cloudModels")}
        action={
          <Button variant="voice-soft" size="sm"
            onClick={() => { setEditTarget(null); setShowForm(true); }}>
            <Plus /> {t("settings.models.addModel")}
          </Button>
        }
      >
        {cloudRows.map((m) => (
          <ModelRow key={m.provider + ":" + m.name} model={m} progress={null} busy={false}
            onActivate={() => m.cloudId && onActivate(m.cloudId)}
            onDownload={() => {}}
            onVerify={() => {}}
            onDelete={() => m.cloudId && onDelete(m.cloudId)}
            onEdit={() => {
              const opt = models.find((o) => o.id === m.cloudId);
              if (opt) onEdit(opt);
            }}
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
          domain="llm"
          editModel={editTarget}
          onSaved={() => { setShowForm(false); setEditTarget(null); load(); }}
          onCancel={() => { setShowForm(false); setEditTarget(null); }}
        />
      )}
    </div>
  );
}
