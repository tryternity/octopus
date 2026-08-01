import * as React from "react";
import { Slot } from "@radix-ui/react-slot";
import { cva, type VariantProps } from "class-variance-authority";
import { cn } from "@/lib/utils";

/**
 * Button —— 设置窗口统一按钮组件。
 *
 * 变体设计基于 8 个面板现有按钮用法的归纳（三份样式审计报告）：
 * - primary（bg-foreground 反色）：PromptsPanel/Models 下载/保存等主操作
 * - voice（bg-voice 品牌色）：Hotword/Agent 等需要强调色的主操作
 * - outline（边框）：取消、次要操作
 * - ghost（透明 hover）：文字按钮、图标按钮
 * - destructive（bg-destructive 红底）：删除二次确认
 * - destructive-ghost（hover 变红）：危险图标按钮（Trash 等）
 * - success（bg-success 绿底）：激活/成功确认
 * - warning-soft（bg-warning 软底）：待激活提示（与 success 已激活对仗，黄/橙暖色区别于绿色）
 *
 * Raycast 签名交互：hover 用 opacity 过渡（0.85~0.9）而非背景色切换。
 * raycast 主题下可叠加 .raycast-btn-shadow 获得 macOS 按钮压感。
 */
const buttonVariants = cva(
  "inline-flex items-center justify-center gap-1.5 whitespace-nowrap font-medium outline-none transition-all duration-150 disabled:pointer-events-none disabled:opacity-40 [&_svg]:pointer-events-none [&_svg]:shrink-0",
  {
    variants: {
      variant: {
        primary: "bg-foreground text-background hover:opacity-85",
        voice: "bg-voice text-white hover:opacity-90",
        outline:
          "border border-border text-foreground hover:bg-accent hover:border-foreground/30",
        ghost:
          "text-muted-foreground hover:text-foreground hover:bg-accent",
        destructive:
          "bg-destructive text-destructive-foreground hover:opacity-90",
        "destructive-ghost":
          "text-muted-foreground hover:text-destructive hover:bg-destructive/10",
        success: "bg-success/15 text-success hover:bg-success/25",
        "warning-soft": "bg-warning/15 text-warning hover:bg-warning/25",
        "voice-soft": "bg-voice/10 text-voice hover:bg-voice/20",
      },
      size: {
        sm: "rounded-md px-2.5 py-1 text-xs [&_svg]:size-3",
        default: "rounded-md px-3 py-1.5 text-sm [&_svg]:size-3.5",
        lg: "rounded-md px-4 py-2 text-sm [&_svg]:size-4",
        icon: "rounded-md p-1.5 [&_svg]:size-4",
        "icon-sm": "rounded p-1 [&_svg]:size-3.5",
      },
    },
    defaultVariants: {
      variant: "ghost",
      size: "default",
    },
  },
);

export interface ButtonProps
  extends React.ButtonHTMLAttributes<HTMLButtonElement>,
    VariantProps<typeof buttonVariants> {
  asChild?: boolean;
}

const Button = React.forwardRef<HTMLButtonElement, ButtonProps>(
  ({ className, variant, size, asChild = false, ...props }, ref) => {
    const Comp = asChild ? Slot : "button";
    return (
      <Comp
        className={cn(buttonVariants({ variant, size, className }))}
        ref={ref}
        {...props}
      />
    );
  },
);
Button.displayName = "Button";

export { Button, buttonVariants };
