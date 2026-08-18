// ActionBar 常量 + 工具函数。从 ActionBarPanel.tsx 拆出（2026-07-30）。

import { cn } from "@/lib/utils";
import { Toggle as UIToggle } from "@/components/ui/toggle";

// ── 类型元信息 ──
// bar: 左侧 3px 色条颜色（列表行签名元素）；dot: 小圆点（表单内引用）
export const TYPE_META: Record<
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
  markdown:   { bar: "bg-teal-500",       dot: "bg-teal-500",    label: "MD",      descKey: "settings.actionBar.typeMarkdownDesc",   placeholderKey: "" },
};

export const ACTION_TYPES = [
  { value: "submenu",    labelKey: "settings.actionBar.typeSubmenu" },
  { value: "ai",         labelKey: "settings.actionBar.typeAi" },
  { value: "url",        labelKey: "settings.actionBar.typeUrl" },
  { value: "script",     labelKey: "settings.actionBar.typeScript" },
  { value: "extension",  labelKey: "settings.actionBar.typeExtension" },
  { value: "agent",      labelKey: "settings.actionBar.typeAgent" },
  { value: "copy_path",  labelKey: "settings.actionBar.typeCopyPath" },
  { value: "markdown",   labelKey: "settings.actionBar.typeMarkdown" },
];

export function deriveAccepts(actionType: string | undefined, explicit?: string): string {
  if (explicit) return explicit;
  if (actionType === "copy_path") return "file";
  if (actionType === "agent") return "any";
  if (actionType === "submenu") return "any";
  if (actionType === "markdown") return "any";
  return "text";
}

export const pad2 = (n: number) => String(n).padStart(2, "0");

// ── 统一控件样式 ──
export const inputBase = "w-full bg-background border border-border rounded-md px-3 py-2 text-sm outline-none transition-all focus:border-voice/50 focus:ring-2 focus:ring-voice/15";

// ── 类型标签 ──
export const TypeTag = ({ type, variant = "dot" }: { type: string; variant?: "dot" | "solid" }) => {
  const meta = TYPE_META[type] ?? { bar: "bg-stone-400", dot: "bg-stone-400", label: type.toUpperCase().slice(0, 8) || "UNKNOWN", descKey: "", placeholderKey: "" };
  return (
    <span className="inline-flex items-center gap-1 font-mono text-[10px] uppercase tracking-wider text-muted-foreground">
      {variant === "dot" && <span className={cn("h-1.5 w-1.5 rounded-full", meta.dot)} />}
      {meta.label}
    </span>
  );
};

// Toggle 本地包装：适配 ActionBar 的 (checked, onChange(v)) 签名到共享 UIToggle (on, onClick)。
export const Toggle = ({ checked, onChange }: { checked: boolean; onChange: (v: boolean) => void }) => (
  <UIToggle on={checked} onClick={() => onChange(!checked)} />
);
