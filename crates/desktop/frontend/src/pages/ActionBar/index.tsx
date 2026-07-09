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
      "flex flex-col items-center justify-center gap-0.5 px-3 py-1.5 rounded-md transition-all",
      active
        ? "bg-voice/15 text-voice ring-1 ring-voice/30"
        : "text-muted-foreground hover:bg-muted hover:text-foreground",
    )}
    onMouseDown={(e) => e.stopPropagation()}
    onClick={onClick}
    title={label}
  >
    <ActionBarIcon icon={icon} className="w-4 h-4" />
    <span className="text-[9px]">{label}</span>
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

  useEffect(() => { viewRef.current = view; }, [view]);
  useEffect(() => { contextRef.current = context; }, [context]);

  // mount + 每次 show 时拉取上下文 + 菜单
  useEffect(() => {
    const refresh = () => {
      invoke<Context | null>("action_bar_get_context").then((ctx) => {
        if (ctx) { setContext(ctx); setView("main"); setSelectedIdx(0); setErrorMsg(""); }
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
  useEffect(() => { selectedIdxRef.current = selectedIdx; }, [selectedIdx]);
  useEffect(() => { subSelectedIdxRef.current = subSelectedIdx; }, [subSelectedIdx]);
  useEffect(() => { mainItemsRef.current = mainItems; }, [mainItems]);
  useEffect(() => {
    subItemsRef.current = submenuParentIdRef.current !== null
      ? getSubItems(submenuParentIdRef.current)
      : [];
  });

  useEffect(() => {
    const handler = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        e.preventDefault();
        if (viewRef.current === "submenu") { setView("main"); return; }
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
        if (viewRef.current === "submenu") {
          setSubSelectedIdx((prev) => {
            const items = subItemsRef.current;
            return e.key === "ArrowRight" ? (prev + 1) % items.length : (prev - 1 + items.length) % items.length;
          });
        } else {
          setSelectedIdx((prev) => {
            const items = mainItemsRef.current;
            return e.key === "ArrowRight" ? (prev + 1) % items.length : (prev - 1 + items.length) % items.length;
          });
        }
        return;
      }

      if (e.key === "ArrowUp" || e.key === "ArrowDown") {
        e.preventDefault();
        if (viewRef.current === "submenu") {
          setView("main");
          submenuParentIdRef.current = null;
        } else {
          const cur = mainItemsRef.current[selectedIdxRef.current];
          if (cur && cur.actionType === "submenu") {
            submenuParentIdRef.current = cur.id;
            setView("submenu");
            setSubSelectedIdx(0);
          }
        }
        return;
      }

      if (e.key === "Enter" || e.key === " ") {
        e.preventDefault();
        if (viewRef.current === "main") {
          const item = mainItemsRef.current[selectedIdxRef.current];
          if (item) executeItem(item);
        } else if (viewRef.current === "submenu") {
          const items = subItemsRef.current;
          const item = items[subSelectedIdxRef.current];
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
      <div data-action-bar className="flex items-center justify-center gap-2 px-4 py-2 bg-background text-foreground rounded-lg border border-border shadow-lg">
        <Loader2 className="w-3.5 h-3.5 animate-spin text-voice" />
        <span className="text-[11px]">处理中…</span>
      </div>
    );
  }

  if (view === "error") {
    return (
      <div data-action-bar className="flex flex-col gap-1 px-4 py-2 bg-background text-foreground rounded-lg border border-border shadow-lg max-w-[240px]">
        <span className="text-[11px] text-red-500">{errorMsg}</span>
        <button
          className="text-[10px] text-muted-foreground hover:text-foreground mt-0.5"
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
    <div data-action-bar className="flex flex-col rounded-lg border border-border shadow-lg overflow-hidden bg-background">
      {/* 主菜单在上 */}
      <div className="flex items-center gap-0.5 px-1 h-[38px] shrink-0">
        {mainItems.map((item, i) => (
          <IconBtn
            key={item.id}
            icon={item.icon}
            label={item.title}
            active={view === "main" ? selectedIdx === i : false}
            onClick={() => executeItem(item)}
          />
        ))}
      </div>
      {/* 子菜单在下——固定占位，无内容时高度 0 */}
      <div className={cn(
        "flex items-center gap-0.5 px-1 h-[38px] shrink-0 overflow-hidden border-t border-border/40",
        view === "submenu" ? "" : "h-0 border-t-0 pt-0 pb-0",
      )}>
        {subItems.map((item, i) => (
          <IconBtn
            key={item.id}
            icon={item.icon}
            label={item.title}
            active={subSelectedIdx === i}
            onClick={() => executeItem(item)}
          />
        ))}
      </div>
    </div>
  );
}
