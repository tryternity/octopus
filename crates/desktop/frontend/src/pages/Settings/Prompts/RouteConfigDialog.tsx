// 提示词路由配置对话框——绑定 app（bundle_id）+ 注入应用上下文开关。
// 应用感知润色：润色时按前台 app 自动选模板（app_bundle_ids 关联）+ inject_context=1
// 时把「当前应用：名称（类别）」注入 user prompt 头部。
//
// 见 spec docs/superpowers/specs/2026-08-01-app-aware-polish-design.md

import { useState, useEffect } from "react";
import { X } from "lucide-react";
import AppPicker from "../ActionBar/AppPicker";
import { Toggle } from "@/components/ui/toggle";

interface RouteConfigDialogProps {
  promptTitle: string;
  appBundleIds: string; // JSON 数组字符串，空串=全局
  injectContext: boolean;
  onCancel: () => void;
  onSave: (appBundleIds: string, injectContext: boolean) => void;
  t: (key: string, params?: Record<string, string | number>) => string;
}

/**
 * 路由配置弹窗：编辑单个 prompt 的 app 关联 + inject_context。
 * 对话框内是本地草稿态，确认（onSave）才写回 DB。
 */
export default function RouteConfigDialog({
  promptTitle,
  appBundleIds,
  injectContext,
  onCancel,
  onSave,
  t,
}: RouteConfigDialogProps) {
  const [draftApps, setDraftApps] = useState(appBundleIds);
  const [draftInject, setDraftInject] = useState(injectContext);
  const [saving, setSaving] = useState(false);

  // Esc 关闭
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") onCancel();
    };
    document.addEventListener("keydown", onKey);
    return () => document.removeEventListener("keydown", onKey);
  }, [onCancel]);

  const handleSave = () => {
    if (saving) return;
    setSaving(true);
    onSave(draftApps, draftInject);
  };

  return (
    <div
      className="fixed inset-0 z-50 flex items-center justify-center bg-black/40"
      onClick={onCancel}
    >
      <div
        className="w-[380px] max-w-[90vw] bg-surface border border-border rounded-lg shadow-lg overflow-hidden border-l-2 border-l-primary"
        onClick={(e) => e.stopPropagation()}
      >
        {/* 标题行 */}
        <div className="flex items-center gap-1.5 px-3 py-2 border-b border-border/60">
          <span className="text-xs font-semibold text-foreground flex-1 min-w-0 truncate">
            {t("settings.prompts.routeConfigTitle")}
          </span>
          <button
            onClick={onCancel}
            className="p-0.5 rounded text-muted-foreground hover:text-foreground hover:bg-accent transition-colors flex-shrink-0"
            title={t("settings.prompts.cancel")}
          >
            <X className="w-3.5 h-3.5" />
          </button>
        </div>

        {/* 关联 prompt 名称（让用户确认操作对象） */}
        <div className="px-3 py-2 border-b border-border/60">
          <p className="text-xs text-foreground truncate" title={promptTitle}>
            {promptTitle}
          </p>
        </div>

        {/* 表单 */}
        <div className="px-3 py-3 space-y-3">
          <AppPicker value={draftApps} onChange={setDraftApps} />

          <label className="flex items-start gap-2 cursor-pointer">
            <Toggle
              on={draftInject}
              onClick={() => setDraftInject((v) => !v)}
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
        </div>

        {/* 底部按钮 */}
        <div className="flex items-center justify-end gap-1.5 px-3 py-2 border-t border-border/60 bg-muted/40">
          <button
            onClick={onCancel}
            className="px-2.5 py-1 rounded text-[10px] font-medium text-muted-foreground hover:text-foreground hover:bg-accent transition-colors"
          >
            {t("settings.prompts.cancel")}
          </button>
          <button
            onClick={handleSave}
            disabled={saving}
            className="px-2.5 py-1 rounded text-[10px] font-medium bg-primary text-primary-foreground hover:bg-primary/90 transition-colors disabled:opacity-50"
          >
            {t("settings.prompts.save")}
          </button>
        </div>
      </div>
    </div>
  );
}
