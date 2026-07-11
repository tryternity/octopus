import { useEffect, useRef, useImperativeHandle, forwardRef } from "react";
import { Compartment, EditorState, type Transaction, type ChangeSpec } from "@codemirror/state";
import { EditorView, keymap, drawSelection } from "@codemirror/view";
import { defaultKeymap, history, historyKeymap, indentWithTab } from "@codemirror/commands";
import { invoke } from "@tauri-apps/api/core";

const IDLE_TIMEOUT = 2000;
const DIVERTED_DELAY_MS = 300;

export interface AsrEditorCommit {
  text: string;
  dirtyRanges: [number, number][];
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
  const idleTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  const pendingDivertedRef = useRef<string | null>(null);
  const divertedTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  const caretRef = useRef(caret);
  caretRef.current = caret;

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
    const sel = view.state.selection.main;
    const caretPos = sel.from === sel.to ? sel.head : undefined;
    const selectionRange = sel.from !== sel.to ? [sel.from, sel.to] as [number, number] : undefined;

    editingRef.current = false;
    dirtyRangesRef.current = [];

    onCommit({ text: docText, dirtyRanges, caret: caretPos, selection: selectionRange });
  }

  useImperativeHandle(ref, () => ({ commit: doCommit }));

  useEffect(() => {
    if (!hostRef.current) return;

    const state = EditorState.create({
      doc: text,
      extensions: [
        history(),
        drawSelection(),
        EditorView.lineWrapping,
        keymap.of([...defaultKeymap, ...historyKeymap, indentWithTab]),
        themeCompartment.current.of(buildTheme(expanded)),
        EditorView.updateListener.of((update) => {
          if (update.selectionSet && !editingRef.current && !update.docChanged) {
            const sel = update.state.selection.main;
            if (sel.from !== sel.to) {
              invoke("set_selection", { start: sel.from, end: sel.to });
            } else {
              invoke("set_caret", { offset: sel.head });
            }
          }
          if (update.docChanged && isUserEdit(update.transactions)) {
            if (!editingRef.current) {
              editingRef.current = true;
              invoke("enter_edit_mode");
            }
            resetIdleTimer();
            update.changes.iterChangedRanges((_fA: number, _tA: number, fB: number, tB: number) => {
              addDirtyRange(fB, tB);
            });
          }
        }),
      ],
    });

    const view = new EditorView({ state, parent: hostRef.current });
    viewRef.current = view;

    return () => {
      view.destroy();
      viewRef.current = null;
      if (idleTimerRef.current) clearTimeout(idleTimerRef.current);
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
    const changes: ChangeSpec = { from: 0, to: view.state.doc.length, insert: newText };
    view.dispatch(
      caretPos != null
        ? { changes, selection: { anchor: caretPos }, scrollIntoView: true }
        : { changes, scrollIntoView: true },
    );
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
