# 设计文档：Transcript 模型 — raw/polished/increase 三文本统一与停顿驱动润色

> 重构识别过程中的文本状态模型，引入 `Transcript` 结构统一管理原生文本、润色文本、增量文本三者关系；润色改为停顿驱动的全量润色（流式 / 伪流式统一）；DB 改为过程增量入库（id = 毫秒时间戳）；剪贴板默认保留识别结果。

> **实现状态（2026-06-14）**：已实现，commits `9bb3b34`..`33b17b8`（`feat/transcript-model` 分支）。`cargo check --workspace --all-targets` 0 error，`cargo test --workspace` 全 PASS（asr 16 + desktop transcript 8 + infra 4）。手动 e2e（§7.3）待用户验证。

## 0. 背景

当前 `crates/desktop/src/coordinator.rs` 的文本状态散落在 `Stage::Streaming` / `VadSegmented` / `WaitingCompletion` / `Pasting` 各变体的 `accumulated_text` / `raw_text` / `polished_text` 字段里，存在三类问题：

### 0.1 三文本关系混乱

`accumulated_text`（展示用）、`raw_text`（原生）、`polished_text`（润色）三者的维护与合并散落各处，语义不清：
- `handle_streaming_tick`（:962-963）把流式返回的**全量** partial 直接覆盖 `accumulated_text` 与 `raw_text`：
  ```rust
  *accumulated_text = new_text.clone();  // 全量覆盖
  *raw_text = new_text;
  ```
- `handle_polish_done`（:1271-1287）用 `skip(polish_base_len)` 取增量再 `format!("{}{}", polished, increment)` 合并 —— 这套「增量合并」在「流式全量覆盖」面前失效。

### 0.2 流式中间润色 P0（polish_mode=2）

`streaming_engine.rs::accept_samples`（:67）对 Paraformer 返回 `Ok(Some(acc.clone()))` —— **全量累积文本，非增量**。coordinator 拿到后整体覆盖 `accumulated_text`，导致：
- 中间润色（mode=2）刚合并进 `accumulated_text` 的 `polished` 结果，在**下一个 tick 被 partial 全量覆盖丢失**。
- 即：流式 + mode=2 时，中间润色结果无法稳定展示。

伪流式（VadSegmented）不受影响 —— 它用 `push_str` 追加段文本（:625-644），是天然增量。

### 0.3 剪贴板不保留识别结果

`paste.rs`：
- `paste_via_clipboard`（:64）：粘贴后**恢复原剪贴板内容**（:101）→ 剪贴板里是用户原来的内容，不是识别结果。
- `paste_direct`（:106）：用 enigo 模拟键盘输入，**完全不碰剪贴板** → 剪贴板里也不是识别结果。
- `write_to_clipboard`（:56，None 模式）：只写剪贴板 → 唯一保留识别结果的模式。

用户需求：粘贴完成后，剪贴板应持有识别结果（方便在他处再粘贴）；展示区清空是正常 UI 契约，不矛盾。

### 0.4 入库只有一次性 INSERT

`crates/asr/src/db.rs`：`transcriptions` 表主键 `id INTEGER PRIMARY KEY AUTOINCREMENT`（:85），**无 UPDATE 接口**，仅 `insert_transcription` 在 `PasteDone` 时一次性 INSERT。识别过程中的中间状态不入库，异常退出则全丢。

## 1. 目标与范围

### 1.1 本次做

| 功能 | 说明 |
|------|------|
| `Transcript` 结构 | 抽出独立 struct，统一管理 `raw` / `polished` / `increase` 三文本 + 润色状态，纯逻辑可单测 |
| 停顿驱动全量润色 | 流式 / 伪流式统一：静音 ≥ 600ms 时把当前完整 ASR 快照送去 LLM 全量润色，不重置流式引擎 |
| 修复流式中间润色 P0 | 停顿 = partial 稳定点（无回改），此时切片安全；raw 作快照基准，increase 为停顿后增量 |
| DB id = 毫秒时间戳 | `id INTEGER PRIMARY KEY`（应用写入，去 AUTOINCREMENT），兼任主键 / 业务 key / 开始时间戳 |
| 过程增量入库 | 首次有 ASR → INSERT；分段 → UPDATE raw；停顿润色 → UPDATE polished；停止 → finalize UPDATE |
| `write_to_clipboard` 配置 | 全局配置（默认 true）：粘贴后是否把识别结果写入剪贴板 |
| 错误降级 | DB / 润色失败不阻塞识别流程（best-effort） |

> 以上全部已实现（见顶部 commits，2026-06-14）。仅 §7.3 手动 e2e 待用户验证。

### 1.2 不做（本次）

| 不做 | 原因 |
|------|------|
| 连续润色失败的降级计数 | YAGNI，失败即保持上次 polished |
| 「识别中」状态字段 | 崩溃残留即最后 UPDATE 状态，YAGNI |
| 同毫秒 id 冲突处理 | 桌面单用户单快捷键，概率近乎 0 |
| 流式 partial 前缀回改的防御性检测 | 依赖「停顿后前缀稳定」假设，实践中成立 |
| 剪贴板历史 / 保留原内容选项 | `write_to_clipboard=false` 已覆盖高级用户需求 |

## 2. Transcript 模型

### 2.1 结构定义

```rust
struct Transcript {
    id: i64,                // 识别开始时刻的毫秒时间戳（Unix epoch ms）
    raw: String,            // 上次停顿时的完整 ASR 快照（稳定，润色基准）
    polished: String,       // 对 raw 的润色结果（mode=0/1 恒空）
    increase: String,       // last_polish_time 之后新识别的增量（mode=0/1 恒空）
    last_polish_time: Instant,
    polish_pending: bool,   // 是否有润色线程在途
    mode: PolishMode,        // 0=禁用, 1=仅最终, 2=中间+最终
}
```

`Transcript` 抽成**独立 struct，纯逻辑方法，不依赖 tauri `AppHandle`**。`Coordinator` 的 `Stage::Streaming` / `VadSegmented` / `WaitingCompletion` / `Pasting` 各持有一个 `Transcript`（或引用），调用其方法。这是可测性与架构清晰的关键（见 §7.1）。

### 2.2 字段语义与不变量

| 字段 | 语义 | 不变量 |
|------|------|--------|
| `id` | 开始识别时刻毫秒戳，DB 主键 | 一次识别内不变；生成于识别开始 |
| `raw` | 上次停顿（或首段）时的完整 ASR 快照 | 停顿触发时更新为当前完整 ASR；是 `polished` 的润色基准 |
| `polished` | 对 `raw` 的润色结果 | 仅 mode=2 中间润色 / 各 mode 最终润色时填值；润色失败保持上次值 |
| `increase` | `last_polish_time` 后新识别的文本 | mode=0/1 恒空；mode=2 实时累积；停顿快照后清空（并入 raw） |
| `last_polish_time` | 上次触发润色的时刻 | 节流判断用（`polish_interval`） |

**核心不变量**：
- 完整 ASR ≡ `raw + increase`（任意时刻）
- mode=0：`increase == ""` 且 `polished == ""` 全程恒成立（不润色）
- mode=1：`increase == ""` 全程恒成立；`polished` 过程中为空，仅停止时最终润色填值
- mode=2：过程 `display_text() == polished + increase`；停止时 increase 并入 raw
- DB 的 `raw_text` 列 ≡ `raw + increase`（落库时拼上 increase，保证完整）

### 2.3 关键方法

```rust
impl Transcript {
    /// 新增识别增量（流式 partial 增量 / 伪流式段文本）
    fn on_segment(&mut self, delta: &str);

    /// 停顿触发：把当前完整 ASR（raw+increase）送润色前的快照输入
    fn snapshot_for_polish(&self) -> String;   // = raw + increase

    /// 润色完成后：更新 polished，raw 快照推进，increase 清空
    fn on_polish_done(&mut self, polished: String);

    /// 展示文本：mode=2 → polished + increase；其他 → raw
    fn display_text(&self) -> String;

    /// 落库文本：raw + increase（完整 ASR）
    fn db_text(&self) -> String;
}
```

### 2.4 各 polish_mode 行为

| 场景 | mode=0（禁用） | mode=1（仅最终） | mode=2（中间+最终） |
|------|----------------|------------------|---------------------|
| `increase` | 恒空 | 恒空 | 实时累积（停顿后清空并入 raw） |
| `polished` | 恒空 | 恒空（过程） | 每停顿全量重润色 |
| 中间展示 | `raw` | `raw` | `polished + increase` |
| 中间润色触发 | 不触发 | 不触发 | 停顿 600ms 触发 |
| 最终润色 | 不润色 | 停止时润色 | 停止时润色 |
| 入库 `raw_text` | `raw` | `raw` | `raw + increase` |
| 入库 `polished_text` | NULL | 最终润色结果 | 最终润色结果 |

### 2.5 流式 vs 伪流式的 increase 来源

`on_segment(delta)` 统一接收增量，但两种模式的 `delta` 来源不同：

**流式**（`StreamingSession::accept_samples` 返回全量 partial）：
- coordinator tick 内计算 `delta = accumulated.chars().skip(raw.chars().count()).collect()`（当前 partial 去掉 raw 前缀）
- 依赖假设：**停顿后 partial 前缀稳定**（无回改），故 `raw` 是当前 `accumulated` 的稳定前缀，`delta` = 后缀增量
- 停顿触发时：`raw = accumulated.clone()`（整体快照），`increase` 清空 → 下次 partial 的 `delta` 基于新 raw

**伪流式**（VadSegmented，段独立识别）：
- `delta` = 本段 `consume_completed_results` 返回的文本（天然增量，`push_str` 追加）
- 不依赖前缀稳定性（段间本就独立）

> 两种来源对 `Transcript` 透明 —— `on_segment` 只累加 `increase`，不关心 delta 怎么算。

## 3. 停顿驱动润色

### 3.1 统一机制

**流式与伪流式统一为：静音 ≥ 600ms 时，把当前完整 ASR 快照（`raw + increase`）送去 LLM 全量润色。**

- 润色输入 = `snapshot_for_polish()` = `raw + increase`（完整 ASR）
- 润色返回 → `on_polish_done(polished)`：`polished` 更新，`raw` 推进为快照，`increase` 清空
- **不重置流式引擎**（只读快照送 LLM，引擎状态原样保留）—— 这是修复 P0 的关键：partial 继续流式累积，不再覆盖 polished

### 3.2 与现有静音机制的协调

流式 tick 内，VAD 的 `silence_duration` 是共享信号，按阈值升序被三个消费者各取所需，互不干扰：

| 阈值 | 消费者 | 作用层 | 现有/新增 |
|------|--------|--------|-----------|
| `PUNCTUATION_SILENCE_THRESHOLD` | 标点插入 | 文本层（加逗号句号） | 现有 |
| `0.5s`（Active Flush） | 引擎补零冲刷 | 引擎层（吐出 buffered partial） | 现有 |
| **`600ms`（停顿润色）** | **全量润色触发** | **润色层** | **新增** |

**顺序保证**：600ms > 500ms，润色触发时 Active Flush 已先冲刷 → `accumulated_text` 是最新完整文本 → 快照可靠。润色在 tick 流程最末执行：
```
drain samples → VAD 更新 silence → Active Flush（500ms）→ 标点 → 润色快照（600ms, mode=2）
```

### 3.3 伪流式的停顿

伪流式无流式引擎 buffer，停顿点 = 分段点（`segment_silence` / `segment_duration` 触发 `consume`）。每段 `consume` 完成后，若 mode=2 → `raw = 截至本段的完整 ASR` → 触发全量润色。不涉及 Active Flush。

## 4. DB 入库

### 4.1 schema 改动（id = 毫秒时间戳）

```sql
CREATE TABLE transcriptions (
    id            INTEGER PRIMARY KEY,   -- 应用写入的毫秒时间戳（去 AUTOINCREMENT）
    created_at    TEXT    NOT NULL,
    engine        TEXT    NOT NULL,
    engine_mode   TEXT,
    raw_text      TEXT    NOT NULL,      -- 完整 ASR（= Transcript.raw + increase）
    polished_text TEXT,                  -- 润色结果；NULL = 未润色/失败
    polish_status TEXT    NOT NULL DEFAULT 'off',
    polish_model  TEXT,
    duration_ms   INTEGER,               -- = finalize_now_ms - id
    char_count    INTEGER
);
```

- `id` 去掉 `AUTOINCREMENT`，由应用写入毫秒时间戳 —— 兼任主键 / 业务定位 key / 开始时间戳
- `duration_ms = finalize_now_ms - id`（id 即开始时间戳，无需额外字段）
- 旧记录 id（迁移自 history.txt 的小整数）与新记录毫秒戳值域不冲突，但本次 migration 直接 DROP 重建（见 4.2）

### 4.2 migration（v2 → v3，DROP 重建）

旧数据无所谓，直接 DROP + 重建（SQLite 不支持 ALTER 列约束，重建最干净）：

```sql
DROP TABLE transcriptions;
CREATE TABLE transcriptions ( /* 上节 schema */ );
CREATE INDEX idx_trans_created ON transcriptions(created_at DESC);
CREATE INDEX idx_trans_engine  ON transcriptions(engine);
PRAGMA user_version = 3;
```

`init_schema` 的 `user_version` 分发：
- `0` → 全新建表（新 schema）+ seed models → `PRAGMA user_version = 3`
- `1` / `2` → DROP 重建 transcriptions（models 表不动）→ `PRAGMA user_version = 3`
- `3` → no-op

### 4.3 入库时机

| 事件 | 触发点 | DB 操作 | 写入内容 |
|------|--------|---------|----------|
| **首次有 ASR** | 首 partial 非空(流式) / 首段 `consume`(伪流式) | `INSERT` | `id`, raw=首段, polished=NULL, status='off', char_count |
| **分段** | 伪流式 `consume` / 流式停顿分段 | `UPDATE raw` | raw_text=`raw+increase`, char_count（WHERE id=?） |
| **中间润色** | 停顿 600ms + `on_polish_done`（mode=2） | `UPDATE polished` | polished_text, status='done', polish_model |
| **结束 finalize** | Toggle 停止 | `UPDATE finalize` | raw_text=`raw`(完整), polished, status, char_count, duration_ms=`now_ms-id` |

> `id` 在识别开始时生成（`SystemTime::now().duration_since(UNIX_EPOCH).as_millis() as i64`），存入 `Transcript.id`。INSERT 延迟到**首次有 ASR 文本**（按快捷键但未说话时不落库）。

### 4.4 接口（crates/asr/src/db.rs）

```rust
pub fn insert_transcription_at_id(id: i64, raw_text: &str, engine: &str, engine_mode: &str) -> Result<()>
pub fn update_raw_text(id: i64, raw_text: &str, char_count: i64) -> Result<()>
pub fn update_polished(id: i64, polished_text: &str, polish_status: &str, polish_model: Option<&str>) -> Result<()>
pub fn finalize_transcription(id: i64, raw_text: &str, polished_text: Option<&str>, polish_status: &str, polish_model: Option<&str>, char_count: i64, duration_ms: Option<i64>) -> Result<()>
```

旧的 `insert_transcription` / `insert_transcription_at`（自增 id 版）删除或改为内部调用 `insert_transcription_at_id`。

### 4.5 崩溃恢复

过程中每次分段 / 润色都 UPDATE，异常退出时 DB 留下最近一次 UPDATE 的完整 `raw_text` 快照（过程值 = `raw+increase`，完整 ASR）。残留记录照常进历史列表，无需「识别中」状态字段。

## 5. 错误处理

### 5.1 原则

**识别核心流程（展示 / 粘贴）永不被 DB 或润色失败阻塞。** DB 是 best-effort 持久化，润色失败降级到 raw。失败一律 warn/error log，绝不 panic、绝不中断识别。

### 5.2 错误矩阵

| 失败点 | 对内存状态影响 | 对 DB 影响 | 对展示/粘贴 |
|--------|----------------|------------|-------------|
| **中间润色 Err**（mode=2，停顿触发） | `polished` 保持上次值，`increase` 不变，`polish_pending=false` | UPDATE polished 跳过（status 不改） | 展示 = `polished_last + increase`，不受影响 |
| **最终润色 Err**（停止时） | `polished=""` | 入库 `polished=NULL, status='failed'`，raw 完整落库 | 粘贴/展示 fallback 到 `raw` |
| **DB INSERT 失败**（首次有文本） | `Transcript.id` 仍在内存，识别继续 | 本条无库记录；后续 UPDATE 因 id 不存在静默失败 | 不受影响 |
| **DB UPDATE 失败** | 内存状态正确 | DB 滞后（下次 UPDATE 若瞬时错误可能自愈） | 不受影响 |
| **流式 accept_samples Err** | 本 tick 不覆盖 `accumulated_text` | 无 | error log，跳过本 tick，下 tick 继续 |

## 6. 剪贴板（write_to_clipboard 配置）

### 6.1 配置定义

新增全局配置（`infra::AppConfig`）：
```yaml
write_to_clipboard: true   # 默认 true
```

**语义**：粘贴流程结束后，是否把识别结果写入剪贴板。
- `true`（默认）：写入识别结果（方便他处再粘贴）
- `false`：不写入，保留用户原剪贴板内容（高级用户，等同现状行为）

写入的文本 = `final_text`（= 展示文本 = 粘贴文本 = `polished`（mode=2 done）/ `raw`（其他））。三者一致。

### 6.2 三模式矩阵（crates/desktop/src/paste.rs）

| 模式 | `write_to_clipboard=true`（默认） | `write_to_clipboard=false`（=现状） |
|------|----------------------------------|-----------------------------------|
| **Clipboard** (`paste_via_clipboard`) | 写结果 → Cmd+V（**不恢复**） | 保存原 → 写结果 → Cmd+V → **恢复原** |
| **Direct** (`paste_direct`) | enigo 输入 → **末尾写剪贴板**（识别结果） | enigo 输入（**不碰剪贴板**） |
| **None** (`write_to_clipboard`) | 写剪贴板（识别结果） | 写剪贴板（识别结果）— *配置对其无意义* |

> None 模式例外：其唯一目的是把识别结果放进剪贴板（不粘贴），`write_to_clipboard` 对它无意义，忽略。
>
> **关键性质**：`write_to_clipboard=false` 时三种粘贴模式的行为 = 当前代码现状（不破坏现有用户习惯）；`true` 是新默认。

### 6.3 展示区清空契约

粘贴完成后 `clear_result`（UI 清空）不变 —— 展示区是临时浮窗，剪贴板是供他处使用的副本，两者不矛盾。

## 7. 测试策略

### 7.1 Transcript 抽成独立 struct（可测性关键）

当前 `raw_text` / `accumulated_text` 散落在 `Stage` 各变体，与 tauri `AppHandle` 耦合，无法单测。**抽出 `Transcript` 为独立 struct，纯逻辑方法**，coordinator 持有并调用。既可单测，也符合隔离单元原则。

### 7.2 单元测试（`cargo test`，纯逻辑）

**Transcript 状态机**（不依赖 tauri/DB）：
- mode=0 / mode=1：`increase` 恒空、`polished` 恒空、`display==raw`、`db==raw`
- mode=2：`on_segment` 累积 `increase`；停顿快照后 `raw` 更新、`increase` 清空；`on_polish_done` 更新 `polished`；`display == polished + increase`
- 边界：空 increase、连续停顿、润色失败（`polished` 保持上次值）

**DB 层**（`crates/asr/src/db.rs`，内存 SQLite `:memory:`）：
- v0→v3 全新建表；v1/v2→v3 DROP 重建（mock 旧 schema 后跑 migration，验证 `PRAGMA user_version=3`、`id` 列无 AUTOINCREMENT）
- `insert_transcription_at_id` / `update_raw_text` / `update_polished` / `finalize_transcription` 往返一致
- id 为毫秒戳、应用写入

### 7.3 手动 e2e（无法自动化，文档化步骤）

**coordinator 流程**（备份 `~/.octopus/` 后）：
- 流式 + mode=2：说话 → 停顿 600ms → 展示跳变为 `polished+increase` → 停止 → 粘贴 `polished`
- 伪流式 + mode=2：分段 → 每段后展示更新 → 停止 → 粘贴
- **错误降级**：断网（LLM 失败）→ 展示降级 raw、入库 `status='failed'`、不崩溃

**剪贴板**：
- `write_to_clipboard=true`：Clipboard / Direct / None 三模式完成后，在他处 Cmd+V 得到识别结果
- `write_to_clipboard=false`：三模式完成后，剪贴板保留用户原内容（等同现状）
- 展示区已清空（与剪贴板保留不矛盾）

### 7.4 不测（YAGNI）

- coordinator tick 的 VAD 集成（依赖音频 / tauri，难自动化）
- 同毫秒 id 冲突（概率近乎 0）
- 连续润色失败的降级计数（§1.2 已决定不做）

## 8. 配置项汇总

| 配置 | 类型 | 默认 | 说明 |
|------|------|------|------|
| `write_to_clipboard` | bool | `true` | 粘贴后是否把识别结果写入剪贴板（§6） |

> 润色相关配置（`polish_mode` / `polish_interval`）由并行进行的 PolishMode 重设计（见 `docs/superpowers/specs/2026-06-14-polish-mode-redesign-design.md`）收敛为 `PolishMode` 枚举，本 spec 直接引用 `PolishMode`（0/1/2）。

## 9. 验证步骤

1. `cargo test -p octopus-desktop`（Transcript 状态机单元测试通过）
2. `cargo test -p octopus-asr`（DB migration + UPDATE 接口测试通过）
3. `cargo check --workspace --all-targets`（编译通过）
4. 备份 `~/.octopus/`，删除 `octopus.db`，启动 → 确认 `PRAGMA user_version=3`、`transcriptions.id` 列无 AUTOINCREMENT
5. 流式 + mode=2 录音：说话 → 停顿 600ms → 结果窗口展示跳变为 polished+increase → 停止 → 粘贴得到 polished；DB 该条 `raw_text` 完整、`polished_text` 有值、`polish_status='done'`
6. 断网模拟 LLM 失败：展示降级 raw、不崩溃、DB `polish_status='failed'`
7. `write_to_clipboard=true`：粘贴后他处 Cmd+V 得识别结果；`write_to_clipboard=false`：剪贴板保留原内容
