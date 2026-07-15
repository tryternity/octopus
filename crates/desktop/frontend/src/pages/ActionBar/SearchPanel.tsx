/**
 * SearchPanel — 搜索结果面板（Tab 栏 + 结果列表）。
 *
 * 由 ActionBar/index.tsx 在搜索模式下渲染。输入框在父组件中管理。
 * 展开方向由父组件传入，影响 DOM 顺序：
 *   - down: [TabBar] [Results]
 *   - up:   [Results] [TabBar]
 */
import { useRef, useEffect } from "react";
import { cn } from "@/lib/utils";
import { t } from "@/lib/i18n";
import { TABS, MAX_VISIBLE_RESULTS, type TabId, type ExpandDirection, type SearchResult } from "./searchTypes";

// 结果列表变化后忽略 hover 的时长（ms）——防止 DOM 重建时误触发
const HOVER_SUPPRESS_MS = 200;

// ── 来源图标 ──

const SOURCE_ICON: Record<string, string> = {
  app: "📦",
  file: "📄",
  shell: "▶",
  bookmark: "🔖",
  menu: "⚡",
  quicklink: "🔗",
};

const SOURCE_BADGE_COLOR: Record<string, string> = {
  app: "bg-blue-500/15 text-blue-500",
  file: "bg-amber-500/15 text-amber-600",
  shell: "bg-green-500/15 text-green-600",
  bookmark: "bg-purple-500/15 text-purple-500",
  menu: "bg-voice/15 text-voice",
  quicklink: "bg-cyan-500/15 text-cyan-600",
};

function SourceBadge({ source, icon }: { source: string; icon?: string }) {
  // 有自定义图标（如应用 icon）→ 渲染 img
  if (icon) {
    return (
      <img
        src={icon}
        alt=""
        className="inline-flex h-5 w-5 shrink-0 rounded object-contain"
      />
    );
  }
  const fallbackIcon = SOURCE_ICON[source] ?? "❓";
  const color = SOURCE_BADGE_COLOR[source] ?? "bg-muted text-muted-foreground";
  return (
    <span className={cn("inline-flex h-5 w-5 shrink-0 items-center justify-center rounded text-[11px]", color)}>
      {fallbackIcon}
    </span>
  );
}

// ── Tab 按钮 ──

function TabButton({
  tab,
  active,
  onClick,
}: {
  tab: (typeof TABS)[number];
  active: boolean;
  onClick: () => void;
}) {
  return (
    <button
      className={cn(
        "flex items-center gap-1 px-2 py-1 rounded-md transition-colors duration-100 shrink-0",
        active
          ? "bg-voice/12 text-voice"
          : "text-muted-foreground hover:bg-foreground/5 hover:text-foreground",
      )}
      onMouseDown={(e) => { e.stopPropagation(); e.preventDefault(); }}
      onClick={onClick}
    >
      <span className="text-[10px] font-medium leading-none whitespace-nowrap">
        {tab.label}
        <span className={cn(
          "ml-1 font-mono text-[9px]",
          active ? "text-voice/70" : "text-muted-foreground/50",
        )}>
          ⌥{tab.key.toUpperCase()}
        </span>
      </span>
    </button>
  );
}

// ── TabBar ──

function TabBar({
  activeTab,
  onTabChange,
}: {
  activeTab: TabId;
  onTabChange: (tab: TabId) => void;
}) {
  return (
    <div className="flex items-center gap-0.5 px-1.5 py-1 border-b border-border/20 shrink-0 h-[30px]">
      {TABS.map((tab) => (
        <TabButton
          key={tab.id}
          tab={tab}
          active={activeTab === tab.id}
          onClick={() => onTabChange(tab.id)}
        />
      ))}
    </div>
  );
}

// ── 结果行 ──

function ResultRow({
  result,
  selected,
  index,
  onClick,
  onHover,
  suppressRef,
}: {
  result: SearchResult;
  selected: boolean;
  index: number;
  onClick: () => void;
  onHover: () => void;
  suppressRef: React.RefObject<number>;
}) {
  const lastPos = useRef<{ x: number; y: number } | null>(null);
  return (
    <div
      role="option"
      aria-selected={selected}
      className={cn(
        "flex items-center gap-2 px-2.5 py-1.5 cursor-default transition-colors duration-75",
        selected ? "bg-voice/10" : "hover:bg-foreground/[0.03]",
      )}
      onMouseDown={(e) => { e.stopPropagation(); e.preventDefault(); }}
      onMouseMove={(e) => {
        if (Date.now() < suppressRef.current) return;
        if (lastPos.current && Math.abs(lastPos.current.x - e.clientX) < 1 && Math.abs(lastPos.current.y - e.clientY) < 1) return;
        lastPos.current = { x: e.clientX, y: e.clientY };
        onHover();
      }}
      onClick={(e) => { e.stopPropagation(); onClick(); }}
    >
      <SourceBadge source={result.source} icon={result.icon ?? undefined} />
      <div className="flex-1 min-w-0">
        <div className="flex items-baseline gap-2">
          <span className={cn(
            "text-[12px] font-medium truncate",
            selected ? "text-voice" : "text-foreground",
          )}>
            {result.title}
          </span>
        </div>
        {result.subtitle && (
          <span className="text-[10px] text-muted-foreground truncate block leading-tight">
            {result.subtitle}
          </span>
        )}
      </div>
      <span className="text-[9px] text-muted-foreground/60 font-mono shrink-0">
        {index + 1}
      </span>
    </div>
  );
}

// ── SearchPanel ──

export interface SearchPanelProps {
  results: SearchResult[];
  activeTab: TabId;
  selectedIdx: number;
  expandDirection: ExpandDirection;
  onTabChange: (tab: TabId) => void;
  onSelect: (idx: number) => void;
  onExecute: (result: SearchResult) => void;
}

export default function SearchPanel({
  results,
  activeTab,
  selectedIdx,
  expandDirection,
  onTabChange,
  onSelect,
  onExecute,
}: SearchPanelProps) {
  const resultsRef = useRef<HTMLDivElement>(null);
  const selectedRef = useRef<HTMLDivElement>(null);
  // 结果/Tab 变化或键盘选中变化后抑制 hover 一小段时间——
  // 防 DOM 重建误触发 mousemove，也防键盘 ↑↓ 选中后鼠标轻微移动覆盖选中（L1）
  const hoverSuppressRef = useRef(0);
  useEffect(() => {
    hoverSuppressRef.current = Date.now() + HOVER_SUPPRESS_MS;
  }, [results, activeTab, selectedIdx]);

  // 选中项滚动到可见区域
  useEffect(() => {
    selectedRef.current?.scrollIntoView({ block: "nearest" });
  }, [selectedIdx]);

  const tabBar = (
    <TabBar activeTab={activeTab} onTabChange={onTabChange} />
  );

  const resultList = (
    <div
      ref={resultsRef}
      className="flex-1 min-h-0 overflow-y-auto overflow-x-hidden"
      style={{ maxHeight: `${MAX_VISIBLE_RESULTS * 36}px` }}
    >
      {results.length === 0 ? (
        <div className="flex items-center justify-center py-4 text-[11px] text-muted-foreground/50">
          {t("actionbar.noResults")}
        </div>
      ) : (
        results.map((result, i) => (
          <div key={`${result.source}-${result.title}-${i}`} ref={i === selectedIdx ? selectedRef : undefined}>
            <ResultRow
              result={result}
              selected={i === selectedIdx}
              index={i}
              onClick={() => onExecute(result)}
              onHover={() => onSelect(i)}
              suppressRef={hoverSuppressRef}
            />
          </div>
        ))
      )}
    </div>
  );

  // 展开方向决定 DOM 顺序
  if (expandDirection === "up") {
    return (
      <div className="flex flex-col flex-1 min-h-0">
        {resultList}
        {tabBar}
      </div>
    );
  }

  return (
    <div className="flex flex-col flex-1 min-h-0">
      {tabBar}
      {resultList}
    </div>
  );
}
