# Hotwords 段：多命中候选列表 + 用户可选

> **日期**：2026-08-02
> **状态**：🔜 待实现
> **依赖**：P3 bigram 上下文打分已完成（find_candidates 已返回排序候选）

## 1. 动机

当前 `correct_greedy` 多命中时自动选得分最高的替换。用户希望看到完整候选列表，手动选择。

## 2. 方案 A：correct 仍替换成第一个，段携带候选列表

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

`finalize_after_stop` 的 `apply_engine_full` 后调 `drain_candidates()`：
- 拿到 `(word, candidates)` 列表
- 在 transcript.segments 里找到 text == word 的段（可能有多个匹配——取第一个未标记的）
- 标记 kind = `Hotwords`，candidates = Some(candidates)

### 前端 CM6 渲染

AsrEditor 接收 segments（新增 prop），对 `hotwords` 段渲染下拉选择：
- 默认显示 text（第一个候选 = 得分最高）
- hover/click 展开候选列表
- 用户选择 → commit_edit（text 改成选中的，kind → Edited）
- 未选择保持 Hotwords（润色时 LLM 选）

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
