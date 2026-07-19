import { useCallback, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { X } from "lucide-react";
import { useT } from "@/lib/i18n";
import { Toast, useToast } from "@/lib/useToast";
import PasswordGenerator from "@/pages/Settings/Vault/PasswordGenerator";

/**
 * PasswordGeneratorWindow —— 独立浮窗 root（外壳 B：Actionbar 触发场景）。
 *
 * 与 CipherEditor Modal（外壳 A）渲染同一个 `<PasswordGenerator>` 主体（跨场景复用）。
 * 区别：
 * - 不传 `onUsePassword`（独立窗口没有"写回字段"语义）
 * - 传 `onAutotype`：点 Auto-type 按钮 → invoke `password_generator_autotype`
 *   → 后端 hide 浮窗 + autotype_login 注入前台浏览器
 *
 * 用户决策（2026-07-19）：点使用后自动 hide（与 VaultPicker 一致）。
 *
 * 视觉：透明 always_on_top 浮窗，本组件负责顶部标题栏（X 关闭）+ 主体容器 + 底部 toast。
 * 透明窗口的 html/body 背景不设——让 transparent:true 真正透明。
 * 主体放卡片里形成视觉边界（用户能看到浮窗边界）。
 *
 * Toast 系统（2026-07-19）：用 lib/useToast 提供"已复制"等反馈。PasswordGenerator
 * 主体的 `showToast` prop 接这里。
 */
export default function PasswordGeneratorWindow() {
  const t = useT();
  const { toast, showToast } = useToast();
  const [busy, setBusy] = useState(false);

  const handleClose = useCallback(() => {
    getCurrentWindow().hide();
  }, []);

  const handleAutotype = useCallback(
    async (pwd: string) => {
      setBusy(true);
      try {
        await invoke("password_generator_autotype", { password: pwd });
        // 点使用后自动 hide（用户决策，与 VaultPicker 一致）
        await getCurrentWindow().hide();
      } catch (e) {
        // autotype 失败已在后端降级（fallback 到剪贴板），不阻塞 hide
        console.error("password_generator_autotype failed:", e);
        await getCurrentWindow().hide();
      } finally {
        setBusy(false);
      }
    },
    [],
  );

  return (
    <div className="relative flex h-screen flex-col bg-background p-3 text-foreground">
      {/* 顶部标题栏 */}
      <div className="mb-2 flex shrink-0 items-center justify-between">
        <span className="text-sm font-medium">
          {t("settings.vault.generator.title")}
        </span>
        <button
          onClick={handleClose}
          className="rounded p-1 text-muted-foreground transition-colors hover:bg-accent hover:text-foreground"
          aria-label="close"
        >
          <X className="size-4" />
        </button>
      </div>

      {/* 主体 + 边框卡片（透明浮窗内的视觉边界） */}
      <div className="min-h-0 flex-1 overflow-y-auto rounded-md border border-border/50">
        <div className="p-3">
          {busy ? (
            <div className="flex h-32 items-center justify-center text-xs text-muted-foreground">
              {t("settings.loading")}
            </div>
          ) : (
            <PasswordGenerator
              onAutotype={handleAutotype}
              onCancel={handleClose}
              showToast={showToast}
            />
          )}
        </div>
      </div>

      {/* Toast 反馈（复制成功等） */}
      <Toast toast={toast} />
    </div>
  );
}
