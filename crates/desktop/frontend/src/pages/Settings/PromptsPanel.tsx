import { useState, useEffect, useCallback } from "react";
import { invoke } from "@tauri-apps/api/core";
import { cn } from "@/lib/utils";
import { Plus, Pencil, Check, Trash2, FileText } from "lucide-react";
import { useT } from "@/lib/i18n";
import { Button } from "@/components/ui/button";
import { Badge } from "@/components/ui/badge";
import { Card } from "@/components/ui/card";

interface Prompt {
  id: number;
  title: string;
  content: string; // 文件名引用（不含 .md）
  description: string;
  is_system: boolean;
}

export default function PromptsPanel({ showToast }: { showToast: (msg: string) => void }) {
  const t = useT();
  const [prompts, setPrompts] = useState<Prompt[]>([]);
  const [activeId, setActiveId] = useState<number | null>(null);
  const [showNewForm, setShowNewForm] = useState(false);
  const [newTitle, setNewTitle] = useState("");
  const [newFileName, setNewFileName] = useState("");
  const [newDesc, setNewDesc] = useState("");
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
    } catch (e) { showToast(t("settings.prompts.loadFailed") + e); }
  }, [showToast]);

  useEffect(() => { load(); }, [load]);

  const activate = async (id: number) => {
    try { await invoke("set_active_prompt", { id }); setActiveId(id); showToast(t("settings.prompts.activated")); }
    catch (e) { showToast(t("settings.prompts.activateFailed") + e); }
  };

  // 编辑 → CompactEditor 打开文件
  const editInEditor = (p: Prompt) => {
    invoke("open_file_in_editor", { name: p.content, category: "polish" })
      .catch((e: unknown) => showToast(String(e)));
  };

  // 新建 prompt：创建文件 + DB 记录 + 打开编辑器
  const createNew = async () => {
    if (!newTitle.trim() || !newFileName.trim()) {
      showToast(t("settings.prompts.emptyError"));
      return;
    }
    try {
      // 1. 创建空白 md 文件
      await invoke("create_prompt_file", { category: "polish", name: newFileName.trim() });
      // 2. DB 存记录（content = 文件名）
      await invoke("create_prompt", { title: newTitle, content: newFileName.trim(), description: newDesc });
      // 3. 刷新列表
      await load();
      setShowNewForm(false);
      setNewTitle(""); setNewFileName(""); setNewDesc("");
      showToast(t("settings.prompts.created"));
      // 4. 自动打开编辑器
      invoke("open_file_in_editor", { name: newFileName.trim(), category: "polish" })
        .catch(() => {});
    } catch (e) { showToast(t("settings.prompts.saveFailed") + e); }
  };

  const del = async (id: number) => {
    try { await invoke("delete_prompt", { id }); load(); showToast(t("settings.prompts.deleted")); }
    catch (e) { showToast(t("settings.prompts.deleteFailed") + e); }
  };

  const handleDelete = (id: number) => {
    if (deletePendingId !== id) {
      setDeletePendingId(id);
    } else {
      del(id);
      setDeletePendingId(null);
    }
  };

  // ── 新建表单 ──
  if (showNewForm) {
    return (
      <div className="flex flex-col h-full">
        <div className="flex items-center justify-between mb-3">
          <h3 className="text-sm font-semibold">{t("settings.prompts.newPrompt")}</h3>
          <Button variant="ghost" size="icon-sm" onClick={() => setShowNewForm(false)}>
            <Trash2 className="h-4 w-4" />
          </Button>
        </div>
        <Card className="p-4 space-y-3">
          <div>
            <label className="text-xs text-muted-foreground mb-1 block">{t("settings.prompts.titlePlaceholder")}</label>
            <input
              type="text"
              className="w-full text-sm outline-none bg-transparent border border-border rounded-md px-3 py-2"
              placeholder={t("settings.prompts.titlePlaceholder")}
              value={newTitle}
              onChange={(e) => setNewTitle(e.target.value)}
            />
          </div>
          <div>
            <label className="text-xs text-muted-foreground mb-1 block">{t("settings.prompts.fileNameLabel")}</label>
            <div className="flex items-center gap-1.5">
              <input
                type="text"
                className="flex-1 text-sm font-mono outline-none bg-transparent border border-border rounded-md px-3 py-2"
                placeholder="my-prompt"
                value={newFileName}
                onChange={(e) => setNewFileName(e.target.value)}
              />
              <span className="text-xs text-muted-foreground/60">.md</span>
            </div>
            <p className="text-[11px] text-muted-foreground/50 mt-1">~/.octopus/.sync/prompts/polish/{newFileName || "..."}.md</p>
          </div>
          <div>
            <label className="text-xs text-muted-foreground mb-1 block">{t("settings.prompts.descPlaceholder")}</label>
            <input
              type="text"
              className="w-full text-xs text-muted-foreground outline-none bg-transparent border border-border rounded-md px-3 py-2"
              placeholder={t("settings.prompts.descPlaceholder")}
              value={newDesc}
              onChange={(e) => setNewDesc(e.target.value)}
            />
          </div>
        </Card>
        <div className="flex gap-2 mt-3">
          <Button variant="primary" size="default" onClick={createNew}>
            <Check /> {t("settings.prompts.save")}
          </Button>
          <Button variant="outline" size="default" onClick={() => setShowNewForm(false)}>
            {t("settings.prompts.cancel")}
          </Button>
        </div>
      </div>
    );
  }

  // ── 列表 ──
  return (
    <div>
      <div className="flex items-center justify-end mb-3">
        <Button variant="primary" size="default" onClick={() => setShowNewForm(true)}>
          <Plus /> {t("settings.prompts.newBtn")}
        </Button>
      </div>
      {prompts.map((p) => {
        const isActive = activeId === p.id;
        return (
          <Card
            key={p.id}
            className={cn(
              "mb-2.5 p-3.5 transition-colors",
              isActive ? "border-voice/40 bg-voice/[0.03]" : "hover:border-foreground/20",
            )}
          >
            <div className="flex items-center gap-2 mb-1">
              <span className="text-sm font-medium">{p.title}</span>
              {p.is_system && <Badge>{t("settings.prompts.builtin")}</Badge>}
              {isActive && <Badge variant="voice">{t("settings.prompts.activeBadge")}</Badge>}
            </div>
            {p.description && <div className="text-xs text-muted-foreground/70 mb-1">{p.description}</div>}
            {/* 文件名引用展示 */}
            <div className="flex items-center gap-1 text-xs text-muted-foreground/50">
              <FileText className="h-3 w-3" />
              <code>{p.content}.md</code>
            </div>
            <div className="flex gap-1.5 mt-2">
              {!isActive && (
                <Button variant="ghost" size="sm" onClick={() => activate(p.id)}>
                  <Check /> {t("settings.prompts.activate")}
                </Button>
              )}
              <Button variant="ghost" size="sm" onClick={() => editInEditor(p)}>
                <Pencil /> {t("settings.prompts.edit")}
              </Button>
              {!p.is_system && (
                <Button
                  variant={deletePendingId === p.id ? "destructive" : "destructive-ghost"}
                  size="sm"
                  onClick={() => handleDelete(p.id)}
                >
                  <Trash2 /> {deletePendingId === p.id ? t("settings.prompts.confirmDelete") : t("settings.prompts.delete")}
                </Button>
              )}
            </div>
          </Card>
        );
      })}
      {prompts.length === 0 && <div className="text-center py-12 text-muted-foreground text-sm">{t("settings.prompts.empty")}</div>}
    </div>
  );
}
