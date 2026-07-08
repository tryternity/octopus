import { useState, useEffect, useRef, useCallback } from "react";
import { invoke } from "@/lib/tauri";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { cn } from "@/lib/utils";
import { Sparkles, Globe, Search, Link as LinkIcon, FileText, Lightbulb, Pencil, Loader2 } from "lucide-react";
import { detectActionUrl } from "./urlDetect";

// ── 类型 ──

interface Context {
  text: string;
}

type View = "main" | "submenu" | "loading" | "error";

// ── 搜索引擎 URL ──

const SEARCH_URLS: Record<string, string> = {
  google: "https://www.google.com/search?q=",
  baidu: "https://www.baidu.com/s?wd=",
  bing: "https://www.bing.com/search?q=",
};

// ── 组件 ──

export default function ActionBar() {
  const [context, setContext] = useState<Context | null>(null);
  const [view, setView] = useState<View>("main");
  const [selectedIdx, setSelectedIdx] = useState(0);
  const [subSelectedIdx, setSubSelectedIdx] = useState(0);
  const [errorMsg, setErrorMsg] = useState("");
  const searchEngineRef = useRef("google");

  // mount + 每次 show 时拉取上下文 + 搜索引擎
  useEffect(() => {
    const refresh = () => {
      invoke<Context | null>("action_bar_get_context").then((ctx) => {
        if (ctx) { setContext(ctx); setView("main"); setSelectedIdx(0); }
      });
    };
    refresh(); // 首次 mount
    // 窗口 show/hide 复用——监听 show 事件重新拉取
    let unlisten: (() => void) | undefined;
    const listenPromise = listen("action-bar://show", () => refresh());
    listenPromise.then((fn) => { /* unlisten 在 cleanup 用 */ });

    invoke<{ config: Record<string, string | number | boolean> }>("get_config").then((resp) => {
      searchEngineRef.current = (resp.config.action_bar_search_engine as string) || "google";
    });

    return () => { listenPromise.then((fn) => fn()); };
  }, []);

  // 点击外部消失（不用 onFocusChanged——会在点击按钮时误触发）
  useEffect(() => {
    const onDown = (e: MouseEvent) => {
      const el = e.target as HTMLElement;
      // 点击浮窗内部不消失
      if (el && el.closest("[data-action-bar]")) return;
      getCurrentWindow().hide();
    };
    // 延迟注册——避免 show 后首个鼠标事件就关掉
    const timer = setTimeout(() => {
      document.addEventListener("mousedown", onDown, true);
    }, 100);
    return () => { clearTimeout(timer); document.removeEventListener("mousedown", onDown, true); };
  }, []);

  // URL 检测
  const urlResult = context ? detectActionUrl(context.text) : { isUrl: false, url: "" };

  // 第一级菜单项
  const mainItems = [
    { id: "ai", icon: Sparkles, label: "AI" },
    { id: "translate", icon: Globe, label: "翻译" },
    { id: "search", icon: Search, label: "搜索" },
    ...(urlResult.isUrl ? [{ id: "url", icon: LinkIcon, label: "网页" }] : []),
  ];

  // AI 子菜单项
  const aiItems = [
    { id: "polish", icon: Pencil, label: "润色" },
    { id: "summarize", icon: FileText, label: "摘要" },
    { id: "explain", icon: Lightbulb, label: "解释" },
  ];

  // ── 动作执行 ──

  const executeAiAction = useCallback(async (action: string) => {
    if (!context) return;
    setView("loading");
    try {
      const result = await invoke<string>("run_ai_action", { action, text: context.text });
      await invoke("action_bar_paste_result", { result });
      getCurrentWindow().hide();
    } catch (e) {
      setErrorMsg(String(e));
      setView("error");
    }
  }, [context]);

  const executeMain = useCallback((id: string) => {
    if (id === "ai") {
      setView("submenu");
      setSubSelectedIdx(0);
    } else if (id === "translate") {
      executeAiAction("translate");
    } else if (id === "search") {
      if (!context) return;
      const baseUrl = SEARCH_URLS[searchEngineRef.current] || SEARCH_URLS.google;
      invoke("action_bar_open_url", { url: baseUrl + encodeURIComponent(context.text) });
    } else if (id === "url") {
      invoke("action_bar_open_url", { url: urlResult.url });
    }
  }, [context, urlResult, executeAiAction]);

  // ── 键盘导航 ──

  useEffect(() => {
    const handler = (e: KeyboardEvent) => {
      // Esc
      if (e.key === "Escape") {
        e.preventDefault();
        if (view === "submenu") { setView("main"); return; }
        getCurrentWindow().hide();
        return;
      }

      // loading/error 状态只响应 Esc
      if (view === "loading" || view === "error") return;

      // Cmd+数字
      if (e.metaKey || e.ctrlKey) {
        const n = parseInt(e.key, 10);
        if (!isNaN(n) && n >= 1) {
          e.preventDefault();
          if (view === "main" && n <= mainItems.length) {
            executeMain(mainItems[n - 1].id);
          } else if (view === "submenu" && n <= aiItems.length) {
            executeAiAction(aiItems[n - 1].id);
          }
          return;
        }
      }

      // ←→
      if (e.key === "ArrowLeft" || e.key === "ArrowRight") {
        e.preventDefault();
        const items = view === "submenu" ? aiItems : mainItems;
        setSelectedIdx((prev) => {
          const next = e.key === "ArrowRight" ? (prev + 1) % items.length : (prev - 1 + items.length) % items.length;
          return next;
        });
        return;
      }

      // ↑↓（子菜单内导航）
      if (view === "submenu" && (e.key === "ArrowUp" || e.key === "ArrowDown")) {
        e.preventDefault();
        setSubSelectedIdx((prev) => {
          const next = e.key === "ArrowDown" ? (prev + 1) % aiItems.length : (prev - 1 + aiItems.length) % aiItems.length;
          return next;
        });
        return;
      }

      // Enter / Space
      if (e.key === "Enter" || e.key === " ") {
        e.preventDefault();
        if (view === "main") {
          executeMain(mainItems[selectedIdx].id);
        } else if (view === "submenu") {
          executeAiAction(aiItems[subSelectedIdx].id);
        }
        return;
      }
    };
    window.addEventListener("keydown", handler);
    return () => window.removeEventListener("keydown", handler);
  }, [view, selectedIdx, subSelectedIdx, mainItems, aiItems, executeMain, executeAiAction]);

  // ── 渲染 ──

  if (view === "loading") {
    return (
      <div className="flex items-center justify-center gap-2 px-4 py-2 bg-background text-foreground rounded-lg border border-border shadow-lg">
        <Loader2 className="w-3.5 h-3.5 animate-spin text-voice" />
        <span className="text-[11px]">处理中…</span>
      </div>
    );
  }

  if (view === "error") {
    return (
      <div className="flex flex-col gap-1 px-4 py-2 bg-background text-foreground rounded-lg border border-border shadow-lg max-w-[240px]">
        <span className="text-[11px] text-red-500">{errorMsg}</span>
        <span className="text-[10px] text-muted-foreground">结果已复制到剪贴板</span>
        <button
          className="text-[10px] text-muted-foreground hover:text-foreground mt-0.5"
          onClick={() => getCurrentWindow().hide()}
        >关闭</button>
      </div>
    );
  }

  const IconBtn = ({ icon: Icon, label, active, onClick }: {
    icon: React.ElementType; label: string; active: boolean; onClick: () => void;
  }) => (
    <button
      className={cn(
        "flex flex-col items-center justify-center gap-0.5 px-3 py-1.5 rounded-md transition-all",
        active ? "bg-accent text-accent-foreground" : "text-muted-foreground hover:bg-accent hover:text-foreground",
      )}
      onClick={onClick}
      title={label}
    >
      <Icon className="w-4 h-4" />
      <span className="text-[9px]">{label}</span>
    </button>
  );

  return (
    <div className="flex flex-col bg-background text-foreground rounded-lg border border-border shadow-lg overflow-hidden">
      {/* 第一级 */}
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

      {/* AI 子菜单 */}
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
