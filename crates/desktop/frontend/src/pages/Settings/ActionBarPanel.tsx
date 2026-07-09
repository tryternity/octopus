import { useState, useEffect, useCallback, memo } from "react";
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

// ── 编辑表单（独立组件，避免每帧重建）──

interface EditFormProps {
  form: Partial<ActionBarItem>;
  isSystem: boolean;
  onChange: (form: Partial<ActionBarItem>) => void;
  onSave: () => void;
  onCancel: () => void;
}

const EditForm = ({ form, isSystem, onChange, onSave, onCancel }: EditFormProps) => (
  <div className="ml-6 mb-2 p-3 bg-muted/30 rounded border border-border space-y-2">
    <div className="flex items-center gap-2">
      <label className="text-xs text-muted-foreground w-10 shrink-0">标题</label>
      <input
        className="flex-1 bg-background border border-border rounded px-2 py-1 text-sm"
        value={form.title || ""}
        onChange={(e) => onChange({ ...form, title: e.target.value })}
      />
    </div>
    <div className="flex items-start gap-2">
      <label className="text-xs text-muted-foreground w-10 shrink-0 mt-1.5">图标</label>
      <div className="flex-1 flex items-center gap-2">
        <input
          className="flex-1 bg-background border border-border rounded px-2 py-1 text-sm font-mono"
          placeholder="图标名（如 search）或 <svg>...</svg>"
          value={form.icon || ""}
          onChange={(e) => onChange({ ...form, icon: e.target.value })}
        />
        {form.icon && (
          <div className="w-6 h-6 flex items-center justify-center text-muted-foreground">
            <ActionBarIcon icon={form.icon} className="w-4 h-4" />
          </div>
        )}
      </div>
    </div>
    <div className="flex items-center gap-2">
      <label className="text-xs text-muted-foreground w-10 shrink-0">类型</label>
      <select
        className="flex-1 bg-background border border-border rounded px-2 py-1 text-sm"
        value={form.actionType || "copy"}
        disabled={isSystem}
        onChange={(e) => onChange({ ...form, actionType: e.target.value })}
      >
        {ACTION_TYPES.map((t) => (
          <option key={t.value} value={t.value}>{t.label}</option>
        ))}
      </select>
    </div>
    {form.actionType !== "submenu" && form.actionType !== "copy" && (
      <div className="flex items-start gap-2">
        <label className="text-xs text-muted-foreground w-10 shrink-0 mt-1.5">内容</label>
        <textarea
          className="flex-1 bg-background border border-border rounded px-2 py-1 text-sm font-mono min-h-[60px]"
          placeholder={ACTION_TYPES.find((t) => t.value === form.actionType)?.placeholder}
          value={form.actionData || ""}
          onChange={(e) => onChange({ ...form, actionData: e.target.value })}
        />
      </div>
    )}
    <div className="flex items-center gap-2">
      <label className="text-xs text-muted-foreground w-10 shrink-0">启用</label>
      <input
        type="checkbox"
        checked={form.isEnabled ?? true}
        onChange={(e) => onChange({ ...form, isEnabled: e.target.checked })}
      />
    </div>
    <div className="flex items-center gap-2 pt-1">
      <button onClick={onSave} className="px-3 py-1 text-xs bg-voice text-white rounded hover:opacity-90">保存</button>
      <button onClick={onCancel} className="px-3 py-1 text-xs bg-muted text-foreground rounded hover:bg-muted/70">取消</button>
    </div>
  </div>
);

// ── 菜单行（memo 化，props 不变时不重渲染）──

interface ItemRowProps {
  item: ActionBarItem;
  siblings: ActionBarItem[];
  allItems: ActionBarItem[];
  editingId: number | null;
  editingForm: Partial<ActionBarItem>;
  onMove: (id: number, direction: number) => void;
  onStartEdit: (item: ActionBarItem) => void;
  onDelete: (id: number) => void;
  onAdd: (parentId: number | null) => void;
  onFormChange: (form: Partial<ActionBarItem>) => void;
  onSaveEdit: () => void;
  onCancelEdit: () => void;
}

const ItemRowBase = ({ item, siblings, allItems, editingId, editingForm, onMove, onStartEdit, onDelete, onAdd, onFormChange, onSaveEdit, onCancelEdit }: ItemRowProps) => {
  const isFirst = siblings[0]?.id === item.id;
  const isLast = siblings[siblings.length - 1]?.id === item.id;
  const subs = allItems.filter((i) => i.parentId === item.id);
  const isEditing = editingId === item.id;

  return (
    <div>
      <div className="flex items-center gap-2 py-1.5 px-2 hover:bg-muted/40 rounded">
        <ActionBarIcon icon={item.icon || "search"} className="w-4 h-4 text-muted-foreground" />
        <span className="text-sm flex-1 truncate">{item.title}</span>
        <span className="text-[10px] text-muted-foreground bg-muted px-1.5 py-0.5 rounded">{item.actionType}</span>
        {!item.isEnabled && <span className="text-[10px] text-red-400">禁用</span>}
        <div className="flex items-center gap-0.5">
          <button onClick={() => onMove(item.id, -1)} disabled={isFirst} className="p-0.5 hover:text-foreground disabled:opacity-30">
            <ChevronUp className="w-3.5 h-3.5" />
          </button>
          <button onClick={() => onMove(item.id, 1)} disabled={isLast} className="p-0.5 hover:text-foreground disabled:opacity-30">
            <ChevronDown className="w-3.5 h-3.5" />
          </button>
          <button onClick={() => onStartEdit(item)} className="p-0.5 hover:text-foreground">
            <Pencil className="w-3.5 h-3.5" />
          </button>
          <button onClick={() => onDelete(item.id)} disabled={item.isSystem} className="p-0.5 hover:text-red-500 disabled:opacity-30">
            <Trash2 className="w-3.5 h-3.5" />
          </button>
        </div>
      </div>

      {isEditing && (
        <EditForm
          form={editingForm}
          isSystem={item.isSystem}
          onChange={onFormChange}
          onSave={onSaveEdit}
          onCancel={onCancelEdit}
        />
      )}

      {subs.length > 0 && (
        <div className="ml-4 border-l border-border/40 pl-2">
          {subs.map((sub) => (
            <ItemRow key={sub.id} item={sub} siblings={subs} {...{ allItems, editingId, editingForm, onMove, onStartEdit, onDelete, onAdd, onFormChange, onSaveEdit, onCancelEdit }} />
          ))}
          <button onClick={() => onAdd(item.id)} className="flex items-center gap-1 py-1 px-2 text-xs text-muted-foreground hover:text-foreground">
            <Plus className="w-3 h-3" /> 新增子项
          </button>
        </div>
      )}
      {item.actionType === "submenu" && subs.length === 0 && (
        <div className="ml-4 border-l border-border/40 pl-2">
          <button onClick={() => onAdd(item.id)} className="flex items-center gap-1 py-1 px-2 text-xs text-muted-foreground hover:text-foreground">
            <Plus className="w-3 h-3" /> 新增子项
          </button>
        </div>
      )}
    </div>
  );
};

const ItemRow = memo(ItemRowBase);

// ── 主面板 ──

export default function ActionBarPanel({ showToast }: { showToast: (msg: string) => void }) {
  const [items, setItems] = useState<ActionBarItem[]>([]);
  const [editingId, setEditingId] = useState<number | null>(null);
  const [editingForm, setEditingForm] = useState<Partial<ActionBarItem>>({});
  const [showAddParent, setShowAddParent] = useState(false);

  const refresh = useCallback(async (): Promise<ActionBarItem[]> => {
    const items = await invoke<ActionBarItem[]>("list_action_bar_items");
    setItems(items);
    return items;
  }, []);

  useEffect(() => { refresh(); }, [refresh]);

  const mainItems = items.filter((i) => i.parentId === null);

  const startEdit = useCallback((item: ActionBarItem) => {
    setEditingId(item.id);
    setEditingForm({ ...item });
  }, []);

  const cancelEdit = useCallback(() => {
    setEditingId(null);
    setEditingForm({});
  }, []);

  const saveEdit = useCallback(async () => {
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
  }, [editingId, editingForm, showToast, cancelEdit, refresh]);

  const handleDelete = useCallback(async (id: number) => {
    try {
      await invoke("delete_action_bar_item", { id });
      showToast("已删除");
      refresh();
    } catch (e) {
      showToast("删除失败：" + e);
    }
  }, [showToast, refresh]);

  const handleMove = useCallback(async (id: number, direction: number) => {
    try {
      await invoke("move_action_bar_item", { id, direction });
      refresh();
    } catch (e) {
      showToast("移动失败：" + e);
    }
  }, [refresh, showToast]);

  const handleAdd = useCallback(async (parentId: number | null) => {
    try {
      const id = await invoke<number>("create_action_bar_item", {
        parentId,
        title: "新菜单项",
        icon: "",
        actionType: "copy",
        actionData: "",
      });
      setShowAddParent(false);
      const latestItems = await refresh();
      const newItem = latestItems.find((i) => i.id === id);
      if (newItem) startEdit(newItem);
    } catch (e) {
      showToast("新增失败：" + e);
    }
  }, [refresh, startEdit, showToast]);

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
          <ItemRow
            key={item.id}
            item={item}
            siblings={mainItems}
            allItems={items}
            editingId={editingId}
            editingForm={editingForm}
            onMove={handleMove}
            onStartEdit={startEdit}
            onDelete={handleDelete}
            onAdd={handleAdd}
            onFormChange={setEditingForm}
            onSaveEdit={saveEdit}
            onCancelEdit={cancelEdit}
          />
        ))}
      </div>

      {items.length === 0 && (
        <p className="text-sm text-muted-foreground text-center py-8">加载中…</p>
      )}
    </div>
  );
}
