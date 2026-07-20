import * as React from "react";
import { Eye, EyeOff, Eraser } from "lucide-react";
import { Input, type inputVariants } from "./input";
import { cva, type VariantProps } from "class-variance-authority";
import { cn } from "@/lib/utils";
import { useT } from "@/lib/i18n";

/**
 * PasswordInput —— 密码输入框 + 右侧 Eye（切换显示）/ Eraser（清空）按钮。
 *
 * **设计动机**（2026-07-20 e2e 反馈）：主密码很长，输错一个字符要全删重输很累。
 * Eye 让用户看到明文定位错字符；Eraser 一键清空重输。
 *
 * 用于：VaultPicker locked/reprompt、Settings UnlockDialog、Settings SetupWizard
 * （设密码 + 确认密码）。CipherEditor 的密码字段已有自己的 suffix 按钮（Eye +
 * 生成 + 复制），不复用此组件。
 *
 * **完全受控**：value/onChange 必填。`revealed` 是内部 state（per-instance 独立），
 * 不暴露——切换显示是 UI 交互，不应进业务 state。
 *
 * **可访问性**：
 * - Eye/Eraser 按钮 `type="button"`（不会触发外层 form submit）
 * - 按钮 title 提供操作描述（hover 显示 + 屏幕阅读器读）
 * - 按钮 tabIndex 默认参与 Tab 序列
 *
 * **样式**：Input 加 `pr-12`（48px 右内边距）让文字不被按钮遮住；按钮 absolute
 * 定位到右侧。Eraser 仅在 value 非空时显示——空输入框右侧的 × 看着别扭。
 */
export interface PasswordInputProps
  extends Omit<React.InputHTMLAttributes<HTMLInputElement>, "size" | "type">,
    VariantProps<typeof inputVariants> {
  /** 是否显示 Eraser 清空按钮——默认 true。某些场景（如确认密码框）可能不需要。 */
  showClear?: boolean;
  /** 清空回调——除了把 value 置空还可能要清相关错误状态。 */
  onClear?: () => void;
}

const PasswordInput = React.forwardRef<HTMLInputElement, PasswordInputProps>(
  ({ className, variant, size, value, onChange, showClear = true, onClear, disabled, ...props }, ref) => {
    const t = useT();
    const [revealed, setRevealed] = React.useState(false);

    const hasValue = typeof value === "string" ? value.length > 0 : Boolean(value);

    const handleClear = React.useCallback(() => {
      if (disabled) return;
      // 模拟用户清空：合成一个 empty value 的 change event 给 onChange
      const syntheticEvent = {
        target: { value: "" },
        currentTarget: { value: "" },
      } as React.ChangeEvent<HTMLInputElement>;
      onChange?.(syntheticEvent);
      onClear?.();
    }, [disabled, onChange, onClear]);

    return (
      <div className="relative">
        <Input
          ref={ref}
          type={revealed ? "text" : "password"}
          variant={variant}
          size={size}
          value={value}
          onChange={onChange}
          disabled={disabled}
          className={cn("pr-12", className)}
          {...props}
        />
        {/* 右侧按钮组：Eye + Eraser。
            按钮 px-1.5 py-1 + size-3.5，激活态用 text-foreground。 */}
        <div className="absolute right-1 top-1/2 flex -translate-y-1/2 items-center gap-0.5">
          <button
            type="button"
            onClick={() => setRevealed((v) => !v)}
            disabled={disabled}
            className={cn(
              "flex items-center rounded px-1.5 py-1 transition-colors hover:bg-accent",
              revealed ? "text-foreground" : "text-muted-foreground",
              disabled && "pointer-events-none opacity-40",
            )}
            title={
              revealed
                ? t("settings.vault.passwordInput.hide")
                : t("settings.vault.passwordInput.reveal")
            }
            aria-label={
              revealed
                ? t("settings.vault.passwordInput.hide")
                : t("settings.vault.passwordInput.reveal")
            }
          >
            {revealed ? <EyeOff className="size-3.5" /> : <Eye className="size-3.5" />}
          </button>
          {showClear && hasValue && (
            <button
              type="button"
              onClick={handleClear}
              disabled={disabled}
              className={cn(
                "flex items-center rounded px-1.5 py-1 text-muted-foreground transition-colors hover:bg-accent hover:text-foreground",
                disabled && "pointer-events-none opacity-40",
              )}
              title={t("settings.vault.passwordInput.clear")}
              aria-label={t("settings.vault.passwordInput.clear")}
            >
              <Eraser className="size-3.5" />
            </button>
          )}
        </div>
      </div>
    );
  },
);

PasswordInput.displayName = "PasswordInput";

export { PasswordInput };
