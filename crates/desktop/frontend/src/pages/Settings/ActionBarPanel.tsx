/**
 * ActionBarPanel —— 命令面板设置页。
 *
 * 2026-07-30 拆分：子组件 + 常量 + 类型移到 ActionBar/ 目录。
 */
import { useState, useCallback, useEffect, useRef } from "react";
import { invoke } from "@tauri-apps/api/core";
import { emit } from "@tauri-apps/api/event";
import { Plus, X } from "lucide-react";
import { cn } from "@/lib/utils";
import { useT, t as ti18n } from "@/lib/i18n";
import ShortcutButton from "@/components/ShortcutButton";
import { Button } from "@/components/ui/button";
import { UnderlineTabs, Segmented } from "@/components/ui/tabs";

// 拆分出的子模块
import type { ActionBarItem } from "./ActionBar/types";
import { deriveAccepts, inputBase, Toggle } from "./ActionBar/constants";
import EditForm from "./ActionBar/EditForm";
import MenuRow from "./ActionBar/MenuRow";
import ScriptRunsList from "./ActionBar/ScriptRunsList";

export default function ActionBarPanel({
  showToast,
}: {
  showToast: (msg: string) => void;
}) {
  const t = useT();
  const [items, setItems] = useState<ActionBarItem[]>([]);
  const [editingId, setEditingId] = useState<number | null>(null);
  const [editingForm, setEditingForm] = useState<Partial<ActionBarItem>>({});
  const [draftParentId, setDraftParentId] = useState<number | null | undefined>(undefined);
  const [loaded, setLoaded] = useState(false);
  const [tab, setTab] = useState<"menu" | "runs">("menu");
  const [scopeFilter, setScopeFilter] = useState<"text" | "file">("text");
  const [selectedMainMenuId, setSelectedMainMenuId] = useState<number | null>(null);
  const [titleDraft, setTitleDraft] = useState<string | null>(null);
  const titleDraftRef = useRef<string | null>(null);
  titleDraftRef.current = titleDraft;
  const [inlineCapturingGlobal, setInlineCapturingGlobal] = useState(false);
  const [capturingItem, setCapturingItem] = useState<{ id: number; kind: "global" } | null>(null);

  const refresh = useCallback(async (): Promise<ActionBarItem[]> => {
    const list = await invoke<ActionBarItem[]>("list_action_bar_items");
    setItems(list);
    emit("action-bar://items-changed", null);
    setLoaded(true);
    return list;
  }, []);

  useEffect(() => {
    (async () => { await refresh(); })();
  }, [refresh]);

  const isItemInScope = (item: ActionBarItem): boolean => {
    const accepts = item.accepts || "text";
    if (scopeFilter === "text") return accepts === "text" || accepts === "any";
    return accepts === "file" || accepts === "any";
  };

  const isSubmenuInScope = (item: ActionBarItem): boolean => {
    const subs = items.filter((i) => i.parentId === item.id);
    if (subs.length === 0) return isItemInScope(item);
    return subs.some((s) => s.actionType === "submenu" ? isSubmenuInScope(s) : isItemInScope(s));
  };

  const mainItems = items.filter((i) => i.parentId === null && (
    i.actionType === "submenu" ? isSubmenuInScope(i) : isItemInScope(i)
  ));

  const effectiveSelectedId = selectedMainMenuId !== null && mainItems.some((m) => m.id === selectedMainMenuId)
    ? selectedMainMenuId
    : mainItems[0]?.id ?? null;
  const selectedMain = effectiveSelectedId !== null
    ? items.find((i) => i.id === effectiveSelectedId) ?? null
    : null;
  const selectedSubs = selectedMain !== null
    ? items.filter((i) => i.parentId === selectedMain.id)
    : [];

  useEffect(() => {
    if (tab === "menu" && effectiveSelectedId === null && mainItems.length > 0) {
      setSelectedMainMenuId(mainItems[0].id);
    }
  }, [tab, effectiveSelectedId, mainItems]);

  const startEdit = useCallback((item: ActionBarItem) => {
    setDraftParentId(undefined);
    setEditingId(item.id);
    const p = item.actionData.trim();
    const isExt = item.actionType === "script" && p.length > 0 && !p.startsWith("#") &&
      (p.startsWith("/") || /^[A-Za-z]:[\\/]/.test(p));
    setEditingForm({ ...item, actionType: isExt ? "extension" : item.actionType });
  }, []);

  const cancelEdit = useCallback(() => {
    setEditingId(null);
    setEditingForm({});
    setDraftParentId(undefined);
  }, []);

  const saveEdit = useCallback(async () => {
    try {
      if (editingForm.actionType === "extension") {
        const actionData = editingForm.actionData || "";
        const hasNewPkg = actionData.includes("|");
        const extName = editingForm.title || t("settings.actionBar.extName");
        const isAsync = editingForm.isAsync ?? true;
        const writeOutput = editingForm.writeOutputToClipboard ?? false;
        const isEnabled = editingForm.isEnabled ?? true;
        if (draftParentId !== undefined) {
          if (!hasNewPkg) { showToast(t("settings.actionBar.selectExtFirst")); return; }
          const [sourcePath, dirName] = actionData.split("|");
          const newId = await invoke<number>("install_extension", {
            sourcePath, dirName, name: extName, isAsync,
            writeOutputToClipboard: writeOutput, parentId: draftParentId, isEnabled,
          });
          await invoke("set_global_shortcut", { id: newId, globalShortcut: editingForm.globalShortcut ?? "" });
          showToast(t("settings.actionBar.created"));
        } else if (editingId) {
          if (hasNewPkg) {
            const [sourcePath, dirName] = actionData.split("|");
            await invoke("install_extension", {
              sourcePath, dirName, name: extName, isAsync,
              writeOutputToClipboard: writeOutput, parentId: editingForm.parentId ?? null,
              isEnabled, replaceId: editingId,
            });
          } else {
            await invoke("update_action_bar_item", {
              id: editingId, title: editingForm.title || "", icon: editingForm.icon || "",
              actionType: "script", actionData: editingForm.actionData || "",
              isEnabled, isAsync, writeOutputToClipboard: writeOutput, agent: "",
              accepts: "text", triggerKeyword: editingForm.triggerKeyword || "", appBundleIds: editingForm.appBundleIds ?? "",
            });
          }
          await invoke("set_global_shortcut", { id: editingId, globalShortcut: editingForm.globalShortcut ?? "" });
          showToast(t("settings.actionBar.saved"));
        }
      } else if (draftParentId !== undefined) {
        const createdId = await invoke<number>("create_action_bar_item", {
          parentId: draftParentId, title: editingForm.title || t("settings.actionBar.newMenuItem"),
          icon: "", actionType: editingForm.actionType || "url", actionData: editingForm.actionData || "",
          isAsync: editingForm.actionType === "script" ? (editingForm.isAsync ?? true) : true,
          writeOutputToClipboard: editingForm.actionType === "script" ? (editingForm.writeOutputToClipboard ?? false) : false,
          agent: editingForm.actionType === "agent" ? (editingForm.agent || "") : "",
          accepts: editingForm.actionType === "submenu" ? "any" : (editingForm.accepts || "text"),
          triggerKeyword: editingForm.actionType !== "submenu" ? (editingForm.triggerKeyword || "") : "",
          isEnabled: editingForm.isEnabled ?? true, appBundleIds: editingForm.appBundleIds ?? "",
        });
        if (editingForm.actionType !== "submenu") {
          await invoke("set_global_shortcut", { id: createdId, globalShortcut: editingForm.globalShortcut ?? "" });
        }
        showToast(t("settings.actionBar.created"));
      } else if (editingId) {
        await invoke("update_action_bar_item", {
          id: editingId, title: editingForm.title || "", icon: editingForm.icon || "",
          actionType: editingForm.actionType || "url", actionData: editingForm.actionData || "",
          isEnabled: editingForm.isEnabled ?? true,
          isAsync: editingForm.actionType === "script" ? (editingForm.isAsync ?? true) : true,
          writeOutputToClipboard: editingForm.actionType === "script" ? (editingForm.writeOutputToClipboard ?? false) : false,
          agent: editingForm.actionType === "agent" ? (editingForm.agent || "") : "",
          accepts: deriveAccepts(editingForm.actionType, editingForm.accepts),
          triggerKeyword: editingForm.actionType !== "submenu" ? (editingForm.triggerKeyword || "") : "",
          appBundleIds: editingForm.appBundleIds ?? "",
        });
        if (editingForm.actionType !== "submenu") {
          await invoke("set_global_shortcut", { id: editingId, globalShortcut: editingForm.globalShortcut ?? "" });
        }
        showToast(t("settings.actionBar.saved"));
      }
      cancelEdit();
      refresh();
    } catch (e) {
      showToast(t("settings.actionBar.saveFailed") + e);
    }
  }, [draftParentId, editingId, editingForm, showToast, cancelEdit, refresh, t]);

  const [deleteConfirmId, setDeleteConfirmId] = useState<number | null>(null);

  const handleDelete = useCallback(async (id: number) => {
    if (deleteConfirmId !== id) { setDeleteConfirmId(id); return; }
    try {
      await invoke("delete_action_bar_item", { id });
      showToast(t("settings.actionBar.deleted"));
      setDeleteConfirmId(null);
      refresh();
    } catch (e) { showToast(t("settings.actionBar.deleteFailed") + e); }
  }, [deleteConfirmId, showToast, refresh, t]);

  const handleMove = useCallback(async (id: number, direction: number) => {
    try { await invoke("move_action_bar_item", { id, direction }); refresh(); }
    catch (e) { showToast(t("settings.actionBar.moveFailed") + e); }
  }, [refresh, showToast, t]);

  const handleAdd = useCallback((parentId: number | null) => {
    setEditingId(null);
    setDraftParentId(parentId);
    setEditingForm({
      title: t("settings.actionBar.newMenuItem"),
      actionType: parentId === null ? "submenu" : "script",
      actionData: "", isEnabled: true,
      accepts: parentId === null ? "any" : (scopeFilter === "file" ? "file" : "text"),
    });
  }, [scopeFilter, t]);

  const updateItemInline = useCallback(async (item: ActionBarItem, patch: Partial<ActionBarItem>) => {
    const merged = { ...item, ...patch };
    try {
      await invoke("update_action_bar_item", {
        id: merged.id, title: merged.title, icon: merged.icon || "",
        actionType: merged.actionType || "url", actionData: merged.actionData || "",
        isEnabled: merged.isEnabled, isAsync: merged.isAsync ?? true,
        writeOutputToClipboard: merged.writeOutputToClipboard ?? false,
        agent: merged.agent || "", accepts: deriveAccepts(merged.actionType, merged.accepts),
        triggerKeyword: merged.triggerKeyword || "", appBundleIds: merged.appBundleIds ?? "",
      });
      if (merged.actionType !== "submenu") {
        await invoke("set_global_shortcut", { id: merged.id, globalShortcut: merged.globalShortcut ?? "" });
      }
      refresh();
    } catch (e) {
      showToast(t("settings.actionBar.saveFailed") + e);
      refresh();
    }
  }, [refresh, showToast, t]);

  const updateMainInline = useCallback((patch: Partial<ActionBarItem>) => {
    if (selectedMain === null) return;
    void updateItemInline(selectedMain, patch);
  }, [selectedMain, updateItemInline]);

  // inline 全局快捷键录制
  useEffect(() => {
    if (!inlineCapturingGlobal || selectedMain === null) return;
    const handler = async (e: KeyboardEvent) => {
      e.preventDefault(); e.stopPropagation();
      if (e.key === "Escape") { setInlineCapturingGlobal(false); return; }
      if (e.key === "Alt" || e.key === "Shift" || e.key === "Control" || e.key === "Meta") return;
      if (e.key === "Backspace" || e.key === "Delete") { updateMainInline({ globalShortcut: "" }); setInlineCapturingGlobal(false); return; }
      const parts: string[] = [];
      if (e.metaKey || e.ctrlKey) parts.push("CmdOrCtrl");
      if (e.altKey) parts.push("Alt");
      if (e.shiftKey) parts.push("Shift");
      const keyName = e.code.startsWith("Key") ? e.code.slice(3) : e.code;
      parts.push(keyName);
      const sc = parts.join("+");
      try { await invoke("check_shortcut", { shortcut: sc }); updateMainInline({ globalShortcut: sc }); }
      catch { /* ignore */ }
      setInlineCapturingGlobal(false);
    };
    document.addEventListener("keydown", handler, true);
    return () => document.removeEventListener("keydown", handler, true);
  }, [inlineCapturingGlobal, selectedMain, updateMainInline]);

  // 菜单项行内快捷键录制（主菜单 + 子菜单统一）
  useEffect(() => {
    if (capturingItem === null) return;
    const target = [...mainItems, ...selectedSubs].find((s) => s.id === capturingItem.id);
    if (!target) { setCapturingItem(null); return; }
    const handler = async (e: KeyboardEvent) => {
      e.preventDefault(); e.stopPropagation();
      if (e.key === "Escape") { setCapturingItem(null); return; }
      if (e.key === "Alt" || e.key === "Shift" || e.key === "Control" || e.key === "Meta") return;
      if (e.key === "Backspace" || e.key === "Delete") { updateItemInline(target, { globalShortcut: "" }); setCapturingItem(null); return; }
      const parts: string[] = [];
      if (e.metaKey || e.ctrlKey) parts.push("CmdOrCtrl");
      if (e.altKey) parts.push("Alt");
      if (e.shiftKey) parts.push("Shift");
      const keyName = e.code.startsWith("Key") ? e.code.slice(3) : e.code;
      parts.push(keyName);
      const sc = parts.join("+");
      try { await invoke("check_shortcut", { shortcut: sc }); updateItemInline(target, { globalShortcut: sc }); }
      catch { /* ignore */ }
      setCapturingItem(null);
    };
    document.addEventListener("keydown", handler, true);
    return () => document.removeEventListener("keydown", handler, true);
  }, [capturingItem, mainItems, selectedSubs, updateItemInline]);

  // 标题 draft debounce
  useEffect(() => {
    if (titleDraft === null) return;
    const timer = setTimeout(() => {
      const draft = titleDraftRef.current;
      if (draft !== null) { updateMainInline({ title: draft }); setTitleDraft(null); }
    }, 300);
    return () => clearTimeout(timer);
  }, [titleDraft, updateMainInline]);

  useEffect(() => { setTitleDraft(null); }, [effectiveSelectedId]);

  const isEditing = editingId !== null || draftParentId !== undefined;

  return (
    <div className={cn("w-full min-w-0", isEditing && "h-full flex flex-col")}>
      {!isEditing && (
        <UnderlineTabs
          items={[
            { key: "menu", label: t("settings.actionBar.menuManage") },
            { key: "runs", label: t("settings.actionBar.scriptRecords") },
          ]}
          active={tab}
          onChange={(k) => setTab(k as "menu" | "runs")}
          className="mb-4"
        />
      )}

      {isEditing ? (
        <EditForm
          form={editingForm}
          isSystem={(editingId !== null && items.find((i) => i.id === editingId)?.isSystem) ?? false}
          onChange={setEditingForm}
          onSave={saveEdit}
          onCancel={cancelEdit}
        />
      ) : tab === "runs" ? (
        <ScriptRunsList showToast={showToast} />
      ) : !loaded ? (
        <p className="py-12 text-center text-sm text-muted-foreground">
          {t("settings.actionBar.loadingRecords")}
        </p>
      ) : mainItems.length === 0 ? (
        <div className="flex flex-col items-center justify-center gap-3 py-20 text-center">
          <div className="flex h-12 w-12 items-center justify-center rounded-full bg-voice/8">
            <Plus className="h-5 w-5 text-voice/60" />
          </div>
          <div>
            <p className="text-sm font-medium">{t("settings.actionBar.noItemsYet")}</p>
            <p className="mt-0.5 text-xs text-muted-foreground">{t("settings.actionBar.noItemsHint")}</p>
          </div>
          <Button onClick={() => handleAdd(null)} variant="voice" size="default">
            <Plus /> {t("settings.actionBar.addMainItem")}
          </Button>
        </div>
      ) : (
        <div className="flex gap-4">
          {/* 左栏：主菜单列表 */}
          <div className="flex w-52 shrink-0 flex-col gap-2">
            <Segmented
              items={[
                { key: "text", label: t("settings.actionBar.scopeText") },
                { key: "file", label: t("settings.actionBar.scopeFile") },
              ]}
              active={scopeFilter}
              onChange={(k) => setScopeFilter(k as "text" | "file")}
            />
            <Button onClick={() => handleAdd(null)} variant="voice" size="sm" className="w-full">
              <Plus /> {t("settings.actionBar.addMainItem")}
            </Button>
            <div className="min-h-0 flex-1 space-y-px overflow-y-auto">
              {mainItems.map((item, i) => (
                <MenuRow
                  key={item.id}
                  item={item}
                  index={i + 1}
                  selected={effectiveSelectedId === item.id}
                  isFirst={i === 0}
                  isLast={i === mainItems.length - 1}
                  deleteConfirmId={deleteConfirmId}
                  isMain
                  onSelect={() => setSelectedMainMenuId(item.id)}
                  onMove={(dir) => handleMove(item.id, dir)}
                  onEdit={() => startEdit(item)}
                  onDelete={() => handleDelete(item.id)}
                />
              ))}
            </div>
          </div>

          {/* 右栏：选中主菜单 inline 编辑 + 子菜单列表 */}
          <div className="min-w-0 flex-1">
            {selectedMain === null ? (
              <div className="flex h-full items-center justify-center py-20 text-sm text-muted-foreground">
                {t("settings.actionBar.selectMenuHint")}
              </div>
            ) : (
              <div className="space-y-4">
                {/* 主菜单 inline 编辑表单 */}
                <div className="space-y-3 rounded-lg border border-border/50 bg-muted/15 p-4">
                  <div className="flex items-center gap-2">
                    <input
                      className={cn(inputBase, "flex-1")}
                      value={titleDraft ?? selectedMain.title}
                      maxLength={12}
                      placeholder={t("settings.actionBar.titleLabel")}
                      onChange={(e) => {
                        const MAX = 12;
                        const raw = e.target.value;
                        let weight = 0;
                        let ok = "";
                        for (const ch of raw) {
                          const w = /[\u4e00-\u9fff\u3040-\u30ff\uac00-\ud7af]/.test(ch) ? 2 : 1;
                          if (weight + w > MAX) break;
                          weight += w;
                          ok += ch;
                        }
                        setTitleDraft(ok);
                      }}
                      onKeyDown={(e) => {
                        if (e.key === "Enter" && titleDraft !== null) {
                          e.preventDefault();
                          updateMainInline({ title: titleDraft });
                          setTitleDraft(null);
                        }
                      }}
                    />
                    <Button
                      variant="voice"
                      size="sm"
                      disabled={titleDraft === null}
                      onClick={() => {
                        if (titleDraft !== null) {
                          updateMainInline({ title: titleDraft });
                          setTitleDraft(null);
                        }
                      }}
                    >
                      {t("settings.actionBar.save")}
                    </Button>
                    <div className="flex items-center gap-1.5">
                      <Toggle
                        checked={selectedMain.isEnabled}
                        onChange={(v) => updateMainInline({ isEnabled: v })}
                      />
                    </div>
                  </div>

                  {selectedMain.actionType !== "submenu" && (
                    <div className="flex items-start justify-between gap-4">
                      {/* 斜杠命令名（左） */}
                      <div className="space-y-1.5">
                        <label className="block text-[11px] font-medium uppercase tracking-wide text-muted-foreground/80">
                          {t("settings.actionBar.slashName")}
                        </label>
                        <input
                          className="w-28 bg-background border border-border rounded-md px-3 py-1.5 text-sm font-mono outline-none transition-all focus:border-voice/50 focus:ring-2 focus:ring-voice/15"
                          placeholder={t("settings.actionBar.slashNamePlaceholder")}
                          value={selectedMain.triggerKeyword || ""}
                          onChange={(e) => {
                            const val = e.target.value.trim().toLowerCase();
                            updateMainInline({ triggerKeyword: val });
                          }}
                        />
                      </div>
                      {/* 全局快捷键（右） */}
                      <div className="flex flex-col items-end gap-1.5">
                        <label className="block text-[11px] font-medium uppercase tracking-wide text-muted-foreground/80">
                          {ti18n("settings.actionBar.globalShortcutLabel")}
                        </label>
                        <div className="flex items-center gap-1">
                          <ShortcutButton
                            shortcut={selectedMain.globalShortcut ?? ""}
                            capturing={inlineCapturingGlobal}
                            onClick={() => setInlineCapturingGlobal((v) => !v)}
                          />
                          {selectedMain.globalShortcut && (
                            <button
                              onClick={() => updateMainInline({ globalShortcut: "" })}
                              className="rounded p-1 text-muted-foreground/60 transition-colors hover:bg-destructive/10 hover:text-destructive"
                              aria-label={ti18n("settings.actionBar.clearShortcut")}
                            >
                              <X className="h-3.5 w-3.5" />
                            </button>
                          )}
                        </div>
                      </div>
                    </div>
                  )}
                </div>

                {/* 子菜单列表（仅 submenu 类型展示） */}
                {selectedMain.actionType === "submenu" ? (
                  <div className="space-y-2">
                    <div className="flex items-center justify-between">
                      <h4 className="text-xs font-medium uppercase tracking-wide text-muted-foreground/80">
                        {t("settings.actionBar.subItemsTitle")}
                      </h4>
                      <Button onClick={() => handleAdd(selectedMain.id)} variant="outline" size="sm">
                        <Plus /> {t("settings.actionBar.addSubItem")}
                      </Button>
                    </div>
                    {selectedSubs.length === 0 ? (
                      <div className="flex items-center justify-center rounded-md border border-dashed border-border py-8 text-xs text-muted-foreground">
                        {t("settings.actionBar.noSubItemsHint")}
                      </div>
                    ) : (
                      <div className="space-y-px">
                        {selectedSubs.map((sub, i) => (
                          <MenuRow
                            key={sub.id}
                            item={sub}
                            index={i + 1}
                            selected={false}
                            isFirst={i === 0}
                            isLast={i === selectedSubs.length - 1}
                            deleteConfirmId={deleteConfirmId}
                            onMove={(dir) => handleMove(sub.id, dir)}
                            onEdit={() => startEdit(sub)}
                            onDelete={() => handleDelete(sub.id)}
                            showShortcuts
                            capturing={capturingItem?.id === sub.id}
                            onCaptureShortcut={() => setCapturingItem({ id: sub.id, kind: "global" })}
                            onClearShortcut={() => updateItemInline(sub, { globalShortcut: "" })}
                          />
                        ))}
                      </div>
                    )}
                  </div>
                ) : (
                  <div className="flex items-center justify-center rounded-md border border-dashed border-border py-8 text-xs text-muted-foreground">
                    {t("settings.actionBar.leafNoSubItemsHint")}
                  </div>
                )}
              </div>
            )}
          </div>
        </div>
      )}
    </div>
  );
}
