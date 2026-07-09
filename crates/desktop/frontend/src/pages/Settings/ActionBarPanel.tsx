import { useState, useEffect, useCallback } from "react";
import { invoke } from "@tauri-apps/api/core";
import { ChevronUp, ChevronDown, Pencil, Trash2, Plus } from "lucide-react";
import { ActionBarIcon } from "@/components/ActionBarIcon";

interface ActionBarItem {
  id: number;
  parentId: number | null;
  title: string;
  icon: string;
  actionType: string;
  actionData: string;
  sortOrder: number;
  isSystem: boolean;
  isEnabled: boolean;
}

const ACTION_TYPES = [
  { value: "submenu", label: "子菜单", placeholder: "" },
  { value: "ai", label: "AI（LLM 处理）", placeholder: "system prompt，或 auto_translate" },
  { value: "url", label: "URL（打开网页/应用）", placeholder: "https://... 或 app://?text={text}（留空=选中文本即URL）" },
  { value: "script", label: "脚本", placeholder: "#shell / #osascript / #powershell / #python\n第一行后写脚本，{text}=选中文本" },
  { value: "copy", label: "复制", placeholder: "" },
];

export default function ActionBarPanel({ showToast }: { showToast: (msg: string) => void }) {
  const [items, setItems] = useState<ActionBarItem[]>([]);
  const [editingId, setEditingId] = useState<number | null>(null);
  const [editingForm, setEditingForm] = useState<Partial<ActionBarItem>>({});
  const [showAddParent, setShowAddParent] = useState(false);

  const refresh = useCallback(() => {
    invoke<ActionBarItem[]>("list_action_bar_items").then(setItems).catch(() => {});
  }, []);

  useEffect(() => { refresh(); }, [refresh]);

  const mainItems = items.filter((i) => i.parentId === null);
  const getSubs = (parentId: number) => items.filter((i) => i.parentId === parentId);

  const startEdit = (item: ActionBarItem) => {
    setEditingId(item.id);
    setEditingForm({ ...item });
  };

  const cancelEdit = () => {
    setEditingId(null);
    setEditingForm({});
  };

  const saveEdit = async () => {
    if (!editingId) return;
    try {
      await invoke("update_action_bar_item", {
        id: editingId,
        title: editingForm.title || "",
        icon: editingForm.icon || "",
        actionType: editingForm.actionType || "copy",
        actionData: editingForm.actionData || "",
        isEnabled: editingForm.isEnabled ?? true,
      });
      showToast("已保存");
      cancelEdit();
      refresh();
    } catch (e) {
      showToast("保存失败：" + e);
    }
  };

  const handleDelete = async (id: number) => {
    try {
      await invoke("delete_action_bar_item", { id });
      showToast("已删除");
      refresh();
    } catch (e) {
      showToast("删除失败：" + e);
    }
  };

  const handleMove = async (id: number, direction: number) => {
    try {
      await invoke("move_action_bar_item", { id, direction });
      refresh();
    } catch (e) {
      showToast("移动失败：" + e);
    }
  };

  const handleAdd = async (parentId: number | null) => {
    try {
      const id = await invoke<number>("create_action_bar_item", {
        parentId,
        title: "新菜单项",
        icon: "",
        actionType: "copy",
        actionData: "",
      });
      setShowAddParent(false);
      refresh();
      // 自动进入编辑
      const newItem = items.find((i) => i.id === id);
      if (newItem) startEdit(newItem);
    } catch (e) {
      showToast("新增失败：" + e);
    }
  };

  const ItemRow = ({ item, siblings }: { item: ActionBarItem; siblings: ActionBarItem[] }) => {
    const isFirst = siblings[0]?.id === item.id;
    const isLast = siblings[siblings.length - 1]?.id === item.id;
    const subs = getSubs(item.id);
    const isEditing = editingId === item.id;

    return (
      <div>
        <div className="flex items-center gap-2 py-1.5 px-2 hover:bg-muted/40 rounded">
          <ActionBarIcon icon={item.icon || "search"} className="w-4 h-4 text-muted-foreground" />
          <span className="text-sm flex-1 truncate">{item.title}</span>
          <span className="text-[10px] text-muted-foreground bg-muted px-1.5 py-0.5 rounded">{item.actionType}</span>
          {!item.isEnabled && <span className="text-[10px] text-red-400">禁用</span>}
          <div className="flex items-center gap-0.5">
            <button onClick={() => handleMove(item.id, -1)} disabled={isFirst} className="p-0.5 hover:text-foreground disabled:opacity-30">
              <ChevronUp className="w-3.5 h-3.5" />
            </button>
            <button onClick={() => handleMove(item.id, 1)} disabled={isLast} className="p-0.5 hover:text-foreground disabled:opacity-30">
              <ChevronDown className="w-3.5 h-3.5" />
            </button>
            <button onClick={() => startEdit(item)} className="p-0.5 hover:text-foreground">
              <Pencil className="w-3.5 h-3.5" />
            </button>
            <button onClick={() => handleDelete(item.id)} disabled={item.isSystem} className="p-0.5 hover:text-red-500 disabled:opacity-30">
              <Trash2 className="w-3.5 h-3.5" />
            </button>
          </div>
        </div>

        {isEditing && (
          <div className="ml-6 mb-2 p-3 bg-muted/30 rounded border border-border space-y-2">
            <div className="flex items-center gap-2">
              <label className="text-xs text-muted-foreground w-10 shrink-0">标题</label>
              <input
                className="flex-1 bg-background border border-border rounded px-2 py-1 text-sm"
                value={editingForm.title || ""}
                onChange={(e) => setEditingForm({ ...editingForm, title: e.target.value })}
              />
            </div>
            <div className="flex items-start gap-2">
              <label className="text-xs text-muted-foreground w-10 shrink-0 mt-1.5">图标</label>
              <div className="flex-1 flex items-center gap-2">
                <input
                  className="flex-1 bg-background border border-border rounded px-2 py-1 text-sm font-mono"
                  placeholder="图标名（如 search）或 <svg>...</svg>"
                  value={editingForm.icon || ""}
                  onChange={(e) => setEditingForm({ ...editingForm, icon: e.target.value })}
                />
                {editingForm.icon && (
                  <div className="w-6 h-6 flex items-center justify-center text-muted-foreground">
                    <ActionBarIcon icon={editingForm.icon} className="w-4 h-4" />
                  </div>
                )}
              </div>
            </div>
            <div className="flex items-center gap-2">
              <label className="text-xs text-muted-foreground w-10 shrink-0">类型</label>
              <select
                className="flex-1 bg-background border border-border rounded px-2 py-1 text-sm"
                value={editingForm.actionType || "copy"}
                disabled={item.isSystem}
                onChange={(e) => setEditingForm({ ...editingForm, actionType: e.target.value })}
              >
                {ACTION_TYPES.map((t) => (
                  <option key={t.value} value={t.value}>{t.label}</option>
                ))}
              </select>
            </div>
            {(editingForm.actionType !== "submenu" && editingForm.actionType !== "copy") && (
              <div className="flex items-start gap-2">
                <label className="text-xs text-muted-foreground w-10 shrink-0 mt-1.5">内容</label>
                <textarea
                  className="flex-1 bg-background border border-border rounded px-2 py-1 text-sm font-mono min-h-[60px]"
                  placeholder={ACTION_TYPES.find((t) => t.value === editingForm.actionType)?.placeholder}
                  value={editingForm.actionData || ""}
                  onChange={(e) => setEditingForm({ ...editingForm, actionData: e.target.value })}
                />
              </div>
            )}
            <div className="flex items-center gap-2">
              <label className="text-xs text-muted-foreground w-10 shrink-0">启用</label>
              <input
                type="checkbox"
                checked={editingForm.isEnabled ?? true}
                onChange={(e) => setEditingForm({ ...editingForm, isEnabled: e.target.checked })}
              />
            </div>
            <div className="flex items-center gap-2 pt-1">
              <button onClick={saveEdit} className="px-3 py-1 text-xs bg-voice text-white rounded hover:opacity-90">保存</button>
              <button onClick={cancelEdit} className="px-3 py-1 text-xs bg-muted text-foreground rounded hover:bg-muted/70">取消</button>
            </div>
          </div>
        )}

        {/* 子菜单项 */}
        {subs.length > 0 && (
          <div className="ml-4 border-l border-border/40 pl-2">
            {subs.map((sub) => (
              <ItemRow key={sub.id} item={sub} siblings={subs} />
            ))}
            <button
              onClick={() => handleAdd(item.id)}
              className="flex items-center gap-1 py-1 px-2 text-xs text-muted-foreground hover:text-foreground"
            >
              <Plus className="w-3 h-3" /> 新增子项
            </button>
          </div>
        )}
        {item.actionType === "submenu" && subs.length === 0 && (
          <div className="ml-4 border-l border-border/40 pl-2">
            <button
              onClick={() => handleAdd(item.id)}
              className="flex items-center gap-1 py-1 px-2 text-xs text-muted-foreground hover:text-foreground"
            >
              <Plus className="w-3 h-3" /> 新增子项
            </button>
          </div>
        )}
      </div>
    );
  };

  return (
    <div className="max-w-2xl">
      <div className="flex items-center justify-between mb-4">
        <h2 className="text-lg font-semibold">AI 命令面板菜单</h2>
        <button
          onClick={() => setShowAddParent(!showAddParent)}
          className="flex items-center gap-1 px-3 py-1.5 text-sm bg-voice text-white rounded-md hover:opacity-90"
        >
          <Plus className="w-4 h-4" /> 新增主菜单项
        </button>
      </div>

      {showAddParent && (
        <div className="mb-3 p-3 bg-muted/30 rounded border border-border">
          <p className="text-xs text-muted-foreground mb-2">确认新增一个主菜单项？</p>
          <div className="flex gap-2">
            <button onClick={() => handleAdd(null)} className="px-3 py-1 text-xs bg-voice text-white rounded">确认</button>
            <button onClick={() => setShowAddParent(false)} className="px-3 py-1 text-xs bg-muted rounded">取消</button>
          </div>
        </div>
      )}

      <div className="space-y-0.5">
        {mainItems.map((item) => (
          <ItemRow key={item.id} item={item} siblings={mainItems} />
        ))}
      </div>

      {items.length === 0 && (
        <p className="text-sm text-muted-foreground text-center py-8">加载中…</p>
      )}
    </div>
  );
}
