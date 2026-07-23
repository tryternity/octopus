import { useState, useEffect, useCallback, useRef } from "react";
import { invoke } from "@tauri-apps/api/core";
import { emit } from "@tauri-apps/api/event";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import {
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
import ShortcutButton from "@/components/ShortcutButton";
import { Button } from "@/components/ui/button";
import { Toggle as UIToggle } from "@/components/ui/toggle";
import { UnderlineTabs, Segmented } from "@/components/ui/tabs";
import AppPicker from "./ActionBar/AppPicker";

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
  triggerKeyword?: string;
  globalShortcut?: string;
  needVoice?: boolean;
  /** JSON 数组字符串 ["com.apple.Safari"]，空串/undefined=全局项 */
  appBundleIds?: string;
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
];

function deriveAccepts(actionType: string | undefined, explicit?: string): string {
  if (explicit) return explicit;
  if (actionType === "agent" || actionType === "copy_path") return "file";
  if (actionType === "submenu") return "any";
  return "text";
}

const pad2 = (n: number) => String(n).padStart(2, "0");

// ── 统一控件样式 ──
// ActionBar 表单用稍大 padding（px-3 py-2）比共享 Input 默认（px-2.5 py-1.5）更宽松，
// 适配树形编辑器的密集表单。focus 规格与共享 Input 对齐（voice/50 + ring-2 voice/15）。
const inputBase = "w-full bg-background border border-border rounded-md px-3 py-2 text-sm outline-none transition-all focus:border-voice/50 focus:ring-2 focus:ring-voice/15";

// ── 类型标签 ──
const TypeTag = ({ type, variant = "dot" }: { type: string; variant?: "dot" | "solid" }) => {
  // fallback 到 url——历史 DB 残留的未知类型（如已删除的 copy）显示为灰色 UNKNOWN
  const meta = TYPE_META[type] ?? { bar: "bg-stone-400", dot: "bg-stone-400", label: type.toUpperCase().slice(0, 8) || "UNKNOWN", descKey: "", placeholderKey: "" };
  return (
    <span className="inline-flex items-center gap-1 font-mono text-[10px] uppercase tracking-wider text-muted-foreground">
      {variant === "dot" && <span className={cn("h-1.5 w-1.5 rounded-full", meta.dot)} />}
      {meta.label}
    </span>
  );
};

// Toggle 本地包装：适配 ActionBar 的 (checked, onChange(v)) 签名到共享 UIToggle (on, onClick)。
const Toggle = ({ checked, onChange }: { checked: boolean; onChange: (v: boolean) => void }) => (
  <UIToggle on={checked} onClick={() => onChange(!checked)} />
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
            className="shrink-0 rounded-md p-1 text-muted-foreground/60 transition-colors hover:bg-destructive/10 hover:text-destructive"
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
      {error && <p className="text-[11px] text-destructive">{error}</p>}
    </div>
  );
};

// ── 表单字段行 ──
const FormField = ({
  label, children, hint, className,
}: {
  label: string; children: React.ReactNode; hint?: string; className?: string;
}) => (
  <div className={cn("space-y-1.5", className)}>
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
  const type = form.actionType || "url";
  const meta = TYPE_META[type];
  const showContent = type !== "submenu" && type !== "extension" && type !== "copy_path";
  const showShortcut = type !== "submenu";
  const [adapters, setAdapters] = useState<{key:string;displayName:string;isAvailable:boolean}[]>([]);
  const [capturingGlobal, setCapturingGlobal] = useState(false);
  useEffect(() => {
    invoke<{key:string;displayName:string;isAvailable:boolean}[]>("list_agent_adapters").then(setAdapters).catch(() => {});
  }, []);

  // 全局快捷键录制：capturingGlobal=true 时拦截按键，组装为 Tauri shortcut 字符串，
  // 经 check_shortcut 校验后写回表单。Esc 退出录制，纯修饰键等待实际按键。
  // 监听器生命周期绑定到 capturingGlobal：结束/卸载时自动 removeEventListener。
  useEffect(() => {
    if (!capturingGlobal) return;
    const handler = async (e: KeyboardEvent) => {
      e.preventDefault();
      e.stopPropagation();
      if (e.key === "Escape") { setCapturingGlobal(false); return; }
      if (e.key === "Alt" || e.key === "Shift" || e.key === "Control" || e.key === "Meta") return;
      // Backspace/Delete 清空快捷键
      if (e.key === "Backspace" || e.key === "Delete") {
        onChange({ ...form, globalShortcut: "" });
        setCapturingGlobal(false);
        return;
      }
      const parts: string[] = [];
      if (e.metaKey || e.ctrlKey) parts.push("CmdOrCtrl");
      if (e.altKey) parts.push("Alt");
      if (e.shiftKey) parts.push("Shift");
      const keyName = e.code.startsWith("Key") ? e.code.slice(3) : e.code;
      parts.push(keyName);
      const shortcut = parts.join("+");
      try {
        await invoke("check_shortcut", { shortcut });
        onChange({ ...form, globalShortcut: shortcut });
      } catch (err) {
        // 校验失败（格式/占用）——不改写表单，仅退出录制
        console.warn("[action-bar] global shortcut check failed:", err);
      }
      setCapturingGlobal(false);
    };
    document.addEventListener("keydown", handler, true);
    return () => document.removeEventListener("keydown", handler, true);
  }, [capturingGlobal, form, onChange]);

  return (
    <div className="flex h-full flex-col gap-3">
      {/* 导航栏 */}
      <div className="flex shrink-0 items-center gap-3 border-b border-border/40 pb-2">
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

      {/* 单卡片紧凑表单——flex-1 overflow-y-auto 内部滚动，消除外层双滚动条 */}
      <div className="flex min-h-0 flex-1 flex-col gap-3 overflow-y-auto rounded-lg border border-border/50 bg-muted/15 p-4">
        {/* 标题 —— 占一行 */}
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

        {/* 类型 + 启用 —— 一行 */}
        <div className="grid grid-cols-2 gap-3">
          <FormField label={t("settings.actionBar.typeLabel")}>
            <select
              className={cn(inputBase, "disabled:opacity-60")}
              value={type}
              disabled={isSystem}
              onChange={(e) => {
                const newType = e.target.value;
                onChange({ ...form, actionType: newType, accepts: deriveAccepts(newType, undefined) });
              }}
            >
              {ACTION_TYPES.map((at) => (
                <option key={at.value} value={at.value}>{t(at.labelKey)}</option>
              ))}
            </select>
          </FormField>
          <FormField label={t("settings.actionBar.enableLabel")}>
            <div className="flex h-[38px] items-center gap-2.5">
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

        {/* 快捷键 + 全局快捷键 —— 一行（仅叶子菜单 submenu 不显示） */}
        {showShortcut && (
          <div className="grid grid-cols-2 gap-3">
            <FormField label={t("settings.actionBar.shortcutLabel")}>
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
                {form.shortcut && (
                  <button
                    onClick={() => onChange({ ...form, shortcut: "" })}
                    className="rounded p-1 text-muted-foreground/60 transition-colors hover:bg-destructive/10 hover:text-destructive"
                  >
                    <X className="h-3.5 w-3.5" />
                  </button>
                )}
              </div>
            </FormField>
            <FormField label={ti18n("settings.actionBar.globalShortcutLabel")}>
              <div className="flex items-center gap-2">
                <ShortcutButton
                  shortcut={form.globalShortcut ?? ""}
                  capturing={capturingGlobal}
                  onClick={() => setCapturingGlobal((v) => !v)}
                />
                {form.globalShortcut && (
                  <button
                    onClick={() => onChange({ ...form, globalShortcut: "" })}
                    className="rounded p-1 text-muted-foreground/60 transition-colors hover:bg-destructive/10 hover:text-destructive"
                    aria-label={ti18n("settings.actionBar.clearShortcut")}
                  >
                    <X className="h-3.5 w-3.5" />
                  </button>
                )}
              </div>
            </FormField>
          </div>
        )}

        {/* 执行选项（仅 script）—— 放在内容前面 */}
        {type === "script" && (
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
        )}

        {/* 搜索关键词（仅 URL）—— 放在内容前面 */}
        {type === "url" && (
          <FormField label={t("settings.actionBar.triggerKeywordLabel")}>
            <div className="flex items-center gap-2">
              <input
                className="w-28 bg-background border border-border rounded-md px-3 py-1.5 text-sm font-mono outline-none transition-all focus:border-voice/50 focus:ring-2 focus:ring-voice/15"
                placeholder="tr"
                value={form.triggerKeyword || ""}
                onChange={(e) => {
                  const val = e.target.value.trim().toLowerCase();
                  onChange({ ...form, triggerKeyword: val });
                }}
              />
              <span className="text-[11px] text-muted-foreground/60">
                {t("settings.actionBar.triggerKeywordHint")}
              </span>
            </div>
          </FormField>
        )}

        {/* 类型特定配置 —— 条件区，inline 在同一卡片内 */}
        {type === "extension" && <ExtensionDropZone form={form} onChange={onChange} />}

        {/* Agent 选择 —— 仅 agent 类型，在内容前一行。
            选「默认 Agent」(空值) 时，运行时按三层 fallback 解析（详见 agent_adapter.rs）。
            need_voice 不暴露给用户——保存时后端按 action_data 含 {{task}} 自动判定。 */}
        {type === "agent" && (
          <FormField label={t("settings.actionBar.agentLabel")}>
            <select
              className={inputBase}
              value={form.agent || ""}
              onChange={(e) => onChange({ ...form, agent: e.target.value })}
            >
              <option value="">{t("settings.actionBar.defaultAgentOption")}</option>
              {adapters.map((a) => (
                <option key={a.key} value={a.key}>
                  {a.displayName}{a.isAvailable ? "" : `（${t("settings.actionBar.agentNotInstalled")}）`}
                </option>
              ))}
            </select>
          </FormField>
        )}

        {/* 内容 textarea —— 固定高度，resize-y 可手动拉大（放最后一行） */}
        {showContent && (
          <FormField label={t("settings.actionBar.contentLabel")}>
            <textarea
              className="w-full min-h-[190px] resize-y bg-background border border-border rounded-md px-3 py-2 font-mono text-xs leading-relaxed outline-none transition-all focus:border-voice/50 focus:ring-2 focus:ring-voice/15"
              placeholder={meta.placeholderKey ? t(meta.placeholderKey) : ""}
              value={form.actionData || ""}
              onChange={(e) => onChange({ ...form, actionData: e.target.value })}
            />
          </FormField>
        )}

        {type === "copy_path" && (
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
        )}

        {/* App 绑定——所有类型通用。空=全局命令（所有 app 显示），绑定=仅指定 app 显示 */}
        <AppPicker
          value={form.appBundleIds ?? ""}
          onChange={(v) => onChange({ ...form, appBundleIds: v })}
        />
      </div>

      {/* 底部操作栏——右下角保存/取消（shrink-0 不被压缩） */}
      <div className="flex shrink-0 justify-end gap-2.5">
        <Button variant="outline" size="sm" onClick={onCancel}>
          {t("settings.actionBar.cancel")}
        </Button>
        <Button variant="voice" size="sm" onClick={onSave}>
          {t("settings.actionBar.save")}
        </Button>
      </div>
    </div>
  );
};
// 2026-07-17 重构：原 TreeNode 树形渲染改为左右分栏，此组件承担行渲染。
// 2026-07-20 扩展：右栏子菜单列表传 showShortcuts=true 时，标题右侧填上 local + global
// 快捷键槽（点击进入 inline 录制），避免原 1fr 列右侧大面积空白。左栏主菜单不传→布局不变。
interface MenuRowProps {
  item: ActionBarItem;
  index: number;            // 1-based 序号
  selected: boolean;        // 主菜单选中态（子菜单恒 false）
  isFirst: boolean;
  isLast: boolean;
  deleteConfirmId: number | null;
  isMain?: boolean;         // 主菜单（加亮加粗）；子菜单不传
  onSelect?: () => void;    // 主菜单点击选中
  onMove: (dir: number) => void;
  onEdit: () => void;
  onDelete: () => void;
  // ── 快捷键槽（右栏子菜单列表专用，2026-07-20）──
  showShortcuts?: boolean;                                    // true 时渲染快捷键列
  onCaptureShortcut?: (kind: "local" | "global") => void;    // 点击槽位进入录制
  capturingKind?: "local" | "global" | null;                  // 当前行处于录制的 kind
  onClearShortcut?: (kind: "local" | "global") => void;       // 清除快捷键
}

const MenuRow = (props: MenuRowProps) => {
  const t = useT();
  const { item, index, selected, isFirst, isLast, deleteConfirmId } = props;
  const meta = TYPE_META[item.actionType] ?? { bar: "bg-stone-400", dot: "bg-stone-400", label: (item.actionType || "unknown").toUpperCase().slice(0, 8), descKey: "", placeholderKey: "" };
  const isDeleting = deleteConfirmId === item.id;
  // 子菜单项行（非 submenu 类型）才显示快捷键槽——submenu 是容器，本身无快捷键
  const showShortcuts = !!props.showShortcuts && item.actionType !== "submenu";

  return (
    <div
      onClick={props.onSelect}
      className={cn(
        "group relative grid items-center gap-x-2 gap-y-0.5 rounded-md py-1.5 pl-1 pr-1.5 transition-colors",
        // 默认 4 列；showShortcuts 时 5 列（多一列放快捷键槽）
        showShortcuts
          ? "[grid-template-columns:auto_auto_minmax(60px,1fr)_auto_auto]"
          : "[grid-template-columns:auto_auto_1fr_auto]",
        selected ? "bg-voice/12" : "hover:bg-muted/40",
        props.onSelect && "cursor-pointer",
      )}
    >
      {/* 签名元素：左侧类型色条（col 1，跨两行） */}
      <div className={cn("row-span-2 h-full w-[3px] shrink-0 self-stretch rounded-full transition-all", meta.bar)} />

      {/* 序号（col 2，跨两行） */}
      <span className="row-span-2 self-start pt-0.5 text-right font-mono text-[11px] tabular-nums text-muted-foreground/50">
        {pad2(index)}
      </span>

      {/* 标题行（col 3） */}
      <span className={cn(
        "min-w-0 truncate",
        props.isMain ? "text-sm font-semibold" : "text-sm",
        item.isEnabled ? "text-foreground" : "text-muted-foreground/50",
      )}>
        {item.title}
      </span>

      {/* 快捷键槽（col 4，仅 showShortcuts 时）——global 在前 + local 贴右
          顺序对调（2026-07-20）：global 录制时 ShortcutButton 文字变长，放在左边
          可向左扩展挤标题而不动 local 槽；local 贴右紧邻操作按钮，宽度恒定。 */}
      {showShortcuts && (
        <div className="flex shrink-0 items-center justify-end gap-2">
          {/* global 快捷键：ShortcutButton（含录制态自渲染） */}
          <div className="flex items-center gap-0.5">
            <ShortcutButton
              shortcut={item.globalShortcut ?? ""}
              capturing={props.capturingKind === "global"}
              onClick={() => props.onCaptureShortcut?.("global")}
              title={t("settings.actionBar.globalShortcutHint")}
            />
            {item.globalShortcut && props.capturingKind !== "global" && (
              <button
                onClick={(e) => { e.stopPropagation(); props.onClearShortcut?.("global"); }}
                className="rounded p-0.5 text-muted-foreground/50 opacity-0 transition-opacity hover:bg-destructive/10 hover:text-destructive group-hover:opacity-100"
                aria-label={t("settings.actionBar.clearShortcut")}
              >
                <X className="h-3 w-3" />
              </button>
            )}
          </div>

          {/* 分隔点 */}
          <span className="text-muted-foreground/30 text-[10px]">·</span>

          {/* local 快捷键：⌥ + 单字符 / 占位 */}
          <div className="flex items-center gap-0.5">
            <span className="text-[10px] text-muted-foreground/50 font-mono">⌥</span>
            <button
              onClick={(e) => { e.stopPropagation(); props.onCaptureShortcut?.("local"); }}
              className={cn(
                "min-w-[22px] text-center rounded border px-1 py-0.5 text-[11px] font-mono transition-all",
                props.capturingKind === "local"
                  ? "border-voice ring-2 ring-voice/15 bg-voice/5 text-voice animate-pulse"
                  : item.shortcut
                    ? "border-border bg-muted/40 text-foreground hover:border-foreground/30"
                    : "border-dashed border-muted-foreground/30 text-muted-foreground/40 hover:border-foreground/30 hover:text-muted-foreground/70",
              )}
              title={t("settings.actionBar.shortcutHint")}
            >
              {props.capturingKind === "local"
                ? "…"
                : (item.shortcut || "—")}
            </button>
            {item.shortcut && props.capturingKind !== "local" && (
              <button
                onClick={(e) => { e.stopPropagation(); props.onClearShortcut?.("local"); }}
                className="rounded p-0.5 text-muted-foreground/50 opacity-0 transition-opacity hover:bg-destructive/10 hover:text-destructive group-hover:opacity-100"
                aria-label={t("settings.actionBar.clearShortcut")}
              >
                <X className="h-3 w-3" />
              </button>
            )}
          </div>
        </div>
      )}

      {/* 悬浮操作栏（最后一列，第一行） */}
      <div className="flex shrink-0 items-center gap-0.5 opacity-0 transition-opacity focus-within:opacity-100 group-hover:opacity-100">
        <button
          onClick={(e) => { e.stopPropagation(); props.onMove(-1); }}
          disabled={isFirst}
          className="rounded p-0.5 text-muted-foreground transition-colors hover:text-foreground disabled:opacity-25"
          aria-label={t("settings.actionBar.moveUp")}
        >
          <ArrowUp className="h-3.5 w-3.5" />
        </button>
        <button
          onClick={(e) => { e.stopPropagation(); props.onMove(1); }}
          disabled={isLast}
          className="rounded p-0.5 text-muted-foreground transition-colors hover:text-foreground disabled:opacity-25"
          aria-label={t("settings.actionBar.moveDown")}
        >
          <ArrowDown className="h-3.5 w-3.5" />
        </button>
        <button
          onClick={(e) => { e.stopPropagation(); props.onEdit(); }}
          className="rounded p-0.5 text-muted-foreground transition-colors hover:text-foreground"
          aria-label={t("settings.actionBar.edit")}
        >
          <Pencil className="h-3.5 w-3.5" />
        </button>
        <button
          onClick={(e) => { e.stopPropagation(); props.onDelete(); }}
          disabled={item.isSystem}
          className={cn(
            "rounded p-0.5 transition-colors disabled:opacity-25",
            isDeleting
              ? "bg-destructive text-destructive-foreground hover:opacity-90"
              : "text-muted-foreground hover:text-destructive",
          )}
          aria-label={t("settings.actionBar.delete")}
          title={isDeleting ? t("settings.actionBar.deleteConfirm") : t("settings.actionBar.delete")}
        >
          {isDeleting ? (
            <span className="px-1 text-[10px] font-medium">{t("settings.actionBar.confirm")}</span>
          ) : (
            <Trash2 className="h-3.5 w-3.5" />
          )}
        </button>
      </div>

      {/* 第二行：类型 + 内置/隐藏 小字（跨最后两列，给足空间防 wrap） */}
      <div className={cn("flex items-center gap-1.5 text-[10px] text-muted-foreground/60", showShortcuts ? "col-span-2" : "col-span-2")}>
        <TypeTag type={item.actionType} />
        {item.isSystem && (
          <span className="text-muted-foreground/40">
            · {t("settings.actionBar.builtin")}
          </span>
        )}
        {!item.isEnabled && (
          <span className="text-muted-foreground/40">
            · {t("settings.actionBar.hidden")}
          </span>
        )}
      </div>
    </div>
  );
};

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
  const [selectedIds, setSelectedIds] = useState<Set<number>>(new Set());

  const refresh = useCallback(async () => {
    try {
      const list = await invoke<ScriptRun[]>("list_script_runs", { limit: 100 });
      setRuns(list);
    } catch {
      // 静默——脚本记录列表加载失败不应阻塞设置页
    }
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

  const toggleSelect = useCallback((id: number) => {
    setSelectedIds((prev) => {
      const next = new Set(prev);
      if (next.has(id)) next.delete(id); else next.add(id);
      return next;
    });
  }, []);

  const allSelected = runs.length > 0 && selectedIds.size === runs.length;
  const toggleSelectAll = useCallback(() => {
    setSelectedIds(allSelected ? new Set() : new Set(runs.map((r) => r.id)));
  }, [allSelected, runs]);

  const handleDeleteSelected = useCallback(async () => {
    if (selectedIds.size === 0) return;
    try {
      await invoke("delete_script_runs", { ids: Array.from(selectedIds) });
      showToast(t("settings.actionBar.deleted"));
      setSelectedIds(new Set());
      refresh();
    } catch (e) {
      showToast(t("settings.actionBar.deleteFailed") + e);
    }
  }, [selectedIds, showToast, refresh]);

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
    if (r.exitCode === null) return "bg-warning";
    return r.exitCode === 0 ? "bg-success" : "bg-destructive";
  };
  const statusLabel = (r: ScriptRun) => {
    if (r.exitCode === null) return r.errorMsg || t("settings.actionBar.statusError");
    return r.exitCode === 0 ? t("settings.actionBar.statusSuccess") : t("settings.actionBar.statusFailed", { n: r.exitCode });
  };

  return (
    <div>
      {/* 顶部工具栏：全选 + 删除选中 + 清理旧记录 */}
      <div className="mb-3 flex items-center gap-3">
        <label className="flex items-center gap-1.5 text-xs text-muted-foreground cursor-pointer">
          <input
            type="checkbox"
            checked={allSelected}
            onChange={toggleSelectAll}
            className="h-3.5 w-3.5 accent-voice"
          />
          {t("settings.actionBar.selectAll")}
        </label>
        <Button
          variant="outline"
          size="sm"
          disabled={selectedIds.size === 0}
          onClick={handleDeleteSelected}
        >
          {t("settings.actionBar.deleteSelected")} ({selectedIds.size})
        </Button>
        <div className="ml-auto">
          <Button variant="outline" size="sm" onClick={handleClear}>
            {t("settings.actionBar.cleanOldRecords")}
          </Button>
        </div>
      </div>
      <div className="space-y-1.5">
        {runs.map((r) => (
          <div key={r.id} className={cn(
            "rounded-lg border bg-muted/15 overflow-hidden transition-colors",
            selectedIds.has(r.id) ? "border-voice/40" : "border-border",
          )}>
            <div className="flex items-center gap-3 px-3 py-2">
              <input
                type="checkbox"
                checked={selectedIds.has(r.id)}
                onChange={() => toggleSelect(r.id)}
                className="h-3.5 w-3.5 shrink-0 accent-voice"
              />
            <button
              onClick={() => setExpandedId(expandedId === r.id ? null : r.id)}
              className="flex flex-1 items-center gap-3 text-left"
            >
              <span className={cn("h-1.5 w-1.5 shrink-0 rounded-full", statusColor(r))} />
              <span className="shrink-0 text-xs font-medium">{r.itemTitle || t("settings.actionBar.untitled")}</span>
              <span className="shrink-0 font-mono text-[10px] uppercase tracking-wider text-muted-foreground">{r.scriptType}</span>
              <span className="shrink-0 text-[11px] text-muted-foreground">{statusLabel(r)}</span>
              <span className="ml-auto shrink-0 text-[11px] text-muted-foreground">
                {r.durationMs != null ? `${r.durationMs}ms` : "—"}
              </span>
            </button>
            </div>
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
                    <p className="mb-1 font-mono text-[10px] uppercase tracking-wider text-destructive/70">stderr</p>
                    <textarea
                      readOnly
                      className="w-full min-h-[40px] resize-y bg-background border border-border rounded px-2 py-1.5 font-mono text-xs leading-relaxed text-destructive/80"
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
  const [editingId, setEditingId] = useState<number | null>(null);
  const [editingForm, setEditingForm] = useState<Partial<ActionBarItem>>({});
  const [draftParentId, setDraftParentId] = useState<number | null | undefined>(undefined);
  const [loaded, setLoaded] = useState(false);
  // tab: 命令管理 / 执行记录（替代原 view）。edit 走独立 editingId 判定（inline 全屏 EditForm）
  const [tab, setTab] = useState<"menu" | "runs">("menu");
  const [scopeFilter, setScopeFilter] = useState<"text" | "file">("text");
  // 左栏选中主菜单 id——首次进 menu tab 默认选第一个；删除/过滤后自动 fallback
  const [selectedMainMenuId, setSelectedMainMenuId] = useState<number | null>(null);
  // 标题 inline 输入的本地 draft——避免每按键 IPC（IME 中文输入会被打断）。
  // null = 显示 selectedMain.title（已落库值）；非 null = 用户正在输入的草稿。
  // onBlur 或 300ms debounce 后 flush 到 updateMainInline，然后置回 null。
  // 2026-07-17 review Critical #1 修复。
  const [titleDraft, setTitleDraft] = useState<string | null>(null);
  const titleDraftRef = useRef<string | null>(null);
  titleDraftRef.current = titleDraft;
  // 主菜单 inline 编辑（右栏顶部表单）的实时字段镜像 + 录制态
  const [inlineCapturingShortcut, setInlineCapturingShortcut] = useState(false);
  const [inlineCapturingGlobal, setInlineCapturingGlobal] = useState(false);
  // 子菜单项行内快捷键录制目标——{id, kind} 或 null。同时只允许一个槽录制。
  // 2026-07-20：右栏子菜单列表每行直接编辑快捷键，避免为每个子项开 EditForm 弹窗。
  const [subCapturing, setSubCapturing] = useState<{ id: number; kind: "local" | "global" } | null>(null);

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

  // selectedMainMenuId 兜底：未选 / 被删 / 不在 scope 过滤结果 → fallback 第一个
  const effectiveSelectedId = selectedMainMenuId !== null && mainItems.some((m) => m.id === selectedMainMenuId)
    ? selectedMainMenuId
    : mainItems[0]?.id ?? null;
  const selectedMain = effectiveSelectedId !== null
    ? items.find((i) => i.id === effectiveSelectedId) ?? null
    : null;
  // 选中主菜单的子菜单列表
  const selectedSubs = selectedMain !== null
    ? items.filter((i) => i.parentId === selectedMain.id)
    : [];

  // 首次加载完 / scope 过滤变化时，selectedMainMenuId 自动跟第一个 mainItem
  useEffect(() => {
    if (tab === "menu" && effectiveSelectedId === null && mainItems.length > 0) {
      setSelectedMainMenuId(mainItems[0].id);
    }
  }, [tab, effectiveSelectedId, mainItems]);

  const startEdit = useCallback((item: ActionBarItem) => {
    setDraftParentId(undefined);
    setEditingId(item.id);
    // extension 判断：action_data 是已安装的脚本绝对路径（非内联脚本）
    // shebang 行（#!/bin/sh）以 # 开头，不会误判
    const p = item.actionData.trim();
    const isExt = item.actionType === "script" && p.length > 0 && !p.startsWith("#") &&
      (p.startsWith("/") || /^[A-Za-z]:[\\/]/.test(p));
    setEditingForm({ ...item, actionType: isExt ? "extension" : item.actionType });
    // 不动 tab——用 editingId !== null 判定 EditForm 全屏覆盖
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
        const shortcut = editingForm.shortcut || "";
        const isEnabled = editingForm.isEnabled ?? true;
        if (draftParentId !== undefined) {
          // 新建：必须先选扩展包
          if (!hasNewPkg) {
            showToast(t("settings.actionBar.selectExtFirst"));
            return;
          }
          const [sourcePath, dirName] = actionData.split("|");
          const newId = await invoke<number>("install_extension", {
            sourcePath, dirName, name: extName, isAsync,
            writeOutputToClipboard: writeOutput,
            parentId: draftParentId, shortcut, isEnabled,
          });
          await invoke("set_global_shortcut", { id: newId, globalShortcut: editingForm.globalShortcut ?? "" });
          showToast(t("settings.actionBar.created"));
        } else if (editingId) {
          if (hasNewPkg) {
            // 重选扩展包：复制新包 + update 现有记录（保持 id/位置，不再 install+delete）
            const [sourcePath, dirName] = actionData.split("|");
            await invoke("install_extension", {
              sourcePath, dirName, name: extName, isAsync,
              writeOutputToClipboard: writeOutput,
              parentId: editingForm.parentId ?? null,
              shortcut, isEnabled, replaceId: editingId,
            });
          } else {
            // 未重选：直接 update（actionData 保持原脚本路径）
            await invoke("update_action_bar_item", {
              id: editingId,
              title: editingForm.title || "",
              icon: editingForm.icon || "",
              actionType: "script",
              actionData: editingForm.actionData || "",
              isEnabled,
              isAsync,
              writeOutputToClipboard: writeOutput,
              shortcut,
              agent: "",
              accepts: "text",
              triggerKeyword: "",
              appBundleIds: editingForm.appBundleIds ?? "",
            });
          }
          await invoke("set_global_shortcut", { id: editingId, globalShortcut: editingForm.globalShortcut ?? "" });
          showToast(t("settings.actionBar.saved"));
        }
      } else if (draftParentId !== undefined) {
        const createdId = await invoke<number>("create_action_bar_item", {
          parentId: draftParentId,
          title: editingForm.title || t("settings.actionBar.newMenuItem"),
          icon: "",
          actionType: editingForm.actionType || "url",
          actionData: editingForm.actionData || "",
          isAsync: editingForm.actionType === "script" ? (editingForm.isAsync ?? true) : true,
          writeOutputToClipboard: editingForm.actionType === "script" ? (editingForm.writeOutputToClipboard ?? false) : false,
          shortcut: editingForm.actionType !== "submenu" ? (editingForm.shortcut || "") : "",
          agent: editingForm.actionType === "agent" ? (editingForm.agent || "") : "",
          // submenu 用 any（两个 scope 都显示）；其他锁定为 handleAdd 设的 scopeFilter 值
          accepts: editingForm.actionType === "submenu" ? "any" : (editingForm.accepts || "text"),
          triggerKeyword: editingForm.actionType === "url" ? (editingForm.triggerKeyword || "") : "",
          isEnabled: editingForm.isEnabled ?? true,
          appBundleIds: editingForm.appBundleIds ?? "",
        });
        // 新建非 submenu 项时设置全局快捷键（Quick Execute）
        if (editingForm.actionType !== "submenu") {
          await invoke("set_global_shortcut", { id: createdId, globalShortcut: editingForm.globalShortcut ?? "" });
        }
        showToast(t("settings.actionBar.created"));
      } else if (editingId) {
        await invoke("update_action_bar_item", {
          id: editingId,
          title: editingForm.title || "",
          icon: editingForm.icon || "",
          actionType: editingForm.actionType || "url",
          actionData: editingForm.actionData || "",
          isEnabled: editingForm.isEnabled ?? true,
          isAsync: editingForm.actionType === "script" ? (editingForm.isAsync ?? true) : true,
          writeOutputToClipboard: editingForm.actionType === "script" ? (editingForm.writeOutputToClipboard ?? false) : false,
          shortcut: editingForm.actionType !== "submenu" ? (editingForm.shortcut || "") : "",
          agent: editingForm.actionType === "agent" ? (editingForm.agent || "") : "",
          accepts: deriveAccepts(editingForm.actionType, editingForm.accepts),
          triggerKeyword: editingForm.actionType === "url" ? (editingForm.triggerKeyword || "") : "",
          appBundleIds: editingForm.appBundleIds ?? "",
        });
        // global_shortcut 单独更新（非 submenu 类型）
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
    // 新建项的 accepts 锁定为当前 scopeFilter——在文本类下只能建文本类菜单，
    // 文件类下只能建文件类。submenu 用 any（两个场景都显示）。
    // 用户不能在表单里改 accepts——没有 UI 控件，saveEdit 时用此值。
    //
    // 默认 actionType 按层级区分：
    //   - 主菜单（parentId=null）→ submenu（父菜单）——最常见的主菜单语义是分组容器
    //   - 子菜单（parentId=某 id）→ script（脚本）——子菜单通常是实际执行动作
    setEditingForm({
      title: t("settings.actionBar.newMenuItem"),
      actionType: parentId === null ? "submenu" : "script",
      actionData: "",
      isEnabled: true,
      accepts: parentId === null ? "any" : (scopeFilter === "file" ? "file" : "text"),
    });
    // 不动 tab——editingId=null + draftParentId !== undefined 表示新建模式，EditForm 全屏覆盖
  }, [scopeFilter, t]);

  // 任意 item 的 inline 更新（主菜单 + 子菜单共用）。patch 合并到 item 后调后端
  // update_action_bar_item + set_global_shortcut，失败 refresh 重置 UI。
  // 2026-07-20：从 updateMainInline 抽出参数化版本，子菜单项行内编辑快捷键复用。
  const updateItemInline = useCallback(async (item: ActionBarItem, patch: Partial<ActionBarItem>) => {
    const merged = { ...item, ...patch };
    try {
      await invoke("update_action_bar_item", {
        id: merged.id,
        title: merged.title,
        icon: merged.icon || "",
        actionType: merged.actionType || "url",
        actionData: merged.actionData || "",
        isEnabled: merged.isEnabled,
        isAsync: merged.isAsync ?? true,
        writeOutputToClipboard: merged.writeOutputToClipboard ?? false,
        shortcut: merged.actionType !== "submenu" ? (merged.shortcut || "") : "",
        agent: merged.agent || "",
        accepts: deriveAccepts(merged.actionType, merged.accepts),
        triggerKeyword: merged.triggerKeyword || "",
        appBundleIds: merged.appBundleIds ?? "",
      });
      if (merged.actionType !== "submenu") {
        await invoke("set_global_shortcut", { id: merged.id, globalShortcut: merged.globalShortcut ?? "" });
      }
      refresh();
    } catch (e) {
      // 失败时也 refresh——把 UI 重置回后端真实状态（防 input/Toggle 视觉态停在
      // 用户输入的新值但实际未落库的混乱）。2026-07-17 review Important #5 修复。
      showToast(t("settings.actionBar.saveFailed") + e);
      refresh();
    }
  }, [refresh, showToast]);

  // 主菜单 inline 更新——updateItemInline 的 selectedMain wrapper（保留所有现有调用点不变）。
  const updateMainInline = useCallback((patch: Partial<ActionBarItem>) => {
    if (selectedMain === null) return;
    void updateItemInline(selectedMain, patch);
  }, [selectedMain, updateItemInline]);

  // inline 全局快捷键录制（复用 EditForm 范式）
  useEffect(() => {
    if (!inlineCapturingGlobal || selectedMain === null) return;
    const handler = async (e: KeyboardEvent) => {
      e.preventDefault();
      e.stopPropagation();
      if (e.key === "Escape") { setInlineCapturingGlobal(false); return; }
      if (e.key === "Alt" || e.key === "Shift" || e.key === "Control" || e.key === "Meta") return;
      if (e.key === "Backspace" || e.key === "Delete") {
        updateMainInline({ globalShortcut: "" });
        setInlineCapturingGlobal(false);
        return;
      }
      const parts: string[] = [];
      if (e.metaKey || e.ctrlKey) parts.push("CmdOrCtrl");
      if (e.altKey) parts.push("Alt");
      if (e.shiftKey) parts.push("Shift");
      const keyName = e.code.startsWith("Key") ? e.code.slice(3) : e.code;
      parts.push(keyName);
      const sc = parts.join("+");
      try {
        await invoke("check_shortcut", { shortcut: sc });
        updateMainInline({ globalShortcut: sc });
      } catch (err) {
        console.warn("[action-bar] inline global shortcut check failed:", err);
      }
      setInlineCapturingGlobal(false);
    };
    document.addEventListener("keydown", handler, true);
    return () => document.removeEventListener("keydown", handler, true);
  }, [inlineCapturingGlobal, selectedMain, updateMainInline]);

  // inline Alt+字符快捷键录制（与 EditForm 同范式）
  useEffect(() => {
    if (!inlineCapturingShortcut || selectedMain === null) return;
    const handler = (e: KeyboardEvent) => {
      e.preventDefault();
      e.stopPropagation();
      if (e.key === "Escape") { setInlineCapturingShortcut(false); return; }
      if (e.key === "Backspace" || e.key === "Delete") {
        updateMainInline({ shortcut: "" });
        setInlineCapturingShortcut(false);
        return;
      }
      const ch = e.key.toLowerCase();
      if (/^[0-9a-z]$/.test(ch)) {
        updateMainInline({ shortcut: ch });
        setInlineCapturingShortcut(false);
      }
    };
    document.addEventListener("keydown", handler, true);
    return () => document.removeEventListener("keydown", handler, true);
  }, [inlineCapturingShortcut, selectedMain, updateMainInline]);

  // 子菜单项行内快捷键录制（与主菜单 inline 录制同范式，但 target 来自 selectedSubs，
  // 按 subCapturing.id 定位）。同一时刻只允许一个槽录制（subCapturing 单值）。
  // 2026-07-20：右栏子菜单列表每行直接编辑快捷键。
  useEffect(() => {
    if (subCapturing === null) return;
    const target = selectedSubs.find((s) => s.id === subCapturing.id);
    if (!target) { setSubCapturing(null); return; }
    const handler = async (e: KeyboardEvent) => {
      e.preventDefault();
      e.stopPropagation();
      if (e.key === "Escape") { setSubCapturing(null); return; }
      if (subCapturing.kind === "global") {
        // 全局快捷键：修饰键单独按下不放行（等组合）；Backspace/Delete 清除
        if (e.key === "Alt" || e.key === "Shift" || e.key === "Control" || e.key === "Meta") return;
        if (e.key === "Backspace" || e.key === "Delete") {
          updateItemInline(target, { globalShortcut: "" });
          setSubCapturing(null);
          return;
        }
        const parts: string[] = [];
        if (e.metaKey || e.ctrlKey) parts.push("CmdOrCtrl");
        if (e.altKey) parts.push("Alt");
        if (e.shiftKey) parts.push("Shift");
        const keyName = e.code.startsWith("Key") ? e.code.slice(3) : e.code;
        parts.push(keyName);
        const sc = parts.join("+");
        try {
          await invoke("check_shortcut", { shortcut: sc });
          updateItemInline(target, { globalShortcut: sc });
        } catch (err) {
          console.warn("[action-bar] sub inline global shortcut check failed:", err);
        }
        setSubCapturing(null);
      } else {
        // 局部快捷键：单字符 0-9a-z；Backspace/Delete 清除
        if (e.key === "Backspace" || e.key === "Delete") {
          updateItemInline(target, { shortcut: "" });
          setSubCapturing(null);
          return;
        }
        const ch = e.key.toLowerCase();
        if (/^[0-9a-z]$/.test(ch)) {
          updateItemInline(target, { shortcut: ch });
          setSubCapturing(null);
        }
      }
    };
    document.addEventListener("keydown", handler, true);
    return () => document.removeEventListener("keydown", handler, true);
  }, [subCapturing, selectedSubs, updateItemInline]);

  // 标题 draft 的 debounce flush——300ms 无输入后落库。
  // 用 ref + setTimeout 避免闭包陈旧。selectedMain 切换时手动清 draft（下面 effect）。
  // 2026-07-17 review Critical #1 修复。
  useEffect(() => {
    if (titleDraft === null) return;
    const timer = setTimeout(() => {
      const draft = titleDraftRef.current;
      if (draft !== null) {
        updateMainInline({ title: draft });
        setTitleDraft(null);
      }
    }, 300);
    return () => clearTimeout(timer);
  }, [titleDraft, updateMainInline]);

  // 切换选中主菜单时清空 draft——防 draft 留在新选中菜单的标题里
  useEffect(() => {
    setTitleDraft(null);
  }, [effectiveSelectedId]);

  // editingId !== null 或 draftParentId !== undefined → EditForm 全屏覆盖（不分 tab）
  const isEditing = editingId !== null || draftParentId !== undefined;

  return (
    <div className={cn("w-full min-w-0", isEditing && "h-full flex flex-col")}>
      {/* ── 顶部 TAB：命令管理 / 执行记录 ──
          替代原 view 切换。EditForm 覆盖时不显示 TAB（全屏编辑）。 */}
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

      {/* ── 内容区 ── */}
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
        /* ── 左右分栏：左主菜单列表 + 右选中菜单详情/子菜单 ── */
        <div className="flex gap-4">
          {/* 左栏：主菜单列表 */}
          <div className="flex w-52 shrink-0 flex-col gap-2">
            {/* 场景过滤——分段控件。语义=该菜单项在什么场景下显示。 */}
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
                {/* ── 主菜单 inline 编辑表单 ── */}
                <div className="space-y-3 rounded-lg border border-border/50 bg-muted/15 p-4">
                  {/* 标题 + 保存按钮 + 启用 toggle 一行（toggle 居右） */}
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
                        // Enter 即时保存
                        if (e.key === "Enter" && titleDraft !== null) {
                          e.preventDefault();
                          updateMainInline({ title: titleDraft });
                          setTitleDraft(null);
                        }
                      }}
                    />
                    {/* 保存按钮——仅 draft 非空时显示可点（视觉提示有未保存改动） */}
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
                    {/* 启用 toggle 居右 */}
                    <div className="flex items-center gap-1.5">
                      <Toggle
                        checked={selectedMain.isEnabled}
                        onChange={(v) => updateMainInline({ isEnabled: v })}
                      />
                    </div>
                  </div>

                  {/* 类型不在此显示——左侧 MenuRow 小字行已有。
                      改类型走 EditForm（点行内编辑按钮）。 */}

                  {/* 叶子菜单（非 submenu）：快捷键 + 全局快捷键 一行 */}
                  {selectedMain.actionType !== "submenu" && (
                    <div className="grid grid-cols-2 gap-3">
                      <FormField label={t("settings.actionBar.shortcutLabel")}>
                        <div className="flex items-center gap-1">
                          <span className="text-xs text-muted-foreground/60 font-mono">⌥ +</span>
                          <button
                            onClick={() => setInlineCapturingShortcut((v) => !v)}
                            className={cn(
                              "w-10 text-center bg-background border rounded-md px-2 py-1.5 text-sm font-mono outline-none transition-all",
                              inlineCapturingShortcut
                                ? "border-voice ring-2 ring-voice/15"
                                : "border-border focus:border-voice/50 focus:ring-2 focus:ring-voice/15",
                            )}
                          >
                            {selectedMain.shortcut || "—"}
                          </button>
                          {selectedMain.shortcut && (
                            <button
                              onClick={() => updateMainInline({ shortcut: "" })}
                              className="rounded p-1 text-muted-foreground/60 transition-colors hover:bg-destructive/10 hover:text-destructive"
                            >
                              <X className="h-3.5 w-3.5" />
                            </button>
                          )}
                        </div>
                      </FormField>

                      <FormField label={ti18n("settings.actionBar.globalShortcutLabel")}>
                        <div className="flex items-center gap-2">
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
                      </FormField>
                    </div>
                  )}
                </div>

                {/* ── 子菜单列表（仅 submenu 类型展示） ── */}
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
                            // 快捷键槽：每行直接显示 + 内联录制（2026-07-20）
                            showShortcuts
                            capturingKind={subCapturing?.id === sub.id ? subCapturing.kind : null}
                            onCaptureShortcut={(kind) => setSubCapturing({ id: sub.id, kind })}
                            onClearShortcut={(kind) =>
                              updateItemInline(sub, kind === "local" ? { shortcut: "" } : { globalShortcut: "" })
                            }
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
