import { useState, useEffect, useCallback } from "react";
import { invoke } from "@tauri-apps/api/core";
import { cn } from "@/lib/utils";
import { Plus, Pencil, Check, Trash2, FileText, Route, CheckCircle2 } from "lucide-react";
import { useT } from "@/lib/i18n";
import { Button } from "@/components/ui/button";
import { Badge } from "@/components/ui/badge";
import { Card } from "@/components/ui/card";
import { Toggle } from "@/components/ui/toggle";
import RouteConfigDialog from "./Prompts/RouteConfigDialog";

interface Prompt {
  id: number;
  title: string;
  content: string; // 文件名引用（不含 .md）
  description: string;
  isSystem: boolean;
  appBundleIds: string; // JSON 数组字符串 ["com.tencent.xinWeChat"]，空=全局
  injectContext: boolean; // 0=不注入 app 上下文，1=注入
}

export default function PromptsPanel({ showToast }: { showToast: (msg: string) => void }) {
  const t = useT();
  const [prompts, setPrompts] = useState<Prompt[]>([]);
  const [activeId, setActiveId] = useState<number | null>(null);
  const [showNewForm, setShowNewForm] = useState(false);
  const [newTitle, setNewTitle] = useState("");
  const [newFileName, setNewFileName] = useState("");
  const [newDesc, setNewDesc] = useState("");
  const [newInjectContext, setNewInjectContext] = useState(true); // 用户自建默认注入（spec：inject_context=1）
  const [deletePendingId, setDeletePendingId] = useState<number | null>(null);
  // 路由配置弹窗：编辑中的 prompt（null=弹窗关闭）
  const [routeEditing, setRouteEditing] = useState<Prompt | null>(null);

  // 删除二次确认
  useEffect(() => {
    if (deletePendingId !== null) {
      const timer = setTimeout(() => setDeletePendingId(null), 3000);
      return () => clearTimeout(timer);
    }
  }, [deletePendingId]);

  const load = useCallback(async () => {
    try {
      const resp = await invoke<{ prompts: Prompt[]; activePromptId: number }>("get_config" as any) as any;
      const list = await invoke<Prompt[]>("list_prompts");
      setPrompts(list);
      setActiveId(resp.activePromptId);
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
      // 2. DB 存记录（content = 文件名；新 prompt 默认全局 app_bundle_ids=''）
      await invoke("create_prompt", {
        title: newTitle,
        content: newFileName.trim(),
        description: newDesc,
        appBundleIds: "",
        injectContext: newInjectContext,
      });
      // 3. 刷新列表
      await load();
      setShowNewForm(false);
      setNewTitle(""); setNewFileName(""); setNewDesc(""); setNewInjectContext(true);
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

  // 路由配置弹窗保存：update_prompt 全字段回写（app_bundle_ids + inject_context 是本次新改的，
  // 其余字段原样回传——update_prompt 是全量 UPDATE）。
  const saveRouteConfig = async (appBundleIds: string, injectContext: boolean) => {
    const p = routeEditing;
    if (!p) return;
    try {
      await invoke("update_prompt", {
        id: p.id,
        title: p.title,
        content: p.content,
        description: p.description,
        appBundleIds,
        injectContext,
      });
      await load();
      setRouteEditing(null);
      showToast(t("settings.prompts.saved"));
    } catch (e) { showToast(t("settings.prompts.saveFailed") + e); }
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
              autoCapitalize="off"
              autoCorrect="off"
              spellCheck={false}
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
                autoCapitalize="off"
                autoCorrect="off"
                spellCheck={false}
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
              autoCapitalize="off"
              autoCorrect="off"
              spellCheck={false}
              className="w-full text-xs text-muted-foreground outline-none bg-transparent border border-border rounded-md px-3 py-2"
              placeholder={t("settings.prompts.descPlaceholder")}
              value={newDesc}
              onChange={(e) => setNewDesc(e.target.value)}
            />
          </div>
          {/* 注入应用上下文：新 prompt 默认 true（spec）。app 关联可在创建后用「路由配置」按钮绑定。 */}
          <label className="flex items-start gap-2 cursor-pointer pt-1">
            <Toggle
              on={newInjectContext}
              onClick={() => setNewInjectContext((v) => !v)}
              aria-label={t("settings.prompts.injectContext")}
            />
            <div className="flex flex-col min-w-0">
              <span className="text-xs text-foreground">
                {t("settings.prompts.injectContext")}
              </span>
              <span className="text-[10px] text-muted-foreground leading-relaxed mt-0.5">
                {t("settings.prompts.injectContextHint")}
              </span>
            </div>
          </label>
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
      {/* 当前激活模板提示（对齐模型管理的 CurrentBanner） */}
      {(() => {
        const active = prompts.find((p) => p.id === activeId);
        if (!active) return null;
        return (
          <div className="flex items-center gap-1.5 px-3 py-1.5 rounded-md border-l-2 border-success bg-success/10 text-[11px] mb-2">
            <CheckCircle2 className="w-3 h-3 text-success shrink-0" />
            <span className="text-muted-foreground">{t("settings.prompts.currentActive")}</span>
            <span className="font-medium text-foreground">{active.title}</span>
          </div>
        );
      })()}
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
              {p.isSystem && <Badge>{t("settings.prompts.builtin")}</Badge>}
            </div>
            {p.description && <div className="text-xs text-muted-foreground/70 mb-1">{p.description}</div>}
            {/* 文件名引用展示 */}
            <div className="flex items-center gap-1 text-xs text-muted-foreground/50">
              <FileText className="h-3 w-3" />
              <code>{p.content}.md</code>
            </div>
            {/* app 关联指示：有绑定→显示绑定的 app 数；无绑定→显示全局。
                系统内置模板固定全局 + inject_context，不展示此行（值不可改，展示易误导） */}
            {!p.isSystem && (
              <div className="flex items-center gap-1.5 mt-1 text-[11px]">
                <Route className="h-3 w-3 text-muted-foreground/50" />
                {(() => {
                  let bound = 0;
                  try { const a = JSON.parse(p.appBundleIds || "[]"); bound = Array.isArray(a) ? a.length : 0; } catch { /* */ }
                  return bound > 0
                    ? <span className="text-voice/80">{t("settings.prompts.boundApps", { n: bound })}</span>
                    : <span className="text-muted-foreground/50">{t("settings.prompts.globalPrompt")}</span>;
                })()}
                {p.injectContext && (
                  <Badge variant="outline" className="text-[10px] px-1 py-0 h-4">
                    {t("settings.prompts.injectContextBadge")}
                  </Badge>
                )}
              </div>
            )}
            <div className="flex gap-1.5 mt-2">
              {/* 激活态对齐模型管理：当前激活→绿色「已激活」灰禁；其余→绿色「激活」可点 */}
              {isActive ? (
                <Button variant="success" size="sm" disabled className="cursor-default">
                  <Check /> {t("settings.prompts.activated")}
                </Button>
              ) : (
                <Button variant="success" size="sm" onClick={() => activate(p.id)}>
                  <Check /> {t("settings.prompts.activate")}
                </Button>
              )}
              <Button variant="ghost" size="sm" onClick={() => editInEditor(p)}>
                <Pencil /> {t("settings.prompts.edit")}
              </Button>
              {/* 系统内置模板锁定全局 + 固定 inject_context（保持 fallback 角色），不可路由配置 */}
              <Button
                variant="ghost"
                size="sm"
                disabled={p.isSystem}
                onClick={() => setRouteEditing(p)}
                title={p.isSystem ? t("settings.prompts.routeConfigDisabled") : undefined}
              >
                <Route /> {t("settings.prompts.routeConfig")}
              </Button>
              {!p.isSystem && (
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
      {routeEditing && (
        <RouteConfigDialog
          promptTitle={routeEditing.title}
          appBundleIds={routeEditing.appBundleIds}
          injectContext={routeEditing.injectContext}
          onCancel={() => setRouteEditing(null)}
          onSave={saveRouteConfig}
          t={t}
        />
      )}
    </div>
  );
}
