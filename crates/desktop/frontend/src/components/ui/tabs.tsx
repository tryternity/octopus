import { cn } from "@/lib/utils";

/**
 * Tabs —— 设置面板内 Tab 切换组件。
 *
 * 统一原本散落的 3 套 Tab 实现：
 * - ModelsPanel/AgentPanel 用反色填充 Pill Tab（bg-foreground text-background）
 * - GeneralPanel 用下划线 Tab（text-voice + 底部 voice 竖条）
 * - ActionBarPanel 用 voice 淡填充分段控件（bg-voice/12 text-voice）
 *
 * 拆成三个组件，语义清晰，视觉收敛。
 */

type TabItem = { key: string; label: string };

/** Pill Tabs —— 药丸式反色填充（ModelsPanel/AgentPanel 子页签）。 */
export function PillTabs({
  items,
  active,
  onChange,
  className,
}: {
  items: TabItem[];
  active: string;
  onChange: (key: string) => void;
  className?: string;
}) {
  return (
    <div
      className={cn(
        "flex gap-1 border-b border-border px-2 pt-1 pb-2",
        className,
      )}
    >
      {items.map((tab) => (
        <button
          key={tab.key}
          className={cn(
            "rounded-md px-2.5 py-1 text-[11px] font-medium transition-all duration-150",
            active === tab.key
              ? "bg-foreground text-background"
              : "text-muted-foreground hover:bg-accent hover:text-foreground",
          )}
          onClick={() => onChange(tab.key)}
        >
          {tab.label}
        </button>
      ))}
    </div>
  );
}

/** Underline Tabs —— 下划线式（GeneralPanel 三个子 tab）。 */
export function UnderlineTabs({
  items,
  active,
  onChange,
  className,
}: {
  items: TabItem[];
  active: string;
  onChange: (key: string) => void;
  className?: string;
}) {
  return (
    <div className={cn("flex items-center border-b border-border", className)}>
      {items.map((tab) => (
        <button
          key={tab.key}
          className={cn(
            "relative px-4 py-2 text-sm font-medium transition-colors",
            active === tab.key
              ? "text-voice"
              : "text-muted-foreground hover:text-foreground",
          )}
          onClick={() => onChange(tab.key)}
        >
          {tab.label}
          {active === tab.key && (
            <span className="absolute bottom-[-1px] left-0 right-0 h-[2px] rounded-full bg-voice" />
          )}
        </button>
      ))}
    </div>
  );
}

/** Segmented —— 分段控件（ActionBarPanel 场景过滤等单选组）。 */
export function Segmented({
  items,
  active,
  onChange,
  className,
}: {
  items: TabItem[];
  active: string;
  onChange: (key: string) => void;
  className?: string;
}) {
  return (
    <div
      className={cn(
        "flex items-center overflow-hidden rounded-md border border-border",
        className,
      )}
    >
      {items.map((tab) => (
        <button
          key={tab.key}
          className={cn(
            "px-2.5 py-1.5 text-xs transition-colors",
            active === tab.key
              ? "bg-voice/12 font-medium text-voice"
              : "text-muted-foreground hover:bg-muted/60 hover:text-foreground",
          )}
          onClick={() => onChange(tab.key)}
        >
          {tab.label}
        </button>
      ))}
    </div>
  );
}
