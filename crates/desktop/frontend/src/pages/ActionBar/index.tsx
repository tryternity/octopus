import { useState, useEffect, useRef } from "react";
import { invoke } from "@/lib/tauri";
import { listen as rawListen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { cn } from "@/lib/utils";
import { Sparkles, Globe, Search, Link as LinkIcon, FileText, Lightbulb, Pencil, Loader2 } from "lucide-react";
import { detectActionUrl } from "./urlDetect";

interface Context {
  text: string;
}

type View = "main" | "submenu" | "loading" | "error";
type SubmenuType = "ai" | "search" | null;

const SEARCH_URLS: Record<string, string> = {
  google: "https://www.google.com/search?q=",
  baidu: "https://www.baidu.com/s?wd=",
  bing: "https://www.bing.com/search?q=",
};

const AI_TRANSLATE_TIMEOUT_MS = 5000;
const AI_TIMEOUT_MS = 10000;

// 定义在组件外部——避免每次渲染创建新组件类型导致 unmount/remount
const IconBtn = ({ icon: Icon, label, active, onClick }: {
  icon: React.ElementType; label: string; active: boolean; onClick: () => void;
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
    <Icon className="w-4 h-4" />
    <span className="text-[9px]">{label}</span>
  </button>
);

export default function ActionBar() {
  const [context, setContext] = useState<Context | null>(null);
  const [view, setView] = useState<View>("main");
  const [selectedIdx, setSelectedIdx] = useState(0);
  const [subSelectedIdx, setSubSelectedIdx] = useState(0);
  const [submenuType, setSubmenuType] = useState<SubmenuType>(null);
  const [errorMsg, setErrorMsg] = useState("");
  const searchEngineRef = useRef("google");
  const contextRef = useRef<Context | null>(null);
  const viewRef = useRef<View>("main");

  useEffect(() => { viewRef.current = view; }, [view]);
  useEffect(() => { contextRef.current = context; }, [context]);

  // mount + 每次 show 时拉取上下文
  useEffect(() => {
    const refresh = () => {
      invoke<Context | null>("action_bar_get_context").then((ctx) => {
        if (ctx) { setContext(ctx); setView("main"); setSelectedIdx(0); setErrorMsg(""); }
      });
    };
    refresh();
    const listenPromise = rawListen("action-bar://show", () => refresh());

    invoke<{ config: Record<string, string | number | boolean> }>("get_config").then((resp) => {
      searchEngineRef.current = (resp.config.action_bar_search_engine as string) || "google";
    });

    return () => { listenPromise.then((fn: () => void) => fn()); };
  }, []);

  // 点击外部消失 + 窗口失焦消失（loading 状态时不禁用——浮窗必须保持可见）
  useEffect(() => {
    const onClick = (e: MouseEvent) => {
      if (viewRef.current === "loading") return; // loading 中不可关闭
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

  const mainItems = [
    { id: "ai", icon: Sparkles, label: "AI" },
    { id: "translate", icon: Globe, label: "翻译" },
    { id: "search", icon: Search, label: "搜索" },
    ...(urlResult.isUrl ? [{ id: "url", icon: LinkIcon, label: "网页" }] : []),
  ];

  const aiItems = [
    { id: "polish", icon: Pencil, label: "润色" },
    { id: "summarize", icon: FileText, label: "摘要" },
    { id: "explain", icon: Lightbulb, label: "解释" },
  ];

  const searchItems = [
    { id: "google", icon: Search, label: "Google" },
    { id: "baidu", icon: Search, label: "百度" },
    { id: "bing", icon: Search, label: "Bing" },
  ];

  // ── 动作执行（不用 useCallback——闭包捕获 ref 已是最新的）──

  const executeAiAction = async (action: string) => {
    const ctx = contextRef.current;
    console.log("[action-bar] executeAiAction:", action, "context:", !!ctx);
    if (!ctx) return;
    setView("loading");

    // 翻译 5 秒超时；其他 AI 动作（润色/摘要/解释）10 秒超时
    const timeoutMs = action === "translate" ? AI_TRANSLATE_TIMEOUT_MS : AI_TIMEOUT_MS;
    const timeoutId = setTimeout(() => {
      setErrorMsg(`请求超时（${timeoutMs / 1000} 秒），请检查网络或 LLM 配置`);
      setView("error");
    }, timeoutMs);

    try {
      console.log("[action-bar] invoking run_ai_action:", action);
      const result = await invoke<string>("run_ai_action", { action, text: ctx.text });
      clearTimeout(timeoutId);
      console.log("[action-bar] AI result len:", result.length);
      await invoke("action_bar_show_result", { result, originalText: ctx.text, action });
      getCurrentWindow().hide();
    } catch (e) {
      clearTimeout(timeoutId);
      console.error("[action-bar] AI error:", e);
      setErrorMsg(String(e));
      setView("error");
    }
  };

  const executeMain = (id: string) => {
    console.log("[action-bar] executeMain:", id);
    const ctx = contextRef.current;
    if (id === "ai") {
      setSubmenuType("ai");
      setView("submenu");
      setSubSelectedIdx(0);
    } else if (id === "search") {
      setSubmenuType("search");
      setView("submenu");
      setSubSelectedIdx(0);
    } else if (id === "translate") {
      executeAiAction("translate");
    } else if (id === "url") {
      const url = detectActionUrl(ctx?.text || "").url;
      invoke("action_bar_open_url", { url });
    }
  };

  // 子菜单项执行
  const executeSubItem = (id: string) => {
    if (submenuType === "ai") {
      executeAiAction(id);
    } else if (submenuType === "search") {
      const ctx = contextRef.current;
      if (!ctx) return;
      const baseUrl = SEARCH_URLS[id] || SEARCH_URLS.google;
      invoke("action_bar_open_url", { url: baseUrl + encodeURIComponent(ctx.text) });
    }
  };

  // ── 键盘导航（用 ref 读最新状态，handler 只注册一次）──

  const selectedIdxRef = useRef(0);
  const subSelectedIdxRef = useRef(0);
  const mainItemsRef = useRef(mainItems);
  const aiItemsRef = useRef(aiItems);
  useEffect(() => { selectedIdxRef.current = selectedIdx; }, [selectedIdx]);
  useEffect(() => { subSelectedIdxRef.current = subSelectedIdx; }, [subSelectedIdx]);
  useEffect(() => { mainItemsRef.current = mainItems; }, [mainItems]);
  useEffect(() => { aiItemsRef.current = submenuType === "search" ? searchItems : aiItems; }, [aiItems, searchItems, submenuType]);

  useEffect(() => {
    const handler = (e: KeyboardEvent) => {
      console.log("[action-bar] keydown:", e.key, "view:", viewRef.current);
      if (e.key === "Escape") {
        e.preventDefault();
        if (viewRef.current === "submenu") { setView("main"); return; }
        invoke("action_bar_dismiss");
        return;
      }

      if (viewRef.current === "loading" || viewRef.current === "error") return;

      // Cmd/Ctrl+数字
      if (e.metaKey || e.ctrlKey) {
        const n = parseInt(e.key, 10);
        if (!isNaN(n) && n >= 1) {
          e.preventDefault();
          if (viewRef.current === "main" && n <= mainItemsRef.current.length) {
            executeMain(mainItemsRef.current[n - 1].id);
          } else if (viewRef.current === "submenu" && n <= aiItemsRef.current.length) {
            executeSubItem(aiItemsRef.current[n - 1].id);
          }
          return;
        }
      }

      // ←→：当前行内移动（主菜单或子菜单）
      if (e.key === "ArrowLeft" || e.key === "ArrowRight") {
        e.preventDefault();
        if (viewRef.current === "submenu") {
          setSubSelectedIdx((prev) => {
            const items = aiItemsRef.current;
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

      // ↑↓：主菜单 ↔ 子菜单循环切换
      if (e.key === "ArrowUp" || e.key === "ArrowDown") {
        e.preventDefault();
        if (viewRef.current === "submenu") {
          // 在子菜单：↑ 或 ↓ 都回到主菜单
          setView("main");
          setSubmenuType(null);
        } else {
          // 在主菜单：↑ 或 ↓ 进入子菜单（仅 AI/搜索有子菜单）
          const cur = mainItemsRef.current[selectedIdxRef.current];
          if (cur && (cur.id === "ai" || cur.id === "search")) {
            setSubmenuType(cur.id as SubmenuType);
            setView("submenu");
            setSubSelectedIdx(0);
          }
        }
        return;
      }

      // Enter / Space
      if (e.key === "Enter" || e.key === " ") {
        e.preventDefault();
        console.log("[action-bar] Enter pressed, view:", viewRef.current, "selectedIdx:", selectedIdxRef.current, "subIdx:", subSelectedIdxRef.current);
        if (viewRef.current === "main") {
          executeMain(mainItemsRef.current[selectedIdxRef.current].id);
        } else if (viewRef.current === "submenu") {
          executeSubItem(aiItemsRef.current[subSelectedIdxRef.current].id);
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
        <span className="text-[10px] text-muted-foreground">结果已复制到剪贴板</span>
        <button
          className="text-[10px] text-muted-foreground hover:text-foreground mt-0.5"
          onMouseDown={(e) => e.stopPropagation()}
          onClick={() => getCurrentWindow().hide()}
        >关闭</button>
      </div>
    );
  }

  return (
    <div data-action-bar className="flex flex-col rounded-lg border border-border shadow-lg overflow-hidden bg-background">
      {/* 主菜单在上——位置固定 */}
      <div className="flex items-center gap-0.5 px-1 h-[38px] shrink-0">
        {mainItems.map((item, i) => (
          <IconBtn
            key={item.id}
            icon={item.icon}
            label={item.label}
            active={view === "main" ? selectedIdx === i : false}
            onClick={() => executeMain(item.id)}
          />
        ))}
      </div>
      {/* 子菜单在下——固定占位，无内容时高度 0（用 overflow hidden + h-0 隐藏） */}
      <div className={cn(
        "flex items-center gap-0.5 px-1 h-[38px] shrink-0 overflow-hidden border-t border-border/40",
        view === "submenu" ? "" : "h-0 border-t-0 pt-0 pb-0",
      )}>
        {(submenuType === "search" ? searchItems : aiItems).map((item, i) => (
          <IconBtn
            key={item.id}
            icon={item.icon}
            label={item.label}
            active={subSelectedIdx === i}
            onClick={() => executeSubItem(item.id)}
          />
        ))}
      </div>
    </div>
  );
}
