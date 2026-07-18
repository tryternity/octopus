import { useEffect } from "react";
import { X } from "lucide-react";
import { useT } from "@/lib/i18n";
import PasswordGenerator from "./PasswordGenerator";

/**
 * PasswordGeneratorModal —— 密码生成器 modal 外壳（外壳 A）。
 *
 * 半透明遮罩 + 居中卡片 + 标题 + 关闭按钮 + Esc 关闭，内部渲染
 * <PasswordGenerator> 主体（跨场景复用）。用于 CipherEditor 密码字段右侧的
 * 🔑 按钮：点 onUsePassword 写回 password 字段并关闭。
 *
 * 关闭交互：点遮罩 / 点 × / 按 Esc。
 *
 * 未来 Actionbar 场景不复用本组件——那会走独立 Tauri 窗口（外壳 B），
 * 直接在窗口 root 渲染 <PasswordGenerator> 主体（onAutotype 触发自动输入）。
 */
export interface PasswordGeneratorModalProps {
  open: boolean;
  onClose: () => void;
  /** 选中密码写回 cipher 编辑器 password 字段 */
  onUsePassword: (pwd: string) => void;
  showToast: (msg: string) => void;
}

export default function PasswordGeneratorModal({
  open,
  onClose,
  onUsePassword,
  showToast,
}: PasswordGeneratorModalProps) {
  const t = useT();

  // Esc 关闭——open 变化时挂/卸。
  useEffect(() => {
    if (!open) return;
    const handler = (e: KeyboardEvent) => {
      if (e.key === "Escape") onClose();
    };
    window.addEventListener("keydown", handler);
    return () => window.removeEventListener("keydown", handler);
  }, [open, onClose]);

  if (!open) return null;

  return (
    // 遮罩：点空白处关闭。z-50 确保盖在编辑器之上。
    <div
      className="fixed inset-0 z-50 flex items-center justify-center bg-black/40 p-4"
      onClick={onClose}
    >
      {/* 卡片：阻止 click 冒泡到遮罩，避免点卡片内部误关闭。 */}
      <div
        className="w-[480px] max-w-full rounded-lg border border-border bg-background p-4 shadow-xl"
        onClick={(e) => e.stopPropagation()}
      >
        <div className="mb-3 flex items-center justify-between">
          <h3 className="text-sm font-semibold">{t("settings.vault.generator.title")}</h3>
          <button
            onClick={onClose}
            className="rounded p-1 text-muted-foreground transition-colors hover:bg-accent hover:text-foreground"
            aria-label="close"
          >
            <X className="size-4" />
          </button>
        </div>
        <PasswordGenerator
          onUsePassword={(pwd) => {
            onUsePassword(pwd);
            onClose();
          }}
          showToast={showToast}
        />
      </div>
    </div>
  );
}
