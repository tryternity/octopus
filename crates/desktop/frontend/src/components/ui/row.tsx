import * as React from "react";
import { cn } from "@/lib/utils";

/**
 * Row —— 设置项行（标签 + 提示 + 生效时机 + 控件）。
 *
 * 统一 GeneralPanel（完整版：label/hint/effect/children）和 HotwordPanel（简化版：仅 children）
 * 两处本地 Row 定义。
 *
 * 结构：左侧 label + 可选 hint（次级说明）+ 可选 effect badge（生效时机），
 * 右侧 children（Toggle/Select/Button 等控件）。border-b 分隔，last:border-0。
 */
export function Row({
  label,
  hint,
  effect,
  className,
  children,
}: {
  label?: string;
  hint?: string;
  effect?: string;
  className?: string;
  children?: React.ReactNode;
}) {
  return (
    <div
      className={cn(
        "flex items-center justify-between gap-3 border-b border-border/40 py-2.5 last:border-0",
        className,
      )}
    >
      {(label || hint) && (
        <div className="flex min-w-0 flex-1 flex-col gap-0.5">
          {label && (
            <span className="text-sm">
              {label}
              {effect && (
                <span className="ml-1.5 rounded bg-muted px-1 py-0.5 text-[10px] text-muted-foreground/60">
                  {effect}
                </span>
              )}
            </span>
          )}
          {hint && (
            <span className="text-xs text-muted-foreground/60">{hint}</span>
          )}
        </div>
      )}
      {children}
    </div>
  );
}
