import { useEffect, useRef, useImperativeHandle, forwardRef, useState, useCallback } from "react";
import { Compartment, EditorState, StateEffect, StateField, type Transaction, type ChangeSpec, type Range, Prec } from "@codemirror/state";
import { EditorView, keymap, drawSelection, Decoration, type DecorationSet } from "@codemirror/view";
import { defaultKeymap, history, historyKeymap, indentWithTab } from "@codemirror/commands";
import { invoke } from "@tauri-apps/api/core";
import { hotwordRanges, findCandidatesById, type Segment } from "./hotwords";
import { cn } from "@/lib/utils";

const IDLE_TIMEOUT = 2000;
const DIVERTED_DELAY_MS = 300;

/** 推入 hotwords 段定位（[from,to,candidates,id] 列表），驱动 hotwordsField 合并 DecorationSet。
 * id = 后端生成的 UUID（稳定标识）。map 主导：已有 id 保留 map 后 offset，新 id 追加，消失的 filter 清除。 */
const setHotwords = StateEffect.define<Array<{ from: number; to: number; candidates: string[]; id: string }>>();

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

/** 清除指定 from 位置附近的 hotword 装饰（selectCandidate 选定后调用）。
 * 用 from 而非 segIndex——map 后位置变但 selectCandidate 知道点击的 from。 */
const removeHotwordAt = StateEffect.define<number>();

/**
 * hotwords 段 Decoration StateField（map 主导 + UUID 稳定标识）。
 *
 * 类比富文本粗体：hotword 装饰一旦建立就跟随内容移动（CM6 decos.map），
 * 编辑/中插/追加/段重建都不破坏它。
 *
 * UUID 稳定标识：后端 mark_hotwords 劈段时生成 UUID 存进 Segment.id，经 segments_json
 * 传到前端。装饰带 data-hw-id 属性。合并按 id（非位置/段 index/word 文本）：
 * - 已有 id 在新 ranges → 保留 map 后 offset（不被后端重算覆盖，防编辑跳位）
 * - 新 id（不在已有）→ 追加
 * - 已有 id 不在新 ranges → filter 清除（用户选定变 Edited / 后端确认移除）
 * removeHotwordAt：选定候选时按位置清除（即时反馈，不等后端）。
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
          const newIds = new Set(e.value.map((r) => r.id));
          const existingIds = new Set<string>();
          const iter = decos.iter();
          while (iter.value) {
            const id = (iter.value.spec as { attributes?: Record<string, string> })?.attributes?.["data-hw-id"];
            if (id) existingIds.add(id);
            iter.next();
          }
          // 新 id 追加；已有 id 保留 map 后 offset。
          const toAdd: Range<Decoration>[] = [];
          for (const r of e.value) {
            if (!existingIds.has(r.id)) {
              toAdd.push(
                Decoration.mark({
                  class: "cm-hotword",
                  attributes: { "data-hw": r.candidates[0] ?? "", "data-hw-id": r.id },
                }).range(r.from, r.to),
              );
            }
          }
          // 已有但不在新 ranges 的 id → filter 清除
          if (toAdd.length > 0 || existingIds.size > 0) {
            decos = decos.update({
              add: toAdd,
              filter: (_from, _to, deco) => {
                const id = (deco.spec as { attributes?: Record<string, string> })?.attributes?.["data-hw-id"];
                if (!id) return true;
                return newIds.has(id);
              },
            });
          }
        }
      } else if (e.is(removeHotwordAt)) {
        // 选定候选：清除包含该 from 位置的 hotword 装饰（即时反馈）
        const pos = e.value;
        decos = decos.update({
          filter: (from, to, deco) => {
            const isHotword = (deco.spec as { attributes?: Record<string, string> })?.attributes?.["data-hw-id"];
            if (!isHotword) return true;
            return !(pos >= from && pos < to);
          },
        });
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

  // 重算 hotwords 装饰：用最新 segments + doc 算 ranges，dispatch setHotwords。
  // 失配（segments 拼接 != doc）→ 跳过保留已有装饰。后端无 segments → 全清。
  // 被 segments effect 和 text effect（writeDoc 后）调用——解决 diverted 延迟期间
  // segments effect 失配跳过、doc 同步后需补算的时序问题。
  const refreshHotwords = useCallback(() => {
    const view = viewRef.current;
    if (!view) return;
    const segs = segmentsRef.current;
    if (!segs) {
      view.dispatch({ effects: setHotwords.of([]) });
      return;
    }
    const doc = view.state.doc.toString();
    const ranges = hotwordRanges(segs, doc);
    if (ranges.length === 0 && segs.some((s) => s.kind === "hotwords")) {
      return; // 失配，保留已有
    }
    view.dispatch({ effects: setHotwords.of(ranges) });
  }, []);

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
            refreshHotwords(); // doc 同步后重算装饰（diverted 延迟期间 segments effect 失配跳过了）
          }
        }, DIVERTED_DELAY_MS);
      }
      return;
    }
    clearDivertedTimer();
    writeDoc(text, caretRef.current);
    refreshHotwords(); // append 路径 doc 已同步，重算装饰
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

  // segments prop 变化 → refreshHotwords
  useEffect(() => { refreshHotwords(); }, [segments, refreshHotwords]);

  // 点击 .cm-hotword → 打开候选下拉浮层。
  // 从装饰 DOM 读 UUID（data-hw-id），用 UUID 去 segments（单一真相源）查 candidates——
  // 不依赖位置/时序：选定某个后该段变 Edited（id 丢），其余段 id 不变仍能查到。
  const handleHotwordClick = useCallback((event: MouseEvent) => {
    const view = viewRef.current;
    if (!view) return;
    const target = event.target as HTMLElement;
    const span = target.closest(".cm-hotword") as HTMLElement | null;
    if (!span) return;
    const id = span.getAttribute("data-hw-id");
    if (!id) return;
    const segs = segmentsRef.current;
    if (!segs) return;
    const candidates = findCandidatesById(segs, id);
    if (!candidates) return;
    // CM6 posAtCoords 定位点击的 char offset
    const pos = view.posAtCoords({ x: event.clientX, y: event.clientY });
    if (pos == null) return;
    // 从装饰 StateField 读该位置的精确 [from,to]（比 posAtCoords 更准）
    const decos = view.state.field(hotwordsField);
    let from = pos, to = pos + 1;
    decos.between(pos, pos, (f, t) => { from = f; to = t; });
    event.preventDefault();
    event.stopPropagation();
    const coords = view.coordsAtPos(from);
    if (!coords) return;
    const hostRect = hostRef.current?.getBoundingClientRect();
    setDropdown({
      from,
      to,
      candidates,
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
    // 清除该位置的 hotword 装饰（选定后变 Edited，不再显示下拉）
    view.dispatch({ effects: removeHotwordAt.of(dd.from) });
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
          className="hotword-dropdown absolute z-40 flex flex-row flex-wrap items-center gap-x-0.5 gap-y-0 max-w-[680px] rounded-md border border-black/[0.10] shadow-lg shadow-black/[0.12] px-1 py-0.5"
          style={{
            left: dropdown.left,
            top: dropdown.top,
            backgroundColor: "var(--color-surface)",
          }}
        >
          {dropdown.candidates.map((c, i) => (
            <div key={c + i} className="flex items-center gap-x-0.5">
              {i > 0 && (
                <span className="text-[12px] select-none" style={{ color: "var(--color-muted-foreground)" }}>·</span>
              )}
              <span
                className={cn(
                  "px-1.5 py-0.5 cursor-pointer text-[13px] rounded transition-colors hover:bg-[#007aff]/[0.08]",
                  i === 0 && "font-medium",
                )}
                style={{ color: i === 0 ? "var(--color-voice, #d97706)" : "var(--color-foreground)" }}
                onMouseDown={(e) => { e.preventDefault(); e.stopPropagation(); selectCandidate(c); }}
              >
                {c}
              </span>
            </div>
          ))}
        </div>
      )}
    </div>
  );
});
