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
  DELAYED_SEARCH_DEBOUNCE_MS,
  type TabId,

  type ExpandDirection,
  type SearchResult as SearchHit,
} from "./searchTypes";
import {
  determineExpandDirection,
  getTabByKey,
  getNextTab,
  shouldTriggerDelayedSearch,
  mergeResults,
  filterByTab,
  parseActionData,
  calcResultsHeight,
  navigateResults,
  hasQuery,
  nextFocusLayerAfterExecute,
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

type View = "main" | "submenu" | "loading";

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
      "flex items-center gap-1.5 px-2 py-1.5 rounded-lg transition-all duration-150 shrink-0",
      active
        ? "bg-voice/12 text-voice"
        : "text-muted-foreground hover:bg-foreground/5 hover:text-foreground",
    )}
    onMouseDown={(e) => e.stopPropagation()}
    onClick={onClick}
    title={`${label} — Alt+${indexLabel(index)} 定位${shortcut ? ` · ⌘${shortcut} 执行` : ""}`}
  >
    <span
      className={cn(
        "inline-flex h-[18px] w-[18px] items-center justify-center rounded-md font-mono text-[11px] font-semibold tabular-nums leading-none",
        active
          ? "bg-voice text-white"
          : "bg-muted text-muted-foreground",
      )}
    >
      {indexLabel(index)}
    </span>
    <span className="text-[10px] font-medium leading-none whitespace-nowrap">{label}</span>
    {shortcut && (
      <span className="text-[9px] text-voice/70 font-mono leading-none">⌘{shortcut}</span>
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
  const [delayedResults, setDelayedResults] = useState<SearchHit[]>([]);
  const [searchSelectedIdx, setSearchSelectedIdx] = useState(0);
  const [expandDirection, setExpandDirection] = useState<ExpandDirection>("down");
  const inputRef = useRef<HTMLInputElement>(null);
  const baseWinPosRef = useRef<{ x: number; y: number } | null>(null);
  const lastImeKeyTime = useRef(0);
  const showTimeRef = useRef(0);

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

  // ── 搜索：合并结果 + 按Tab过滤（需在 resize effect 之前声明）──
  const allResults = useMemo(() => mergeResults(instantResults, delayedResults), [instantResults, delayedResults]);

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
      // 无选中（context=null）时只有搜索框，无菜单条
      const menuHeight = !context ? 0 : view === "submenu" ? 78 : view === "loading" ? 48 : 40;
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
      await win.setSize(new LogicalSize(380, totalHeight));
      if (gen !== resizeGenRef.current) return;
      if (targetX !== null && targetY !== null) {
        await win.setPosition(new LogicalPosition(targetX, targetY));
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
        setQuery(""); setInstantResults([]); setDelayedResults([]);
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

  // 即时搜索（应用+菜单+Quicklinks，无防抖，纯内存索引）
  useEffect(() => {
    if (!hasQuery(query)) {
      setInstantResults([]);
      return;
    }
    let cancelled = false;
    // 始终用 "quick" tab 做即时搜索（应用+菜单+Quicklinks）
    // 文件/书签结果由延迟搜索补充，避免每次按键触发 mdfind
    invoke<SearchHit[]>("search_all", { query, tab: "quick" }).then((results) => {
      if (!cancelled) setInstantResults(results);
    }).catch(() => {
      if (!cancelled) setInstantResults([]);
    });
    return () => { cancelled = true; };
  }, [query]);

  // 延迟搜索（文件+书签，150ms 防抖，query ≥ 2 字符）
  // all/files/bookmarks tab 都需要延迟搜索结果
  useEffect(() => {
    if (!shouldTriggerDelayedSearch(query)) {
      setDelayedResults([]);
      return;
    }
    // apps/shell tab 不需要文件/书签
    if (activeTab !== "all" && activeTab !== "files" && activeTab !== "bookmarks") {
      setDelayedResults([]);
      return;
    }
    // 确定 tab 参数：all → files_bookmarks，files → files，bookmarks → bookmarks
    const searchTab = activeTab === "files" ? "files" : activeTab === "bookmarks" ? "bookmarks" : "files_bookmarks";
    let cancelled = false;
    const timer = setTimeout(() => {
      invoke<SearchHit[]>("search_all", { query, tab: searchTab }).then((results) => {
        if (!cancelled) setDelayedResults(results);
      }).catch(() => {
        if (!cancelled) setDelayedResults([]);
      });
    }, DELAYED_SEARCH_DEBOUNCE_MS);
    return () => { cancelled = true; clearTimeout(timer); };
  }, [query, activeTab]);

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
    if (!context) return true;
    const accepts = item.accepts || "text";
    if (accepts === "any") return true;
    if (context.kind === "text") return accepts === "text";
    return accepts === "file";
  };

  // submenu 可见性：子项全不可见则自身也隐藏
  const isSubmenuVisible = (item: ActionBarItem): boolean => {
    const subs = menuItems.filter((i) => i.parentId === item.id);
    if (subs.length === 0) return true;
    return subs.some((s) =>
      s.actionType === "submenu" ? isSubmenuVisible(s) : isItemVisible(s)
    );
  };

  // 派生菜单项
  const allMainItems = menuItems.filter((i) => i.parentId === null);
  const mainItems = allMainItems.filter((i) => {
    if (!isItemVisible(i)) return false;
    if (i.actionType === "submenu" && !isSubmenuVisible(i)) return false;
    // 网页项仅当选中文本是 URL 时显示
    if (i.actionType === "url" && i.actionData === "") return urlResult.isUrl;
    return true;
  });
  const getSubItems = (parentId: number) => menuItems.filter((i) => i.parentId === parentId && isItemVisible(i));

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
        const command = data.command as string;
        if (!command) return;
        setView("loading");
        try {
          await invoke<string>("execute_shell", { command });
          invoke("action_bar_dismiss", { reason: "execute-shell" });
        } catch (e) {
          showQuickError(String(e).slice(0, 40));
          setView("main");
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
      // IME 处理中的按键（keyCode 229）——记录时间，放行
      if (e.keyCode === 229) {
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

      // ── 搜索模式键盘导航（query 非空时）──
      // 简化设计：输入框始终是焦点，Tab 键只切换 Tab 页，不改变焦点
      // Cmd+字母 快捷定位 Tab 页
      if (hasQuery(queryRef.current)) {
        const results = filteredResultsRef.current;

        // Alt + 字母 → 快捷切换 Tab 页（统一 Alt=定位/切换；Cmd/Ctrl 留给执行）
        // Alt 改变 e.key 输出（如 Alt+A → "å"），用 codeToChar(e.code) 取物理键
        if (e.altKey) {
          const ch = codeToChar(e.code);
          if (ch) {
            const tabByKey = getTabByKey(ch);
            if (tabByKey) {
              e.preventDefault();
              setActiveTab(tabByKey);
            }
          }
          return;
        }

        // Tab → 循环切换 Tab 页
        if (e.key === "Tab") {
          e.preventDefault();
          setActiveTab(getNextTab(activeTabRef.current, e.shiftKey ? -1 : 1));
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

        // 其他键 → 交给输入框处理
        return;
      }

      // ── 菜单模式键盘导航（query 为空时，沿用现有逻辑）──

      // input 始终 focus：放行【无修饰】的可打印字符（字母/数字/Backspace）与左右方向键（移光标）进输入框；
      // 修饰键(Alt/Cmd/Ctrl)+字符不放行——交下方分支（Alt=定位 / Cmd·Ctrl=执行）。
      // Tab（菜单项切换）/ ↑↓（主子菜单层切换）/ Enter·Space（执行菜单项）也由本 handler 处理。
      // input 无多行/回车概念——Enter 交给 handler 执行菜单项，不放行给输入框。Escape 已在上方处理。
      if (document.activeElement === inputRef.current && !e.altKey && !e.metaKey && !e.ctrlKey) {
        const navKeys = ["ArrowUp", "ArrowDown", "Tab", "Enter", " "];
        if (!navKeys.includes(e.key)) {
          return;
        }
      }

      // Cmd/Ctrl + 数字/字母 → 直接执行（按菜单项配置的 shortcut 匹配；原 Alt+字母 的功能）
      // macOS Cmd 不改变字母输出，统一用 codeToChar(e.code) 取物理键
      if (e.metaKey || e.ctrlKey) {
        const ch = codeToChar(e.code);
        if (ch) {
          const item = menuItemsRef.current.find((i: ActionBarItem) => i.shortcut === ch);
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
                  const subs = menuItemsRef.current.filter((i: ActionBarItem) => i.parentId === item.id);
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

      if (e.key === "Tab") {
        e.preventDefault();
        const forward = !e.shiftKey;
        if (focusLayerRef.current === "sub") {
          // 焦点在子菜单——Tab/Shift+Tab 在子菜单项间移动
          setSubSelectedIdx((prev) => {
            const items = subItemsRef.current;
            if (items.length === 0) return 0;
            return forward ? (prev + 1) % items.length : (prev - 1 + items.length) % items.length;
          });
        } else {
          // 焦点在主菜单——Tab/Shift+Tab 在主菜单项间移动，submenu 项自动展开子菜单预览
          setSelectedIdx((prev) => {
            const items = mainItemsRef.current;
            if (items.length === 0) return 0;
            const next = forward ? (prev + 1) % items.length : (prev - 1 + items.length) % items.length;
            const item = items[next];
            if (item && item.actionType === "submenu") {
              submenuParentIdRef.current = item.id;
              const subs = menuItemsRef.current.filter((i: ActionBarItem) => i.parentId === item.id);
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
              const subs = menuItemsRef.current.filter((i: ActionBarItem) => i.parentId === cur.id);
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

  // 搜索输入框组件
  const searchInputEl = (
    <div
      className={cn(
        "flex items-center gap-2 px-3 shrink-0",
        inSearch ? "h-[36px]" : "h-[36px] border-b border-border/20",
      )}
      onMouseDown={(e) => e.stopPropagation()}
    >
      <Search className="w-3.5 h-3.5 text-muted-foreground shrink-0" />
      <input
        ref={inputRef}
        type="text"
        value={query}
        onChange={(e) => setQuery(e.target.value)}
        placeholder={t("actionbar.searchPlaceholder")}
        className="flex-1 bg-transparent text-[12px] text-foreground placeholder:text-muted-foreground/50 outline-none border-none min-w-0"
        autoComplete="off"
        autoCorrect="off"
        spellCheck={false}
      />
    </div>
  );

  if (view === "loading") {
    return (
      <div
        data-action-bar
        className="relative flex flex-col rounded-lg border border-border/50 shadow-2xl shadow-black/10 overflow-hidden bg-background/95 backdrop-blur-xl"
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
          ? "border-t border-border/30 bg-foreground/[0.02]"
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
      onTabChange={setActiveTab}
      onSelect={setSearchSelectedIdx}
      onExecute={executeSearchResult}
    />
  ) : null;

  return (
    <div
      data-action-bar
      className="relative flex flex-col rounded-lg border border-border/50 shadow-2xl shadow-black/10 overflow-hidden bg-background/95 backdrop-blur-xl"
    >
      {toast && (
        <div className="absolute inset-x-0 top-0 z-10 flex items-center justify-center bg-red-500/90 backdrop-blur-sm px-3 py-2 animate-in fade-in duration-150">
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
