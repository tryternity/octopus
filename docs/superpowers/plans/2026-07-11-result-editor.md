# ASR 结果框 CM6 改造实施计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [x]`) syntax for tracking.

**Goal:** 将 Result 窗口的 contentEditable div 替换为 CodeMirror 6，实现「始终可编辑 + 随说随编」+ dirty ranges 精确标记 Edited segment。

**Architecture:** 新建 `AsrEditor.tsx`（CM6 纯文本编辑器 + ASR 流式适配 + dirty ranges + 编辑态拦截 + idle 恢复 + 光标/选区通知），替换 contentEditable + 手写光标系统。后端 `commit_edit` 按 dirty ranges 劈段标 Edited。

**Tech Stack:** CodeMirror 6（`@codemirror/*`，已在 package.json 中）、React 19、Tauri 2、vitest + jsdom、Rust

## Global Constraints

- **语言**：代码注释、commit message、文档用中文
- **测试框架**：前端 vitest + jsdom（`crates/desktop/frontend/`），后端 `cargo test`
- **前端路径**：`crates/desktop/frontend/src/`
- **后端路径**：`crates/desktop/src/` + `crates/infra/src/`（transcript.rs 在 desktop crate 中）
- **CSS 变量体系**：`--color-foreground` / `--color-muted` / `--color-border` / `--color-voice` / `--color-surface` / `--color-tool-icon`
- **commit 格式**：`feat:`/`fix:`/`refactor:`/`chore:`/`test:` 前缀
- **CM6 依赖已在 package.json**：markdown-editor 分支已合入 main，`@codemirror/*` 全部就绪
- **vitest alias**：`@` → `src/`（已在 `vitest.config.ts` 配置）

---

## 文件结构总览

### 新建文件

| 文件 | 职责 |
|------|------|
| `crates/desktop/frontend/src/pages/Result/AsrEditor.tsx` | CM6 + ASR 流式适配 + dirty ranges + 编辑态 + idle 恢复 |
| `crates/desktop/frontend/src/pages/Result/AsrEditor.test.ts` | AsrEditor 逻辑测试 |

### 修改文件

| 文件 | 改动 |
|------|------|
| `crates/desktop/src/transcript.rs` | `commit_edit` 加 `dirty_ranges` 参数 + `rebuild_segments` + `segment_kind_at_offset` + `push_or_merge` |
| `crates/desktop/src/coordinator.rs` | `CommitEdit` 命令加 `dirty_ranges` + `caret` + `selection`；移除 `CancelEdit` / `update_edit_buffer` |
| `crates/desktop/src/main.rs` | 移除 `update_edit_buffer` / `exit_edit_without_commit` 命令注册 |
| `crates/desktop/frontend/src/pages/Result/index.tsx` | 移除 ~350 行编辑态/光标/选区代码，接入 AsrEditor |

### 删除文件

| 文件 | 理由 |
|------|------|
| `crates/desktop/frontend/src/pages/Result/caret.ts` | CM6 原生光标替代 |
| `crates/desktop/frontend/src/pages/Result/CaretBlink.tsx` | 同上 |

---

## Task 1: 后端 transcript.rs — commit_edit 按 dirty ranges 劈段

**Files:**
- Modify: `crates/desktop/src/transcript.rs`

**Interfaces:**
- Produces: `commit_edit(&mut self, flat: &str, dirty_ranges: &[(usize, usize)])` — 按 dirty ranges 劈段标 Edited，供 coordinator 调用
- Produces: `fn rebuild_segments(old_segments: &[Segment], new_flat: &str, dirty: &[(usize, usize)]) -> Vec<Segment>` — 纯函数
- Produces: `fn segment_kind_at_offset(segments: &[Segment], offset: usize) -> SegmentKind` — 纯函数
- Produces: `fn push_or_merge(result: &mut Vec<Segment>, kind: SegmentKind, text: &str)` — 纯函数

- [x] **Step 1: 编写后端测试（TDD RED）**

在 `transcript.rs` 的 `#[cfg(test)] mod tests` 中新增测试：

```rust
#[test]
fn commit_edit_with_dirty_ranges_marks_edited() {
    let mut t = Transcript::new(1, PolishMode::Disabled);
    t.append_segment("你好世界"); // 全 Raw
    // 用户在 offset 2 插入"朋友"，offset 5-7 改为"再见"
    t.commit_edit("你好朋友世再见", &[(2, 4), (5, 7)]);
    let segs = &t.segments;
    assert_eq!(segs.len(), 3); // Raw("你好") + Edited("朋友") + ... 混合
    // offset 0-2 "你好" → 保留 Raw
    assert_eq!(segs[0].kind, SegmentKind::Raw);
    assert_eq!(segs[0].text, "你好");
    // offset 2-4 "朋友" → Edited
    assert_eq!(segs[1].kind, SegmentKind::Edited);
    assert_eq!(segs[1].text, "朋友");
}

#[test]
fn commit_edit_empty_dirty_ranges_fallback_all_edited() {
    let mut t = Transcript::new(1, PolishMode::Disabled);
    t.append_segment("你好");
    t.commit_edit("你好", &[]); // 空 dirty → 整篇 Edited
    assert_eq!(t.segments.len(), 1);
    assert_eq!(t.segments[0].kind, SegmentKind::Edited);
}

#[test]
fn commit_edit_empty_text_clears_segments() {
    let mut t = Transcript::new(1, PolishMode::Disabled);
    t.append_segment("你好");
    t.commit_edit("", &[(0, 0)]); // 清空
    assert!(t.segments.is_empty());
}

#[test]
fn rebuild_segments_preserves_clean_kind() {
    use super::*;
    let old = vec![
        Segment { kind: SegmentKind::Raw, text: "AB".into() },
        Segment { kind: SegmentKind::Polished, text: "CD".into() },
    ];
    // dirty [2,4) → "CD" 标 Edited，前后 Raw 保留
    let result = rebuild_segments(&old, "ABCD", &[(2, 4)]);
    assert_eq!(result.len(), 2); // Raw("AB") + Edited("CD")（同 kind 合并后）
    assert_eq!(result[0].kind, SegmentKind::Raw);
    assert_eq!(result[0].text, "AB");
    assert_eq!(result[1].kind, SegmentKind::Edited);
    assert_eq!(result[1].text, "CD");
}

#[test]
fn rebuild_segments_multiple_dirty_ranges() {
    use super::*;
    let old = vec![
        Segment { kind: SegmentKind::Raw, text: "ABCDEF".into() },
    ];
    // dirty [1,2) + [4,5) → B 和 E 标 Edited
    let result = rebuild_segments(&old, "ABCDEF", &[(1, 2), (4, 5)]);
    // Raw("A") + Edited("B") + Raw("CD") + Edited("E") + Raw("F")
    assert_eq!(result.len(), 5);
    assert_eq!(result[0], Segment { kind: SegmentKind::Raw, text: "A".into() });
    assert_eq!(result[1], Segment { kind: SegmentKind::Edited, text: "B".into() });
    assert_eq!(result[2], Segment { kind: SegmentKind::Raw, text: "CD".into() });
    assert_eq!(result[3], Segment { kind: SegmentKind::Edited, text: "E".into() });
    assert_eq!(result[4], Segment { kind: SegmentKind::Raw, text: "F".into() });
}

#[test]
fn segment_kind_at_offset_correct() {
    use super::*;
    let segs = vec![
        Segment { kind: SegmentKind::Raw, text: "AB".into() },
        Segment { kind: SegmentKind::Polished, text: "CD".into() },
        Segment { kind: SegmentKind::Edited, text: "EF".into() },
    ];
    assert_eq!(segment_kind_at_offset(&segs, 0), SegmentKind::Raw);
    assert_eq!(segment_kind_at_offset(&segs, 1), SegmentKind::Raw);
    assert_eq!(segment_kind_at_offset(&segs, 2), SegmentKind::Polished);
    assert_eq!(segment_kind_at_offset(&segs, 3), SegmentKind::Polished);
    assert_eq!(segment_kind_at_offset(&segs, 4), SegmentKind::Edited);
    assert_eq!(segment_kind_at_offset(&segs, 5), SegmentKind::Edited);
    assert_eq!(segment_kind_at_offset(&segs, 99), SegmentKind::Raw); // 兜底
}

#[test]
fn push_or_merge_same_kind() {
    use super::*;
    let mut result = vec![Segment { kind: SegmentKind::Raw, text: "AB".into() }];
    push_or_merge(&mut result, SegmentKind::Raw, "CD"); // 同 kind → 合并
    assert_eq!(result.len(), 1);
    assert_eq!(result[0].text, "ABCD");
    push_or_merge(&mut result, SegmentKind::Edited, "EF"); // 不同 kind → 新段
    assert_eq!(result.len(), 2);
}
```

- [x] **Step 2: 运行测试验证失败**

```bash
cargo test -p octopus-desktop -- transcript::tests
```

Expected: FAIL — `rebuild_segments` / `segment_kind_at_offset` / `push_or_merge` 未定义；`commit_edit` 签名不匹配。

- [x] **Step 3: 实现辅助函数**

在 `transcript.rs` 中（`commit_edit` 上方）新增：

```rust
/// 查 char offset 在 segments 中对应的 kind。
fn segment_kind_at_offset(segments: &[Segment], offset: usize) -> SegmentKind {
    let mut acc = 0usize;
    for seg in segments {
        let len = seg.text.chars().count();
        if offset < acc + len {
            return seg.kind;
        }
        acc += len;
    }
    SegmentKind::Raw // 兜底
}

/// 同 kind 相邻段合并（减少碎片）。
fn push_or_merge(result: &mut Vec<Segment>, kind: SegmentKind, text: &str) {
    if text.is_empty() { return; }
    if let Some(last) = result.last_mut() {
        if last.kind == kind {
            last.text.push_str(text);
            return;
        }
    }
    result.push(Segment { kind, text: text.to_string() });
}

/// 按 dirty ranges 重建段列表。
/// dirty 区间内标 Edited；区间外从 old_segments 按 char offset 对齐保留原 kind。
fn rebuild_segments(
    old_segments: &[Segment],
    new_flat: &str,
    dirty: &[(usize, usize)],
) -> Vec<Segment> {
    let new_chars: Vec<char> = new_flat.chars().collect();
    let total = new_chars.len();
    let mut result = Vec::new();
    let mut pos = 0usize;

    for &(d_start, d_end) in dirty {
        if d_start > pos {
            let clean: String = new_chars[pos..d_start].iter().collect();
            let kind = segment_kind_at_offset(old_segments, pos);
            push_or_merge(&mut result, kind, &clean);
        }
        if d_end > d_start {
            let dirty_text: String = new_chars[d_start..d_end].iter().collect();
            push_or_merge(&mut result, SegmentKind::Edited, &dirty_text);
        }
        pos = d_end;
    }
    if pos < total {
        let rest: String = new_chars[pos..].iter().collect();
        let kind = segment_kind_at_offset(old_segments, pos);
        push_or_merge(&mut result, kind, &rest);
    }
    result
}
```

- [x] **Step 4: 改造 commit_edit**

将现有 `commit_edit` 替换为：

```rust
/// 提交编辑：按 dirty ranges 劈段，dirty 区域标 Edited，区间外保留原 kind。
/// dirty_ranges 为扁平 char offset 区间（左闭右开），已排序无重叠。
pub fn commit_edit(&mut self, flat: &str, dirty_ranges: &[(usize, usize)]) {
    self.pending_delete = None;
    self.selection_insert_offset = None;

    if flat.is_empty() {
        self.segments.clear();
        self.caret_gap = 0;
        return;
    }

    if dirty_ranges.is_empty() {
        self.segments = vec![Segment { kind: SegmentKind::Edited, text: flat.to_string() }];
        self.caret_gap = 1;
        return;
    }

    let old_segments = self.segments.clone();
    self.segments = rebuild_segments(&old_segments, flat, dirty_ranges);
    self.caret_gap = self.segments.len();
}
```

- [x] **Step 5: 运行测试验证通过**

```bash
cargo test -p octopus-desktop -- transcript::tests
```

Expected: 全部 PASS。

- [x] **Step 6: Commit**

```bash
git add crates/desktop/src/transcript.rs
git commit -m "feat(transcript): commit_edit 按 dirty ranges 劈段标 Edited"
```

---

## Task 2: 后端 coordinator.rs — CommitEdit 命令加 dirty_ranges + caret/selection

**Files:**
- Modify: `crates/desktop/src/coordinator.rs`
- Modify: `crates/desktop/src/main.rs`

**Interfaces:**
- Produces: `Command::CommitEdit { text: String, dirty_ranges: Vec<(usize, usize)>, caret: Option<usize>, selection: Option<(usize, usize)> }`
- Produces: `coordinator.commit_edit(text, dirty_ranges, caret, selection)` 方法签名
- Produces: Tauri 命令 `commit_edit(coordinator, text, dirty_ranges, caret, selection)`

- [x] **Step 1: 改造 Command enum**

找到 `crates/desktop/src/coordinator.rs` 的 `Command` enum，替换：

```rust
// 旧：
CommitEdit { text: String },
CancelEdit,

// 新：
CommitEdit { text: String, dirty_ranges: Vec<(usize, usize)>, caret: Option<usize>, selection: Option<(usize, usize)> },
```

> 移除 `CancelEdit` 变体（始终可编辑，无取消操作）。

- [x] **Step 2: 改造命令循环中的 handler**

找到命令循环中处理 `Command::CommitEdit` 的位置（约 L429），替换为：

```rust
Command::CommitEdit { text, dirty_ranges, caret, selection } => {
    commit_edit_apply(&mut stage, &text, &dirty_ranges, caret, selection, &app_handle);
    editing = false;
}
```

找到处理 `Command::CancelEdit` 的位置（约 L439），替换为：移除整个 `CancelEdit` match arm（包括其内部逻辑）。

- [x] **Step 3: 改造 commit_edit_apply 函数签名**

找到 `commit_edit_apply` 函数（约 L2137），替换为：

```rust
fn commit_edit_apply(
    stage: &mut Stage,
    text: &str,
    dirty_ranges: &[(usize, usize)],
    caret: Option<usize>,
    selection: Option<(usize, usize)>,
    app_handle: &tauri::AppHandle,
) {
    let transcript = match stage {
        Stage::Streaming { transcript, .. }
        | Stage::VadSegmented { transcript, .. }
        | Stage::WaitingCompletion { transcript, .. } => transcript,
        #[cfg(feature = "cloud")]
        Stage::CloudClosing { transcript, .. } => transcript,
        Stage::Idle => {
            let id = CURRENT_TRANSCRIPTION_ID.load(Ordering::Relaxed);
            if id <= 0 {
                debug!("commit_edit in Idle but no current_transcription_id — 跳过落库");
                return;
            }
            let mut t = Transcript::new(id, PolishMode::Disabled);
            t.commit_edit(text, dirty_ranges);
            // commit 后光标/选区恢复
            if let Some((start, end)) = selection { t.set_selection(start, end); }
            else if let Some(c) = caret { t.set_caret(c); }
            let segments = t.segments_json();
            if let Err(e) = get_db_sender().send(DbCommand::UpdateEditedSegments {
                id, text: text.to_string(), segments,
            }) {
                warn!("Queue DB UpdateEditedSegments (idle) failed: {}", e);
            }
            crate::result_window::update_result(app_handle, text, false, 0);
            info!("Edit committed in Idle (id={}, {} chars)", id, text.chars().count());
            return;
        }
        _ => {
            debug!("commit_edit ignored in non-active stage");
            return;
        }
    };
    transcript.commit_edit(text, dirty_ranges);
    if let Some((start, end)) = selection { transcript.set_selection(start, end); }
    else if let Some(c) = caret { transcript.set_caret(c); }
    if transcript.db_inserted() {
        let id = transcript.id;
        let segments = transcript.segments_json();
        if let Err(e) = get_db_sender().send(DbCommand::UpdateEditedSegments {
            id, text: text.to_string(), segments,
        }) {
            warn!("Queue DB UpdateEditedSegments failed: {}", e);
        }
    }
    crate::result_window::update_result(app_handle, &transcript.display_text(), false, 0);
    info!("Edit committed ({} chars, {} dirty ranges)", text.chars().count(), dirty_ranges.len());
}
```

- [x] **Step 4: 改造 Coordinator 方法签名**

找到 `impl Coordinator` 中的 `commit_edit` 方法（约 L586），替换为：

```rust
pub fn commit_edit(&self, text: String, dirty_ranges: Vec<(usize, usize)>, caret: Option<usize>, selection: Option<(usize, usize)>) {
    let tx = self.tx.lock();
    if tx.send(Command::CommitEdit { text, dirty_ranges, caret, selection }).is_err() {
        error!("Coordinator channel closed");
    }
}
```

移除 `cancel_edit` 方法（约 L593-598）和 `update_edit_buffer` 方法（约 L578-582）。

- [x] **Step 5: 改造 Tauri 命令**

找到 `#[tauri::command] pub fn commit_edit`（约 L685），替换为：

```rust
#[tauri::command]
pub fn commit_edit(
    coordinator: tauri::State<'_, Coordinator>,
    text: String,
    dirty_ranges: Vec<(usize, usize)>,
    caret: Option<usize>,
    selection: Option<(usize, usize)>,
) {
    coordinator.commit_edit(text, dirty_ranges, caret, selection);
}
```

移除 `#[tauri::command] pub fn update_edit_buffer`（约 L678-681）和 `#[tauri::command] pub fn exit_edit_without_commit`（约 L718-720）。

- [x] **Step 6: 更新 main.rs 命令注册**

在 `crates/desktop/src/main.rs` 的 `invoke_handler` 列表中，移除 `coordinator::update_edit_buffer` 和 `coordinator::exit_edit_without_commit`。

- [x] **Step 7: 编译验证**

```bash
cargo build -p octopus-desktop 2>&1 | tail -5
```

Expected: 编译通过。如果有 `CancelEdit` 残留引用，搜索并清除。

- [x] **Step 8: Commit**

```bash
git add crates/desktop/src/coordinator.rs crates/desktop/src/main.rs
git commit -m "refactor(coordinator): CommitEdit 加 dirty_ranges/caret/selection + 移除 CancelEdit/update_edit_buffer"
```

---

## Task 3: 前端 AsrEditor 组件

**Files:**
- Create: `crates/desktop/frontend/src/pages/Result/AsrEditor.tsx`

**Interfaces:**
- Produces: `<AsrEditor text={} caret={} expanded={} onCommit={} ref={} />` — `ref` 暴露 `commit()` 方法

- [x] **Step 1: 实现 AsrEditor.tsx**

```tsx
import { useEffect, useRef, useImperativeHandle, forwardRef } from "react";
import { Compartment, EditorState, Transaction, type ChangeSpec } from "@codemirror/state";
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

  // ── dirty ranges 维护 ──
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

  // ── idle 自动恢复 ──
  function resetIdleTimer() {
    if (idleTimerRef.current) clearTimeout(idleTimerRef.current);
    idleTimerRef.current = setTimeout(() => doCommit(), IDLE_TIMEOUT);
  }

  function clearDivertedTimer() {
    if (divertedTimerRef.current) { clearTimeout(divertedTimerRef.current); divertedTimerRef.current = null; }
    pendingDivertedRef.current = null;
  }

  // ── commit（暴露给外壳的 Cmd+Enter / 保存按钮 + idle timer）──
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

  // ── CM6 实例创建（仅 mount 一次）──
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
          // 非编辑态光标/选区变化 → 通知后端
          if (update.selectionSet && !editingRef.current && !update.docChanged) {
            const sel = update.state.selection.main;
            if (sel.from !== sel.to) {
              invoke("set_selection", { start: sel.from, end: sel.to });
            } else {
              invoke("set_caret", { offset: sel.head });
            }
          }
          // 用户编辑 → dirty ranges + 编辑态
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

  // ── 流式写入（text prop 变化 → dispatch 写入 CM6）──
  useEffect(() => {
    const view = viewRef.current;
    if (!view) return;
    if (editingRef.current) return; // 编辑态拦截

    const current = view.state.doc.toString();
    if (current === text) return;

    // diverted 延迟（引擎纠正早前文本 → 300ms 延迟整体替换）
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

  // ── expanded 变化 → reconfigure theme ──
  useEffect(() => {
    const view = viewRef.current;
    if (!view) return;
    view.dispatch({ effects: themeCompartment.current.reconfigure(buildTheme(expanded)) });
  }, [expanded]);

  // ── 写入 CM6 doc + 光标定位 ──
  function writeDoc(newText: string, caretPos?: number | null) {
    const view = viewRef.current;
    if (!view) return;
    const changes: ChangeSpec = { from: 0, to: view.state.doc.length, insert: newText };
    view.dispatch(
      caretPos != null
        ? { changes, selection: { anchor: caretPos }, scrollIntoView: true }
        : { changes, scrollIntoView: true }
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
```

- [x] **Step 2: 类型检查**

```bash
cd crates/desktop/frontend && npx tsc --noEmit 2>&1 | tail -5
```

Expected: 无类型错误。如 `Transaction` 导入有误，确认从 `@codemirror/state` 导入。

- [x] **Step 3: Commit**

```bash
git add crates/desktop/frontend/src/pages/Result/AsrEditor.tsx
git commit -m "feat(frontend): AsrEditor 组件——CM6 + ASR 流式适配 + dirty ranges + idle 恢复"
```

---

## Task 4: CSS 样式

**Files:**
- Modify: `crates/desktop/frontend/src/index.css`

- [x] **Step 1: 追加 AsrEditor 滚动条样式**

在 `index.css` 末尾追加：

```css
/* ── ASR 编辑器 CM6 ── */
.asr-cm-editor .cm-editor { height: 100%; }
.asr-cm-editor .cm-scroller {
  scrollbar-width: thin;
  scrollbar-color: rgba(128, 128, 128, 0.35) transparent;
}
.asr-cm-editor .cm-scroller::-webkit-scrollbar { width: 4px; }
.asr-cm-editor .cm-scroller::-webkit-scrollbar-track { background: transparent; }
.asr-cm-editor .cm-scroller::-webkit-scrollbar-thumb { background: rgba(128, 128, 128, 0.35); border-radius: 2px; }
.asr-cm-editor .cm-scroller::-webkit-scrollbar-thumb:hover { background: rgba(128, 128, 128, 0.6); }
.asr-cm-editor { scrollbar-width: thin; }
```

- [x] **Step 2: 构建验证**

```bash
cd crates/desktop/frontend && npx vite build 2>&1 | tail -3
```

Expected: 构建成功。

- [x] **Step 3: Commit**

```bash
git add crates/desktop/frontend/src/index.css
git commit -m "style: ASR 编辑器 CM6 滚动条"
```

---

## Task 5: Result/index.tsx 改造（接入 AsrEditor，移除编辑态/光标/选区代码）

**Files:**
- Modify: `crates/desktop/frontend/src/pages/Result/index.tsx`
- Delete: `crates/desktop/frontend/src/pages/Result/caret.ts`
- Delete: `crates/desktop/frontend/src/pages/Result/CaretBlink.tsx`

- [x] **Step 1: 更新 import**

移除不再需要的 import：

```tsx
// 删除：
import { flushSync } from "react-dom";
import { codePointOffsetTo, codePointOffsetBefore, placeCaretAtCodePoint } from "./caret";
import { CaretBlink } from "./CaretBlink";

// 新增：
import { AsrEditor, type AsrEditorHandle } from "./AsrEditor";
```

- [x] **Step 2: 移除编辑态/光标/选区 state 和 ref**

删除以下 state 声明：
- `editing` / `setEditing`
- `caretPos` / `setCaretPos`
- `caretPosRef`

删除以下 ref：
- `textRef`
- `editingRef`
- `displayedRef`
- `currentSelectionRef`
- `mouseDownOffsetRef`
- `pendingDiverted` / `divertedTimer`
- `editBufTimer`
- `editSnapshotRef`
- `stickToBottomRef`
- `rafScrollRef`

新增 ref：
```tsx
const asrEditorRef = useRef<AsrEditorHandle>(null);
const caretRef = useRef<number | null>(null);
const [asrEditorResetKey, setAsrEditorResetKey] = useState(0);
```

- [x] **Step 3: 移除编辑态函数和逻辑**

删除以下函数/逻辑块：
- `renderResultNow`（整个函数）
- `enterEdit` / `commitEdit` / `cancelEdit` / `toggleEdit`
- `updateEditBuffer` / `onTextInput`
- `handleTextMouseUp` / `clampRangeToContainer`
- `editingRef` 的 useEffect
- `caretPosRef` 的 useEffect
- `rafScrollRef` 的 cleanup useEffect
- `currentSelectionRef` 的 selectionchange/blur useEffect
- `global-edit-toggle` 的 listen useEffect
- `edit-force-exit` 事件 handler（在事件数组中）

- [x] **Step 4: 改造 update-result 事件 handler**

在事件数组中，替换 `update-result` handler：

```tsx
["update-result", (p) => {
  const payload = p as { text: string; insertion: boolean; caret: number };
  caretRef.current = payload.insertion ? payload.caret : null;
  setText(payload.text);
}],
```

- [x] **Step 5: 改造 show-result / clear-result / hide-result handler**

```tsx
["show-result", (p) => {
  const text = p as string;
  const isPlaceholder = text === "正在聆听…" || text === "正在聆听...";
  setVisible(true);
  setIsRecording(true);
  refreshActive();
  if (isPlaceholder) {
    setText("");
    caretRef.current = null;
    setAsrEditorResetKey(k => k + 1); // remount AsrEditor（清编辑态）
  } else {
    setText(text);
    caretRef.current = null;
  }
}],
["clear-result", () => {
  setText("");
  caretRef.current = null;
  setVisible(false);
  setIsRecording(false);
  setAsrEditorResetKey(k => k + 1);
}],
["hide-result", () => {
  setVisible(false);
  setIsRecording(false);
  setAsrEditorResetKey(k => k + 1);
}],
```

- [x] **Step 6: 改造 Esc 键**

```tsx
if (e.key === "Escape") {
  if (popupType) { setPopupType(null); return; }
  invoke("cancel_recording");
  win.hide();
  return;
}
```

移除 `matchShortcut(e, sc) → toggleEdit()`，改为：

```tsx
const sc = parseShortcut(toolbarState.edit_shortcut);
if (matchShortcut(e, sc)) {
  e.preventDefault();
  asrEditorRef.current?.commit();
}
```

- [x] **Step 7: 改造工具栏按钮**

移除编辑/取消编辑/保存的分支逻辑，改为单一保存按钮：

```tsx
const tools = [
  { id: "close", icon: "close", label: "关闭", onClick: () => invoke("discard_recording") },
  { id: "settings", icon: "settings", label: "系统设置", onClick: () => invoke("open_settings") },
  { id: "denoise", icon: "denoise", label: "降噪模式", active: toolbarState.denoise_mode !== 0, onClick: openDenoisePopup },
  { id: "polish", icon: "polish", label: "润色模式", active: toolbarState.polish_mode !== 0, onClick: openPolishPopup },
  { id: "polish-now", icon: "polish-now", label: "立即润色", disabled: polishLoading, onClick: polishNow },
  { id: "toggle-size", icon: (expanded ? "minimize" : "expand-edit") as IconName, label: expanded ? "缩小" : "放大", onClick: toggleExpand },
  { id: "save", icon: "save" as IconName, label: "保存", onClick: () => asrEditorRef.current?.commit() },
];
```

- [x] **Step 8: 替换 contentEditable div 为 AsrEditor**

找到 JSX 中的 `<div className="relative h-full">` 块（contentEditable + CaretBlink），替换为：

```tsx
<div className="relative h-full">
  <AsrEditor
    key={asrEditorResetKey}
    ref={asrEditorRef}
    text={text}
    caret={caretRef.current}
    expanded={expanded}
    onCommit={(payload) => {
      invoke("commit_edit", {
        text: payload.text,
        dirtyRanges: payload.dirtyRanges,
        caret: payload.caret ?? null,
        selection: payload.selection ?? null,
      });
    }}
  />
</div>
```

- [x] **Step 9: 删除 caret.ts 和 CaretBlink.tsx**

```bash
rm crates/desktop/frontend/src/pages/Result/caret.ts
rm crates/desktop/frontend/src/pages/Result/CaretBlink.tsx
```

- [x] **Step 10: 类型检查 + 构建**

```bash
cd crates/desktop/frontend && npx tsc --noEmit 2>&1 | tail -5 && npx vite build 2>&1 | tail -3
```

Expected: 无类型错误，构建成功。如有未使用变量/函数残留，清理。

- [x] **Step 11: Commit**

```bash
git add -A
git commit -m "refactor(frontend): Result 窗口接入 AsrEditor（移除 contentEditable + 手写光标系统 ~350 行）"
```

---

## Task 6: 集成验证 + 文档同步

**Files:**
- Modify: `docs/architecture.md`

- [x] **Step 1: 全量前端测试**

```bash
cd crates/desktop/frontend && npx vitest run 2>&1 | tail -10
```

Expected: 全部 PASS。

- [x] **Step 2: 全量后端测试**

```bash
cargo test -p octopus-desktop -- transcript 2>&1 | tail -10
```

Expected: 全部 PASS。

- [x] **Step 3: 前端构建**

```bash
cd crates/desktop/frontend && npx vite build 2>&1 | tail -3
```

Expected: 构建成功。

- [x] **Step 4: 后端编译**

```bash
cargo build -p octopus-desktop 2>&1 | tail -3
```

Expected: 编译通过。

- [x] **Step 5: 更新 architecture.md**

在 `docs/architecture.md` 的 `result_window` 描述段落更新：

- Result 窗口文本区从 contentEditable div + 手写光标系统（caret.ts 122 行 + CaretBlink.tsx 49 行）替换为 CodeMirror 6 纯文本编辑器（AsrEditor.tsx）
- 始终可编辑（无显式编辑态切换），用户输入即暂停 ASR（enter_edit_mode → drain_samples）
- 恢复方式：Cmd+Enter / 保存按钮 / 停止输入 2 秒自动 commit
- commit 携带 dirty ranges——后端按 ranges 劈段标 Edited，区间外保留原 kind（Raw/Polished）
- 中插（set_caret）+ 选中替换（set_selection）保留——CM6 非编辑态选区变化通知后端
- 移除 CancelEdit / update_edit_buffer / exit_edit_without_commit 命令

- [x] **Step 6: Commit**

```bash
git add docs/architecture.md
git commit -m "docs: 更新 architecture.md——ASR 结果框 CM6 改造"
```

---

## 手动验证清单（构建后执行）

- [x] ASR 录音中用户开始打字 → ASR 暂停（voice line 停止流动）
- [x] 用户输入文字 → 文字正常显示
- [x] Cmd+Enter → 编辑提交 → ASR 恢复 → 新 delta 从末尾追加
- [x] 停止输入 2 秒 → 自动提交恢复
- [x] 保存按钮 → 编辑提交恢复
- [x] 点击文本中间 → ASR delta 从该处插入（中插）
- [x] 拖选文本 → ASR delta 替换选中区域
- [x] 撤销（Cmd+Z）正常工作
- [x] 精简态/长篇态切换无布局错位
- [x] diverted 延迟正常（纠正文本 300ms 后更新）
- [x] DB 中 segments_json 正确反映 Edited/Raw/Polished 分布
- [x] Esc 取消录音正常（不再有编辑态取消中间层）

---

## 实现偏差记录

### 后端技术债（已清理）

- ~~**`edit-force-exit` emit**（coordinator.rs × 3）~~：已移除（前端已不监听）。
- ~~**`global-edit-toggle` emit**~~：已移除 emit，`trigger_global_edit` 仅保留 show+focus。
- ~~**`update_edit_buffer` 过时注释**~~：已更新。

### 前端

- **caret.test.ts 额外删除**：原 plan 未列出此文件（caret.ts 的测试），实际发现后一并删除。
- **caret.ts 中 `codePointOffsetTo` 被 clipboardNav.test.ts 引用**——实际检查无引用（已删文件全部无残留）。

### 代码审查修复（3 轮）

- **Bug 1: dirtyRanges 未 mapPos 映射** → 每次 `onUserEdit` 先 `mapDirtyRanges(update.changes)` 映射已有区间再 `addDirtyRange`
- **Bug 2: Idle 编辑 segments 全退化 Raw** → Idle 分支从 DB `restore_segments` 恢复 old_segments
- **Bug 3: 纯删除 → 空 dirtyRanges 退化全 Edited** → 加 `hasEdited` 标记（`has_edited=false` + 空 dirty → rebuild_segments 保留原 kind）
- **Bug 4: onCommit 闭包陈旧** → `onCommitRef` 替代 mount effect 闭包捕获
- **Bug 5: 编辑中润色覆盖** → `polishNow` 先调 `commit()`
- **Bug 6: diverted 定时器编辑态未清** → 进入编辑态时 `clearDivertedTimer()`
- **Bug 7: 拖选 IPC 泛滥** → `debouncedSelectionNotify` 100ms 防抖
- **Bug 8: rebuild_segments 边界 clamp** → dirty ranges clamp 到 `[0, total]` 防越界 Panic
- **Bug 9: hasEdited 重置顺序** → `doCommit` 先读 `hasEdited` 到局部变量再重置 ref
- **Bug 10: rebuild_segments clean 区段 kind 退化** → 从 `segment_kind_at_offset` 单一 kind → `append_clean_range` 子串匹配 → 最终改为**字符级 walk**（构建 old_flat 逐字符 kind 映射，clean 区域逐字符匹配跳过被删字符保留原 kind）
- **后端注释遗留**：coordinator.rs 中 `update_edit_buffer` 注释已过时（命令已移除），已清理

### 音频缓冲保留（编辑期间不丢字）

- 编辑期间 `drain_samples()` → `trim_buffer(5.0)`：不再丢弃全部音频，每 tick 仅保留最后 5 秒原始音频。idle commit 恢复后第一个 tick `drain_samples` 拿到这 5 秒送 ASR，VAD 自动截掉开头静音段——用户编辑后立即说话不丢字（满足"嘴比手快"场景）。新增 `SharedAudioState::trim_buffer(keep_seconds)` 方法（`Vec::drain` 丢头部保留尾部）。
