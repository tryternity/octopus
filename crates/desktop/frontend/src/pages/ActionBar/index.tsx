import { useState, useEffect, useRef, useLayoutEffect } from "react";
import { invoke } from "@/lib/tauri";
import { listen as rawListen } from "@tauri-apps/api/event";
import { getCurrentWindow, LogicalSize } from "@tauri-apps/api/window";
import { cn } from "@/lib/utils";
import { Loader2, ChevronLeft, ChevronRight } from "lucide-react";
import { detectActionUrl } from "./urlDetect";

interface Context {
  text: string;
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
}

const AI_TRANSLATE_TIMEOUT_MS = 5000;
const AI_TIMEOUT_MS = 10000;

const IconBtn = ({ index, label, active, onClick, btnRef }: {
  index: number; label: string; active: boolean; onClick: () => void;
  btnRef?: (el: HTMLButtonElement | null) => void;
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
    title={label}
  >
    <span
      className={cn(
        "inline-flex h-[18px] w-[18px] items-center justify-center rounded-md font-mono text-[11px] font-semibold tabular-nums leading-none",
        active
          ? "bg-voice text-white"
          : "bg-muted text-muted-foreground",
      )}
    >
      {index}
    </span>
    <span className="text-[10px] font-medium leading-none whitespace-nowrap">{label}</span>
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

  // 动态调整窗口高度——主菜单 1 行（~40px），子菜单 2 行（~76px），
  // 避免透明区域遮挡下层点击
  useEffect(() => {
    const height = view === "submenu" ? 78 : view === "loading" ? 48 : 40;
    const win = getCurrentWindow();
    win.setSize(new LogicalSize(380, height)).catch(() => {});
  }, [view]);

  // mount + 每次 show 时拉取上下文 + 菜单 + 配置
  useEffect(() => {
    const refresh = () => {
      // 前端获取键盘焦点（后端 show 不调 set_focus 避免激活 app），
      // 让方向键直接可用——用户无需鼠标点击
      window.focus();
      invoke<Context | null>("action_bar_get_context").then((ctx) => {
        // 每次 show 都重置基础状态——防止遗留旧状态
        setView("main"); setSelectedIdx(0); setFocusLayer("main");
        if (ctx) { setContext(ctx); }
      });
      // 每次唤起都重新加载菜单项 + 配置（设置页可能已改）
      invoke<ActionBarItem[]>("list_action_bar_items").then((items) => {
        setMenuItems(items);
      });
      invoke<{ config: Record<string, string | number | boolean> }>("get_config").then((resp) => {
        searchEngineRef.current = (resp.config.action_bar_search_engine as string) || "google";
      });
    };
    refresh();
    const listenPromise = rawListen("action-bar://show", () => refresh());

    return () => { listenPromise.then((fn: () => void) => fn()); };
  }, []);

  // 点击外部消失 + 窗口失焦消失
  useEffect(() => {
    const onClick = (e: MouseEvent) => {
      if (viewRef.current === "loading") return;
      const el = e.target as HTMLElement;
      if (el && el.closest("[data-action-bar]")) return;
      invoke("action_bar_dismiss");
    };
    const unlistenFocus = getCurrentWindow().onFocusChanged(({ payload: focused }) => {
      if (!focused && viewRef.current !== "loading") invoke("action_bar_dismiss");
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

  const urlResult = context ? detectActionUrl(context.text) : { isUrl: false, url: "" };

  // 派生菜单项
  const allMainItems = menuItems.filter((i) => i.parentId === null);
  const mainItems = allMainItems.filter((i) => {
    // 网页项仅当选中文本是 URL 时显示
    if (i.actionType === "url" && i.actionData === "") return urlResult.isUrl;
    return true;
  });
  const getSubItems = (parentId: number) => menuItems.filter((i) => i.parentId === parentId);

  // ── 动作执行 ──

  const executeAiItem = async (item: ActionBarItem) => {
    const ctx = contextRef.current;
    if (!ctx) return;
    setView("loading");
    timedOutRef.current = false;

    const timeoutMs = item.actionData === "auto_translate" ? AI_TRANSLATE_TIMEOUT_MS : AI_TIMEOUT_MS;
    const timeoutId = setTimeout(() => {
      timedOutRef.current = true;
      showQuickError(`请求超时（${timeoutMs / 1000}s）`);
      setView("main");
    }, timeoutMs);

    try {
      await invoke("execute_action_bar", { itemId: item.id, text: ctx.text });
      clearTimeout(timeoutId);
      if (timedOutRef.current) {
        console.warn("[action-bar] AI result arrived after timeout, discarding");
        return;
      }
      // action_bar_show_result 后端已隐藏本窗口并展示 CompactEditor
    } catch (e) {
      clearTimeout(timeoutId);
      if (timedOutRef.current) return;
      showQuickError(String(e).slice(0, 40));
      setView("main");
    }
  };

  const executeItem = async (item: ActionBarItem) => {
    const ctx = contextRef.current;
    if (!ctx) return;

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
      setView("submenu");
      return;
    }

    if (item.actionType === "ai") {
      executeAiItem(item);
      return;
    }

    // url / script / copy → 脚本类错误显示红色气泡提示（1 秒消失），其他类切 error 视图
    try {
      await invoke("execute_action_bar", { itemId: item.id, text: ctx.text });
    } catch (e) {
      showQuickError(String(e).replace(/^脚本执行失败:\s*/, "").slice(0, 40));
    }
  };

  // ── 键盘导航 ──

  const selectedIdxRef = useRef(0);
  const subSelectedIdxRef = useRef(0);
  const mainItemsRef = useRef<ActionBarItem[]>([]);
  const subItemsRef = useRef<ActionBarItem[]>([]);
  const menuItemsRef = useRef<ActionBarItem[]>([]);
  useEffect(() => { selectedIdxRef.current = selectedIdx; }, [selectedIdx]);
  useEffect(() => { subSelectedIdxRef.current = subSelectedIdx; }, [subSelectedIdx]);
  useEffect(() => { mainItemsRef.current = mainItems; }, [mainItems]);
  useEffect(() => { menuItemsRef.current = menuItems; }, [menuItems]);
  useEffect(() => {
    subItemsRef.current = submenuParentIdRef.current !== null
      ? getSubItems(submenuParentIdRef.current)
      : [];
  });

  useEffect(() => {
    const handler = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        e.preventDefault();
        invoke("action_bar_dismiss");
        return;
      }

      if (viewRef.current === "loading") return;

      // 数字键（无修饰）定位当前焦点层第 N 项——只移动高亮，不执行；超出范围无效
      if (/^[1-9]$/.test(e.key)) {
        e.preventDefault();
        const n = parseInt(e.key, 10);
        if (focusLayerRef.current === "sub") {
          if (n <= subItemsRef.current.length) setSubSelectedIdx(n - 1);
        } else {
          if (n <= mainItemsRef.current.length) {
            const item = mainItemsRef.current[n - 1];
            setSelectedIdx(n - 1);
            // submenu 项同步展开子菜单预览（与左右键行为一致）
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
        return;
      }

      if (e.key === "ArrowLeft" || e.key === "ArrowRight") {
        e.preventDefault();
        if (focusLayerRef.current === "sub") {
          // 焦点在子菜单——左右键在子菜单移动
          setSubSelectedIdx((prev) => {
            const items = subItemsRef.current;
            return e.key === "ArrowRight" ? (prev + 1) % items.length : (prev - 1 + items.length) % items.length;
          });
        } else {
          // 焦点在主菜单——左右键在主菜单移动，submenu 项自动展开子菜单预览
          setSelectedIdx((prev) => {
            const items = mainItemsRef.current;
            const next = e.key === "ArrowRight" ? (prev + 1) % items.length : (prev - 1 + items.length) % items.length;
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
        // 上下键只切换焦点层，不展开/收起子菜单
        // 子菜单展开/收起由左右键移动主菜单项时控制
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

  if (view === "loading") {
    return (
      <div data-action-bar className="flex items-center justify-center gap-2.5 px-6 py-3 bg-background/95 backdrop-blur-xl text-foreground rounded-lg border border-border/50 shadow-2xl shadow-black/10">
        <Loader2 className="w-4 h-4 animate-spin text-voice" />
        <span className="text-[12px] font-medium">处理中</span>
        <span className="flex gap-0.5">
          <span className="w-1 h-1 rounded-full bg-voice/40 animate-pulse" style={{ animationDelay: "0ms" }} />
          <span className="w-1 h-1 rounded-full bg-voice/40 animate-pulse" style={{ animationDelay: "150ms" }} />
          <span className="w-1 h-1 rounded-full bg-voice/40 animate-pulse" style={{ animationDelay: "300ms" }} />
        </span>
      </div>
    );
  }

  const subItems = submenuParentIdRef.current !== null
    ? getSubItems(submenuParentIdRef.current)
    : [];

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
      {/* 主菜单 */}
      <ScrollRow>
        {mainItems.map((item, i) => (
          <IconBtn
            key={item.id}
            index={i + 1}
            label={item.title}
            active={selectedIdx === i}
            onClick={() => executeItem(item)}
            btnRef={(el: HTMLButtonElement | null) => { mainBtnRefs.current[i] = el; }}
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
            index={i + 1}
            label={item.title}
            active={focusLayer === "sub" && subSelectedIdx === i}
            onClick={() => executeItem(item)}
            btnRef={(el: HTMLButtonElement | null) => { subBtnRefs.current[i] = el; }}
          />
        ))}
      </ScrollRow>
    </div>
  );
}
