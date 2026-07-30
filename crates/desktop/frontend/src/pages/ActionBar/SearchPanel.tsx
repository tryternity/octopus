/**
 * SearchPanel — 搜索结果面板（Tab 栏 + 结果列表）。
 *
 * 由 ActionBar/index.tsx 在搜索模式下渲染。输入框在父组件中管理。
 * 展开方向由父组件传入，影响 DOM 顺序：
 *   - down: [TabBar] [Results]
 *   - up:   [Results] [TabBar]
 */
import { useRef, useEffect } from "react";
import { Package, FileText, Terminal, Bookmark, Zap, Link as LinkIcon, HelpCircle } from "lucide-react";
import { cn } from "@/lib/utils";
import { t } from "@/lib/i18n";
import { TABS, MAX_VISIBLE_RESULTS, RESULT_ROW_HEIGHT, type TabId, type ExpandDirection, type SearchResult } from "./searchTypes";
import { getVisibleTabs } from "./searchLogic";

// 结果列表变化后忽略 hover 的时长（ms）——防止 DOM 重建时误触发
const HOVER_SUPPRESS_MS = 200;

// ── 来源图标（Lucide SVG，替代 emoji——跨平台一致 + 可主题着色）──

const SOURCE_ICON: Record<string, React.ComponentType<{ className?: string }>> = {
  app: Package,
  file: FileText,
  shell: Terminal,
  bookmark: Bookmark,
  menu: Zap,
  quicklink: LinkIcon,
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
        className="inline-flex h-[22px] w-[22px] shrink-0 rounded-[6px] object-contain"
      />
    );
  }
  const Icon = SOURCE_ICON[source] ?? HelpCircle;
  const color = SOURCE_BADGE_COLOR[source] ?? "bg-muted text-muted-foreground";
  return (
    <span className={cn("inline-flex h-[22px] w-[22px] shrink-0 items-center justify-center rounded-[6px]", color)}>
      <Icon className="w-3.5 h-3.5" />
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
        "flex items-center gap-1 px-2.5 py-1 rounded-[6px] transition-colors duration-100 shrink-0",
        active
          ? "bg-voice/15 text-voice"
          : "text-muted-foreground hover:bg-foreground/5 hover:text-foreground",
      )}
      onMouseDown={(e) => { e.stopPropagation(); e.preventDefault(); }}
      onClick={onClick}
    >
      <span className="text-[11px] font-medium leading-none whitespace-nowrap">
        {tab.label}
        <span className={cn(
          "ml-1 font-mono text-[9px]",
          active ? "text-voice/60" : "text-muted-foreground/50",
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
  hasContext,
  isSlashMode,
  onTabChange,
}: {
  activeTab: TabId;
  hasContext: boolean;
  isSlashMode: boolean;
  onTabChange: (tab: TabId) => void;
}) {
  const visibleTabs = getVisibleTabs(hasContext, isSlashMode);
  return (
    <div className="flex items-center gap-0.5 px-1.5 py-1 border-b border-border/30 shrink-0 h-[30px]">
      {visibleTabs.map((tab) => (
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
        "relative flex items-center gap-2.5 px-3 py-2 cursor-default transition-colors duration-75",
        selected ? "bg-voice/12" : "hover:bg-foreground/[0.04]",
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
      {selected && (
        <span className="absolute left-0 top-1/2 -translate-y-1/2 h-5 w-[3px] rounded-r-full bg-voice" aria-hidden />
      )}
      <SourceBadge source={result.source} icon={result.icon ?? undefined} />
      <div className="flex-1 min-w-0">
        <div className="flex items-baseline gap-2">
          <span className={cn(
            "text-[13px] font-medium truncate",
            selected ? "text-voice" : "text-foreground",
          )}>
            {result.title}
          </span>
        </div>
        {result.subtitle && (
          <span className="text-[11px] text-muted-foreground truncate block leading-tight">
            {result.subtitle}
          </span>
        )}
      </div>
      <span className="text-[10px] text-muted-foreground/60 font-mono shrink-0 tabular-nums">
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
  hasContext: boolean;
  /** query 是否 / 开头（slash 命令模式）——true 时只显示 slash tab */
  isSlashMode: boolean;
  onTabChange: (tab: TabId) => void;
  onSelect: (idx: number) => void;
  onExecute: (result: SearchResult) => void;
}

export default function SearchPanel({
  results,
  activeTab,
  selectedIdx,
  expandDirection,
  hasContext,
  isSlashMode,
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

  // 选中项滚动到可见区域——手动调 scrollTop，不调 scrollIntoView。
  // scrollIntoView 在 overflow-hidden 的浮窗内，结果少时容器无滚动条会向外找祖先链
  // 滚动整个 webview，导致列表盖住搜索框。手动 scrollTop 只动容器自身，无副作用。
  // 用 getBoundingClientRect 比较（不依赖 offsetParent 定位）。
  useEffect(() => {
    const container = resultsRef.current;
    const selected = selectedRef.current;
    if (!container || !selected) return;
    const cRect = container.getBoundingClientRect();
    const sRect = selected.getBoundingClientRect();
    // 选中项在容器可视区上方 → 上滚
    if (sRect.top < cRect.top) {
      container.scrollTop -= cRect.top - sRect.top;
    // 选中项在容器可视区下方 → 下滚
    } else if (sRect.bottom > cRect.bottom) {
      container.scrollTop += sRect.bottom - cRect.bottom;
    }
  }, [selectedIdx]);

  const tabBar = (
    <TabBar activeTab={activeTab} hasContext={hasContext} isSlashMode={isSlashMode} onTabChange={onTabChange} />
  );

  const resultList = (
    <div
      ref={resultsRef}
      className="overflow-y-auto overflow-x-hidden"
      style={{ maxHeight: `${MAX_VISIBLE_RESULTS * RESULT_ROW_HEIGHT}px` }}
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
