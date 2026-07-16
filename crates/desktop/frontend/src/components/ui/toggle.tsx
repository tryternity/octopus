import { cn } from "@/lib/utils";

/**
 * Toggle —— 开关控件。
 *
 * 统一原本散落在 GeneralPanel/HotwordPanel/ActionBarPanel 的 3 套本地 Toggle
 * （尺寸不一：w-10 h-[22px] / w-10 h-[22px] / w-8 h-[18px]）为单一规格 w-10 h-[22px]。
 *
 * 开=bg-voice（品牌色），关=bg-muted-foreground/25。白圆点 translate-x 过渡。
 * 带 role="switch" + aria-checked（无障碍，HotwordPanel 原有做法）。
 */
export function Toggle({
  on,
  onClick,
  className,
  "aria-label": ariaLabel,
}: {
  on: boolean;
  onClick: () => void;
  className?: string;
  "aria-label"?: string;
}) {
  return (
    <button
      type="button"
      role="switch"
      aria-checked={on}
      aria-label={ariaLabel}
      className={cn(
        "relative flex h-[22px] w-10 flex-shrink-0 rounded-full transition-colors",
        on ? "bg-voice" : "bg-muted-foreground/25",
        className,
      )}
      onClick={onClick}
    >
      <span
        className={cn(
          "absolute left-0.5 top-0.5 h-[18px] w-[18px] rounded-full bg-white shadow-sm transition-transform",
          on && "translate-x-[18px]",
        )}
      />
    </button>
  );
}
