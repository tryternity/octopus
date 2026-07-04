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

现状（改造前旧实现，已被 §3 段模型替代；保留以说明改造动机）：

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

`Transcript` 加运行时态字段 `pending_delete: Option<(usize, usize)>`（扁平 char 范围 [start, end)）。`set_selection(start,end)` 只**记录**待删范围 + 把 `caret_gap` 劈到 `start`（**不删字**，保留浏览器原生高亮反馈）。**两条 delta 入口**——`apply_engine_full`（流式 local：zipformer/paraformer streaming）与 `append_segment`（VadSegmented 离线：sensevoice/firered/qwen3/whisper + cloud partial 拼接）——都在**首个非空 delta** 插入前消费 `pending_delete`：`delete_range(start,end)` 真删 + 随后走已有 `push_delta_at_caret`（= 普通中插）。第二个 delta 起就是普通中插，自动衔接。⚠️ 两入口**必须对称消费**（见 §11.7）：漏任一即在该类引擎下选中替换失效——初版漏 `append_segment`，离线/cloud 引擎选中后首词只插不删、选中文本残留，已修（9d4a654）。

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

**消费点**（三个，缺一不可——见 §11.7 bug 教训）：
- `apply_engine_full`：`combined_delta.is_empty()` 检查之后、`push_delta_at_caret` 之前，`if let Some((s,e)) = self.pending_delete.take() { self.delete_range(s,e); }`。
- `append_segment`：`delta.is_empty()` 检查之后、`push_delta_at_caret`/`pending_delta` 之前，同样 `if let Some((s,e)) = self.pending_delete.take() { self.delete_range(s,e); }`（与 `apply_engine_full` 对称——VadSegmented 离线引擎 sensevoice/firered/qwen3/whisper + cloud partial 拼接的首词走此路径）。
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

### 11.5 测试（transcript.rs，7 个单测）

- `delete_range_basic`：删单段中间范围，caret 落删除点。
- `delete_range_spans_segments`：跨段（Raw/Edited 混合）删除。
- `set_selection_then_first_delta_replaces`：拖选 → 流式路径（`apply_engine_full`）首词删旧插新、caret 跟随增长。
- `set_selection_then_first_append_segment_replaces`：拖选 → VadSegmented/cloud 路径（`append_segment`）首词同样删旧插新（bug 回归测试，9d4a654；初版漏此路径消费 `pending_delete`，断言 `"你好新词世界"` 失败，修后得 `"你好新词"`）。
- `set_caret_clears_pending_delete`：选中后点别处 → 后续 apply 不删、文字保留。
- `pending_delete_consumed_once`：首词消费后第二词普通中插（不再删）。
- `pending_delete_consumed_in_take_polish_input`：润色快照基于删后文本。

### 11.6 同期修复：编辑保存后光标归末尾（f32f1a9）

选中替换 e2e 时发现独立 bug：编辑态保存（`commitEdit`）后闪烁光标错落**首位**（应末尾）。根因：`CaretBlink` 原把 `container={textRef.current}` 当 **prop**——render 阶段求值时 `textRef.current` 是旧值（ref 在 commit 后才更新），而保存时 `editing` true→false 致 textRef 的 `key` 从 `"edit"`→`"view"` 重挂载，此时 `textRef.current` 仍是**即将卸载的旧 contentEditable div**。effect 去量这个 detached 旧 div，`getBoundingClientRect()` 返回 `(0,0)` → 光标落首位，且后续无 state 变化不重测 → 卡死首位。

修复：`CaretBlink` 改接收 `RefObject`，在 effect（commit 后执行）内读 `.current` 拿到已挂载的新 view div，量到真实末尾。中插/点击场景 `editing` 不变、textRef 不重挂，行为不变。**通用教训**：React 中把 `ref.current` 作为子组件 prop 传递有 render-commit 滞后陷阱，重挂载场景应传 RefObject 在 effect 内读取。

### 11.7 同期修复：append_segment 漏消费 pending_delete（9d4a654）

选中替换 e2e（用户用 VadSegmented 离线引擎）发现：选中后说话，选中文本未删、识别字插在选中文本**前面**。根因：`pending_delete` 只在 `apply_engine_full`（流式路径）首词消费，`append_segment`（VadSegmented 离线 sensevoice/firered/qwen3/whisper + cloud partial 路径）漏了同样逻辑——`set_selection` 劈 `caret_gap` 到 start 但不立即删，首词经 `append_segment` 插到 start（选中文字前）却永不触发 `delete_range`，选中文字残留。流式引擎（zipformer/paraformer streaming）走 `apply_engine_full` 不受影响，故 bug 未在中插特性 e2e（流式）暴露。

修复：`append_segment` 开头（delta 非空检查后）消费 `pending_delete`，与 `apply_engine_full` 对称。新增 `set_selection_then_first_append_segment_replaces` 回归测试（§11.5）。**通用教训**：`Transcript` 有两条 delta 入口（流式 `apply_engine_full` / 分段 `append_segment`），任何「首词触发」型运行时状态（如 `pending_delete`）都必须在两入口对称消费，否则只在对应引擎下失效——两入口相关改动务必成对核对。

---

## 12. 前端渲染健壮性修复（2026-07-04，e2e 后 4 bug）

§1–§11 合入并 e2e 通过后，深度使用暴露 4 个 Result 窗前端渲染 bug，全集中在 `crates/desktop/frontend/src/pages/Result/index.tsx`。

### 12.1 文字不渲染 — contentEditable 容器的 React children 不 reconcile（最关键）

`textRef` 是 `contentEditable={editing}` 的 div（view 态 `contentEditable={false}`）。React 19（`createRoot` concurrent）对带 `contentEditable` 属性的 div **reconcile 时不写其 text children 的 DOM**——commit 阶段跳过 children 写入（保护用户编辑不被覆盖），即使 `contentEditable={false}`、即使 `flushSync` 强制 commit 也无效（commit 本身就跳过 children）。后果：流式 `setText(newText)` 改了 state，DOM textNode 始终旧 → 文字不渲染（空白），继续说话时积压文字一次性出现。查 `setTextContent` 源码不检查 contentEditable 会误判「会更新」。

修复：`renderResultNow` 里 imperative `textRef.textContent = newText`（非编辑态），绕过 React 强制 DOM = state。`measureCaretPx` 长度改读 DOM `firstText.nodeValue`（移除 `text` 参数）——否则按 state 新 text 算 `target`、DOM `firstText` 旧文本 clamp 到旧末尾 → 光标错位到旧末尾、新文字位置空白。`flushSync(setText)` 保留驱动 state 让 `CaretBlink` 的 `useEffect[text]` 触发重测。

判别：非 contentEditable 元素 React 正常 reconcile，imperative 写冗余但无害（`textContent === newText` 条件不成立、零开销）；只有 contentEditable 容器需要。触发条件：同一 div 既是 contentEditable 编辑容器又是流式展示容器（`key={editing?"edit":"view"}` 切换但 DOM 元素复用 contentEditable 属性）。

### 12.2 闪烁光标滚动错位 + 视口外隐藏

`CaretBlink` 的 px 原只在 `text/pos` 变时重算。但 `px.top = rect.top - cRect.top` 是**视口相对值，随 `scrollTop` 变**。流式 stickToBottom 时末尾在容器底；用户上滚后末尾滚到视口下方，但 `text` 未变 → px 不更新 → 光标停在容器底旧位闪烁，视觉错位（像跑到前面）。

修复：`CaretBlink` 加 scroll 监听（passive，rAF 节流）重测 px；渲染时 `px.top < -2 || px.top > clientHeight + 2` 则 `return null`（视口外隐藏）。stickToBottom 时 `px.top ≈ clientH - 行高 < clientH`（显示），上滚后 `px.top > clientH`（隐藏）。`CaretBlink` 只在 `!editing` 渲染，editing 切换即重挂载 → effect 重跑绑新 div，无监听漂移。

### 12.3 滚动跟随间隙 — onScroll 恢复 stickToBottom 立即滚底

用户上滚（`stickToBottom=false`）后滚回底部区域，原 `onScroll` 只更新 `stickToBottomRef`，实际滚底要等下个 tick（100-200ms）的 rAF → 间隙内最新文字滞留视口下方「空白」。

修复：`onScroll` 检测 stick 恢复 true 时立即 `scrollTop = scrollHeight`，不等 tick。tick 的 rAF 滚底保留用于持续跟随。

### 12.4 换行符显示 — whitespace-pre-wrap

view 态 div 默认 `white-space:normal`，把编辑态 `innerText` 提取的 `\n` 折叠成空格 → 编辑加的换行存库不丢（`commit_edit`/`finish_text`/DB `text` 列全保留 `\n`）但显示丢。纯前端 CSS 问题。

修复：textRef div 加 `whitespace-pre-wrap`。编辑态与 view 态共用同一 div（key 切换），pre-wrap 对 contentEditable 输入无副作用；流式识别文本通常无 `\n`，不受影响。

### 12.5 光标首位 — collapsed range 锚点敏感（O(1) 优化回归）

曾优化 `measureCaretPx` 末尾态为 `selectNodeContents(container)+collapse(false)`（锚容器边界，省 `Array.from(text)`），但**容器边界的 collapsed range 在 Chrome 常返回 zero rect** → 触发兜底 `{left:0,top:0}` → 光标落首位。两路径 reflow 相同（都一次 `getBoundingClientRect`），省的纯 CPU 对流式短文本微不足道。回退为文本节点内 `setStart(firstText, offset)+collapse(true)`（锚文本节点内 offset，rect 可靠）。

### 12.6 通用教训汇总

- **contentEditable 容器的 React children reconcile 不可靠**（12.1）——流式/高频更新 contentEditable div 的文本须 imperative 同步。
- **视口相对像素须随 scroll 重测**（12.2）——`getBoundingClientRect()` 差值随滚动变，光标定位组件要监听 scroll。
- **CSS `white-space` 决定 `\n` 可见性**（12.4）——contentEditable innerText 的 `\n` 在 normal 下折叠，需 pre-wrap。
- **collapsed range 的 `getBoundingClientRect` 锚点敏感**（12.5）——锚容器边界常返 zero rect，应锚文本节点内 offset。

## 13. 前端渲染健壮性追加修复（代码审查，2026-07-04）

§12 的 e2e 4 bug 之后，第三轮代码审查又发现 2 个 Result 窗前端渲染 bug，同在 `Result/index.tsx`。

### 13.1 Bug 1.1：最终文本被 pending diverted 延迟覆盖

show-result handler 的 diverted 容错（§5.A / §12 关联）：光标在末尾且引擎纠正早前文本时，前端走 300ms `divertedTimer` 延迟整体替换（防抖，等下一帧 delta）。但 **else 分支（最终文本到达，`insertion` 态或纯追加）漏清 pending 的 diverted 计时器**——若中途某帧误判 diverted 启动了计时器，最终文本立即渲染后，300ms 后旧的 diverted 回调仍触发，用**旧基准的整体替换**覆盖掉刚渲染的最终文本（视觉：文字闪回旧值）。

修复：show-result 的 else（最终/插入态立即渲染）分支显式 `clearTimeout(divertedTimer)` + 清 `pendingDiverted`，确保最终文本落地后 diverted 路径不再触发。

### 13.2 Bug 2.1：CaretBlink 初始 measure 同步触发 layout thrashing

`renderResultNow` 同帧 `flushSync(setText)` + imperative `textRef.textContent = newText`（DOM 写，§12.1），紧接 `CaretBlink` 的 `useEffect[text,pos]` **同步**调 `measure()` → `measureCaretPx` 同步 `getBoundingClientRect`。同帧 write→read = 强制回流（layout thrashing）；高频 ASR（10-20Hz）下每帧叠加。§12.2 的 scroll 重测已用 rAF 节流，但**初始 measure 漏改同步**。

修复：初始 `measure()` 推到 `requestAnimationFrame`——DOM 写先落地、布局稳定后再读。代价 1 帧（~16ms）光标滞后，肉眼无感，且天然合并到帧边界。初始 raf 与 scroll raf 分变量（`raf`/`scrollRaf`）独立 cancel；`!el` 提前返回也 cancel 初始 raf 防泄漏。`flushSync` 保留（驱动 state 让 effect 同步 schedule rAF）。

## 14. 前端渲染健壮性第三轮修复（代码审查 4/5，2026-07-04）

§12（e2e 4 bug）、§13（审查 2 bug）之后，第四/五轮代码审查又发现 2 个 Result 窗前端 bug，同在 `Result/`（`caret.ts` + `index.tsx`）。前端无组件级单测（`renderResultNow` 耦合 Tauri），靠 `npm run build` + `npm run test`（caret 纯函数）+ 用户 e2e。

### 14.1 Bug 3.1：measureCaretPx 仅定位首文本节点（多节点错位）

`measureCaretPx` 原只取 `TreeWalker` 的**第一个** text node，用首节点长度 clamp `pos`。`whitespace-pre-wrap`（§12.4）下多行文本 / 编辑残留 `<br>` 可能使容器含**多个** text node；当 `pos` 超出首节点长度时本应跳到后续节点，旧实现却 clamp 在首节点末尾 → 光标测量错位。

当前结果窗 `textContent` 单行写入（§12.1 imperative）通常单节点，此 bug 未被 e2e 触发，修复属**防御性**正确化。

修复：抽共享 helper `locateCpOffset(container, pos)`——`TreeWalker` 收集所有 text node，按各节点 code-point 长度累加，定位 `pos` 落在哪个节点的哪个 UTF-16 offset（`pos=null`→末节点末尾，越界→末节点末尾）；code-point → UTF-16 offset 转换沿用 §6 对齐（与 Rust `char` 一致）。`measureCaretPx`（量像素）与新增的 `placeCaretAtCodePoint`（设选区，§14.2）共用此遍历（DRY）。单节点主路径行为与旧实现一致。长度仍读 DOM `nodeValue`（§12.1），非 state text。

### 14.2 Bug 3.4：进编辑态光标无条件落末尾

`enterEdit` 进入编辑态时 `setCaretPos(null)`（闪烁光标交还 DOM 选区），随后 `setTimeout` 内 `range.selectNodeContents(el)+collapse(false)` **无条件**把光标置到末尾。长文本下用户在非编辑态点过中间某处再进编辑，光标跑到末尾、得重新找位置。

修复：`enterEdit` 在 `setCaretPos(null)` **之前**用 `caretPosRef`（caretPos 的 ref 镜像，`useRef` + 同步赋值 effect，避免闭包 stale）捕获点击位 `restorePos`；`setTimeout` 内若 `restorePos != null` → `placeCaretAtCodePoint(el, restorePos)` 精准恢复，失败 / `null` 再走末尾兜底。新增 `placeCaretAtCodePoint`（复用 §14.1 `locateCpOffset` 设 collapsed Selection）。

**边界**：`caretPos` 仅**纯点击**设值（`handleTextMouseUp` 折叠分支）；**拖选**置 `null`（交 `set_selection`）。故拖选后进编辑仍落末尾（设计如此——拖选态本就无单点光标位）。
