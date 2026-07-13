import { useState, useEffect } from "react";
import { invoke } from "@tauri-apps/api/core";
import { cn } from "@/lib/utils";
import { useT } from "@/lib/i18n";
import { Bot, Plus, RefreshCw, Check, X } from "lucide-react";

interface AgentAdapter {
  key: string;
  displayName: string;
  detectBinary: string;
  commandTemplate: string;
  isBuiltin: boolean;
  isAvailable: boolean;
}

interface AgentPanelProps {
  showToast: (msg: string) => void;
}

export default function AgentPanel({ showToast }: AgentPanelProps) {
  const t = useT();
  const [adapters, setAdapters] = useState<AgentAdapter[]>([]);
  const [editing, setEditing] = useState<Partial<AgentAdapter> | null>(null);
  const [editId, setEditId] = useState<number | null>(null);

  const refresh = () => {
    invoke<AgentAdapter[]>("list_agent_adapters").then(setAdapters).catch(() => {});
  };

  useEffect(refresh, []);

  const handleRefresh = async () => {
    await invoke<AgentAdapter[]>("refresh_agent_detection");
    refresh();
    showToast(t("agentPanel.refreshed"));
  };

  const handleSave = async () => {
    if (!editing) return;
    try {
      if (editId !== null) {
        await invoke("update_agent_adapter", {
          id: editId,
          key: editing.key || "",
          displayName: editing.displayName || "",
          detectBinary: editing.detectBinary || "",
          commandTemplate: editing.commandTemplate || "",
        });
      } else {
        await invoke("create_agent_adapter", {
          key: editing.key || "",
          displayName: editing.displayName || "",
          detectBinary: editing.detectBinary || "",
          commandTemplate: editing.commandTemplate || "",
        });
      }
      setEditing(null);
      setEditId(null);
      refresh();
      showToast(t("agentPanel.saved"));
    } catch (e) {
      showToast(t("agentPanel.saveFailed") + String(e));
    }
  };

  const startCreate = () => {
    setEditing({
      key: "",
      displayName: "",
      detectBinary: "",
      commandTemplate: "{prompt}",
    });
    setEditId(null);
  };

  const inputClass = "w-full bg-background border border-border rounded-md px-3 py-2 text-sm outline-none focus:border-voice/50 focus:ring-1 focus:ring-voice/20 transition-all";

  return (
    <div className="h-full overflow-y-auto p-6 space-y-6">
      <div className="flex items-center justify-between">
        <div className="flex items-center gap-2">
          <Bot className="w-5 h-5 text-voice" />
          <h2 className="text-lg font-semibold">{t("agentPanel.title")}</h2>
        </div>
        <div className="flex items-center gap-2">
          <button
            onClick={handleRefresh}
            className="flex items-center gap-1.5 text-xs text-muted-foreground hover:text-foreground transition-colors"
          >
            <RefreshCw className="w-3.5 h-3.5" />
            {t("agentPanel.refresh")}
          </button>
          <button
            onClick={startCreate}
            className="flex items-center gap-1.5 text-xs text-voice hover:underline"
          >
            <Plus className="w-3.5 h-3.5" />
            {t("agentPanel.addNew")}
          </button>
        </div>
      </div>

      {/* Adapter 列表 */}
      <div className="space-y-2">
        {adapters.map((a) => (
          <div
            key={a.key}
            className={cn(
              "flex items-center justify-between rounded-lg border p-4",
              a.isAvailable ? "border-voice/30 bg-voice/5" : "border-border",
            )}
          >
            <div className="flex-1 min-w-0">
              <div className="flex items-center gap-2">
                <span className="font-medium text-sm">{a.displayName}</span>
                {a.isBuiltin && (
                  <span className="text-[10px] px-1.5 py-0.5 rounded bg-muted text-muted-foreground">
                    {t("agentPanel.builtin")}
                  </span>
                )}
              </div>
              <div className="text-xs text-muted-foreground font-mono mt-1">
                {a.detectBinary}
              </div>
              <div className="text-xs text-muted-foreground font-mono mt-0.5 truncate">
                {a.commandTemplate}
              </div>
            </div>
            <div className="flex items-center gap-2 ml-3">
              {a.isAvailable ? (
                <span className="text-xs text-emerald-500 whitespace-nowrap">
                  {t("agentPanel.installed")}
                </span>
              ) : (
                <span className="text-xs text-muted-foreground whitespace-nowrap">
                  {t("agentPanel.notFound")}
                </span>
              )}
            </div>
          </div>
        ))}
      </div>

      {/* 新建/编辑表单 */}
      {editing && (
        <div className="rounded-lg border border-voice/30 bg-card p-4 space-y-3">
          <div className="flex items-center justify-between">
            <h3 className="text-sm font-medium">
              {editId !== null ? t("agentPanel.editAdapter") : t("agentPanel.newAdapter")}
            </h3>
            <button onClick={() => { setEditing(null); setEditId(null); }} className="text-muted-foreground hover:text-foreground">
              <X className="w-4 h-4" />
            </button>
          </div>
          <div className="grid grid-cols-2 gap-3">
            <div>
              <label className="text-xs text-muted-foreground">{t("agentPanel.keyLabel")}</label>
              <input
                className={inputClass}
                value={editing.key || ""}
                onChange={(e) => setEditing({ ...editing, key: e.target.value })}
                placeholder="myagent"
              />
            </div>
            <div>
              <label className="text-xs text-muted-foreground">{t("agentPanel.displayNameLabel")}</label>
              <input
                className={inputClass}
                value={editing.displayName || ""}
                onChange={(e) => setEditing({ ...editing, displayName: e.target.value })}
                placeholder="My Agent"
              />
            </div>
            <div>
              <label className="text-xs text-muted-foreground">{t("agentPanel.detectBinaryLabel")}</label>
              <input
                className={inputClass}
                value={editing.detectBinary || ""}
                onChange={(e) => setEditing({ ...editing, detectBinary: e.target.value })}
                placeholder="myagent"
              />
            </div>
            <div>
              <label className="text-xs text-muted-foreground">{t("agentPanel.templateLabel")}</label>
              <input
                className={inputClass}
                value={editing.commandTemplate || ""}
                onChange={(e) => setEditing({ ...editing, commandTemplate: e.target.value })}
                placeholder="myagent {prompt} {files}"
              />
            </div>
          </div>
          <button
            onClick={handleSave}
            className="flex items-center gap-1.5 px-3 py-1.5 rounded-md bg-voice text-white text-sm hover:bg-voice/90 transition-colors"
          >
            <Check className="w-3.5 h-3.5" />
            {t("agentPanel.saved")}
          </button>
        </div>
      )}

      {/* 命令模板占位符说明 */}
      <div className="rounded-lg border border-border p-4 space-y-2">
        <h3 className="text-sm font-medium">{t("agentPanel.templateHelpTitle")}</h3>
        <div className="space-y-1 text-xs text-muted-foreground font-mono">
          <div><span className="text-foreground">{"{prompt}"}</span> {t("agentPanel.templateHelpPrompt")}</div>
          <div><span className="text-foreground">{"{files}"}</span> {t("agentPanel.templateHelpFiles")}</div>
          <div><span className="text-foreground">{"{files_at}"}</span> {t("agentPanel.templateHelpFilesAt")}</div>
          <div><span className="text-foreground">{"{cwd}"}</span> {t("agentPanel.templateHelpCwd")}</div>
        </div>
      </div>
    </div>
  );
}
