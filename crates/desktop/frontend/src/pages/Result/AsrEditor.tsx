import { useEffect, useRef, useImperativeHandle, forwardRef, useState, useCallback } from "react";
import { Compartment, EditorState, StateEffect, StateField, type Transaction, type ChangeSpec, Prec } from "@codemirror/state";
import { EditorView, keymap, drawSelection, Decoration, type DecorationSet } from "@codemirror/view";
import { defaultKeymap, history, historyKeymap, indentWithTab } from "@codemirror/commands";
import { invoke } from "@tauri-apps/api/core";
import { hotwordRanges, type Segment } from "./hotwords";
import { useT } from "@/lib/i18n";

const IDLE_TIMEOUT = 2000;
const DIVERTED_DELAY_MS = 300;

/** 推入 hotwords 段定位（[from,to,candidates] 列表），驱动 hotwordsField 重建 DecorationSet。 */
const setHotwords = StateEffect.define<Array<{ from: number; to: number; candidates: string[] }>>();

export interface AsrEditorCommit {
  text: string;
  dirtyRanges: [number, number][];
  hasEdited: boolean;         // 用户是否编辑过（纯删除也 true，防后端退化全 Edited）
  caret?: number;
  selection?: [number, number];
}

export interface AsrEditorHandle {
  commit: () => void;
  getText: () => string;
}

interface AsrEditorProps {
  text: string;
  /** 后端 segments（含 hotwords 候选）。null = 无段信息（流式 / placeholder），不渲染装饰。 */
  segments?: Segment[] | null;
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
    // hotwords 段：波浪下划线 + voice 色 + 可点光标。点击展开候选下拉浮层。
    ".cm-hotword": {
      textDecoration: "underline wavy var(--color-voice, #d97706)",
      textDecorationThickness: "1.5px",
      textUnderlineOffset: "3px",
      cursor: "pointer",
      borderRadius: "2px",
    },
    ".cm-hotword:hover": {
      backgroundColor: "color-mix(in srgb, var(--color-voice, #d97706) 12%, transparent)",
    },
  }, { dark: false });
}

/**
 * hotwords 段 Decoration StateField。
 * - create：空 DecorationSet。
 * - setHotwords effect：用新 ranges 重建（每段一个 Decoration.mark [from,to) class=cm-hotword）。
 * - doc 变化（用户编辑）：CM6 自动 map 装饰到新坐标；但 hotwords offset 由后端 segments 决定，
 *   编辑后会失配——故编辑态下 React 不再 dispatch setHotwords（editingRef gate），提交后后端
 *   返回新 segments 自然恢复。
 */
const hotwordsField = StateField.define<DecorationSet>({
  create: () => Decoration.none,
  update(decos, tr) {
    decos = decos.map(tr.changes);
    for (const e of tr.effects) {
      if (e.is(setHotwords)) {
        if (e.value.length === 0) {
          decos = Decoration.none;
        } else {
          decos = Decoration.set(e.value.map((r) =>
            Decoration.mark({ class: "cm-hotword" }).range(r.from, r.to),
          ), true);
        }
      }
    }
    return decos;
  },
  provide: (f) => EditorView.decorations.from(f),
});

export const AsrEditor = forwardRef<AsrEditorHandle, AsrEditorProps>(function AsrEditor(
  { text, segments, caret, expanded, onCommit },
  ref,
) {
  const t = useT();
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

  // segments ref——segments prop 变化时 dispatch setHotwords effect（mount effect 闭包读最新值）。
  const segmentsRef = useRef(segments);
  segmentsRef.current = segments;

  // 候选下拉浮层状态：点击 .cm-hotword 时打开，选中候选 / 外部点击 / Esc 时关闭。
  const [dropdown, setDropdown] = useState<{ from: number; to: number; candidates: string[]; left: number; top: number } | null>(null);

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

    const view0 = viewRef.current;
    const textLen = view0?.state.doc.length ?? -1;
    void invoke("perf_log_cmd", { msg: `[FE] doCommit text_len=${textLen}` });

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

  useImperativeHandle(ref, () => ({
    commit: doCommit,
    getText: () => viewRef.current?.state.doc.toString() ?? "",
  }));

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
        hotwordsField,
        // 点击 .cm-hotword → 打开候选下拉（domEventHandlers 在 capture 后冒泡，不干扰 selection）
        EditorView.domEventHandlers({ click: handleHotwordClick }),
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
              void invoke("perf_log_cmd", { msg: "[FE] enter_edit_mode invoked" });
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

  // segments prop 变化 → dispatch setHotwords effect（驱动 hotwordsField 重建装饰）。
  // 编辑态下不渲染（editingRef gate）：用户键入后段 offset 失配，装饰会错位；提交后后端
  // 返回新 segments（该段已 Edited）自然恢复。dropdown 打开时也不重算（避免下拉闪烁）。
  useEffect(() => {
    const view = viewRef.current;
    if (!view) return;
    if (editingRef.current || dropdown) {
      view.dispatch({ effects: setHotwords.of([]) });
      return;
    }
    const segs = segmentsRef.current;
    if (!segs) {
      view.dispatch({ effects: setHotwords.of([]) });
      return;
    }
    const doc = view.state.doc.toString();
    const ranges = hotwordRanges(segs, doc);
    view.dispatch({ effects: setHotwords.of(ranges) });
  }, [segments, dropdown]);

  // 点击 .cm-hotword → 打开候选下拉浮层。
  const handleHotwordClick = useCallback((event: MouseEvent) => {
    const view = viewRef.current;
    if (!view) return;
    const target = event.target as HTMLElement;
    if (!target.closest(".cm-hotword")) return;
    // CM6 posAtCoords 需要 {x, y}（客户端坐标，相对视口）
    const pos = view.posAtCoords({ x: event.clientX, y: event.clientY });
    if (pos == null) return;
    // 在当前 segments 里找包含 pos 的 hotwords 段
    const segs = segmentsRef.current;
    if (!segs) return;
    const doc = view.state.doc.toString();
    const ranges = hotwordRanges(segs, doc);
    const hit = ranges.find((r) => pos >= r.from && pos < r.to);
    if (!hit) return;
    event.preventDefault();
    event.stopPropagation();
    // 浮层定位：段起点的屏幕坐标（coordsAtPos 返回相对视口的 Rect）
    const coords = view.coordsAtPos(hit.from);
    if (!coords) return;
    // host 容器的视口偏移（浮层绝对定位相对 host）
    const hostRect = hostRef.current?.getBoundingClientRect();
    setDropdown({
      from: hit.from,
      to: hit.to,
      candidates: hit.candidates,
      left: coords.left - (hostRect?.left ?? 0),
      top: coords.bottom - (hostRect?.top ?? 0) + 2,
    });
  }, []);

  // 选中候选 → applyCandidate 替换 doc + 标 dirty + 立即 commit（走 commit_edit，后端 rebuild 标 Edited）。
  // 即便选了 == 原文的第一个候选，也提交（标 dirty 让后端 rebuild 该段为 Edited，润色时不再标 <候选>）。
  const selectCandidate = useCallback((candidate: string) => {
    const view = viewRef.current;
    const dd = dropdown;
    if (!view || !dd) return;
    setDropdown(null);
    // dispatch 替换 [from, to) → candidate（CM6 对相同文本的替换是 no-op changes，但 dirtyRange 仍标记）
    view.dispatch({
      changes: { from: dd.from, to: dd.to, insert: candidate },
      selection: { anchor: dd.from + candidate.length },
    });
    // 标 dirty + 进编辑态 + 立即 commit（复用现有提交流程）
    if (!editingRef.current) {
      editingRef.current = true;
      clearDivertedTimer();
      void invoke("perf_log_cmd", { msg: "[FE] enter_edit_mode invoked (hotword select)" });
      invoke("enter_edit_mode");
    }
    hasEditedRef.current = true;
    addDirtyRange(dd.from, dd.from + candidate.length);
    doCommit();
  }, [dropdown]);

  // 外部点击 / Esc 关闭下拉（对齐现有 popup close 模式）
  useEffect(() => {
    if (!dropdown) return;
    const onDown = (e: MouseEvent) => {
      const target = e.target as HTMLElement;
      if (!target.closest(".hotword-dropdown")) {
        setDropdown(null);
      }
    };
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") setDropdown(null);
    };
    document.addEventListener("mousedown", onDown);
    document.addEventListener("keydown", onKey);
    return () => {
      document.removeEventListener("mousedown", onDown);
      document.removeEventListener("keydown", onKey);
    };
  }, [dropdown]);

  return (
    <div
      ref={hostRef}
      className="asr-cm-editor relative"
      style={{
        height: expanded ? "100%" : "63px",
        width: "100%",
        overflow: "hidden",
      }}
    >
      {dropdown && (
        <div
          className="hotword-dropdown absolute z-40 min-w-[120px] max-w-[280px] rounded-md border border-black/[0.10] shadow-lg shadow-black/[0.12] py-0.5"
          style={{
            left: dropdown.left,
            top: dropdown.top,
            backgroundColor: "var(--color-surface)",
          }}
        >
          <div className="px-2.5 py-1 text-[10px] select-none" style={{ color: "var(--color-muted-foreground)" }}>
            {t("result.hotwords.tooltip")}
          </div>
          {dropdown.candidates.map((c, i) => (
            <div
              key={c + i}
              className="px-2.5 py-1 cursor-pointer text-[13px] flex items-center gap-1.5 transition-colors hover:bg-[#007aff]/[0.08]"
              style={{ color: "var(--color-foreground)" }}
              onMouseDown={(e) => { e.preventDefault(); e.stopPropagation(); selectCandidate(c); }}
            >
              {i === 0 && (
                <span className="text-[10px] shrink-0" style={{ color: "var(--color-voice, #d97706)" }}>
                  {t("result.hotwords.default")}
                </span>
              )}
              <span className="flex-1 min-w-0 truncate">{c}</span>
            </div>
          ))}
        </div>
      )}
    </div>
  );
});
