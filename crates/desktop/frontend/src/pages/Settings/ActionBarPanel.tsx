import { useState, useEffect, useCallback, memo } from "react";
import { invoke } from "@tauri-apps/api/core";
import { emit } from "@tauri-apps/api/event";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import {
  ChevronRight,
  ChevronDown,
  ChevronsUpDown,
  ChevronsDownUp,
  ArrowUp,
  ArrowDown,
  Trash2,
  Plus,
  Pencil,
  ArrowLeft,
  X,
} from "lucide-react";
import { cn } from "@/lib/utils";
import { useT, t as ti18n } from "@/lib/i18n";

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
  shortcut?: string;
  agent?: string;
  accepts?: string;
}

// ── 类型元信息 ──
// bar: 左侧 3px 色条颜色（列表行签名元素）；dot: 小圆点（表单内引用）
const TYPE_META: Record<
  string,
  { bar: string; dot: string; label: string; descKey: string; placeholderKey: string }
> = {
  submenu:    { bar: "bg-voice/60",       dot: "bg-voice",       label: "SUBMENU", descKey: "settings.actionBar.typeSubmenuDesc",    placeholderKey: "" },
  ai:         { bar: "bg-violet-500",     dot: "bg-violet-500",  label: "AI",      descKey: "settings.actionBar.typeAiDesc",         placeholderKey: "settings.actionBar.typeAiPlaceholder" },
  url:        { bar: "bg-sky-500",        dot: "bg-sky-500",     label: "URL",     descKey: "settings.actionBar.typeUrlDesc",        placeholderKey: "settings.actionBar.typeUrlPlaceholder" },
  script:     { bar: "bg-emerald-500",    dot: "bg-emerald-500", label: "SCRIPT",  descKey: "settings.actionBar.typeScriptDesc",     placeholderKey: "settings.actionBar.typeScriptPlaceholder" },
  extension:  { bar: "bg-amber-500",      dot: "bg-amber-500",   label: "EXT",     descKey: "settings.actionBar.typeExtensionDesc",  placeholderKey: "" },
  copy:       { bar: "bg-stone-400",      dot: "bg-stone-400",   label: "COPY",    descKey: "settings.actionBar.typeCopyDesc",       placeholderKey: "" },
  agent:      { bar: "bg-rose-500",       dot: "bg-rose-500",    label: "AGENT",   descKey: "settings.actionBar.typeAgentDesc",      placeholderKey: "settings.actionBar.typeAgentPlaceholder" },
  copy_path:  { bar: "bg-cyan-500",       dot: "bg-cyan-500",    label: "PATH",    descKey: "settings.actionBar.typeCopyPathDesc",   placeholderKey: "" },
};

const ACTION_TYPES = [
  { value: "submenu",    labelKey: "settings.actionBar.typeSubmenu" },
  { value: "ai",         labelKey: "settings.actionBar.typeAi" },
  { value: "url",        labelKey: "settings.actionBar.typeUrl" },
  { value: "script",     labelKey: "settings.actionBar.typeScript" },
  { value: "extension",  labelKey: "settings.actionBar.typeExtension" },
  { value: "agent",      labelKey: "settings.actionBar.typeAgent" },
  { value: "copy_path",  labelKey: "settings.actionBar.typeCopyPath" },
  { value: "copy",       labelKey: "settings.actionBar.typeCopy" },
];

function deriveAccepts(actionType: string | undefined, explicit?: string): string {
  if (explicit) return explicit;
  if (actionType === "agent" || actionType === "copy_path") return "file";
  if (actionType === "submenu") return "any";
  return "text";
}

const pad2 = (n: number) => String(n).padStart(2, "0");

// ── 统一控件样式 ──
const inputBase = "w-full bg-background border border-border rounded-md px-3 py-2 text-sm outline-none transition-all focus:border-voice/50 focus:ring-2 focus:ring-voice/15";

// ── 类型标签 ──
const TypeTag = ({ type, variant = "dot" }: { type: string; variant?: "dot" | "solid" }) => {
  const meta = TYPE_META[type] ?? TYPE_META.copy;
  return (
    <span className="inline-flex items-center gap-1 font-mono text-[10px] uppercase tracking-wider text-muted-foreground">
      {variant === "dot" && <span className={cn("h-1.5 w-1.5 rounded-full", meta.dot)} />}
      {meta.label}
    </span>
  );
};

// ── 开关 ──
const Toggle = ({ checked, onChange }: { checked: boolean; onChange: (v: boolean) => void }) => (
  <button
    type="button"
    role="switch"
    aria-checked={checked}
    onClick={() => onChange(!checked)}
    className={cn(
      "relative inline-flex h-[18px] w-8 items-center rounded-full transition-colors duration-200",
      checked ? "bg-voice" : "bg-muted-foreground/25",
    )}
  >
    <span
      className={cn(
        "inline-block h-3.5 w-3.5 transform rounded-full bg-white shadow-sm transition-transform duration-200",
        checked ? "translate-x-4" : "translate-x-0.5",
      )}
    />
  </button>
);

// ── 工具栏按钮 ──
const ToolBtn = ({
  onClick, children, title, variant = "ghost",
}: {
  onClick: () => void; children: React.ReactNode; title?: string;
  variant?: "ghost" | "solid" | "outline";
}) => (
  <button
    onClick={onClick}
    title={title}
    className={cn(
      "flex items-center gap-1.5 rounded-md px-2.5 py-1.5 text-xs transition-all",
      variant === "ghost" && "text-muted-foreground hover:bg-muted/60 hover:text-foreground",
      variant === "outline" && "border border-border text-muted-foreground hover:bg-muted/40 hover:text-foreground",
      variant === "solid" && "bg-voice font-medium text-white hover:opacity-90",
    )}
  >
    {children}
  </button>
);

// ── 编辑表单 ──
interface EditFormProps {
  form: Partial<ActionBarItem>;
  isSystem: boolean;
  onChange: (form: Partial<ActionBarItem>) => void;
  onSave: () => void;
  onCancel: () => void;
}

interface ImportResult {
  name: string;
  sourcePath: string;
  dirName: string;
  isAsync: boolean;
  writeOutputToClipboard: boolean;
}

const ExtensionDropZone = ({
  form, onChange,
}: {
  form: Partial<ActionBarItem>;
  onChange: (form: Partial<ActionBarItem>) => void;
}) => {
  const [dragging, setDragging] = useState(false);
  const [importing, setImporting] = useState(false);
  const [error, setError] = useState("");

  const doImport = useCallback(
    async (sourcePath: string) => {
      setImporting(true);
      setError("");
      try {
        const result = await invoke<ImportResult>("import_extension", { sourcePath });
        onChange({
          ...form,
          title: result.name || form.title,
          actionData: `${result.sourcePath}|${result.dirName}`,
          isAsync: result.isAsync,
          writeOutputToClipboard: result.writeOutputToClipboard,
        });
      } catch (e) {
        setError(String(e));
      }
      setImporting(false);
    },
    [form, onChange],
  );

  const handleOpenFile = useCallback(async () => {
    try {
      const selected = await openDialog({
        multiple: false,
        filters: [{ name: ti18n("settings.actionBar.extensionFilter"), extensions: ["zip"] }],
      });
      if (typeof selected === "string") doImport(selected);
    } catch { /* cancelled */ }
  }, [doImport]);

  const handleOpenDir = useCallback(async () => {
    try {
      const selected = await openDialog({ directory: true, multiple: false });
      if (typeof selected === "string") doImport(selected);
    } catch { /* cancelled */ }
  }, [doImport]);

  useEffect(() => {
    const win = getCurrentWebview();
    const unlisten = win.onDragDropEvent((event) => {
      const { type } = event.payload;
      if (type === "enter" || type === "over") setDragging(true);
      else if (type === "drop") {
        setDragging(false);
        const paths = (event.payload as { paths: string[] }).paths;
        if (paths.length > 0) doImport(paths[0]);
      } else if (type === "leave") setDragging(false);
    });
    return () => { unlisten.then((fn: () => void) => fn()); };
  }, [doImport]);

  const hasPackage = form.actionData && form.actionData.startsWith("/");

  return (
    <div
      className={cn(
        "rounded-lg border-2 border-dashed transition-colors min-h-[80px] flex flex-col items-center justify-center gap-1.5 p-3",
        dragging ? "border-voice bg-voice/5" : "border-border",
        hasPackage && "border-solid",
      )}
    >
      {importing ? (
        <p className="text-xs text-muted-foreground">{ti18n("settings.actionBar.importing")}</p>
      ) : hasPackage ? (
        <div className="flex items-center gap-2 w-full">
          <div className="flex-1 min-w-0">
            <p className="text-xs font-medium text-foreground truncate">{form.title}</p>
            <p className="font-mono text-[10px] text-muted-foreground/70 truncate">
              {form.actionData?.split("|")[0]}
            </p>
          </div>
          <button
            onClick={() => onChange({ ...form, actionData: "", title: "" })}
            className="shrink-0 rounded-md p-1 text-muted-foreground/60 transition-colors hover:bg-red-500/10 hover:text-red-500"
            aria-label={ti18n("settings.actionBar.clearSelection")}
          >
            <X className="h-3.5 w-3.5" />
          </button>
        </div>
      ) : (
        <>
          <p className="text-[11px] text-muted-foreground/70">{ti18n("settings.actionBar.dropHint")}</p>
          <div className="flex items-center gap-3">
            <button onClick={handleOpenFile} className="text-[11px] text-voice hover:underline">
              {ti18n("settings.actionBar.selectZip")}
            </button>
            <span className="text-[11px] text-muted-foreground/40">|</span>
            <button onClick={handleOpenDir} className="text-[11px] text-voice hover:underline">
              {ti18n("settings.actionBar.selectFolder")}
            </button>
          </div>
        </>
      )}
      {error && <p className="text-[11px] text-red-500">{error}</p>}
    </div>
  );
};

// ── 表单字段行 ──
const FormField = ({
  label, children, hint,
}: {
  label: string; children: React.ReactNode; hint?: string;
}) => (
  <div className="space-y-1.5">
    <label className="block text-[11px] font-medium uppercase tracking-wide text-muted-foreground/80">
      {label}
    </label>
    <div className="min-w-0">{children}</div>
    {hint && <p className="text-[11px] text-muted-foreground/60">{hint}</p>}
  </div>
);

const EditForm = ({
  form, isSystem, onChange, onSave, onCancel,
}: EditFormProps) => {
  const t = useT();
  const type = form.actionType || "copy";
  const meta = TYPE_META[type];
  const showContent = type !== "submenu" && type !== "copy" && type !== "extension" && type !== "copy_path";
  const showShortcut = type !== "submenu";
  const [adapters, setAdapters] = useState<{key:string;displayName:string;isAvailable:boolean}[]>([]);
  useEffect(() => {
    invoke<{key:string;displayName:string;isAvailable:boolean}[]>("list_agent_adapters").then(setAdapters).catch(() => {});
  }, []);

  return (
    <div className="space-y-5">
      {/* 导航栏 */}
      <div className="flex items-center gap-3 border-b border-border/40 pb-3">
        <button
          onClick={onCancel}
          className="flex items-center gap-1.5 text-xs text-muted-foreground transition-colors hover:text-foreground"
        >
          <ArrowLeft className="h-3.5 w-3.5" />
          {ti18n("settings.actionBar.backToMenu")}
        </button>
        <div className="flex items-center gap-2">
          <span className={cn("h-2 w-2 rounded-full", meta.dot)} />
          <span className="font-mono text-[10px] uppercase tracking-widest text-muted-foreground/70">
            {meta.label}
          </span>
        </div>
      </div>

      {/* 基础信息卡片 */}
      <div className="space-y-4 rounded-lg border border-border/50 bg-muted/15 p-4">
        <FormField label={t("settings.actionBar.titleLabel")}>
          <input
            className={inputBase}
            value={form.title || ""}
            maxLength={12}
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
              onChange({ ...form, title: ok });
            }}
          />
        </FormField>

        <FormField
          label={t("settings.actionBar.typeLabel")}
          hint={t(meta.descKey) + (isSystem ? " · " + t("settings.actionBar.builtinTypeLocked") : "")}
        >
          <select
            className={cn(inputBase, "disabled:opacity-60")}
            value={type}
            disabled={isSystem}
            onChange={(e) => onChange({ ...form, actionType: e.target.value })}
          >
            {ACTION_TYPES.map((at) => (
              <option key={at.value} value={at.value}>{t(at.labelKey)}</option>
            ))}
          </select>
        </FormField>
      </div>

      {/* 类型特定配置卡片 */}
      {showContent && (
        <div className="space-y-4 rounded-lg border border-border/50 bg-muted/15 p-4">
          <FormField label={t("settings.actionBar.contentLabel")}>
            <textarea
              className="w-full min-h-[120px] resize-y bg-background border border-border rounded-md px-3 py-2 font-mono text-xs leading-relaxed outline-none transition-all focus:border-voice/50 focus:ring-2 focus:ring-voice/15"
              placeholder={meta.placeholderKey ? t(meta.placeholderKey) : ""}
              value={form.actionData || ""}
              onChange={(e) => onChange({ ...form, actionData: e.target.value })}
            />
          </FormField>
        </div>
      )}

      {type === "extension" && (
        <div className="rounded-lg border border-border/50 bg-muted/15 p-4">
          <ExtensionDropZone form={form} onChange={onChange} />
        </div>
      )}

      {type === "script" && (
        <div className="space-y-3 rounded-lg border border-border/50 bg-muted/15 p-4">
          <FormField label={t("settings.actionBar.execOptions")}>
            <div className="space-y-2.5">
              <div className="flex items-center gap-2.5">
                <Toggle
                  checked={form.isAsync ?? true}
                  onChange={(v) => onChange({
                    ...form, isAsync: v,
                    writeOutputToClipboard: v ? false : form.writeOutputToClipboard,
                  })}
                />
                <span className="text-xs text-muted-foreground">{t("settings.actionBar.asyncExec")}</span>
              </div>
              {!(form.isAsync ?? true) && (
                <div className="flex items-center gap-2.5">
                  <Toggle
                    checked={form.writeOutputToClipboard ?? false}
                    onChange={(v) => onChange({ ...form, writeOutputToClipboard: v })}
                  />
                  <span className="text-xs text-muted-foreground">{t("settings.actionBar.writeToClipboard")}</span>
                </div>
              )}
            </div>
          </FormField>
        </div>
      )}

      {type === "agent" && (
        <div className="rounded-lg border border-border/50 bg-muted/15 p-4">
          <FormField label={t("settings.actionBar.agentLabel")}>
            <select
              className={inputBase}
              value={form.agent || ""}
              onChange={(e) => onChange({ ...form, agent: e.target.value })}
            >
              <option value="">{t("settings.actionBar.selectAgent")}</option>
              {adapters.filter((a) => a.isAvailable).map((a) => (
                <option key={a.key} value={a.key}>{a.displayName}</option>
              ))}
            </select>
          </FormField>
        </div>
      )}

      {type === "copy_path" && (
        <div className="rounded-lg border border-border/50 bg-muted/15 p-4">
          <FormField label={t("settings.actionBar.pathFormat")}>
            <select
              className={inputBase}
              value={form.actionData || "plain"}
              onChange={(e) => onChange({ ...form, actionData: e.target.value })}
            >
              <option value="plain">{t("settings.actionBar.pathFormatPlain")}</option>
              <option value="url">{t("settings.actionBar.pathFormatUrl")}</option>
              <option value="quoted">{t("settings.actionBar.pathFormatQuoted")}</option>
            </select>
          </FormField>
        </div>
      )}

      {/* 通用配置 */}
      <div className="space-y-4 rounded-lg border border-border/50 bg-muted/15 p-4">
        {showShortcut && (
          <FormField label={t("settings.actionBar.shortcutLabel")}>
            <div className="flex items-center gap-2">
              <div className="flex items-center gap-1">
                <span className="text-xs text-muted-foreground/60 font-mono">⌥ +</span>
                <input
                  className="w-10 text-center bg-background border border-border rounded-md px-2 py-1.5 text-sm font-mono outline-none transition-all focus:border-voice/50 focus:ring-2 focus:ring-voice/15"
                  placeholder="—"
                  maxLength={1}
                  value={form.shortcut || ""}
                  onChange={(e) => {
                    const raw = e.target.value.toLowerCase();
                    const filtered = raw.replace(/[^0-9a-z]/g, "").slice(-1);
                    onChange({ ...form, shortcut: filtered });
                  }}
                />
              </div>
              {form.shortcut && (
                <button
                  onClick={() => onChange({ ...form, shortcut: "" })}
                  className="rounded p-1 text-muted-foreground/60 transition-colors hover:bg-red-500/10 hover:text-red-500"
                >
                  <X className="h-3.5 w-3.5" />
                </button>
              )}
              <span className="text-[11px] text-muted-foreground/60">
                {t("settings.actionBar.shortcutHint")}
              </span>
            </div>
          </FormField>
        )}

        <FormField label={t("settings.actionBar.enableLabel")}>
          <div className="flex items-center gap-2.5">
            <Toggle
              checked={form.isEnabled ?? true}
              onChange={(v) => onChange({ ...form, isEnabled: v })}
            />
            <span className="text-xs text-muted-foreground">
              {form.isEnabled ? t("settings.actionBar.showInMenu") : t("settings.actionBar.hidden")}
            </span>
          </div>
        </FormField>
      </div>

      {/* 操作栏 */}
      <div className="flex justify-end gap-2.5 border-t border-border/40 pt-4">
        <button
          onClick={onCancel}
          className="rounded-md border border-border px-4 py-2 text-xs transition-colors hover:bg-muted/60"
        >
          {t("settings.actionBar.cancel")}
        </button>
        <button
          onClick={onSave}
          className="rounded-md bg-voice px-5 py-2 text-xs font-medium text-white transition-opacity hover:opacity-90"
        >
          {t("settings.actionBar.save")}
        </button>
      </div>
    </div>
  );
};

// ── 树节点 ──
interface NodeProps {
  item: ActionBarItem;
  siblings: ActionBarItem[];
  allItems: ActionBarItem[];
  index: number;
  depth: number;
  parentLabel?: string;
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
  deleteConfirmId: number | null;
  draftParentId: number | null | undefined;
}

const TreeNodeBase = (props: NodeProps) => {
  const t = useT();
  const { item, siblings, allItems, index, depth, parentLabel, expanded, editingId } = props;
  const isFirst = siblings[0]?.id === item.id;
  const isLast = siblings[siblings.length - 1]?.id === item.id;
  const subs = allItems.filter((i) => i.parentId === item.id);
  const isSubmenu = item.actionType === "submenu";
  const isOpen = expanded.has(item.id);
  const isEditing = editingId === item.id;
  const indexLabel = depth === 0 ? pad2(index) : `${parentLabel}.${index}`;
  const meta = TYPE_META[item.actionType] ?? TYPE_META.copy;

  return (
    <div>
      <div
        className={cn(
          "group relative flex items-center gap-2 rounded-md py-1.5 pl-0 pr-1.5 transition-colors",
          isEditing ? "bg-voice/[0.06]" : isSubmenu ? "cursor-pointer hover:bg-muted/40" : "hover:bg-muted/40",
        )}
        onClick={() => { if (isSubmenu) props.onToggle(item.id); }}
      >
        {/* 签名元素：左侧类型色条 */}
        <div className={cn("h-5 w-[3px] shrink-0 rounded-full", meta.bar)} />

        {/* 展开箭头 */}
        <button
          onClick={(e) => { e.stopPropagation(); props.onToggle(item.id); }}
          tabIndex={isSubmenu ? 0 : -1}
          className={cn(
            "flex h-4 w-4 shrink-0 items-center justify-center rounded text-muted-foreground transition-colors hover:text-foreground",
            !isSubmenu && "invisible pointer-events-none",
          )}
          aria-label={isOpen ? t("settings.actionBar.collapse") : t("settings.actionBar.expand")}
        >
          {isOpen ? <ChevronDown className="h-3.5 w-3.5" /> : <ChevronRight className="h-3.5 w-3.5" />}
        </button>

        {/* 序号 */}
        <span className="w-7 shrink-0 text-right font-mono text-[11px] tabular-nums text-muted-foreground/50">
          {indexLabel}
        </span>

        {/* 标题 */}
        <span className={cn(
          "flex-1 truncate text-sm",
          item.isEnabled ? "text-foreground" : "text-muted-foreground/50",
        )}>
          {item.title}
        </span>

        {/* 子项计数 */}
        {isSubmenu && subs.length > 0 && (
          <span className="shrink-0 rounded-full bg-muted px-1.5 py-0.5 font-mono text-[10px] tabular-nums text-muted-foreground">
            {subs.length}
          </span>
        )}

        {/* 类型标签 */}
        <TypeTag type={item.actionType} />

        {/* 快捷键徽章 */}
        {item.shortcut && (
          <span className="shrink-0 rounded bg-voice/10 px-1 py-0.5 font-mono text-[10px] text-voice/80">
            ⌥{item.shortcut}
          </span>
        )}

        {/* 内置标记 */}
        {item.isSystem && (
          <span className="shrink-0 text-[10px] text-muted-foreground/40">
            {t("settings.actionBar.builtin")}
          </span>
        )}

        {/* 悬浮操作栏 */}
        <div className="flex shrink-0 items-center gap-0.5 opacity-0 transition-opacity group-hover:opacity-100">
          <button
            onClick={(e) => { e.stopPropagation(); props.onMove(item.id, -1); }}
            disabled={isFirst}
            className="rounded p-0.5 text-muted-foreground transition-colors hover:text-foreground disabled:opacity-25"
            aria-label="Move up"
          >
            <ArrowUp className="h-3.5 w-3.5" />
          </button>
          <button
            onClick={(e) => { e.stopPropagation(); props.onMove(item.id, 1); }}
            disabled={isLast}
            className="rounded p-0.5 text-muted-foreground transition-colors hover:text-foreground disabled:opacity-25"
            aria-label="Move down"
          >
            <ArrowDown className="h-3.5 w-3.5" />
          </button>
          <button
            onClick={(e) => { e.stopPropagation(); props.onStartEdit(item); }}
            className="rounded p-0.5 text-muted-foreground transition-colors hover:text-foreground"
            aria-label={t("settings.actionBar.edit")}
          >
            <Pencil className="h-3.5 w-3.5" />
          </button>
          <button
            onClick={(e) => { e.stopPropagation(); props.onDelete(item.id); }}
            disabled={item.isSystem}
            className={cn(
              "rounded p-0.5 transition-colors disabled:opacity-25",
              props.deleteConfirmId === item.id
                ? "bg-red-500 text-white hover:bg-red-600"
                : "text-muted-foreground hover:text-red-500",
            )}
            aria-label={t("settings.actionBar.delete")}
            title={props.deleteConfirmId === item.id ? t("settings.actionBar.deleteConfirm") : t("settings.actionBar.delete")}
          >
            {props.deleteConfirmId === item.id ? (
              <span className="px-1 text-[10px] font-medium">{t("settings.actionBar.confirm")}</span>
            ) : (
              <Trash2 className="h-3.5 w-3.5" />
            )}
          </button>
        </div>
      </div>

      {/* 子树 */}
      {isSubmenu && isOpen && (
        <div className="relative ml-[10px] border-l border-border/40 pl-3">
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
            onClick={(e) => { e.stopPropagation(); props.onAdd(item.id); }}
            className="my-1 flex w-full items-center gap-1.5 rounded-md border border-dashed border-border py-1.5 pl-2 text-xs text-muted-foreground transition-colors hover:border-voice/40 hover:text-voice"
          >
            <Plus className="h-3 w-3" /> {t("settings.actionBar.addSubItem")}
          </button>
        </div>
      )}
    </div>
  );
};

const TreeNode = memo(TreeNodeBase);

// ── 执行记录 ──
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
  const t = useT();
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
      showToast(t("settings.actionBar.cleanedOldRecords"));
      refresh();
    } catch (e) {
      showToast(t("settings.actionBar.cleanFailed") + e);
    }
  }, [showToast, refresh]);

  if (!loaded) {
    return <p className="py-12 text-center text-sm text-muted-foreground">{t("settings.actionBar.loadingRecords")}</p>;
  }

  if (runs.length === 0) {
    return (
      <div className="flex flex-col items-center justify-center gap-2 py-16 text-center">
        <p className="text-sm font-medium">{t("settings.actionBar.noRecords")}</p>
        <p className="text-xs text-muted-foreground">{t("settings.actionBar.recordsHint")}</p>
      </div>
    );
  }

  const statusColor = (r: ScriptRun) => {
    if (r.exitCode === null) return "bg-orange-500";
    return r.exitCode === 0 ? "bg-emerald-500" : "bg-red-500";
  };
  const statusLabel = (r: ScriptRun) => {
    if (r.exitCode === null) return r.errorMsg || t("settings.actionBar.statusError");
    return r.exitCode === 0 ? t("settings.actionBar.statusSuccess") : t("settings.actionBar.statusFailed", { n: r.exitCode });
  };

  return (
    <div>
      <div className="mb-3 flex justify-end">
        <button
          onClick={handleClear}
          className="rounded-md border border-border px-2.5 py-1 text-xs text-muted-foreground transition-colors hover:bg-muted/60 hover:text-foreground"
        >
          {t("settings.actionBar.cleanOldRecords")}
        </button>
      </div>
      <div className="space-y-1.5">
        {runs.map((r) => (
          <div key={r.id} className="rounded-lg border border-border bg-muted/15 overflow-hidden">
            <button
              onClick={() => setExpandedId(expandedId === r.id ? null : r.id)}
              className="flex w-full items-center gap-3 px-3.5 py-2 text-left"
            >
              <span className={cn("h-1.5 w-1.5 shrink-0 rounded-full", statusColor(r))} />
              <span className="shrink-0 text-xs font-medium">{r.itemTitle || t("settings.actionBar.untitled")}</span>
              <span className="shrink-0 font-mono text-[10px] uppercase tracking-wider text-muted-foreground">{r.scriptType}</span>
              <span className="shrink-0 text-[11px] text-muted-foreground">{statusLabel(r)}</span>
              <span className="ml-auto shrink-0 text-[11px] text-muted-foreground">
                {r.durationMs != null ? `${r.durationMs}ms` : "—"}
              </span>
            </button>
            {expandedId === r.id && (
              <div className="space-y-2 border-t border-border/50 px-3.5 py-2.5">
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
                {r.errorMsg && <p className="text-xs text-orange-600">{r.errorMsg}</p>}
              </div>
            )}
          </div>
        ))}
      </div>
    </div>
  );
};

// ── 主面板 ──

export default function ActionBarPanel({
  showToast,
}: {
  showToast: (msg: string) => void;
}) {
  const t = useT();
  const [items, setItems] = useState<ActionBarItem[]>([]);
  const [expanded, setExpanded] = useState<Set<number>>(new Set());
  const [editingId, setEditingId] = useState<number | null>(null);
  const [editingForm, setEditingForm] = useState<Partial<ActionBarItem>>({});
  const [draftParentId, setDraftParentId] = useState<number | null | undefined>(undefined);
  const [loaded, setLoaded] = useState(false);
  const [view, setView] = useState<"menu" | "runs" | "edit">("menu");
  const [scopeFilter, setScopeFilter] = useState<"all" | "text" | "file">("all");

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

  // accepts 过滤
  const isItemInScope = (item: ActionBarItem): boolean => {
    if (scopeFilter === "all") return true;
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

  const toggle = useCallback((id: number) => {
    setExpanded((prev) => {
      const next = new Set(prev);
      if (next.has(id)) next.delete(id); else next.add(id);
      return next;
    });
  }, []);

  const submenuIds = items.filter((i) => i.actionType === "submenu").map((i) => i.id);
  const allExpanded = submenuIds.length > 0 && submenuIds.every((id) => expanded.has(id));

  const expandAll = useCallback(() => setExpanded(new Set(submenuIds)), [submenuIds]);
  const collapseAll = useCallback(() => setExpanded(new Set()), []);

  const startEdit = useCallback((item: ActionBarItem) => {
    setDraftParentId(undefined);
    setEditingId(item.id);
    const isExt = item.actionType === "script" && item.actionData.startsWith("/");
    setEditingForm({ ...item, actionType: isExt ? "extension" : item.actionType });
    setView("edit");
  }, []);

  const cancelEdit = useCallback(() => {
    setEditingId(null);
    setEditingForm({});
    setDraftParentId(undefined);
    setView("menu");
  }, []);

  const saveEdit = useCallback(async () => {
    try {
      if (editingForm.actionType === "extension") {
        const actionData = editingForm.actionData || "";
        const [sourcePath, dirName] = actionData.split("|");
        if (!sourcePath || !dirName) {
          showToast(t("settings.actionBar.selectExtFirst"));
          return;
        }
        if (draftParentId !== undefined) {
          await invoke("install_extension", {
            sourcePath, dirName,
            name: editingForm.title || t("settings.actionBar.extName"),
            isAsync: editingForm.isAsync ?? true,
            writeOutputToClipboard: editingForm.writeOutputToClipboard ?? false,
            parentId: draftParentId,
          });
          showToast(t("settings.actionBar.created"));
        } else if (editingId) {
          if (actionData.includes("|")) {
            await invoke("install_extension", {
              sourcePath, dirName,
              name: editingForm.title || t("settings.actionBar.extName"),
              isAsync: editingForm.isAsync ?? true,
              writeOutputToClipboard: editingForm.writeOutputToClipboard ?? false,
              parentId: editingForm.parentId ?? null,
            });
            await invoke("delete_action_bar_item", { id: editingId });
          } else {
            await invoke("update_action_bar_item", {
              id: editingId,
              title: editingForm.title || "",
              icon: editingForm.icon || "",
              actionType: "script",
              actionData: editingForm.actionData || "",
              isEnabled: editingForm.isEnabled ?? true,
              isAsync: editingForm.isAsync ?? true,
              writeOutputToClipboard: editingForm.writeOutputToClipboard ?? false,
              shortcut: editingForm.shortcut || "",
              agent: "",
              accepts: "text",
            });
          }
          showToast(t("settings.actionBar.saved"));
        }
      } else if (draftParentId !== undefined) {
        await invoke("create_action_bar_item", {
          parentId: draftParentId,
          title: editingForm.title || t("settings.actionBar.newMenuItem"),
          icon: "",
          actionType: editingForm.actionType || "copy",
          actionData: editingForm.actionData || "",
          isAsync: editingForm.actionType === "script" ? (editingForm.isAsync ?? true) : true,
          writeOutputToClipboard: editingForm.actionType === "script" ? (editingForm.writeOutputToClipboard ?? false) : false,
          shortcut: editingForm.actionType !== "submenu" ? (editingForm.shortcut || "") : "",
          agent: editingForm.actionType === "agent" ? (editingForm.agent || "") : "",
          accepts: deriveAccepts(editingForm.actionType, editingForm.accepts),
        });
        showToast(t("settings.actionBar.created"));
      } else if (editingId) {
        await invoke("update_action_bar_item", {
          id: editingId,
          title: editingForm.title || "",
          icon: editingForm.icon || "",
          actionType: editingForm.actionType || "copy",
          actionData: editingForm.actionData || "",
          isEnabled: editingForm.isEnabled ?? true,
          isAsync: editingForm.actionType === "script" ? (editingForm.isAsync ?? true) : true,
          writeOutputToClipboard: editingForm.actionType === "script" ? (editingForm.writeOutputToClipboard ?? false) : false,
          shortcut: editingForm.actionType !== "submenu" ? (editingForm.shortcut || "") : "",
          agent: editingForm.actionType === "agent" ? (editingForm.agent || "") : "",
          accepts: deriveAccepts(editingForm.actionType, editingForm.accepts),
        });
        showToast(t("settings.actionBar.saved"));
      }
      cancelEdit();
      refresh();
    } catch (e) {
      showToast(t("settings.actionBar.saveFailed") + e);
    }
  }, [draftParentId, editingId, editingForm, showToast, cancelEdit, refresh]);

  const [deleteConfirmId, setDeleteConfirmId] = useState<number | null>(null);

  const handleDelete = useCallback(
    async (id: number) => {
      if (deleteConfirmId !== id) {
        setDeleteConfirmId(id);
        return;
      }
      try {
        await invoke("delete_action_bar_item", { id });
        showToast(t("settings.actionBar.deleted"));
        setDeleteConfirmId(null);
        refresh();
      } catch (e) {
        showToast(t("settings.actionBar.deleteFailed") + e);
      }
    },
    [deleteConfirmId, showToast, refresh],
  );

  const handleMove = useCallback(
    async (id: number, direction: number) => {
      try {
        await invoke("move_action_bar_item", { id, direction });
        refresh();
      } catch (e) {
        showToast(t("settings.actionBar.moveFailed") + e);
      }
    },
    [refresh, showToast],
  );

  const handleAdd = useCallback((parentId: number | null) => {
    setEditingId(null);
    setDraftParentId(parentId);
    setEditingForm({
      title: t("settings.actionBar.newMenuItem"),
      actionType: "copy",
      actionData: "",
      isEnabled: true,
    });
    setView("edit");
  }, []);

  const nodeCommon = {
    allItems: items, expanded, editingId, editingForm,
    onToggle: toggle, onStartEdit: startEdit, onMove: handleMove,
    onDelete: handleDelete, onAdd: handleAdd, onFormChange: setEditingForm,
    onSaveEdit: saveEdit, onCancelEdit: cancelEdit,
    deleteConfirmId, draftParentId,
  };

  return (
    <div className="w-full min-w-0">
      {/* ── 页头 ── */}
      <div className="mb-6 flex items-center justify-between gap-4">
        <div className="flex items-center gap-2">
          {/* 签名色条——琥珀竖线 */}
          <div className="h-4 w-[3px] rounded-full bg-voice" />
          <span className="font-mono text-[10px] uppercase tracking-widest text-muted-foreground/60">
            {view === "edit"
              ? t("settings.actionBar.editMenuItem")
              : view === "runs"
                ? t("settings.actionBar.scriptRecords")
                : t("settings.actionBar.menuManage")}
          </span>
        </div>

        {/* 操作组 */}
        <div className="flex shrink-0 items-center gap-1.5">
          {view === "menu" && (
            <>
              {/* 场景过滤——分段控件 */}
              <div className="flex items-center rounded-md border border-border overflow-hidden">
                {(["all", "text", "file"] as const).map((s) => (
                  <button
                    key={s}
                    onClick={() => setScopeFilter(s)}
                    className={cn(
                      "px-2.5 py-1.5 text-xs transition-colors",
                      scopeFilter === s
                        ? "bg-voice/12 text-voice font-medium"
                        : "text-muted-foreground hover:bg-muted/60 hover:text-foreground",
                    )}
                  >
                    {s === "all"
                      ? t("settings.actionBar.scopeAll")
                      : s === "text"
                        ? t("settings.actionBar.scopeText")
                        : t("settings.actionBar.scopeFile")}
                  </button>
                ))}
              </div>

              <ToolBtn onClick={() => setView("runs")} variant="outline">
                {t("settings.actionBar.recordsBtn")}
              </ToolBtn>
              <ToolBtn
                onClick={allExpanded ? collapseAll : expandAll}
                variant="outline"
                title={allExpanded ? t("settings.actionBar.collapseAll") : t("settings.actionBar.expandAll")}
              >
                {allExpanded ? <ChevronsDownUp className="h-3.5 w-3.5" /> : <ChevronsUpDown className="h-3.5 w-3.5" />}
                <span className="hidden sm:inline">
                  {allExpanded ? t("settings.actionBar.collapseAll") : t("settings.actionBar.expandAll")}
                </span>
              </ToolBtn>
              <ToolBtn onClick={() => handleAdd(null)} variant="solid">
                <Plus className="h-4 w-4" /> {t("settings.actionBar.addMainItem")}
              </ToolBtn>
            </>
          )}
          {view !== "menu" && (
            <ToolBtn onClick={() => setView("menu")} variant="outline">
              <ArrowLeft className="h-3.5 w-3.5" />
              {ti18n("settings.actionBar.backToMenu")}
            </ToolBtn>
          )}
        </div>
      </div>

      {/* ── 内容区 ── */}
      {view === "edit" ? (
        <EditForm
          form={editingForm}
          isSystem={(editingId !== null && items.find((i) => i.id === editingId)?.isSystem) ?? false}
          onChange={setEditingForm}
          onSave={saveEdit}
          onCancel={cancelEdit}
        />
      ) : view === "runs" ? (
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
          <ToolBtn onClick={() => handleAdd(null)} variant="solid">
            <Plus className="h-4 w-4" /> {t("settings.actionBar.addMainItem")}
          </ToolBtn>
        </div>
      ) : (
        <div className="space-y-px">
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
