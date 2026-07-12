import { useEffect, useRef, useImperativeHandle, forwardRef } from "react";
import { Compartment, EditorState, type Transaction, type ChangeSpec, Prec } from "@codemirror/state";
import { EditorView, keymap, drawSelection } from "@codemirror/view";
import { defaultKeymap, history, historyKeymap, indentWithTab } from "@codemirror/commands";
import { invoke } from "@tauri-apps/api/core";

const IDLE_TIMEOUT = 2000;
const DIVERTED_DELAY_MS = 300;

export interface AsrEditorCommit {
  text: string;
  dirtyRanges: [number, number][];
  hasEdited: boolean;         // 用户是否编辑过（纯删除也 true，防后端退化全 Edited）
  caret?: number;
  selection?: [number, number];
}

export interface AsrEditorHandle {
  commit: () => void;
}

interface AsrEditorProps {
  text: string;
  caret?: number | null;
  expanded: boolean;
  onCommit: (payload: AsrEditorCommit) => void;
}

function isUserEdit(transactions: readonly Transaction[]): boolean {
  return transactions.some(tr =>
    tr.isUserEvent("input") || tr.isUserEvent("delete") || tr.isUserEvent("drop") || tr.isUserEvent("paste")
  );
}

function buildTheme(expanded: boolean) {
  return EditorView.theme({
    "&": {
      height: "100%",
      backgroundColor: "transparent",
      color: "var(--color-foreground)",
      fontFamily: "-apple-system, BlinkMacSystemFont, sans-serif",
      fontSize: "14px",
    },
    ".cm-scroller": {
      lineHeight: "1.6",
      padding: expanded ? "4px 14px 8px" : "4px 14px 4px",
      fontFamily: "inherit",
    },
    ".cm-gutters": { display: "none" },
    ".cm-activeLine": { backgroundColor: "transparent" },
    ".cm-activeLineGutter": { backgroundColor: "transparent" },
    "&.cm-focused": { outline: "none" },
    "&.cm-focused .cm-selectionBackground, ::selection, .cm-selectionBackground": {
      backgroundColor: "color-mix(in srgb, var(--color-voice, #d97706) 22%, transparent)",
    },
    ".cm-cursor, .cm-dropCursor": {
      borderLeftColor: "var(--color-voice, #d97706)",
      borderLeftWidth: "1.5px",
    },
    ".cm-line": { padding: "0" },
  }, { dark: false });
}

export const AsrEditor = forwardRef<AsrEditorHandle, AsrEditorProps>(function AsrEditor(
  { text, caret, expanded, onCommit },
  ref,
) {
  const hostRef = useRef<HTMLDivElement>(null);
  const viewRef = useRef<EditorView | null>(null);
  const themeCompartment = useRef(new Compartment());
  const editingRef = useRef(false);
  const dirtyRangesRef = useRef<Array<[number, number]>>([]);
  const hasEditedRef = useRef(false); // 纯删除也标记为已编辑（避免空 dirtyRanges 退化全 Edited）
  const idleTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  const pendingDivertedRef = useRef<string | null>(null);
  const divertedTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  const caretRef = useRef(caret);
  caretRef.current = caret;

  // onCommit ref——避免 mount effect 闭包陈旧
  const onCommitRef = useRef(onCommit);
  onCommitRef.current = onCommit;

  // selection IPC 防抖
  const selectionTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);

  // ── dirty ranges 维护 ──
  // 每次用户编辑后，用 CM6 changes.mapPos 映射已有 dirty ranges 到新坐标
  function mapDirtyRanges(changes: { mapPos: (pos: number, mode?: number) => number }) {
    const ranges = dirtyRangesRef.current;
    const mapped: Array<[number, number]> = [];
    for (const [s, e] of ranges) {
      const ns = changes.mapPos(s, -1); // MapMode.TrackDel — 保留被删除位置的映射
      const ne = changes.mapPos(e, 1);
      if (ns < ne) mapped.push([ns, ne]);
    }
    mapped.sort((a, b) => a[0] - b[0]);
    // 合并相邻
    const merged: Array<[number, number]> = [];
    for (const [s, e] of mapped) {
      const last = merged[merged.length - 1];
      if (last && s <= last[1]) last[1] = Math.max(last[1], e);
      else merged.push([s, e]);
    }
    dirtyRangesRef.current = merged;
  }

  function addDirtyRange(from: number, to: number) {
    if (from >= to) return;
    const ranges = dirtyRangesRef.current;
    ranges.push([from, to]);
    ranges.sort((a, b) => a[0] - b[0]);
    const merged: Array<[number, number]> = [];
    for (const [s, e] of ranges) {
      const last = merged[merged.length - 1];
      if (last && s <= last[1]) last[1] = Math.max(last[1], e);
      else merged.push([s, e]);
    }
    dirtyRangesRef.current = merged;
  }

  function resetIdleTimer() {
    if (idleTimerRef.current) clearTimeout(idleTimerRef.current);
    idleTimerRef.current = setTimeout(() => doCommit(), IDLE_TIMEOUT);
  }

  function clearDivertedTimer() {
    if (divertedTimerRef.current) { clearTimeout(divertedTimerRef.current); divertedTimerRef.current = null; }
    pendingDivertedRef.current = null;
  }

  function doCommit() {
    if (!editingRef.current) return;
    if (idleTimerRef.current) { clearTimeout(idleTimerRef.current); idleTimerRef.current = null; }
    clearDivertedTimer();

    const view = viewRef.current;
    if (!view) return;
    const docText = view.state.doc.toString();
    const dirtyRanges = [...dirtyRangesRef.current];
    const hasEdited = hasEditedRef.current;
    const sel = view.state.selection.main;
    const caretPos = sel.from === sel.to ? sel.head : undefined;
    const selectionRange = sel.from !== sel.to ? [sel.from, sel.to] as [number, number] : undefined;

    editingRef.current = false;
    hasEditedRef.current = false;
    dirtyRangesRef.current = [];

    onCommitRef.current({ text: docText, dirtyRanges, hasEdited, caret: caretPos, selection: selectionRange });
  }

  useImperativeHandle(ref, () => ({ commit: doCommit }));

  // ── selection IPC：折叠选区（点击定位）即时发送，非折叠（拖选）防抖 ──
  function notifySelection(start: number, end: number) {
    if (start === end) {
      // 纯点击——即时通知后端（中插定位需灵敏）
      if (selectionTimerRef.current) { clearTimeout(selectionTimerRef.current); selectionTimerRef.current = null; }
      invoke("set_caret", { offset: start });
    } else {
      // 拖选——防抖（拖动过程不逐帧 invoke）
      if (selectionTimerRef.current) clearTimeout(selectionTimerRef.current);
      selectionTimerRef.current = setTimeout(() => {
        selectionTimerRef.current = null;
        invoke("set_selection", { start, end });
      }, 100);
    }
  }

  useEffect(() => {
    if (!hostRef.current) return;

    const state = EditorState.create({
      doc: text,
      extensions: [
        history(),
        drawSelection(),
        EditorView.lineWrapping,
        // Cmd/Ctrl+Enter → commit（高优先级拦截，防止 defaultKeymap 插入换行）
        Prec.highest(keymap.of([{
          key: "Mod-Enter",
          preventDefault: true,
          run: () => { doCommit(); return true; },
        }])),
        keymap.of([...defaultKeymap, ...historyKeymap, indentWithTab]),
        themeCompartment.current.of(buildTheme(expanded)),
        EditorView.updateListener.of((update) => {
          // 非编辑态光标/选区变化 → 防抖通知后端
          if (update.selectionSet && !editingRef.current && !update.docChanged) {
            const sel = update.state.selection.main;
            notifySelection(sel.from, sel.to);
          }
          // 用户编辑 → dirty ranges + 编辑态
          if (update.docChanged && isUserEdit(update.transactions)) {
            if (!editingRef.current) {
              editingRef.current = true;
              clearDivertedTimer(); // 进入编辑态时清 diverted 定时器（防覆盖用户输入）
              invoke("enter_edit_mode");
            }
            hasEditedRef.current = true;
            resetIdleTimer();
            // 先映射已有 dirty ranges（把旧区间移到新坐标）
            mapDirtyRanges(update.changes);
            // 再加本次插入的新区间
            update.changes.iterChangedRanges((_fA: number, _tA: number, fB: number, tB: number) => {
              addDirtyRange(fB, tB);
            });
          }
        }),
      ],
    });

    const view = new EditorView({ state, parent: hostRef.current });
    viewRef.current = view;
    // 窗口启动即聚焦 CM6 显示光标（用户无需手动点击）
    view.focus();

    return () => {
      view.destroy();
      viewRef.current = null;
      if (idleTimerRef.current) clearTimeout(idleTimerRef.current);
      if (selectionTimerRef.current) clearTimeout(selectionTimerRef.current);
      clearDivertedTimer();
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  useEffect(() => {
    const view = viewRef.current;
    if (!view) return;
    if (editingRef.current) return;

    const current = view.state.doc.toString();
    if (current === text) return;

    if (!text.startsWith(current)) {
      pendingDivertedRef.current = text;
      if (!divertedTimerRef.current) {
        divertedTimerRef.current = setTimeout(() => {
          divertedTimerRef.current = null;
          if (pendingDivertedRef.current) {
            writeDoc(pendingDivertedRef.current, caretRef.current);
            pendingDivertedRef.current = null;
          }
        }, DIVERTED_DELAY_MS);
      }
      return;
    }
    clearDivertedTimer();
    writeDoc(text, caretRef.current);
  }, [text]);

  useEffect(() => {
    const view = viewRef.current;
    if (!view) return;
    view.dispatch({ effects: themeCompartment.current.reconfigure(buildTheme(expanded)) });
  }, [expanded]);

  function writeDoc(newText: string, caretPos?: number | null) {
    const view = viewRef.current;
    if (!view) return;
    const cur = view.state.doc.toString();
    const curLen = cur.length;
    // 流式追加快路径：新文本是旧文本的前缀扩展 → 只插尾部 O(delta)；
    // 否则（中插 / 润色重写 / 分支回退）走全量替换。改前是无条件全量替换，
    // 长文本 + 高频 emit 时 O(n) 重建是 Result 窗卡顿主嫌。
    const isAppend = newText.length > curLen && newText.startsWith(cur);
    const changes: ChangeSpec = isAppend
      ? { from: curLen, insert: newText.slice(curLen) }
      : { from: 0, to: curLen, insert: newText };
    const anchor = caretPos != null ? caretPos : newText.length;
    const t0 = performance.now();
    view.dispatch({ changes, selection: { anchor }, scrollIntoView: true });
    const dt = performance.now() - t0;
    // 阈值打点（dispatch 慢 或 文本已长）：事后翻 ~/.octopus/logs/asr.log 对账卡顿时刻。
    if (dt > 8 || newText.length > 800) {
      void invoke("perf_log_cmd", {
        msg: `[FE writeDoc] ${dt.toFixed(1)}ms total=${newText.length} delta=${newText.length - curLen} mode=${isAppend ? "append" : "full"}`,
      }).catch(() => {});
    }
  }

  return (
    <div
      ref={hostRef}
      className="asr-cm-editor"
      style={{
        height: expanded ? "100%" : "63px",
        width: "100%",
        overflow: "hidden",
      }}
    />
  );
});
