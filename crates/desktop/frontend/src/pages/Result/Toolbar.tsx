// Result 窗口的工具栏（顶部 8 个按钮）。
//
// 2026-07-21 perf：从 Result/index.tsx 内联 tools 数组抽取为独立 memo 组件。
// 原实现每次 update-result 流式事件都重建 tools 数组（8 个对象 + 8 个 inline
// 箭头函数）+ 重渲染整树。memo 后仅当 props 真变化才重渲染。
import { memo } from "react";
import { cn } from "@/lib/utils";
import { SvgIcon, type IconName } from "@/components/SvgIcon";

export interface ToolDef {
  id: string;
  icon: IconName;
  label: string;
  active?: boolean;
  disabled?: boolean;
  onClick: () => void;
}

interface ToolbarProps {
  tools: ToolDef[];
  /** 0..1，控制可见性（hide_toolbar 配置 + toolbarVisible hover 状态共同决定） */
  opacityClass: string;
  onDragStart: (e: React.MouseEvent) => void;
}

function ToolbarImpl({ tools, opacityClass, onDragStart }: ToolbarProps) {
  return (
    <div
      className={cn(
        "flex items-center gap-[2px] px-1.5 pt-0.5 transition-opacity duration-150 cursor-grab active:cursor-grabbing",
        opacityClass,
      )}
      onMouseDown={onDragStart}
    >
      {tools.map(({ id, icon, label, active, disabled, onClick }) => (
        <button
          key={id}
          className={cn(
            "tool-btn w-[20px] h-[20px] flex items-center justify-center rounded-[4px] transition-colors cursor-default",
            "hover:text-[#007aff] hover:bg-black/[0.05]",
            active && "text-[#007aff]!",
            disabled && "cursor-default hover:bg-transparent",
          )}
          style={{ color: active ? "#007aff" : "var(--color-tool-icon)", opacity: disabled ? 0.35 : 1 }}
          title={label}
          aria-label={label}
          disabled={disabled}
          onMouseDown={(e) => e.stopPropagation()}
          onClick={onClick}
        >
          <SvgIcon name={icon} size={16} />
        </button>
      ))}
    </div>
  );
}

export const Toolbar = memo(ToolbarImpl);
