# ASR 光标定位与中间插入 — 设计

- 日期：2026-07-03
- 状态：设计中（待审阅）
- worktree / 分支：`clean-used-feature` / `worktree-clean-used-feature`（分支名无关，编码亦在此）
- 相关代码：
  - `crates/desktop/src/transcript.rs`（核心数据结构，重构）
  - `crates/desktop/src/coordinator.rs`（编排：caret 命令、delta 注入、polish、stop 落库）
  - `crates/desktop/src/pipeline.rs`（`StreamingPipeline` / `VadSegmentedPipeline`）
  - `crates/desktop/src/result_window.rs`（`show_result` / `update_result`）
  - `crates/desktop/frontend/src/pages/Result/index.tsx`（前端光标渲染 + 点击）
  - `crates/infra/src/db.rs`（`transcriptions` 表迁移 v13→v14、`search_transcriptions`、`insert_asr_item`）

---

## 1. 背景与问题

现状（已核实）：

- Result 语音识别窗在非编辑态是 `<div contentEditable={false}>`，**无光标**（hover 仅 `cursor-text` 视觉提示）。
- 识别文本由 `Transcript` 的单一 `full: String` 承载，**只从末尾增长**：
  - 流式引擎（local/cloud）每 tick 返回**累积全量** → `set_full` 整体替换。
  - VadSegmented 用 `append_segment(delta)` 追加 delta。
- `display_text() = committed前缀(edited ≻ polished ≻ full[..raw_len]) + increase(full[raw_len..])`。
- 后端发**全量** `display_text()` 给前端；前端用 `newText.startsWith(displayedRef.current)` 判追加、否则 300ms 后整体替换（diverted 容错）。
- 润色（mode=2）建立在 `raw_len` 线性快照边界上：停顿 → 润色 increase → `raw_len` 推进。
- 新录音开始 `show_result("正在聆听…")` 会清空上次文本。

需求：

- 非编辑态（录音中）也显示**闪烁光标**，可点击定位到任意位置。
- 点在文本中间后，**新语音实时从光标处流式插入**（而非末尾追加），原光标后文本右推。
- 用户预期这会较大改造数据结构（末尾追加 → 允许中间插入）。

---

## 2. 目标 / 非目标

**目标**

- 非编辑态自定义闪烁光标，点击定位。
- 新语音实时从光标处流式插入，光标后文本右推，光标随新词推进。
- 显示 / 落库 / 复制顺序**一致**（真中间插入，非视觉假象）。
- 默认（不点光标）行为与今天**逐字一致**（零回归）。

**非目标**

- 多光标、段拖拽重排（未来可选）。
- 实时纠正（diverted）的精确中间插入（容忍，同现状）。
- 富文本 / 格式段。
- per-segment 独立润色调用（本设计仍是全篇一次调用）。

---

## 3. 关键设计决策（已确认）

1. **路线：段（segment）模型**。废弃 `raw_text/polished_text/edited_text` 三字段 + `edited≻polished≻raw` 优先级链，改为 `Vec<Segment>`，每段自带类型。
2. **插入方式：实时流式插入光标处**（录音中点选光标，继续说话时新词从光标处冒出）。
3. **光标移动时机：立即切换**。点击即更新 `caret_gap`，下一段 delta 立即去新光标；旧活动 Raw 段自然冻结。VadSegmented 天然按段边界回填，无劈词风险；仅流式引擎存在「半词被劈」的罕见代价（可接受）。
4. **润色：一次全篇调用**。
   - `edited` = **冻结**（preserve verbatim，best-effort，唯一可信源）。
   - `raw` → `polished`。
   - `polished` → **重润**（用最新全篇上下文，文本可能被修正）。
   - **调用后无 `raw` 段**。
   - 原则：只要不是用户编辑的（raw/polished），默认都可能有错、每次都该用全篇上下文重算。
5. **自动停顿润色（mode=2）无需禁用**：段模型下润色是「全篇一次、edited 冻醒、其余重润」，与光标位置无关——中间插入态照常触发，把活动 Raw 段（含光标处那条）定型为 Polished，光标保持原 gap、后续新语音在该 gap 新建 Raw 段。手动「立即润色」同样允许。
6. **编辑态**：进编辑显示 `finish_text()`（整篇扁平）；`commit_edit(flat)` → `segments = [Edited(flat)]`、`caret_gap = 1`；**raw/polished 清零**；之后光标可把 Edited 段劈成多段。
7. **`finish_text`**：段扁平化纯文本，**派生**（不另存），供 display / 落库搜索 / clipboard。
8. **编辑后原始 raw ASR 不再保留**（行为变更：今天 `raw_text` 列编辑后仍在，新模型编辑即丢弃）。已确认接受。
9. **润色映射 best-effort**：依赖 LLM 遵 preserve 指令 + edited 段串匹配回填；LLM 偶发擅改致段界错位。同现状，已确认接受。

---

## 4. §1 数据结构

```rust
/// 段类型。后态覆盖前态：Raw → Polished → Edited。
pub enum SegmentKind { Raw, Polished, Edited }

pub struct Segment {
    pub kind: SegmentKind,
    pub text: String,
}

pub struct Transcript {
    pub id: i64,
    mode: PolishMode,
    /// 结构化真相源。空 Vec = 无文本。
    segments: Vec<Segment>,
    /// 新语音生长的「缝隙」下标，0..=segments.len()；==len 即末尾追加（默认/今天行为）。
    caret_gap: usize,
    /// 引擎累积全量，仅作 delta 提取基准。不显示、不落库。
    engine_cumulative: String,
    /// engine_cumulative 已消费到的 char 位（取 delta 用）。
    engine_consumed_chars: usize,
    last_polish_time: Instant,
    polish_pending: bool,
    db_inserted: bool,
}
```

**核心方法**

```rust
/// 段顺序拼接 → 纯文本（= display = 落库搜索文本 = clipboard）。派生。
pub fn finish_text(&self) -> String {
    self.segments.iter().map(|s| s.text.as_str()).collect()
}
/// 兼容旧名；段模型下 == finish_text。
pub fn display_text(&self) -> String { self.finish_text() }

/// 引擎累积全量 → 取尾部 delta → 在 caret_gap 处生长 Raw 段。
pub fn apply_engine_full(&mut self, full: &str) {
    let delta = if full.starts_with(&self.engine_cumulative) {
        // 正常追加：取 consumed 之后的尾部
        full.chars().skip(self.engine_consumed_chars).collect::<String>()
    } else {
        // diverted（引擎纠正早前文本）：delta 提取失准。
        // 容错：重算基准、丢弃本次 delta（不回退已展示文本），与现状 diverted 容忍一致。
        self.engine_cumulative = full.to_string();
        self.engine_consumed_chars = full.chars().count();
        return;
    };
    self.push_delta_at_caret(&delta);
    self.engine_cumulative = full.to_string();
    self.engine_consumed_chars = full.chars().count();
}

/// VadSegmented 的 append_segment 走此（delta 直接生长）。
pub fn append_segment(&mut self, delta: &str) { self.push_delta_at_caret(delta); }

/// 在 caret_gap 处确保有 Raw 段并追加 delta：
/// - 前邻段为 Raw 且光标在其尾 → 追加到该段
/// - 否则插入一条空 Raw 段到 caret_gap，再追加（caret_gap 随之后移到新 Raw 段之后）
fn push_delta_at_caret(&mut self, delta: &str) { /* 见上 */ }

/// 前端点击 → char offset → 定位光标。
/// 落在段内 → 劈段（同 kind 一分为二）；落在段界 → 直接置 caret_gap。
pub fn set_caret(&mut self, char_off: usize) { /* 遍历段累计 char，定位 gap；clamp 到 [0, len] */ }

/// 全篇润色结果回填：edited 保留、raw/polished → polished。
/// 连续非 edited 段合并为一条 polished（无 edited 边界可保，合并合理）。
pub fn polish_apply(&mut self, /* edited 串匹配回填结果 */) { /* 见 §5.C */ }

/// 编辑提交：整篇压成一条 Edited；raw/polished 清零。
pub fn commit_edit(&mut self, flat: &str) {
    if flat.is_empty() { self.segments.clear(); self.caret_gap = 0; return; }
    self.segments = vec![Segment { kind: SegmentKind::Edited, text: flat.to_string() }];
    self.caret_gap = 1; // 末尾
}
```

**默认零回归**：`segments = []`、`caret_gap = 0`、`engine_cumulative = ""` → 等价今天的空文档；`caret_gap == segments.len()` → 等价今天「末尾追加」。用户不点光标时整条路径与现状逐字一致。

**废弃**：`full` / `raw_len` / `polished` / `edited` 四字段 + `edited≻polished≻raw` 优先级链 + `increase` / `take_polish_input` / `on_polish_done` 折回逻辑（被 ~25 个单测覆盖）——全部由 `segments` + 段类型语义取代，重写 + 重测。

**两条类型不变量**：

- ① 润色后无 `Raw`（raw → polished）。
- ② 编辑后只剩 `Edited`（raw/polished 清零）。

其余时段 segments 自由组合（如录音中 `[Edited][Raw][Edited][Polished][Raw]`）。

---

## 5. §2 数据流

### A. 流式插入（核心）

- 引擎每 tick 给累积 `full` → `apply_engine_full(full)`：
  1. `delta = full[consumed..]`（若 `full` 不以 `engine_cumulative` 为前缀 = diverted → 容错重算基准、丢弃本次 delta）。
  2. `push_delta_at_caret(delta)`：在 `caret_gap` 处生长 Raw 段。
  3. 更新 `engine_cumulative` / `consumed`。
  4. coordinator emit `result_window::update_result(app, &transcript.finish_text(), insertion)` —— `insertion = caret_gap != segments.len()`（中间插入态），前端据此立即渲染（见 §3 渲染策略）。
- VadSegmented 的 `append_segment(delta)` → 同一 `push_delta_at_caret`。
- **delta 追踪与光标位置无关**：`engine_consumed_chars` 只记「引擎累积吐到哪」，跨光标移动连续递增。
- 默认 `caret_gap == len` + 末段非 Raw 时新建 Raw → 与今天「末尾追加」逐字一致。

### B. 光标定位（点击）

- 前端算点击处在 `finish_text` 的 **char offset**（code-point 计数，见 §6）→ `invoke("set_caret", { offset })`。
- `set_caret(off)`：遍历段累计 char 定位；落段内 → 劈段（同 kind 一分为二）；置 `caret_gap`。劈 Edited 段 → 产生多条 Edited 段。
- **立即切换**：流式中定位后，`caret_gap` 已更新，后续 delta 从新 gap 生长；旧活动 Raw 段在点击瞬间自然冻结（vec 里一条不再被追加的段）。视觉上新词从该处冒出、原后续文本右移。

### C. 润色（手动 / 最终 / 中间自动，均全篇一次）

- **输入**：把 segments 带类型送 LLM；edited 段标 preserve，连续非 edited 段视为一个 polish 区。
- **LLM 调用**：全篇一次，instruction = preserve edited 区 verbatim、重润其余。
- **映射回**：
  1. 在 LLM 输出里按 verbatim 定位各 edited 段（best-effort 串匹配；LLM 若擅改 → 接受其输出、kind 仍 Edited）。
  2. edited 段之间的间隙 = polished 文本，按序填回各非 edited 区。
  3. 每个非 edited 区产出一条 `Polished` 段（连续非 edited 合并为一条 polished）。
- 调用后：无 raw；edited 不变；polished 可能被修正。
- `mode=2` 自动停顿润色：段模型下与光标位置无关，**照常触发**（停顿 → 全篇润色 → 活动 Raw 段含光标处那条 → Polished；光标保持原 gap，后续新语音在该 gap 新建 Raw 段）。pending 期间段不变（无 `raw_len`/`increase` 拆分，比旧模型更不易 flicker）。手动「立即润色」走同一全篇路径。

### D. 编辑态

- 进编辑：textarea 显示 `finish_text()`；后端进入 edit mode（暂停流式 emit，等同今天）。
- `commit_edit(flat)`：`segments = [Edited(flat)]`、`caret_gap = 1`。raw/polished 清零。
- 取消：恢复原 segments 快照。
- 编辑后光标可再劈开 Edited 段 → 多条 Edited 段。

### E. 停止 / 落库 / 粘贴

- 停止：flush 尾部（finish 喂尾帧）→ 最终 segments 落库（`segments` JSON + `text` 列）；clipboard / 粘贴用 `finish_text()`。
- 下次录音：新建 Transcript（`segments = []`、`caret_gap = 0`、`engine_cumulative = ""`）。

---

## 6. §3 前端光标

- **自定义闪烁光标**（不用 `contentEditable`——那是编辑态，会引入键盘输入 / IME 问题）。光标 = 纯定位指示器。
- **位置**：`caret_gap` → `finish_text` char offset → 用 `Range` 量像素，绝对定位一条 1px 宽、`bg-foreground`、CSS `@keyframes blink` 的竖条。光标恒在「活动 Raw 段尾部」。
- **点击**：`mousedown` 在 text div 上算 char offset（标准 `document.caretRangeFromPoint` / Range 交集测距）→ `invoke("set_caret", { offset })`。现有 `cursor-text` hover 已是合适 affordance。
- **offset 计数**：前端用 code-point 计数（`Array.from(text).length` 语义），后端按 Rust `char`（Unicode scalar）对齐——二者对 BMP（含中文）一致，对 emoji/代理对需统一用 code-point，避免错位。前后端约定一致即可。
- **点击穿透兼容**：精简态下方透明区穿透不变（后端轮询）；文本区（strip 内）本就非穿透，点选光标天然吃在文本上。长篇态整窗可交互。两模式皆可用（精简态仅可见 ~2 行，目标滚出视口需先滚动）。
- **`update-result` 渲染策略（关键）**：现有前端用 `newText.startsWith(displayedRef.current)` 判追加（立即渲染）vs diverted（300ms 延迟整体替换）。中间插入时 `finish_text` 在**中间**变化、不是尾部延伸 → 会被误判 diverted → 每 tick 等待 300ms → 卡顿。修法：后端 `update-result` 附带插入态（如 `insertion: bool` 或 `caret_offset`），前端在**插入态直接立即整体渲染**（跳过 300ms 延迟）；diverted 延迟仅保留给「光标在末尾 + 引擎纠正」场景。
- ESC / 编辑快捷键 / 润色快捷键等不变。

---

## 7. §4 DB 迁移（v13 → v14）

`transcriptions` 表：

- 新增 `segments TEXT`（JSON：`[{ "kind": "raw|polished|edited", "text": "..." }]`，顺序即段序）。
- 新增 `text TEXT`（= `finish_text` 扁平，denormalize 给 search / clipboard 直接读，段变即更新）。
- 废弃 `raw_text` / `polished_text` / `edited_text` 三列（SQLite 删列用重建表方式，或先保留 nullable 一个版本再清）。

**遗留数据迁移**（旧记录 → 单段）：

- 有 `edited_text` → `[Edited(edited_text)]`
- 否则有 `polished_text` → `[Polished(polished_text)]`
- 否则 → `[Raw(raw_text)]`
- `text` = 该段文本。

**调用方改造**：

- `search_transcriptions`：`WHERE raw_text LIKE ? OR polished_text LIKE ? OR edited_text LIKE ?` → `WHERE text LIKE ?`。
- `insert_asr_item`（clipboard_history）：写 `finish_text()` 为 content（不变，本就存扁平）。
- `update_edited_text` → 改为 `update_segments(segments, text)`（commit_edit 路径）。
- 停止落库 / 中间落库：写 `segments` + `text`。

`clipboard_history` 表不变（本就存扁平 content）。

---

## 8. 边界用例

| 用例 | 处理 |
|---|---|
| **diverted（引擎纠正早前文本）** | `full` 不以 `engine_cumulative` 为前缀 → 重算基准、丢弃本次 delta、不回退已展示文本。同现状 300ms 整体替换容忍。 |
| **流式中点选光标劈半词** | 立即切换：剩余字符落到新光标，旧 Raw 冻结。Chinese 字级最多劈 1–2 字，可接受。VadSegmented 无此问题（段边界回填）。 |
| **连续非 edited 段润色** | 合并为一条 `Polished`（无 edited 边界可保）。 |
| **LLM 擅改 edited 段** | 接受其输出、kind 仍 Edited（best-effort）。 |
| **空文档点光标** | `set_caret` clamp 到 0；无文本可劈，等价末尾。 |
| **编辑后光标多次定位** | Edited 段被劈成多条 Edited 段；模型天然支持（`set_caret` 不区分 kind）。 |
| **停止时仍有 pending 流式** | finish flush 尾帧后落库；caret_gap 落库但下次录音新建 Transcript 不复用。 |
| **跨光标移动的 delta 连续性** | `engine_consumed_chars` 与位置无关，连续递增；光标移动不改 consumed。 |
| **空 Raw 段** | `push_delta_at_caret` 仅在有 delta 时插入；空 delta 不产生空段。 |

---

## 9. 测试要点

**Transcript 单测（重写，替代旧 ~25 个）**

- 默认零回归：空 segments + caret_gap=len → `finish_text`/`apply_engine_full` 行为等价旧 full 末尾追加。
- `set_caret`：段界定位、段内劈段（含劈 Edited）、clamp。
- `apply_engine_full`：正常前缀追加、diverted 容错（重算基准不崩）、跨光标移动 delta 连续。
- `push_delta_at_caret`：前邻 Raw 追加 vs 新建空 Raw；caret_gap 推进正确。
- `polish_apply`：edited 保留、raw→polished、polished 重润、连续非 edited 合并、调用后无 raw。
- `commit_edit`：压成单 Edited、raw/polished 清零、空串清空。
- 类型不变量：润色后无 Raw、编辑后只剩 Edited。

**pipeline 单测**

- StreamingPipeline：delta 经 `apply_engine_full` 落到 caret_gap（用 FakePipelineEngine）。
- VadSegmentedPipeline：`append_segment` delta 落到 caret_gap。

**DB 单测**

- v14 迁移：旧三列 → 单段映射正确（edited≻polished≻raw 优先）。
- `search_transcriptions` 查 `text` 列。
- `update_segments` 落库 + `text` 同步。

**前端（手动 / e2e）**

- 非编辑态闪烁光标渲染、点击定位、流式插入在光标处冒出、原文本右推。
- offset code-point 对齐（含 emoji）。
- 精简 / 长篇两模式点击均生效。

---

## 10. 未来 / 可选（YAGNI，本期不做）

- **`pending_caret` 延迟切换**：若立即切换的劈词体感难受，加「点击记 pending，下次静音 / 段尾再切」。纯增量，不动数据结构。
- **审计 raw 保留**：若需保留编辑前的原始 ASR，单独加 `raw_audit` 字段（不进 segments 主结构）。
- **per-segment 独立润色调用** / 段拖拽重排 / 多光标。

---

## 11. 追加特性：选中替换（Selection Replace，2026-07-04）

中插特性（§1–§10）合入并 e2e 通过后追加。需求：非编辑态**拖选**一段文字 → 在选中处说话 → **说话时（首个词到达）**删掉选中文字、识别文字从该处插入。区别于中插（保留原字、新词右推）：选中替换是「删旧换新」。用户原话「在这部分文字上面说话，**这时**就把选中那部分文字给删掉」——明确是**开口才删**，非选中即删。

### 11.1 关键决策：延迟到首词删（pending_delete）

`Transcript` 加运行时态字段 `pending_delete: Option<(usize, usize)>`（扁平 char 范围 [start, end)）。`set_selection(start,end)` 只**记录**待删范围 + 把 `caret_gap` 劈到 `start`（**不删字**，保留浏览器原生高亮反馈）。`apply_engine_full` 在**首个非空 delta** 插入前消费 `pending_delete`：`delete_range(start,end)` 真删 + 随后走已有 `push_delta_at_caret`（= 普通中插）。第二个 delta 起就是普通中插，自动衔接。

不采用「立即删」（选中即删）的理由：① 与用户「说话时才删」意图相悖；② 误操作不可逆；③ 延迟到首词保留高亮反馈、取消容易（点别处 `set_caret` 清 `pending_delete`，文字不动）。

`pending_delete` 是运行时态，**不入库**：选中后未开口就停录，`pending_delete` 残留但 segments 未变，落库的是完整文本（正确）。

### 11.2 数据结构与方法（transcript.rs）

```rust
pub struct Transcript {
    // ...既有字段...
    /// 选中替换待删范围（扁平 char [start,end)）。运行时态，不入库。
    pending_delete: Option<(usize, usize)>,
}

/// 在 char_off 处劈段，返回劈后 gap index。幂等（char_off 已在段界则不重复劈）。
/// 从 set_caret 抽出，set_caret 与 delete_range 共用（DRY）。
fn split_at(&mut self, char_off: usize) -> usize { /* 遍历段累计 char，落段内劈成两段返回 i+1；落段界返回 i；超出返回 len */ }

/// 前端点击 → 定位光标（= 取消待删选区）。
pub fn set_caret(&mut self, char_off: usize) {
    self.caret_gap = self.split_at(char_off);
    self.pending_delete = None; // 点击定位 = 取消选中
}

/// 删除扁平 char 范围 [start,end)：split_at(start) → split_at(end) → drain 中间段，caret 落 start。
fn delete_range(&mut self, start: usize, end: usize) { /* split_at 幂等 */ }

/// 选中替换：记录待删范围 + 劈 caret 到 start（不立即删字，保留高亮直到开口）。
pub fn set_selection(&mut self, start: usize, end: usize) {
    self.pending_delete = Some((start, end));
    self.caret_gap = self.split_at(start);
}
```

**消费点**：
- `apply_engine_full`：`combined_delta.is_empty()` 检查之后、`push_delta_at_caret` 之前，`if let Some((s,e)) = self.pending_delete.take() { self.delete_range(s,e); }`。
- `take_polish_input`：方法开头先删待删区，避免润色快照含选中旧字。

**清除点**：`set_caret`（取消）、`on_polish_failed`、`commit_edit`、`new`（重置）。润色成功走 `take_polish_input` 已消费。

### 11.3 命令通道（coordinator.rs，镜像 set_caret 六处）

`Command::SetSelection { start, end }` → 命令循环 match 臂（`if !editing { if let Some(t) = stage_transcript(&mut stage) { t.set_selection(start, end); } }`）← `Coordinator::set_selection` ← `#[tauri::command] set_selection(coordinator, start, end)` ← main.rs invoke_handler 注册。`stage_transcript` 复用 set_caret 同一组活跃 stage（Streaming/VadSegmented/WaitingCompletion/StoppingPolish/CloudClosing）。

### 11.4 前端拖选（Result/index.tsx）

- textRef 的 JSX：`onClick` → **`onMouseUp`**（拖选选区在 mouseup 才完整）。
- `handleTextMouseUp`（原 `handleTextClick` 改名）按 `window.getSelection().isCollapsed` 分流：
  - **折叠（纯点击）**：`caretRangeFromPoint` → `codePointOffsetBefore` → `setCaretPos(offset)` + `invoke("set_caret", { offset })`（普通中插，原逻辑）。
  - **非折叠（拖选）**：`start = codePointOffsetBefore(el, range)`、`end = codePointOffsetTo(el, range.endContainer, range.endOffset)` → `setCaretPos(null)`（隐藏闪烁光标，交浏览器原生蓝色高亮）+ `invoke("set_selection", { start, end })`。
- 选区须落在文本容器内（`el.contains(range.commonAncestorContainer)`，排除工具栏按钮）。
- **首词后衔接**：后端删待删 + 插入 → `caret_char_offset` 增长 → `Emit{caret}` → `update_result` → 前端 `setCaretPos(caret)` → CaretBlink 复现并跟随右移（复用 §3 caret 透传链，零额外改动）。React 重渲染 textRef.textContent={text} 自然清除浏览器选区（DOM 文本节点被替换）。

**offset 工具**：抽 `codePointOffsetTo(container, node, offset)` 支持任意 Range 端点（end 复用），`codePointOffsetBefore` 退化为它的 wrapper（start 端点）。

### 11.5 测试（transcript.rs，6 个单测）

- `delete_range_basic`：删单段中间范围，caret 落删除点。
- `delete_range_spans_segments`：跨段（Raw/Edited 混合）删除。
- `set_selection_then_first_delta_replaces`：拖选 → 首词删旧插新、caret 跟随增长。
- `set_caret_clears_pending_delete`：选中后点别处 → 后续 apply 不删、文字保留。
- `pending_delete_consumed_once`：首词消费后第二词普通中插（不再删）。
- `pending_delete_consumed_in_take_polish_input`：润色快照基于删后文本。
