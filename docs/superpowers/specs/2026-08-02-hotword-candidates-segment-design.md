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
    pub candidates: Option<Vec<String>>,  // Hotwords 段的候选（最多 5 个，得分降序，含原词）
    pub id: Option<String>,               // Hotwords 段的 UUID（mark_hotwords 生成，前端装饰稳定标识）
}
```

### segments_json

```json
[
  { "kind": "raw", "text": "需要你修正这个" },
  { "kind": "hotwords", "text": "注释", "candidates": ["注释", "主意", "注意"], "id": "a1b2c3..." },
  { "kind": "raw", "text": "修复下面的错误。" }
]
```

`candidates`/`id` 是可选字段——其他 kind 不含（向后兼容旧 segments JSON）。
候选列表**含原词**（用户可选回原文）。

### corrector 层

`correct_greedy` 命中热词时（候选 > 1，含原词），除了替换成第一个，还收集完整候选列表：
- 新增 `pending_candidates: Mutex<Vec<(String, Vec<String>)>>`——(命中的词, 该位置全部候选)
- `find_candidates` 排序后 truncate(5)，**不排除原词**（用户可选回原文）
- 新增 `drain_candidates() -> Vec<(String, Vec<String>)>`
- 候选含原词（至少热词 + 原词 2 个）才收集

**候选含原词的设计理由**（方言归一场景）：方言规则（如 `r→l` + `si→ci`）让不同拼音
归一到同一 key。用户设热词"热词"(re-ci)，说带口音的"热词"被引擎识别成"乐视"(le-si)。
`find_candidates("乐视")` 按 `le-ci` 查（归一后）→ 命中热词"热词" → 候选 `["热词","乐视"]`
（热词 + 原词）。correct_greedy 替换成"热词"（纠正引擎误识别），但候选保留"乐视"——
万一用户真想说"乐视"（引擎识别正确），可选回去。这是方言纠错的正确效果。

### coordinator 层

**流式实时标记**（`apply_pipeline_events` 的 Emit 分支）：每帧 correct 后 `drain_candidates()`
→ 非空时 `transcript.mark_hotwords`（生成 UUID）→ 有 Hotwords 段就传 segments（`has_hotwords` 判定，
无新候选也保留装饰）。录音中即可见候选下拉，无需等 stop。

**stop 兜底**（`finalize_after_stop`）：同样 drain_candidates + mark_hotwords，覆盖流式
未 drain 的残留（如 close 时引擎 end-of-stream 纠正）。

`mark_hotwords`：在 transcript.segments 里找**含 word 子串**的段（流式/VadSegmented 场景段是整句，
word 是单个词，精确匹配 text==word 永远匹配不到）。子串劈段：含 word 的段劈成
`[前缀(原kind)] + [word(Hotwords)] + [后缀(原kind)]`。已标 Hotwords/Edited 跳过（用户已选定）。

### 前端 CM6 渲染（Task 4 已实现，经多轮迭代定型）

**传输**：扩展 `update-result`/`show-result` payload 加可选 `segments` 字段（`Option<&str>`）。
- 流式 tick 有新候选时传 `segments_json()`（实时标记），无候选传 `None`（零开销）；
  stop 后 `finalize_after_stop` 的 `show_result` 传 `transcript.segments_json()`；
  Idle 编辑 / polish 后的 `update_result` 也传 segments。
- `show-result` 原 bare string payload 改 object `{ text, segments }`（前端 handler 兼容旧 bare string）。

**Segment UUID 稳定标识**（关键演进）：`Segment` 加 `id: Option<String>` 字段，`mark_hotwords`
劈段时 `uuid::Uuid::new_v4()` 生成。`segments_json`/`restore_segments` 序列化/解析 id。
UUID 是装饰生命周期的核心——不依赖位置/段 index/word 文本，中插/追加/段重建都不影响已生成 id。

**渲染**（map 主导 + UUID 标识，类比富文本粗体）：
- CM6 `StateField<DecorationSet>` + `Decoration.mark`（`.cm-hotword` 波浪下划线 + voice 色）。
- 装饰带 `data-hw-id` 属性（UUID）。`decos.map(tr.changes)` 主导——用户编辑/中插/追加时装饰自动跟随。
- `setHotwords` StateEffect 智能合并（非全量替换）：已有 id 保留 map 后 offset、新 id 追加、消失的 id filter 清。
- `removeHotwordById` StateEffect：选定候选时按 UUID 精确清除该装饰（即时反馈，不等后端）。
- 失配（segments 拼接 != doc，如 writeDoc diverted 300ms 延迟期间）→ 跳过 dispatch 保留已有装饰。
  `refreshHotwords` 在 writeDoc 后（append + diverted 两路径）补算——doc 同步后重算。

**点击查候选**（按 UUID，不按位置/时序）：点击 `.cm-hotword` → 读 `data-hw-id` →
`findCandidatesById(segments, id)` 在 segments 按 UUID 查候选（单一真相源，不在 DOM 存数据）。
从 StateField 读精确 `[from,to]` + `coordsAtPos` 算屏幕坐标。

**下拉浮层**（React，横向排列）：候选词横向 flex 一行铺开（`·` 分隔），第一个用 voice 色 + 加粗区分。
- 自适应定位：右侧空间不够 → 左移贴右边缘；下方空间不够 → 向上弹（防溢出折叠）。
- 外部点击 / Esc 关闭。

**选词**：复用 `commit_edit`（不新增 IPC）——`selectCandidate` → `removeHotwordById`（清该装饰）+
`view.dispatch({changes: 替换})` + `addDirtyRange` + `doCommit()`。候选含原词（用户可选回原文）。

**rebuild_segments 保留 clean 区域 id**（关键修复）：`commit_edit` 走 `rebuild_segments` 字符级重建，
加了 `old_cands`/`old_ids` 按字符映射——dirty 区间标 Edited（丢弃 id），clean 区域的 hotword 段
经 `push_or_merge_full` 恢复 candidates/id（同 id 合并）。否则选定某个后其余 hotword 段 id 全丢 →
前端装饰全清。

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
