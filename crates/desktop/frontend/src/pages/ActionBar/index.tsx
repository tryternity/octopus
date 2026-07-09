import { useState, useEffect, useRef } from "react";
import { invoke } from "@/lib/tauri";
import { listen as rawListen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { cn } from "@/lib/utils";
import { Loader2 } from "lucide-react";
import { detectActionUrl } from "./urlDetect";
import { ActionBarIcon } from "@/components/ActionBarIcon";

interface Context {
  text: string;
}

type View = "main" | "submenu" | "loading" | "error";

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

const IconBtn = ({ icon, label, active, onClick }: {
  icon: string; label: string; active: boolean; onClick: () => void;
}) => (
  <button
    className={cn(
      "flex items-center gap-1.5 px-2.5 py-1.5 rounded-lg transition-all duration-150",
      active
        ? "bg-voice/12 text-voice"
        : "text-muted-foreground hover:bg-foreground/5 hover:text-foreground",
    )}
    onMouseDown={(e) => e.stopPropagation()}
    onClick={onClick}
    title={label}
  >
    <ActionBarIcon icon={icon} className="text-[14px]" />
    <span className="text-[10px] font-medium leading-none whitespace-nowrap">{label}</span>
  </button>
);

export default function ActionBar() {
  const [context, setContext] = useState<Context | null>(null);
  const [view, setView] = useState<View>("main");
  const [selectedIdx, setSelectedIdx] = useState(0);
  const [subSelectedIdx, setSubSelectedIdx] = useState(0);
  const [menuItems, setMenuItems] = useState<ActionBarItem[]>([]);
  const [errorMsg, setErrorMsg] = useState("");
  const searchEngineRef = useRef("google");
  const timedOutRef = useRef(false);
  const contextRef = useRef<Context | null>(null);
  const viewRef = useRef<View>("main");
  const submenuParentIdRef = useRef<number | null>(null);
  const [focusLayer, setFocusLayer] = useState<"main" | "sub">("main");
  const focusLayerRef = useRef<"main" | "sub">("main");

  useEffect(() => { viewRef.current = view; }, [view]);
  useEffect(() => { focusLayerRef.current = focusLayer; }, [focusLayer]);
  useEffect(() => { contextRef.current = context; }, [context]);

  // mount + 每次 show 时拉取上下文 + 菜单
  useEffect(() => {
    const refresh = () => {
      invoke<Context | null>("action_bar_get_context").then((ctx) => {
        if (ctx) { setContext(ctx); setView("main"); setSelectedIdx(0); setFocusLayer("main"); setErrorMsg(""); }
      });
    };
    refresh();
    const listenPromise = rawListen("action-bar://show", () => refresh());

    // 加载菜单项
    invoke<ActionBarItem[]>("list_action_bar_items").then((items) => {
      setMenuItems(items);
    });

    invoke<{ config: Record<string, string | number | boolean> }>("get_config").then((resp) => {
      searchEngineRef.current = (resp.config.action_bar_search_engine as string) || "google";
    });

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
      setErrorMsg(`请求超时（${timeoutMs / 1000} 秒），请检查网络或 LLM 配置`);
      setView("error");
    }, timeoutMs);

    try {
      await invoke("execute_action_bar", { itemId: item.id, text: ctx.text });
      clearTimeout(timeoutId);
      if (timedOutRef.current) {
        console.warn("[action-bar] AI result arrived after timeout, discarding");
        return;
      }
      getCurrentWindow().hide();
    } catch (e) {
      clearTimeout(timeoutId);
      if (timedOutRef.current) return;
      setErrorMsg(String(e));
      setView("error");
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

    // url / script / copy → 直接 invoke
    await invoke("execute_action_bar", { itemId: item.id, text: ctx.text });
    getCurrentWindow().hide();
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

      if (viewRef.current === "loading" || viewRef.current === "error") return;

      if (e.metaKey || e.ctrlKey) {
        const n = parseInt(e.key, 10);
        if (!isNaN(n) && n >= 1) {
          e.preventDefault();
          if (viewRef.current === "main" && n <= mainItemsRef.current.length) {
            executeItem(mainItemsRef.current[n - 1]);
          } else if (viewRef.current === "submenu" && n <= subItemsRef.current.length) {
            executeItem(subItemsRef.current[n - 1]);
          }
          return;
        }
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
      <div data-action-bar className="flex items-center justify-center gap-2.5 px-6 py-3 bg-background/95 backdrop-blur-xl text-foreground rounded-2xl border border-border/50 shadow-2xl shadow-black/10">
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

  if (view === "error") {
    return (
      <div data-action-bar className="flex flex-col gap-1.5 px-4 py-3 bg-background/95 backdrop-blur-xl text-foreground rounded-2xl border border-red-500/30 shadow-2xl shadow-black/10 max-w-[260px]">
        <span className="text-[12px] text-red-500 font-medium leading-snug">{errorMsg}</span>
        <button
          className="text-[11px] text-muted-foreground hover:text-foreground transition-colors w-fit"
          onMouseDown={(e) => e.stopPropagation()}
          onClick={() => getCurrentWindow().hide()}
        >关闭</button>
      </div>
    );
  }

  const subItems = submenuParentIdRef.current !== null
    ? getSubItems(submenuParentIdRef.current)
    : [];

  return (
    <div
      data-action-bar
      className="flex flex-col rounded-2xl border border-border/50 shadow-2xl shadow-black/10 overflow-hidden bg-background/95 backdrop-blur-xl"
    >
      {/* 主菜单 */}
      <div className="flex items-center gap-1 px-1.5 py-1.5 shrink-0">
        {mainItems.map((item, i) => (
          <IconBtn
            key={item.id}
            icon={item.icon}
            label={item.title}
            active={selectedIdx === i}
            onClick={() => executeItem(item)}
          />
        ))}
      </div>
      {/* 子菜单——展开时用渐变分隔线 + 轻微底色区分 */}
      <div className={cn(
        "flex items-center gap-1 px-1.5 py-1.5 shrink-0 overflow-hidden transition-all duration-200",
        view === "submenu"
          ? "border-t border-border/30 bg-foreground/[0.02]"
          : "h-0 py-0 overflow-hidden border-t-0",
      )}>
        {subItems.map((item, i) => (
          <IconBtn
            key={item.id}
            icon={item.icon}
            label={item.title}
            active={focusLayer === "sub" && subSelectedIdx === i}
            onClick={() => executeItem(item)}
          />
        ))}
      </div>
    </div>
  );
}
