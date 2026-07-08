import { useState, useRef, useEffect, useCallback, type ReactNode } from "react";
import { invoke, listen } from "@/lib/tauri";
import {
  Undo2, Redo2, ZoomIn, ZoomOut, Search, Eraser, Save, X,
  ChevronUp, ChevronDown, Replace, Check, Type, Eye, Mic,
} from "lucide-react";
import ImagePreviewComponent from "@/pages/ImagePreview";

interface Tab {
  key: string;
  source: 'clipboard' | 'transcription';
  itemId: number;
  itemType?: 'text' | 'image';
  text?: string;
  imgWidth?: number;
  imgHeight?: number;
}
interface OpenTabPayload {
  itemId: number;
  source: string;
}
// 后端 get_pending_compact_tabs 返回（含完整数据，前端免再查 DB）。
interface PendingTabFull {
  itemId: number;
  source: string;
  itemType: string;
  text: string;
  imgWidth?: number;
  imgHeight?: number;
}
function pendingToTab(p: PendingTabFull): Tab {
  const key = `${p.source}:${p.itemId}`;
  const source = p.source as Tab['source'];
  if (p.itemType === 'image') {
    return { key, source, itemId: p.itemId, itemType: 'image', imgWidth: p.imgWidth || 0, imgHeight: p.imgHeight || 0 };
  }
  return { key, source, itemId: p.itemId, itemType: 'text', text: p.text };
}

const FONT_KEY = "compact-editor-font-size";
const FONT_MIN = 12;
const FONT_MAX = 24;
const MAX_IMAGE_TABS = 5;

function tabTitle(tab: Tab): string {
  const text = tab.text || "";
  const head = text.slice(0, 5).replace(/\s+/g, " ").trim() || (tab.itemType === 'image' ? "图片" : "空");
  const tail = tab.itemId.toString(16).slice(-5);
  return `${head}-${tail}`;
}

function tabIcon(tab: Tab) {
  if (tab.source === 'transcription') return <Mic className="w-3 h-3 text-violet-500 flex-shrink-0" />;
  if (tab.itemType === 'image') return <Eye className="w-3 h-3 text-blue-500 flex-shrink-0" />;
  return <Type className="w-3 h-3 text-muted-foreground flex-shrink-0" />;
}

// URL 参数初始化：Rust 建窗时拼入首个 tab 数据，前端首次渲染即有内容（零 IPC）。
function readInitialTabFromUrl(): { tabs: Tab[]; hasInitial: boolean } {
  const params = new URLSearchParams(window.location.search);
  const itemId = params.get("itemId");
  const source = params.get("source");
  if (!itemId || !source) return { tabs: [], hasInitial: false };
  const id = Number(itemId);
  const itemType = params.get("itemType") || "text";
  const text = params.get("text") || "";
  const key = `${source}:${id}`;
  if (itemType === "image") {
    const imgWidth = Number(params.get("imgWidth") || 0);
    const imgHeight = Number(params.get("imgHeight") || 0);
    return { tabs: [{ key, source: source as any, itemId: id, itemType: "image" as const, imgWidth, imgHeight }], hasInitial: true };
  }
  return { tabs: [{ key, source: source as any, itemId: id, itemType: "text" as const, text }], hasInitial: true };
}

// 定义在组件外部——避免每次渲染创建新函数引用导致子树 unmount/remount
const ToolBtn = ({ onClick, title, disabled, children }: {
  onClick: () => void; title: string; disabled?: boolean; children: ReactNode;
}) => (
  <button
    type="button"
    disabled={disabled}
    title={title}
    onClick={onClick}
    className="p-1.5 rounded-md text-muted-foreground hover:bg-accent hover:text-foreground disabled:opacity-30 disabled:hover:bg-transparent transition-colors"
  >{children}</button>
);

function CompactEditor() {
  const [initial] = useState(() => readInitialTabFromUrl());
  const [tabs, setTabs] = useState<Tab[]>(initial.tabs);
  const [initialLoading, setInitialLoading] = useState(!initial.hasInitial);
  const [activeIdx, setActiveIdx] = useState(0);
  const [fontSize, setFontSize] = useState(() => {
    const saved = Number(localStorage.getItem(FONT_KEY));
    return saved >= FONT_MIN && saved <= FONT_MAX ? saved : 15;
  });
  const [savedFlash, setSavedFlash] = useState(false);
  const savedFlashTimer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const [showFind, setShowFind] = useState(false);
  const [findQuery, setFindQuery] = useState("");
  const [replaceQuery, setReplaceQuery] = useState("");
  const [matchIdx, setMatchIdx] = useState(-1);
  const [matches, setMatches] = useState<number[]>([]);

  const taRef = useRef<HTMLTextAreaElement>(null);
  const tabsRef = useRef<Tab[]>([]);
  useEffect(() => { tabsRef.current = tabs; }, [tabs]);
  // 并发加载占位：loadAndAddTab 的 await 期间 tabsRef 尚未更新，快速连点同一 item 会
  // 两路都过 findIndex（-1）→ setTabs 各加一份 → 重复 key。await 前同步占位拦截。
  const pendingKeysRef = useRef<Set<string>>(new Set());

  const active = tabs[activeIdx];
  const isReadOnly = active?.source === 'transcription';

  const updateActiveText = useCallback((next: string) => {
    setTabs(prev => prev.map((t, i) => (i === activeIdx ? { ...t, text: next } : t)));
  }, [activeIdx]);

  // 按 index 更新任意 tab 文本（hidden 挂载的 textarea 需要）
  const updateActiveTextAt = useCallback((next: string, idx: number) => {
    setTabs(prev => prev.map((t, i) => (i === idx ? { ...t, text: next } : t)));
  }, []);

  // 加载某 item 并新增 tab；已存在则切过去。source 决定从哪个表读 + 是否只读。
  const loadAndAddTab = useCallback(async (itemId: number, source: string) => {
    const key = `${source}:${itemId}`;
    const existIdx = tabsRef.current.findIndex(t => t.key === key);
    if (existIdx >= 0) { setActiveIdx(existIdx); return; }
    // 并发拦截：await invoke 期间 tabsRef 尚未更新，同一 key 的二次调用会重复添加。
    // pendingKeysRef 在 await 前同步占位（JS 单线程，await 前同步段不被中断）；setTabs
    // 内再 prev.some 双保险（防 tabsRef 本身 stale）。finally 释放，失败可重试。
    if (pendingKeysRef.current.has(key)) return;
    pendingKeysRef.current.add(key);
    try {
      if (source === 'transcription') {
        const text = await invoke<string>("get_transcription_text", { id: itemId }).catch(() => "");
        setTabs(prev => prev.some(t => t.key === key) ? prev : [...prev, { key, source: 'transcription', itemId, text }]);
        setTabs(prev => { const idx = prev.findIndex(t => t.key === key); if (idx >= 0) { tabsRef.current = prev; setActiveIdx(idx); } return prev; });
        return;
      }

      // clipboard：先查类型，再加载
      const itemType = await invoke<string>("get_clipboard_item_type", { itemId }).catch(() => "text");
      if (itemType === 'image') {
        // 图片 tab ≤5 限制
        setTabs(prev => {
          if (prev.some(t => t.key === key)) return prev;
          const imageTabs = prev.filter(t => t.itemType === 'image');
          let next = prev;
          if (imageTabs.length >= MAX_IMAGE_TABS) {
            const oldestKey = imageTabs[0].key;
            next = prev.filter(t => t.key !== oldestKey);
          }
          return [...next, { key, source: 'clipboard' as const, itemId, itemType: 'image' as const }];
        });
        // setActiveIdx 在 setTabs 回调外无法拿到新长度——用 next.length 计算。
        // 淘汰时数组长度不变（去掉一个加一个），新增时 +1。
        // tabsRef 可能 stale，所以用 setTabs 的 callback 形式同步更新 ref。
        setTabs(prev => {
          const newIdx = prev.findIndex(t => t.key === key);
          if (newIdx >= 0) {
            tabsRef.current = prev;
            setActiveIdx(newIdx);
          }
          return prev;
        });
      } else {
        const text = await invoke<string>("get_clipboard_item_text", { itemId }).catch(() => "");
        setTabs(prev => prev.some(t => t.key === key) ? prev : [...prev, { key, source: 'clipboard' as const, itemId, itemType: 'text' as const, text }]);
        setTabs(prev => { const idx = prev.findIndex(t => t.key === key); if (idx >= 0) { tabsRef.current = prev; setActiveIdx(idx); } return prev; });
      }
    } finally {
      pendingKeysRef.current.delete(key);
    }
  }, []);

  // mount：URL 已注入首个 tab（零 IPC）；invoke get_pending_compact_tabs take 剩余
  // pending（批量双开场景：URL 只注入首个，其余 pending tab 于此补齐，与 URL 首个
  // 按 key 去重）；注册 open-tab 事件用于后续已 mount 窗口的新开 tab。
  useEffect(() => {
    let cancelled = false;
    let unlisten: (() => void) | undefined;
    (async () => {
      const pendingTabs = await invoke<PendingTabFull[]>("get_pending_compact_tabs");
      if (cancelled) return;
      if (pendingTabs.length > 0) {
        setTabs(prev => {
          const seen = new Set(prev.map(t => t.key));
          const added: Tab[] = [];
          for (const p of pendingTabs) {
            const key = `${p.source}:${p.itemId}`;
            if (seen.has(key)) continue; // URL 首个已在 prev，去重
            seen.add(key);
            added.push(pendingToTab(p));
          }
          return added.length > 0 ? [...prev, ...added] : prev;
        });
      }
      setInitialLoading(false);
      setTimeout(() => taRef.current?.focus(), 0);
      const fn = await listen("compact-editor://open-tab", (payload) => {
        const p = payload as OpenTabPayload;
        loadAndAddTab(p.itemId, p.source);
      });
      if (cancelled) fn();
      else unlisten = fn;
    })();
    return () => {
      cancelled = true;
      unlisten?.();
    };
  }, [loadAndAddTab]);

  const doSave = useCallback(async () => {
    if (!active) return;
    try {
      if ((active.text || "").trim() === "") {
        // 清空后保存 = 删除条目（空内容无意义）；后端 delete_clipboard_item 已 emit clipboard://changed 通知列表刷新
        await invoke("delete_clipboard_item", { id: active.itemId });
        // 关闭该 tab：仅剩一个则关窗，否则移除并修正 activeIdx
        if (tabs.length <= 1) {
          invoke("close_compact_editor");
          return;
        }
        const idx = activeIdx;
        setTabs(prev => prev.filter((_, i) => i !== idx));
        setActiveIdx(i => (idx < i ? i - 1 : idx === i ? Math.min(i, tabs.length - 2) : i));
        return;
      }
      await invoke("set_clipboard_item_text", { itemId: active.itemId, text: active.text || "" });
      setSavedFlash(true);
      if (savedFlashTimer.current) clearTimeout(savedFlashTimer.current);
      savedFlashTimer.current = setTimeout(() => setSavedFlash(false), 1200);
    } catch (e) {
      console.error("保存失败:", e);
    }
  }, [active, activeIdx, tabs.length]);

  // keydown 监听器稳定化：doSave 依赖 [active, activeIdx, tabs.length]，active.text 每键变 → doSave
  // 每键拿新引用；若 keydown useEffect deps 含 doSave，监听器每键 remove+add（GC 压力）。改用 ref：
  // 监听器只挂载一次，调 doSaveRef.current() 取最新；doSave 本身仍供按钮 onClick 直接用。
  const doSaveRef = useRef(doSave);
  useEffect(() => { doSaveRef.current = doSave; }, [doSave]);

  // 关闭 tab：仅剩一个则关窗；否则移除并修正 activeIdx。
  const closeTab = (idx: number) => {
    if (tabs.length <= 1) {
      invoke("close_compact_editor");
      return;
    }
    // 函数式更新：快速连点关两个 tab 时，非函数式 setTabs(next) 基于闭包 tabs（stale），
    // 第二次覆盖第一次 → 被关的 tab 复活。setTabs 走 prev 链式（setActiveIdx 本就函数式）。
    setTabs(prev => prev.filter((_, i) => i !== idx));
    setActiveIdx(i => {
      if (idx < i) return i - 1;
      if (idx === i) return Math.min(i, tabs.length - 2); // 过滤后 length=tabs.length-1，max idx=tabs.length-2
      return i;
    });
  };

  const charCount = active ? [...(active.text || "")].length : 0;

  // ── 字号 ──
  const decFont = () => setFontSize(f => Math.max(FONT_MIN, f - 1));
  const incFont = () => setFontSize(f => Math.min(FONT_MAX, f + 1));
  useEffect(() => { localStorage.setItem(FONT_KEY, String(fontSize)); }, [fontSize]);

  // ── 撤销/重做：execCommand 触发 textarea 原生栈（按钮路径；实测在 WKWebView 工作）──
  const undo = () => { taRef.current?.focus(); document.execCommand("undo"); };
  const redo = () => { taRef.current?.focus(); document.execCommand("redo"); };

  // ── 清空当前 tab 文本（二次确认）──
  const [clearPending, setClearPending] = useState(false);
  const clearAll = () => {
    if (!clearPending) { setClearPending(true); setTimeout(() => setClearPending(false), 2000); return; }
    updateActiveText(""); setClearPending(false); setMatches([]); setMatchIdx(-1);
  };

  // ── 查找/替换（基于 active.text；图片 tab text=undefined → ""）──
  // 收集 text 中 q 的所有匹配起点（大小写不敏感，runFind/replaceOne/replaceAll 共用同一口径）。
  const collectMatches = (text: string, q: string): number[] => {
    if (!q) return [];
    const lower = text.toLowerCase();
    const needle = q.toLowerCase();
    const idxs: number[] = [];
    let from = 0;
    while (true) {
      const i = lower.indexOf(needle, from);
      if (i === -1) break;
      idxs.push(i);
      from = i + needle.length;
    }
    return idxs;
  };

  const selectRange = (start: number, len: number) => {
    const ta = taRef.current;
    if (!ta || !active) return;
    ta.focus();
    ta.setSelectionRange(start, start + len);
    // 不手算 scrollTop：split("\n").length 在 soft wrap 下低估实际渲染行 → 算出的
    // scrollTop 偏上 → 目标匹配挡在视口下方看不见。setSelectionRange 原生会滚动使选区
    // 可见，WebKit 按实际渲染行（soft-wrap / 硬换行都准确）定位，无需手算行高。
    // 焦点留在正文（不还回查找框）：WKWebView 下 textarea 失焦后选区高亮不渲染，必须保持
    // focus 才能看到匹配被选中。查找模式下的连续 Enter 跳转 / 防误删换行改由正文 onKeyDown
    // 拦截 Enter 实现（见下方 textarea），而非靠查找框持焦。
  };

  // runFind 只负责「重算匹配 + 更新计数」，绝不 selectRange：selectRange 内部 ta.focus()
  // 会把焦点从查找框抢到正文，导致用户在查找框每打一个字符就被拽走、输不全查找词。
  // matchIdx 统一重置为 -1（未定位哨兵）：计数显示 0/N；跳转交给 Enter / ↑↓ 按钮（gotoMatch），
  // 首次 gotoMatch(1) 落第一项、gotoMatch(-1) 落末项。
  const runFind = useCallback((): number[] => {
    if (!findQuery || !active) { setMatches([]); setMatchIdx(-1); return []; }
    const idxs = collectMatches(active.text || "", findQuery);
    setMatches(idxs);
    setMatchIdx(-1);
    return idxs;
  }, [findQuery, active]);

  // 仅在 findQuery 变化（用户输入查找词）时跳转到第一个匹配，不在 text 变化时跳
  const prevFindQuery = useRef("");
  const findDebounce = useRef<ReturnType<typeof setTimeout> | null>(null);
  useEffect(() => {
    if (!showFind) return;
    // findQuery 变化 → 立即重新匹配 + 跳转。用 runFind 同步返回的 idxs 跳转，不读 matches：
    // setMatches 异步，同帧闭包 matches 是旧值（首次为 []）→ 跳转失效或落到旧匹配错位。
    if (findQuery !== prevFindQuery.current) {
      // 用户在查找框输入 → 立即重算匹配计数，但【不跳转/不 selectRange】：selectRange 的
      // ta.focus() 会抢走查找框焦点，导致每输一个字符就被拽到正文、输不全查找词。
      // 计数由 runFind 更新（matchIdx=-1 → 显示 0/N）；跳转交给 Enter / ↑↓ 按钮（gotoMatch）。
      prevFindQuery.current = findQuery;
      if (findDebounce.current) clearTimeout(findDebounce.current);
      runFind();
      return;
    }
    // text 变化（打字/编辑）→ debounce 150ms 后更新匹配计数（避免每键扫描全文）
    if (findDebounce.current) clearTimeout(findDebounce.current);
    findDebounce.current = setTimeout(() => runFind(), 150);
    // 清理：关查找栏（showFind→false 重跑本 effect）或卸载时取消 pending timer，避免到期后
    // runFind 在已隐藏/已卸载组件上跑全文扫描。deps 不含 matches——否则 runFind 设 matches
    // （每次新数组引用）→ effect 重跑 → 再设 timer → 每 150ms 空转循环扫描全文。
    return () => { if (findDebounce.current) clearTimeout(findDebounce.current); };
  }, [findQuery, showFind, runFind]);

  const gotoMatch = (delta: number) => {
    if (matches.length === 0) return;
    // matchIdx=-1（未定位，刚改完查找词）时，Enter/↓ 落第一项、Shift+Enter/↑ 落末项；
    // 否则按 delta 环形跳转。-1 是哨兵不是真实索引，不能直接进模运算。
    let next: number;
    if (matchIdx < 0) {
      next = delta > 0 ? 0 : matches.length - 1;
    } else {
      next = (matchIdx + delta + matches.length) % matches.length;
    }
    setMatchIdx(next);
    selectRange(matches[next], findQuery.length);
  };

  const replaceOne = () => {
    if (matchIdx < 0 || !findQuery || !active) return;
    const start = matches[matchIdx];
    const next = (active.text || "").slice(0, start) + replaceQuery + (active.text || "").slice(start + findQuery.length);
    updateActiveText(next);
    // 基于 next 重算（setTimeout(runFind) 闭包会拿到旧 active，在替换前文本上匹配/选中错位）。
    // 替换当前项后该位置匹配消失、后续前移 → 原 matchIdx 即指向下一项；clamp 防越界（末项被替后回退到新末项）。
    const idxs = collectMatches(next, findQuery);
    setMatches(idxs);
    const nextIdx = idxs.length > 0 ? Math.min(matchIdx, idxs.length - 1) : -1;
    setMatchIdx(nextIdx);
    if (nextIdx >= 0) selectRange(idxs[nextIdx], findQuery.length);
  };

  const replaceAll = () => {
    if (!findQuery || !active) return;
    // 大小写不敏感全局替换（与 runFind 的 toLowerCase 口径一致；split 是大小写敏感的，会漏替大小写不同的匹配）。
    const escaped = findQuery.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
    const next = (active.text || "").replace(new RegExp(escaped, "gi"), () => replaceQuery);
    updateActiveText(next);
    const idxs = collectMatches(next, findQuery);
    setMatches(idxs);
    setMatchIdx(idxs.length > 0 ? 0 : -1);
    if (idxs.length > 0) selectRange(idxs[0], findQuery.length);
  };

  // ── 快捷键 ──
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      // IME 组字期放行所有快捷键：让 Esc 等键交给 IME 取消候选词。
      if (e.isComposing || e.keyCode === 229) return;
      const mod = e.metaKey || e.ctrlKey;
      // Cmd/Ctrl+Z undo、+Shift 或 +Y redo：受控 textarea 的原生 undo 栈被每次输入后 React value
      // 同步清空（WebKit 行为）→ 键盘失灵；改走 execCommand（同按钮路径，文档级 transaction 栈，可用）。
      if (mod && (e.key.toLowerCase() === "z" || e.key.toLowerCase() === "y")) {
        e.preventDefault();
        taRef.current?.focus();
        const isRedo = e.key.toLowerCase() === "y" || e.shiftKey;
        document.execCommand(isRedo ? "redo" : "undo");
        return;
      }
      // doSave 经 ref 调用：监听器只挂载一次（deps 仅 showFind），避免 active.text 每键变 → doSave
      // 新引用 → 监听器每键 remove+add 的 GC 压力。
      if (mod && e.key === "Enter") { e.preventDefault(); if (!active?.itemType || active.itemType === 'text') doSaveRef.current(); return; }
      if (mod && e.key.toLowerCase() === "s") { e.preventDefault(); if (!active?.itemType || active.itemType === 'text') doSaveRef.current(); return; }
      if (e.key === "Escape") {
        if (showFind) { setShowFind(false); return; }
      }
      if (mod && e.key.toLowerCase() === "f") { e.preventDefault(); setShowFind(true); return; }
    };
    document.addEventListener("keydown", onKey);
    return () => document.removeEventListener("keydown", onKey);
  }, [showFind]);

  return (
    <div className="flex flex-col h-full bg-background">
      {/* tab 栏 */}
      {tabs.length > 0 && (
        <div className="flex-shrink-0 flex items-center gap-0.5 px-1.5 py-1 border-b border-border bg-muted overflow-x-auto thin-scrollbar">
          {tabs.map((t, i) => (
            <div
              key={t.key}
              className={`group/tab flex items-center gap-1 pl-2.5 pr-1 py-1 rounded-md text-xs whitespace-nowrap cursor-pointer transition-colors ${
                i === activeIdx
                  ? "bg-background text-foreground shadow-sm border border-border"
                  : "text-muted-foreground hover:bg-accent"
              }`}
              onClick={() => setActiveIdx(i)}
            >
              {tabIcon(t)}
              <span className="max-w-[140px] truncate">{tabTitle(t)}</span>
              <button
                type="button"
                title="关闭"
                onClick={(e) => { e.stopPropagation(); closeTab(i); }}
                className="p-0.5 rounded hover:bg-accent text-muted-foreground hover:text-foreground"
              >
                <X className="w-3 h-3" />
              </button>
            </div>
          ))}
        </div>
      )}

      {/* 工具栏（仅文本 tab 显示） */}
      {active && active.itemType !== 'image' && !isReadOnly && (
        <div className="flex-shrink-0 flex items-center gap-0.5 px-2 py-1.5 border-b border-border bg-muted">
          <ToolBtn onClick={undo} title="撤销 (Cmd+Z)"><Undo2 className="w-4 h-4" /></ToolBtn>
          <ToolBtn onClick={redo} title="重做 (Cmd+Shift+Z)"><Redo2 className="w-4 h-4" /></ToolBtn>
          <span className="w-px h-4 bg-border mx-1" />
          <ToolBtn onClick={decFont} title="缩小字号" disabled={fontSize <= FONT_MIN}><ZoomOut className="w-4 h-4" /></ToolBtn>
          <span className="text-[11px] text-muted-foreground w-7 text-center tabular-nums">{fontSize}</span>
          <ToolBtn onClick={incFont} title="放大字号" disabled={fontSize >= FONT_MAX}><ZoomIn className="w-4 h-4" /></ToolBtn>
          <span className="w-px h-4 bg-border mx-1" />
          <ToolBtn onClick={() => setShowFind(v => !v)} title="查找/替换 (Cmd+F)"><Search className="w-4 h-4" /></ToolBtn>
          <ToolBtn onClick={clearAll} title="清空">
            {clearPending ? <Check className="w-4 h-4 text-red-500" /> : <Eraser className="w-4 h-4" />}
          </ToolBtn>
          <div className="flex-1" />
          <span className="text-[11px] text-muted-foreground mr-2 tabular-nums">{charCount} 字</span>
          <button
            type="button"
            onClick={doSave}
            className={`flex items-center gap-1 px-2.5 py-1 rounded-md text-xs text-white transition-colors ${
              savedFlash ? "bg-emerald-600" : "bg-[#007aff] hover:bg-[#0066d6]"
            }`}
          >
            {savedFlash ? <Check className="w-3.5 h-3.5" /> : <Save className="w-3.5 h-3.5" />}
            {savedFlash ? "已保存" : "保存"}
            <span className="text-[10px] opacity-70">⌘↵</span>
          </button>
        </div>
      )}

      {/* 查找/替换条 */}
      {showFind && active && active.itemType !== 'image' && !isReadOnly && (
        <div className="flex-shrink-0 flex flex-wrap items-center gap-1.5 px-2 py-1.5 border-b border-border bg-muted">
          <input
            autoFocus
            value={findQuery}
            onChange={e => setFindQuery(e.target.value)}
            onKeyDown={e => { if (e.key === "Enter") { e.preventDefault(); gotoMatch(e.shiftKey ? -1 : 1); } }}
            placeholder="查找"
            className="w-32 px-2 py-0.5 text-xs border border-border rounded bg-background outline-none focus:border-voice"
          />
          <span className="text-[10px] text-muted-foreground w-12 tabular-nums">
            {matches.length > 0 ? `${matchIdx + 1}/${matches.length}` : "0/0"}
          </span>
          <ToolBtn onClick={() => gotoMatch(-1)} title="上一个" disabled={matches.length === 0}><ChevronUp className="w-3.5 h-3.5" /></ToolBtn>
          <ToolBtn onClick={() => gotoMatch(1)} title="下一个" disabled={matches.length === 0}><ChevronDown className="w-3.5 h-3.5" /></ToolBtn>
          <input
            value={replaceQuery}
            onChange={e => setReplaceQuery(e.target.value)}
            placeholder="替换"
            className="w-32 px-2 py-0.5 text-xs border border-border rounded bg-background outline-none focus:border-voice"
          />
          <button type="button" onClick={replaceOne} className="px-2 py-0.5 text-[11px] rounded border border-border hover:bg-accent">替换</button>
          <button type="button" onClick={replaceAll} className="flex items-center gap-0.5 px-2 py-0.5 text-[11px] rounded border border-border hover:bg-accent">
            <Replace className="w-3 h-3" /> 全替
          </button>
        </div>
      )}

      {/* 内容区：所有 tab hidden 挂载（图片保持状态），仅活跃 tab 可见 */}
      {tabs.length > 0 ? (
        tabs.map((tab, i) => (
          <div key={tab.key} className="flex-1 flex flex-col" style={{ display: i === activeIdx ? 'flex' : 'none' }}>
            {tab.itemType === 'image' ? (
              // 图片 Tab 懒加载：仅活跃 Tab 挂载 ImagePreview，避免隐藏 Tab 仍并发拉全图
              // （get_image_full）+ 建 createImageBitmap——5 张全分辨率图常驻致内存×Tab 数暴涨。
              // 切回重新加载（标注/缩放状态重置可接受：图片非高频切换、用户通常逐张处理）。
              i === activeIdx ? (
                <ImagePreviewComponent imageId={tab.itemId} initialWidth={tab.imgWidth} initialHeight={tab.imgHeight} />
              ) : (
                <div className="flex-1 flex items-center justify-center text-xs text-muted-foreground bg-background">
                  切换到此标签加载图片
                </div>
              )
            ) : (
              <textarea
                ref={i === activeIdx ? taRef : undefined}
                value={tab.text || ''}
                onChange={e => {
                  const idx = tabs.findIndex(t => t.key === tab.key);
                  if (idx >= 0) updateActiveTextAt(e.target.value, idx);
                }}
                onKeyDown={e => {
                  // 查找栏打开时，正文里的 Enter/Shift+Enter 走跳转（不插换行、不替换选区），
                  // 与查找框 Enter 行为一致，支持跳转后连续跳；Esc 关查找栏恢复正常编辑。
                  if (showFind && e.key === "Enter") { e.preventDefault(); gotoMatch(e.shiftKey ? -1 : 1); }
                }}
                readOnly={tab.source === 'transcription'}
                style={{ fontSize: `${fontSize}px`, lineHeight: 1.6 }}
                spellCheck={false}
                className="flex-1 w-full resize-none outline-none p-4 bg-background text-foreground thin-scrollbar"
                placeholder={tab.source === 'transcription' ? "语音识别记录（只读）" : "在此编辑…"}
              />
            )}
          </div>
        ))
      ) : initialLoading ? (
        <div className="flex-1" />
      ) : (
        <div className="flex-1 flex items-center justify-center text-sm text-muted-foreground">没有打开的条目</div>
      )}
    </div>
  );
}

export default CompactEditor;
