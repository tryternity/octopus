import * as React from "react";
import { cva, type VariantProps } from "class-variance-authority";
import { cn } from "@/lib/utils";

/**
 * Input —— 设置窗口统一文本输入控件。
 *
 * 收敛原本散落在 GeneralPanel/HotwordPanel/ActionBarPanel/AgentPanel/CloudModelForm
 * 的 4 套 inputClass/selectClass（focus ring 不一致：ring-1/ring-2、voice/15/voice/20）。
 *
 * 基础样式对齐原 selectClass 范本（最通用）：
 * border + rounded-md + bg-background（输入框比页面背景深一档形成层级）
 * + focus:border-voice/50 + focus:ring-2 focus:ring-voice/15（统一 ring 规格）。
 *
 * variant：
 * - default：标准输入框
 * - mono：等宽字体（代码/快捷键/环境变量值）
 * - bare：无边框透明（PromptsPanel 编辑器内嵌输入，依赖外层容器边框）
 */
const inputVariants = cva(
  "outline-none transition-all placeholder:text-muted-foreground/50",
  {
    variants: {
      variant: {
        default:
          "border border-border rounded-md bg-background px-2.5 py-1.5 text-sm focus:border-voice/50 focus:ring-2 focus:ring-voice/15",
        mono:
          "border border-border rounded-md bg-background px-2.5 py-1.5 text-sm font-mono focus:border-voice/50 focus:ring-2 focus:ring-voice/15",
        bare: "bg-transparent text-sm",
      },
      size: {
        default: "min-w-[160px] max-w-[220px]",
        full: "w-full",
        sm: "min-w-0",
      },
    },
    defaultVariants: {
      variant: "default",
      size: "default",
    },
  },
);

export interface InputProps
  extends Omit<React.InputHTMLAttributes<HTMLInputElement>, "size">,
    VariantProps<typeof inputVariants> {}

const Input = React.forwardRef<HTMLInputElement, InputProps>(
  ({ className, variant, size, ...props }, ref) => (
    <input
      ref={ref}
      className={cn(inputVariants({ variant, size, className }))}
      autoCapitalize="off"
      autoCorrect="off"
      spellCheck={false}
      {...props}
    />
  ),
);
Input.displayName = "Input";

/** Select —— 与 Input 同样式（原 selectClass 范本）。 */
const Select = React.forwardRef<
  HTMLSelectElement,
  Omit<React.SelectHTMLAttributes<HTMLSelectElement>, "size"> &
    VariantProps<typeof inputVariants>
>(({ className, variant, size, ...props }, ref) => (
  <select
    ref={ref}
    className={cn(
      inputVariants({ variant, size }),
      "cursor-pointer disabled:opacity-60",
      className,
    )}
    {...props}
  />
));
Select.displayName = "Select";

/** Textarea —— 与 Input 同 focus 规格。 */
const Textarea = React.forwardRef<
  HTMLTextAreaElement,
  Omit<React.TextareaHTMLAttributes<HTMLTextAreaElement>, "size"> &
    VariantProps<typeof inputVariants>
>(({ className, variant, size, ...props }, ref) => (
  <textarea
    ref={ref}
    className={cn(inputVariants({ variant, size }), "leading-relaxed", className)}
    autoCapitalize="off"
    autoCorrect="off"
    spellCheck={false}
    {...props}
  />
));
Textarea.displayName = "Textarea";

export { Input, Select, Textarea, inputVariants };
