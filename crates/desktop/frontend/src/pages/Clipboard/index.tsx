import { useState, useCallback, useEffect, useRef } from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { invoke, listen } from "@/lib/tauri";
import { useClipboardHistory } from "@/hooks/useClipboardHistory";
import { useTauriEvent } from "@/hooks/useTauriEvent";
import { moveIndex, moveTab } from "@/lib/clipboardNav";
import FilterTabs from "./FilterTabs";
import SearchBar from "./SearchBar";
import ClipboardItemRow from "./ClipboardItem";
import { Pin, X, Settings2, CircleCheck, CircleX, Trash2, Eye, EyeOff } from "lucide-react";
import { cn } from "@/lib/utils";
import { useT } from "@/lib/i18n";
import type { ClipboardItem } from "@/types/clipboard";

interface ConfigResponse {
  config: Record<string, string | number | boolean>;
}

// 与 FilterTabs.tsx TABS 数组顺序一致——Cmd+N 序号映射。
const TABS_VALUES = ["all", "favorite", "asr", "text", "ocr", "image", "file"] as const;

export default function Clipboard() {
  const t = useT();
  const [filter, setFilter] = useState("all");
  const [search, setSearch] = useState("");
  const [pinned, setPinned] = useState(false);
  // 键盘导航以数组索引为第一性 citizen；执行动作时从 items[selectedIndex].id 取。
  const [selectedIndex, setSelectedIndex] = useState<number | null>(null);
  const [recording, setRecording] = useState(true);
  // 预览面板开关（标题栏按钮控制）；默认关闭，记住用户选择
  const [previewEnabled, setPreviewEnabled] = useState(() => {
    return localStorage.getItem("clipboard-preview-enabled") !== "false";
  });
  const togglePreview = useCallback(() => {
    setPreviewEnabled(prev => {
      const next = !prev;
      localStorage.setItem("clipboard-preview-enabled", String(next));
      return next;
    });
  }, []);
  // 预览内容：当前选中/hover 条目的完整数据
  const [previewItem, setPreviewItem] = useState<ClipboardItem | null>(null);
  const [previewThumb, setPreviewThumb] = useState<string | null>(null);
  // 一键清理两步确认：点 1 次 → confirming=true（变红 + 3s 超时），再点才执行。
  const [confirming, setConfirming] = useState(false);
  const confirmTimer = useRef<number | null>(null);
  // 浮窗内切 Tab 的修饰键（cmd/ctrl/alt），由设置页配置，默认 ctrl。
  const tabModifierRef = useRef<"cmd" | "ctrl" | "alt">("ctrl");

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

  // filter 切换 → 清确认态 + 清 timer（避免在 A tab 点了第一步、切到 B tab 后第二次点击误清 B）
  useEffect(() => {
    setConfirming(false);
    if (confirmTimer.current) {
      clearTimeout(confirmTimer.current);
      confirmTimer.current = null;
    }
  }, [filter]);

  // 卸载清 timer，防泄漏
  useEffect(() => {
    return () => {
      if (confirmTimer.current) clearTimeout(confirmTimer.current);
    };
  }, []);

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

  // 选中变化时更新预览内容
  useEffect(() => {
    if (selectedIndex === null) { setPreviewItem(null); return; }
    const item = items[selectedIndex];
    if (item) setPreviewItem(item);
  }, [selectedIndex, items]);

  // 图片类型拉缩略图
  useEffect(() => {
    if (previewItem?.item_type === "image") {
      invoke<string>("get_image_thumb", { id: previewItem.id })
        .then(setPreviewThumb)
        .catch(() => setPreviewThumb(null));
    } else {
      setPreviewThumb(null);
    }
  }, [previewItem]);

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
      // +1..7：直接跳 tab。修饰键由设置页配置（cmd/ctrl/alt），默认 ctrl。
      // 注意：cmd 在 Accessory 激活策略下可能被前一 app 菜单栏 key equivalent 拦截。
      // 用 e.code（物理键位）而非 e.key（产生的字符）——macOS Option+数字会产生
      // 特殊字符（Option+1="¡"），e.key 不匹配 "1".."7"。
      const mod = tabModifierRef.current;
      const modPressed = mod === "cmd" ? e.metaKey : mod === "ctrl" ? e.ctrlKey : e.altKey;
      const digitMatch = modPressed ? e.code.match(/^Digit([1-7])$/) : null;
      if (digitMatch) {
        e.preventDefault();
        const n = parseInt(digitMatch[1], 10) - 1;
        if (n < TABS_VALUES.length) setFilter(TABS_VALUES[n]);
        return;
      }
    };
    window.addEventListener("keydown", handler);
    return () => window.removeEventListener("keydown", handler);
  }, []);

  // 监听开关 + Tab 修饰键：mount 读 get_config + 监听 config-changed 同步。
  const loadRecording = useCallback(async () => {
    try {
      const resp = await invoke<ConfigResponse>("get_config");
      setRecording(resp.config.clipboard_enabled !== false);
      const mod = resp.config.clipboard_tab_modifier as string;
      if (mod === "cmd" || mod === "ctrl" || mod === "alt") {
        tabModifierRef.current = mod;
      }
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

  // dock 状态：吸附边缘 + 当前模式
  const [dockEdge, setDockEdge] = useState<"right" | "left" | null>(null);
  const [dockMode, setDockMode] = useState<"none" | "collapsed" | "expanded">("none");

  // 监听 dock 事件
  useEffect(() => {
    const unlisteners: Array<() => void> = [];
    listen("clipboard://dock-changed", (edge) => {
      if (edge === "right" || edge === "left") {
        setDockEdge(edge);
        setDockMode("collapsed");
      } else {
        setDockEdge(null);
        setDockMode("none");
      }
    }).then(f => unlisteners.push(f));
    listen("clipboard://expand", () => setDockMode("expanded")).then(f => unlisteners.push(f));
    listen("clipboard://collapse", () => setDockMode("collapsed")).then(f => unlisteners.push(f));
    return () => unlisteners.forEach(f => f());
  }, []);

  return (
    <div
      className={cn(
        "flex flex-col h-screen select-none data-tauri-drag-region",
        dockMode === "collapsed"
          ? "w-[300px]"
          : "w-[300px] overflow-hidden rounded-xl border border-border shadow-2xl shadow-black/8",
      )}
      style={{
        background: dockMode === "collapsed" ? "transparent" : "var(--color-background)",
      }}
    >
      {/* dock 收缩态：只显示 8px 细条 */}
      {dockMode === "collapsed" && dockEdge && (
        <div
          className={cn(
            "absolute top-0 bottom-0 w-2 bg-voice/80 shadow-[0_0_8px_rgba(0,0,0,0.3)] hover:bg-voice transition-colors duration-150 cursor-pointer",
            dockEdge === "right" ? "right-0" : "left-0",
          )}
          style={{ pointerEvents: "auto" }}
          onMouseEnter={() => {
            invoke("clipboard_dock_expand");
            setDockMode("expanded");
          }}
          onMouseDown={() => {
            invoke("clipboard_dock_expand");
            setDockMode("expanded");
          }}
        />
      )}
      {/* dock 展开态 / 正常态：显示完整内容 */}
      {dockMode !== "collapsed" && (
        <>
      {/* Title bar — 极简，去掉"历史" */}
      {/* deep：点击标题文本/空白均触发拖动；按钮仍因 clickable 元素被 drag.js 跳过，不受影响 */}
      <div className="flex items-center justify-between px-2 py-1.5 cursor-grab active:cursor-grabbing" data-tauri-drag-region="deep">
        <button
          className="p-1 rounded cursor-default hover:bg-accent text-muted-foreground hover:text-foreground transition-colors"
          onClick={() => getCurrentWindow().hide()}
          title={t("clipboard.close")}
        >
          <X className="w-3.5 h-3.5" />
        </button>
        <span className="text-[11px] font-medium tracking-wide text-muted-foreground">{t("clipboard.title")}</span>
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
            title={recording ? t("clipboard.pauseListen") : t("clipboard.resumeListen")}
          >
            {recording
              ? <CircleCheck className="w-3.5 h-3.5" />
              : <CircleX className="w-3.5 h-3.5" />}
          </button>
          <button
            className={cn(
              "p-1 rounded cursor-default transition-colors",
              previewEnabled ? "text-voice bg-voice/10" : "text-muted-foreground hover:bg-accent hover:text-foreground",
            )}
            onClick={togglePreview}
            title={previewEnabled ? t("clipboard.previewOn") : t("clipboard.previewOff")}
          >
            {previewEnabled ? <Eye className="w-3.5 h-3.5" /> : <EyeOff className="w-3.5 h-3.5" />}
          </button>
          <button
            className={cn(
              "p-1 rounded cursor-default transition-colors",
              pinned ? "text-voice bg-voice/10" : "text-muted-foreground hover:bg-accent hover:text-foreground",
            )}
            onClick={togglePin}
            title={t("clipboard.pin")}
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
      <div className="clipboard-list flex-1 overflow-y-auto pb-1 relative">
        {items.length === 0 ? (
          <div className="flex flex-col items-center justify-center h-full gap-1 text-muted-foreground/50">
            <span className="text-xs">{t("clipboard.empty")}</span>
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

        {/* hover 预览 overlay：200px 宽，高度约为列表 1/3，根据选中位置上/下弹出 */}
        {previewEnabled && previewItem && (() => {
          // 选中条目在列表上半部分 → 预览弹在下方；下半部分 → 弹在上方
          const itemEl = document.querySelector(`[data-clip-index="${selectedIndex}"]`) as HTMLElement | null;
          const listEl = itemEl?.offsetParent as HTMLElement | null;
          const previewH = 200;
          let previewTop = '0px';
          if (itemEl && listEl) {
            const itemMid = itemEl.offsetTop + itemEl.offsetHeight / 2 - listEl.scrollTop;
            const listH = listEl.clientHeight;
            if (itemMid < listH / 2) {
              // 选中在上半 → 预览在下方，底边与条目底边重叠 2px
              previewTop = `${itemEl.offsetTop + itemEl.offsetHeight - 2}px`;
            } else {
              // 选中在下半 → 预览在上方，顶边与条目顶边重叠 2px
              previewTop = `${itemEl.offsetTop - previewH + 2}px`;
            }
          }
          return (
          <div
            className="absolute right-0 w-[200px] z-30 flex flex-col overflow-hidden rounded-l-lg border border-foreground/15 shadow-2xl shadow-black/20 bg-background"
            style={{ top: previewTop, height: `${previewH}px` }}
          >
            <div className="flex items-center gap-1 px-2 py-1.5 border-b border-border/60 flex-shrink-0">
              <span className="text-[9px] font-medium text-muted-foreground uppercase tracking-wide">
                {previewItem.item_type === "voice" ? "ASR" : previewItem.item_type}
              </span>
            </div>
            <div className="flex-1 overflow-y-auto thin-scrollbar min-h-0">
              {previewItem.item_type === "image" ? (
                <div className="flex items-center justify-center p-2 min-h-full">
                  {previewThumb ? (
                    <img src={previewThumb} alt="preview" className="max-w-full max-h-full rounded object-contain" />
                  ) : (
                    <span className="text-[11px] text-muted-foreground">Loading...</span>
                  )}
                </div>
              ) : previewItem.item_type === "file" ? (
                <pre className="px-2 py-1.5 text-[11px] text-muted-foreground whitespace-pre-wrap break-all font-mono">
                  {previewItem.ref_data || ""}
                </pre>
              ) : (
                <pre className="px-2 py-1.5 text-[11px] text-foreground whitespace-pre-wrap break-words font-mono leading-relaxed">
                  {previewItem.content}
                </pre>
              )}
            </div>
          </div>
          );
        })()}
      </div>

      {/* Footer */}
      <div className="flex items-center justify-between px-3 py-1 border-t border-border text-[10px] text-muted-foreground/80">
        {/* 左：条数 + 一键清理 */}
        <div className="flex items-center gap-3">
          <span>{total} {t("clipboard.count", { n: total })}</span>
          {/* 一键清理：删当前 tab 类别下所有非收藏条目（与搜索框正交）。
              两步确认：点 1 次 → 变红「再点确认」+ 3s 超时，再点才执行。
              收藏 tab 因 is_favorite=1 AND is_favorite=0 恒假删 0 条，禁用按钮。
              默认高亮一档（text-foreground），hover 预告危险偏红。 */}
          <button
            className={cn(
              "flex items-center gap-0.5 transition-colors",
              filter === "favorite" || !!search
                ? "opacity-50 cursor-not-allowed"
                : confirming
                  ? "text-red-500"
                  : "text-foreground hover:text-red-500",
            )}
            disabled={filter === "favorite" || !!search}
            title={
              filter === "favorite"
                ? t("clipboard.cleanFavoriteEmpty")
                : !!search
                  ? t("clipboard.cleanSearchError")
                : confirming
                  ? t("clipboard.cleanConfirmFull")
                  : t("clipboard.cleanNonFavorite")
            }
            onClick={() => {
              if (filter === "favorite" || !!search) return;
              if (!confirming) {
                setConfirming(true);
                confirmTimer.current = window.setTimeout(() => {
                  setConfirming(false);
                  confirmTimer.current = null;
                }, 3000);
              } else {
                if (confirmTimer.current) {
                  clearTimeout(confirmTimer.current);
                  confirmTimer.current = null;
                }
                setConfirming(false);
                invoke("clear_clipboard_history_by_filter", { filter, keepFavorite: true }).catch(console.error);
              }
            }}
          >
            <Trash2 className="w-2.5 h-2.5" />
            {confirming ? t("clipboard.cleanConfirm") : t("clipboard.cleanAll")}
          </button>
        </div>
        {/* 右：管理 */}
        <button
          className="flex items-center gap-0.5 hover:text-foreground transition-colors"
          onClick={() => invoke("open_settings", { initialPage: "clipboard" })}
          title={t("clipboard.manageMode")}
        >
          <Settings2 className="w-2.5 h-2.5" />
          {t("clipboard.manage")}
        </button>
      </div>
        </>
      )}
    </div>
  );
}
