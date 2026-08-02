# Hotwords 段：多命中候选列表 + 用户可选

> **日期**：2026-08-02
> **状态**：✅ 已实现（后端 Task 1-3/5 + 前端 Task 4）
> **依赖**：P3 bigram 上下文打分已完成（find_candidates 已返回排序候选）

## 1. 动机

当前 `correct_greedy` 多命中时自动选得分最高的替换。用户希望看到完整候选列表，手动选择。

## 2. 设计：correct 仍替换成第一个，段携带候选列表

corrector 行为不变（替换成得分最高的），但额外收集候选列表传给 Transcript。Transcript 把命中替换的段标记为 `Hotwords` kind + 携带候选列表。前端渲染下拉选择。

**兜底保护**：候选列表最多 5 个（得分降序截断 `truncate(5)`）。

### Segment 扩展

```rust
pub enum SegmentKind { Raw, Polished, Edited, Hotwords }
pub struct Segment {
    pub kind: SegmentKind,
    pub text: String,
    pub candidates: Option<Vec<String>>,  // Hotwords 段的候选（最多 5 个，得分降序）
}
```

### segments_json

```json
[
  { "kind": "raw", "text": "需要你修正这个" },
  { "kind": "hotwords", "text": "注释", "candidates": ["注释", "主意", "注意"] },
  { "kind": "raw", "text": "修复下面的错误。" }
]
```

`candidates` 是可选字段——其他 kind 不含此字段（向后兼容旧 segments JSON）。

### corrector 层

`correct_greedy` 命中多候选时（> 1），除了替换成第一个，还收集完整候选列表：
- 新增 `pending_candidates: Mutex<Vec<(String, Vec<String>)>>`——(命中的词, 该位置全部候选)
- `find_candidates` 排序后 truncate(5)
- 新增 `drain_candidates() -> Vec<(String, Vec<String>)>`
- 单候选（== 1）不收集（无选择意义）

### coordinator 层

**流式实时标记**（`apply_pipeline_events` 的 Emit 分支）：每帧 correct 后 `drain_candidates()`
→ 非空时 `transcript.mark_hotwords` → `update_result` 传 segments。录音中即可见候选下拉，
无需等 stop。仅在有新候选时序列化 segments（无命中传 None，零开销）。

**stop 兜底**（`finalize_after_stop`）：同样 drain_candidates + mark_hotwords，覆盖流式
未 drain 的残留（如 close 时引擎 end-of-stream 纠正）。

`mark_hotwords`：在 transcript.segments 里找到 text == word 的段（可能有多个匹配——取第一个未标记的），
标记 kind = `Hotwords`，candidates = Some(candidates)。

### 前端 CM6 渲染（Task 4 已实现）

**传输**：扩展 `update-result`/`show-result` payload 加可选 `segments` 字段（`Option<&str>`）。
- 流式 tick 有新候选时传 `segments_json()`（实时标记），无候选传 `None`（零开销）；
  stop 后 `finalize_after_stop` 的 `show_result` 传 `transcript.segments_json()`；
  Idle 编辑 / polish 后的 `update_result` 也传 segments。
- `show-result` 原 bare string payload 改 object `{ text, segments }`（前端 handler 兼容旧 bare string）。

**渲染**：CM6 `StateField<DecorationSet>` + `Decoration.mark`（**非** WidgetType——避免 widget 内文本
不参与光标定位/搜索的复杂度）：
- `.cm-hotword` class：波浪下划线（`--color-voice` 色）+ hover 高亮 + `cursor: pointer`。
- `hotwords.ts::hotwordRanges(segments, doc)` 纯函数：遍历段累加 char offset 产 `[from, to, candidates]`。
  段拼接 != doc 时返回空（降级，防 offset 错位）。
- `setHotwords` StateEffect 从 React 推 ranges；`editingRef` 为真 / dropdown 打开时不重算（防编辑态错位 + 下拉闪烁）。

**下拉浮层**（React，非 CM6 widget）：点击 `.cm-hotword` → `view.posAtCoords` 定位段 →
`view.coordsAtPos(from)` 算屏幕坐标 → 绝对定位浮层。渲染候选列表（第一项标"推荐/Top"）。
外部点击 / Esc 关闭（对齐现有 popup close 模式）。

**选词**：复用 `commit_edit`（不新增 IPC）——`selectCandidate` → `view.dispatch({changes: 替换})` +
`addDirtyRange` + `doCommit()`。后端 `rebuild_segments` 自动把该段标 Edited、去掉 Hotwords。
即便选了 == 原文的第一个候选也提交（标 dirty 让后端标 Edited，润色时不再 `<候选>` 标记）。

**编辑态抑制**：用户键入时 `editingRef.current === true` → segments effect dispatch 空装饰
（防 hotwords offset 漂移错位）。提交后后端返回新 segments（该段已 Edited）自然恢复。

### regions_prompt（润色 LLM）

Hotwords 段用 `<候选1|候选2|候选3>` 标记（Edited 仍用 `{...}`）：
```
需要你修正这个{注释}<然后|如何|任何>修复下面的错误。
```

系统提示加规则：`<a|b|c>` 标记的是语音识别候选词，请根据上下文选择最合适的一个，去掉尖括号。

PolishRegion 加 `candidates: Option<Vec<String>>` 字段，`regions_prompt` 遇到时用 `<a|b|c>` 格式。

`rebuild_after_polish`：Hotwords 段在 LLM 输出里定位（选中的词），匹配到则标 Edited（用户/LLM 选定的），匹配不到则标 Polished。

## 3. 不在范围

- function calling（后续升级路径，当前用 `<...>` 标记作为 fallback）
- correct 热路径性能（候选收集只在多命中时触发）
- 候选列表持久化（segments_json 已含 candidates，天然持久化）
