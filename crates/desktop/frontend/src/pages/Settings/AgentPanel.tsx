import { useState, useEffect, useCallback } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { cn } from "@/lib/utils";
import { useT } from "@/lib/i18n";
import { Plus, RefreshCw, Check, X, Terminal, Clock, Star } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Badge } from "@/components/ui/badge";
import { Input } from "@/components/ui/input";
import { PillTabs } from "@/components/ui/tabs";

interface AgentAdapter {
  id: number;
  key: string;
  displayName: string;
  detectBinary: string;
  commandTemplate: string;
  isSystem: boolean;
  isDefault: boolean;
  isAvailable: boolean;
}

interface AgentPanelProps {
  showToast: (msg: string) => void;
}

export default function AgentPanel({ showToast }: AgentPanelProps) {
  const t = useT();
  const [activeTab, setActiveTab] = useState<"adapters" | "tasks">("adapters");

  const tabs = [
    { key: "adapters", label: t("agentPanel.title") },
    { key: "tasks", label: t("agentPanel.tasksTitle") },
  ];

  return (
    <div className="flex flex-col h-full">
      <PillTabs items={tabs} active={activeTab} onChange={(k) => setActiveTab(k as "adapters" | "tasks")} />

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

  // 设为默认 / 取消默认（全局唯一）
  const handleToggleDefault = async (a: AgentAdapter) => {
    try {
      if (a.isDefault) {
        await invoke("clear_default_agent");
        showToast(t("agentPanel.defaultCleared"));
      } else {
        await invoke("set_default_agent", { id: a.id });
        showToast(t("agentPanel.defaultSet"));
      }
      refresh();
    } catch (e) {
      showToast(t("agentPanel.defaultFailed") + String(e));
    }
  };

  return (
    <div className="space-y-4">
      {/* 操作栏——左侧默认 agent 提示，右侧操作按钮 */}
      <div className="flex items-center justify-between gap-2">
        {(() => {
          const def = adapters.find((a) => a.isDefault);
          if (!def) {
            return (
              <div className="flex items-center gap-1.5 px-3 py-1.5 rounded-md border-l-2 border-muted-foreground/30 bg-muted/30 text-[11px]">
                <Star className="w-3 h-3 text-muted-foreground/50 shrink-0" />
                <span className="text-muted-foreground">{t("agentPanel.noDefault")}</span>
              </div>
            );
          }
          return (
            <div className={cn(
              "flex items-center gap-1.5 px-3 py-1.5 rounded-md border-l-2 text-[11px]",
              def.isAvailable
                ? "border-success bg-success/[0.08]"
                : "border-muted-foreground/30 bg-muted/30",
            )}>
              <Star className={cn(
                "w-3 h-3 shrink-0",
                def.isAvailable ? "fill-success text-success" : "text-muted-foreground/50",
              )} />
              <span className="text-muted-foreground">{t("agentPanel.defaultAgentLabel")}</span>
              <span className="font-medium text-foreground">{def.displayName}</span>
              {!def.isAvailable && (
                <span className="text-muted-foreground/60">（{t("agentPanel.notFound")}）</span>
              )}
            </div>
          );
        })()}
        <div className="flex items-center gap-2 shrink-0">
          <Button variant="outline" size="sm" onClick={handleRefresh}>
            <RefreshCw />
            {t("agentPanel.refresh")}
          </Button>
          <Button
            variant="voice"
            size="sm"
            onClick={() => setEditing({
              key: "", displayName: "", detectBinary: "", commandTemplate: "{prompt}",
            })}
          >
            <Plus />
            {t("agentPanel.addNew")}
          </Button>
        </div>
      </div>

      {/* Adapter 列表 */}
      <div className="space-y-2">
        {adapters.map((a) => (
          <div
            key={a.key}
            className={cn(
              "group relative flex items-start gap-3 rounded-lg border p-3.5 transition-colors",
              a.isAvailable ? "border-success/25 bg-success/[0.03]" : "border-border",
              a.isDefault && "border-success/40",
            )}
          >
            {/* 状态色条——配色对齐提示配方：已安装/默认=绿色 success，不可用=灰 */}
            <div className={cn(
              "absolute left-0 top-3 bottom-3 w-[3px] rounded-full",
              a.isAvailable ? "bg-success" : "bg-muted-foreground/30",
            )} />

            <div className="flex-1 min-w-0 pl-2">
              <div className="flex items-center gap-2">
                <span className="text-sm font-medium">{a.displayName}</span>
                {a.isSystem && (
                  <Badge>{t("agentPanel.builtin")}</Badge>
                )}
              </div>
              <div className="text-xs text-muted-foreground font-mono mt-1 truncate">
                <span>{a.detectBinary}</span>
                <span className="text-muted-foreground/60"> {a.commandTemplate}</span>
              </div>
            </div>
            <div className="shrink-0 flex flex-col items-end gap-1.5">
              {/* 未安装：红色 X + 「未找到」文字。已安装不显示状态标识（靠按钮区分）。 */}
              {!a.isAvailable && (
                <span className="text-[10px] text-destructive font-mono flex items-center gap-0.5">
                  <X className="w-3 h-3" />
                  {t("agentPanel.notFound")}
                </span>
              )}
              {/* 设为默认 / 取消默认——配色对齐提示配方：默认=绿色 success 五角星，非默认=黄/橙 warning-soft 五角星。
                  已默认仍可点（取消默认），不 disabled。 */}
              {a.isAvailable && (
                <Button
                  variant={a.isDefault ? "success" : "warning-soft"}
                  size="sm"
                  onClick={() => handleToggleDefault(a)}
                >
                  {a.isDefault ? t("agentPanel.unsetDefault") : t("agentPanel.setDefault")}
                  <Star className={cn("w-3.5 h-3.5", a.isDefault ? "fill-success text-success" : "fill-warning text-warning")} />
                </Button>
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
            <Button variant="ghost" size="icon-sm" onClick={() => setEditing(null)}>
              <X />
            </Button>
          </div>
          <div className="grid grid-cols-2 gap-3">
            <div>
              <label className="block text-[11px] font-medium uppercase tracking-wide text-muted-foreground/80 mb-1">
                {t("agentPanel.keyLabel")}
              </label>
              <Input
                variant="mono"
                size="full"
                value={editing.key || ""}
                onChange={(e) => setEditing({ ...editing, key: e.target.value })}
                placeholder="myagent"
              />
            </div>
            <div>
              <label className="block text-[11px] font-medium uppercase tracking-wide text-muted-foreground/80 mb-1">
                {t("agentPanel.displayNameLabel")}
              </label>
              <Input
                variant="default"
                size="full"
                value={editing.displayName || ""}
                onChange={(e) => setEditing({ ...editing, displayName: e.target.value })}
                placeholder="My Agent"
              />
            </div>
            <div>
              <label className="block text-[11px] font-medium uppercase tracking-wide text-muted-foreground/80 mb-1">
                {t("agentPanel.detectBinaryLabel")}
              </label>
              <Input
                variant="mono"
                size="full"
                value={editing.detectBinary || ""}
                onChange={(e) => setEditing({ ...editing, detectBinary: e.target.value })}
                placeholder="myagent"
              />
            </div>
            <div>
              <label className="block text-[11px] font-medium uppercase tracking-wide text-muted-foreground/80 mb-1">
                {t("agentPanel.templateLabel")}
              </label>
              <Input
                variant="mono"
                size="full"
                value={editing.commandTemplate || ""}
                onChange={(e) => setEditing({ ...editing, commandTemplate: e.target.value })}
                placeholder="myagent {prompt} {files}"
              />
            </div>
          </div>
          <div className="text-xs text-muted-foreground/60">
            {t("agentPanel.templateHelpPrompt")} · {t("agentPanel.templateHelpFiles")} · {t("agentPanel.templateHelpFilesAt")} · {t("agentPanel.templateHelpCwd")}
          </div>
          <Button variant="voice" size="sm" onClick={handleSave}>
            <Check />
            {t("agentPanel.saved")}
          </Button>
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

  // 状态色用 token：done=success, failed=destructive, executing=info, 其他 muted
  const statusBar = (s: string) =>
    s === "done" ? "bg-success" : s === "failed" ? "bg-destructive" : s === "executing" ? "bg-info" : "bg-muted-foreground/50";
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
          <Button variant="ghost" size="sm" onClick={handleClearAll}>
            {t("agentPanel.clearFinished")}
          </Button>
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
              <Button
                variant="destructive-ghost"
                size="icon-sm"
                className="shrink-0"
                onClick={() => handleDelete(task.id)}
              >
                <X />
              </Button>

              {/* 错误提示 */}
              {task.errorMsg && task.status === "failed" && (
                <span className="absolute left-8 -bottom-4 text-[10px] text-destructive/60 truncate max-w-[200px]">
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
