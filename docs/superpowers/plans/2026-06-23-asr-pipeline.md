# ASR Pipeline 重构（阶段1：批处理 helper + cli）实施计划

> **For agentic workers:** REQUIRED SUB-SKILL: 用 superpowers:subagent-driven-development（推荐）或 superpowers:executing-plans 按 task 实施。Steps 用 checkbox（`- [ ]`）跟踪。
>
> spec：`docs/superpowers/specs/2026-06-23-asr-pipeline-design.md`。worktree：`worktree-model-mgmt-ui`。

**Scope（重要）**：本计划只覆盖 **阶段1 = asr 批处理 helper + cli 走新 pipeline**。spec 里的流式 trait（`StreamingEngine`/`AudioSource`/`TranscriptEvent`）、`StreamingRunner`、desktop `StreamingPipeline`（coordinator 状态机拆分）、server、denoise 迁入 runner —— 全部留**后续 plan**（阶段2 desktop、阶段3 server）。阶段1 完成后：cli 走新 `transcribe_batch`（补齐原 cli 缺失的 VAD/纠错/简繁链），desktop 仍用旧 `transcribe_with_vad`（行为不变，因旧入口委托新 helper），互不阻塞。

**Goal**：asr 新增 `pipeline` 模块（`PipelineConfig` + `transcribe_batch`），把 `transcribe_with_vad` 的 VAD 分段编排收编、纠错/简繁从「读全局 app_config」参数化；cli `do_transcribe` 改走 `AsrEngineManager + transcribe_batch`。

**Architecture**：`transcribe_batch(engine, samples, &cfg)` 收编 `transcribe_with_vad` 主体，`correct`/`simplify` 改读 `cfg`（`ngram` 预留位，未实现仅 warn）。`transcribe_with_vad` 退化为「从 app_config 构造 cfg → 委托 transcribe_batch」，desktop 行为零变化。cli 经 `AsrEngineManager.transcribe_batch` 复用同一编排。

**Tech Stack**：Rust、anyhow、log、octopus-asr / octopus-infra / octopus-cli crate。

---

## File Structure

| 文件 | 动作 | 职责 |
|---|---|---|
| `crates/asr/src/pipeline.rs` | **Create** | `PipelineConfig` + `transcribe_batch` + `transcribe_segments`（VAD 分段主体）+ 单测 |
| `crates/asr/src/lib.rs` | Modify | `pub mod pipeline;` |
| `crates/asr/src/engine.rs` | Modify | `transcribe_with_vad` 改委托；`AsrEngineManager` 加 `transcribe_batch` 方法 |
| `crates/cli/src/pipeline.rs` | **Create** | cli 批处理 pipeline 入口 `run(model, language, samples)` |
| `crates/cli/src/main.rs` | Modify | `mod pipeline;` + `do_transcribe` 改调 `pipeline::run` |
| `docs/architecture.md` | Modify | 新增 `asr::pipeline` 模块说明 |

---

## Task 1：asr 新增 `pipeline.rs` 模块 + `PipelineConfig`

**Files:**
- Create: `crates/asr/src/pipeline.rs`
- Modify: `crates/asr/src/lib.rs`（在 `pub mod hans;` 后加 `pub mod pipeline;`）

- [ ] **Step 1: 创建 `crates/asr/src/pipeline.rs`**

```rust
//! ASR pipeline 编排：批处理 helper（流式 helper / StreamingRunner 见后续阶段）。
//!
//! `transcribe_batch` 收编原 `engine::transcribe_with_vad` 的 VAD 分段编排，把纠错
//! （`correct`）与简繁归一化（`simplify`）从「读全局 app_config」参数化为 `PipelineConfig`
//! 字段，使编排可被多端（cli/desktop/server）以明确参数复用，而非隐式依赖全局配置。
//!
//! 设计详见 `docs/superpowers/specs/2026-06-23-asr-pipeline-design.md`。

use crate::config::load_app_config_cached;
use crate::engine::OfflineAsrEngine;

/// 批处理 pipeline 配置。
///
/// 阶段1 精简版：`correct` / `simplify` 在 `transcribe_batch` 内替代原 `transcribe_with_vad`
/// 对全局 `app_config` 的读取；`ngram` 为预留字段（解码纠错，尚未实现）。流式相关字段
/// （`backend` / `denoise` / 音频源）随阶段2 流式 helper 加入。
pub struct PipelineConfig {
    pub language: String,
    /// 是否对 ASR 输出做拼音/bigram 纠错（原 `app_config.asr_correct`）。
    pub correct: bool,
    /// true→输出简体，false→输出繁体（原 `app_config.output_simplified`）。
    pub simplify: bool,
    /// ngram 解码纠错开关（预留，尚未实现；`transcribe_batch` 见到 true 仅 warn）。
    pub ngram: bool,
}

impl PipelineConfig {
    /// 从全局 `app_config` 构造（向后兼容 `transcribe_with_vad` / desktop 既有行为）。
    pub fn from_app_config(language: &str) -> Self {
        let app = load_app_config_cached();
        Self {
            language: language.to_string(),
            correct: app.asr_correct,
            simplify: app.output_simplified,
            ngram: false,
        }
    }
}
```

- [ ] **Step 2: 在 `crates/asr/src/lib.rs` 注册模块**

在 `pub mod hans;`（第 23 行）后加一行：

```rust
pub mod pipeline;
```

- [ ] **Step 3: 编译验证**

Run: `cargo check -p octopus-asr`
Expected: 通过（`PipelineConfig` 暂无调用点，`load_app_config_cached` 字段名 `asr_correct` / `output_simplified` 已核对存在）。

- [ ] **Step 4: Commit**

```bash
git add crates/asr/src/pipeline.rs crates/asr/src/lib.rs
git commit -m "feat(asr): 新增 pipeline 模块 + PipelineConfig（阶段1）"
```

---

## Task 2：`transcribe_batch` + `transcribe_with_vad` 委托（TDD）

**Files:**
- Modify: `crates/asr/src/pipeline.rs`（加 `transcribe_batch` + `transcribe_segments` + 测试）
- Modify: `crates/asr/src/engine.rs:150-243`（`transcribe_with_vad` 改委托）

- [ ] **Step 1: 在 `pipeline.rs` 末尾加测试模块（先写失败测试）**

在 `pipeline.rs` 末尾追加：

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::Result;

    struct FakeEngine {
        text: String,
        skip: bool,
    }
    impl OfflineAsrEngine for FakeEngine {
        fn transcribe(&self, _samples: &[f32], _language: &str) -> Result<String> {
            Ok(self.text.clone())
        }
        fn skip_corrector(&self) -> bool {
            self.skip
        }
    }

    fn cfg(simplify: bool, correct: bool) -> PipelineConfig {
        PipelineConfig {
            language: "zh".into(),
            correct,
            simplify,
            ngram: false,
        }
    }

    #[test]
    fn batch_simplify_on_converts_traditional() {
        let eng = FakeEngine { text: "語言識別".into(), skip: false };
        let out = transcribe_batch(&eng, &[], &cfg(true, false)).unwrap();
        assert_eq!(out, "语言识别");
    }

    #[test]
    fn batch_simplify_off_keeps_traditional() {
        let eng = FakeEngine { text: "语言".into(), skip: false };
        let out = transcribe_batch(&eng, &[], &cfg(false, false)).unwrap();
        assert_eq!(out, "語言");
    }

    #[test]
    fn batch_ngram_flag_does_not_panic() {
        let eng = FakeEngine { text: "你好".into(), skip: false };
        let mut c = cfg(true, false);
        c.ngram = true;
        let out = transcribe_batch(&eng, &[], &c).unwrap();
        assert_eq!(out, "你好");
    }

    #[test]
    fn batch_short_audio_calls_engine_directly() {
        // ≤480k samples 走直连，不经 VAD（FakeEngine 不依赖真实模型即可验证路径）
        let eng = FakeEngine { text: "短音频".into(), skip: false };
        let samples = vec![0.0f32; 1000];
        let out = transcribe_batch(&eng, &samples, &cfg(true, false)).unwrap();
        assert_eq!(out, "短音频");
    }
}
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test -p octopus-asr pipeline::tests 2>&1 | head -30`
Expected: 编译失败 `cannot find function transcribe_batch`（尚未实现）。

- [ ] **Step 3: 在 `pipeline.rs` 实现 `transcribe_batch` + `transcribe_segments`**

在 `impl PipelineConfig { ... }` 块之后、`#[cfg(test)]` 之前插入（`use anyhow::Result;` 加到文件顶部 `use` 区）：

```rust
use anyhow::Result;

/// 批处理转写：VAD 分段 → 逐段 `engine.transcribe` → 连接 → 纠错 → 简繁归一化。
///
/// 收编自原 `engine::transcribe_with_vad`；纠错/简繁改由 `cfg` 控制（不读全局 config），
/// 使 cli/server 能以明确参数复用同一编排。短音频（≤480k samples = 30s）跳过 VAD 直连引擎。
/// ngram 解码尚未实现（`cfg.ngram=true` 时仅 warn，不改变行为）——预留接入点。
pub fn transcribe_batch(
    engine: &dyn OfflineAsrEngine,
    samples: &[f32],
    cfg: &PipelineConfig,
) -> Result<String> {
    if cfg.ngram {
        log::warn!("ngram 解码尚未实现，忽略 cfg.ngram 开关");
    }

    let raw_text = transcribe_segments(engine, samples, &cfg.language)?;

    let is_english = cfg.language.eq_ignore_ascii_case("en");
    let text = if cfg.correct && !engine.skip_corrector() && !is_english {
        crate::corrector::get_corrector().correct(&raw_text)
    } else {
        raw_text
    };

    Ok(if cfg.simplify {
        crate::hans::to_simplified(&text)
    } else {
        crate::hans::to_traditional(&text)
    })
}

/// VAD 分段转写：短音频直连；长音频用 Silero VAD 切片后逐段转写，并按 CJK/非 CJK 规则连接。
/// VAD 不可用时降级为整段转写。搬自原 `engine::transcribe_with_vad` 的分段主体（逻辑不变）。
fn transcribe_segments(
    engine: &dyn OfflineAsrEngine,
    samples: &[f32],
    language: &str,
) -> Result<String> {
    if samples.len() <= 480_000 {
        return engine.transcribe(samples, language);
    }

    let vad_path = match crate::config::find_silero_vad() {
        Ok(p) => Some(p),
        Err(e) => {
            eprintln!(
                "Warning: Silero VAD not found, falling back to full audio transcription: {}", e);
            None
        }
    };
    let vad = vad_path.and_then(|p| match crate::vad::SileroVad::new(&p) {
        Ok(v) => Some(v),
        Err(e) => {
            eprintln!(
                "Warning: Failed to initialize Silero VAD, falling back to full audio transcription: {}", e);
            None
        }
    });

    if let Some(mut v) = vad {
        let total_secs = samples.len() as f64 / 16000.0;
        eprintln!("[ASR] Long audio detected ({:.2}s). Segmenting audio using VAD...", total_secs);
        let segments = crate::audio::segment_audio_vad(samples, &mut v, 480, 0.4, 500, 25000);
        eprintln!("[ASR] Audio segmented into {} speech chunks.", segments.len());

        let mut final_text = String::new();
        for (idx, seg) in segments.iter().enumerate() {
            if !seg.is_empty() {
                let seg_secs = seg.len() as f64 / 16000.0;
                eprintln!(
                    "[ASR] Transcribing segment {}/{} ({:.2}s)...", idx + 1, segments.len(), seg_secs);
                let text = engine.transcribe(seg, language)?;
                let text_cleaned = text.replace("<|nospeech|>", "");
                let text_trimmed = text_cleaned.trim();
                if !text_trimmed.is_empty() {
                    if !final_text.is_empty() {
                        let last_char = final_text.chars().last();
                        let next_char = text_trimmed.chars().next();
                        let needs_space = match (last_char, next_char) {
                            (Some(lc), Some(nc)) => {
                                let is_cjk = |c: char| {
                                    let u = c as u32;
                                    (0x4E00..=0x9FFF).contains(&u) // CJK Unified Ideographs
                                        || (0x3040..=0x309F).contains(&u) // Hiragana
                                        || (0x30A0..=0x30FF).contains(&u) // Katakana
                                        || (0xAC00..=0xD7AF).contains(&u)  // Hangul
                                };
                                !is_cjk(lc) || !is_cjk(nc)
                            }
                            _ => true,
                        };
                        if needs_space {
                            final_text.push(' ');
                        }
                    }
                    final_text.push_str(text_trimmed);
                }
            }
        }
        Ok(final_text)
    } else {
        engine.transcribe(samples, language)
    }
}
```

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test -p octopus-asr pipeline::tests`
Expected: 4 passed。

- [ ] **Step 5: `transcribe_with_vad` 改委托**

在 `crates/asr/src/engine.rs` 把第 150-243 行的整个 `transcribe_with_vad` 函数体替换为：

```rust
/// 保留入口（desktop 经 `AsrEngineManager::transcribe` 使用）：从全局 app_config 构造 cfg
/// 后委托 `pipeline::transcribe_batch`。行为与重构前完全一致（asr_correct / output_simplified
/// 仍来自 app_config）。
pub fn transcribe_with_vad(
    engine: &dyn OfflineAsrEngine,
    samples: &[f32],
    language: &str,
) -> Result<String> {
    let cfg = crate::pipeline::PipelineConfig::from_app_config(language);
    crate::pipeline::transcribe_batch(engine, samples, &cfg)
}
```

> 说明：原函数体（VAD 分段 + 纠错 + hans）已整体搬到 `pipeline::transcribe_batch` / `transcribe_segments`，此处只保留向后兼容的薄包装。`OfflineAsrEngine` trait、`AsrEngineManager`（除 Task 3 加的方法外）不动。

- [ ] **Step 6: 编译验证 asr**

Run: `cargo check -p octopus-asr`
Expected: 通过（`AsrEngineManager::transcribe` 在 engine.rs:143 调 `transcribe_with_vad`，委托后行为不变）。

- [ ] **Step 7: Commit**

```bash
git add crates/asr/src/pipeline.rs crates/asr/src/engine.rs
git commit -m "feat(asr): transcribe_batch 收编 transcribe_with_vad，纠错/简繁参数化"
```

---

## Task 3：`AsrEngineManager::transcribe_batch` 方法

**Files:**
- Modify: `crates/asr/src/engine.rs`（`impl AsrEngineManager` 块内，`transcribe` 方法后追加）

- [ ] **Step 1: 加 `transcribe_batch` 方法**

在 `crates/asr/src/engine.rs` 的 `impl AsrEngineManager { ... }` 块内，紧接现有 `pub fn transcribe(&self, samples: &[f32], language: &str) -> Result<String>`（第 137-147 行）之后追加：

```rust
    /// 批处理转写（pipeline 入口）：用 active engine + cfg 调 `pipeline::transcribe_batch`。
    /// 供 cli/server 等多端复用，取代端侧各自读全局 config 的旧路径。
    pub fn transcribe_batch(
        &self,
        samples: &[f32],
        cfg: &crate::pipeline::PipelineConfig,
    ) -> Result<String> {
        let engine = {
            let active = self.active_engine.read().unwrap();
            active.clone()
        };
        let eng = engine
            .ok_or_else(|| anyhow::anyhow!("No active ASR engine loaded in AsrEngineManager"))?;
        crate::pipeline::transcribe_batch(eng.as_ref(), samples, cfg)
    }
```

- [ ] **Step 2: 编译验证**

Run: `cargo check -p octopus-asr`
Expected: 通过。

- [ ] **Step 3: Commit**

```bash
git add crates/asr/src/engine.rs
git commit -m "feat(asr): AsrEngineManager::transcribe_batch 多端入口"
```

---

## Task 4：cli 批处理 pipeline + `do_transcribe` 改造

**Files:**
- Create: `crates/cli/src/pipeline.rs`
- Modify: `crates/cli/src/main.rs`（顶部 `mod pipeline;` + 替换 `do_transcribe` 函数体）

- [ ] **Step 1: 创建 `crates/cli/src/pipeline.rs`**

```rust
//! CLI 批处理转写 pipeline：`switch_model` → `AsrEngineManager::transcribe_batch`。
//!
//! 取代旧 `do_transcribe`（直接调各引擎裸 `transcribe` 自由函数、无 VAD/纠错/简繁），
//! 让 cli 与 desktop 共用 `asr::pipeline::transcribe_batch` 的完整编排
//! （VAD 分段 + 纠错 + 简繁归一化）。cfg 从全局 app_config 构造，与 desktop 行为一致。

use anyhow::Result;
use octopus_asr::engine::AsrEngineManager;
use octopus_asr::pipeline::PipelineConfig;

/// 批处理转写：加载引擎 → transcribe_batch（VAD + 纠错 + 简繁）。
///
/// `model` 为 DB models 表的 model_name（支持 `provider:category:model` spec）。
/// 云端引擎（火山/腾讯/百度/阿里）会在 `switch_model` 阶段 bail（仅支持流式）——
/// 与旧 `do_transcribe` 的行为一致。
pub fn run(model: &str, language: &str, samples: &[f32]) -> Result<String> {
    let mgr = AsrEngineManager::new();
    mgr.switch_model(model)?;
    let cfg = PipelineConfig::from_app_config(language);
    mgr.transcribe_batch(samples, &cfg)
}
```

- [ ] **Step 2: `main.rs` 注册模块**

在 `crates/cli/src/main.rs` 顶部 `use` 区之后（第 3 行 `use cpal::...` 之后）加：

```rust
mod pipeline;
```

- [ ] **Step 3: 替换 `do_transcribe` 函数体**

把 `crates/cli/src/main.rs` 第 478-516 行的整个 `fn do_transcribe(model: &str, language: &str, samples: &[f32]) -> Result<String> { ... }` 替换为：

```rust
/// 批处理转写入口（transcribe / transcribe-url 共用）：委托 `pipeline::run`，
/// 走 AsrEngineManager + transcribe_batch（VAD 分段 + 纠错 + 简繁归一化）。
fn do_transcribe(model: &str, language: &str, samples: &[f32]) -> Result<String> {
    pipeline::run(model, language, samples)
}
```

> 说明：原 `do_transcribe` 按 `EngineCategory` 分发到各引擎裸 `transcribe` 自由函数（whisper::transcribe 等）的逻辑整体删除——引擎实例化 + category 路由由 `AsrEngineManager::switch_model` 接管，编排由 `pipeline::transcribe_batch` 接管。调用点 `transcribe_file`（main.rs:186）与 `transcribe_url`（main.rs:365）签名不变，自动受益。

- [ ] **Step 4: 编译验证 cli**

Run: `cargo check -p octopus-cli`
Expected: 通过。

> 注意：若 `octopus-asr` 未在 cli 的 `Cargo.toml` 依赖中，需确认已存在（cli 已大量用 `octopus_asr::*`，应已声明）。

- [ ] **Step 5: Commit**

```bash
git add crates/cli/src/pipeline.rs crates/cli/src/main.rs
git commit -m "feat(cli): do_transcribe 走 pipeline::run（补齐 VAD/纠错/简繁链）"
```

---

## Task 5：workspace 验证 + 文档同步

**Files:**
- Modify: `docs/architecture.md`

- [ ] **Step 1: workspace 全量 check**

Run: `cargo check --workspace --all-targets`
Expected: 通过，零 error。

- [ ] **Step 2: asr pipeline 单测**

Run: `cargo test -p octopus-asr pipeline`
Expected: 4 passed。

- [ ] **Step 3: workspace clippy（零新 warning）**

Run: `cargo clippy --workspace --all-targets 2>&1 | grep -E "warning|error" | head`
Expected: 无新增 warning（`PipelineConfig` 字段均被读：`language`/`correct`/`simplify` 在 transcribe_batch 用，`ngram` 在 warn 分支用，无 dead_code）。

- [ ] **Step 4: 更新 `docs/architecture.md`**

在 asr 模块说明里（models 表描述附近）加 `asr::pipeline` 模块条目：

```markdown
- `asr::pipeline`（新）：批处理 pipeline 编排。`PipelineConfig`（language/correct/simplify/ngram）
  + `transcribe_batch`（VAD 分段 → 逐段转写 → 纠错 → 简繁归一化，收编自 `transcribe_with_vad`，
  纠错/简繁参数化）。`transcribe_with_vad` 退化为从 app_config 构造 cfg 的薄包装（desktop 向后兼容）。
  cli 经 `AsrEngineManager::transcribe_batch` 复用。流式 helper / StreamingRunner 见后续阶段。
```

- [ ] **Step 5: 标注 spec 阶段1 完成**

在 `docs/superpowers/specs/2026-06-23-asr-pipeline-design.md` 顶部「> 2026-06-23 初版」行下加：

```markdown
> **阶段1 已实施（2026-06-23）**：`asr::pipeline`（PipelineConfig + transcribe_batch）、
> `transcribe_with_vad` 委托、cli 走新 pipeline。流式 trait / StreamingRunner / desktop / server 留阶段2/3。
```

- [ ] **Step 6: Commit**

```bash
git add docs/architecture.md docs/superpowers/specs/2026-06-23-asr-pipeline-design.md
git commit -m "docs: 同步 ASR pipeline 阶段1（architecture + spec 状态）"
```

---

## Self-Review

**Spec coverage（阶段1 范围内）**：
- §3.3 `transcribe_batch` helper → Task 2 ✓
- §3.5 `PipelineConfig`（阶段1 精简：language/correct/simplify/ngram）→ Task 1 ✓
- §3.7 ngram 预留位（warn 忽略）→ Task 2 Step 3 ✓
- §9 迁移映射「transcribe_with_vad → transcribe_batch」「cli 走新 pipeline」→ Task 2/4 ✓

**阶段外（后续 plan，本计划显式不做）**：§3.2 trait（StreamingEngine/AudioSource/TranscriptEvent）、§3.3 StreamingRunner、§3.4 desktop StreamingPipeline + coordinator 拆分、§3.6 denoise 迁入 runner、server —— 均为阶段2/3，spec §10 已声明分阶段。

**Placeholder 扫描**：无 TBD/TODO；ngram 是明确的「未实现则 warn」行为（非占位）。

**Type 一致**：`PipelineConfig { language, correct, simplify, ngram }` 在 Task 1 定义、Task 2/3/4 引用一致；`transcribe_batch(engine: &dyn OfflineAsrEngine, samples: &[f32], cfg: &PipelineConfig) -> Result<String>` 在 Task 2 定义、Task 3（manager 转发）一致；`AsrEngineManager::transcribe_batch(&self, samples, cfg)` 在 Task 3 定义、Task 4（cli `pipeline::run` 调用）一致。

**风险**：
- cli 改用 `AsrEngineManager::switch_model` 后，引擎实例化走 DB 配置（原 `do_transcribe` 走 `resolve_engine_category` + 自由函数）。两者都基于 DB models 表，但路径不同——Task 4 后需手动验证 `octopus-cli transcribe <wav> --model <name>` 输出正常（GUI/网络集成无自动化测试，靠手动）。
- `transcribe_with_vad` 委托后 desktop 行为应零变化，但依赖既有 desktop 测试 + 手动 GUI 验证确认。
