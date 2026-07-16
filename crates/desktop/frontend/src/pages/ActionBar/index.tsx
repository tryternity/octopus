import { useState, useEffect, useRef, useLayoutEffect, useMemo } from "react";
import { invoke } from "@/lib/tauri";
import { listen as rawListen } from "@tauri-apps/api/event";
import { getCurrentWindow, LogicalSize, LogicalPosition } from "@tauri-apps/api/window";
import { cn } from "@/lib/utils";
import { Loader2, ChevronLeft, ChevronRight, Search } from "lucide-react";
import { detectActionUrl } from "./urlDetect";
import { t } from "@/lib/i18n";
import SearchPanel from "./SearchPanel";
import {
  INPUT_HEIGHT,
  TAB_BAR_HEIGHT,
  WINDOW_WIDTH,
  DELAYED_SEARCH_DEBOUNCE_MS,
  type TabId,
  type View,
  type ExpandDirection,
  type SearchResult as SearchHit,
} from "./searchTypes";
// 只有走 mdfind（file/bookmark Provider，慢）的 Tab 才需防抖；其他 Tab（含 all）
// 走内存 Provider（app/menu/calculator/url），亚毫秒，无需防抖。
// all tab 虽也会跑 mdfind，但后端流式扇出——快 Provider 结果先 emit，mdfind 慢的
// 后追加，首屏由 app/menu 等即时结果提供，故 all tab 不防抖也不阻塞首屏（spec §9 < 30ms）。
const DEBOUNCED_TABS = new Set<TabId | "files_bookmarks" | "quick">([
  "files",
  "bookmarks",
  "files_bookmarks",
]);
function getDebounceMs(tab: TabId): number {
  return (DEBOUNCED_TABS as Set<string>).has(tab) ? DELAYED_SEARCH_DEBOUNCE_MS : 0;
}
import { executeSearchStream, cleanupSearchStream } from "./searchStream";
import {
  determineExpandDirection,
  getTabByKey,
  getNextTab,
  filterByTab,
  parseActionData,
  calcResultsHeight,
  navigateResults,
  hasQuery,
  nextFocusLayerAfterExecute,
  calcMenuHeight,
  moveDirection,
} from "./searchLogic";

type ContextKind = "text" | "files";

type AppKind = 'editor' | 'terminal' | 'browser' | 'chat' | 'unknown';

interface AppSource {
  bundleId?: string;
  name: string;
  kind: AppKind;
}

interface SurroundingText {
  before?: string;
  after?: string;
  windowTitle?: string;
}

interface Context {
  kind: ContextKind;
  text: string;
  files: string[];
  source?: AppSource;
  surrounding?: SurroundingText;
}

interface ActionBarItem {
  id: number;
  parentId: number | null;
  title: string;
  icon: string;
  actionType: string;
  actionData: string;
  sortOrder: number;
  isSystem: boolean;
  isEnabled: boolean;
  shortcut?: string;
  agent?: string;
  accepts?: string;
}

const AI_TIMEOUT_MS = 10000;

/** 菜单模式左右键行为开关。
 *  - true（默认）：←/→ 在菜单项间移动（等同 Tab/Shift+Tab）， ActionBar 场景下输入框
 *    无需手动移光标（内容短），左右键挪给菜单导航更实用。
 *  - false：←/→ 放行给输入框移光标（原行为）。
 *  搜索模式不受影响（←/→ 始终移光标，搜索模式无"菜单项间移动"概念）。 */
const ARROW_AS_TAB = true;

import { indexLabel, labelToIndex } from "./label";

/** KeyboardEvent.code → 单字符（0-9 a-z）。非字母数字返回 null。
 *  macOS 上 Alt 会改变 e.key 输出（如 Alt+H → "˙"），用 e.code 取物理键。 */
function codeToChar(code: string): string | null {
  if (code.startsWith("Key") && code.length === 4) return code[3].toLowerCase();
  if (code.startsWith("Digit") && code.length === 6) return code[5].toLowerCase();
  return null;
}

const IconBtn = ({ index, label, active, onClick, btnRef, shortcut }: {
  index: number; label: string; active: boolean; onClick: () => void;
  btnRef?: (el: HTMLButtonElement | null) => void;
  shortcut?: string;
}) => (
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
    title={`${label} — Alt+${indexLabel(index)} 定位${shortcut ? ` · ⌘${shortcut} 执行` : ""}`}
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
    {shortcut && (
      <span className="text-[9px] text-voice/60 font-mono leading-none">⌘{shortcut}</span>
    )}
  </button>
);

/** 带左右溢出指示器的横向滚动容器 */
const ScrollRow = ({ children, className }: {
  children: React.ReactNode; className?: string;
}) => {
  const ref = useRef<HTMLDivElement>(null);
  const [overflow, setOverflow] = useState({ left: false, right: false });

  useLayoutEffect(() => {
    const el = ref.current;
    if (!el) return;
    const check = () => {
      setOverflow({
        left: el.scrollLeft > 4,
        right: el.scrollLeft + el.clientWidth < el.scrollWidth - 4,
      });
    };
    check();
    el.addEventListener("scroll", check, { passive: true });
    const ro = new ResizeObserver(check);
    ro.observe(el);
    return () => { el.removeEventListener("scroll", check); ro.disconnect(); };
  }, []);

  return (
    <div className={cn("relative", className)}>
      <div
        ref={ref}
        className="flex items-center gap-1 px-1.5 py-[3px] shrink-0 overflow-x-auto scrollbar-none"
      >
        {children}
      </div>
      {overflow.left && (
        <div className="absolute left-0 top-0 bottom-0 flex items-center pl-0.5 pointer-events-none bg-gradient-to-r from-background/95 to-transparent">
          <ChevronLeft className="w-3 h-3 text-voice" />
        </div>
      )}
      {overflow.right && (
        <div className="absolute right-0 top-0 bottom-0 flex items-center pr-0.5 pointer-events-none bg-gradient-to-l from-background/95 to-transparent">
          <ChevronRight className="w-3 h-3 text-voice" />
        </div>
      )}
    </div>
  );
};

export default function ActionBar() {
  const [context, setContext] = useState<Context | null>(null);
  const [view, setView] = useState<View>("main");
  const [selectedIdx, setSelectedIdx] = useState(0);
  const [subSelectedIdx, setSubSelectedIdx] = useState(0);
  const [menuItems, setMenuItems] = useState<ActionBarItem[]>([]);
  const [toast, setToast] = useState("");
  const mainBtnRefs = useRef<(HTMLButtonElement | null)[]>([]);
  const subBtnRefs = useRef<(HTMLButtonElement | null)[]>([]);
  const toastTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  const searchEngineRef = useRef("google");
  const timedOutRef = useRef(false);
  const contextRef = useRef<Context | null>(null);
  const viewRef = useRef<View>("main");
  const submenuParentIdRef = useRef<number | null>(null);
  const [focusLayer, setFocusLayer] = useState<"main" | "sub">("main");
  const focusLayerRef = useRef<"main" | "sub">("main");

  // ── 搜索状态 ──
  const [query, setQuery] = useState("");
  const [activeTab, setActiveTab] = useState<TabId>("all");
  const [instantResults, setInstantResults] = useState<SearchHit[]>([]);
  const [searchSelectedIdx, setSearchSelectedIdx] = useState(0);
  const [expandDirection, setExpandDirection] = useState<ExpandDirection>("down");
  const inputRef = useRef<HTMLInputElement>(null);
  const baseWinPosRef = useRef<{ x: number; y: number } | null>(null);
  const lastImeKeyTime = useRef(0);
  const showTimeRef = useRef(0);
  // 浮窗根容器 ref——resize effect 实测 scrollHeight 精确修正窗口高度（不再依赖估算常量）
  const actionBarRef = useRef<HTMLDivElement>(null);

  useEffect(() => { viewRef.current = view; }, [view]);

  // 高亮项变化时自动滚动到可见区域
  useEffect(() => {
    mainBtnRefs.current[selectedIdx]?.scrollIntoView({ behavior: "smooth", block: "nearest", inline: "nearest" });
  }, [selectedIdx]);
  useEffect(() => {
    subBtnRefs.current[subSelectedIdx]?.scrollIntoView({ behavior: "smooth", block: "nearest", inline: "nearest" });
  }, [subSelectedIdx]);

  const showQuickError = (msg: string) => {
    if (toastTimerRef.current) clearTimeout(toastTimerRef.current);
    setToast(msg);
    toastTimerRef.current = setTimeout(() => setToast(""), 2000);
  };
  useEffect(() => { focusLayerRef.current = focusLayer; }, [focusLayer]);
  useEffect(() => { contextRef.current = context; }, [context]);

  // task-input 视图已移除（agent 含 {{task}} 改为联动语音）

  // ── 搜索：结果（流式后端 emit 累积 top-N，前端整体替换；无 delayed 合并）──
  const allResults = instantResults;

  // 按 context.accepts 过滤菜单/quicklink 搜索结果
  // （Files 场景下 text-only 项如翻译不应出现，反之亦然）
  // 无选中（context=null）时仅显示 accepts="any" 的 menu/quicklink 项
  const contextFilteredResults = useMemo(() => {
    return allResults.filter((r) => {
      if (r.source !== "menu" && r.source !== "quicklink") return true;
      const data = parseActionData(r.actionData);
      const item = menuItems.find((i) => i.id === (data.id as number));
      if (!item) return true;
      const accepts = item.accepts || "text";
      if (accepts === "any") return true;
      if (!context) return false; // 无选中：非 any 的 menu 项不显示
      const isFiles = context.kind === "files";
      return isFiles ? accepts === "file" : accepts === "text";
    });
  }, [allResults, context, menuItems]);

  const filteredResults = useMemo(() => filterByTab(contextFilteredResults, activeTab), [contextFilteredResults, activeTab]);

  // 动态调整窗口高度 + 位置（展开方向）
  // 用 generation token 防"快速 Tab 切换"时异步乱序——只让最后一次 resize 生效
  const resizeGenRef = useRef(0);
  useEffect(() => {
    const win = getCurrentWindow();
    const inSearch = hasQuery(query);

    // 计算目标高度 + 位置
    let totalHeight: number;
    let targetX: number | null = null;
    let targetY: number | null = null;

    if (inSearch) {
      const resultsHeight = calcResultsHeight(filteredResults.length);
      totalHeight = INPUT_HEIGHT + TAB_BAR_HEIGHT + resultsHeight;
      if (baseWinPosRef.current) {
        targetX = baseWinPosRef.current.x;
        if (expandDirection === "up") {
          targetY = baseWinPosRef.current.y - (TAB_BAR_HEIGHT + resultsHeight);
        } else {
          targetY = baseWinPosRef.current.y;
        }
      }
    } else {
      // 菜单条高度——抽纯函数 calcMenuHeight（防护 context 依赖遗漏导致窗口裁剪菜单条）
      const menuHeight = calcMenuHeight(!!context, view);
      totalHeight = INPUT_HEIGHT + menuHeight;
      if (baseWinPosRef.current) {
        targetX = baseWinPosRef.current.x;
        targetY = baseWinPosRef.current.y;
      }
    }

    // 序列化：每次只执行最新一代 resize
    const gen = ++resizeGenRef.current;
    const apply = async () => {
      if (gen !== resizeGenRef.current) return; // 已被更新一代取代
      // 阶段 1：用估算常量初步 setSize，让 DOM 按新窗口尺寸重排
      await win.setSize(new LogicalSize(WINDOW_WIDTH, totalHeight));
      if (gen !== resizeGenRef.current) return;
      if (targetX !== null && targetY !== null) {
        await win.setPosition(new LogicalPosition(targetX, targetY));
      }

      // 阶段 2：实测根容器实际高度，精确修正窗口。
      // setSize resolve 后 webview 已按新尺寸重排，scrollHeight 是准确的。
      // 不再依赖估算常量——跨平台/DPR/分辨率/字体变化自适应，无闪烁（paint 前完成修正）。
      // +2px 圆角预留：rounded-[10px] 底部弧线会裁剪最后一点内容，scrollHeight 不含此视觉空间。
      const el = actionBarRef.current;
      if (el) {
        const actual = Math.ceil(el.scrollHeight) + 2;
        if (Math.abs(actual - totalHeight) > 1) {
          await win.setSize(new LogicalSize(WINDOW_WIDTH, actual));
        }
      }

      // setSize/setPosition 在 macOS 调整 NSWindow frame 会触发 webview blur，
      // 致 input 失焦（query 变化、搜索结果展开时尤其明显——"打第一个字母即失焦"）。
      // resize 后重新 focus 输入框，保证连续输入不中断。
      if (gen === resizeGenRef.current && document.activeElement !== inputRef.current) {
        inputRef.current?.focus();
      }
    };
    apply().catch(() => {});
  }, [view, query, filteredResults.length, expandDirection, context]);

  // mount + 每次 show 时拉取上下文 + 菜单 + 配置
  // showPayload: show 事件携带的 context（消除首屏竞态）；mount 首次为 undefined（走 invoke 兜底）
  useEffect(() => {
    const refresh = (showPayload?: Context | null) => {
      showTimeRef.current = Date.now(); // 记录 show 时刻，供 onFocusChanged 宽限判定
      // 前端获取键盘焦点（后端 show 不调 set_focus 避免激活 app），
      // 让方向键直接可用——用户无需鼠标点击
      window.focus();
      // 每次 show 都重置基础状态——防止遗留旧状态
      const applyContext = (ctx: Context | null) => {
        setView("main"); setSelectedIdx(0); setFocusLayer("main");
        setQuery(""); setInstantResults([]);
        setActiveTab("all"); setSearchSelectedIdx(0);
        // 清空 stale 位置——等 compute() 从后端重新读取
        baseWinPosRef.current = null;
        setContext(ctx);
        // 输入框始终保持 DOM focus（重构后设计意图——键盘 handler 738 行的放行逻辑依赖
        // activeElement===input）。无论有无选中文本，字母键都要能进搜索框触发过滤；
        // 有选中时菜单的方向键 / Alt+字母 快捷键由 window 级 handler 拦截，不受 input focus 影响
        // （handler 已对 Arrow/Tab/Enter 等导航键精确放行，不会被 input focus 劫持）。
        setTimeout(() => inputRef.current?.focus(), 50);
      };
      // 优先用 show 事件 payload（零延迟，消除首屏竞态）；无 payload（mount 首次）走 invoke 兜底
      if (showPayload !== undefined) {
        applyContext(showPayload);
      } else {
        invoke<Context | null>("action_bar_get_context").then((ctx) => applyContext(ctx));
      }
      // 每次唤起都重新加载菜单项 + 配置（设置页可能已改）
      invoke<ActionBarItem[]>("list_action_bar_items").then((items) => {
        setMenuItems(items);
      });
      invoke<{ config: Record<string, string | number | boolean> }>("get_config").then((resp) => {
        searchEngineRef.current = (resp.config.action_bar_search_engine as string) || "google";
      });
    };
    refresh();
    // show 事件携带 context payload——消除首屏竞态（原 refresh 内 invoke(get_context)
    // 是异步 Promise，窗口已 show 但 ctx 还在 pending 时首屏用陈旧 context state 渲染）。
    // mount 首次仍走 refresh（invoke 兜底），后续 show 从 payload 直接拿 context。
    const listenPromise = rawListen<Context | null>("action-bar://show", (event) => {
      refresh(event.payload);
    });
    // 设置页保存后 emit 此事件 → 浮窗立即刷新菜单（无需关闭再打开）
    const itemsListenPromise = rawListen("action-bar://items-changed", () => {
      invoke<ActionBarItem[]>("list_action_bar_items").then((items) => {
        setMenuItems(items);
      });
    });

    return () => {
      listenPromise.then((fn: () => void) => fn());
      itemsListenPromise.then((fn: () => void) => fn());
    };
  }, []);

  // 点击外部消失 + 窗口失焦消失
  useEffect(() => {
    const onClick = (e: MouseEvent) => {
      if (viewRef.current === "loading") return;
      const el = e.target as HTMLElement;
      if (el && el.closest("[data-action-bar]")) return;
      invoke("action_bar_dismiss", { reason: "click-outside" });
    };
    const unlistenFocus = getCurrentWindow().onFocusChanged(({ payload: focused }) => {
      const sinceShow = Date.now() - showTimeRef.current;
      if (!focused && viewRef.current !== "loading") {
        // show 后宽限：app 激活/窗口成为 key 有时序抖动，期间 spurious focus-lost 不触发 dismiss
        if (sinceShow < 500) return;
        invoke("action_bar_dismiss", { reason: "focus-lost" });
      }
    });
    const timer = setTimeout(() => {
      document.addEventListener("click", onClick, false);
    }, 300);
    return () => {
      clearTimeout(timer);
      document.removeEventListener("click", onClick, false);
      unlistenFocus.then((fn) => fn());
    };
  }, []);

  // ── 搜索：结果同步 + 展开方向 + 搜索请求 ──
  // filteredResultsRef 供键盘 handler 读取最新值（声明在使用之前）
  const filteredResultsRef = useRef<SearchHit[]>([]);
  useEffect(() => { filteredResultsRef.current = filteredResults; }, [filteredResults]);

  // 结果数量变化时 clamp 选中索引
  // 结果列表变化时重置选中到第一个——用户在输入时焦点在搜索框，
  // 鼠标悬停选择只在鼠标主动移动时生效（onMouseEnter）
  useEffect(() => {
    setSearchSelectedIdx(0);
  }, [filteredResults]);

  // 展开方向判定（show 时计算一次，一次 show 中固定）
  useEffect(() => {
    const compute = async () => {
      try {
        const win = getCurrentWindow();
        const pos = await win.outerPosition();
        const scaleFactor = await win.scaleFactor();
        // 用 window.screen 估算屏幕高度（逻辑坐标）
        // 多显示器时 outerPosition 可能超出主屏，但 ActionBar 靠近鼠标，通常在主屏
        const screenH = window.screen.height;
        const winYLogical = pos.y / scaleFactor;
        const winXLogical = pos.x / scaleFactor;
        baseWinPosRef.current = { x: winXLogical, y: winYLogical };
        setExpandDirection(determineExpandDirection(winYLogical, screenH));
      } catch { /* ignore */ }
    };
    compute();

    // 每次 show 重新计算
    const listenPromise = rawListen("action-bar://show", () => { compute(); });
    return () => { listenPromise.then((fn: () => void) => fn()); };
  }, []);

  // 流式搜索：后端 Provider 并发扇出，每完成一个 emit 当前全局 top-10（已去重排序）。
  // 前端单路流式：150ms 防抖避免逐字符打爆后端；payload.runId 校验防旧批次串扰；
  // 每次 batch 用最新结果整体替换（后端 emit 的是累积 top-N，不是单 Provider 增量）。
  // tab 参数 = 当前选中 Tab，后端据此决定哪些 Provider 跑（all → 全部）。
  useEffect(() => {
    if (!hasQuery(query)) {
      setInstantResults([]);
      return;
    }
    let cancelled = false;
    const timer = setTimeout(() => {
      if (cancelled) return;
      // onBatch 收到的已是本次会话（runId 匹配）的全局 top-N 累积结果，直接替换
      executeSearchStream(query, activeTab, (results) => {
        if (!cancelled) setInstantResults(results);
      }).catch(() => {
        if (!cancelled) setInstantResults([]);
      });
    }, getDebounceMs(activeTab));
    return () => {
      cancelled = true;
      clearTimeout(timer);
    };
  }, [query, activeTab]);

  // 组件卸载时清理 searchStream 的全局 listen 句柄（防内存泄漏）。
  // 注意：每次 query/activeTab 变化时 executeSearchStream 内部已 unlisten 旧监听，
  // 此 effect 只兜底最终卸载场景。
  useEffect(() => {
    return () => {
      cleanupSearchStream();
    };
  }, []);

  // 查询清空时重置搜索状态
  useEffect(() => {
    if (!hasQuery(query)) {
      setActiveTab("all");
      setSearchSelectedIdx(0);
    }
  }, [query]);

  const urlResult = context ? detectActionUrl(context.text || "") : { isUrl: false, url: "" };

  // accepts 过滤：按选中类型（text/files）过滤菜单项可见性
  const isItemVisible = (item: ActionBarItem): boolean => {
    // 禁用项不显示（与搜索引擎 engine.rs 的 .filter(|r| r.is_enabled && ...) 对齐）
    if (!item.isEnabled) return false;
    if (!context) return true;
    const accepts = item.accepts || "text";
    if (accepts === "any") return true;
    if (context.kind === "text") return accepts === "text";
    return accepts === "file";
  };

  // submenu 可见性：子项全不可见/全禁用则自身也隐藏
  const isSubmenuVisible = (item: ActionBarItem): boolean => {
    const allSubs = menuItems.filter((i) => i.parentId === item.id);
    if (allSubs.length === 0) return true; // 无子项——叶 submenu，自身可见
    const enabledSubs = allSubs.filter((i) => i.isEnabled);
    if (enabledSubs.length === 0) return false; // 有子项但全禁用——隐藏
    return enabledSubs.some((s) =>
      s.actionType === "submenu" ? isSubmenuVisible(s) : isItemVisible(s)
    );
  };

  // 派生菜单项
  const allMainItems = menuItems.filter((i) => i.parentId === null && i.isEnabled);
  const mainItems = allMainItems.filter((i) => {
    if (!isItemVisible(i)) return false;
    if (i.actionType === "submenu" && !isSubmenuVisible(i)) return false;
    // 网页项仅当选中文本是 URL 时显示
    if (i.actionType === "url" && i.actionData === "") return urlResult.isUrl;
    return true;
  });
  const getSubItems = (parentId: number) => menuItems.filter((i) => i.parentId === parentId && i.isEnabled && isItemVisible(i));

  // items 变化时 clamp 选中索引——防删除/设置改动后越界
  useEffect(() => {
    if (selectedIdx >= mainItems.length && mainItems.length > 0) setSelectedIdx(mainItems.length - 1);
  }, [mainItems.length]);

  // ── 动作执行 ──

  const executeAiItem = async (item: ActionBarItem) => {
    const ctx = contextRef.current;
    const text = ctx?.text || "";
    setView("loading");
    timedOutRef.current = false;

    // 本地翻译（auto_translate）可能耗时很长（长文本分段），不设超时
    // LLM 操作保留 10s 超时
    const isTranslate = item.actionData === "auto_translate";
    const timeoutMs = isTranslate ? 0 : AI_TIMEOUT_MS;
    const timeoutId = timeoutMs > 0 ? setTimeout(() => {
      timedOutRef.current = true;
      showQuickError(t("actionbar.timeout", { n: timeoutMs / 1000 }));
      setView("main");
    }, timeoutMs) : null;

    try {
      await invoke("execute_action_bar", { itemId: item.id, text });
      if (timeoutId) clearTimeout(timeoutId);
      if (timedOutRef.current) {
        console.warn("[action-bar] AI result arrived after timeout, discarding");
        return;
      }
      // LLM 路径：action_bar_show_result 后端已隐藏本窗口并展示 CompactEditor
      // 本地翻译路径：后端 return Ok(true) 不预隐藏，翻译完成后主线程隐藏 + 开结果 tab
      // 两种路径都依赖后端收口，前端 view 保持 "loading" 直到窗口被隐藏
    } catch (e) {
      if (timeoutId) clearTimeout(timeoutId);
      if (timedOutRef.current) return;
      showQuickError(String(e).slice(0, 40));
      setView("main");
    }
  };

  const executeItem = async (item: ActionBarItem) => {
    const ctx = contextRef.current;
    // accepts=any 的项不需要选中内容——无 ctx 时用空文本
    const text = ctx?.text || "";

    if (item.actionType === "submenu") {
      submenuParentIdRef.current = item.id;
      const subs = getSubItems(item.id);
      // 搜索子菜单默认高亮配置引擎
      if (subs.length > 0 && subs[0].actionType === "url") {
        const engineIdx = subs.findIndex((s) => s.title.toLowerCase() === searchEngineRef.current);
        setSubSelectedIdx(engineIdx >= 0 ? engineIdx : 0);
      } else {
        setSubSelectedIdx(0);
      }
      // executeItem 是终结性动作——展开 submenu 后焦点应进 sub 层，
      // 否则按 Enter 会走 main 分支再次执行父项（重复展开，Enter 失灵）。
      // 与 Tab/Alt+字母 的「预览展开不抢焦点」区分（架构文档契约）。
      setFocusLayer(nextFocusLayerAfterExecute(item.actionType, focusLayerRef.current));
      setView("submenu");
      return;
    }

    if (item.actionType === "ai") {
      executeAiItem(item);
      return;
    }

    // agent 类型：含 {{task}} → 联动语音录音；否则直接执行
    if (item.actionType === "agent") {
      if (item.actionData.includes("{{task}}")) {
        setView("loading");
        try {
          await invoke("trigger_agent_voice", { itemId: item.id });
        } catch (e) {
          showQuickError(String(e).slice(0, 40));
          setView("main");
        }
        return;
      }
      setView("loading");
      try {
        await invoke("execute_action_bar", { itemId: item.id, text: "" });
      } catch (e) {
        showQuickError(String(e).slice(0, 40));
        setView("main");
      }
      return;
    }

    // copy_path / url / script / copy
    try {
      await invoke("execute_action_bar", { itemId: item.id, text });
    } catch (e) {
      showQuickError(String(e).replace(/^脚本执行失败:\s*/, "").slice(0, 40));
    }
  };

  // ── 搜索结果执行 ──

  const executeSearchResult = async (result: SearchHit) => {
    const data = parseActionData(result.actionData);

    // 频次加权记录（fire-and-forget，失败不影响动作执行）。
    // spec §5.4：执行动作时记录，让 frequency.boost 在后续搜索中加权用户常用结果。
    // 放在 switch 之前，对所有 actionType 通用（含 launch_app/open_file/menu/url/copy）。
    invoke("record_search_hit", {
      source: result.source,
      actionType: result.actionType,
      actionData: result.actionData,
      query: queryRef.current,
    }).catch(() => {});

    switch (result.actionType) {
      case "launch_app": {
        const path = data.path as string;
        if (!path) return;
        try {
          await invoke("launch_app", { path });
          invoke("action_bar_dismiss", { reason: "launch-app" });
        } catch (e) {
          showQuickError(String(e).slice(0, 40));
        }
        break;
      }
      case "open_file": {
        const path = data.path as string;
        if (!path) return;
        try {
          await invoke("open_file", { path });
          invoke("action_bar_dismiss", { reason: "open-file" });
        } catch (e) {
          showQuickError(String(e).slice(0, 40));
        }
        break;
      }
      case "menu": {
        const itemId = data.id as number;
        const item = menuItemsRef.current.find((i) => i.id === itemId);
        if (item) {
          executeItem(item);
        }
        break;
      }
      case "url": {
        // Quicklink 关键词触发时 data.url 已含替换后的 URL
        // 非关键词匹配时 data.action_data 是原始模板（可能含 {query}/{text}）
        const ctx = contextRef.current;
        const fallbackText = ctx?.text || queryRef.current;
        const rawUrl = (data.url as string) || (data.action_data as string) || "";
        // 替换 URL 模板中的 {query} / {text} 占位符
        const url = rawUrl
          .replace(/\{query\}/g, encodeURIComponent(fallbackText))
          .replace(/\{text\}/g, encodeURIComponent(fallbackText));
        if (url) {
          try {
            await invoke("open_url", { url });
            invoke("action_bar_dismiss", { reason: "open-url" });
          } catch (e) {
            showQuickError(String(e).slice(0, 40));
          }
        }
        break;
      }
      case "shell": {
        // shell provider 已移除（launcher 场景下无终端上下文/无输出展示，伪需求）
        // 保留 case 防御性兜底——若历史频次记录里残留 shell 结果触发，静默忽略
        break;
      }
      case "copy": {
        // calculator / url 等"复制结果"动作：actionData = {"text": "..."}
        const text = data.text as string;
        if (!text) return;
        try {
          await navigator.clipboard.writeText(text);
          invoke("action_bar_dismiss", { reason: "copy" });
        } catch (e) {
          showQuickError(String(e).slice(0, 40));
        }
        break;
      }
    }
  };

  // ── 键盘导航 ──

  const selectedIdxRef = useRef(0);
  const subSelectedIdxRef = useRef(0);
  const mainItemsRef = useRef<ActionBarItem[]>([]);
  const subItemsRef = useRef<ActionBarItem[]>([]);
  const menuItemsRef = useRef<ActionBarItem[]>([]);
  // 搜索 refs（供键盘 handler 读取最新值）
  const queryRef = useRef("");
  const activeTabRef = useRef<TabId>("all");
  const searchSelectedIdxRef = useRef(0);
  useEffect(() => { selectedIdxRef.current = selectedIdx; }, [selectedIdx]);
  useEffect(() => { subSelectedIdxRef.current = subSelectedIdx; }, [subSelectedIdx]);
  useEffect(() => { mainItemsRef.current = mainItems; }, [mainItems]);
  useEffect(() => { menuItemsRef.current = menuItems; }, [menuItems]);
  useEffect(() => { queryRef.current = query; }, [query]);
  useEffect(() => { activeTabRef.current = activeTab; }, [activeTab]);
  useEffect(() => { searchSelectedIdxRef.current = searchSelectedIdx; }, [searchSelectedIdx]);
  useEffect(() => {
    subItemsRef.current = submenuParentIdRef.current !== null
      ? getSubItems(submenuParentIdRef.current)
      : [];
  });

  useEffect(() => {
    const handler = (e: KeyboardEvent) => {
      // IME 组合中的按键（keyCode 229 或 isComposing=true）——一律放行，不干预。
      // 两种模式（搜索/菜单）统一处理：IME 接管输入，handler 不 preventDefault/return 吃掉按键。
      // 否则菜单模式下 IME 按键被 handler 条件分支拦截，input 没收到原生事件，
      // 而 IME compositionend 仍插入字符 → 字符重复（输 wx 变 wwx）。
      if (e.keyCode === 229 || e.isComposing) {
        lastImeKeyTime.current = Date.now();
        return;
      }
      // Enter(13) 在 IME 按键后 500ms 内 → 选词确认，跳过
      if (e.key === "Enter" && Date.now() - lastImeKeyTime.current < 500) {
        lastImeKeyTime.current = 0;
        return;
      }

      // Escape 在任何视图都生效——防止 loading 卡住时困死用户
      if (e.key === "Escape") {
        e.preventDefault();
        if (hasQuery(queryRef.current)) {
          setQuery("");
          inputRef.current?.focus();
        } else {
          invoke("action_bar_dismiss", { reason: "escape" });
        }
        return;
      }

      // loading 视图不拦截其他键盘导航
      if (viewRef.current === "loading") return;

      // ── 统一放行：无修饰的可打印字符（字母/数字/Backspace）交给输入框原生处理 ──
      // 搜索模式和菜单模式共享此逻辑——两种模式对输入框输入行为完全一致。
      // IME 组合按键已在顶部拦截（229/isComposing），修饰键(Alt/Cmd/Ctrl)交各自分支处理。
      // 导航键（Tab/Arrow/Enter/Space）不走这里——它们在各自模式的分支里处理。
      if (!e.altKey && !e.metaKey && !e.ctrlKey) {
        const navKeys = ARROW_AS_TAB
          ? ["ArrowUp", "ArrowDown", "ArrowLeft", "ArrowRight", "Tab", "Enter", " "]
          : ["ArrowUp", "ArrowDown", "Tab", "Enter", " "];
        if (!navKeys.includes(e.key)) {
          return; // 可打印字符 → 放行给 input
        }
      }

      // ── 搜索模式键盘导航（query 非空时）──
      // 简化设计：输入框始终是焦点，Tab 键只切换 Tab 页，不改变焦点
      // Cmd+字母 快捷定位 Tab 页
      if (hasQuery(queryRef.current)) {
        const results = filteredResultsRef.current;

        // Alt + 字母 → 快捷切换 Tab 页（统一 Alt=定位/切换；Cmd/Ctrl 留给执行）
        // Alt 改变 e.key 输出（如 Alt+A → "å"），用 codeToChar(e.code) 取物理键
        // 无选中时隐藏"动作"Tab（菜单项需要选中内容）
        // Alt+字母始终 preventDefault——用户意图是切 Tab，不是输入 Alt 变异字符（如 Alt+Z → "ˀ"）
        const hasCtx = !!contextRef.current;
        if (e.altKey) {
          const ch = codeToChar(e.code);
          if (ch) {
            e.preventDefault();
            const tabByKey = getTabByKey(ch, hasCtx);
            if (tabByKey) {
              setActiveTab(tabByKey);
            }
          }
          return;
        }

        // Tab 或（ARROW_AS_TAB 时）←/→ → 循环切换 Tab 页
        const dir = moveDirection(e.key, e.shiftKey, ARROW_AS_TAB);
        if (dir !== null) {
          e.preventDefault();
          setActiveTab(getNextTab(activeTabRef.current, dir ? 1 : -1, hasCtx));
          return;
        }

        // ↑↓ → 导航结果列表
        if (e.key === "ArrowDown") {
          e.preventDefault();
          setSearchSelectedIdx(navigateResults(searchSelectedIdxRef.current, 1, results.length));
          return;
        }
        if (e.key === "ArrowUp") {
          e.preventDefault();
          setSearchSelectedIdx(navigateResults(searchSelectedIdxRef.current, -1, results.length));
          return;
        }

        // Enter → 执行选中项（或第一个）
        if (e.key === "Enter") {
          e.preventDefault();
          const selected = results[searchSelectedIdxRef.current] ?? results[0];
          if (selected) executeSearchResult(selected);
          return;
        }

        // 其他键 → 已在上游统一放行（无修饰可打印字符），这里兜底
        return;
      }

      // ── 菜单模式键盘导航（query 为空时）──
      // 无修饰的可打印字符已在上游统一放行，这里只处理修饰键 + 导航键。

      // Cmd/Ctrl + 数字/字母 → 直接执行（按菜单项配置的 shortcut 匹配；原 Alt+字母 的功能）
      // macOS Cmd 不改变字母输出，统一用 codeToChar(e.code) 取物理键
      if (e.metaKey || e.ctrlKey) {
        const ch = codeToChar(e.code);
        if (ch) {
          const item = menuItemsRef.current.find((i: ActionBarItem) => i.isEnabled && i.shortcut === ch);
          if (item) {
            e.preventDefault();
            executeItem(item);
          }
        }
        return;
      }

      // Alt + 数字/字母 → 定位菜单项（按位置 labelToIndex，选中不执行；原 无修饰数字/字母 的功能）
      // Alt 改变 e.key 输出（如 Alt+H → "˙"），用 codeToChar(e.code) 取物理键再 labelToIndex
      if (e.altKey) {
        const ch = codeToChar(e.code);
        if (ch) {
          const idx = labelToIndex(ch);
          if (idx >= 0) {
            e.preventDefault();
            if (focusLayerRef.current === "sub") {
              if (idx < subItemsRef.current.length) setSubSelectedIdx(idx);
            } else {
              if (idx < mainItemsRef.current.length) {
                const item = mainItemsRef.current[idx];
                setSelectedIdx(idx);
                // submenu 项同步展开子菜单预览（与 Tab 移动行为一致）
                if (item.actionType === "submenu") {
                  submenuParentIdRef.current = item.id;
                  const subs = menuItemsRef.current.filter((i: ActionBarItem) => i.isEnabled && i.parentId === item.id);
                  if (subs.length > 0 && subs[0].actionType === "url") {
                    const engineIdx = subs.findIndex((s: ActionBarItem) => s.title.toLowerCase() === searchEngineRef.current);
                    setSubSelectedIdx(engineIdx >= 0 ? engineIdx : 0);
                  } else {
                    setSubSelectedIdx(0);
                  }
                  setView("submenu");
                } else {
                  submenuParentIdRef.current = null;
                  setView("main");
                }
              }
            }
          }
        }
        return;
      }

      // Tab 或（ARROW_AS_TAB 时）←/→ → 菜单项间移动
      const menuDir = moveDirection(e.key, e.shiftKey, ARROW_AS_TAB);
      if (menuDir !== null) {
        e.preventDefault();
        const forward = menuDir;
        if (focusLayerRef.current === "sub") {
          // 焦点在子菜单——在子菜单项间移动
          setSubSelectedIdx((prev) => {
            const items = subItemsRef.current;
            if (items.length === 0) return 0;
            return forward ? (prev + 1) % items.length : (prev - 1 + items.length) % items.length;
          });
        } else {
          // 焦点在主菜单——在主菜单项间移动，submenu 项自动展开子菜单预览
          setSelectedIdx((prev) => {
            const items = mainItemsRef.current;
            if (items.length === 0) return 0;
            const next = forward ? (prev + 1) % items.length : (prev - 1 + items.length) % items.length;
            const item = items[next];
            if (item && item.actionType === "submenu") {
              submenuParentIdRef.current = item.id;
              const subs = menuItemsRef.current.filter((i: ActionBarItem) => i.isEnabled && i.parentId === item.id);
              if (subs.length > 0 && subs[0].actionType === "url") {
                const engineIdx = subs.findIndex((s: ActionBarItem) => s.title.toLowerCase() === searchEngineRef.current);
                setSubSelectedIdx(engineIdx >= 0 ? engineIdx : 0);
              } else {
                setSubSelectedIdx(0);
              }
              setView("submenu");
            } else {
              submenuParentIdRef.current = null;
              setView("main");
            }
            return next;
          });
        }
        return;
      }

      if (e.key === "ArrowUp" || e.key === "ArrowDown") {
        e.preventDefault();
        // 上下键只切换焦点层（main↔sub），不展开/收起子菜单
        // 子菜单展开/收起由 Tab/Shift+Tab 移动主菜单项时控制
        if (focusLayerRef.current === "sub") {
          setFocusLayer("main");
        } else {
          // 焦点在主菜单——只有当前主菜单项有子菜单时才能进入
          const cur = mainItemsRef.current[selectedIdxRef.current];
          if (cur && cur.actionType === "submenu") {
            setFocusLayer("sub");
            // 如果子菜单还没展开（理论上左右键已经展开了），确保展开
            if (viewRef.current !== "submenu") {
              submenuParentIdRef.current = cur.id;
              setView("submenu");
              const subs = menuItemsRef.current.filter((i: ActionBarItem) => i.isEnabled && i.parentId === cur.id);
              if (subs.length > 0 && subs[0].actionType === "url") {
                const engineIdx = subs.findIndex((s: ActionBarItem) => s.title.toLowerCase() === searchEngineRef.current);
                setSubSelectedIdx(engineIdx >= 0 ? engineIdx : 0);
              } else {
                setSubSelectedIdx(0);
              }
            }
          }
        }
        return;
      }

      if (e.key === "Enter" || e.key === " ") {
        e.preventDefault();
        if (focusLayerRef.current === "sub") {
          const items = subItemsRef.current;
          const item = items[subSelectedIdxRef.current];
          if (item) executeItem(item);
        } else {
          const item = mainItemsRef.current[selectedIdxRef.current];
          if (item) executeItem(item);
        }
        return;
      }
    };
    window.addEventListener("keydown", handler);
    return () => window.removeEventListener("keydown", handler);
  }, []);

  // ── 渲染 ──

  const inSearch = hasQuery(query);

  // submenu 项变化时 clamp subSelectedIdx——防缩短后越界 Enter 静默失败
  // 必须在 early return (loading view) 之前，否则 React hooks 数量不一致
  const subItems = submenuParentIdRef.current !== null
    ? getSubItems(submenuParentIdRef.current)
    : [];
  useEffect(() => {
    if (view === "submenu" && subSelectedIdx >= subItems.length && subItems.length > 0) {
      setSubSelectedIdx(subItems.length - 1);
    }
  }, [subItems.length, view]);

  // 搜索输入框组件——高度 = INPUT_HEIGHT（44px），字号 15px，视觉重心
  const searchInputEl = (
    <div
      className={cn(
        "flex items-center gap-2.5 px-4 shrink-0",
        inSearch ? "h-[44px]" : "h-[44px] border-b border-border/30",
      )}
      onMouseDown={(e) => e.stopPropagation()}
    >
      <Search className="w-[18px] h-[18px] text-muted-foreground/80 shrink-0" />
      <input
        ref={inputRef}
        type="text"
        value={query}
        onChange={(e) => setQuery(e.target.value)}
        placeholder={t("actionbar.searchPlaceholder")}
        className="flex-1 bg-transparent text-[15px] font-medium text-foreground placeholder:text-muted-foreground/40 placeholder:font-normal outline-none border-none min-w-0"
        autoComplete="off"
        autoCorrect="off"
        spellCheck={false}
      />
    </div>
  );

  if (view === "loading") {
    return (
      <div
        ref={actionBarRef}
        data-action-bar
        className="relative flex flex-col rounded-[10px] border border-border/40 shadow-2xl shadow-black/20 overflow-hidden bg-background/90 backdrop-blur-2xl"
      >
        {searchInputEl}
        <div className="flex items-center justify-center gap-2.5 px-6 py-3 text-foreground">
          <Loader2 className="w-4 h-4 animate-spin text-voice" />
          <span className="text-[12px] font-medium">{t("actionbar.processing")}</span>
          <span className="flex gap-0.5">
            <span className="w-1 h-1 rounded-full bg-voice/40 animate-pulse" style={{ animationDelay: "0ms" }} />
            <span className="w-1 h-1 rounded-full bg-voice/40 animate-pulse" style={{ animationDelay: "150ms" }} />
            <span className="w-1 h-1 rounded-full bg-voice/40 animate-pulse" style={{ animationDelay: "300ms" }} />
          </span>
        </div>
      </div>
    );
  }

  const menuContent = (
    <>
      {/* 主菜单 */}
      <ScrollRow>
        {mainItems.map((item, i) => (
          <IconBtn
            key={item.id}
            index={i}
            label={item.title}
            active={selectedIdx === i}
            onClick={() => executeItem(item)}
            btnRef={(el: HTMLButtonElement | null) => { mainBtnRefs.current[i] = el; }}
            shortcut={item.shortcut}
          />
        ))}
      </ScrollRow>
      {/* 子菜单——展开时用渐变分隔线 + 轻微底色区分 */}
      <ScrollRow className={cn(
        "transition-all duration-200",
        view === "submenu"
          ? "border-t border-border/25 bg-foreground/[0.025]"
          : "h-0 overflow-hidden",
      )}>
        {subItems.map((item, i) => (
          <IconBtn
            key={item.id}
            index={i}
            label={item.title}
            active={focusLayer === "sub" && subSelectedIdx === i}
            onClick={() => executeItem(item)}
            btnRef={(el: HTMLButtonElement | null) => { subBtnRefs.current[i] = el; }}
            shortcut={item.shortcut}
          />
        ))}
      </ScrollRow>
    </>
  );

  const searchContent = inSearch ? (
    <SearchPanel
      results={filteredResults}
      activeTab={activeTab}
      selectedIdx={searchSelectedIdx}
      expandDirection={expandDirection}
      hasContext={!!context}
      onTabChange={setActiveTab}
      onSelect={setSearchSelectedIdx}
      onExecute={executeSearchResult}
    />
  ) : null;

  return (
    <div
      ref={actionBarRef}
      data-action-bar
      className="relative flex flex-col rounded-[10px] border border-border/40 shadow-2xl shadow-black/20 overflow-hidden bg-background/90 backdrop-blur-2xl"
    >
      {toast && (
        <div className="absolute inset-x-0 top-0 z-10 flex items-center justify-center bg-red-500/95 backdrop-blur-md rounded-t-[10px] px-3 py-2 animate-in fade-in duration-150">
          <span className="text-[11px] font-medium text-white text-center leading-tight line-clamp-2">{toast}</span>
        </div>
      )}
      {/* 展开方向决定 DOM 顺序：
          down → [Input] [Search/Menu]
          up   → [Search] [Input]  (搜索模式) 或 [Input] [Menu] (菜单模式)
          无选中（context=null）时只显示搜索框，不显示菜单条 */}
      {inSearch && expandDirection === "up" ? (
        <>
          {searchContent}
          {searchInputEl}
        </>
      ) : (
        <>
          {searchInputEl}
          {inSearch ? searchContent : (context ? menuContent : null)}
        </>
      )}
    </div>
  );
}
