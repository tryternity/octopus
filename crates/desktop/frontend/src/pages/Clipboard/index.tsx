import { useState, useCallback, useEffect, useRef } from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { invoke } from "@/lib/tauri";
import { useClipboardHistory } from "@/hooks/useClipboardHistory";
import { useTauriEvent } from "@/hooks/useTauriEvent";
import { moveIndex, moveTab } from "@/lib/clipboardNav";
import FilterTabs from "./FilterTabs";
import SearchBar from "./SearchBar";
import ClipboardItemRow from "./ClipboardItem";
import { Pin, X, Settings2, CircleCheck, CircleX } from "lucide-react";
import { cn } from "@/lib/utils";

interface ConfigResponse {
  config: Record<string, string | number | boolean>;
}

// 与 FilterTabs.tsx TABS 数组顺序一致——Cmd+N 序号映射。
const TABS_VALUES = ["all", "favorite", "asr", "text", "ocr", "image", "file"] as const;

export default function Clipboard() {
  const [filter, setFilter] = useState("all");
  const [search, setSearch] = useState("");
  const [pinned, setPinned] = useState(false);
  // 键盘导航以数组索引为第一性 citizen；执行动作时从 items[selectedIndex].id 取。
  const [selectedIndex, setSelectedIndex] = useState<number | null>(null);
  const [recording, setRecording] = useState(true);

  const { items, total, refresh } = useClipboardHistory(filter, search);

  // items 变化（过滤/搜索/刷新）后夹紧 selectedIndex：越界则重置到首条或 null。
  useEffect(() => {
    setSelectedIndex((prev) => {
      if (items.length === 0) return null;
      if (prev === null) return 0;
      if (prev >= items.length) return 0;
      return prev;
    });
  }, [items]);

  // 稳定选中句柄：ClipboardItemRow 已 memo，inline 箭头 onSelect={() => ...} 会让
  // 每行 prop 引用每帧变化 → memo 失效、50 行全重绘。setSelectedIndex 来自 useState 稳定，
  // useCallback([]) 产出恒定引用，行内再以 onSelect(index) 回带 index。
  const handleSelect = useCallback((index: number) => setSelectedIndex(index), []);

  // 选中变化时滚动到可见行。
  useEffect(() => {
    if (selectedIndex === null) return;
    const el = document.querySelector(`[data-clip-index="${selectedIndex}"]`);
    el?.scrollIntoView({ block: "nearest" });
  }, [selectedIndex]);

  // 全局按键处理需要读最新 items/selectedIndex/search，用 ref 避免闭包过期。
  const itemsRef = useRef(items);
  itemsRef.current = items;
  const selectedIndexRef = useRef(selectedIndex);
  selectedIndexRef.current = selectedIndex;
  const searchRef = useRef(search);
  searchRef.current = search;
  const filterRef = useRef(filter);
  filterRef.current = filter;

  useEffect(() => {
    const handler = (e: KeyboardEvent) => {
      // ↑↓ 移动选中（无条件拦截，即使焦点在搜索框）
      if (e.key === "ArrowDown" || e.key === "ArrowUp") {
        e.preventDefault();
        const cur = itemsRef.current;
        if (cur.length === 0) return;
        setSelectedIndex((prev) => moveIndex(prev, cur.length, e.key === "ArrowDown" ? 1 : -1));
        return;
      }
      // Enter：对选中条目执行粘贴（复用 paste_clipboard_item，后端已双保险：写剪贴板+模拟粘贴）
      if (e.key === "Enter") {
        e.preventDefault();
        const cur = itemsRef.current;
        const idx = selectedIndexRef.current;
        if (idx === null || idx >= cur.length) return;
        invoke("paste_clipboard_item", { id: cur[idx].id }).catch(console.error);
        return;
      }
      // Esc：有搜索内容则清空，已空则隐藏浮窗
      if (e.key === "Escape") {
        e.preventDefault();
        if (searchRef.current !== "") {
          setSearch("");
        } else {
          getCurrentWindow().hide();
        }
        return;
      }
      // Tab / Shift+Tab：恒定切过滤 tab（preventDefault 拦截，不让浏览器遍历全浮窗焦点）
      if (e.key === "Tab") {
        e.preventDefault();
        const cur = TABS_VALUES.indexOf(filterRef.current as (typeof TABS_VALUES)[number]);
        const next = moveTab(cur < 0 ? 0 : cur, TABS_VALUES.length, e.shiftKey ? -1 : 1);
        setFilter(TABS_VALUES[next]);
        return;
      }
      // ←→：仅搜索框为空时切 tab（有内容时让出给光标移动，不拦截）
      if ((e.key === "ArrowLeft" || e.key === "ArrowRight") && searchRef.current === "") {
        e.preventDefault();
        const cur = TABS_VALUES.indexOf(filterRef.current as (typeof TABS_VALUES)[number]);
        const next = moveTab(cur < 0 ? 0 : cur, TABS_VALUES.length, e.key === "ArrowLeft" ? -1 : 1);
        setFilter(TABS_VALUES[next]);
        return;
      }
      // Cmd+1..7：直接跳 tab（metaKey=macOS，ctrlKey=Windows/Linux）
      if ((e.metaKey || e.ctrlKey) && e.key >= "1" && e.key <= "7") {
        e.preventDefault();
        const n = parseInt(e.key, 10) - 1;
        if (n < TABS_VALUES.length) setFilter(TABS_VALUES[n]);
        return;
      }
    };
    window.addEventListener("keydown", handler);
    return () => window.removeEventListener("keydown", handler);
  }, []);

  // 监听开关：mount 读 get_config + 监听 config-changed 同步（与设置页 toggle 双向同步）。
  const loadRecording = useCallback(async () => {
    try {
      const resp = await invoke<ConfigResponse>("get_config");
      setRecording(resp.config.clipboard_enabled !== false);
    } catch (e) {
      console.error(e);
    }
  }, []);
  useEffect(() => { loadRecording(); }, [loadRecording]);
  useTauriEvent("config-changed", () => loadRecording());

  const toggleRecording = useCallback(async () => {
    const next = !recording;
    setRecording(next); // 乐观更新；config-changed 回调会校正
    try {
      await invoke("set_config", { key: "clipboard_enabled", value: next });
    } catch (e) {
      setRecording(!next); // 回滚
      console.error(e);
    }
  }, [recording]);

  const togglePin = useCallback(async () => {
    const next = !pinned;
    setPinned(next);
    try {
      const win = getCurrentWindow();
      await win.setAlwaysOnTop(next);
    } catch (e) {
      console.error(e);
    }
  }, [pinned]);

  return (
    <div className="flex flex-col h-screen bg-background text-foreground select-none overflow-hidden rounded-xl border border-border shadow-2xl shadow-black/8" data-tauri-drag-region>
      {/* Title bar — 极简，去掉"历史" */}
      {/* deep：点击标题文本/空白均触发拖动；按钮仍因 clickable 元素被 drag.js 跳过，不受影响 */}
      <div className="flex items-center justify-between px-2 py-1.5 cursor-grab active:cursor-grabbing" data-tauri-drag-region="deep">
        <button
          className="p-1 rounded cursor-default hover:bg-accent text-muted-foreground hover:text-foreground transition-colors"
          onClick={() => getCurrentWindow().hide()}
          title="关闭"
        >
          <X className="w-3.5 h-3.5" />
        </button>
        <span className="text-[11px] font-medium tracking-wide text-muted-foreground">剪贴板</span>
        <div className="flex items-center gap-0.5">
          {/* 监听开关：复制敏感内容前可在此快速暂停。与 Pin 同为状态 toggle，成组于右侧。 */}
          <button
            className={cn(
              "p-1 rounded cursor-default transition-colors",
              recording
                ? "text-green-500 hover:bg-green-500/15"
                : "text-red-500 bg-red-500/15 hover:bg-red-500/25",
            )}
            onClick={toggleRecording}
            title={recording ? "暂停监听" : "恢复监听"}
          >
            {recording
              ? <CircleCheck className="w-3.5 h-3.5" />
              : <CircleX className="w-3.5 h-3.5" />}
          </button>
          <button
            className={cn(
              "p-1 rounded cursor-default transition-colors",
              pinned ? "text-voice bg-voice/10" : "text-muted-foreground hover:bg-accent hover:text-foreground",
            )}
            onClick={togglePin}
            title="置顶"
          >
            <Pin className="w-3.5 h-3.5" />
          </button>
        </div>
      </div>

      {/* Search + Filter */}
      <div className="px-2 pb-1.5 flex flex-col gap-1.5">
        <SearchBar value={search} onChange={setSearch} />
        <FilterTabs value={filter} onChange={setFilter} />
      </div>

      {/* List */}
      <div className="clipboard-list flex-1 overflow-y-auto pb-1">
        {items.length === 0 ? (
          <div className="flex flex-col items-center justify-center h-full gap-1 text-muted-foreground/50">
            <span className="text-xs">暂无记录</span>
          </div>
        ) : (
          items.map((item, index) => (
            <ClipboardItemRow
              key={item.id}
              item={item}
              index={index}
              isLast={index === items.length - 1}
              isSelected={selectedIndex === index}
              onSelect={handleSelect}
              onChanged={refresh}
            />
          ))
        )}
      </div>

      {/* Footer */}
      <div className="flex items-center justify-between px-3 py-1 border-t border-border text-[10px] text-muted-foreground/80">
        <span>{total} 条</span>
        <button
          className="flex items-center gap-0.5 hover:text-foreground transition-colors"
          onClick={() => invoke("open_settings", { initialPage: "clipboard" })}
          title="管理剪贴板"
        >
          <Settings2 className="w-2.5 h-2.5" />
          管理
        </button>
      </div>
    </div>
  );
}
