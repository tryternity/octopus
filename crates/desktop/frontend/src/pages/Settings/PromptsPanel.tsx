import { useState, useEffect, useCallback } from "react";
import { invoke } from "@tauri-apps/api/core";
import { cn } from "@/lib/utils";

interface Prompt {
  id: number;
  title: string;
  content: string;
  description: string;
  is_system: boolean;
  created_at: string;
  updated_at: string;
}

export default function PromptsPanel({ showToast }: { showToast: (msg: string) => void }) {
  const [prompts, setPrompts] = useState<Prompt[]>([]);
  const [activeId, setActiveId] = useState<number | null>(null);
  const [editing, setEditing] = useState<Prompt | null>(null);
  const [isNew, setIsNew] = useState(false);
  const [title, setTitle] = useState("");
  const [content, setContent] = useState("");
  const [description, setDescription] = useState("");

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
    try {
      await invoke("set_active_prompt", { id });
      setActiveId(id);
      showToast("已激活");
    } catch (e) { showToast("激活失败：" + e); }
  };

  const newPrompt = () => {
    setEditing({ id: 0, title: "", content: "", description: "", is_system: false, created_at: "", updated_at: "" });
    setIsNew(true);
    setTitle(""); setContent(""); setDescription("");
  };

  const editPrompt = (p: Prompt) => {
    setEditing(p);
    setIsNew(false);
    setTitle(p.title); setContent(p.content); setDescription(p.description);
  };

  const save = async () => {
    if (!title.trim() || !content.trim()) { showToast("标题和内容不能为空"); return; }
    try {
      if (isNew) {
        await invoke("create_prompt", { title, content, description });
      } else if (editing) {
        await invoke("update_prompt", { id: editing.id, title, content, description });
      }
      setEditing(null);
      load();
      showToast(isNew ? "已创建" : "已保存");
    } catch (e) { showToast("保存失败：" + e); }
  };

  const del = async (id: number) => {
    try {
      await invoke("delete_prompt", { id });
      load();
      showToast("已删除");
    } catch (e) { showToast("删除失败：" + e); }
  };

  if (editing) {
    return (
      <div className="bg-card border border-border rounded-lg p-4 mb-4 flex flex-col flex-1 min-h-0">
        <h3 className="text-sm font-semibold mb-3">{isNew ? "新建提示词" : "编辑提示词"}</h3>
        <div className="mb-3">
          <label className="block text-[13px] mb-1">标题</label>
          <input
            type="text"
            className="w-full px-2.5 py-1.5 border border-border rounded-md text-sm"
            value={title}
            onChange={(e) => setTitle(e.target.value)}
          />
        </div>
        <div className="mb-3">
          <label className="block text-[13px] mb-1">描述</label>
          <input
            type="text"
            className="w-full px-2.5 py-1.5 border border-border rounded-md text-sm"
            value={description}
            onChange={(e) => setDescription(e.target.value)}
          />
        </div>
        <div className="mb-3 flex-1 min-h-0 flex flex-col">
          <label className="block text-[13px] mb-1">内容</label>
          <textarea
            className="w-full flex-1 px-2.5 py-1.5 border border-border rounded-md text-sm resize-none font-mono min-h-[200px]"
            value={content}
            onChange={(e) => setContent(e.target.value)}
          />
        </div>
        <div className="flex gap-2 mt-2">
          <button
            className="px-4 py-1.5 bg-primary text-white border border-primary rounded-md text-sm hover:opacity-90"
            onClick={save}
          >
            保存
          </button>
          <button
            className="px-4 py-1.5 border border-border rounded-md text-sm hover:border-primary hover:text-primary"
            onClick={() => setEditing(null)}
          >
            取消
          </button>
        </div>
      </div>
    );
  }

  return (
    <div>
      <div className="flex items-center justify-between mb-4">
        <span className="text-base font-semibold">提示词</span>
        <button
          className="px-4 py-1.5 bg-primary text-white border border-primary rounded-md text-sm hover:opacity-90"
          onClick={newPrompt}
        >
          新建
        </button>
      </div>
      {prompts.map((p) => (
        <div
          key={p.id}
          className={cn(
            "bg-card border rounded-lg p-3.5 mb-3 transition-colors",
            activeId === p.id ? "border-primary shadow-[0_0_0_2px_rgba(0,122,255,0.1)]" : "border-border",
          )}
        >
          <div className="flex items-center justify-between gap-2 mb-1.5">
            <span className="text-sm font-semibold flex items-center gap-1.5">
              {p.title}
              {p.is_system && <span className="text-[10px] px-1.5 py-0.5 rounded bg-primary/10 text-primary">内置</span>}
              {activeId === p.id && <span className="text-[10px] px-1.5 py-0.5 rounded bg-green-500/15 text-green-600">已激活</span>}
            </span>
          </div>
          {p.description && <div className="text-xs text-muted-foreground mb-1.5">{p.description}</div>}
          <div className="text-xs text-muted-foreground whitespace-pre-wrap max-h-[60px] overflow-hidden leading-[1.4]">{p.content}</div>
          <div className="flex gap-2 mt-2">
            {!p.is_system && (
              <button
                className="px-3 py-1 border border-border rounded-md text-xs hover:border-primary hover:text-primary"
                onClick={() => editPrompt(p)}
              >
                编辑
              </button>
            )}
            {activeId !== p.id && (
              <button
                className="px-3 py-1 bg-primary text-white border border-primary rounded-md text-xs hover:opacity-90"
                onClick={() => activate(p.id)}
              >
                激活
              </button>
            )}
            {!p.is_system && (
              <button
                className="px-3 py-1 border border-border rounded-md text-xs hover:border-red-500 hover:text-red-500"
                onClick={() => del(p.id)}
              >
                删除
              </button>
            )}
          </div>
        </div>
      ))}
      {prompts.length === 0 && <div className="text-center py-12 text-muted-foreground">暂无提示词</div>}
    </div>
  );
}
