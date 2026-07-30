// 序号图标按钮——纯展示组件。从 index.tsx 拆出（2026-07-30）。

import { cn } from "@/lib/utils";
import { indexLabel } from "./label";

export default function IconBtn({ index, label, active, onClick, btnRef }: {
  index: number; label: string; active: boolean; onClick: () => void;
  btnRef?: (el: HTMLButtonElement | null) => void;
}) {
  return (
    <button
      ref={btnRef}
      className={cn(
        "flex items-center gap-1.5 px-2.5 py-[7px] rounded-[8px] transition-all duration-150 shrink-0",
        active
          ? "bg-voice/15 text-voice ring-1 ring-inset ring-voice/20"
          : "text-muted-foreground hover:bg-foreground/5 hover:text-foreground",
      )}
      onMouseDown={(e) => e.stopPropagation()}
      onClick={onClick}
      title={`${label} — Alt+${indexLabel(index)} 定位`}
    >
      <span
        className={cn(
          "inline-flex h-[20px] w-[20px] items-center justify-center rounded-[6px] font-mono text-[11px] font-semibold tabular-nums leading-none",
          active
            ? "bg-voice text-white"
            : "bg-muted/60 text-muted-foreground",
        )}
      >
        {indexLabel(index)}
      </span>
      <span className="text-[11px] font-medium leading-none whitespace-nowrap">{label}</span>
    </button>
  );
}
