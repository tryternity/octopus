import { useState } from "react";
import { ChevronDown } from "lucide-react";
import { cn } from "@/lib/utils";

export function CollapsibleSection({
  icon: Icon,
  label,
  count,
  action,
  children,
  defaultOpen = true,
}: {
  icon: React.ElementType;
  label: string;
  count?: string;
  /** 渲染在 header 右侧的操作按钮（如「添加模型」），与 title 同行 */
  action?: React.ReactNode;
  children: React.ReactNode;
  defaultOpen?: boolean;
}) {
  const [open, setOpen] = useState(defaultOpen);

  return (
    <div className="pt-2 first:pt-0">
      <div className="flex items-center justify-between pb-0.5">
        <button
          className="flex items-center gap-1.5 group"
          onClick={() => setOpen(!open)}
        >
          <ChevronDown
            className={cn(
              "w-2.5 h-2.5 text-muted-foreground/40 transition-transform",
              !open && "-rotate-90",
            )}
          />
          <Icon className="w-3 h-3 text-muted-foreground/60" />
          <span className="text-[11px] font-medium text-muted-foreground">{label}</span>
          {count && <span className="text-[10px] text-muted-foreground/40">{count}</span>}
        </button>
        {action}
      </div>
      {open && <div className="mt-0.5">{children}</div>}
    </div>
  );
}
