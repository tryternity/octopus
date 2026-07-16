import * as React from "react";
import { cva, type VariantProps } from "class-variance-authority";
import { cn } from "@/lib/utils";

/**
 * Badge —— 小型标签/徽章。
 *
 * 替代原本散落在各面板的内联 `<span className="text-[9px] px-1.5 py-0.5 rounded bg-muted ...">`。
 *
 * variant：
 * - muted：中性灰底（内置标记、类型标记、计数）
 * - voice：品牌色淡底（使用中、激活态）
 * - success：成功色淡底（当前模型）
 * - destructive：危险色淡底（失败、错误）
 * - outline：仅边框（轻量标记）
 */
const badgeVariants = cva(
  "inline-flex items-center gap-1 rounded font-medium whitespace-nowrap",
  {
    variants: {
      variant: {
        muted: "bg-muted text-muted-foreground",
        voice: "bg-voice/15 text-voice",
        success: "bg-success/15 text-success",
        destructive: "bg-destructive/15 text-destructive",
        outline: "border border-border text-muted-foreground",
      },
      size: {
        default: "px-1.5 py-0.5 text-[9px]",
        sm: "px-1 py-0.5 text-[10px]",
      },
    },
    defaultVariants: {
      variant: "muted",
      size: "default",
    },
  },
);

export interface BadgeProps
  extends React.HTMLAttributes<HTMLSpanElement>,
    VariantProps<typeof badgeVariants> {}

function Badge({ className, variant, size, ...props }: BadgeProps) {
  return (
    <span className={cn(badgeVariants({ variant, size }), className)} {...props} />
  );
}

export { Badge, badgeVariants };
