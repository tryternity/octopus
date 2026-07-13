import { useState, useEffect, useCallback } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { cn } from "@/lib/utils";
import { useT } from "@/lib/i18n";
import { Bot, Plus, RefreshCw, Check, X, Terminal, Clock } from "lucide-react";

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

const inputClass = "w-full bg-background border border-border rounded-md px-3 py-2 text-sm outline-none focus:border-voice/50 focus:ring-1 focus:ring-voice/20 transition-all";

export default function AgentPanel({ showToast }: AgentPanelProps) {
  const t = useT();
  const [activeTab, setActiveTab] = useState<"adapters" | "tasks">("adapters");

  return (
    <div className="flex flex-col h-full">
      {/* Pill tab 条 */}
      <div className="flex gap-1 px-2 pt-1 pb-2 border-b border-border">
        <button
          className={cn(
            "flex items-center gap-1.5 px-2.5 py-1 text-[11px] font-medium rounded-md transition-all duration-150",
            activeTab === "adapters"
              ? "bg-foreground text-background"
              : "text-muted-foreground hover:text-foreground hover:bg-accent",
          )}
          onClick={() => setActiveTab("adapters")}
        >
          <Bot className="w-3 h-3" />
          {t("agentPanel.title")}
        </button>
        <button
          className={cn(
            "flex items-center gap-1.5 px-2.5 py-1 text-[11px] font-medium rounded-md transition-all duration-150",
            activeTab === "tasks"
              ? "bg-foreground text-background"
              : "text-muted-foreground hover:text-foreground hover:bg-accent",
          )}
          onClick={() => setActiveTab("tasks")}
        >
          <Terminal className="w-3 h-3" />
          {t("agentPanel.tasksTitle")}
        </button>
      </div>

      {/* Tab 内容 */}
      <div className="flex-1 overflow-y-auto px-3 py-3">
        {activeTab === "adapters" && <AdapterTab showToast={showToast} />}
        {activeTab === "tasks" && <TaskTab showToast={showToast} />}
      </div>
    </div>
  );
}

// ════════ Tab: Adapter 管理 ════════

function AdapterTab({ showToast }: { showToast: (msg: string) => void }) {
  const t = useT();
  const [adapters, setAdapters] = useState<AgentAdapter[]>([]);
  const [editing, setEditing] = useState<Partial<AgentAdapter> | null>(null);

  const refresh = useCallback(() => {
    invoke<AgentAdapter[]>("list_agent_adapters").then(setAdapters).catch(() => {});
  }, []);

  useEffect(refresh, [refresh]);

  const handleRefresh = async () => {
    await invoke<AgentAdapter[]>("refresh_agent_detection");
    refresh();
    showToast(t("agentPanel.refreshed"));
  };

  const handleSave = async () => {
    if (!editing) return;
    try {
      await invoke("create_agent_adapter", {
        key: editing.key || "",
        displayName: editing.displayName || "",
        detectBinary: editing.detectBinary || "",
        commandTemplate: editing.commandTemplate || "",
      });
      setEditing(null);
      refresh();
      showToast(t("agentPanel.saved"));
    } catch (e) {
      showToast(t("agentPanel.saveFailed") + String(e));
    }
  };

  return (
    <div className="space-y-4">
      {/* 操作栏 */}
      <div className="flex items-center justify-end gap-2">
        <button
          onClick={handleRefresh}
          className="flex items-center gap-1.5 rounded-md border border-border px-2.5 py-1.5 text-xs text-muted-foreground transition-colors hover:bg-muted/40 hover:text-foreground"
        >
          <RefreshCw className="w-3.5 h-3.5" />
          {t("agentPanel.refresh")}
        </button>
        <button
          onClick={() => setEditing({
            key: "", displayName: "", detectBinary: "", commandTemplate: "{prompt}",
          })}
          className="flex items-center gap-1.5 rounded-md bg-voice px-3 py-1.5 text-xs font-medium text-white transition-opacity hover:opacity-90"
        >
          <Plus className="w-3.5 h-3.5" />
          {t("agentPanel.addNew")}
        </button>
      </div>

      {/* Adapter 列表 */}
      <div className="space-y-2">
        {adapters.map((a) => (
          <div
            key={a.key}
            className={cn(
              "group relative flex items-start gap-3 rounded-lg border p-3.5 transition-colors",
              a.isAvailable ? "border-voice/25 bg-voice/[0.03]" : "border-border",
            )}
          >
            {/* 状态色条 */}
            <div className={cn(
              "absolute left-0 top-3 bottom-3 w-[3px] rounded-full",
              a.isAvailable ? "bg-emerald-500" : "bg-muted-foreground/30",
            )} />

            <div className="flex-1 min-w-0 pl-2">
              <div className="flex items-center gap-2">
                <span className="text-sm font-medium">{a.displayName}</span>
                {a.isBuiltin && (
                  <span className="text-[10px] px-1.5 py-0.5 rounded bg-muted text-muted-foreground">
                    {t("agentPanel.builtin")}
                  </span>
                )}
                {a.isAvailable ? (
                  <span className="text-[10px] text-emerald-500">●</span>
                ) : (
                  <span className="text-[10px] text-muted-foreground/40">○</span>
                )}
              </div>
              <div className="text-xs text-muted-foreground font-mono mt-1">{a.detectBinary}</div>
              <div className="text-xs text-muted-foreground/70 font-mono mt-0.5 truncate">
                {a.commandTemplate}
              </div>
            </div>
            <div className="shrink-0">
              {a.isAvailable ? (
                <span className="text-[10px] text-emerald-500 font-mono">{t("agentPanel.installed")}</span>
              ) : (
                <span className="text-[10px] text-muted-foreground font-mono">{t("agentPanel.notFound")}</span>
              )}
            </div>
          </div>
        ))}
      </div>

      {/* 新建表单 */}
      {editing && (
        <div className="rounded-lg border border-voice/25 bg-muted/15 p-4 space-y-3">
          <div className="flex items-center justify-between">
            <h3 className="text-sm font-medium">{t("agentPanel.newAdapter")}</h3>
            <button onClick={() => setEditing(null)} className="text-muted-foreground hover:text-foreground">
              <X className="w-4 h-4" />
            </button>
          </div>
          <div className="grid grid-cols-2 gap-3">
            <div>
              <label className="block text-[11px] font-medium uppercase tracking-wide text-muted-foreground/80 mb-1">
                {t("agentPanel.keyLabel")}
              </label>
              <input
                className={inputClass}
                value={editing.key || ""}
                onChange={(e) => setEditing({ ...editing, key: e.target.value })}
                placeholder="myagent"
              />
            </div>
            <div>
              <label className="block text-[11px] font-medium uppercase tracking-wide text-muted-foreground/80 mb-1">
                {t("agentPanel.displayNameLabel")}
              </label>
              <input
                className={inputClass}
                value={editing.displayName || ""}
                onChange={(e) => setEditing({ ...editing, displayName: e.target.value })}
                placeholder="My Agent"
              />
            </div>
            <div>
              <label className="block text-[11px] font-medium uppercase tracking-wide text-muted-foreground/80 mb-1">
                {t("agentPanel.detectBinaryLabel")}
              </label>
              <input
                className={inputClass}
                value={editing.detectBinary || ""}
                onChange={(e) => setEditing({ ...editing, detectBinary: e.target.value })}
                placeholder="myagent"
              />
            </div>
            <div>
              <label className="block text-[11px] font-medium uppercase tracking-wide text-muted-foreground/80 mb-1">
                {t("agentPanel.templateLabel")}
              </label>
              <input
                className={inputClass}
                value={editing.commandTemplate || ""}
                onChange={(e) => setEditing({ ...editing, commandTemplate: e.target.value })}
                placeholder="myagent {prompt} {files}"
              />
            </div>
          </div>
          <div className="text-xs text-muted-foreground/60">
            {t("agentPanel.templateHelpPrompt")} · {t("agentPanel.templateHelpFiles")} · {t("agentPanel.templateHelpFilesAt")} · {t("agentPanel.templateHelpCwd")}
          </div>
          <button
            onClick={handleSave}
            className="flex items-center gap-1.5 rounded-md bg-voice px-4 py-1.5 text-xs font-medium text-white transition-opacity hover:opacity-90"
          >
            <Check className="w-3.5 h-3.5" />
            {t("agentPanel.saved")}
          </button>
        </div>
      )}

      {/* 占位符参考 */}
      <div className="rounded-lg border border-border/60 bg-muted/10 p-3 space-y-1">
        <div className="flex items-center gap-1.5 mb-1">
          <Terminal className="w-3 h-3 text-muted-foreground" />
          <span className="text-[11px] font-medium uppercase tracking-wide text-muted-foreground/70">
            {t("agentPanel.templateHelpTitle")}
          </span>
        </div>
        <div className="grid grid-cols-2 gap-x-4 gap-y-0.5 text-[11px] text-muted-foreground font-mono">
          <div><span className="text-foreground">{`{prompt}`}</span> — {t("agentPanel.templateHelpPrompt")}</div>
          <div><span className="text-foreground">{`{files}`}</span> — {t("agentPanel.templateHelpFiles")}</div>
          <div><span className="text-foreground">{`{files_at}`}</span> — {t("agentPanel.templateHelpFilesAt")}</div>
          <div><span className="text-foreground">{`{cwd}`}</span> — {t("agentPanel.templateHelpCwd")}</div>
        </div>
      </div>
    </div>
  );
}

// ════════ Tab: Agent 任务 ════════

interface AgentTask {
  id: string;
  status: string;
  agentKey: string;
  transcribedText: string;
  errorMsg: string;
  createdAt: string;
}

function TaskTab({ showToast }: { showToast: (msg: string) => void }) {
  const t = useT();
  const [tasks, setTasks] = useState<AgentTask[]>([]);

  const refresh = useCallback(() => {
    invoke<AgentTask[]>("list_agent_tasks", { limit: 100 }).then(setTasks).catch(() => {});
  }, []);

  useEffect(refresh, [refresh]);

  useEffect(() => {
    const unlisten = listen("agent-task://updated", () => refresh());
    return () => { unlisten.then((f) => f()); };
  }, [refresh]);

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

  const handleClearAll = async () => {
    for (const task of tasks.filter(t => t.status === "done" || t.status === "failed")) {
      try { await invoke("delete_agent_task", { id: task.id }); } catch { /* skip */ }
    }
    refresh();
  };

  const statusBar = (s: string) =>
    s === "done" ? "bg-emerald-500" : s === "failed" ? "bg-red-500" : s === "executing" ? "bg-sky-500" : "bg-muted-foreground/50";
  const statusText = (s: string) =>
    s === "done" ? t("agentPanel.taskStatusDone") : s === "failed" ? t("agentPanel.taskStatusFailed")
    : s === "executing" ? t("agentPanel.taskStatusExecuting") : t("agentPanel.taskStatusPending");

  const doneOrFailed = tasks.filter(t => t.status === "done" || t.status === "failed");

  return (
    <div className="space-y-3">
      {/* 操作栏 */}
      <div className="flex items-center justify-between">
        <span className="text-xs text-muted-foreground">
          {tasks.length > 0 ? `${tasks.length} ${t("agentPanel.tasksTitle")}` : ""}
        </span>
        {doneOrFailed.length > 0 && (
          <button
            onClick={handleClearAll}
            className="text-xs text-muted-foreground hover:text-foreground transition-colors"
          >
            {t("agentPanel.clearFinished")}
          </button>
        )}
      </div>

      {/* 任务列表 */}
      {tasks.length === 0 ? (
        <div className="flex flex-col items-center justify-center gap-2 py-16 text-center">
          <Clock className="w-8 h-8 text-muted-foreground/30" />
          <p className="text-sm text-muted-foreground">{t("agentPanel.noTasks")}</p>
        </div>
      ) : (
        <div className="space-y-1.5">
          {tasks.map((task) => (
            <div
              key={task.id}
              className="group relative flex items-center gap-3 rounded-lg border border-border/60 bg-muted/10 px-3 py-2.5 transition-colors hover:bg-muted/20"
            >
              {/* 状态色条 */}
              <div className={cn("absolute left-0 top-2.5 bottom-2.5 w-[3px] rounded-full", statusBar(task.status))} />

              {/* 状态点 */}
              <span className={cn("h-1.5 w-1.5 shrink-0 rounded-full ml-1.5", statusBar(task.status))} />

              {/* Agent key */}
              <span className="shrink-0 text-xs font-medium">{task.agentKey}</span>

              {/* 识别文本 */}
              <span className="flex-1 truncate text-xs text-muted-foreground">
                {task.transcribedText || "—"}
              </span>

              {/* 状态标签 */}
              <span className="shrink-0 text-[10px] text-muted-foreground/70 font-mono">
                {statusText(task.status)}
              </span>

              {/* ID（hover 显示） */}
              <span className="shrink-0 font-mono text-[10px] text-muted-foreground/30 hidden group-hover:inline">
                {task.id.slice(0, 8)}
              </span>

              {/* 重试 */}
              {task.status === "failed" && (
                <button
                  onClick={() => handleRetry(task.id)}
                  className="shrink-0 text-[10px] text-voice hover:underline"
                >
                  {t("agentPanel.retry")}
                </button>
              )}

              {/* 删除 */}
              <button
                onClick={() => handleDelete(task.id)}
                className="shrink-0 text-muted-foreground/40 hover:text-red-500 transition-colors"
              >
                <X className="w-3 h-3" />
              </button>

              {/* 错误提示 */}
              {task.errorMsg && task.status === "failed" && (
                <span className="absolute left-8 -bottom-4 text-[10px] text-red-500/60 truncate max-w-[200px]">
                  {task.errorMsg}
                </span>
              )}
            </div>
          ))}
        </div>
      )}
    </div>
  );
}
