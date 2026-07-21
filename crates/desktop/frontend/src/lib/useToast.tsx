import { useCallback, useEffect, useRef, useState } from "react";
import { X } from "lucide-react";

/**
 * useToast —— 窗口级 toast 反馈 hook。
 *
 * 提取自 Settings/index.tsx 的局部实现，让独立窗口（PasswordGeneratorWindow /
 * VaultPicker 等）也能用统一的 toast 反馈，无需各自重复 useState + setTimeout。
 *
 * 用法：
 * ```tsx
 * const { toast, showToast, dismissToast } = useToast();
 * // 成功（默认）：2s 自动消失
 * showToast("已复制");
 * // 错误：不自动消失，用户必须手动关闭
 * showToast("主密码错误", "error");
 * // 渲染（窗口底部居中）：
 * <Toast toast={toast} onClose={dismissToast} />
 * ```
 *
 * **行为**（2026-07-21 修订）：
 * - success（默认）：durationMs（默认 2000ms）后自动消失
 * - error：**不自动消失**——错误信息需要用户看清楚，由用户点 X 关闭
 *   （旧实现所有 toast 都 2s 自动消失，用户来不及看清错误）
 */
export type ToastVariant = "success" | "error";

export interface ToastState {
  msg: string;
  variant: ToastVariant;
}

export function useToast(durationMs: number = 2000) {
  const [toast, setToast] = useState<ToastState | null>(null);
  const timerRef = useRef<number | null>(null);

  const dismissToast = useCallback(() => {
    setToast(null);
    if (timerRef.current != null) {
      window.clearTimeout(timerRef.current);
      timerRef.current = null;
    }
  }, []);

  const showToast = useCallback(
    (msg: string, variant: ToastVariant = "success") => {
      setToast({ msg, variant });
      if (timerRef.current != null) {
        window.clearTimeout(timerRef.current);
      }
      // error 不自动消失——让用户自己关闭
      if (variant === "error") {
        timerRef.current = null;
        return;
      }
      timerRef.current = window.setTimeout(() => setToast(null), durationMs);
    },
    [durationMs],
  );

  // 卸载时清 timer 防 leak
  useEffect(() => {
    return () => {
      if (timerRef.current != null) {
        window.clearTimeout(timerRef.current);
      }
    };
  }, []);

  return { toast, showToast, dismissToast };
}

/**
 * Toast —— 通用渲染组件。
 *
 * 渲染在窗口底部居中。配 useToast 使用：
 * ```tsx
 * <Toast toast={toast} onClose={dismissToast} />
 * ```
 *
 * - success：半透明黑底白字（适合透明浮窗）
 * - error：红色描边 + 浅红背景 + 关闭按钮（用户手动关闭，不自动消失）
 *
 * toast 为 null 时不渲染（return null）。
 */
export function Toast({
  toast,
  onClose,
}: {
  toast: ToastState | null;
  onClose?: () => void;
}) {
  if (!toast) return null;
  const isError = toast.variant === "error";
  return (
    <div
      className={[
        "absolute bottom-4 left-1/2 -translate-x-1/2 flex items-center gap-2",
        "rounded-md px-3 py-2 text-xs font-medium shadow-lg",
        isError
          ? "border border-destructive/50 bg-destructive/10 text-destructive"
          : "bg-foreground/90 text-background",
      ].join(" ")}
      role={isError ? "alert" : "status"}
    >
      <span className="max-w-[80vw] break-words">{toast.msg}</span>
      {isError && onClose && (
        <button
          type="button"
          onClick={onClose}
          aria-label="关闭"
          className="shrink-0 rounded-sm p-0.5 hover:bg-destructive/20"
        >
          <X className="size-3" />
        </button>
      )}
    </div>
  );
}
