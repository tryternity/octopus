import { useState, useRef, useEffect, useCallback, type ReactNode } from "react";
import { invoke, listen } from "@/lib/tauri";
import { emit } from "@tauri-apps/api/event";
import {
  Undo2, Redo2, ZoomIn, ZoomOut, Search, Eraser, Save, X,
  ChevronUp, ChevronDown, Replace, Check,
} from "lucide-react";

interface PendingEdit {
  text: string;
  requestId: string;
}

const FONT_KEY = "compact-editor-font-size";
const FONT_MIN = 12;
const FONT_MAX = 24;

function CompactEditor() {
  const [text, setText] = useState("");
  const [fontSize, setFontSize] = useState(() => {
    const saved = Number(localStorage.getItem(FONT_KEY));
    return saved >= FONT_MIN && saved <= FONT_MAX ? saved : 15;
  });
  const [showFind, setShowFind] = useState(false);
  const [findQuery, setFindQuery] = useState("");
  const [replaceQuery, setReplaceQuery] = useState("");
  const [matchIdx, setMatchIdx] = useState(-1);
  const [matches, setMatches] = useState<number[]>([]);

  const taRef = useRef<HTMLTextAreaElement>(null);
  const requestIdRef = useRef<string>("");
  const savedRef = useRef(false); // 区分 unmount 时该发 result 还是 cancel

  // ── mount：拉取初始文本 + 监听并发再开 ──
  useEffect(() => {
    let unlisten: (() => void) | undefined;
    (async () => {
      const pending = await invoke<PendingEdit | null>("get_pending_compact_edit");
      if (pending) {
        setText(pending.text);
        requestIdRef.current = pending.requestId;
        setTimeout(() => taRef.current?.focus(), 0);
      }
      unlisten = await listen("compact-editor://load", (payload) => {
        const p = payload as PendingEdit;
        setText(p.text);
        requestIdRef.current = p.requestId;
        savedRef.current = false;
        setMatches([]);
        setMatchIdx(-1);
        setTimeout(() => taRef.current?.focus(), 0);
      });
    })();
    return () => {
      unlisten?.();
      // 兜底：未保存的卸载（X 关窗/系统关闭）发 cancel，防调用方监听悬空。
      if (!savedRef.current && requestIdRef.current) {
        emit("compact-editor://cancel", { requestId: requestIdRef.current });
      }
    };
  }, []);

  const charCount = [...text].length;

  const doSave = useCallback(async () => {
    if (!requestIdRef.current) return;
    savedRef.current = true;
    // await emit：跨窗口事件先发到后端再关窗（防 close 先于 emit 完成的竞态）；
    // catch 兜底——若 compact_editor_window 未被 capability 授权 allow-emit，emit 会 reject，
    // 显式打日志而非静默吞（曾因 ACL 缺失导致保存不回传且无报错，极难定位）。
    try {
      await emit("compact-editor://result", { requestId: requestIdRef.current, text });
    } catch (e) {
      console.error("compact-editor emit result 失败（检查 capability allow-emit）：", e);
    }
    invoke("close_compact_editor");
  }, [text]);

  const doCancel = useCallback(() => {
    if (requestIdRef.current) {
      savedRef.current = true; // 已显式发 cancel，别让 unmount 再发
      emit("compact-editor://cancel", { requestId: requestIdRef.current });
    }
    invoke("close_compact_editor");
  }, []);

  // ── 字号 ──
  const decFont = () => setFontSize((f) => Math.max(FONT_MIN, f - 1));
  const incFont = () => setFontSize((f) => Math.min(FONT_MAX, f + 1));
  useEffect(() => { localStorage.setItem(FONT_KEY, String(fontSize)); }, [fontSize]);

  // ── 撤销/重做：execCommand 触发 textarea 原生栈（Cmd+Z/Y 原生亦生效，作可靠兜底）──
  const undo = () => { taRef.current?.focus(); document.execCommand("undo"); };
  const redo = () => { taRef.current?.focus(); document.execCommand("redo"); };

  // ── 清空（二次确认）──
  const [clearPending, setClearPending] = useState(false);
  const clearAll = () => {
    if (!clearPending) { setClearPending(true); setTimeout(() => setClearPending(false), 2000); return; }
    setText(""); setClearPending(false); setMatches([]); setMatchIdx(-1);
  };

  // ── 查找/替换 ──
  const runFind = useCallback(() => {
    const q = findQuery;
    if (!q) { setMatches([]); setMatchIdx(-1); return; }
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
    setMatches(idxs);
    setMatchIdx(idxs.length > 0 ? 0 : -1);
    if (idxs.length > 0) selectRange(idxs[0], q.length);
  }, [findQuery, text]);

  const selectRange = (start: number, len: number) => {
    const ta = taRef.current;
    if (!ta) return;
    ta.focus();
    ta.setSelectionRange(start, start + len);
    const lineHeight = fontSize * 1.6;
    const lineNum = text.slice(0, start).split("\n").length;
    ta.scrollTop = Math.max(0, (lineNum - 2) * lineHeight);
  };

  useEffect(() => { if (showFind) runFind(); }, [runFind, showFind]);

  const gotoMatch = (delta: number) => {
    if (matches.length === 0) return;
    const next = (matchIdx + delta + matches.length) % matches.length;
    setMatchIdx(next);
    selectRange(matches[next], findQuery.length);
  };

  const replaceOne = () => {
    if (matchIdx < 0 || !findQuery) return;
    const start = matches[matchIdx];
    const next = text.slice(0, start) + replaceQuery + text.slice(start + findQuery.length);
    setText(next);
    setTimeout(runFind, 0);
  };

  const replaceAll = () => {
    if (!findQuery) return;
    setText(text.split(findQuery).join(replaceQuery));
    setTimeout(runFind, 0);
  };

  // ── 快捷键 ──
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      // IME 组字期（中文/日文输入法）放行所有快捷键：让 Esc 等键交给 IME 取消候选词，
      // 而非误触 doCancel 关窗丢文本。
      if (e.isComposing || e.keyCode === 229) return;
      const mod = e.metaKey || e.ctrlKey;
      if (mod && e.key === "Enter") { e.preventDefault(); doSave(); return; }
      if (e.key === "Escape") {
        if (showFind) { setShowFind(false); return; }
        doCancel(); return;
      }
      if (mod && e.key.toLowerCase() === "f") { e.preventDefault(); setShowFind(true); return; }
    };
    document.addEventListener("keydown", onKey);
    return () => document.removeEventListener("keydown", onKey);
  }, [doSave, doCancel, showFind]);

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
      {/* 工具栏 */}
      <div className="flex-shrink-0 flex items-center gap-0.5 px-2 py-1.5 border-b border-border bg-stone-50">
        <ToolBtn onClick={undo} title="撤销 (Cmd+Z)"><Undo2 className="w-4 h-4" /></ToolBtn>
        <ToolBtn onClick={redo} title="重做 (Cmd+Shift+Z)"><Redo2 className="w-4 h-4" /></ToolBtn>
        <span className="w-px h-4 bg-stone-200 mx-1" />
        <ToolBtn onClick={decFont} title="缩小字号" disabled={fontSize <= FONT_MIN}><ZoomOut className="w-4 h-4" /></ToolBtn>
        <span className="text-[11px] text-stone-500 w-7 text-center tabular-nums">{fontSize}</span>
        <ToolBtn onClick={incFont} title="放大字号" disabled={fontSize >= FONT_MAX}><ZoomIn className="w-4 h-4" /></ToolBtn>
        <span className="w-px h-4 bg-stone-200 mx-1" />
        <ToolBtn onClick={() => setShowFind((v) => !v)} title="查找/替换 (Cmd+F)"><Search className="w-4 h-4" /></ToolBtn>
        <ToolBtn onClick={clearAll} title="清空">
          {clearPending ? <Check className="w-4 h-4 text-red-500" /> : <Eraser className="w-4 h-4" />}
        </ToolBtn>
        <div className="flex-1" />
        <span className="text-[11px] text-stone-400 mr-2 tabular-nums">{charCount} 字</span>
        <button
          type="button"
          onClick={doCancel}
          className="flex items-center gap-1 px-2.5 py-1 rounded-md text-xs text-stone-600 hover:bg-stone-200 transition-colors"
        >
          <X className="w-3.5 h-3.5" /> 取消
        </button>
        <button
          type="button"
          onClick={doSave}
          className="flex items-center gap-1 px-2.5 py-1 rounded-md text-xs text-white bg-[#007aff] hover:bg-[#0066d6] transition-colors"
        >
          <Save className="w-3.5 h-3.5" /> 保存
          <span className="text-[10px] opacity-70">⌘↵</span>
        </button>
      </div>

      {/* 查找/替换条 */}
      {showFind && (
        <div className="flex-shrink-0 flex flex-wrap items-center gap-1.5 px-2 py-1.5 border-b border-border bg-stone-100">
          <input
            autoFocus
            value={findQuery}
            onChange={(e) => setFindQuery(e.target.value)}
            onKeyDown={(e) => { if (e.key === "Enter") gotoMatch(e.shiftKey ? -1 : 1); }}
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
            onChange={(e) => setReplaceQuery(e.target.value)}
            placeholder="替换"
            className="w-32 px-2 py-0.5 text-xs border border-stone-300 rounded bg-white outline-none focus:border-[#007aff]"
          />
          <button type="button" onClick={replaceOne} className="px-2 py-0.5 text-[11px] rounded border border-stone-300 hover:bg-stone-200">替换</button>
          <button type="button" onClick={replaceAll} className="flex items-center gap-0.5 px-2 py-0.5 text-[11px] rounded border border-stone-300 hover:bg-stone-200">
            <Replace className="w-3 h-3" /> 全替
          </button>
        </div>
      )}

      {/* 文本区 */}
      <textarea
        ref={taRef}
        value={text}
        onChange={(e) => setText(e.target.value)}
        style={{ fontSize: `${fontSize}px`, lineHeight: 1.6 }}
        spellCheck={false}
        className="flex-1 w-full resize-none outline-none p-4 bg-background text-foreground thin-scrollbar"
        placeholder="在此编辑…"
      />
    </div>
  );
}

export default CompactEditor;
