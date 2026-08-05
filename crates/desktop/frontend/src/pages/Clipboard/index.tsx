import { useState, useCallback, useEffect, useRef, memo } from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { DndContext, PointerSensor, useSensor, useSensors, closestCenter } from "@dnd-kit/core";
import { SortableContext, useSortable, verticalListSortingStrategy } from "@dnd-kit/sortable";
import { invoke, listen } from "@/lib/tauri";
import { useClipboardHistory } from "@/hooks/useClipboardHistory";
import { useTauriEvent } from "@/hooks/useTauriEvent";
import { moveIndex, moveTab } from "@/lib/clipboardNav";
import FilterTabs from "./FilterTabs";
import SearchBar from "./SearchBar";
import ClipboardItemRow from "./ClipboardItem";
import { Pin, X, Settings2, CircleCheck, CircleX, Trash2, Eye, EyeOff, ClipboardList, Layers, GripVertical, Type, Mic, ScanText, Image as ImageIcon, FileText, FileQuestion } from "lucide-react";
import { cn } from "@/lib/utils";
import { useT } from "@/lib/i18n";
import type { ClipboardItem } from "@/types/clipboard";

interface ConfigResponse {
  config: Record<string, string | number | boolean>;
}

// 队列 tab 单条目 DTO（与后端 PasteStackItemDto 对齐：historyId / itemType / preview）。
interface PasteStackItemDto {
  historyId: string;
  itemType: string;
  preview: string;
}

// 序号 badge：① ② ③ … ⑨ ⑩（U+2460..U+2469）；超过 10 用「N.」。
const CIRCLED_DIGITS = ["①", "②", "③", "④", "⑤", "⑥", "⑦", "⑧", "⑨", "⑩"];
function queueBadge(n: number): string {
  return n >= 1 && n <= CIRCLED_DIGITS.length ? CIRCLED_DIGITS[n - 1] : `${n}.`;
}

// 与 FilterTabs.tsx TAB_DEFS 数组顺序一致——Cmd+N / Ctrl+N / ←→ 按 tab 在数组中的序号映射。
const TABS_VALUES = ["all", "favorite", "asr", "text", "ocr", "image", "file", "queue"] as const;

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

  // ── 粘贴队列（Paste Stack）多选 + 栈计数 ──
  // selectedIds: Cmd+点击追加的 history_id 列表（按入栈顺序）。空=无多选。
  // 用数组而非 Set：序号 badge 需要按入栈顺序映射（①②③…），数组天然保序且 toggle
  // 时 splice 简单；规模小（用户多选几条），O(n) 删除可忽略。
  const [selectedIds, setSelectedIds] = useState<string[]>([]);
  // selectedBadgeOf(id): 返回 1-based 序号（按 selectedIds 顺序）或 undefined（未选中）。
  const selectedBadgeOf = useCallback(
    (id: string): number | undefined => {
      const idx = selectedIds.indexOf(id);
      return idx === -1 ? undefined : idx + 1;
    },
    [selectedIds],
  );
  const handleCmdClick = useCallback((id: string) => {
    setSelectedIds((prev) => {
      const idx = prev.indexOf(id);
      if (idx === -1) return [...prev, id]; // 追加（保序）
      const next = [...prev];
      next.splice(idx, 1); // 再次 Cmd+点击取消选中
      return next;
    });
  }, []);
  // 入栈：把当前多选 ids 推到 paste_stack → toast「已入栈 N 条」→ 清多选 → 切到 queue tab。
  // 同步反馈用入栈结果 size（后端 push 返回栈大小），不依赖外部 toast 库——
  // 浮窗无 toast 依赖（package.json 无 sonner 等），用一个 1.5s 的本地气泡反馈。
  const [pushedCount, setPushedCount] = useState<number | null>(null);
  const pushTimer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const handlePushToStack = useCallback(async () => {
    if (selectedIds.length === 0) return;
    try {
      const size = await invoke<number>("push_to_paste_stack", { ids: selectedIds });
      setPushedCount(size);
      if (pushTimer.current) clearTimeout(pushTimer.current);
      pushTimer.current = setTimeout(() => setPushedCount(null), 1500);
      setSelectedIds([]); // 入栈后清多选
      setFilter("queue"); // 入栈后自动聚焦到队列 tab（2026-08-05 反馈需求）
    } catch (e) {
      console.error(e);
    }
  }, [selectedIds]);
  // 卸载清 timer
  useEffect(() => {
    return () => {
      if (pushTimer.current) clearTimeout(pushTimer.current);
    };
  }, []);

  // 粘贴队列栈计数：mount poll 一次 + 监听 paste-stack://updated 实时更新。
  // remaining=0 时隐藏计数 badge（spec §4.3）。
  const [stackRemaining, setStackRemaining] = useState(0);
  const [stackPreview, setStackPreview] = useState<string | null>(null);

  // ── 队列 tab 数据源（filter === "queue" 时启用，与 useClipboardHistory 互斥）──
  // 必须在 paste-stack://updated 监听 effect 之前声明：监听 effect 把 refreshQueue
  // 列入 deps，JS const TDZ 要求引用前先初始化。
  const [queueItems, setQueueItems] = useState<PasteStackItemDto[]>([]);
  const refreshQueue = useCallback(async () => {
    try {
      const items = await invoke<PasteStackItemDto[]>("peek_paste_stack");
      setQueueItems(items);
    } catch (e) {
      console.error("peek_paste_stack failed:", e);
    }
  }, []);
  // 拖拽状态已迁移到 @dnd-kit/sortable（SortableQueueItem 内部 useSortable 自管）。
  const handleQueueRemove = useCallback(async (index: number) => {
    try {
      await invoke("remove_from_paste_stack", { index });
    } catch (e) {
      console.error("remove_from_paste_stack failed:", e);
    }
    // 后端 emit paste-stack://updated 会触发 listen → refreshQueue，但显式调一次
    // 保证 UI 立即响应（监听是异步，drag/连续删除场景下延迟感知差）。
    await refreshQueue();
  }, [refreshQueue]);
  const handleQueueMove = useCallback(async (from: number, to: number) => {
    if (from === to) return;
    try {
      await invoke("move_paste_stack_item", { from, to });
    } catch (e) {
      console.error("move_paste_stack_item failed:", e);
    }
    await refreshQueue();
  }, [refreshQueue]);
  const handleQueueClear = useCallback(async () => {
    try {
      await invoke("clear_paste_stack");
    } catch (e) {
      console.error("clear_paste_stack failed:", e);
    }
    await refreshQueue();
  }, [refreshQueue]);

  useEffect(() => {
    let cancelled = false;
    invoke<{ remaining: number; nextPreview: string | null }>("paste_stack_status")
      .then((s) => {
        if (cancelled) return;
        setStackRemaining(s.remaining);
        setStackPreview(s.nextPreview);
      })
      .catch(() => {});
    const unlisten = listen("paste-stack://updated", (payload: unknown) => {
      const remaining = typeof payload === "number" ? payload : 0;
      // 后端 emit 的是裸 remaining 数字（event payload）。
      const n = typeof remaining === "number" ? remaining : 0;
      setStackRemaining(n);
      // 同步刷新 preview（含下一条内容）
      invoke<{ remaining: number; nextPreview: string | null }>("paste_stack_status")
        .then((s) => { if (!cancelled) setStackPreview(s.nextPreview); })
        .catch(() => {});
      // 队列 tab 打开时同步刷新列表（pop/push/remove/move/clear 都 emit 此事件）。
      if (filterRef.current === "queue") {
        refreshQueue();
      }
    });
    return () => {
      cancelled = true;
      unlisten.then((f) => f());
    };
  }, [refreshQueue]);
  // filter 切到 queue 时拉一次列表；离开时清空（避免残留旧数据误导）。
  useEffect(() => {
    if (filter === "queue") {
      refreshQueue();
    } else {
      setQueueItems([]);
    }
    // queue tab 没有选中条目概念——切到 queue 时清 previewItem，
    // 否则上次历史 tab 的 previewItem 会驱动 hover overlay 渲染，
    // 叠加到 queue 列表第一个条目上（selectedIndex 仍是 0）。
    if (filter === "queue") {
      setPreviewItem(null);
    }
  }, [filter, refreshQueue]);
  const handleClearStack = useCallback(() => {
    invoke("clear_paste_stack").catch(console.error);
    setStackRemaining(0);
    setStackPreview(null);
  }, []);
  // 一键清理两步确认：点 1 次 → confirming=true（变红 + 3s 超时），再点才执行。
  const [confirming, setConfirming] = useState(false);
  const confirmTimer = useRef<number | null>(null);

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
      if (keyboardNavTimerRef.current) clearTimeout(keyboardNavTimerRef.current);
    };
  }, []);

  // 稳定选中句柄：ClipboardItemRow 已 memo，inline 箭头 onSelect={() => ...} 会让
  // 每行 prop 引用每帧变化 → memo 失效、50 行全重绘。setSelectedIndex 来自 useState 稳定，
  // useCallback([]) 产出恒定引用，行内再以 onSelect(index) 回带 index。
  const handleSelect = useCallback((index: number) => setSelectedIndex(index), []);
  // hover 专用：键盘导航期间忽略（scrollIntoView 滚动会误触 mouseEnter）
  const handleHover = useCallback((index: number) => {
    if (keyboardNavRef.current) return;
    setSelectedIndex(index);
  }, []);

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

  // 图片类型拉缩略图（竞态守卫：快速切换时只保留最后一个结果）
  useEffect(() => {
    let cancelled = false;
    if (previewItem?.itemType === "image") {
      invoke<string>("get_image_thumb", { id: previewItem.id })
        .then(data => { if (!cancelled) setPreviewThumb(data); })
        .catch(() => { if (!cancelled) setPreviewThumb(null); });
    } else {
      setPreviewThumb(null);
    }
    return () => { cancelled = true; };
  }, [previewItem]);

  // 浮窗失焦时清空预览内容（隐藏 overlay）
  useEffect(() => {
    const win = getCurrentWindow();
    const unlisten = win.onFocusChanged(({ payload: focused }) => {
      if (!focused) setPreviewItem(null);
    });
    return () => { unlisten.then((f) => f()); };
  }, []);

  // 全局按键处理需要读最新 items/selectedIndex/search，用 ref 避免闭包过期。
  const itemsRef = useRef(items);
  itemsRef.current = items;
  const selectedIndexRef = useRef(selectedIndex);
  selectedIndexRef.current = selectedIndex;
  const searchRef = useRef(search);
  searchRef.current = search;
  const filterRef = useRef(filter);
  filterRef.current = filter;
  // 区分键盘/鼠标导航：键盘按 ↑↓ 时设 true，阻止 scrollIntoView 触发的 mouseEnter 抢 selectedIndex
  const keyboardNavRef = useRef(false);
  const keyboardNavTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);

  useEffect(() => {
    const handler = (e: KeyboardEvent) => {
      // ↑↓ 移动选中（无条件拦截，即使焦点在搜索框）
      if (e.key === "ArrowDown" || e.key === "ArrowUp") {
        e.preventDefault();
        const cur = itemsRef.current;
        if (cur.length === 0) return;
        keyboardNavRef.current = true;
        setSelectedIndex((prev) => moveIndex(prev, cur.length, e.key === "ArrowDown" ? 1 : -1));
        // 300ms 后恢复鼠标 hover 响应（留时间给 scrollIntoView + mouseEnter 事件平息）
        if (keyboardNavTimerRef.current) clearTimeout(keyboardNavTimerRef.current);
        keyboardNavTimerRef.current = setTimeout(() => { keyboardNavRef.current = false; }, 300);
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
      // Ctrl+1..7：直接跳 tab（写死，不可配置）。
      // 不用 cmd：Accessory 激活策略下会被前一 app 菜单栏 key equivalent 拦截。
      // 用 e.code（物理键位）而非 e.key——避免 macOS Option+数字产生特殊字符。
      const digitMatch = e.ctrlKey ? e.code.match(/^Digit([1-7])$/) : null;
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

  // 监听开关：mount 读 get_config + 监听 config-changed 同步。
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

  // dock 状态：吸附边缘 + 当前模式
  const [dockEdge, setDockEdge] = useState<"right" | "left" | null>(null);
  const [dockMode, setDockMode] = useState<"none" | "collapsed" | "expanded">("none");

  // 监听 dock 事件
  // cancelled 哨兵：listen() 是 async Promise，若组件在 Promise resolve 前卸载，
  // cleanup 跑时 unlisteners 还是空的 → 监听器泄漏。resolve 时检查 cancelled，
  // 已卸载则立即注销刚拿到的 unlisten 函数。（常驻 dock 窗口实际不会触发，
  // 但防未来改成可关闭窗口 + React StrictMode 双调用。）
  useEffect(() => {
    let cancelled = false;
    const unlisteners: Array<() => void> = [];
    listen("clipboard://dock-changed", (edge) => {
      if (edge === "right" || edge === "left") {
        setDockEdge(edge);
        setDockMode("collapsed");
      } else {
        setDockEdge(null);
        setDockMode("none");
      }
    }).then(f => {
      if (cancelled) { f(); } else { unlisteners.push(f); }
    });
    listen("clipboard://expand", () => setDockMode("expanded")).then(f => {
      if (cancelled) { f(); } else { unlisteners.push(f); }
    });
    listen("clipboard://collapse", () => setDockMode("collapsed")).then(f => {
      if (cancelled) { f(); } else { unlisteners.push(f); }
    });
    return () => {
      cancelled = true;
      unlisteners.forEach(f => f());
    };
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
          {/* 粘贴队列栈计数 badge：remaining>0 时显示，含下一条预览（title）+ × 清空。
              点击 × 调 clear_paste_stack；点击 badge 本身聚焦提示（无动作）。
              remaining=0 时完全隐藏（spec §4.3）。 */}
          {stackRemaining > 0 && (
            <button
              className="flex items-center gap-0.5 px-1 rounded cursor-default text-emerald-600 hover:bg-emerald-500/15 transition-colors"
              title={`粘贴队列剩余 ${stackRemaining} 条${stackPreview ? `（下一条：${stackPreview}）` : ""}\n按 Cmd+Shift+V 逐条粘贴`}
            >
              <ClipboardList className="w-3.5 h-3.5" />
              <span className="text-[10px] font-bold tabular-nums">{stackRemaining}</span>
              <span
                role="button"
                tabIndex={-1}
                className="ml-0.5 -mr-0.5 rounded p-0.5 text-muted-foreground hover:text-red-500 hover:bg-red-500/15"
                onClick={(e) => {
                  e.stopPropagation();
                  handleClearStack();
                }}
                title="清空粘贴队列"
              >
                <X className="w-2.5 h-2.5" />
              </span>
            </button>
          )}
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
        {/* ── 队列 tab：渲染 paste stack 内容（与历史列表互斥）──
            序号 badge（①②③）= 出栈顺序（index 0 = 下一个 Cmd+Shift+V 弹出）；
            拖拽 = @dnd-kit/sortable → move_paste_stack_item；× = 单条删除；
            底部「清空队列」按钮。 */}
        {filter === "queue" ? (
          <QueueListView
            items={queueItems}
            onRemove={handleQueueRemove}
            onMove={handleQueueMove}
            onClear={handleQueueClear}
            emptyText={t("clipboard.queue.empty")}
            clearText={t("clipboard.queue.clear")}
          />
        ) : items.length === 0 ? (
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
              onHover={handleHover}
              onChanged={refresh}
              selectedBadge={selectedBadgeOf(item.id)}
              onCmdClick={handleCmdClick}
            />
          ))
        )}

        {/* hover 预览 overlay：200px 宽，高度约为列表 1/3，根据选中位置上/下弹出。
            queue tab 不渲染此 overlay（QueueListView 内部自管 hover 详情，数据源独立）。 */}
        {previewEnabled && previewItem && filter !== "queue" && (() => {
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
              const top = itemEl.offsetTop + itemEl.offsetHeight - 2;
              previewTop = `${Math.min(top, listEl.scrollTop + listH - previewH)}px`;
            } else {
              // 选中在下半 → 预览在上方，顶边与条目顶边重叠 2px
              const top = itemEl.offsetTop - previewH + 2;
              previewTop = `${Math.max(top, listEl.scrollTop)}px`;
            }
          }
          return (
            <HoverOverlay item={previewItem} thumb={previewThumb} top={previewTop} height={previewH} />
          );
        })()}

        {/* 粘贴队列「入栈」浮动按钮：Cmd+点击多选 ≥1 条时显示，固定在列表底部居中。
            点击 → push_to_paste_stack → toast（pushedCount）→ 清多选。
            出栈粘贴由全局热键 Cmd+Shift+V 触发（后端 pop_and_paste）。 */}
        {selectedIds.length > 0 && (
          <div className="absolute left-1/2 bottom-2 z-40 -translate-x-1/2 flex items-center gap-1 rounded-full bg-emerald-600 px-3 py-1.5 shadow-lg shadow-emerald-900/30">
            <button
              className="flex items-center gap-1.5 text-[11px] font-semibold text-white"
              onClick={handlePushToStack}
              title="入栈后切到目标应用，按 Cmd+Shift+V 逐条粘贴"
            >
              <Layers className="w-3.5 h-3.5" />
              入栈 {selectedIds.length} 条
            </button>
            <button
              className="text-white/80 hover:text-white"
              onClick={() => setSelectedIds([])}
              title="取消多选"
            >
              <X className="w-3 h-3" />
            </button>
          </div>
        )}
        {/* 入栈成功 toast 气泡（无 sonner 依赖，本地 1.5s 反馈） */}
        {pushedCount !== null && (
          <div className="absolute left-1/2 bottom-12 z-50 -translate-x-1/2 whitespace-nowrap rounded-md bg-foreground px-2.5 py-1 text-[10px] font-medium text-background shadow">
            已入栈，剩余 {pushedCount} 条 · Cmd+Shift+V 逐条粘贴
          </div>
        )}
      </div>

      {/* Footer */}
      <div className="flex items-center justify-between px-3 py-1 border-t border-border text-[10px] text-muted-foreground/80">
        {/* 左：条数 + 一键清理 */}
        <div className="flex items-center gap-3">
          {/* 队列 tab 显示栈内条数（与历史 total 解耦）；其余 tab 显示历史 total。 */}
          <span>{filter === "queue" ? queueItems.length : total} {t("clipboard.count", { n: filter === "queue" ? queueItems.length : total })}</span>
          {/* 一键清理：删当前 tab 类别下所有非收藏条目（与搜索框正交）。
              两步确认：点 1 次 → 变红「再点确认」+ 3s 超时，再点才执行。
              收藏 tab 因 isFavorite=1 AND isFavorite=0 恒假删 0 条，禁用按钮。
              默认高亮一档（text-foreground），hover 预告危险偏红。 */}
          <button
            className={cn(
              "flex items-center gap-0.5 transition-colors",
              filter === "favorite" || filter === "queue" || !!search
                ? "opacity-50 cursor-not-allowed"
                : confirming
                  ? "text-red-500"
                  : "text-foreground hover:text-red-500",
            )}
            disabled={filter === "favorite" || filter === "queue" || !!search}
            title={
              filter === "favorite"
                ? t("clipboard.cleanFavoriteEmpty")
                : filter === "queue"
                  ? t("clipboard.queue.clear")
                  : !!search
                    ? t("clipboard.cleanSearchError")
                    : confirming
                      ? t("clipboard.cleanConfirmFull")
                      : t("clipboard.cleanNonFavorite")
            }
            onClick={() => {
              if (filter === "favorite" || filter === "queue" || !!search) return;
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

/// hover/预览 overlay 的内容体——history tab 和 queue tab 共用。
/// 区分 image（缩略图）/ file（refData）/ 其他（content 前 500 字符），
/// 顶部带类型 badge（voice→ASR）。容器定位（top/height）由调用方决定。
function HoverOverlay({
  item,
  thumb,
  top,
  height,
}: {
  item: ClipboardItem;
  thumb?: string | null;
  top: string;
  height: number;
}) {
  return (
    <div
      className="absolute right-0 w-[200px] z-30 flex flex-col overflow-hidden rounded-l-lg border border-foreground/15 shadow-2xl shadow-black/20 bg-background"
      style={{ top, height: `${height}px` }}
    >
      <div className="flex items-center gap-1 px-2 py-1.5 border-b border-border/60 flex-shrink-0">
        <ItemTypeGlyph type={item.itemType} />
        <span className="text-[9px] font-medium text-muted-foreground uppercase tracking-wide">
          {item.itemType === "voice" ? "ASR" : item.itemType}
        </span>
      </div>
      <div className="flex-1 overflow-y-auto thin-scrollbar min-h-0">
        {item.itemType === "image" ? (
          <div className="flex items-center justify-center p-2 min-h-full">
            {thumb ? (
              <img src={thumb} alt="preview" className="max-w-full max-h-full rounded object-contain" />
            ) : (
              <span className="text-[11px] text-muted-foreground">Loading...</span>
            )}
          </div>
        ) : item.itemType === "file" ? (
          <pre className="px-2 py-1.5 text-[11px] text-muted-foreground whitespace-pre-wrap break-all font-mono">
            {item.refData || ""}
          </pre>
        ) : (
          <pre className="px-2 py-1.5 text-[11px] text-foreground whitespace-pre-wrap break-words font-mono leading-relaxed">
            {item.content.length > 500
              ? item.content.slice(0, 500) + "\n\n…"
              : item.content}
          </pre>
        )}
      </div>
    </div>
  );
}

/// 队列 tab 内容：渲染 paste stack（FIFO，① = 下一个弹出）。
/// 拖拽用 @dnd-kit/sortable（WKWebView 的 HTML5 DnD 不可靠，AGENTS.md 已踩坑）。
/// onDragEnd → arrayMove 计算 from/to → onMove → move_paste_stack_item。
function QueueListView({
  items,
  onRemove,
  onMove,
  onClear,
  emptyText,
  clearText,
}: {
  items: PasteStackItemDto[];
  onRemove: (index: number) => void;
  onMove: (from: number, to: number) => void;
  onClear: () => void;
  emptyText: string;
  clearText: string;
}) {
  // PointerSensor：8px 激活距离避免普通点击误判为拖拽（点删除按钮时不触发 drag）。
  const sensors = useSensors(
    useSensor(PointerSensor, { activationConstraint: { distance: 8 } }),
  );
  // hover 详情：独立于 history tab 的 previewItem——queue 数据源是 queueItems，
  // 复用父 selectedIndex/items 会错位（已踩坑：overlay 永远盖第一个）。
  // 这里基于 queueItems 自己管 hoverIndex，overlay 显示按 historyId 查到的完整
  // ClipboardItem（content 前 500 字符 / image 缩略图，与 history overlay 一致）。
  const [hoverIndex, setHoverIndex] = useState<number | null>(null);
  const [hoverFullItem, setHoverFullItem] = useState<ClipboardItem | null>(null);
  const [hoverThumb, setHoverThumb] = useState<string | null>(null);
  // 拖拽期间禁用 hover（避免 dragOver 误触 hover 高亮）。
  const [isDragging, setIsDragging] = useState(false);
  const hoverId = hoverIndex === null ? null : items[hoverIndex]?.historyId ?? null;

  // hoverIndex 变化 → 按 historyId 查完整 ClipboardItem（竞态守卫）。
  // PasteStackItemDto 只有 preview（前 50 字符），overlay 需完整 content 才能和
  // history overlay 显示一致长度（500 字符）。
  useEffect(() => {
    if (hoverId === null) { setHoverFullItem(null); setHoverThumb(null); return; }
    let cancelled = false;
    invoke<ClipboardItem | null>("get_clipboard_item", { id: hoverId })
      .then((item) => { if (!cancelled) setHoverFullItem(item); })
      .catch((e) => { console.error("get_clipboard_item failed:", e); if (!cancelled) setHoverFullItem(null); });
    return () => { cancelled = true; };
  }, [hoverId]);

  // hoverFullItem 是 image 时拉缩略图（竞态守卫，复用 get_image_thumb）。
  useEffect(() => {
    if (hoverFullItem?.itemType !== "image") { setHoverThumb(null); return; }
    let cancelled = false;
    invoke<string>("get_image_thumb", { id: hoverFullItem.id })
      .then((data) => { if (!cancelled) setHoverThumb(data); })
      .catch(() => { if (!cancelled) setHoverThumb(null); });
    return () => { cancelled = true; };
  }, [hoverFullItem]);

  if (items.length === 0) {
    return (
      <div className="flex flex-col items-center justify-center h-full gap-1 text-muted-foreground/50">
        <Layers className="w-5 h-5 opacity-40" />
        <span className="text-xs">{emptyText}</span>
      </div>
    );
  }
  const onDragEnd = (e: { active: { id: string | number }; over: { id: string | number } | null }) => {
    setIsDragging(false);
    if (!e.over || e.active.id === e.over.id) return;
    const from = items.findIndex((it) => it.historyId === e.active.id);
    const to = items.findIndex((it) => it.historyId === e.over!.id);
    if (from === -1 || to === -1) return;
    onMove(from, to);
  };
  return (
    <div className="flex flex-col h-full">
      <DndContext
        sensors={sensors}
        collisionDetection={closestCenter}
        onDragEnd={onDragEnd}
        onDragStart={() => { setIsDragging(true); setHoverIndex(null); }}
        onDragCancel={() => setIsDragging(false)}
      >
        <SortableContext items={items.map((it) => it.historyId)} strategy={verticalListSortingStrategy}>
          <ul className="flex-1 overflow-y-auto thin-scrollbar">
            {items.map((item, i) => (
              <SortableQueueItem
                key={item.historyId}
                item={item}
                index={i}
                onRemove={onRemove}
                isHovered={!isDragging && hoverIndex === i}
                onHover={(idx) => setHoverIndex(idx)}
              />
            ))}
          </ul>
        </SortableContext>
      </DndContext>
      {/* hover 详情 overlay：与 history tab 同样 200px 宽，按 hover 行位置上/下弹出。
          内容用完整 ClipboardItem（500 字符 / image 缩略图），通过 HoverOverlay 组件复用。 */}
      {hoverFullItem && (() => {
        const itemEl = document.querySelector(`[data-queue-index="${hoverIndex}"]`) as HTMLElement | null;
        const listEl = itemEl?.offsetParent as HTMLElement | null;
        const previewH = 200;
        let previewTop = '0px';
        if (itemEl && listEl) {
          const itemMid = itemEl.offsetTop + itemEl.offsetHeight / 2 - listEl.scrollTop;
          const listH = listEl.clientHeight;
          if (itemMid < listH / 2) {
            const top = itemEl.offsetTop + itemEl.offsetHeight - 2;
            previewTop = `${Math.min(top, listEl.scrollTop + listH - previewH)}px`;
          } else {
            const top = itemEl.offsetTop - previewH + 2;
            previewTop = `${Math.max(top, listEl.scrollTop)}px`;
          }
        }
        return (
          <HoverOverlay item={hoverFullItem} thumb={hoverThumb} top={previewTop} height={previewH} />
        );
      })()}
      {/* 底部「清空队列」按钮 */}
      <div className="flex-shrink-0 px-2 py-1.5 border-t border-border">
        <button
          className="w-full flex items-center justify-center gap-1 rounded py-1 text-[11px] text-muted-foreground hover:text-red-500 hover:bg-red-500/10 transition-colors"
          onClick={onClear}
        >
          <Trash2 className="w-3 h-3" />
          {clearText}
        </button>
      </div>
    </div>
  );
}

/// 单条队列条目——memo 包裹避免拖拽时 sibling 重绘。
/// useSortable 提供 transform/transition + listeners + isDragging。
/// 删除按钮需 stopPropagation 避免 listeners 拦截 onClick。
/// hover 通过 onHover(index|null) 上报——dnd-kit listeners 不会拦截 mouseEnter/Leave。
const SortableQueueItem = memo(function SortableQueueItem({
  item,
  index,
  onRemove,
  isHovered,
  onHover,
}: {
  item: PasteStackItemDto;
  index: number;
  onRemove: (index: number) => void;
  isHovered: boolean;
  onHover: (index: number | null) => void;
}) {
  const { attributes, listeners, setNodeRef, transform, transition, isDragging } = useSortable({
    id: item.historyId,
  });
  const style: React.CSSProperties = {
    transform: transform ? `translate3d(${transform.x}px, ${transform.y}px, 0)` : undefined,
    transition,
    zIndex: isDragging ? 10 : undefined,
  };
  return (
    <li
      ref={setNodeRef}
      style={style}
      data-queue-index={index}
      {...attributes}
      {...listeners}
      // mouseEnter/Leave 上报 hover——dnd-kit 的 PointerSensor listeners 走 pointer 事件，
      // 不拦截 mouse 事件，可以共存。
      onMouseEnter={() => onHover(index)}
      onMouseLeave={() => onHover(null)}
      className={cn(
        "group flex items-center gap-1.5 px-2 py-1.5 border-b border-border/40 cursor-grab active:cursor-grabbing transition-colors bg-background",
        isDragging && "opacity-50 shadow-lg",
        isHovered && "bg-accent/50",
      )}
    >
      {/* 序号 badge：①②③… = 出栈顺序；前置 Grip 提示可拖 */}
      <GripVertical className="w-3 h-3 text-muted-foreground/40 flex-shrink-0" />
      <span className="flex-shrink-0 w-4 text-center text-sm text-emerald-600 leading-none">
        {queueBadge(index + 1)}
      </span>
      <ItemTypeGlyph type={item.itemType} />
      <span className="flex-1 min-w-0 text-xs text-foreground truncate">
        {item.preview || <span className="text-muted-foreground/50">（无内容预览）</span>}
      </span>
      <button
        className="flex-shrink-0 opacity-0 group-hover:opacity-100 rounded p-0.5 text-muted-foreground hover:text-red-500 hover:bg-red-500/15 transition-all"
        // listeners 会拦截整个 li 的 pointer 事件——删除按钮必须 stopPropagation
        // 才能让 onClick 触发（dnd-kit 的 listeners 优先级高于 React onClick）。
        onPointerDown={(e) => e.stopPropagation()}
        onClick={(e) => {
          e.stopPropagation();
          onRemove(index);
        }}
        title="删除"
      >
        <X className="w-3 h-3" />
      </button>
    </li>
  );
});

/// 队列条目类型小图标——按 itemType 字段挑 lucide 图标，与 FilterTabs 视觉呼应。
/// 未知类型退化为 FileQuestion（不致崩）；itemType 来自后端 as_str()，恒为
/// text/voice/ocr/image/file 之一。
function ItemTypeGlyph({ type }: { type: string }) {
  // text→Type, voice→Mic, ocr→ScanText, image→Image, file→FileText。
  const cls = "w-3.5 h-3.5 flex-shrink-0 text-muted-foreground";
  switch (type) {
    case "text":
      return <Type className={cls} />;
    case "voice":
      return <Mic className={cls} />;
    case "ocr":
      return <ScanText className={cls} />;
    case "image":
      return <ImageIcon className={cls} />;
    case "file":
      return <FileText className={cls} />;
    default:
      return <FileQuestion className={cls} />;
  }
}
