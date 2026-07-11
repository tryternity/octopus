# ASR 结果框 CM6 改造设计

> 日期：2026-07-11
> 分支：`feature/result-editor`
> 状态：设计已确认，待编写实施计划

## 一、背景

当前 Result 窗口（792 行 `index.tsx` + 122 行 `caret.ts` + 49 行 `CaretBlink.tsx`）是手写的 contentEditable div + 自定义光标系统，核心痛点：

- **显式编辑态切换**——必须按快捷键/点按钮进入编辑，不能「随说随编」
- **contentEditable 与 React 冲突**——需 `flushSync` + imperative `textContent` + `key` remount 绕过
- **自建光标系统**——122 行 `caret.ts` 手写 code-point 偏移计算，WKWebView 反复踩坑
- **编辑粒度粗**——`commit_edit` 把整篇压成一条 `Edited` 段，无法区分「用户输入」和「ASR 识别」

本次改造将 contentEditable 替换为 CodeMirror 6，实现「始终可编辑 + 随说随编」，同时按 dirty ranges 精确标记用户编辑的 segment。

## 二、需求决策

| # | 决策项 | 选择 |
|---|--------|------|
| 1 | 编辑暂停触发 | 用户开始输入即暂停（前端 `updateListener` 拦截 + fire-and-forget `enter_edit_mode`） |
| 2 | segment 编辑粒度 | 插入文本标 Edited，选中替换的区域标 Edited，纯删除不改原段 kind |
| 3 | 编辑信息回传 | CM6 维护 dirty ranges `Array<[from, to]>`，commit 时传 `{ text, dirtyRanges, caret?, selection? }` |
| 4 | 流式更新竞态 | 前端 `editingRef` 拦截——用户输入即设 true，后续 `update-result` 不写入 CM6 |
| 5 | 编辑结束恢复 | Cmd+Enter / 保存按钮 / 停止输入 2 秒自动恢复（前端 idle 定时器） |
| 6 | 编辑期间麦克风 | 沿用现有 `drain_samples`（麦克风不停，丢弃不送 ASR），VAD 恢复后续迭代 |
| 7 | dirty ranges 数据结构 | 手动维护 `Array<[from, to]>`，每次 changes 用 `iterChangedRanges` 更新 + 排序合并 |
| 8 | 中插 + 选中替换 | 非编辑态 CM6 `selectionSet` → `set_caret` / `set_selection`；用户选中后输入 → 走 dirty range（不走 ASR 替换） |

## 三、整体架构

### 方案：新建 ASR 专用 CM6 组件（方案 B）

ASR 编辑器的需求与静态编辑器差异大（高频流式写入、dirty tracking、编辑/非编辑态切换、光标同步），不复用 CompactEditor 的 `CodeMirrorEditor`。

```
Result/index.tsx (外壳)
  ├── 工具栏（保留：关闭/设置/降噪/润色/立即润色/放大缩小/保存）
  ├── AsrEditor.tsx（新）← 替换 contentEditable div + CaretBlink
  │    ├── CM6 实例（纯文本，无 markdown extension）
  │    ├── 流式写入（update-result 事件 → dispatch changes）
  │    ├── dirty ranges 维护（updateListener 检测用户编辑）
  │    ├── 编辑态拦截（用户输入 → enter_edit_mode + 拦截流式）
  │    ├── idle 自动恢复（停止输入 2 秒 → commit）
  │    └── 非编辑态光标/选区通知（中插 + 选中替换）
  └── Popup / Toast / Voice line（不变）

删除：
  caret.ts (122 行) — CM6 原生光标替代
  CaretBlink.tsx (49 行) — CM6 原生光标替代
  enterEdit / commitEdit / cancelEdit (~70 行) — 始终可编辑，无显式切换
  handleTextMouseUp / clampRangeToContainer / mouseDownOffsetRef (~60 行) — CM6 原生选区
  renderResultNow 中的 flushSync + textContent hack (~20 行) — CM6 管理自己的 DOM
```

## 四、新增文件

| 文件 | 职责 | 行数估算 |
|------|------|----------|
| `pages/Result/AsrEditor.tsx` | CM6 + ASR 流式适配 + dirty ranges + 编辑态 + idle 恢复 | ~250 |

## 五、删除的文件

| 文件 | 行数 | 理由 |
|------|------|------|
| `pages/Result/caret.ts` | 122 | CM6 原生光标替代 |
| `pages/Result/CaretBlink.tsx` | 49 | 同上 |

## 六、修改的文件

| 文件 | 改动 |
|------|------|
| `pages/Result/index.tsx` | 移除 ~350 行编辑态/光标/选区代码，接入 AsrEditor |
| `transcript.rs::commit_edit` | 从 `(flat: &str)` 改为 `(flat: &str, dirty_ranges: &[(usize, usize)])`——按 dirty ranges 劈段标 Edited |
| `coordinator.rs::CommitEdit` | payload 加 `dirty_ranges: Vec<(usize, usize)>` |
| `coordinator.rs::commit_edit_apply` | 传 dirty_ranges 到 transcript |
| `coordinator.rs` Tauri 命令 `commit_edit` | 签名加 `dirty_ranges` 参数 |

## 七、AsrEditor 组件设计

### 7.1 Props 接口

```tsx
interface AsrEditorProps {
  text: string;              // 当前文本（外壳 state，由 update-result 事件驱动）
  caret?: number | null;     // 光标位置（来自 update-result payload）
  expanded: boolean;         // 精简态 vs 长篇态
  onCommit: (payload: AsrEditorCommit) => void;  // 编辑提交回调
}

interface AsrEditorCommit {
  text: string;
  dirtyRanges: [number, number][];
  caret?: number;              // 光标位置（无选区时）——后续 ASR delta 从此插入
  selection?: [number, number]; // 选区范围——后续 ASR delta 替换此区间
}

interface AsrEditorHandle {
  commit: () => void;  // 供 Cmd+Enter / 保存按钮调用（useImperativeHandle 暴露）
}
```

### 7.2 CM6 extensions（纯文本，无 markdown）

```tsx
const extensions = [
  history(),                        // 撤销/重做
  drawSelection(),
  EditorView.lineWrapping,          // 自动换行
  keymap.of([...defaultKeymap, ...historyKeymap, indentWithTab]),
  buildTheme(expanded),              // 主题（去行号、去 gutters，精简态 63px / 长篇态 100%）
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
    if (update.docChanged && isUserEdit(update)) {
      onUserEdit(update);
    }
  }),
];
```

> 不含 `markdown()`、`syntaxHighlighting()`、`search()`、`lineNumbers()`——ASR 窗口不需要。

### 7.3 用户编辑检测

```tsx
function isUserEdit(update: ViewUpdate): boolean {
  return update.transactions.some(tr =>
    tr.isUserEvent("input") || tr.isUserEvent("delete") || tr.isUserEvent("drop") || tr.isUserEvent("paste")
  );
}
```

程序写入（流式 dispatch）不带 userEvent annotation → `isUserEdit` 返回 false → 不触发编辑态。

### 7.4 流式写入（update-result → CM6 dispatch）

```tsx
useEffect(() => {
  const view = viewRef.current;
  if (!view) return;
  if (editingRef.current) return;  // 编辑态拦截

  const current = view.state.doc.toString();
  if (current === text) return;

  // diverted 延迟（引擎纠正早前文本 → 300ms 延迟整体替换）
  if (!text.startsWith(current)) {
    pendingDivertedRef.current = text;
    if (!divertedTimerRef.current) {
      divertedTimerRef.current = setTimeout(() => {
        divertedTimerRef.current = null;
        if (pendingDivertedRef.current) {
          writeDoc(pendingDivertedRef.current, caret);
          pendingDivertedRef.current = null;
        }
      }, 300);
    }
    return;
  }
  // 纯追加 / 中插 → 立即写入
  clearDivertedTimer();
  writeDoc(text, caret);
}, [text, caret]);

function writeDoc(newText: string, caret?: number | null) {
  const view = viewRef.current;
  if (!view) return;
  view.dispatch({
    changes: { from: 0, to: view.state.doc.length, insert: newText },
    selection: caret != null ? { anchor: caret } : undefined,
    scrollIntoView: true,
  });
}
```

### 7.5 dirty ranges 维护

```tsx
const dirtyRangesRef = useRef<Array<[number, number]>>([]);

function onUserEdit(update: ViewUpdate) {
  // 首次用户编辑 → 进入编辑态
  if (!editingRef.current) {
    editingRef.current = true;
    invoke("enter_edit_mode");
  }
  resetIdleTimer();

  update.changes.iterChangedRanges((_fromA, _toA, fromB, toB) => {
    if (toB > fromB) addDirtyRange(fromB, toB);  // 仅插入产生 dirty range
  });
}

function addDirtyRange(from: number, to: number) {
  const ranges = dirtyRangesRef.current;
  ranges.push([from, to]);
  ranges.sort((a, b) => a[0] - b[0]);
  // 合并相邻/重叠
  const merged: Array<[number, number]> = [];
  for (const [s, e] of ranges) {
    const last = merged[merged.length - 1];
    if (last && s <= last[1]) last[1] = Math.max(last[1], e);
    else merged.push([s, e]);
  }
  dirtyRangesRef.current = merged;
}
```

### 7.6 编辑态恢复（3 种路径）

```tsx
const idleTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);
const IDLE_TIMEOUT = 2000;

function resetIdleTimer() {
  if (idleTimerRef.current) clearTimeout(idleTimerRef.current);
  idleTimerRef.current = setTimeout(() => doCommit(), IDLE_TIMEOUT);
}

// useImperativeHandle 暴露 commit（供 Cmd+Enter / 保存按钮调用）
useImperativeHandle(ref, () => ({
  commit: doCommit,
}));

function doCommit() {
  if (!editingRef.current) return;
  if (idleTimerRef.current) { clearTimeout(idleTimerRef.current); idleTimerRef.current = null; }
  clearDivertedTimer();

  const view = viewRef.current;
  if (!view) return;
  const text = view.state.doc.toString();
  const dirtyRanges = [...dirtyRangesRef.current];
  const sel = view.state.selection.main;
  const caret = sel.from === sel.to ? sel.head : undefined;
  const selection = sel.from !== sel.to ? [sel.from, sel.to] as [number, number] : undefined;

  editingRef.current = false;
  dirtyRangesRef.current = [];

  onCommit({ text, dirtyRanges, caret, selection });
}
```

### 7.7 非编辑态光标/选区通知

CM6 `updateListener` 在 `selectionSet && !docChanged && !editingRef.current` 时：

- **折叠选区**（`from === to`）→ `invoke("set_caret", { offset: sel.head })` —— 后续 ASR delta 从该处插入
- **非折叠选区**（`from !== to`）→ `invoke("set_selection", { start: sel.from, end: sel.to })` —— 下个 delta 到达时删旧插新

用户选中后开始输入 → `isUserEdit=true` → 进入编辑态 → 不传 `set_selection`（用户手动覆盖走 dirty range）。

### 7.8 CM6 主题（精简态 vs 长篇态）

```tsx
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
    },
    ".cm-gutters": { display: "none" },  // ASR 窗口无行号
    ".cm-activeLine": { backgroundColor: "transparent" },
    "&.cm-focused": { outline: "none" },
    "&.cm-focused .cm-selectionBackground, ::selection, .cm-selectionBackground": {
      backgroundColor: "color-mix(in srgb, var(--color-voice, #d97706) 22%, transparent)",
    },
    ".cm-cursor": { borderLeftColor: "var(--color-voice, #d97706)" },
  }, { dark: false });
}
```

## 八、运行态状态机

| 态 | 触发 | CM6 行为 | 后端行为 | update-result 事件 |
|----|------|---------|---------|-------------------|
| **流式态** | ASR 录音中，用户未输入 | 接收 dispatch 写入 + 光标同步 | 正常 tick + emit | 前端写入 CM6 |
| **编辑态** | 用户开始输入（isUserEdit） | 接收用户编辑 + 维护 dirty ranges | drain_samples（麦克风不停） | 前端拦截（不写入） |
| **空闲态** | 录音已停/未开始，用户浏览 | 接收用户编辑（随时可编辑） | editing=false（无 drain） | 无事件到达 |

### 恢复时序

```
用户开始输入 → editingRef=true + invoke("enter_edit_mode") + dirty ranges + idle timer
    │
    │  update-result 到达 → 前端拦截（editingRef=true → return）
    │
    ├─ 恢复路径 1：Cmd+Enter ─────┐
    ├─ 恢复路径 2：保存按钮 ──────┤
    ├─ 恢复路径 3：idle 2秒 ──────┤
    │                            ↓
    └─→ doCommit():
         ├─ text + dirtyRanges + caret/selection
         ├─ editingRef=false + dirtyRangesRef=[]
         └─→ onCommit → invoke("commit_edit", { text, dirtyRanges, caret, selection })
              └─→ 后端 commit_edit（劈段标 Edited）+ set_caret/set_selection
                   └─→ editing=false → 恢复 tick → emit update-result → 前端恢复写入
```

## 九、后端改动

### 9.1 transcript.rs `commit_edit` 重构

```rust
/// 提交编辑：按 dirty ranges 劈段，dirty 区域标 Edited，区间外保留原 kind。
pub fn commit_edit(&mut self, flat: &str, dirty_ranges: &[(usize, usize)], has_edited: bool) {
    self.pending_delete = None;
    self.selection_insert_offset = None;

    if flat.is_empty() {
        self.segments.clear();
        self.caret_gap = 0;
        return;
    }

    if dirty_ranges.is_empty() {
        if has_edited {
            self.segments = vec![Segment { kind: SegmentKind::Edited, text: flat.to_string() }];
            self.caret_gap = 1;
        } else {
            // has_edited=false → 纯删除，用 rebuild_segments 重建（保留原 kind）
            let old_segments = self.segments.clone();
            self.segments = rebuild_segments(&old_segments, flat, &[]);
            self.caret_gap = self.segments.len();
        }
        return;
    }

    // 保存旧 segments 的 kind 映射（用于区间外保留原 kind）
    let old_segments = self.segments.clone();

    // 按 dirty ranges 重建 segments
    self.segments = rebuild_segments(&old_segments, flat, dirty_ranges);
    self.caret_gap = self.segments.len(); // 新语音从末尾追加
}
```

### 9.2 `rebuild_segments` 核心算法（字符级 walk）

> **设计演进**：原方案用 `segment_kind_at_offset(old_segments, pos)` 查单个 kind 代表整个 clean 区域——当 clean 跨多 segment 或删除导致偏移时取到错误 kind。后改为 `append_clean_range` 子串匹配——但删除中间字符后 clean 不是连续子串 → 匹配失败 → 文本损坏。最终方案：**字符级 walk**。

```rust
/// 按 dirty ranges 重建段列表。
/// 1. 构建 old_flat 逐字符 kind 映射（old_chars + old_kinds）
/// 2. 标记 new 中每个 char 是否在 dirty range 内
/// 3. walk：dirty 连续段标 Edited；clean 逐字符在 old_chars[old_idx..] 中
///    按序匹配（跳过被删字符），保留 old_kinds[old_idx] 作为 kind
/// dirty ranges 被 clamp 到 [0, total] 防越界。
fn rebuild_segments(old_segments, new_flat, dirty) -> Vec<Segment> {
    // old_flat + old_kinds 逐字符映射
    // is_dirty 标记 new 中 dirty chars
    // walk dirty→Edited，clean→逐字符匹配 old 保留 kind
}
```

**关键**：clean 区域的每个字符按序在 `old_chars[old_idx..]` 中匹配——遇到不匹配的字符（被删的）跳过 `old_idx`，直到找到匹配的字符，取 `old_kinds[old_idx]` 作为 kind。这样即使中间删除导致偏移也能正确保留剩余字符的 kind。

### 9.3 `restore_segments`（Idle 态从 DB 恢复）

```rust
/// 从 DB JSON 恢复 segments（Idle 态编辑时用——保留已有 Raw/Polished/Edited 标记）。
pub fn restore_segments(&mut self, json: &str) {
    // 解析 [{"kind":"raw|polished|edited","text":"..."}] → segments
    // 否则 Idle 态 commit_edit 的 old_segments 为空 → clean 区域全退化为 Raw
}
```

### 9.4 `push_or_merge`

```rust
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
```

### 9.3 coordinator.rs Command 改动

```rust
enum Command {
    // 旧：CommitEdit { text: String }
    CommitEdit { text: String, dirty_ranges: Vec<(usize, usize)> },
    // ...
}
```

`commit_edit_apply` 和 Tauri 命令同步加 `dirty_ranges` 参数。

### 9.4 commit 后的光标/选区恢复

`commit_edit_apply` 在写入 segments 后，根据 commit payload 的 caret/selection 设置后端状态：

```rust
fn commit_edit_apply(stage: &mut Stage, text: &str, dirty_ranges: &[(usize, usize)],
                     caret: Option<usize>, selection: Option<(usize, usize)>,
                     app_handle: &tauri::AppHandle) {
    // ... commit_edit(text, dirty_ranges) ...

    // 光标/选区恢复（后续 ASR delta 从此插入/替换）
    if let (Some(t), Some((start, end))) = (stage_transcript(&mut stage), selection) {
        t.set_selection(start, end);
    } else if let Some(t) = stage_transcript(&mut stage) {
        if let Some(c) = caret {
            t.set_caret(c);
        }
    }
}
```

## 十、移除的命令/逻辑

| 移除 | 理由 |
|------|------|
| `exit_edit_without_commit` 命令 | 不再需要——没有显式「取消编辑」操作 |
| `update_edit_buffer` 命令 | 不再需要——编辑期间不实时推送，commit 时一次性传 dirtyRanges |
| `CancelEdit` 命令 + `edit-force-exit` emit | 始终可编辑，无显式取消；前端已不监听 |
| 前端 `enterEdit` / `commitEdit` / `cancelEdit` / `toggleEdit` | 始终可编辑 |
| 前端 `handleTextMouseUp` / `clampRangeToContainer` | CM6 原生选区 |
| 前端 `CaretBlink` 组件 | CM6 原生光标 |
| 前端 `renderResultNow` 的 flushSync + textContent | CM6 管理自己的 DOM |
| 前端 `global-edit-toggle` listen | 无显式编辑态切换 |
| 后端 `global-edit-toggle` emit | 已移除——`trigger_global_edit` 仅保留 show+focus |
| `segment_kind_at_offset` 函数 | 字符级 walk 替代后已删除 |

> **保留**：`enter_edit_mode` 命令——后端需要此信号切 `editing=true` → `drain_samples`。调用时机从「用户显式按快捷键」变为「CM6 检测到首次用户输入」。
>
> **保留**：`set_caret` / `set_selection` 命令——CM6 非编辑态光标/选区变化时调用，中插和选中替换功能不变。
>
> **保留**：`edit_shortcut` 配置——含义从「进入/退出编辑态」变为「提交编辑（commit + 恢复 ASR）」。

## 十一、错误处理与边界情况

### 11.1 竞态窗口

用户开始输入到后端收到 `enter_edit_mode` 之间（< 1 tick = 100-200ms），可能有 1-2 个 `update-result` 事件到达。这些事件被前端 `editingRef=true` 拦截（不写入 CM6），后端暂停后不再有新帧。

### 11.2 空文本编辑

用户清空全部文本 → `dirtyRanges` 为空（纯删除无插入）→ `commit_edit` 走 `dirty_ranges.is_empty()` 分支 → 整篇压成一条 Edited（空串清空 segments）。

### 11.3 窗口尺寸切换

精简态（520×116）→ 长篇态（720×480）。CM6 通过 `buildTheme(expanded)` 的 `height` 属性适配。`expanded` 变化时用 Compartment reconfigure theme（同 CompactEditor 的 fontSize Compartment 模式）。

### 11.4 编辑态中录音被取消（Esc / 关闭按钮）

后端收到 `cancel_recording` → coordinator 清 editing → emit `clear-result` / `hide-result`。前端清空 text state。AsrEditor 的 `text` prop 变为空串 → `useEffect` 检测 `editingRef.current=true` 时 `clear-result` handler 先从外部强制重置：外壳在 `clear-result` / `hide-result` / `show-result` 事件中递增 `asrEditorResetKey` state，AsrEditor 用 `key={resetKey}` remount（编辑态 ref/dirty/idle 全部清零，全新实例）。

### 11.5 diverted 延迟期间用户开始编辑

diverted 定时器 pending 中用户开始输入 → `editingRef=true`。定时器到期后 `writeDoc` 检查 `editingRef.current` → return（编辑态拦截）。diverted 文本被丢弃——后端已暂停不会有新 delta，下一个恢复的 tick 会推最新全量。

## 十二、不引入的功能（YAGNI）

| 功能 | 理由 | 后续扩展路径 |
|------|------|-------------|
| VAD 编辑期检测恢复 | 后端 drain 路径需跑 VAD，增加复杂度 | drain_samples 时送 VAD，检测到语音即 editing=false |
| 查找替换 | ASR 窗口场景不需要 | CM6 `search()` extension（按需加） |
| 行号 | ASR 窗口太小 | CM6 `lineNumbers()`（按需加） |
| 语法高亮 | 纯文本不需要 | CM6 `markdown()` extension（按需加） |

## 十三、测试策略

### 13.1 前端单元测试（vitest）

- `rebuild_segments` 逻辑（纯函数抽到 `lib/segmentRebuild.ts`）：给定 old_flat + old_segments + new_flat + dirty_ranges → 验证重建后的 segments kind 分布

### 13.2 后端单元测试

- `transcript.rs::commit_edit(flat, dirty_ranges, has_edited)`：验证按 dirty ranges 劈段 + kind 标记
- `rebuild_segments`：字符级 walk 验证（全 Edited / 全 clean / 混合 dirty + clean / 中间删除偏移）
- `push_or_merge`：同 kind 合并

### 13.3 手动验证

- ASR 录音中用户开始打字 → ASR 暂停（voice line 停止流动）
- 用户输入文字 → 文字正常显示
- Cmd+Enter → 编辑提交 → ASR 恢复 → 新 delta 从末尾追加
- idle 2 秒 → 自动提交恢复
- 点击文本中间 → ASR delta 从该处插入（中插）
- 拖选文本 → ASR delta 替换选中区域
- 撤销（Cmd+Z）正常工作
- 精简态/长篇态切换无布局错位
- diverted 延迟正常（纠正文本 300ms 后更新）
- DB 中 segments_json 正确反映 Edited/Raw/Polished 分布
