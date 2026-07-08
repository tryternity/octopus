import { useState, useEffect, useRef, useCallback } from "react";
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

const SEARCH_URLS: Record<string, string> = {
  google: "https://www.google.com/search?q=",
  baidu: "https://www.baidu.com/s?wd=",
  bing: "https://www.bing.com/search?q=",
};

// 定义在组件外部——避免每次渲染创建新组件类型导致 unmount/remount
const IconBtn = ({ icon: Icon, label, active, onClick }: {
  icon: React.ElementType; label: string; active: boolean; onClick: () => void;
}) => (
  <button
    className={cn(
      "flex flex-col items-center justify-center gap-0.5 px-3 py-1.5 rounded-md transition-all",
      active ? "bg-accent text-accent-foreground" : "text-muted-foreground hover:bg-accent hover:text-foreground",
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

  // 点击外部消失
  useEffect(() => {
    const onDown = (e: MouseEvent) => {
      const el = e.target as HTMLElement;
      if (el && el.closest("[data-action-bar]")) return;
      getCurrentWindow().hide();
    };
    const timer = setTimeout(() => {
      document.addEventListener("mousedown", onDown, true);
    }, 200);
    return () => { clearTimeout(timer); document.removeEventListener("mousedown", onDown, true); };
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

  // ── 动作执行 ──

  const executeAiAction = useCallback(async (action: string) => {
    const ctx = contextRef.current;
    if (!ctx) return;
    setView("loading");
    try {
      const result = await invoke<string>("run_ai_action", { action, text: ctx.text });
      await invoke("action_bar_paste_result", { result });
      getCurrentWindow().hide();
    } catch (e) {
      setErrorMsg(String(e));
      setView("error");
    }
  }, []);

  const executeMain = useCallback((id: string) => {
    const ctx = contextRef.current;
    if (id === "ai") {
      setView("submenu");
      setSubSelectedIdx(0);
    } else if (id === "translate") {
      executeAiAction("translate");
    } else if (id === "search") {
      if (!ctx) return;
      const baseUrl = SEARCH_URLS[searchEngineRef.current] || SEARCH_URLS.google;
      invoke("action_bar_open_url", { url: baseUrl + encodeURIComponent(ctx.text) });
    } else if (id === "url") {
      const url = detectActionUrl(ctx?.text || "").url;
      invoke("action_bar_open_url", { url });
    }
  }, [executeAiAction]);

  // ── 键盘导航（用 ref 读最新状态，handler 只注册一次）──

  const selectedIdxRef = useRef(0);
  const subSelectedIdxRef = useRef(0);
  const mainItemsRef = useRef(mainItems);
  const aiItemsRef = useRef(aiItems);
  useEffect(() => { selectedIdxRef.current = selectedIdx; }, [selectedIdx]);
  useEffect(() => { subSelectedIdxRef.current = subSelectedIdx; }, [subSelectedIdx]);
  useEffect(() => { mainItemsRef.current = mainItems; }, [mainItems]);
  useEffect(() => { aiItemsRef.current = aiItems; }, [aiItems]);

  useEffect(() => {
    const handler = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        e.preventDefault();
        if (viewRef.current === "submenu") { setView("main"); return; }
        getCurrentWindow().hide();
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
            executeAiAction(aiItemsRef.current[n - 1].id);
          }
          return;
        }
      }

      // ←→
      if (e.key === "ArrowLeft" || e.key === "ArrowRight") {
        e.preventDefault();
        const items = viewRef.current === "submenu" ? aiItemsRef.current : mainItemsRef.current;
        setSelectedIdx((prev) => {
          const next = e.key === "ArrowRight" ? (prev + 1) % items.length : (prev - 1 + items.length) % items.length;
          return next;
        });
        return;
      }

      // ↑↓
      if (viewRef.current === "submenu" && (e.key === "ArrowUp" || e.key === "ArrowDown")) {
        e.preventDefault();
        setSubSelectedIdx((prev) => {
          const next = e.key === "ArrowDown" ? (prev + 1) % aiItemsRef.current.length : (prev - 1 + aiItemsRef.current.length) % aiItemsRef.current.length;
          return next;
        });
        return;
      }

      // Enter / Space
      if (e.key === "Enter" || e.key === " ") {
        e.preventDefault();
        if (viewRef.current === "main") {
          executeMain(mainItemsRef.current[selectedIdxRef.current].id);
        } else if (viewRef.current === "submenu") {
          executeAiAction(aiItemsRef.current[subSelectedIdxRef.current].id);
        }
        return;
      }
    };
    window.addEventListener("keydown", handler);
    return () => window.removeEventListener("keydown", handler);
  }, [executeMain, executeAiAction]);

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
    <div data-action-bar className="flex flex-col bg-background text-foreground rounded-lg border border-border shadow-lg overflow-hidden">
      <div className="flex items-center gap-0.5 px-1 py-1">
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

      {view === "submenu" && (
        <div className="flex items-center gap-0.5 px-1 pb-1 border-t border-border/40 pt-1">
          {aiItems.map((item, i) => (
            <IconBtn
              key={item.id}
              icon={item.icon}
              label={item.label}
              active={subSelectedIdx === i}
              onClick={() => executeAiAction(item.id)}
            />
          ))}
        </div>
      )}
    </div>
  );
}
