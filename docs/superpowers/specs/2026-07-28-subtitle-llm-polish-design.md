# 字幕 LLM 润色（Subtitle LLM Polish）— 设计规格（spec）

> **Status: 📝 设计阶段**（2026-07-28，brainstorming 完成，待写实施 plan）
>
> **本 spec 范围**：在录屏自动字幕生成流程中加入可选的 LLM 润色步骤——用户点「转字幕」时弹对话框选择是否润色 + 用哪个 LLM；整段润色（保留上下文）+ `[[N]]` 标记边界拆回 cue；粗略拆分降级。
>
> **前置依赖**：录屏自动字幕 v2（`docs/superpowers/specs/2026-07-28-record-auto-subtitle-design.md`，已实现）。
>
> **不在本 spec 范围**：流式润色（SSE）、字幕内联编辑器、多轮润色迭代、润色结果与原文本的 diff 可视化。

## 0. 决策回顾（brainstorming 确认）

| 决策项 | 结论 | 理由 |
|---|---|---|
| **实现路径** | 写完整 spec（独立功能） | 新 prompt 路径 + 解析逻辑 + 前端交互，超出现有 generate_subtitle 的扩展范围 |
| **触发时机** | 生成时弹对话框选选项 | 透明可控：用户每次显式选是否润色 + 用哪个 LLM |
| **标记格式** | `[[N]]` 符号标记 | 符号稳定、token 少、解析简单、LLM 遵从率高 |
| **降级策略** | 粗略拆分润色文本 | LLM 不按格式输出时，按原 cue 字符比例切润色后文本，尽力保留润色效果 |
| **分层位置** | desktop 编排层（generate_subtitle_inner step8→step10 之间） | record/asr-local 都不依赖 octopus-llm；desktop 已依赖 octopus-llm + 已是编排层 |

## 1. 范围与边界

### 1.1 MVP 范围（IN）

- 点「转字幕」弹对话框：「LLM 润色」checkbox（默认跟随 Settings）+ 「润色用 LLM」下拉（可选已配置的 LLM）+ 确认/取消
- 用户确认后：ASR → 整段 LLM 润色（`[[N]]` 标记）→ 解析拆回 cue（时间戳保持原样）→ 写 `.N.srt`
- 粗略拆分降级：LLM 输出无 `[[N]]` 标记时，按原 cue 字符比例切润色后文本
- 进度反馈：润色阶段（Recognizing 之后加 Polishing 阶段，percent 40→70）
- 失败降级：LLM 调用失败/超时/panic → 回退用原 ASR 文本（不润色）+ 前端提示「LLM 润色失败，使用原始识别」
- Settings 加全局默认：「字幕默认 LLM 润色」开关 + 「字幕默认润色 LLM」选择（弹框 checkbox 的初始值来源）

### 1.2 MVP 范围外（OUT）

- 流式润色（SSE）—— 当前 octopus-llm 是同步阻塞，无流式
- 字幕内联编辑器（cue 文本增删改）
- 多轮润色迭代（润色后再润色）
- 润色结果与原文本的 diff 可视化
- 专门为字幕场景定制 system prompt（复用现有 default-polish / advanced-polish，仅 user prompt 不同的标记格式指令）

### 1.3 成功标准

1. 5 分钟带麦录屏 → 点「转字幕」勾选「LLM 润色」→ ≤ 90 秒完成（ASR ~1s + LLM ~30-60s）→ SRT 内容比纯 ASR 更通顺（标点/同音字/填充词修正）
2. `[[N]]` 标记解析成功率 ≥ 80%（主流中文 LLM 遵从率应该更高）
3. LLM 不遵从格式 → 粗略拆分降级生效，cue 数量与原 ASR 一致（时间戳对齐）
4. LLM 调用失败 → 回退原 ASR 文本，不阻塞字幕生成，前端有提示
5. 重复点「转字幕」→ 正常递增 `.N.srt`（与 v2 一致）

## 2. 架构总览

### 2.1 数据流（在 generate_subtitle_inner step8 与 step10 之间插入润色步骤）

```
[已有] step8: transcribe_segments_with_timestamps → Vec<TimestampedSegment>
      ↓
[新增] step8.5: LLM 润色（可选，用户勾选时）
      ├─ 构造润色输入：每条 cue 文本用 [[N]] 包裹拼接
      │    "请润色以下语音识别文本，保留 [[1]][[2]]... 标记边界：
      │     [[1]]第一句原文[[2]]第二句原文[[3]]..."
      ├─ spawn_blocking + catch_unwind 调 octopus_llm::chat_text_with_prompt
      │    （system prompt 复用 system_prompt()；user prompt 含标记格式指令）
      ├─ 解析 LLM 输出：按 [[N]] split 回 N 段
      ├─ 失败降级 A（标记解析失败）：按原 cue 字符比例切润色文本（split_polished_by_ratio）
      └─ 失败降级 B（LLM 调用失败/超时/panic）：用原 ASR 文本 + emit 警告
      ↓
[已有] step10: TimestampedSegment → SubtitleCue（text 用润色后的，时间戳不变）
```

### 2.2 分层职责

| 层 | crate | 职责 | 新增/修改 |
|---|---|---|---|
| LLM 核心 | `octopus-llm` | `chat_text_with_prompt`（已有，复用） | 无改动（复用现有 API） |
| 润色编排 | `desktop` | 构造标记输入、调 LLM、解析标记、降级 | **新增**：`subtitle_polish.rs` 模块 |
| 字幕命令 | `desktop` | generate_subtitle 加 `polish: Option<PolishOption>` 参数 | **修改**：generate_subtitle_inner 加 step8.5 |
| 配置 | `infra` | Settings 加字幕润色默认开关 + 默认 LLM | **新增**：app_config 两个字段 |
| 展示 | `frontend` | 弹对话框 + 进度 + 失败提示 | **修改**：转字幕按钮交互 + SubtitleProgress 加 Polishing 阶段 |

### 2.3 依赖方向（无变化）

```
infra ← record ← desktop → asr-local
                    ↓
               desktop → octopus-llm（已有依赖）
```

record/asr-local 不依赖 octopus-llm（分层约束不变）。润色逻辑在 desktop 新模块 `subtitle_polish.rs`。

## 3. 数据模型

### 3.1 新增 Rust 结构（crates/desktop/src/subtitle_polish.rs）

```rust
/// 字幕润色选项（generate_subtitle 命令参数）。
/// None = 不润色；Some = 用指定 LLM 润色。
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PolishOption {
    /// LLM 配置标识（provider:model 字符串，如 "openai:gpt-4o"）。
    /// None = 用 resolve_active_engine("llm") 默认。
    pub llm_key: Option<String>,
}

/// 标记格式：LLM 输出须用 [[N]] 包裹每条 cue（N 从 1 递增）。
/// 解析时按字面量 "[[" split（不依赖 regex）。
const CUE_MARKER_OPEN: &str = "[[";
const CUE_MARKER_CLOSE: &str = "]]";
```

### 3.2 Settings 新增字段（crates/infra/src/config.rs AppConfig）

```rust
pub struct AppConfig {
    // ... 现有字段
    /// 字幕生成时默认是否启用 LLM 润色（弹框 checkbox 初始值）。
    pub subtitle_llm_polish_default: bool,      // 默认 false
    /// 字幕润色默认用的 LLM key（provider:model）。空 = resolve_active_engine("llm")。
    pub subtitle_polish_llm_key: String,         // 默认 ""
}
```

通过 `set_config` 命令持久化（与现有 `record_*` 配置同模式）。

### 3.3 SubtitleProgress 加 Polishing 阶段（crates/record/src/subtitle.rs）

```rust
pub enum SubtitleProgress {
    ExtractingAudio { percent: u32 },    // 0~30%
    Recognizing { percent: u32 },        // 30~40%
    Polishing { percent: u32 },          // 40~90%（新增）
    Finalizing { percent: u32 },         // 90~100%
    Done { cue_count: usize },
    Error { message: String },
}
```

注意：`SubtitleProgress` 在 record crate 定义（跨 Tauri 边界 DTO），desktop emit 时用。前端 TS type 同步加 `polishing` stage。

## 4. 核心算法

### 4.1 构造润色输入（build_polish_input）

```rust
/// 把 cue 文本列表构造成带 [[N]] 标记的润色输入。
fn build_polish_input(texts: &[String]) -> String {
    let mut s = String::new();
    for (i, t) in texts.iter().enumerate() {
        s.push_str(&format!("[[{}]]{}", i + 1, t));
    }
    s
}
```

user prompt（在 `chat_text_with_prompt` 的 user 参数里）：

```
请润色以下语音识别文本，修正同音错字、补充标点、去除填充词（嗯/啊/那个）。
重要：保留 [[1]][[2]]... 这样的标记边界，每条标记对应一条字幕，不要合并或拆分标记。
仅输出润色后的文本（含标记），不要任何解释。

[[1]]第一句原文[[2]]第二句原文[[3]]...
```

system prompt 用现有 `octopus_llm::system_prompt()`（default-polish / advanced-polish + INCREMENTAL_RULE）。

### 4.2 解析标记输出（parse_polished_with_markers）

```rust
/// 解析 LLM 输出的 [[N]] 标记文本，返回按 N 排序的文本列表。
/// 失败（标记数量不符/格式错乱）返回 None，调用方走降级。
///
/// 实现用字符串 split（split("[[") + parse N + 找 "]]" 闭合），
/// 不依赖 regex crate（desktop 未依赖 regex；标记是固定字面量无需正则）。
fn parse_polished_with_markers(polished: &str, expected_count: usize) -> Option<Vec<String>> {
    // 伪代码（实际实现用 split）：
    // 1. split("[[") → 每段开头是 "N]]文本"
    // 2. 解析 N（parse 到 "]]" 为止），剩余是文本直到下一个 "[[" 或字符串末尾
    // 3. 收集到 HashMap<N, text>
    // 4. 检查 N 连续 1..=expected_count，不连续或数量不符 → None
    // 5. 任一文本为空 → None
    // 6. 返回按 N 排序的 Vec<String>
    todo!("见 plan 实现细节")
}
```

**判定失败的条件**（返回 None 走降级）：
- 标记数量 ≠ 预期 cue 数（LLM 合并/拆分了标记）
- N 不连续（缺号）
- 任一标记间文本为空

**判定失败的条件**（返回 None 走降级）：
- 标记数量 ≠ 预期 cue 数（LLM 合并/拆分了标记）
- N 不连续（缺号）
- 任一标记间文本为空

### 4.3 粗略拆分降级（split_polished_by_ratio）

LLM 输出无标记或解析失败时，按原 cue 的字符比例切润色后文本：

```rust
/// 按原 cue 的字符比例，把整段润色文本切回 N 段。
/// 尽力保留润色效果，但边界可能不准（LLM 可能改变了句子数量）。
fn split_polished_by_ratio(polished: &str, original_texts: &[String]) -> Vec<String> {
    let total_chars: usize = original_texts.iter().map(|t| t.chars().count()).sum();
    if total_chars == 0 {
        return original_texts.to_vec();
    }
    let polished_chars: Vec<char> = polished.chars().collect();
    let polished_total = polished_chars.len();
    let mut result = Vec::with_capacity(original_texts.len());
    let mut pos = 0;
    for (i, orig) in original_texts.iter().enumerate() {
        let ratio = orig.chars().count() as f64 / total_chars as f64;
        let end = if i == original_texts.len() - 1 {
            polished_total // 最后一条取剩余全部（避免四舍五入丢字）
        } else {
            (pos + (polished_total as f64 * ratio).round() as usize).min(polished_total)
        };
        let chunk: String = polished_chars[pos..end].iter().collect();
        result.push(chunk.trim().to_string());
        pos = end;
    }
    result
}
```

### 4.4 润色编排（polish_subtitle_cues）

```rust
/// 对 cue 文本列表做整段 LLM 润色。
/// 返回润色后的文本列表（长度与输入一致）。失败时返回原 texts（调用方据此提示）。
///
/// polish: Some 时润色，None 时直接返回原 texts。
/// 失败模式：LLM panic/超时/HTTP 错误 → 返回原 texts + log warn。
pub async fn polish_subtitle_cues(
    texts: Vec<String>,
    polish: &PolishOption,
    app: &AppHandle,
) -> (Vec<String>, PolishOutcome) {
    if texts.is_empty() {
        return (texts, PolishOutcome::Skipped);
    }
    // 1. 构造输入
    let input = build_polish_input(&texts);
    let system = octopus_llm::system_prompt();
    let user = format!(
        "请润色以下语音识别文本，修正同音错字、补充标点、去除填充词。\n\
         重要：保留 [[1]][[2]]... 标记边界，每条标记对应一条字幕，不要合并或拆分标记。\n\
         仅输出润色后的文本（含标记），不要任何解释。\n\n{}", input);
    // 2. 解析 LLM 配置
    let llm_config = resolve_subtitle_llm_config(&polish.llm_key);
    let llm_config = match llm_config {
        Some(c) => c,
        None => return (texts, PolishOutcome::NoLlmConfig),
    };
    // 3. spawn_blocking + catch_unwind（参考 coordinator.rs:1697-1724）
    let result = tokio::task::spawn_blocking(move || {
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            octopus_llm::chat_text_with_prompt(&system, &user, &llm_config, None)
        }))
    })
    .await;
    let polished = match result {
        Ok(Ok(Ok(text))) => text,
        Ok(Ok(Err(e))) => {
            log::warn!("[subtitle-polish] LLM 调用失败，用原文本: {e}");
            return (texts, PolishOutcome::Failed(e.to_string()));
        }
        Ok(Err(panic)) => {
            log::warn!("[subtitle-polish] LLM panic，用原文本: {:?}", panic);
            return (texts, PolishOutcome::Failed("LLM panicked".into()));
        }
        Err(e) => {
            log::warn!("[subtitle-polish] spawn_blocking join 失败: {e}");
            return (texts, PolishOutcome::Failed(e.to_string()));
        }
    };
    // 4. 解析标记
    if let Some(polished_texts) = parse_polished_with_markers(&polished, texts.len()) {
        (polished_texts, PolishOutcome::Polished)
    } else {
        // 5. 降级：粗略拆分
        log::warn!("[subtitle-polish] 标记解析失败（{}/{}），走粗略拆分降级",
            count_markers(&polished), texts.len());
        let split = split_polished_by_ratio(&polished, &texts);
        (split, PolishOutcome::FallbackRatio)
    }
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub enum PolishOutcome {
    Skipped,           // 未启用润色
    Polished,          // 标记解析成功
    FallbackRatio,     // 标记失败，粗略拆分
    NoLlmConfig,       // 无可用 LLM 配置
    Failed(String),    // LLM 调用失败（panic/超时/HTTP）
}
```

## 5. 接口设计

### 5.1 generate_subtitle 命令加 polish 参数

```rust
#[tauri::command]
pub async fn generate_subtitle(
    app: AppHandle,
    engine_manager: State<'_, Arc<AsrEngineManager>>,
    id: i64,
    track: Option<String>,
    polish: Option<PolishOption>,  // 新增：None=不润色，Some=润色
) -> Result<SubtitleResult, String>
```

前端 invoke：`invoke("generate_subtitle", { id, track, polish })`（polish 为 `{llmKey: "openai:gpt-4o"}` 或 null）。

### 5.2 SubtitleResult 加 polish_outcome 字段

```rust
pub struct SubtitleResult {
    pub cues: Vec<SubtitleCue>,
    pub srt_text: String,
    pub model: String,
    pub track_used: AudioTrackSource,
    pub polish_outcome: Option<PolishOutcome>,  // 新增：None=未尝试润色
}
```

前端据此显示提示：
- `Polished` → 无提示（正常）
- `FallbackRatio` → 黄色提示「LLM 标记解析失败，已粗略拆分，边界可能不准」
- `NoLlmConfig` → 提示「未配置可用 LLM，使用原始识别」
- `Failed(msg)` → 红色提示「LLM 润色失败：{msg}，使用原始识别」

### 5.3 新增命令：list_subtitle_llms（弹框下拉填充）

```rust
#[tauri::command]
pub async fn list_subtitle_llms() -> Result<Vec<LlmOption>, String> {
    // 查 DB 已配置 + is_enabled 的 LLM 列表，返回 [{key, label}]
    // key = "provider:model"，label = "GPT-4o (OpenAI)" 等友好名
}

pub struct LlmOption {
    pub key: String,     // "openai:gpt-4o"
    pub label: String,   // "GPT-4o (OpenAI)"
}
```

### 5.4 SubtitleProgress 加 Polishing

（见 §3.3）前端 listen 时处理 `polishing` stage，显示「✨ LLM 润色中...」。

### 5.5 错误处理与降级

| 失败场景 | 行为 |
|---|---|
| 用户未勾选润色 | polish=None，直接走原 ASR 流程 |
| 勾选但无可用 LLM 配置 | polish_outcome=NoLlmConfig，用原文本，前端提示 |
| LLM HTTP 错误/超时 | polish_outcome=Failed(msg)，用原文本，前端提示 |
| LLM panic | catch_unwind 捕获，polish_outcome=Failed，用原文本 |
| 标记解析失败 | polish_outcome=FallbackRatio，粗略拆分润色文本，前端提示 |
| 粗略拆分后文本为空 | 该 cue 用原文本兜底 |

**核心原则**：润色是「锦上添花」，任何失败都不阻塞字幕生成——用户至少拿到纯 ASR 字幕。

## 6. 前端 UI

### 6.1 转字幕弹对话框

点「转字幕」不再直接 invoke，而是弹一个轻量对话框：

```
┌─────────────────────────────────────┐
│  生成字幕                            │
│  ─────────────────────────────────  │
│  ☑ LLM 润色（修正识别/标点/填充词）  │
│                                      │
│  润色用 LLM：                        │
│  ┌─────────────────────────────┐    │
│  │ GPT-4o (OpenAI)        ▾    │    │
│  └─────────────────────────────┘    │
│                                      │
│  ASR 模型：sensevoice（跟随设置）    │
│                                      │
│            [取消]  [生成]            │
└─────────────────────────────────────┘
```

- checkbox 初始值 = Settings `subtitle_llm_polish_default`
- 下拉初始值 = Settings `subtitle_polish_llm_key`（空 = 默认 LLM）
- checkbox 取消勾选时，下拉禁用
- 「生成」按钮触发 invoke generate_subtitle，传 polish 参数
- 对话框用现有 frontend-design skill 设计视觉

### 6.2 进度反馈加 Polishing 阶段

现有进度条加第 4 阶段：

```
🎵 提取音轨...  [▬▬▬░░░░░] 20%
🎤 识别中...    [▬▬▬░░░░░] 40%
✨ LLM 润色...  [▬▬▬▬▬░░░] 65%   ← 新增
📐 生成字幕...  [▬▬▬▬▬▬▬░] 92%
✅ 完成（14 条）             ← + 润色结果提示（如有）
```

### 6.3 润色结果提示

字幕生成完成后，根据 `polish_outcome` 在 cue 面板顶部或 toast 显示：
- `FallbackRatio` → 黄色 toast「LLM 标记解析失败，已粗略拆分」
- `NoLlmConfig` / `Failed` → 红色 toast「LLM 润色失败，使用原始识别：{msg}」

### 6.4 Settings 加默认配置

Settings 录屏页加：
- 「字幕默认 LLM 润色」开关（持久化 `subtitle_llm_polish_default`）
- 「字幕默认润色 LLM」下拉（持久化 `subtitle_polish_llm_key`）

## 7. 测试策略

### 7.1 desktop subtitle_polish.rs 单测（TDD）

| 测试 | 覆盖点 | TDD |
|---|---|---|
| `build_polish_input_basic` | 3 条文本 → `[[1]]a[[2]]b[[3]]c` | ✅ 先写 |
| `parse_polished_with_markers_success` | 标记数量正确 → 解析出 N 段 | ✅ 先写 |
| `parse_polished_with_markers_count_mismatch` | 标记数 ≠ 预期 → None | ✅ 先写 |
| `parse_polished_with_markers_missing_n` | N 不连续（缺 [[2]]）→ None | ✅ 先写 |
| `parse_polished_with_markers_empty_text` | 标记间空文本 → None | ✅ 先写 |
| `parse_polished_with_markers_no_markers` | 完全无标记 → None | ✅ 先写 |
| `split_polished_by_ratio_basic` | 按字符比例切回 N 段 | ✅ 先写 |
| `split_polished_by_ratio_empty_original` | 原 texts 全空 → 返回原 texts | ✅ 先写 |
| `split_polished_by_ratio_last_takes_remainder` | 最后一条取剩余全部 | ✅ 先写 |

**不写单测**：`polish_subtitle_cues`（依赖真实 LLM HTTP，归 e2e）。

### 7.2 手动 e2e 验证清单

- [ ] 勾选润色 + GPT-4o → 字幕比纯 ASR 更通顺，`[[N]]` 标记解析成功
- [ ] 勾选润色 + 弱模型（故意输出无标记）→ 粗略拆分降级，前端黄色提示
- [ ] 关闭网络后勾选润色 → LLM 失败，用原 ASR，前端红色提示
- [ ] 不勾选润色 → 与 v2 行为完全一致（polish_outcome=None）
- [ ] Settings 默认开关 → 弹框 checkbox 初始值跟随
- [ ] 长录屏（>5min）润色 → 不超时（120s LLM 超时），UI 不卡死

## 8. 实施分阶段

### Phase 1：核心润色逻辑（desktop subtitle_polish.rs）

| 任务 | 文件 | 验证 |
|---|---|---|
| 1.1 `build_polish_input` + 1 TDD | `crates/desktop/src/subtitle_polish.rs` | 1 测试 |
| 1.2 `parse_polished_with_markers` + 5 TDD | 同上 | 5 测试 |
| 1.3 `split_polished_by_ratio` + 3 TDD | 同上 | 3 测试 |
| 1.4 `polish_subtitle_cues` 编排（含降级） | 同上 | 编译通过（e2e 验证） |

### Phase 2：命令层集成

| 任务 | 文件 | 验证 |
|---|---|---|
| 2.1 generate_subtitle 加 polish 参数 + step8.5 | `record_commands.rs` | 编译 |
| 2.2 SubtitleProgress 加 Polishing + emit | `subtitle.rs` + `record_commands.rs` | 编译 |
| 2.3 SubtitleResult 加 polish_outcome 字段 | `subtitle.rs` | 编译 + 前端 TS 同步 |
| 2.4 list_subtitle_llms 命令 | `record_commands.rs` | 编译 |
| 2.5 main.rs 注册 list_subtitle_llms | `main.rs` | invoke 可调 |

### Phase 3：配置（Settings）

| 任务 | 文件 | 验证 |
|---|---|---|
| 3.1 AppConfig 加两个字段 | `crates/infra/src/config.rs` | 编译 |
| 3.2 set_config 支持新字段 | `settings_commands.rs` | 持久化测试 |

### Phase 4：前端（frontend-design skill）

| 任务 | 文件 | 验证 |
|---|---|---|
| 4.1 转字幕弹对话框组件 | `RecordingPanel.tsx` | 手动 e2e |
| 4.2 list_subtitle_llms invoke 填充下拉 | 同上 | 手动 e2e |
| 4.3 Polishing 阶段进度显示 | 同上 | 手动 e2e |
| 4.4 polish_outcome 提示 UI | 同上 | 手动 e2e |
| 4.5 Settings 默认配置 UI | `RecordingPanel.tsx` 或 Settings 录屏页 | 手动 e2e |
| 4.6 i18n（润色相关文案） | `locales/` | 无缺失键 |

### 阶段依赖

```
Phase 1 (核心逻辑) → Phase 2 (命令集成) → Phase 3 (配置) ‖ Phase 4 (前端)
```

Phase 3 和 Phase 4 可并行（不同文件）。

## 9. 调研事实基础

| 维度 | 现状 | 对润色功能的影响 |
|---|---|---|
| 润色 API | `polish_regions`（整段）+ `chat_text_with_prompt`（通用）| 字幕用 chat_text_with_prompt（需自定义 user prompt 的标记格式指令，polish_regions 的 prompt 不支持标记） |
| LLM 配置 | `llm_config_ignore_mode()` + `resolve_active_engine("llm")` | 字幕命令已在 desktop 层，可直接拿 |
| 时间戳 | `PolishRegion` 无时间戳 | 润色只改 text，时间戳保持原 cue 的 start_ms/end_ms |
| panic 处理 | `catch_unwind`（coordinator.rs:1697-1724）| 字幕润色同样用 spawn_blocking + catch_unwind |
| 超时 | 120s 默认（`HTTP_TIMEOUT_SECS`）| 长录屏可能超时，失败降级用原文本 |
| system prompt | `system_prompt()` 全局（default-polish/advanced-polish）| 复用，user prompt 自定义标记格式指令 |
| 分层 | record/asr-local 不依赖 octopus-llm | 润色逻辑放 desktop `subtitle_polish.rs` |

关键代码引用：
- 润色 API：`crates/llm/src/client.rs:181`（chat_text_with_prompt）、`:198`（polish_regions）
- system prompt：`crates/llm/src/prompt.rs:34`（system_prompt()）
- LLM 配置解析：`crates/desktop/src/config.rs:46`（llm_config_ignore_mode）
- panic 处理先例：`crates/desktop/src/coordinator.rs:1697-1724`
- 字幕命令插入点：`crates/desktop/src/record_commands.rs::generate_subtitle_inner` step8→step10
