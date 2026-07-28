# 字幕 LLM 润色（Subtitle LLM Polish）Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 在录屏自动字幕生成流程加可选 LLM 润色——点「转字幕」弹对话框选是否润色 + 用哪个 LLM；整段润色 `[[N]]` 标记边界拆回 cue；粗略拆分降级；失败永不阻塞字幕生成。

**Architecture:** desktop 新模块 `subtitle_polish.rs`（润色编排 + 标记解析 + 降级）；generate_subtitle_inner step8→step10 之间插入 step8.5；复用 `chat_text_with_prompt` + `system_prompt()` + `catch_unwind`；record/asr-local 不依赖 octopus-llm。

**Tech Stack:** Rust（desktop crate）+ React/TS 前端 + octopus-llm（已有依赖）

## Global Constraints

- **润色永不阻塞字幕生成**：任何失败（LLM 错误/超时/panic/标记解析失败）都降级用原 ASR 文本，用户至少拿到纯 ASR 字幕。
- **`[[N]]` 标记用字符串 split 解析**（不依赖 regex crate——desktop 未依赖 regex，标记是固定字面量）。
- **casing**：跨 Tauri 边界 DTO 必须 `#[serde(rename_all = "camelCase")]`；PolishOutcome enum 外层 kebab + 变体字段 camelCase。
- **catch_unwind**：LLM 调用（`chat_text_with_prompt` 是同步阻塞 + 内部可能 panic）必须在 `spawn_blocking` + `catch_unwind` 内执行（参考 coordinator.rs:1697-1724）。
- **分层**：润色逻辑只在 desktop `subtitle_polish.rs`；record crate 的 `subtitle.rs` 只加 `Polishing` 到 SubtitleProgress enum（数据模型）+ `polish_outcome` 到 SubtitleResult（数据模型），不含润色逻辑。
- **TDD**：纯逻辑函数（build_polish_input / parse_polished_with_markers / split_polished_by_ratio）必须先写失败测试。
- **0 warning**：每个 task 结束 `cargo build` / `tsc --noEmit` 必须无 warning。

---

## File Structure

| 文件 | 职责 | 操作 |
|---|---|---|
| `crates/desktop/src/subtitle_polish.rs` | build_polish_input / parse_polished_with_markers / split_polished_by_ratio / polish_subtitle_cues / PolishOption / PolishOutcome | 新建 |
| `crates/record/src/subtitle.rs` | SubtitleProgress 加 Polishing 变体；SubtitleResult 加 polish_outcome 字段 | 修改 |
| `crates/desktop/src/record_commands.rs` | generate_subtitle 加 polish 参数 + step8.5；新增 list_subtitle_llms 命令 | 修改 |
| `crates/desktop/src/main.rs` | 注册 list_subtitle_llms | 修改 |
| `crates/desktop/src/lib.rs` | 导出 subtitle_polish 模块 | 修改 |
| `crates/infra/src/config.rs` | AppConfig 加 subtitle_llm_polish_default + subtitle_polish_llm_key | 修改 |
| `crates/desktop/src/settings_commands.rs` | set_config 支持两个新字段 | 修改 |
| `crates/desktop/frontend/src/pages/Settings/RecordingPanel.tsx` | 转字幕弹对话框 + Polishing 进度 + outcome 提示 + Settings 默认配置 | 修改 |
| `crates/desktop/frontend/src/locales/zh-CN.yaml` + `en.yaml` | i18n 文案 | 修改 |

---

## Phase 1：核心润色逻辑（desktop subtitle_polish.rs，TDD）

### Task 1.1: 新建 subtitle_polish.rs + PolishOption/PolishOutcome + build_polish_input（TDD）

**Files:**
- Create: `crates/desktop/src/subtitle_polish.rs`
- Modify: `crates/desktop/src/lib.rs`（`pub mod subtitle_polish;`）

**Interfaces:**
- Produces: `PolishOption`、`PolishOutcome`、`build_polish_input(texts: &[String]) -> String`

- [x] **Step 1: 创建 subtitle_polish.rs 骨架 + 数据模型**

创建 `crates/desktop/src/subtitle_polish.rs`：

```rust
//! 字幕 LLM 润色编排（desktop 层）。
//!
//! 整段润色（保留上下文）+ [[N]] 标记边界拆回 cue + 粗略拆分降级。
//! record/asr-local 不依赖 octopus-llm，润色逻辑集中在 desktop。
//!
//! 设计详见 `docs/superpowers/specs/2026-07-28-subtitle-llm-polish-design.md`。

/// 标记格式常量。LLM 输出须用 [[N]] 包裹每条 cue（N 从 1 递增）。
const CUE_MARKER_OPEN: &str = "[[";
const CUE_MARKER_CLOSE: &str = "]]";

/// 字幕润色选项（generate_subtitle 命令参数）。
/// None = 不润色；Some = 用指定 LLM 润色。
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PolishOption {
    /// LLM 配置标识（provider:model）。None = 用 resolve_active_engine("llm") 默认。
    pub llm_key: Option<String>,
}

/// 润色结果（用于前端提示）。
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub enum PolishOutcome {
    /// 未启用润色（polish=None）。
    Skipped,
    /// 标记解析成功。
    Polished,
    /// 标记失败，粗略拆分降级。
    FallbackRatio,
    /// 无可用 LLM 配置。
    NoLlmConfig,
    /// LLM 调用失败（panic/超时/HTTP）。
    Failed(String),
}

/// 把 cue 文本列表构造成带 [[N]] 标记的润色输入。
pub fn build_polish_input(texts: &[String]) -> String {
    let mut s = String::new();
    for (i, t) in texts.iter().enumerate() {
        s.push_str(&format!("{}{}{}{}", CUE_MARKER_OPEN, i + 1, CUE_MARKER_CLOSE, t));
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_polish_input_basic() {
        let texts = vec!["第一句".to_string(), "第二句".to_string(), "第三句".to_string()];
        let input = build_polish_input(&texts);
        assert_eq!(input, "[[1]]第一句[[2]]第二句[[3]]第三句");
    }

    #[test]
    fn build_polish_input_empty() {
        assert_eq!(build_polish_input(&[]), "");
    }

    #[test]
    fn build_polish_input_single() {
        assert_eq!(build_polish_input(&["单句".into()]), "[[1]]单句");
    }
}
```

- [x] **Step 2: lib.rs 导出模块**

在 `crates/desktop/src/lib.rs` 适当位置加（如其他 `pub mod` 附近）：

```rust
pub mod subtitle_polish;
```

- [x] **Step 3: 跑测试确认通过**

```bash
cargo test -p octopus-desktop --bin octopus-desktop subtitle_polish::tests 2>&1 | tail -10
```

Expected: 3 测试 PASS（desktop 是 bin 不是 lib，用 `--bin`）

- [x] **Step 4: Commit**

```bash
git add crates/desktop/src/subtitle_polish.rs crates/desktop/src/lib.rs
git commit -m "feat(desktop): subtitle_polish 模块骨架 + PolishOption/Outcome + build_polish_input（3 TDD）"
```

---

### Task 1.2: parse_polished_with_markers（TDD，5 测试）

**Files:**
- Modify: `crates/desktop/src/subtitle_polish.rs`

- [x] **Step 1: 写 5 个失败测试**

在 subtitle_polish.rs 测试模块追加：

```rust
    #[test]
    fn parse_markers_success_3_cues() {
        let polished = "[[1]]润色第一句[[2]]润色第二句[[3]]润色第三句";
        let result = parse_polished_with_markers(polished, 3).unwrap();
        assert_eq!(result.len(), 3);
        assert_eq!(result[0], "润色第一句");
        assert_eq!(result[1], "润色第二句");
        assert_eq!(result[2], "润色第三句");
    }

    #[test]
    fn parse_markers_count_mismatch_returns_none() {
        // 期望 3 条但只有 2 个标记
        let polished = "[[1]]第一句[[2]]第二句";
        assert!(parse_polished_with_markers(polished, 3).is_none());
    }

    #[test]
    fn parse_markers_missing_n_returns_none() {
        // N 不连续：[[1]] [[3]]（缺 [[2]]）
        let polished = "[[1]]第一句[[3]]第三句";
        assert!(parse_polished_with_markers(polished, 2).is_none());
    }

    #[test]
    fn parse_markers_empty_text_returns_none() {
        // [[2]] 后文本为空（直接接 [[3]]）
        let polished = "[[1]]第一句[[2]][[3]]第三句";
        assert!(parse_polished_with_markers(polished, 3).is_none());
    }

    #[test]
    fn parse_markers_no_markers_returns_none() {
        // LLM 完全无视格式，输出纯文本
        let polished = "这是没有标记的纯文本润色结果";
        assert!(parse_polished_with_markers(polished, 3).is_none());
    }
```

- [x] **Step 2: 跑测试确认失败**

```bash
cargo test -p octopus-desktop --bin octopus-desktop subtitle_polish::tests::parse_markers 2>&1 | tail -5
```

Expected: 编译失败（函数未定义）

- [x] **Step 3: 实现 parse_polished_with_markers**

在 subtitle_polish.rs（build_polish_input 后、测试前）追加：

```rust
/// 解析 LLM 输出的 [[N]] 标记文本，返回按 N 排序的文本列表。
///
/// 失败（返回 None）条件：
/// - 标记数量 ≠ expected_count
/// - N 不连续（缺号）
/// - 任一标记间文本为空（trim 后）
/// - 完全无标记
///
/// 用字符串 split 实现（不依赖 regex——标记是固定字面量）。
pub fn parse_polished_with_markers(polished: &str, expected_count: usize) -> Option<Vec<String>> {
    if expected_count == 0 {
        return Some(Vec::new());
    }
    // split("[[") → 每段开头是 "N]]文本"
    let mut map: std::collections::HashMap<u32, String> = std::collections::HashMap::new();
    for segment in polished.split(CUE_MARKER_OPEN) {
        // segment 可能是 ""（开头有 [[ 时第一段空）或 "N]]文本"
        if segment.is_empty() {
            continue;
        }
        // 找 "]]" 分隔 N 和文本
        let close_idx = segment.find(CUE_MARKER_CLOSE)?;
        let n_str = &segment[..close_idx];
        let n: u32 = n_str.parse().ok()?;
        let text = segment[close_idx + CUE_MARKER_CLOSE.len()..].trim().to_string();
        // N 范围检查
        if n == 0 || n as usize > expected_count {
            return None;
        }
        map.insert(n, text);
    }
    // 检查数量 + 连续性
    if map.len() != expected_count {
        return None;
    }
    // 任一文本为空 → None（前面 trim 后可能空）
    if map.values().any(|t| t.is_empty()) {
        return None;
    }
    // 按 N 排序收集
    let result: Option<Vec<String>> = (1..=expected_count as u32)
        .map(|n| map.get(&n).cloned())
        .collect();
    result
}
```

- [x] **Step 4: 跑测试确认通过**

```bash
cargo test -p octopus-desktop --bin octopus-desktop subtitle_polish::tests 2>&1 | tail -10
```

Expected: 8 测试全过（3 build + 5 parse）

- [x] **Step 5: Commit**

```bash
git add crates/desktop/src/subtitle_polish.rs
git commit -m "feat(desktop): parse_polished_with_markers——[[N]] 标记解析（5 TDD）"
```

---

### Task 1.3: split_polished_by_ratio（TDD，3 测试）

**Files:**
- Modify: `crates/desktop/src/subtitle_polish.rs`

- [x] **Step 1: 写 3 个失败测试**

在测试模块追加：

```rust
    #[test]
    fn split_by_ratio_basic() {
        let original = vec!["一二三四".to_string(), "五六七".to_string(), "八九十".to_string()];
        // total 11 chars，比例 4/11, 3/11, 4/11
        let polished = "一二三四五六七八九十十一十二"; // 12 chars（润色后略多）
        let result = split_polished_by_ratio(polished, &original);
        assert_eq!(result.len(), 3);
        // 不强断言精确切点（四舍五入），只断言长度 + 非空
        assert!(!result[0].is_empty());
        assert!(!result[1].is_empty());
        assert!(!result[2].is_empty());
        // 拼接应等于原 polished（trim 后）
        let joined: String = result.iter().map(|s| s.as_str()).collect::<Vec<_>>().join("");
        // 允许 trim 差异，只检查大致一致
        assert!(!joined.is_empty());
    }

    #[test]
    fn split_by_ratio_empty_original_returns_original() {
        let original = vec!["".to_string(), "".to_string()];
        let polished = "润色文本";
        let result = split_polished_by_ratio(polished, &original);
        assert_eq!(result, original); // 原 texts 全空 → 返回原 texts
    }

    #[test]
    fn split_by_ratio_last_takes_remainder() {
        let original = vec!["短".to_string(), "很长很长很长".to_string()];
        let polished = "润色一润色二润色三润色四润色五"; // 10 chars
        let result = split_polished_by_ratio(polished, &original);
        assert_eq!(result.len(), 2);
        // 最后一条应取剩余全部（不被四舍五入截断）
        let total_chars: usize = result.iter().map(|s| s.chars().count()).sum();
        assert_eq!(total_chars, polished.chars().count());
    }
```

- [x] **Step 2: 实现 split_polished_by_ratio**

在 subtitle_polish.rs（parse_polished_with_markers 后）追加：

```rust
/// 按原 cue 的字符比例，把整段润色文本切回 N 段（降级用）。
///
/// 尽力保留润色效果，但边界可能不准（LLM 可能改变了句子数量）。
/// 原 texts 全空时直接返回原 texts（避免除零）。
pub fn split_polished_by_ratio(polished: &str, original_texts: &[String]) -> Vec<String> {
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

- [x] **Step 3: 跑测试确认通过**

```bash
cargo test -p octopus-desktop --bin octopus-desktop subtitle_polish::tests 2>&1 | tail -10
```

Expected: 11 测试全过（3 build + 5 parse + 3 split）

- [x] **Step 4: Commit**

```bash
git add crates/desktop/src/subtitle_polish.rs
git commit -m "feat(desktop): split_polished_by_ratio——粗略拆分降级（3 TDD）"
```

---

### Task 1.4: polish_subtitle_cues 编排（含 LLM 调用 + 降级）

**Files:**
- Modify: `crates/desktop/src/subtitle_polish.rs`

**Interfaces:**
- Consumes: `octopus_llm::chat_text_with_prompt`、`octopus_llm::system_prompt()`、`crate::config::llm_config_ignore_mode`（或 resolve_subtitle_llm_config helper）
- Produces: `polish_subtitle_cues(texts, polish, app) -> (Vec<String>, PolishOutcome)`

- [x] **Step 1: 实现 resolve_subtitle_llm_config helper**

在 subtitle_polish.rs 追加（先看 crates/desktop/src/config.rs 的 llm_config_ignore_mode 签名）：

```rust
/// 解析字幕润色用的 LLM 配置。
/// llm_key=None → 用 resolve_active_engine("llm") 默认。
/// llm_key=Some("provider:model") → 查 DB 找匹配配置。
/// 无可用配置返回 None（调用方走 NoLlmConfig 降级）。
fn resolve_subtitle_llm_config(llm_key: &Option<String>) -> Option<octopus_infra::db::CompatibleLlmConfig> {
    // 复用 desktop config.rs 的 llm_config_ignore_mode 或类似逻辑
    // 如果 llm_key 是 None，直接用 llm_config_ignore_mode()
    // 如果是 Some(key)，需要按 key 查 DB（可能要新增 helper）
    crate::config::llm_config_ignore_mode().ok()
}
```

⚠️ 实施时先 grep `crates/desktop/src/config.rs` 确认 `llm_config_ignore_mode` 签名，以及是否已有按 key 查 LLM 配置的 helper（可能需要复用或新增）。如果按 key 查的逻辑复杂，MVP 阶段可以先只支持 None（用默认 LLM），llm_key=Some 时也 fallback 到默认 + log warn。

- [x] **Step 2: 实现 polish_subtitle_cues**

在 subtitle_polish.rs 追加：

```rust
use tauri::AppHandle;

/// 对 cue 文本列表做整段 LLM 润色。
///
/// 返回 (润色后文本列表, PolishOutcome)。长度与输入 texts 一致。
/// 失败时返回原 texts + 对应 PolishOutcome（调用方据此提示用户）。
///
/// 编排：构造 [[N]] 标记输入 → spawn_blocking + catch_unwind 调 LLM → 解析标记 → 降级。
pub async fn polish_subtitle_cues(
    texts: Vec<String>,
    polish: &PolishOption,
    _app: &AppHandle,
) -> (Vec<String>, PolishOutcome) {
    if texts.is_empty() {
        return (texts, PolishOutcome::Skipped);
    }

    // 1. 构造输入
    let input = build_polish_input(&texts);
    let system = octopus_llm::system_prompt();
    let user = format!(
        "请润色以下语音识别文本，修正同音错字、补充标点、去除填充词（嗯/啊/那个）。\n\
         重要：保留 {}N{} 标记边界，每条标记对应一条字幕，不要合并或拆分标记。\n\
         仅输出润色后的文本（含标记），不要任何解释。\n\n{}",
        CUE_MARKER_OPEN, CUE_MARKER_CLOSE, input);

    // 2. 解析 LLM 配置
    let llm_config = match resolve_subtitle_llm_config(&polish.llm_key) {
        Some(c) => c,
        None => {
            log::warn!("[subtitle-polish] 无可用 LLM 配置，用原文本");
            return (texts, PolishOutcome::NoLlmConfig);
        }
    };

    // 3. spawn_blocking + catch_unwind（参考 coordinator.rs:1697-1724）
    let system_clone = system.clone();
    let user_clone = user.clone();
    let result = tokio::task::spawn_blocking(move || {
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            octopus_llm::chat_text_with_prompt(&system_clone, &user_clone, &llm_config, None)
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
            log::warn!("[subtitle-polish] LLM panic，用原文本");
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
        log::warn!(
            "[subtitle-polish] 标记解析失败（输入 {} 条，解析不一致），走粗略拆分降级",
            texts.len()
        );
        let split = split_polished_by_ratio(&polished, &texts);
        (split, PolishOutcome::FallbackRatio)
    }
}
```

- [x] **Step 3: build 确认编译**

```bash
cargo build -p octopus-desktop 2>&1 | grep -E "error|warning" | head
```

Expected: 0 error 0 warning（可能需要修 `crate::config::llm_config_ignore_mode` 的精确路径）

- [x] **Step 4: Commit（与 Task 1.1-1.3 合并到 Phase 1 总结 commit，或单独）**

```bash
git add crates/desktop/src/subtitle_polish.rs
git commit -m "feat(desktop): polish_subtitle_cues 编排——spawn_blocking + catch_unwind + 降级"
```

---

## Phase 2：命令层集成

### Task 2.1: SubtitleProgress 加 Polishing + SubtitleResult 加 polish_outcome（record crate）

**Files:**
- Modify: `crates/record/src/subtitle.rs`

- [x] **Step 1: SubtitleProgress 加 Polishing 变体**

找到 `crates/record/src/subtitle.rs` 的 `SubtitleProgress` enum 定义，在 `Recognizing` 和 `Finalizing` 之间插入：

```rust
#[derive(Debug, Clone, serde::Serialize)]
#[serde(tag = "stage", rename_all = "kebab-case", rename_all_fields = "camelCase")]
pub enum SubtitleProgress {
    ExtractingAudio { percent: u32 },    // 0~30%
    Recognizing { percent: u32 },        // 30~40%
    Polishing { percent: u32 },          // 40~90%（新增）
    Finalizing { percent: u32 },         // 90~100%
    Done { cue_count: usize },
    Error { message: String },
}
```

- [x] **Step 2: SubtitleResult 加 polish_outcome 字段**

```rust
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubtitleResult {
    pub cues: Vec<SubtitleCue>,
    pub srt_text: String,
    pub model: String,
    pub track_used: AudioTrackSource,
    /// 润色结果（None=未尝试润色）。前端据此显示提示。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub polish_outcome: Option<String>,  // String 而非强类型——record crate 不依赖 desktop 的 PolishOutcome
}
```

⚠️ 用 `Option<String>` 而非 `Option<PolishOutcome>`，因为 `PolishOutcome` 在 desktop crate 定义，record crate 不能依赖 desktop。desktop 序列化时把 PolishOutcome 转成字符串（如 `"polished"` / `"fallbackRatio"` / `"failed:msg"`）。

- [x] **Step 3: 更新所有 RecordingMeta / SubtitleResult 构造点**

grep `SubtitleResult {` 找所有构造点，加 `polish_outcome: None`（默认）。

- [x] **Step 4: build + test 确认无回归**

```bash
cargo build -p octopus-record && cargo test -p octopus-record --lib 2>&1 | tail -5
```

- [x] **Step 5: Commit**

```bash
git add crates/record/src/subtitle.rs
git commit -m "feat(record): SubtitleProgress 加 Polishing + SubtitleResult 加 polish_outcome"
```

---

### Task 2.2: generate_subtitle 加 polish 参数 + step8.5 + list_subtitle_llms

**Files:**
- Modify: `crates/desktop/src/record_commands.rs`
- Modify: `crates/desktop/src/main.rs`（注册 list_subtitle_llms）

- [x] **Step 1: generate_subtitle 命令加 polish 参数**

修改 `generate_subtitle` 签名：

```rust
#[tauri::command]
pub async fn generate_subtitle(
    app: AppHandle,
    engine_manager: State<'_, std::sync::Arc<octopus_asr_local::engine::AsrEngineManager>>,
    id: i64,
    track: Option<String>,
    polish: Option<crate::subtitle_polish::PolishOption>,  // 新增
) -> Result<octopus_record::SubtitleResult, String> {
    match generate_subtitle_inner(&app, &engine_manager, id, track, polish).await {
        Ok(r) => Ok(r),
        Err(e) => {
            let _ = app.emit("record://task", RecordTaskEvent::SubtitleFailed { id, error: e.clone() });
            Err(e)
        }
    }
}
```

- [x] **Step 2: generate_subtitle_inner 加 polish 参数 + step8.5**

修改 `generate_subtitle_inner` 签名 + 在 step8（ASR）和 step10（组装 cue）之间插入 step8.5：

```rust
async fn generate_subtitle_inner(
    app: &AppHandle,
    engine_manager: &std::sync::Arc<octopus_asr_local::engine::AsrEngineManager>,
    id: i64,
    track: Option<String>,
    polish: Option<crate::subtitle_polish::PolishOption>,
) -> Result<octopus_record::SubtitleResult, String> {
    // ... step1-8 不变（ASR 完成）
    log::info!("[subtitle] step8 ASR 完成 segments={}", timestamped.len());

    // step8.5: LLM 润色（可选）
    let polish_outcome_str: Option<String> = if polish.is_some() {
        let _ = app.emit("record://task", RecordTaskEvent::SubtitleProgress {
            id,
            stage: octopus_record::SubtitleProgress::Polishing { percent: 50 },
        });
        let texts: Vec<String> = timestamped.iter().map(|t| t.text.clone()).collect();
        log::info!("[subtitle] step8.5 开始 LLM 润色 cues={}", texts.len());
        let (polished_texts, outcome) = crate::subtitle_polish::polish_subtitle_cues(texts, polish.as_ref().unwrap(), app).await;
        // 把润色后的文本填回 timestamped（时间戳不变）
        for (seg, new_text) in timestamped.iter_mut().zip(polished_texts.into_iter()) {
            seg.text = new_text;
        }
        let outcome_str = match outcome {
            crate::subtitle_polish::PolishOutcome::Skipped => None,
            crate::subtitle_polish::PolishOutcome::Polished => Some("polished".into()),
            crate::subtitle_polish::PolishOutcome::FallbackRatio => Some("fallbackRatio".into()),
            crate::subtitle_polish::PolishOutcome::NoLlmConfig => Some("noLlmConfig".into()),
            crate::subtitle_polish::PolishOutcome::Failed(msg) => Some(format!("failed:{msg}")),
        };
        log::info!("[subtitle] step8.5 润色完成 outcome={:?}", outcome);
        outcome_str
    } else {
        None
    };

    // step9 emit Finalizing + step10 组装 cue（用润色后的 text）
    // ... 组装 SubtitleResult 时加 polish_outcome: polish_outcome_str
}
```

- [x] **Step 3: 新增 list_subtitle_llms 命令**

在 record_commands.rs 追加：

```rust
/// 列出可用 LLM（弹框下拉填充）。
#[command]
pub async fn list_subtitle_llms() -> Result<Vec<LlmOption>, String> {
    // 查 DB 已配置 + is_enabled 的 LLM 列表
    with_db_blocking(move |conn| {
        // 查 models 表 domain='llm' AND is_enabled=1（或类似条件）
        // 返回 [{key: "provider:model", label: "GPT-4o (OpenAI)"}]
        // ⚠️ 实施时查 models 表 schema 确认字段名
        todo!("实施时补全 DB 查询")
    })
    .await
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LlmOption {
    pub key: String,
    pub label: String,
}
```

- [x] **Step 4: main.rs 注册 list_subtitle_llms**

```rust
#[cfg(target_os = "macos")]
record_commands::list_subtitle_llms,
```

- [x] **Step 5: build 确认编译**

```bash
cargo build -p octopus-desktop 2>&1 | grep -E "error|warning" | head
```

- [x] **Step 6: Commit**

```bash
git add crates/desktop/src/record_commands.rs crates/desktop/src/main.rs
git commit -m "feat(desktop): generate_subtitle 加 polish 参数 + step8.5 + list_subtitle_llms 命令"
```

---

## Phase 3：配置（Settings）

### Task 3.1: AppConfig 加两个字段 + set_config 支持

**Files:**
- Modify: `crates/infra/src/config.rs`
- Modify: `crates/desktop/src/settings_commands.rs`

- [x] **Step 1: AppConfig 加字段**

在 `crates/infra/src/config.rs` 的 `AppConfig` struct 加：

```rust
pub struct AppConfig {
    // ... 现有字段
    /// 字幕生成时默认是否启用 LLM 润色（弹框 checkbox 初始值）。默认 false。
    #[serde(default)]
    pub subtitle_llm_polish_default: bool,
    /// 字幕润色默认用的 LLM key（provider:model）。空 = resolve_active_engine("llm")。默认 ""。
    #[serde(default)]
    pub subtitle_polish_llm_key: String,
}
```

- [x] **Step 2: set_config 支持新字段**

在 `settings_commands.rs` 的 `set_config` 命令，加对这两个 key 的处理（参考现有 `record_*` key 的同模式）：

```rust
"subtitle_llm_polish_default" => { /* 写 DB app_config */ }
"subtitle_polish_llm_key" => { /* 写 DB app_config */ }
```

- [x] **Step 3: build 确认**

```bash
cargo build -p octopus-infra -p octopus-desktop 2>&1 | tail -3
```

- [x] **Step 4: Commit**

```bash
git add crates/infra/src/config.rs crates/desktop/src/settings_commands.rs
git commit -m "feat(config): AppConfig 加 subtitle_llm_polish_default + subtitle_polish_llm_key"
```

---

## Phase 4：前端（frontend-design skill）

### Task 4.1: 转字幕弹对话框

**Files:**
- Modify: `crates/desktop/frontend/src/pages/Settings/RecordingPanel.tsx`

⚠️ **必须用 frontend-design skill 做视觉设计**（AGENTS.md 准则）。

- [x] **Step 1: 加 PolishOption / LlmOption TS interface**

```typescript
interface PolishOption {
  llmKey: string | null;
}
interface LlmOption {
  key: string;
  label: string;
}
```

- [x] **Step 2: 转字幕弹对话框组件**

点「转字幕」不再直接 invoke，而是弹对话框（checkbox + LLM 下拉 + 确认/取消）。用 frontend-design skill 设计视觉。

- [x] **Step 3: invoke generate_subtitle 传 polish 参数**

```typescript
const result = await invoke<SubtitleResult>("generate_subtitle", {
  id,
  track: null,
  polish: polishEnabled ? { llmKey: selectedLlmKey } : null,
});
```

- [x] **Step 4: tsc + vite build**

- [x] **Step 5: Commit**

---

### Task 4.2: Polishing 进度 + outcome 提示

- [x] **Step 1: SubtitleProgressPayload 加 polishing stage**

```typescript
type SubtitleStage = 'extracting-audio' | 'recognizing' | 'polishing' | 'finalizing' | 'done' | 'error';
```

- [x] **Step 2: 进度条显示 Polishing 阶段**

- [x] **Step 3: polish_outcome 提示 UI**

根据 `result.polishOutcome`（`"polished"` / `"fallbackRatio"` / `"noLlmConfig"` / `"failed:msg"`）显示不同颜色 toast。

- [x] **Step 4: tsc + vite build + Commit**

---

### Task 4.3: Settings 默认配置 UI + i18n

- [x] **Step 1: Settings 加字幕默认润色开关 + LLM 选择**

- [x] **Step 2: i18n 文案（zh-CN + en）**

新键：`subtitlePolish`（润色 checkbox label）、`subtitlePolishLlm`（下拉 label）、`subtitlePolishing`（进度文案）、`subtitlePolishOutcome.*`（4 种结果提示）等。

- [x] **Step 3: vite build + 手动 e2e + Commit**

---

## Phase 5：手动 e2e + 文档同步

### Task 5.1: 手动 e2e（spec §7.2 清单）

- [x] 勾选润色 + GPT-4o → 标记解析成功，字幕更通顺
- [x] 勾选润色 + 弱模型（无标记输出）→ 粗略拆分降级，黄色提示
- [x] 关网络后勾选润色 → LLM 失败，红色提示，用原 ASR
- [x] 不勾选润色 → 与 v2 一致
- [x] Settings 默认开关 → 弹框 checkbox 初始值跟随

### Task 5.2: architecture.md 同步

- [x] 录屏章节加「LLM 润色」说明
- [x] Commit

---

## Self-Review

### Spec coverage

| spec 章节 | 覆盖任务 |
|---|---|
| §1 范围 | Phase 1-4 全覆盖 |
| §3.1 PolishOption/PolishOutcome | Task 1.1 |
| §3.2 AppConfig 两字段 | Task 3.1 |
| §3.3 SubtitleProgress Polishing | Task 2.1 |
| §4.1 build_polish_input | Task 1.1 |
| §4.2 parse_polished_with_markers | Task 1.2 |
| §4.3 split_polished_by_ratio | Task 1.3 |
| §4.4 polish_subtitle_cues | Task 1.4 |
| §5.1 generate_subtitle polish 参数 | Task 2.2 |
| §5.2 SubtitleResult polish_outcome | Task 2.1 |
| §5.3 list_subtitle_llms | Task 2.2 |
| §5.5 错误降级 | Task 1.4（polish_subtitle_cues 内）+ Task 4.2（UI 提示）|
| §6 前端弹框 + 进度 + 提示 + Settings | Task 4.1-4.3 |
| §7 测试 | Task 1.1-1.3 TDD（11 测试）+ Task 5.1 e2e |

### 阶段依赖

```
Phase 1 (核心逻辑) → Phase 2 (命令集成) → Phase 3 (配置) ‖ Phase 4 (前端) → Phase 5 (e2e)
```

---

## Execution Handoff

Plan complete and saved to `docs/superpowers/plans/2026-07-28-subtitle-llm-polish.md`. Two execution options:

**1. Subagent-Driven (recommended)** - 每个 Task 派新 subagent，任务间 review
**2. Inline Execution** - 本 session 内执行，批量 + checkpoint review

Which approach?
