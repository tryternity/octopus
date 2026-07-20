import { useState, useEffect, useCallback } from "react";
import { invoke } from "@tauri-apps/api/core";
import { cn } from "@/lib/utils";
import { Plus, Pencil, Check, Trash2, X, Eye, RotateCcw } from "lucide-react";
import { useT } from "@/lib/i18n";
import { Button } from "@/components/ui/button";
import { Badge } from "@/components/ui/badge";
import { Textarea } from "@/components/ui/input";
import { Card } from "@/components/ui/card";

interface Prompt {
  id: number;
  title: string;
  content: string;
  description: string;
  is_system: boolean;
}

export default function PromptsPanel({ showToast }: { showToast: (msg: string) => void }) {
  const t = useT();
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
    } catch (e) { showToast(t("settings.prompts.loadFailed") + e); }
  }, [showToast]);

  useEffect(() => { load(); }, [load]);

  const activate = async (id: number) => {
    try { await invoke("set_active_prompt", { id }); setActiveId(id); showToast(t("settings.prompts.activated")); }
    catch (e) { showToast(t("settings.prompts.activateFailed") + e); }
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
    if (!title.trim() || !content.trim()) { showToast(t("settings.prompts.emptyError")); return; }
    try {
      if (isNew) await invoke("create_prompt", { title, content, description });
      else if (editing) await invoke("update_prompt", { id: editing.id, title, content, description });
      setEditing(null); load(); showToast(isNew ? t("settings.prompts.created") : t("settings.prompts.saved"));
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

  // ── 查看视图（只读）──
  if (viewing) {
    return (
      <div className="flex flex-col h-full">
        <div className="flex items-center justify-between mb-3">
          <h3 className="text-sm font-semibold">{viewing.title}</h3>
          <Button variant="ghost" size="icon-sm" onClick={() => setViewing(null)}>
            <X />
          </Button>
        </div>
        {viewing.description && (
          <div className="text-xs text-muted-foreground/70 mb-2">{viewing.description}</div>
        )}
        <Card className="flex-1 min-h-0 overflow-hidden">
          <pre className="px-4 py-3 text-xs font-mono leading-relaxed whitespace-pre-wrap overflow-y-auto thin-scrollbar h-full">{viewing.content}</pre>
        </Card>
      </div>
    );
  }

  // ── 编辑器 ──
  if (editing) {
    return (
      <div className="flex flex-col h-full">
        <div className="flex items-center justify-between mb-3">
          <h3 className="text-sm font-semibold">{isNew ? t("settings.prompts.newPrompt") : t("settings.prompts.editPrompt")}</h3>
          <Button variant="ghost" size="icon-sm" onClick={() => setEditing(null)}>
            <X />
          </Button>
        </div>
        <Card className="flex-1 min-h-0 flex flex-col overflow-hidden">
          <div className="px-4 py-2.5 border-b border-border">
            <input
              type="text"
              className="w-full text-sm font-medium outline-none bg-transparent"
              placeholder={t("settings.prompts.titlePlaceholder")}
              value={title}
              onChange={(e) => setTitle(e.target.value)}
            />
          </div>
          <div className="px-4 py-2 border-b border-border">
            <input
              type="text"
              className="w-full text-xs text-muted-foreground outline-none bg-transparent"
              placeholder={t("settings.prompts.descPlaceholder")}
              value={description}
              onChange={(e) => setDescription(e.target.value)}
            />
          </div>
          <Textarea
            variant="bare"
            size="full"
            className="flex-1 px-4 py-3 font-mono resize-none min-h-[200px]"
            placeholder={t("settings.prompts.contentPlaceholder")}
            value={content}
            onChange={(e) => setContent(e.target.value)}
          />
        </Card>
        <div className="flex gap-2 mt-3">
          <Button variant="primary" size="default" onClick={save}>
            <Check /> {t("settings.prompts.save")}
          </Button>
          {editing.is_system && (
            <Button
              variant="outline"
              size="default"
              onClick={async () => {
                try {
                  const restored = await invoke<string>("restore_prompt_from_seed", { promptId: editing.id });
                  setContent(restored);
                  showToast(t("settings.prompts.restored"));
                } catch (e) { showToast(t("settings.prompts.restoreFailed") + e); }
              }}
            >
              <RotateCcw /> {t("settings.prompts.restore")}
            </Button>
          )}
          <Button variant="outline" size="default" onClick={() => setEditing(null)}>
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
        <Button variant="primary" size="default" onClick={newPrompt}>
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
            <div className="text-xs text-muted-foreground/50 whitespace-pre-wrap max-h-12 overflow-hidden leading-relaxed">{p.content}</div>
            <div className="flex gap-1.5 mt-2">
              {!isActive && (
                <Button variant="ghost" size="sm" onClick={() => activate(p.id)}>
                  <Check /> {t("settings.prompts.activate")}
                </Button>
              )}
              {p.is_system && (
                <>
                  <Button variant="ghost" size="sm" onClick={() => setViewing(p)}>
                    <Eye /> {t("settings.prompts.view")}
                  </Button>
                  <Button variant="ghost" size="sm" onClick={() => editPrompt(p)}>
                    <Pencil /> {t("settings.prompts.edit")}
                  </Button>
                </>
              )}
              {!p.is_system && (
                <>
                  <Button variant="ghost" size="sm" onClick={() => editPrompt(p)}>
                    <Pencil /> {t("settings.prompts.edit")}
                  </Button>
                  <Button
                    variant={deletePendingId === p.id ? "destructive" : "destructive-ghost"}
                    size="sm"
                    onClick={() => handleDelete(p.id)}
                  >
                    <Trash2 /> {deletePendingId === p.id ? t("settings.prompts.confirmDelete") : t("settings.prompts.delete")}
                  </Button>
                </>
              )}
            </div>
          </Card>
        );
      })}
      {prompts.length === 0 && <div className="text-center py-12 text-muted-foreground text-sm">{t("settings.prompts.empty")}</div>}
    </div>
  );
}
