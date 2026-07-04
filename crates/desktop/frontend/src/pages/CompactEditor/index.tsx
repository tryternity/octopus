import { useState, useRef, useEffect, useCallback, type ReactNode } from "react";
import { invoke, listen } from "@/lib/tauri";
import {
  Undo2, Redo2, ZoomIn, ZoomOut, Search, Eraser, Save, X,
  ChevronUp, ChevronDown, Replace, Check, Type, Eye, Mic,
} from "lucide-react";
// ImagePreviewComponent 在 Task 3 接入（改为接受 props 的组件）
// import ImagePreviewComponent from "@/pages/ImagePreview";

interface Tab {
  key: string;
  source: 'clipboard' | 'transcription';
  itemId: number;
  itemType?: 'text' | 'image';
  text?: string;
}
interface OpenTabPayload {
  itemId: number;
  source: string;
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
  return <Type className="w-3 h-3 text-stone-400 flex-shrink-0" />;
}

function CompactEditor() {
  const [tabs, setTabs] = useState<Tab[]>([]);
  const [activeIdx, setActiveIdx] = useState(0);
  const [fontSize, setFontSize] = useState(() => {
    const saved = Number(localStorage.getItem(FONT_KEY));
    return saved >= FONT_MIN && saved <= FONT_MAX ? saved : 15;
  });
  const [savedFlash, setSavedFlash] = useState(false);
  const [showFind, setShowFind] = useState(false);
  const [findQuery, setFindQuery] = useState("");
  const [replaceQuery, setReplaceQuery] = useState("");
  const [matchIdx, setMatchIdx] = useState(-1);
  const [matches, setMatches] = useState<number[]>([]);

  const taRef = useRef<HTMLTextAreaElement>(null);
  const tabsRef = useRef<Tab[]>([]);
  useEffect(() => { tabsRef.current = tabs; }, [tabs]);

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

    if (source === 'transcription') {
      const text = await invoke<string>("get_transcription_text", { id: itemId }).catch(() => "");
      setTabs(prev => [...prev, { key, source: 'transcription', itemId, text }]);
      setActiveIdx(tabsRef.current.length);
      return;
    }

    // clipboard：先查类型，再加载
    const itemType = await invoke<string>("get_clipboard_item_type", { itemId }).catch(() => "text");
    if (itemType === 'image') {
      // 图片 tab ≤5 限制
      setTabs(prev => {
        const imageTabs = prev.filter(t => t.itemType === 'image');
        let next = prev;
        if (imageTabs.length >= MAX_IMAGE_TABS) {
          const oldestKey = imageTabs[0].key;
          next = prev.filter(t => t.key !== oldestKey);
        }
        return [...next, { key, source: 'clipboard' as const, itemId, itemType: 'image' as const }];
      });
    } else {
      const text = await invoke<string>("get_clipboard_item_text", { itemId }).catch(() => "");
      setTabs(prev => [...prev, { key, source: 'clipboard' as const, itemId, itemType: 'text' as const, text }]);
    }
    setActiveIdx(tabsRef.current.length);
  }, []);

  // mount：取首个 pending tab；监听并发再开的 open-tab 事件
  useEffect(() => {
    let unlisten: (() => void) | undefined;
    (async () => {
      const pending = await invoke<{ itemId: number; source: string } | null>("get_pending_compact_tab");
      if (pending) {
        await loadAndAddTab(pending.itemId, pending.source);
      }
      unlisten = await listen("compact-editor://open-tab", (payload) => {
        const p = payload as OpenTabPayload;
        loadAndAddTab(p.itemId, p.source);
      });
    })();
    return () => { unlisten?.(); };
  }, [loadAndAddTab]);

  const doSave = useCallback(async () => {
    if (!active) return;
    try {
      if (active.text || "".trim() === "") {
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
      setTimeout(() => setSavedFlash(false), 1200);
    } catch (e) {
      console.error("保存失败:", e);
    }
  }, [active, activeIdx, tabs.length]);

  // 关闭 tab：仅剩一个则关窗；否则移除并修正 activeIdx。
  const closeTab = (idx: number) => {
    if (tabs.length <= 1) {
      invoke("close_compact_editor");
      return;
    }
    const next = tabs.filter((_, i) => i !== idx);
    setTabs(next);
    setActiveIdx(i => {
      if (idx < i) return i - 1;
      if (idx === i) return Math.min(i, next.length - 1);
      return i;
    });
  };

  const charCount = active ? [...(active.text || "" || "")].length : 0;

  // ── 字号 ──
  const decFont = () => setFontSize(f => Math.max(FONT_MIN, f - 1));
  const incFont = () => setFontSize(f => Math.min(FONT_MAX, f + 1));
  useEffect(() => { localStorage.setItem(FONT_KEY, String(fontSize)); }, [fontSize]);

  // ── 撤销/重做：execCommand 触发 textarea 原生栈（Cmd+Z/Y 原生亦生效，作可靠兜底）──
  const undo = () => { taRef.current?.focus(); document.execCommand("undo"); };
  const redo = () => { taRef.current?.focus(); document.execCommand("redo"); };

  // ── 清空当前 tab 文本（二次确认）──
  const [clearPending, setClearPending] = useState(false);
  const clearAll = () => {
    if (!clearPending) { setClearPending(true); setTimeout(() => setClearPending(false), 2000); return; }
    updateActiveText(""); setClearPending(false); setMatches([]); setMatchIdx(-1);
  };

  // ── 查找/替换（基于 active.text || ""）──
  const selectRange = (start: number, len: number) => {
    const ta = taRef.current;
    if (!ta || !active) return;
    ta.focus();
    ta.setSelectionRange(start, start + len);
    const lineHeight = fontSize * 1.6;
    const lineNum = (active.text || "").slice(0, start).split("\n").length;
    ta.scrollTop = Math.max(0, (lineNum - 2) * lineHeight);
  };

  const runFind = useCallback(() => {
    const q = findQuery;
    if (!q || !active) { setMatches([]); setMatchIdx(-1); return; }
    const lower = active.text || "".toLowerCase();
    const needle = q.toLowerCase();
    const idxs: number[] = [];
    let from = 0;
    while (true) {
      const i = lower.indexOf(needle, from);
      if (i === -1) break;
      idxs.push(i);
      from = i + needle.length;
    }
    setMatches(idxs);
    setMatchIdx(idxs.length > 0 ? 0 : -1);
    if (idxs.length > 0) selectRange(idxs[0], q.length);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [findQuery, active, fontSize]);

  useEffect(() => { if (showFind) runFind(); }, [runFind, showFind]);

  const gotoMatch = (delta: number) => {
    if (matches.length === 0) return;
    const next = (matchIdx + delta + matches.length) % matches.length;
    setMatchIdx(next);
    selectRange(matches[next], findQuery.length);
  };

  const replaceOne = () => {
    if (matchIdx < 0 || !findQuery || !active) return;
    const start = matches[matchIdx];
    const next = active.text || "".slice(0, start) + replaceQuery + active.text || "".slice(start + findQuery.length);
    updateActiveText(next);
    setTimeout(runFind, 0);
  };

  const replaceAll = () => {
    if (!findQuery || !active) return;
    updateActiveText(active.text || "".split(findQuery).join(replaceQuery));
    setTimeout(runFind, 0);
  };

  // ── 快捷键 ──
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      // IME 组字期放行所有快捷键：让 Esc 等键交给 IME 取消候选词。
      if (e.isComposing || e.keyCode === 229) return;
      const mod = e.metaKey || e.ctrlKey;
      if (mod && e.key === "Enter") { e.preventDefault(); doSave(); return; }
      if (mod && e.key.toLowerCase() === "s") { e.preventDefault(); doSave(); return; }
      if (e.key === "Escape") {
        if (showFind) { setShowFind(false); return; }
      }
      if (mod && e.key.toLowerCase() === "f") { e.preventDefault(); setShowFind(true); return; }
    };
    document.addEventListener("keydown", onKey);
    return () => document.removeEventListener("keydown", onKey);
  }, [doSave, showFind]);

  const ToolBtn = ({ onClick, title, disabled, children }: {
    onClick: () => void; title: string; disabled?: boolean; children: ReactNode;
  }) => (
    <button
      type="button"
      disabled={disabled}
      title={title}
      onClick={onClick}
      className="p-1.5 rounded-md text-stone-600 hover:bg-stone-100 hover:text-stone-900 disabled:opacity-30 disabled:hover:bg-transparent transition-colors"
    >{children}</button>
  );

  return (
    <div className="flex flex-col h-full bg-background">
      {/* tab 栏 */}
      {tabs.length > 0 && (
        <div className="flex-shrink-0 flex items-center gap-0.5 px-1.5 py-1 border-b border-border bg-stone-50 overflow-x-auto thin-scrollbar">
          {tabs.map((t, i) => (
            <div
              key={t.key}
              className={`group/tab flex items-center gap-1 pl-2.5 pr-1 py-1 rounded-md text-xs whitespace-nowrap cursor-pointer transition-colors ${
                i === activeIdx
                  ? "bg-white text-stone-900 shadow-sm border border-stone-200"
                  : "text-stone-500 hover:bg-stone-100"
              }`}
              onClick={() => setActiveIdx(i)}
            >
              {tabIcon(t)}
              <span className="max-w-[140px] truncate">{tabTitle(t)}</span>
              <button
                type="button"
                title="关闭"
                onClick={(e) => { e.stopPropagation(); closeTab(i); }}
                className="p-0.5 rounded hover:bg-stone-200 text-stone-400 hover:text-stone-700"
              >
                <X className="w-3 h-3" />
              </button>
            </div>
          ))}
        </div>
      )}

      {/* 工具栏（仅文本 tab 显示） */}
      {active && active.itemType !== 'image' && !isReadOnly && (
        <div className="flex-shrink-0 flex items-center gap-0.5 px-2 py-1.5 border-b border-border bg-stone-50">
          <ToolBtn onClick={undo} title="撤销 (Cmd+Z)"><Undo2 className="w-4 h-4" /></ToolBtn>
          <ToolBtn onClick={redo} title="重做 (Cmd+Shift+Z)"><Redo2 className="w-4 h-4" /></ToolBtn>
          <span className="w-px h-4 bg-stone-200 mx-1" />
          <ToolBtn onClick={decFont} title="缩小字号" disabled={fontSize <= FONT_MIN}><ZoomOut className="w-4 h-4" /></ToolBtn>
          <span className="text-[11px] text-stone-500 w-7 text-center tabular-nums">{fontSize}</span>
          <ToolBtn onClick={incFont} title="放大字号" disabled={fontSize >= FONT_MAX}><ZoomIn className="w-4 h-4" /></ToolBtn>
          <span className="w-px h-4 bg-stone-200 mx-1" />
          <ToolBtn onClick={() => setShowFind(v => !v)} title="查找/替换 (Cmd+F)"><Search className="w-4 h-4" /></ToolBtn>
          <ToolBtn onClick={clearAll} title="清空">
            {clearPending ? <Check className="w-4 h-4 text-red-500" /> : <Eraser className="w-4 h-4" />}
          </ToolBtn>
          <div className="flex-1" />
          <span className="text-[11px] text-stone-400 mr-2 tabular-nums">{charCount} 字</span>
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
        <div className="flex-shrink-0 flex flex-wrap items-center gap-1.5 px-2 py-1.5 border-b border-border bg-stone-100">
          <input
            autoFocus
            value={findQuery}
            onChange={e => setFindQuery(e.target.value)}
            onKeyDown={e => { if (e.key === "Enter") gotoMatch(e.shiftKey ? -1 : 1); }}
            placeholder="查找"
            className="w-32 px-2 py-0.5 text-xs border border-stone-300 rounded bg-white outline-none focus:border-[#007aff]"
          />
          <span className="text-[10px] text-stone-500 w-12 tabular-nums">
            {matches.length > 0 ? `${matchIdx + 1}/${matches.length}` : "0/0"}
          </span>
          <ToolBtn onClick={() => gotoMatch(-1)} title="上一个" disabled={matches.length === 0}><ChevronUp className="w-3.5 h-3.5" /></ToolBtn>
          <ToolBtn onClick={() => gotoMatch(1)} title="下一个" disabled={matches.length === 0}><ChevronDown className="w-3.5 h-3.5" /></ToolBtn>
          <input
            value={replaceQuery}
            onChange={e => setReplaceQuery(e.target.value)}
            placeholder="替换"
            className="w-32 px-2 py-0.5 text-xs border border-stone-300 rounded bg-white outline-none focus:border-[#007aff]"
          />
          <button type="button" onClick={replaceOne} className="px-2 py-0.5 text-[11px] rounded border border-stone-300 hover:bg-stone-200">替换</button>
          <button type="button" onClick={replaceAll} className="flex items-center gap-0.5 px-2 py-0.5 text-[11px] rounded border border-stone-300 hover:bg-stone-200">
            <Replace className="w-3 h-3" /> 全替
          </button>
        </div>
      )}

      {/* 内容区：所有 tab hidden 挂载（图片保持状态），仅活跃 tab 可见 */}
      {tabs.length > 0 ? (
        tabs.map((tab, i) => (
          <div key={tab.key} className="flex-1 flex flex-col" style={{ display: i === activeIdx ? 'flex' : 'none' }}>
            {tab.itemType === 'image' ? (
              <div className="flex-1 flex items-center justify-center text-sm text-stone-400">图片预览（Task 3 接入）</div>
            ) : (
              <textarea
                value={tab.text || ''}
                onChange={e => {
                  const idx = tabs.findIndex(t => t.key === tab.key);
                  if (idx >= 0) updateActiveTextAt(e.target.value, idx);
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
      ) : (
        <div className="flex-1 flex items-center justify-center text-sm text-stone-400">没有打开的条目</div>
      )}
    </div>
  );
}

export default CompactEditor;
