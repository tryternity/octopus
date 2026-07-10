import { useState, useEffect, useCallback, memo } from "react";
import { invoke } from "@tauri-apps/api/core";
import {
  ChevronRight,
  ChevronDown,
  ChevronsUpDown,
  ChevronsDownUp,
  ArrowUp,
  ArrowDown,
  Trash2,
  Plus,
  X,
} from "lucide-react";
import { cn } from "@/lib/utils";

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
  isAsync?: boolean;
  writeOutputToClipboard?: boolean;
}

// 动作类型元信息：颜色点 + 标签 + 说明 + 占位符。
// 颜色用 Tailwind 默认调色板的中饱和值，在三套主题（暖纸 / 黑曜 / 北欧）下都可读。
const TYPE_META: Record<
  string,
  { dot: string; label: string; desc: string; placeholder: string }
> = {
  submenu: {
    dot: "bg-voice",
    label: "SUBMENU",
    desc: "本身不执行动作，展开后显示其子菜单项",
    placeholder: "",
  },
  ai: {
    dot: "bg-violet-500",
    label: "AI",
    desc: "选中文本发送给 LLM，结果展示在浮窗。填 auto_translate 自动判断中英互译",
    placeholder: "system prompt，或 auto_translate",
  },
  url: {
    dot: "bg-sky-500",
    label: "URL",
    desc: "默认浏览器打开，{text} 会被替换为 URL 编码后的选中文本",
    placeholder: "https://... 或 app://?text={text}（留空=选中文本即 URL）",
  },
  script: {
    dot: "bg-emerald-500",
    label: "SCRIPT",
    desc: "首行 #shell / #osascript / #powershell / #python / #node / #deno / #bun / #javascript / #typescript；选中文本经 $OCTOPUS_TEXT 传入",
    placeholder:
      "#shell / #osascript / #powershell / #python\n#node / #deno / #bun\n#javascript / #typescript\n选中文本在 $OCTOPUS_TEXT 环境变量中",
  },
  copy: {
    dot: "bg-stone-400",
    label: "COPY",
    desc: "将选中文本复制到剪贴板",
    placeholder: "",
  },
};

const ACTION_TYPES = [
  { value: "submenu", label: "子菜单" },
  { value: "ai", label: "AI（LLM 处理）" },
  { value: "url", label: "URL（打开网页/应用）" },
  { value: "script", label: "脚本" },
  { value: "copy", label: "复制" },
];

const pad2 = (n: number) => String(n).padStart(2, "0");

// ── 类型标签：小色点 + 等宽大写名 ──
const TypeTag = ({ type }: { type: string }) => {
  const meta = TYPE_META[type] ?? TYPE_META.copy;
  return (
    <span className="inline-flex items-center gap-1 font-mono text-[10px] uppercase tracking-wider text-muted-foreground">
      <span className={cn("h-1.5 w-1.5 rounded-full", meta.dot)} />
      {meta.label}
    </span>
  );
};

// ── 启用开关（细条样式，比原生 checkbox 更克制）──
const Toggle = ({
  checked,
  onChange,
}: {
  checked: boolean;
  onChange: (v: boolean) => void;
}) => (
  <button
    type="button"
    role="switch"
    aria-checked={checked}
    onClick={() => onChange(!checked)}
    className={cn(
      "relative inline-flex h-4 w-7 items-center rounded-full transition-colors",
      checked ? "bg-voice" : "bg-muted-foreground/30",
    )}
  >
    <span
      className={cn(
        "inline-block h-3 w-3 transform rounded-full bg-white transition-transform",
        checked ? "translate-x-3.5" : "translate-x-0.5",
      )}
    />
  </button>
);

// ── 编辑表单：作为树节点的内联展开卡片 ──
interface EditFormProps {
  form: Partial<ActionBarItem>;
  isSystem: boolean;
  onChange: (form: Partial<ActionBarItem>) => void;
  onSave: () => void;
  onCancel: () => void;
}

const EditForm = ({
  form,
  isSystem,
  onChange,
  onSave,
  onCancel,
}: EditFormProps) => {
  const type = form.actionType || "copy";
  const meta = TYPE_META[type];
  const showContent = type !== "submenu" && type !== "copy";

  return (
    <div className="mb-1 ml-[26px] rounded-lg border border-border bg-muted/20 p-3.5 shadow-sm">
      <div className="mb-3 flex items-center justify-between">
        <span className="font-mono text-[10px] uppercase tracking-widest text-muted-foreground">
          编辑菜单项
        </span>
        <button
          onClick={onCancel}
          className="text-muted-foreground hover:text-foreground"
          aria-label="取消"
        >
          <X className="h-3.5 w-3.5" />
        </button>
      </div>

      <div className="space-y-3">
        <Field label="标题">
          <input
            className="w-full bg-background border border-border rounded px-2.5 py-1.5 text-sm outline-none focus:border-voice/50"
            value={form.title || ""}
            maxLength={6}
            onChange={(e) => {
              // 汉字算 2 字符、ASCII 算 1，总权重上限 6
              const MAX = 6;
              const raw = e.target.value;
              let weight = 0;
              let ok = "";
              for (const ch of raw) {
                const w = /[\u4e00-\u9fff\u3040-\u30ff\uac00-\ud7af]/.test(ch) ? 2 : 1;
                if (weight + w > MAX) break;
                weight += w;
                ok += ch;
              }
              onChange({ ...form, title: ok });
            }}
          />
        </Field>

        <Field label="类型">
          <div>
            <select
              className="w-full bg-background border border-border rounded px-2.5 py-1.5 text-sm outline-none focus:border-voice/50 disabled:opacity-60"
              value={type}
              disabled={isSystem}
              onChange={(e) =>
                onChange({ ...form, actionType: e.target.value })
              }
            >
              {ACTION_TYPES.map((t) => (
                <option key={t.value} value={t.value}>
                  {t.label}
                </option>
              ))}
            </select>
            <p className="mt-1 text-[11px] text-muted-foreground/80">
              {meta.desc}
              {isSystem && " · 内置项类型不可更改"}
            </p>
          </div>
        </Field>

        {showContent && (
          <Field label="内容">
            <textarea
              className="w-full min-h-[120px] resize-y bg-background border border-border rounded px-2.5 py-1.5 font-mono text-xs leading-relaxed outline-none focus:border-voice/50"
              placeholder={meta.placeholder}
              value={form.actionData || ""}
              onChange={(e) =>
                onChange({ ...form, actionData: e.target.value })
              }
            />
          </Field>
        )}

        {type === "script" && (
          <Field label="执行选项">
            <div className="space-y-2">
              <div className="flex items-center gap-2">
                <Toggle
                  checked={form.isAsync ?? true}
                  onChange={(v) =>
                    onChange({
                      ...form,
                      isAsync: v,
                      writeOutputToClipboard: v ? false : form.writeOutputToClipboard,
                    })
                  }
                />
                <span className="text-xs text-muted-foreground">
                  异步执行（不等待结果，后台运行）
                </span>
              </div>
              {!(form.isAsync ?? true) && (
                <div className="flex items-center gap-2">
                  <Toggle
                    checked={form.writeOutputToClipboard ?? false}
                    onChange={(v) =>
                      onChange({ ...form, writeOutputToClipboard: v })
                    }
                  />
                  <span className="text-xs text-muted-foreground">
                    结果写入剪贴板
                  </span>
                </div>
              )}
            </div>
          </Field>
        )}

        <Field label="启用">
          <div className="flex items-center gap-2">
            <Toggle
              checked={form.isEnabled ?? true}
              onChange={(v) => onChange({ ...form, isEnabled: v })}
            />
            <span className="text-xs text-muted-foreground">
              {form.isEnabled ? "显示在菜单中" : "已隐藏"}
            </span>
          </div>
        </Field>
      </div>

      <div className="mt-3.5 flex justify-end gap-2 border-t border-border/60 pt-3">
        <button
          onClick={onCancel}
          className="rounded-md border border-border px-3.5 py-1.5 text-xs transition-colors hover:bg-muted/60"
        >
          取消
        </button>
        <button
          onClick={onSave}
          className="rounded-md bg-voice px-4 py-1.5 text-xs font-medium text-white transition-opacity hover:opacity-90"
        >
          保存
        </button>
      </div>
    </div>
  );
};

// 字段行：固定宽标签 + 内容，保证多个字段左边对齐
const Field = ({
  label,
  children,
}: {
  label: string;
  children: React.ReactNode;
}) => (
  <div className="grid grid-cols-[44px_1fr] items-start gap-2">
    <label className="mt-1.5 text-[11px] uppercase tracking-wide text-muted-foreground">
      {label}
    </label>
    <div className="min-w-0">{children}</div>
  </div>
);

// ── 树节点（递归）──
interface NodeProps {
  item: ActionBarItem;
  siblings: ActionBarItem[];
  allItems: ActionBarItem[];
  index: number; // 同级 1-based 序号
  depth: number;
  parentLabel?: string; // 父级序号字符串，子项显示为 "父.子"
  expanded: Set<number>;
  editingId: number | null;
  editingForm: Partial<ActionBarItem>;
  onToggle: (id: number) => void;
  onStartEdit: (item: ActionBarItem) => void;
  onMove: (id: number, dir: number) => void;
  onDelete: (id: number) => void;
  onAdd: (parentId: number | null) => void;
  onFormChange: (f: Partial<ActionBarItem>) => void;
  onSaveEdit: () => void;
  onCancelEdit: () => void;
  draftParentId: number | null | undefined; // undefined=非草稿, null=顶层草稿, number=子菜单草稿
}

const TreeNodeBase = (props: NodeProps) => {
  const {
    item,
    siblings,
    allItems,
    index,
    depth,
    parentLabel,
    expanded,
    editingId,
  } = props;
  const isFirst = siblings[0]?.id === item.id;
  const isLast = siblings[siblings.length - 1]?.id === item.id;
  const subs = allItems.filter((i) => i.parentId === item.id);
  const isSubmenu = item.actionType === "submenu";
  const isOpen = expanded.has(item.id);
  const isEditing = editingId === item.id;
  const indexLabel = depth === 0 ? pad2(index) : `${parentLabel}.${index}`;

  return (
    <div>
      <div
        className={cn(
          "group relative flex items-center gap-2 rounded-md py-1.5 pl-1 pr-1.5 transition-colors",
          isEditing ? "bg-voice/[0.06]" : "cursor-pointer hover:bg-muted/40",
        )}
        onClick={() => props.onStartEdit(item)}
      >
        {/* 展开箭头（仅 submenu 有，其余占位保持对齐） */}
        <button
          onClick={(e) => {
            e.stopPropagation();
            props.onToggle(item.id);
          }}
          tabIndex={isSubmenu ? 0 : -1}
          className={cn(
            "flex h-4 w-4 shrink-0 items-center justify-center rounded text-muted-foreground hover:text-foreground",
            !isSubmenu && "invisible pointer-events-none",
          )}
          aria-label={isOpen ? "收起" : "展开"}
        >
          {isOpen ? (
            <ChevronDown className="h-3.5 w-3.5" />
          ) : (
            <ChevronRight className="h-3.5 w-3.5" />
          )}
        </button>

        {/* 序号：等宽定位，像注册表条目 */}
        <span className="w-6 shrink-0 text-right font-mono text-[11px] tabular-nums text-muted-foreground/60">
          {indexLabel}
        </span>

        {/* 标题 */}
        <span
          className={cn(
            "flex-1 truncate text-sm",
            item.isEnabled ? "text-foreground" : "text-muted-foreground/60",
          )}
        >
          {item.title}
        </span>

        {/* 子项计数 */}
        {isSubmenu && subs.length > 0 && (
          <span className="shrink-0 rounded-full bg-muted px-1.5 py-0.5 font-mono text-[10px] text-muted-foreground">
            {subs.length}
          </span>
        )}

        {/* 类型标签 */}
        <TypeTag type={item.actionType} />

        {/* 内置标记 */}
        {item.isSystem && (
          <span className="shrink-0 text-[10px] text-muted-foreground/50">
            内置
          </span>
        )}

        {/* 悬浮工具栏 */}
        <div className="flex shrink-0 items-center gap-0.5 opacity-0 transition-opacity group-hover:opacity-100">
          <button
            onClick={(e) => {
              e.stopPropagation();
              props.onMove(item.id, -1);
            }}
            disabled={isFirst}
            className="rounded p-0.5 text-muted-foreground hover:text-foreground disabled:opacity-25 disabled:hover:text-muted-foreground"
            aria-label="上移"
          >
            <ArrowUp className="h-3.5 w-3.5" />
          </button>
          <button
            onClick={(e) => {
              e.stopPropagation();
              props.onMove(item.id, 1);
            }}
            disabled={isLast}
            className="rounded p-0.5 text-muted-foreground hover:text-foreground disabled:opacity-25 disabled:hover:text-muted-foreground"
            aria-label="下移"
          >
            <ArrowDown className="h-3.5 w-3.5" />
          </button>
          <button
            onClick={(e) => {
              e.stopPropagation();
              props.onDelete(item.id);
            }}
            disabled={item.isSystem}
            className="rounded p-0.5 text-muted-foreground hover:text-red-500 disabled:opacity-25 disabled:hover:text-muted-foreground"
            aria-label="删除"
          >
            <Trash2 className="h-3.5 w-3.5" />
          </button>
        </div>
      </div>

      {/* 内联编辑器 */}
      {isEditing && (
        <EditForm
          form={props.editingForm}
          isSystem={item.isSystem}
          onChange={props.onFormChange}
          onSave={props.onSaveEdit}
          onCancel={props.onCancelEdit}
        />
      )}

      {/* 子树：细导引线 + 递归 */}
      {isSubmenu && isOpen && (
        <div className="relative ml-3 border-l border-border/50 pl-3">
          {/* 子菜单草稿表单 */}
          {props.draftParentId === item.id && (
            <EditForm
              form={props.editingForm}
              isSystem={false}
              onChange={props.onFormChange}
              onSave={props.onSaveEdit}
              onCancel={props.onCancelEdit}
            />
          )}
          {subs.map((sub, i) => (
            <TreeNode
              key={sub.id}
              {...props}
              item={sub}
              siblings={subs}
              index={i + 1}
              depth={depth + 1}
              parentLabel={String(index)}
            />
          ))}
          <button
            onClick={(e) => {
              e.stopPropagation();
              props.onAdd(item.id);
            }}
            className="my-1 flex w-full items-center gap-1.5 rounded-md border border-dashed border-border py-1.5 pl-2 text-xs text-muted-foreground transition-colors hover:border-voice/40 hover:text-voice"
          >
            <Plus className="h-3 w-3" /> 新增子项
          </button>
        </div>
      )}
    </div>
  );
};

const TreeNode = memo(TreeNodeBase);

// ── 主面板 ──

interface ScriptRun {
  id: number;
  itemId: number;
  itemTitle: string | null;
  scriptType: string;
  exitCode: number | null;
  stdout: string;
  stderr: string;
  errorMsg: string;
  startedAt: string;
  finishedAt: string | null;
  durationMs: number | null;
}

const ScriptRunsList = ({ showToast }: { showToast: (msg: string) => void }) => {
  const [runs, setRuns] = useState<ScriptRun[]>([]);
  const [loaded, setLoaded] = useState(false);
  const [expandedId, setExpandedId] = useState<number | null>(null);

  const refresh = useCallback(async () => {
    const list = await invoke<ScriptRun[]>("list_script_runs", { limit: 100 });
    setRuns(list);
    setLoaded(true);
  }, []);

  useEffect(() => { refresh(); }, [refresh]);

  const handleClear = useCallback(async () => {
    try {
      await invoke("clear_script_runs", { keepRecent: 100 });
      showToast("已清理旧记录");
      refresh();
    } catch (e) {
      showToast("清理失败：" + e);
    }
  }, [showToast, refresh]);

  if (!loaded) {
    return <p className="py-12 text-center text-sm text-muted-foreground">加载中…</p>;
  }

  if (runs.length === 0) {
    return (
      <div className="flex flex-col items-center justify-center gap-2 py-16 text-center">
        <p className="text-sm font-medium">暂无执行记录</p>
        <p className="text-xs text-muted-foreground">运行脚本后，执行结果会记录在这里。</p>
      </div>
    );
  }

  const statusColor = (r: ScriptRun) => {
    if (r.exitCode === null) return "bg-orange-500";
    return r.exitCode === 0 ? "bg-emerald-500" : "bg-red-500";
  };
  const statusLabel = (r: ScriptRun) => {
    if (r.exitCode === null) return r.errorMsg || "异常";
    return r.exitCode === 0 ? "成功" : `失败(${r.exitCode})`;
  };

  return (
    <div>
      <div className="mb-3 flex justify-end">
        <button
          onClick={handleClear}
          className="rounded-md border border-border px-2.5 py-1 text-xs text-muted-foreground transition-colors hover:bg-muted/60 hover:text-foreground"
        >
          清理旧记录
        </button>
      </div>
      <div className="space-y-px">
        {runs.map((r) => (
          <div key={r.id} className="rounded-lg border border-border bg-muted/20">
            <button
              onClick={() => setExpandedId(expandedId === r.id ? null : r.id)}
              className="flex w-full items-center gap-3 px-3.5 py-2 text-left"
            >
              <span className={cn("h-1.5 w-1.5 shrink-0 rounded-full", statusColor(r))} />
              <span className="shrink-0 text-xs font-medium">
                {r.itemTitle || "已删除"}
              </span>
              <span className="shrink-0 font-mono text-[10px] uppercase tracking-wider text-muted-foreground">
                {r.scriptType}
              </span>
              <span className="shrink-0 text-[11px] text-muted-foreground">
                {statusLabel(r)}
              </span>
              <span className="ml-auto shrink-0 text-[11px] text-muted-foreground">
                {r.durationMs != null ? `${r.durationMs}ms` : "—"}
              </span>
            </button>
            {expandedId === r.id && (
              <div className="space-y-2 border-t border-border/60 px-3.5 py-2.5">
                {r.stdout && (
                  <div>
                    <p className="mb-1 font-mono text-[10px] uppercase tracking-wider text-muted-foreground">stdout</p>
                    <textarea
                      readOnly
                      className="w-full min-h-[60px] resize-y bg-background border border-border rounded px-2 py-1.5 font-mono text-xs leading-relaxed"
                      value={r.stdout.slice(0, 8000)}
                    />
                  </div>
                )}
                {r.stderr && (
                  <div>
                    <p className="mb-1 font-mono text-[10px] uppercase tracking-wider text-red-500/70">stderr</p>
                    <textarea
                      readOnly
                      className="w-full min-h-[40px] resize-y bg-background border border-border rounded px-2 py-1.5 font-mono text-xs leading-relaxed text-red-600/80"
                      value={r.stderr.slice(0, 8000)}
                    />
                  </div>
                )}
                {r.errorMsg && (
                  <p className="text-xs text-orange-600">{r.errorMsg}</p>
                )}
              </div>
            )}
          </div>
        ))}
      </div>
    </div>
  );
};

export default function ActionBarPanel({
  showToast,
}: {
  showToast: (msg: string) => void;
}) {
  const [items, setItems] = useState<ActionBarItem[]>([]);
  const [expanded, setExpanded] = useState<Set<number>>(new Set());
  const [editingId, setEditingId] = useState<number | null>(null);
  const [editingForm, setEditingForm] = useState<Partial<ActionBarItem>>({});
  // draft 状态：新增时不写 DB，只在内存编辑，保存时才 create。取消只清 state，零脏数据。
  const [draftParentId, setDraftParentId] = useState<number | null | undefined>(undefined); // undefined=非草稿, null=顶层草稿, number=子菜单草稿
  const [loaded, setLoaded] = useState(false);
  const [view, setView] = useState<"menu" | "runs">("menu");

  const refresh = useCallback(async (): Promise<ActionBarItem[]> => {
    const list = await invoke<ActionBarItem[]>("list_action_bar_items");
    setItems(list);
    // 注意：不在此处改动 expanded，否则会覆盖用户的折叠选择。
    // 首次全部展开见下方 useEffect；新增子项时由 handleAdd 显式展开父节点。
    setLoaded(true);
    return list;
  }, []);

  useEffect(() => {
    (async () => {
      const list = await refresh();
      // 仅首次加载默认展开全部 submenu 节点，让结构一眼可见
      setExpanded(() => {
        const next = new Set<number>();
        list.forEach((i) => {
          if (i.actionType === "submenu") next.add(i.id);
        });
        return next;
      });
    })();
  }, [refresh]);

  const mainItems = items.filter((i) => i.parentId === null);
  const enabledCount = items.filter((i) => i.isEnabled).length;

  const toggle = useCallback((id: number) => {
    setExpanded((prev) => {
      const next = new Set(prev);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      return next;
    });
  }, []);

  const submenuIds = items
    .filter((i) => i.actionType === "submenu")
    .map((i) => i.id);
  const allExpanded =
    submenuIds.length > 0 && submenuIds.every((id) => expanded.has(id));

  const expandAll = useCallback(() => {
    setExpanded(new Set(submenuIds));
  }, [submenuIds]);

  const collapseAll = useCallback(() => {
    setExpanded(new Set());
  }, []);

  const startEdit = useCallback((item: ActionBarItem) => {
    setDraftParentId(undefined);
    setEditingId(item.id);
    setEditingForm({ ...item });
  }, []);

  const cancelEdit = useCallback(() => {
    setEditingId(null);
    setEditingForm({});
    setDraftParentId(undefined);
  }, []);

  const saveEdit = useCallback(async () => {
    try {
      if (draftParentId !== undefined) {
        // 新建草稿——此时才写 DB
        await invoke("create_action_bar_item", {
          parentId: draftParentId,
          title: editingForm.title || "新菜单项",
          icon: "",
          actionType: editingForm.actionType || "copy",
          actionData: editingForm.actionData || "",
          isAsync: editingForm.actionType === "script" ? (editingForm.isAsync ?? true) : true,
          writeOutputToClipboard: editingForm.actionType === "script" ? (editingForm.writeOutputToClipboard ?? false) : false,
        });
        showToast("已创建");
      } else if (editingId) {
        // 编辑已有项
        await invoke("update_action_bar_item", {
          id: editingId,
          title: editingForm.title || "",
          icon: editingForm.icon || "",
          actionType: editingForm.actionType || "copy",
          actionData: editingForm.actionData || "",
          isEnabled: editingForm.isEnabled ?? true,
          isAsync: editingForm.actionType === "script" ? (editingForm.isAsync ?? true) : true,
          writeOutputToClipboard: editingForm.actionType === "script" ? (editingForm.writeOutputToClipboard ?? false) : false,
        });
        showToast("已保存");
      }
      cancelEdit();
      refresh();
    } catch (e) {
      showToast("保存失败：" + e);
    }
  }, [draftParentId, editingId, editingForm, showToast, cancelEdit, refresh]);

  const handleDelete = useCallback(
    async (id: number) => {
      try {
        await invoke("delete_action_bar_item", { id });
        showToast("已删除");
        refresh();
      } catch (e) {
        showToast("删除失败：" + e);
      }
    },
    [showToast, refresh],
  );

  const handleMove = useCallback(
    async (id: number, direction: number) => {
      try {
        await invoke("move_action_bar_item", { id, direction });
        refresh();
      } catch (e) {
        showToast("移动失败：" + e);
      }
    },
    [refresh, showToast],
  );

  const handleAdd = useCallback((parentId: number | null) => {
    // 纯内存草稿——不碰 DB，保存时才 create
    setEditingId(null);
    setDraftParentId(parentId);
    setEditingForm({
      title: "新菜单项",
      actionType: "copy",
      actionData: "",
      isEnabled: true,
    });
    // 子菜单草稿需展开父节点才能看到表单
    if (parentId !== null) {
      setExpanded((prev) => new Set(prev).add(parentId));
    }
  }, []);

  const nodeCommon = {
    allItems: items,
    expanded,
    editingId,
    editingForm,
    onToggle: toggle,
    onStartEdit: startEdit,
    onMove: handleMove,
    onDelete: handleDelete,
    onAdd: handleAdd,
    onFormChange: setEditingForm,
    onSaveEdit: saveEdit,
    onCancelEdit: cancelEdit,
    draftParentId,
  };

  return (
    <div className="max-w-2xl">
      {/* Header */}
      <div className="mb-5 flex items-start justify-between gap-4">
        <div className="min-w-0">
          <div className="font-mono text-[10px] uppercase tracking-widest text-muted-foreground/70">
            命令面板 · 菜单管理
          </div>
          <h2 className="mt-0.5 text-lg font-semibold tracking-tight">
            AI 命令面板菜单
          </h2>
          <p className="mt-1 text-xs text-muted-foreground">
            选中文本 → 全局热键唤出的两级菜单。共 {items.length} 项，{
              enabledCount
            }{" "}
            项启用。
          </p>
        </div>
        <div className="flex shrink-0 items-center gap-1.5">
          <button
            onClick={() => setView(view === "menu" ? "runs" : "menu")}
            className="flex items-center gap-1.5 rounded-md border border-border px-2.5 py-1.5 text-xs text-muted-foreground transition-colors hover:bg-muted/60 hover:text-foreground"
          >
            {view === "menu" ? "执行记录" : "返回菜单"}
          </button>
          {view === "menu" && (
            <>
              <button
                onClick={allExpanded ? collapseAll : expandAll}
                className="flex items-center gap-1.5 rounded-md border border-border px-2.5 py-1.5 text-xs text-muted-foreground transition-colors hover:bg-muted/60 hover:text-foreground"
                title={allExpanded ? "全部收缩" : "全部展开"}
              >
                {allExpanded ? (
                  <ChevronsDownUp className="h-3.5 w-3.5" />
                ) : (
                  <ChevronsUpDown className="h-3.5 w-3.5" />
                )}
                <span className="hidden sm:inline">
                  {allExpanded ? "全部收缩" : "全部展开"}
                </span>
              </button>
              <button
                onClick={() => handleAdd(null)}
                className="flex items-center gap-1.5 rounded-md bg-voice px-3.5 py-1.5 text-sm font-medium text-white transition-opacity hover:opacity-90"
              >
                <Plus className="h-4 w-4" /> 新增主菜单项
              </button>
            </>
          )}
        </div>
      </div>

      {/* Body */}
      {view === "runs" ? (
        <ScriptRunsList showToast={showToast} />
      ) : !loaded ? (
        <p className="py-12 text-center text-sm text-muted-foreground">
          加载中…
        </p>
      ) : mainItems.length === 0 ? (
        <div className="flex flex-col items-center justify-center gap-3 py-16 text-center">
          <div className="flex h-12 w-12 items-center justify-center rounded-full bg-muted text-muted-foreground">
            <Plus className="h-5 w-5" />
          </div>
          <div>
            <p className="text-sm font-medium">还没有菜单项</p>
            <p className="mt-0.5 text-xs text-muted-foreground">
              新增第一个主菜单项，开始配置你的命令面板。
            </p>
          </div>
          <button
            onClick={() => handleAdd(null)}
            className="flex items-center gap-1.5 rounded-md bg-voice px-3.5 py-1.5 text-sm font-medium text-white transition-opacity hover:opacity-90"
          >
            <Plus className="h-4 w-4" /> 新增主菜单项
          </button>
        </div>
      ) : (
        <div className="space-y-px">
          {/* 新建草稿表单（内存态，不在树中） */}
          {draftParentId !== undefined && draftParentId === null && (
            <EditForm
              form={editingForm}
              isSystem={false}
              onChange={setEditingForm}
              onSave={saveEdit}
              onCancel={cancelEdit}
            />
          )}
          {mainItems.map((item, i) => (
            <TreeNode
              key={item.id}
              {...nodeCommon}
              item={item}
              siblings={mainItems}
              index={i + 1}
              depth={0}
            />
          ))}
        </div>
      )}
    </div>
  );
}
