import { useState, useEffect, useCallback } from "react";
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

      {/* 任务列表区 */}
      <TaskList showToast={showToast} />
    </div>
  );
}

// ── 任务列表 ──
interface AgentTask {
  id: string;
  status: string;
  agentKey: string;
  transcribedText: string;
  errorMsg: string;
  createdAt: string;
}

function TaskList({ showToast }: { showToast: (msg: string) => void }) {
  const t = useT();
  const [tasks, setTasks] = useState<AgentTask[]>([]);

  const refresh = useCallback(() => {
    invoke<AgentTask[]>("list_agent_tasks", { limit: 50 }).then(setTasks).catch(() => {});
  }, []);

  useEffect(refresh, [refresh]);

  const handleRetry = async (id: string) => {
    try {
      await invoke("retry_agent_task", { id });
      showToast(t("agentPanel.retry"));
      refresh();
    } catch (e) { showToast(String(e)); }
  };

  const handleDelete = async (id: string) => {
    try {
      await invoke("delete_agent_task", { id });
      refresh();
    } catch (e) { showToast(String(e)); }
  };

  const statusColor = (s: string) =>
    s === "done" ? "bg-emerald-500" : s === "failed" ? "bg-red-500" : s === "executing" ? "bg-sky-500" : "bg-muted-foreground";
  const statusLabel = (s: string) =>
    s === "done" ? t("agentPanel.taskStatusDone") : s === "failed" ? t("agentPanel.taskStatusFailed")
    : s === "executing" ? t("agentPanel.taskStatusExecuting") : t("agentPanel.taskStatusPending");

  return (
    <div className="space-y-2">
      <h3 className="text-sm font-medium">{t("agentPanel.tasksTitle")}</h3>
      {tasks.length === 0 ? (
        <p className="text-xs text-muted-foreground">{t("agentPanel.noTasks")}</p>
      ) : (
        <div className="space-y-1">
          {tasks.map((task) => (
            <div key={task.id} className="flex items-center gap-3 rounded-lg border border-border p-3">
              <span className={cn("h-1.5 w-1.5 shrink-0 rounded-full", statusColor(task.status))} />
              <span className="font-mono text-[10px] text-muted-foreground">{task.id.slice(0, 8)}</span>
              <span className="text-xs">{task.agentKey}</span>
              <span className="flex-1 truncate text-xs text-muted-foreground">
                {task.transcribedText || "—"}
              </span>
              <span className="text-[10px] text-muted-foreground">{statusLabel(task.status)}</span>
              {task.status === "failed" && (
                <button onClick={() => handleRetry(task.id)} className="text-[10px] text-voice hover:underline">
                  {t("agentPanel.retry")}
                </button>
              )}
              <button onClick={() => handleDelete(task.id)} className="text-[10px] text-muted-foreground hover:text-red-500">
                ✕
              </button>
            </div>
          ))}
        </div>
      )}
    </div>
  );
}
