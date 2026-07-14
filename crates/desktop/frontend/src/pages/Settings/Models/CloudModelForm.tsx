import { useState, useEffect } from "react";
import { invoke } from "@/lib/tauri";
import { cn } from "@/lib/utils";
import { X } from "lucide-react";
import { useT } from "@/lib/i18n";

export interface CloudModelData {
  id?: number;
  domain: "asr" | "llm";
  provider: string;
  category: string;
  modelName: string;
  source: string;
  secretKey: string;
  isStreaming: boolean;
  isThinking: boolean;
}

interface AsrPreset {
  provider: string;
  category: string;
  models: string[];
}

interface LlmPreset {
  provider: string;
  baseUrl: string;
}

// ASR provider 固定配置
const ASR_CONFIG: Record<string, Record<string, { source: string; keyLabel: string }>> = {
  aliyun: {
    "Fun-ASR": { source: "wss://dashscope.aliyuncs.com/api-ws/v1/inference", keyLabel: "DashScope API Key" },
    "Paraformer": { source: "wss://dashscope.aliyuncs.com/api-ws/v1/inference", keyLabel: "DashScope API Key" },
    "Qwen-ASR": { source: "wss://dashscope.aliyuncs.com/api-ws/v1/realtime", keyLabel: "DashScope API Key" },
  },
  bytedance: {
    "Doubao-ASR": { source: "volc.bigasr.sauc.duration", keyLabel: "火山引擎 API Key" },
    "Doubao-ASR-2.0": { source: "volc.seedasr.sauc.duration", keyLabel: "火山引擎 API Key" },
  },
  tencent: {
    "Tencent-ASR": { source: "{appid}:{secretid}", keyLabel: "腾讯云 SecretKey" },
    "Tencent-ASR-Multi": { source: "{appid}:{secretid}", keyLabel: "腾讯云 SecretKey" },
  },
  baidu: {
    "Baidu-ASR": { source: "{appid}", keyLabel: "百度 API Key (appkey)" },
    "Baidu-ASR-EN": { source: "{appid}", keyLabel: "百度 API Key (appkey)" },
  },
};

const inputClass = "w-full px-2.5 py-1.5 border border-border rounded-md text-sm bg-background outline-none focus:border-voice/40 transition-colors";
const labelClass = "text-[11px] text-muted-foreground mb-1";

export function CloudModelForm({
  domain,
  editModel,
  onSaved,
  onCancel,
}: {
  domain: "asr" | "llm";
  editModel?: CloudModelData | null;
  onCancel: () => void;
  onSaved: () => void;
}) {
  const t = useT();
  const [asrPresets, setAsrPresets] = useState<AsrPreset[]>([]);
  const [llmPresets, setLlmPresets] = useState<LlmPreset[]>([]);
  const [provider, setProvider] = useState(editModel?.provider ?? "");
  const [category, setCategory] = useState(editModel?.category ?? "");
  const [modelName, setModelName] = useState(editModel?.modelName ?? "");
  const [source, setSource] = useState(editModel?.source ?? "");
  const [secretKey, setSecretKey] = useState(editModel?.secretKey ?? "");
  const [isStreaming, setIsStreaming] = useState(editModel?.isStreaming ?? true);
  const [isThinking, setIsThinking] = useState(editModel?.isThinking ?? false);
  const [saving, setSaving] = useState(false);
  const [testing, setTesting] = useState(false);
  const [testResult, setTestResult] = useState<{ ok: boolean; message: string } | null>(null);

  useEffect(() => {
    if (domain === "asr") {
      invoke<AsrPreset[]>("list_asr_cloud_presets").then(setAsrPresets).catch(() => {});
    } else {
      invoke<LlmPreset[]>("list_llm_provider_presets").then(setLlmPresets).catch(() => {});
    }
  }, [domain]);

  // LLM: provider 变更 → 自动填 base_url
  useEffect(() => {
    if (domain === "llm" && provider) {
      const preset = llmPresets.find((p) => p.provider === provider);
      if (preset && !editModel) {
        setSource(preset.baseUrl);
      }
    }
  }, [provider, domain, llmPresets, editModel]);

  // ASR: provider+category 变更 → 自动填 source
  useEffect(() => {
    if (domain === "asr" && provider && category) {
      const cfg = ASR_CONFIG[provider]?.[category];
      if (cfg && !editModel) {
        setSource(cfg.source);
      }
    }
  }, [provider, category, domain, editModel]);

  const availableCategories = domain === "asr"
    ? asrPresets.filter((p) => p.provider === provider).map((p) => p.category)
    : [];
  const referenceModels = domain === "asr"
    ? asrPresets.find((p) => p.provider === provider && p.category === category)?.models ?? []
    : [];
  const keyLabel = domain === "asr"
    ? ASR_CONFIG[provider]?.[category]?.keyLabel ?? "API Key"
    : "API Key";

  const handleSave = async () => {
    if (!provider || !modelName) return;
    setSaving(true);
    try {
      // 编辑时：如果 secret_key 未改（仍是脱敏值），不传 secret_key（后端保留原值）
      const keyToSend = (editModel && secretKey.includes("********"))
        ? "" : secretKey;
      const input = {
        domain,
        provider,
        category: category || provider,
        modelName,
        source,
        secretKey: keyToSend,
        isStreaming,
        isThinking,
      };
      if (editModel?.id) {
        await invoke("edit_cloud_model", { id: editModel.id, input });
      } else {
        await invoke("add_cloud_model", { input });
      }
      onSaved();
    } catch (e) {
      alert(e);
    } finally {
      setSaving(false);
    }
  };

  const handleTest = async () => {
    if (!source || !secretKey) return;
    setTesting(true);
    setTestResult(null);
    try {
      // 编辑时 secretKey 可能是脱敏值（含 ********），需查 DB 取真实 key
      let realKey = secretKey;
      if (editModel?.id && secretKey.includes("********")) {
        // 从后端取真实 key
        try {
          const result = await invoke<{ source: string; secret_key: string }>("get_model_detail", { id: editModel.id });
          realKey = result.secret_key;
        } catch { /* 取不到就用脱敏值（会失败，用户需重新输入） */ }
      }
      const result = await invoke<{ ok: boolean; message: string }>("test_cloud_model", {
        source, secretKey: realKey,
      });
      setTestResult(result);
    } catch (e) {
      setTestResult({ ok: false, message: String(e) });
    } finally {
      setTesting(false);
    }
  };

  return (
    <div className="fixed inset-0 bg-black/30 flex items-center justify-center z-50" onClick={onCancel}>
      <div
        className="bg-background border border-border rounded-lg p-5 w-[420px] max-w-[90vw] shadow-xl"
        onClick={(e) => e.stopPropagation()}
      >
        <div className="flex items-center justify-between mb-4">
          <h3 className="text-sm font-semibold">
            {editModel ? t("settings.models.editModel") : t("settings.models.addModel")}
          </h3>
          <button onClick={onCancel} className="text-muted-foreground hover:text-foreground">
            <X className="w-4 h-4" />
          </button>
        </div>

        <div className="space-y-3">
          {/* Provider */}
          <div>
            <div className={labelClass}>Provider</div>
            {domain === "llm" ? (
              <select className={inputClass} value={provider}
                onChange={(e) => setProvider(e.target.value)}>
                <option value="">-- 选择 --</option>
                {llmPresets.map((p) => (
                  <option key={p.provider} value={p.provider}>{p.provider}</option>
                ))}
                <option value="custom">自定义</option>
              </select>
            ) : (
              <select className={inputClass} value={provider}
                onChange={(e) => { setProvider(e.target.value); setCategory(""); }}>
                <option value="">-- 选择 --</option>
                {Object.keys(ASR_CONFIG).map((p) => (
                  <option key={p} value={p}>{p}</option>
                ))}
              </select>
            )}
          </div>

          {/* Category (ASR only, LLM auto from provider) */}
          {domain === "asr" && availableCategories.length > 0 && (
            <div>
              <div className={labelClass}>Category</div>
              <select className={inputClass} value={category}
                onChange={(e) => setCategory(e.target.value)}>
                <option value="">-- 选择 --</option>
                {availableCategories.map((c) => (
                  <option key={c} value={c}>{c}</option>
                ))}
              </select>
            </div>
          )}

          {/* Source / Base URL */}
          <div>
            <div className={labelClass}>{domain === "llm" ? "Base URL" : "Source"}</div>
            <input className={inputClass} value={source}
              onChange={(e) => setSource(e.target.value)}
              placeholder={domain === "llm" ? "https://api.example.com/v1" : ""} />
          </div>

          {/* Model Name */}
          <div>
            <div className={labelClass}>Model Name</div>
            {domain === "asr" && referenceModels.length > 0 ? (
              <input className={inputClass} value={modelName} list="ref-models"
                onChange={(e) => setModelName(e.target.value)} placeholder="选择或输入" />
            ) : (
              <input className={inputClass} value={modelName}
                onChange={(e) => setModelName(e.target.value)} placeholder="如 deepseek-chat" />
            )}
            {domain === "asr" && referenceModels.length > 0 && (
              <datalist id="ref-models">
                {referenceModels.map((m) => <option key={m} value={m} />)}
              </datalist>
            )}
          </div>

          {/* API Key */}
          <div>
            <div className={labelClass}>{keyLabel}</div>
            <input className={inputClass} type="password" value={secretKey}
              onChange={(e) => setSecretKey(e.target.value)} placeholder="sk-..." />
          </div>

          {/* Checkboxes */}
          <div className="flex items-center gap-4 pt-1">
            <label className="flex items-center gap-1.5 text-xs cursor-pointer">
              <input type="checkbox" checked={isStreaming}
                onChange={(e) => setIsStreaming(e.target.checked)} />
              Streaming
            </label>
            {domain === "llm" && (
              <label className="flex items-center gap-1.5 text-xs cursor-pointer">
                <input type="checkbox" checked={isThinking}
                  onChange={(e) => setIsThinking(e.target.checked)} />
                Thinking
              </label>
            )}
          </div>
        </div>

        {/* Test result */}
        {testResult && (
          <div className={cn(
            "text-[11px] px-2.5 py-1.5 rounded-md",
            testResult.ok ? "bg-emerald-500/10 text-emerald-600" : "bg-destructive/10 text-destructive",
          )}>
            {testResult.ok ? "✓ " : "✗ "}{testResult.message}
          </div>
        )}

        {/* Actions */}
        <div className="flex justify-between items-center gap-2 mt-5">
          {/* 测试连接（仅 LLM——ASR 的 source 不是 HTTP base_url） */}
          {domain === "llm" ? (
            <button className={cn(
              "px-3 py-1.5 text-xs rounded-md border transition-colors",
              (!source || !secretKey || testing)
                ? "border-border text-muted-foreground/40 cursor-not-allowed"
                : "border-voice/40 text-voice hover:bg-voice/10",
            )}
            disabled={!source || !secretKey || testing}
            onClick={handleTest}>
              {testing ? "..." : t("settings.models.testConnection")}
            </button>
          ) : <div />}

          <div className="flex gap-2">
            <button className="px-3 py-1.5 text-xs rounded-md border border-border text-muted-foreground hover:bg-accent transition-colors"
              onClick={onCancel}>
              {t("settings.models.cancel")}
            </button>
            <button className={cn(
              "px-3 py-1.5 text-xs rounded-md transition-colors",
              (!provider || !modelName || saving)
                ? "bg-muted text-muted-foreground cursor-not-allowed"
                : "bg-foreground text-background hover:opacity-85",
            )}
            disabled={!provider || !modelName || saving}
            onClick={handleSave}>
              {saving ? "..." : t("settings.models.save")}
            </button>
          </div>
        </div>
      </div>
    </div>
  );
}
