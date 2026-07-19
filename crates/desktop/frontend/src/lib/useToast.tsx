import { useCallback, useEffect, useRef, useState } from "react";

/**
 * useToast —— 窗口级 toast 反馈 hook。
 *
 * 提取自 Settings/index.tsx 的局部实现，让独立窗口（PasswordGeneratorWindow /
 * VaultPicker 等）也能用统一的 toast 反馈，无需各自重复 useState + setTimeout。
 *
 * 用法：
 * ```tsx
 * const { toast, showToast } = useToast();
 * // 触发：showToast("已复制");
 * // 渲染（窗口底部居中）：
 * <Toast toast={toast} />
 * ```
 *
 * 默认 2 秒后自动消失，多次触发重置计时器（最后一次为准）。
 */
export function useToast(durationMs: number = 2000) {
  const [toast, setToast] = useState<string | null>(null);
  const timerRef = useRef<number | null>(null);

  const showToast = useCallback(
    (msg: string) => {
      setToast(msg);
      if (timerRef.current != null) {
        window.clearTimeout(timerRef.current);
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

  return { toast, showToast };
}

/**
 * Toast —— 通用渲染组件。
 *
 * 渲染在窗口底部居中，半透明黑底白字（适合透明浮窗）。
 * 配 useToast 使用：`<Toast toast={toast} />`。
 *
 * toast 为 null 时不渲染（return null）。
 */
export function Toast({ toast }: { toast: string | null }) {
  if (!toast) return null;
  return (
    <div className="pointer-events-none absolute bottom-4 left-1/2 -translate-x-1/2 rounded-md bg-foreground/90 px-3 py-1.5 text-xs font-medium text-background shadow-lg">
      {toast}
    </div>
  );
}
