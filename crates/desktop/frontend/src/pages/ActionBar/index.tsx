import { useState, useEffect, useRef, useMemo, useCallback } from "react";
import { invoke } from "@/lib/tauri";
import { listen as rawListen } from "@tauri-apps/api/event";
import { getCurrentWindow, LogicalSize, LogicalPosition } from "@tauri-apps/api/window";
import { cn } from "@/lib/utils";
import { Loader2, Search } from "lucide-react";
import { detectActionUrl } from "./urlDetect";
import { t } from "@/lib/i18n";
import SearchPanel from "./SearchPanel";
import IconBtn from "./IconBtn";
import ScrollRow from "./ScrollRow";
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
import { executeSearchStream, cleanupSearchStream } from "./searchStream";
import {
  determineExpandDirection,
  filterByTab,
  parseActionData,
  calcResultsHeight,
  hasQuery,
  nextFocusLayerAfterExecute,
  calcMenuHeight,
} from "./searchLogic";
import type { Context, ActionBarItem } from "./types";
import { useActionBarKeydown } from "./useActionBarKeydown";

// 只有走 mdfind（file/bookmark Provider，慢）的 Tab 才需防抖；其他 Tab（含 all）
// 走内存 Provider（app/menu/calculator/url），亚毫秒，无需防抖。
const DEBOUNCED_TABS = new Set<TabId | "files_bookmarks" | "quick">([
  "files",
  "bookmarks",
  "files_bookmarks",
]);
function getDebounceMs(tab: TabId): number {
  return (DEBOUNCED_TABS as Set<string>).has(tab) ? DELAYED_SEARCH_DEBOUNCE_MS : 0;
}

const AI_TIMEOUT_MS = 10000;

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
  // slash Tab 补全锁定的菜单项 id。非空时：query 变化不重新搜索（保持候选），
  // 执行用此 id + query 参数（不从文本解析命令）。解锁条件见 query effect。
  const [slashLockedItemId, setSlashLockedItemId] = useState<number | null>(null);
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

  // task-input 视图已移除（agent 含 {{voice}} 改为联动语音）

  // ── 搜索：结果（流式后端 emit 累积 top-N，前端整体替换；无 delayed 合并）──
  const allResults = instantResults;

  // 解析菜单项的 app_bundle_ids JSON 数组。空串/非法 JSON → 空数组（全局项）。
  const parseAppBundleIds = (s?: string): string[] => {
    if (!s) return [];
    try {
      const arr = JSON.parse(s);
      return Array.isArray(arr) ? arr.filter((x) => typeof x === "string") : [];
    } catch {
      return [];
    }
  };

  // app-aware 过滤：app_bundle_ids 为空 = 全局项永远显示；非空 = 仅当前前台 app 在列表中才显示。
  // 拿不到前台 bundle_id 时，专属项隐藏（保守——INV-A5）。
  const isItemVisibleForApp = (item: ActionBarItem, bundleId?: string): boolean => {
    const ids = parseAppBundleIds(item.appBundleIds);
    if (ids.length === 0) return true; // 全局项
    if (!bundleId) return false; // 有绑定但拿不到前台 app → 隐藏专属项
    return ids.includes(bundleId);
  };

  // 按 context.accepts + app_bundle_ids 过滤菜单/quicklink 搜索结果
  // （Files 场景下 text-only 项如翻译不应出现，反之亦然；app 绑定的项不在当前 app 也不显示）
  // 无选中（context=null）时仅显示 accepts="any" 的 menu/quicklink 项
  const contextFilteredResults = useMemo(() => {
    return allResults.filter((r) => {
      if (r.source !== "menu" && r.source !== "quicklink") return true;
      const data = parseActionData(r.actionData);
      const item = menuItems.find((i) => i.id === (data.id as number));
      if (!item) return true;
      const accepts = item.accepts || "text";
      if (accepts !== "any") {
        if (!context) return false; // 无选中：非 any 的 menu 项不显示
        const isFiles = context.kind === "files";
        if (isFiles ? accepts !== "file" : accepts !== "text") return false;
      }
      // app 过滤（与 isItemVisible 同逻辑）
      return isItemVisibleForApp(item, context?.source?.bundleId);
    });
  }, [allResults, context, menuItems]);

  const filteredResults = useMemo(() => filterByTab(contextFilteredResults, activeTab), [contextFilteredResults, activeTab]);

  // 动态调整窗口高度 + 位置（展开方向）
  // 用 generation token 防"快速 Tab 切换"时异步乱序——只让最后一次 resize 生效
  const resizeGenRef = useRef(0);
  // 上次 resize 时的目标高度——搜索流式 batch 触发 effect 时，若 totalHeight 未变则跳过
  // setSize（避免每 batch setSize+setPosition 抖动 + focus 争夺）。2026-07-17 性能优化。
  const lastTotalHeightRef = useRef<number | null>(null);
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

    // 高度未变 → 跳过本次 resize（搜索流多 batch 但 length 未涨 / 仅 query 文本变化等场景）
    // 仍保留 focus 恢复（input 可能因别的操作失焦）
    const heightChanged = lastTotalHeightRef.current !== totalHeight;
    lastTotalHeightRef.current = totalHeight;

    // 序列化：每次只执行最新一代 resize
    const gen = ++resizeGenRef.current;
    const apply = async () => {
      if (gen !== resizeGenRef.current) return; // 已被更新一代取代
      if (heightChanged) {
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
        setSlashLockedItemId(null);
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
  // 输入 / 开头 → 自动跳 slash tab（命令模式）。
  // 不在删掉 / 时强制切回 all——用户可能想手动切（query 清空时下方 reset effect 已兜底回 all）。
  useEffect(() => {
    if (query.startsWith("/") && activeTab !== "slash") {
      setActiveTab("slash");
    }
  }, [query, activeTab]);

  // slash 补全锁定后的解锁检测（必须在 search stream effect 之前声明，
  // 保证解锁优先于"锁定时跳过搜索"判断）：
  // - 切走 slash tab → 解锁（补全语义只在 slash tab 有效）
  // - 锁定菜单项已不存在（设置页删除）→ 解锁
  // - query 不再以 `/标题` 或 `、标题` 开头（用户删了标题/改了前缀）→ 解锁，恢复 fuzzy 候选
  useEffect(() => {
    if (slashLockedItemId === null) return;
    if (activeTab !== "slash") {
      setSlashLockedItemId(null);
      return;
    }
    const locked = menuItems.find((i) => i.id === slashLockedItemId);
    const title = locked?.title;
    if (typeof title !== "string") {
      setSlashLockedItemId(null);
      return;
    }
    // 补全后 query 形如 `/标题 ` 或 `/标题 params`；标题含空格也成立（前缀整体匹配）
    if (!query.startsWith("/" + title) && !query.startsWith("、" + title)) {
      setSlashLockedItemId(null);
    }
  }, [query, slashLockedItemId, activeTab, menuItems]);

  useEffect(() => {
    if (!hasQuery(query)) {
      setInstantResults([]);
      return;
    }
    // slash 补全锁定时：query 变化（输参数）不重新搜索，保持补全时的候选列表。
    // 解锁由上方 effect 检测；解锁后 slashLockedItemId 变 null → 本 effect 重跑搜索。
    if (slashLockedItemId !== null && activeTab === "slash") {
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
  }, [query, activeTab, slashLockedItemId]);

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
      setSlashLockedItemId(null);
    }
  }, [query]);

  const urlResult = context ? detectActionUrl(context.text || "") : { isUrl: false, url: "" };

  // accepts + app 双维度过滤：两者都通过才显示（INV-A3 独立 AND）
  const isItemVisible = (item: ActionBarItem): boolean => {
    // 禁用项不显示（与搜索引擎 engine.rs 的 .filter(|r| r.is_enabled && ...) 对齐）
    if (!item.isEnabled) return false;
    if (!context) return true;
    // accepts 过滤（选中类型 text/files）
    const accepts = item.accepts || "text";
    if (accepts !== "any") {
      if (context.kind === "text" && accepts !== "text") return false;
      if (context.kind === "files" && accepts !== "file") return false;
    }
    // app 过滤（前台 app 绑定）——accepts=any 也要过 app 过滤
    return isItemVisibleForApp(item, context.source?.bundleId);
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

    // agent 类型：need_voice=true → 联动语音录音；否则直接执行
    // 2026-07-19 v40 改：从扫描 actionData.includes("{{voice}}") 改为 needVoice 字段
    if (item.actionType === "agent") {
      if (item.needVoice) {
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
        await invoke("execute_action_bar", { itemId: item.id, text });
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

    // url 模板替换 helper（I2）：slash 分流与 case "url" 共用，消除重复的
    // {query}/{text} 替换 + open_url + dismiss 三段逻辑。调用方各自传入 fallbackText。
    const openUrlTemplate = async (rawUrl: string, fallbackText: string, reason: string) => {
      const url = rawUrl
        .replace(/\{query\}/g, encodeURIComponent(fallbackText))
        .replace(/\{text\}/g, encodeURIComponent(fallbackText));
      if (!url) return;
      try {
        await invoke("open_url", { url });
        invoke("action_bar_dismiss", { reason });
      } catch (e) {
        showQuickError(String(e).slice(0, 40));
      }
    };

    // 频次加权记录（fire-and-forget，失败不影响动作执行）。
    // spec §5.4：执行动作时记录，让 frequency.boost 在后续搜索中加权用户常用结果。
    // 放在 switch 之前，对所有 actionType 通用（含 launch_app/open_file/menu/url/copy）。
    invoke("record_search_hit", {
      source: result.source,
      actionType: result.actionType,
      actionData: result.actionData,
      query: queryRef.current,
    }).catch(() => {});

    // ── slash 命令分流 ──
    // slash 结果的 actionType 是 DB 原始值（url/agent/ai/script），不是 "slash"，
    // 故不能用 switch case "slash"，改在 switch 前按 source === "slash" 分流。
    // action_data 形如 {id, cmd, params, action_type, action_data, title}（见 menu.rs:153）。
    //
    // v2 Task 2：补全锁定后执行用锁定 id（slashLockedItemIdRef），参数从 query 实时解析
    // （用户补全后可能改了参数，data.params 是补全时的旧值）。未锁定时回退 data.id + data.params。
    if (result.source === "slash") {
      const lockedId = slashLockedItemIdRef.current;
      const itemId = lockedId ?? (data.id as number);
      // 参数：锁定时从 query 空格后解析（`/标题 params` 或 `、标题 params`）；
      // 未锁定（直接 Enter 选中候选）用 action_data.params。
      let params: string;
      if (lockedId !== null) {
        const locked = menuItemsRef.current.find((i) => i.id === lockedId);
        const title = locked?.title;
        if (typeof title === "string") {
          // query 形如 `/标题 params`——去掉前缀（`/` 或 `、`）+ title，剩 trim 即参数
          // ⚠️ 必须用 queryRef.current 而非闭包 query——keydown handler 空依赖，
          // 闭包 query 恒为 mount 时的初始值，键盘 Enter 执行时参数会丢失。
          const q = queryRef.current;
          let afterTitle = "";
          if (q.startsWith("/" + title)) afterTitle = q.slice(1 + title.length);
          else if (q.startsWith("、" + title)) afterTitle = q.slice("、".length + title.length);
          params = afterTitle.trim();
        } else {
          // 锁定项已删除（菜单改了）→ 回退 data.params 兜底
          params = (data.params as string) || "";
        }
      } else {
        params = (data.params as string) || "";
      }
      const actionType = (data.action_type as string) || result.actionType;
      const item = menuItemsRef.current.find((i) => i.id === itemId);
      if (!item) {
        console.warn("[slash] 菜单项未找到:", itemId);
        return;
      }
      // url 类型：params 替换 {query}/{text}，无 params 用选中文本
      if (actionType === "url") {
        const ctx = contextRef.current;
        const fallbackText = params || ctx?.text || "";
        const rawUrl = (data.action_data as string) || item.actionData || "";
        await openUrlTemplate(rawUrl, fallbackText, "slash-url");
        return;
      }
      // agent need_voice + 无参数 → 联动语音录音路径（与 executeItem 一致）
      if (actionType === "agent" && item.needVoice && !params) {
        setView("loading");
        try {
          await invoke("trigger_agent_voice", { itemId });
        } catch (e) {
          showQuickError(String(e).slice(0, 40));
          setView("main");
        }
        return;
      }
      // 其他（agent/ai/script + 有参数，或 agent 非 need_voice）→ execute_action_bar
      // text 用 slash params，回退选中文本（execute_action_bar_inner 据 action_type 分流）
      const ctx = contextRef.current;
      const text = params || ctx?.text || "";
      setView("loading");
      try {
        await invoke("execute_action_bar", { itemId, text });
        // ai/script 异步结果由后端收口（action_bar_show_result 隐藏浮窗）；
        // url/agent-without-voice 已在上面分流，此处多为 script/ai，同步 dismiss 兜底
        invoke("action_bar_dismiss", { reason: "slash-exec" });
      } catch (e) {
        showQuickError(String(e).replace(/^脚本执行失败:\s*/, "").slice(0, 40));
        setView("main");
      }
      return;
    }

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
        await openUrlTemplate(rawUrl, fallbackText, "open-url");
        break;
      }
      case "shell": {
        // shell provider 已移除（launcher 场景下无终端上下文/无输出展示，伪需求）
        // 保留 case 防御性兜底——若历史频次记录里残留 shell 结果触发，静默忽略
        break;
      }
      case "copy": {
        // 搜索结果 copy 动作（calculator 计算结果、command 命令助手复制命令名）：
        // actionData = {"text": "..."}——纯前端 clipboard.writeText，不走后端命令。
        // 注意：这与「Settings UI 的 copy 菜单类型」（已删除）是两回事——后者走 executeItem
        // → 后端 execute_action_bar，本 case 仅服务于搜索结果的运行时 actionType="copy"。
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
      case "copy_and_reveal": {
        // command 查阅：复制命令名 + 在文件管理器中定位命令文件
        const text = data.text as string;
        const path = data.path as string;
        if (!text) return;
        try {
          await navigator.clipboard.writeText(text);
          if (path) {
            await invoke("reveal_path", { path });
          }
          invoke("action_bar_dismiss", { reason: "copy_and_reveal" });
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
  // slash 补全锁定的菜单 id ref——executeSearchResult 内异步读取最新值，
  // 避免闭包陈旧（executeSearchResult 由 keyboard handler 调用，需拿实时锁定态）
  const slashLockedItemIdRef = useRef<number | null>(null);
  useEffect(() => { selectedIdxRef.current = selectedIdx; }, [selectedIdx]);
  useEffect(() => { subSelectedIdxRef.current = subSelectedIdx; }, [subSelectedIdx]);
  useEffect(() => { mainItemsRef.current = mainItems; }, [mainItems]);
  useEffect(() => { menuItemsRef.current = menuItems; }, [menuItems]);
  useEffect(() => { queryRef.current = query; }, [query]);
  useEffect(() => { activeTabRef.current = activeTab; }, [activeTab]);
  useEffect(() => { searchSelectedIdxRef.current = searchSelectedIdx; }, [searchSelectedIdx]);
  useEffect(() => { slashLockedItemIdRef.current = slashLockedItemId; }, [slashLockedItemId]);
  useEffect(() => {
    subItemsRef.current = submenuParentIdRef.current !== null
      ? getSubItems(submenuParentIdRef.current)
      : [];
  });

  useActionBarKeydown({
    queryRef, viewRef, focusLayerRef, contextRef,
    selectedIdxRef, subSelectedIdxRef, searchSelectedIdxRef,
    activeTabRef, mainItemsRef, subItemsRef, menuItemsRef,
    searchEngineRef, filteredResultsRef, inputRef, lastImeKeyTime,
    submenuParentIdRef,
    setQuery, setActiveTab, setSearchSelectedIdx,
    setSelectedIdx, setSubSelectedIdx, setView, setFocusLayer,
    setSlashLockedItemId,
    executeItem, executeSearchResult,
  });

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

  // 搜索输入框组件——高度 = INPUT_HEIGHT（44px），字号 15px，视觉重心。
  // 右侧加内置「密码生成器」按钮（独立于 DB 菜单项），点击 → invoke → 后端开浮窗。
  const handleOpenPasswordGenerator = useCallback(async () => {
    try {
      await invoke("open_password_generator");
      // 关闭 ActionBar 浮窗，让生成器浮窗替代它（生成器浮窗会显示在鼠标附近）
      await getCurrentWindow().hide();
    } catch (e) {
      console.error("open_password_generator failed:", e);
    }
  }, []);

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
        autoCapitalize="off"
        autoCorrect="off"
        spellCheck={false}
        value={query}
        onChange={(e) => setQuery(e.target.value)}
        placeholder={t("actionbar.searchPlaceholder")}
        className="flex-1 bg-transparent text-[15px] font-medium text-foreground placeholder:text-muted-foreground/40 placeholder:font-normal outline-none border-none min-w-0"
        autoComplete="off"
      />
      {/* 内置按钮：密码生成器（独立于 DB items）*/}
      <button
        type="button"
        onClick={handleOpenPasswordGenerator}
        title={t("settings.vault.generator.title")}
        className="flex items-center justify-center rounded-md p-1.5 text-muted-foreground transition-colors hover:bg-accent hover:text-foreground shrink-0"
      >
        <img
          src="/icons/generate-key.svg"
          alt={t("settings.vault.generator.title")}
          className="size-[18px]"
          draggable={false}
        />
      </button>
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
