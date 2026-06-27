import { useState, useEffect, useCallback } from "react";
import { invoke } from "@tauri-apps/api/core";
import { cn } from "@/lib/utils";
import { Plus, Pencil, Check, Trash2, X, Eye } from "lucide-react";

interface Prompt {
  id: number;
  title: string;
  content: string;
  description: string;
  is_system: boolean;
}

export default function PromptsPanel({ showToast }: { showToast: (msg: string) => void }) {
  const [prompts, setPrompts] = useState<Prompt[]>([]);
  const [activeId, setActiveId] = useState<number | null>(null);
  const [editing, setEditing] = useState<Prompt | null>(null);
  const [isNew, setIsNew] = useState(false);
  const [title, setTitle] = useState("");
  const [content, setContent] = useState("");
  const [description, setDescription] = useState("");
  const [viewing, setViewing] = useState<Prompt | null>(null);
  const [deletePendingId, setDeletePendingId] = useState<number | null>(null);

  // 删除二次确认
  useEffect(() => {
    if (deletePendingId !== null) {
      const timer = setTimeout(() => setDeletePendingId(null), 3000);
      return () => clearTimeout(timer);
    }
  }, [deletePendingId]);

  const load = useCallback(async () => {
    try {
      const resp = await invoke<{ prompts: Prompt[]; active_prompt_id: number }>("get_config" as any) as any;
      const list = await invoke<Prompt[]>("list_prompts");
      setPrompts(list);
      setActiveId(resp.active_prompt_id);
    } catch (e) { showToast("加载失败：" + e); }
  }, [showToast]);

  useEffect(() => { load(); }, [load]);

  const activate = async (id: number) => {
    try { await invoke("set_active_prompt", { id }); setActiveId(id); showToast("已激活"); }
    catch (e) { showToast("激活失败：" + e); }
  };

  const newPrompt = () => {
    setEditing({ id: 0, title: "", content: "", description: "", is_system: false });
    setIsNew(true); setTitle(""); setContent(""); setDescription("");
  };

  const editPrompt = (p: Prompt) => {
    setEditing(p); setIsNew(false);
    setTitle(p.title); setContent(p.content); setDescription(p.description);
  };

  const save = async () => {
    if (!title.trim() || !content.trim()) { showToast("标题和内容不能为空"); return; }
    try {
      if (isNew) await invoke("create_prompt", { title, content, description });
      else if (editing) await invoke("update_prompt", { id: editing.id, title, content, description });
      setEditing(null); load(); showToast(isNew ? "已创建" : "已保存");
    } catch (e) { showToast("保存失败：" + e); }
  };

  const del = async (id: number) => {
    try { await invoke("delete_prompt", { id }); load(); showToast("已删除"); }
    catch (e) { showToast("删除失败：" + e); }
  };

  const handleDelete = (id: number) => {
    if (deletePendingId !== id) {
      setDeletePendingId(id);
    } else {
      del(id);
      setDeletePendingId(null);
    }
  };

  // ── 查看视图（只读）──
  if (viewing) {
    return (
      <div className="max-w-[640px] flex flex-col h-full">
        <div className="flex items-center justify-between mb-3">
          <h3 className="text-sm font-semibold">{viewing.title}</h3>
          <button className="p-1 text-muted-foreground hover:text-foreground" onClick={() => setViewing(null)}>
            <X className="w-4 h-4" />
          </button>
        </div>
        {viewing.description && (
          <div className="text-xs text-muted-foreground/70 mb-2">{viewing.description}</div>
        )}
        <div className="border border-border rounded-lg overflow-hidden flex-1 min-h-0 bg-background">
          <pre className="px-4 py-3 text-xs font-mono leading-relaxed whitespace-pre-wrap overflow-y-auto thin-scrollbar h-full">{viewing.content}</pre>
        </div>
      </div>
    );
  }

  // ── 编辑器 ──
  if (editing) {
    return (
      <div className="max-w-[640px] flex flex-col h-full">
        <div className="flex items-center justify-between mb-3">
          <h3 className="text-sm font-semibold">{isNew ? "新建提示词" : "编辑提示词"}</h3>
          <button className="p-1 text-muted-foreground hover:text-foreground" onClick={() => setEditing(null)}>
            <X className="w-4 h-4" />
          </button>
        </div>
        <div className="border border-border rounded-lg overflow-hidden flex flex-col flex-1 min-h-0 bg-background">
          <div className="px-4 py-2.5 border-b border-border">
            <input
              type="text"
              className="w-full text-sm font-medium outline-none bg-transparent"
              placeholder="标题"
              value={title}
              onChange={(e) => setTitle(e.target.value)}
            />
          </div>
          <div className="px-4 py-2 border-b border-border">
            <input
              type="text"
              className="w-full text-xs text-muted-foreground outline-none bg-transparent"
              placeholder="简短描述（可选）"
              value={description}
              onChange={(e) => setDescription(e.target.value)}
            />
          </div>
          <textarea
            className="flex-1 px-4 py-3 text-xs font-mono leading-relaxed outline-none resize-none bg-background min-h-[200px]"
            placeholder="提示词内容…"
            value={content}
            onChange={(e) => setContent(e.target.value)}
          />
        </div>
        <div className="flex gap-2 mt-3">
          <button
            className="flex items-center gap-1 px-4 py-1.5 bg-foreground text-background rounded-md text-sm hover:opacity-85 transition-opacity"
            onClick={save}
          >
            <Check className="w-3.5 h-3.5" /> 保存
          </button>
          <button
            className="px-4 py-1.5 border border-border rounded-md text-sm hover:border-foreground/30 transition-colors"
            onClick={() => setEditing(null)}
          >
            取消
          </button>
        </div>
      </div>
    );
  }

  // ── 列表 ──
  return (
    <div className="max-w-[640px]">
      <div className="flex items-center justify-between mb-3">
        <span className="text-sm font-semibold">提示词</span>
        <button
          className="flex items-center gap-1 px-3 py-1.5 bg-foreground text-background rounded-md text-sm hover:opacity-85 transition-opacity"
          onClick={newPrompt}
        >
          <Plus className="w-3.5 h-3.5" /> 新建
        </button>
      </div>
      {prompts.map((p) => {
        const isActive = activeId === p.id;
        return (
          <div
            key={p.id}
            className={cn(
              "border rounded-lg p-3.5 mb-2.5 transition-colors",
              isActive ? "border-voice/40 bg-voice/[0.03]" : "border-border hover:border-foreground/20",
            )}
          >
            <div className="flex items-center gap-2 mb-1">
              <span className="text-sm font-medium">{p.title}</span>
              {p.is_system && <span className="text-[9px] px-1.5 py-0.5 rounded bg-muted text-muted-foreground">内置</span>}
              {isActive && <span className="text-[9px] px-1.5 py-0.5 rounded bg-voice/15 text-voice font-medium">激活中</span>}
            </div>
            {p.description && <div className="text-xs text-muted-foreground/70 mb-1">{p.description}</div>}
            <div className="text-xs text-muted-foreground/50 whitespace-pre-wrap max-h-12 overflow-hidden leading-relaxed">{p.content}</div>
            <div className="flex gap-1.5 mt-2">
              {!isActive && (
                <button
                  className="flex items-center gap-1 px-2.5 py-1 text-xs rounded text-muted-foreground hover:text-voice transition-colors"
                  onClick={() => activate(p.id)}
                >
                  <Check className="w-3 h-3" /> 激活
                </button>
              )}
              {p.is_system && (
                <button
                  className="flex items-center gap-1 px-2.5 py-1 text-xs rounded text-muted-foreground hover:text-foreground transition-colors"
                  onClick={() => setViewing(p)}
                >
                  <Eye className="w-3 h-3" /> 查看
                </button>
              )}
              {!p.is_system && (
                <>
                  <button
                    className="flex items-center gap-1 px-2.5 py-1 text-xs rounded text-muted-foreground hover:text-foreground transition-colors"
                    onClick={() => editPrompt(p)}
                  >
                    <Pencil className="w-3 h-3" /> 编辑
                  </button>
                  <button
                    className={cn(
                      "flex items-center gap-1 px-2.5 py-1 text-xs rounded transition-colors",
                      deletePendingId === p.id
                        ? "bg-red-600 text-white"
                        : "text-muted-foreground hover:text-red-500",
                    )}
                    onClick={() => handleDelete(p.id)}
                  >
                    <Trash2 className="w-3 h-3" /> {deletePendingId === p.id ? "确认删除" : "删除"}
                  </button>
                </>
              )}
            </div>
          </div>
        );
      })}
      {prompts.length === 0 && <div className="text-center py-12 text-muted-foreground text-sm">暂无提示词</div>}
    </div>
  );
}
