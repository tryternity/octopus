import { useState, useEffect } from "react";
import { invoke } from "@tauri-apps/api/core";
import { cn } from "@/lib/utils";
import { useT } from "@/lib/i18n";
import { Bot, Plus, Trash2, RefreshCw, Pencil, Check, X } from "lucide-react";

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
    showToast("已刷新检测");
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
      showToast("已保存");
    } catch (e) {
      showToast(String(e));
    }
  };

  const handleDelete = async (key: string) => {
    const adapter = adapters.find((a) => a.key === key);
    if (!adapter || adapter.isBuiltin) return;
    // 找到 DB id 需要额外接口——此处用 key 删除（后端需要 key-based delete 或前端存 id）
    // 简化：用 list 中的 key 匹配，后端 delete_agent_adapter 需要 id
    // 先查 adapter 列表获取可能的 id——但 list_agent_adapters 返回的 AgentAdapter 无 id
    // TODO: 后端 AgentAdapter 加 id 字段
    showToast("自定义 adapter 删除功能待完善");
  };

  const startEdit = (adapter: AgentAdapter) => {
    setEditing({ ...adapter });
    // 内置 adapter 不允许编辑，仅查看
    if (adapter.isBuiltin) return;
    // 自定义 adapter 需要 id 来更新——暂用 key 查
    setEditId(null); // 需要 DB id，后端 AgentAdapter 暂无 id 字段
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
          <h2 className="text-lg font-semibold">Agent 管理</h2>
        </div>
        <div className="flex items-center gap-2">
          <button
            onClick={handleRefresh}
            className="flex items-center gap-1.5 text-xs text-muted-foreground hover:text-foreground transition-colors"
          >
            <RefreshCw className="w-3.5 h-3.5" />
            刷新检测
          </button>
          <button
            onClick={startCreate}
            className="flex items-center gap-1.5 text-xs text-voice hover:underline"
          >
            <Plus className="w-3.5 h-3.5" />
            新增
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
                  <span className="text-[10px] px-1.5 py-0.5 rounded bg-muted text-muted-foreground">内置</span>
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
                <span className="text-xs text-emerald-500 whitespace-nowrap">✅ 已安装</span>
              ) : (
                <span className="text-xs text-muted-foreground whitespace-nowrap">❌ 未找到</span>
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
              {editId !== null ? "编辑 Adapter" : "新增 Adapter"}
            </h3>
            <button onClick={() => { setEditing(null); setEditId(null); }} className="text-muted-foreground hover:text-foreground">
              <X className="w-4 h-4" />
            </button>
          </div>
          <div className="grid grid-cols-2 gap-3">
            <div>
              <label className="text-xs text-muted-foreground">Key（唯一标识）</label>
              <input
                className={inputClass}
                value={editing.key || ""}
                onChange={(e) => setEditing({ ...editing, key: e.target.value })}
                placeholder="myagent"
              />
            </div>
            <div>
              <label className="text-xs text-muted-foreground">显示名称</label>
              <input
                className={inputClass}
                value={editing.displayName || ""}
                onChange={(e) => setEditing({ ...editing, displayName: e.target.value })}
                placeholder="My Agent"
              />
            </div>
            <div>
              <label className="text-xs text-muted-foreground">检测二进制名</label>
              <input
                className={inputClass}
                value={editing.detectBinary || ""}
                onChange={(e) => setEditing({ ...editing, detectBinary: e.target.value })}
                placeholder="myagent"
              />
            </div>
            <div>
              <label className="text-xs text-muted-foreground">命令模板</label>
              <input
                className={inputClass}
                value={editing.commandTemplate || ""}
                onChange={(e) => setEditing({ ...editing, commandTemplate: e.target.value })}
                placeholder="myagent {prompt} {files}"
              />
            </div>
          </div>
          <div className="text-xs text-muted-foreground">
            可用占位符：{"{prompt}"} {"{files}"} {"{files_at}"} {"{cwd}"}
          </div>
          <button
            onClick={handleSave}
            className="flex items-center gap-1.5 px-3 py-1.5 rounded-md bg-voice text-white text-sm hover:bg-voice/90 transition-colors"
          >
            <Check className="w-3.5 h-3.5" />
            保存
          </button>
        </div>
      )}

      {/* 命令模板占位符说明 */}
      <div className="rounded-lg border border-border p-4 space-y-2">
        <h3 className="text-sm font-medium">命令模板占位符</h3>
        <div className="space-y-1 text-xs text-muted-foreground font-mono">
          <div><span className="text-foreground">{"{prompt}"}</span> — 渲染后的 prompt（含用户 task）</div>
          <div><span className="text-foreground">{"{files}"}</span> — 空格分隔的文件路径</div>
          <div><span className="text-foreground">{"{files_at}"}</span> — @ 前缀的文件路径（pi 风格）</div>
          <div><span className="text-foreground">{"{cwd}"}</span> — 首个文件的父目录</div>
        </div>
      </div>
    </div>
  );
}
