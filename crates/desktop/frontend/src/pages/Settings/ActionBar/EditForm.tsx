// 编辑表单（含 FormField + ExtensionDropZone 引用）。
// 从 ActionBarPanel.tsx 拆出（2026-07-30）。

import { useState, useEffect } from "react";
import { invoke } from "@tauri-apps/api/core";
import { ArrowLeft, X } from "lucide-react";
import { cn } from "@/lib/utils";
import { useT, t as ti18n } from "@/lib/i18n";
import ShortcutButton from "@/components/ShortcutButton";
import { Button } from "@/components/ui/button";
import AppPicker from "../ActionBar/AppPicker";
import PromptEditor from "../ActionBar/PromptEditor";
import type { ActionBarItem } from "./types";
import {
  TYPE_META, ACTION_TYPES, deriveAccepts, inputBase,
  Toggle,
} from "./constants";
import ExtensionDropZone from "./ExtensionDropZone";

export interface EditFormProps {
  form: Partial<ActionBarItem>;
  isSystem: boolean;
  onChange: (form: Partial<ActionBarItem>) => void;
  onSave: () => void;
  onCancel: () => void;
}

// 标题字符约束：CJK（中日韩）+ 字母数字 + -_（支持 slash Tab 补全无歧义）
// 范围与下方长度权重正则一致（含中文扩展/平假名片假名/韩文）
const TITLE_REGEX = /^[\u4e00-\u9fff\u3040-\u30ff\uac00-\ud7afa-zA-Z0-9_-]+$/;

// ── 表单字段行 ──
export const FormField = ({
  label, children, hint, className,
}: {
  label: string; children: React.ReactNode; hint?: string; className?: string;
}) => (
  <div className={cn("space-y-1.5", className)}>
    <label className="block text-[11px] font-medium uppercase tracking-wide text-muted-foreground/80">
      {label}
    </label>
    <div className="min-w-0 w-full">{children}</div>
    {hint && <p className="text-[11px] text-muted-foreground/60">{hint}</p>}
  </div>
);

export default function EditForm({
  form, isSystem, onChange, onSave, onCancel,
}: EditFormProps) {
  const t = useT();
  const type = form.actionType || "url";
  const meta = TYPE_META[type];
  const showContent = type !== "submenu" && type !== "extension" && type !== "copy_path";
  const isPromptType = type === "agent" || type === "ai";
  const [adapters, setAdapters] = useState<{key:string;displayName:string;isAvailable:boolean}[]>([]);
  const [capturingGlobal, setCapturingGlobal] = useState(false);
  useEffect(() => {
    invoke<{key:string;displayName:string;isAvailable:boolean}[]>("list_agent_adapters").then(setAdapters).catch(() => {});
  }, []);

  useEffect(() => {
    if (!capturingGlobal) return;
    const handler = async (e: KeyboardEvent) => {
      e.preventDefault();
      e.stopPropagation();
      if (e.key === "Escape") { setCapturingGlobal(false); return; }
      if (e.key === "Alt" || e.key === "Shift" || e.key === "Control" || e.key === "Meta") return;
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

      {/* 单卡片紧凑表单 */}
      <div className="flex min-h-0 flex-1 flex-col gap-3 overflow-y-auto rounded-lg border border-border/50 bg-muted/15 p-4">
        <FormField label={t("settings.actionBar.titleLabel")}>
          <div className="space-y-1">
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
            {form.title && !TITLE_REGEX.test(form.title) && (
              <p className="text-[11px] text-destructive">
                {t("settings.actionBar.titleInvalid")}
              </p>
            )}
          </div>
        </FormField>

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

        {type !== "submenu" && (
          <div className="flex items-start justify-between gap-4">
            {/* 斜杠命令名（左） */}
            <div className="space-y-1.5">
              <label className="block text-[11px] font-medium uppercase tracking-wide text-muted-foreground/80">
                {t("settings.actionBar.slashName")}
              </label>
              <input
                className="w-28 bg-background border border-border rounded-md px-3 py-1.5 text-sm font-mono outline-none transition-all focus:border-voice/50 focus:ring-2 focus:ring-voice/15"
                placeholder={t("settings.actionBar.slashNamePlaceholder")}
                value={form.triggerKeyword || ""}
                onChange={(e) => {
                  const val = e.target.value.trim().toLowerCase();
                  onChange({ ...form, triggerKeyword: val });
                }}
              />
              {form.triggerKeyword && !TITLE_REGEX.test(form.triggerKeyword) && (
                <p className="text-[11px] text-destructive">
                  {t("settings.actionBar.slashNameInvalid")}
                </p>
              )}
            </div>
            {/* 全局快捷键（右列，左对齐与上方字段对齐） */}
            <div className="flex flex-col items-start gap-1.5">
              <label className="block text-[11px] font-medium uppercase tracking-wide text-muted-foreground/80">
                {ti18n("settings.actionBar.globalShortcutLabel")}
              </label>
              <div className="flex items-center gap-1">
                <ShortcutButton
                  shortcut={form.globalShortcut ?? ""}
                  capturing={capturingGlobal}
                  onClick={() => setCapturingGlobal((v) => !v)}
                />
                <button
                  onClick={() => form.globalShortcut && onChange({ ...form, globalShortcut: "" })}
                  className={cn(
                    "rounded p-1 text-muted-foreground/60 transition-colors hover:bg-destructive/10 hover:text-destructive",
                    form.globalShortcut ? "" : "invisible pointer-events-none",
                  )}
                  aria-label={ti18n("settings.actionBar.clearShortcut")}
                >
                  <X className="h-3.5 w-3.5" />
                </button>
              </div>
            </div>
          </div>
        )}

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

        {type === "extension" && <ExtensionDropZone form={form} onChange={onChange} />}

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

        <AppPicker
          value={form.appBundleIds ?? ""}
          onChange={(v) => onChange({ ...form, appBundleIds: v })}
        />

        {showContent && isPromptType && (
          <PromptEditor
            key={form.id ?? "new"}
            value={form.actionData || ""}
            onChange={(v) => onChange({ ...form, actionData: v })}
            placeholder={meta.placeholderKey ? t(meta.placeholderKey) : ""}
          />
        )}
        {showContent && !isPromptType && (
          <FormField label={t("settings.actionBar.contentLabel")}>
            <textarea
              className="w-full min-h-[190px] resize-y bg-background border border-border rounded-md px-3 py-2 font-mono text-xs leading-relaxed outline-none transition-all focus:border-voice/50 focus:ring-2 focus:ring-voice/15"
              placeholder={meta.placeholderKey ? t(meta.placeholderKey) : ""}
              value={form.actionData || ""}
              autoCapitalize="off"
              autoCorrect="off"
              spellCheck={false}
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
      </div>

      {/* 底部操作栏 */}
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
}
