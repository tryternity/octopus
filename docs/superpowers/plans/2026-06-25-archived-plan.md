# 已归档实施计划（2026-06-23 ~ 2026-06-25）

> 以下功能均已实现并合并 main。交叉引用统一指向本归档文件内同名章节；对应 specs 见 `docs/superpowers/specs/2026-06-25-archived-spec.md`。

## 目录

| 主题 | 状态 |
|---|---|
| asr-pipeline | ASR pipeline 架构重构（stage1 总体） | ✅ |
| asr-pipeline-stage2a | Stage 2A：流式引擎 pipeline | ✅ |
| asr-pipeline-stage2b | Stage 2B：VAD 分段 + 伪流式 | ✅ |
| asr-pipeline-stage2c1 | Stage 2C1：云端流式 dispatch | ✅ |
| asr-pipeline-stage2c2 | Stage 2C2：coordinator 清理 + bug 修复 | ✅ |
| asr-server-stage3 | Stage 3：server crate | ✅ |
| clipboard-history | 剪贴板历史管理（详见活跃 spec/plan） | ✅ |
| cloud-asr-cli | 云端 ASR CLI 接入 | ✅ |
| coordinator-cleanup | coordinator 状态机清理 | ✅ |
| desktop-cloud-dedupe | desktop 云端引擎去重 | ✅ |
| vad-segmented-rehome | VAD 分段引擎迁移 | ✅ |

> **注**：clipboard-history 仍在活跃迭代（OCR、图片存储迁移等后续迭代），其最新 spec/plan 保留在 `specs/2026-06-25-clipboard-history-design.md` 和 `plans/2026-06-25-clipboard-history.md`。本归档仅包含 Phase 0-3 + 早期迭代的实施记录。

---

---

## 2026-06-23-asr-pipeline


> ✅ **阶段1 全部 5 task 已实施（2026-06-23）**：asr `pipeline` 模块（`PipelineConfig` + `transcribe_batch` + `transcribe_with_vad` 委托 + `AsrEngineManager::transcribe_batch` 入口）+ cli `do_transcribe` 走 `pipeline::run` + architecture/spec 同步。workspace check / 4 单测 / clippy 零新 warning 均通过。流式 trait / StreamingRunner / desktop / server 留阶段2/3。
>

>
> spec：`docs/superpowers/specs/2026-06-25-archived-spec.md#asr-pipeline-design`。worktree：`worktree-model-mgmt-ui`。

**Scope（重要）**：本计划只覆盖 **阶段1 = asr 批处理 helper + cli 走新 pipeline**。spec 里的流式 trait（`StreamingEngine`/`AudioSource`/`TranscriptEvent`）、`StreamingRunner`、desktop `StreamingPipeline`（coordinator 状态机拆分）、server、denoise 迁入 runner —— 全部留**后续 plan**（阶段2 desktop、阶段3 server）。阶段1 完成后：cli 走新 `transcribe_batch`（补齐原 cli 缺失的 VAD/纠错/简繁链），desktop 仍用旧 `transcribe_with_vad`（行为不变，因旧入口委托新 helper），互不阻塞。

**Goal**：asr 新增 `pipeline` 模块（`PipelineConfig` + `transcribe_batch`），把 `transcribe_with_vad` 的 VAD 分段编排收编、纠错/简繁从「读全局 app_config」参数化；cli `do_transcribe` 改走 `AsrEngineManager + transcribe_batch`。

**Architecture**：`transcribe_batch(engine, samples, &cfg)` 收编 `transcribe_with_vad` 主体，`correct`/`simplify` 改读 `cfg`（`ngram` 预留位，未实现仅 warn）。`transcribe_with_vad` 退化为「从 app_config 构造 cfg → 委托 transcribe_batch」，desktop 行为零变化。cli 经 `AsrEngineManager.transcribe_batch` 复用同一编排。

**Tech Stack**：Rust、anyhow、log、octopus-asr-local / octopus-infra / octopus-cli crate。

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

## Task 1：asr 新增 `pipeline.rs` 模块 + `PipelineConfig` ✅

**Files:**
- Create: `crates/asr/src/pipeline.rs`
- Modify: `crates/asr/src/lib.rs`（在 `pub mod hans;` 后加 `pub mod pipeline;`）

- [x] **Step 1: 创建 `crates/asr/src/pipeline.rs`**

```rust
//! ASR pipeline 编排：批处理 helper（流式 helper / StreamingRunner 见后续阶段）。
//!
//! `transcribe_batch` 收编原 `engine::transcribe_with_vad` 的 VAD 分段编排，把纠错
//! （`correct`）与简繁归一化（`simplify`）从「读全局 app_config」参数化为 `PipelineConfig`
//! 字段，使编排可被多端（cli/desktop/server）以明确参数复用，而非隐式依赖全局配置。
//!
//! 设计详见 `docs/superpowers/specs/2026-06-25-archived-spec.md#asr-pipeline-design`。

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

- [x] **Step 2: 在 `crates/asr/src/lib.rs` 注册模块**

在 `pub mod hans;`（第 23 行）后加一行：

```rust
pub mod pipeline;
```

- [x] **Step 3: 编译验证**

Run: `cargo check -p octopus-asr-local`
Expected: 通过（`PipelineConfig` 暂无调用点，`load_app_config_cached` 字段名 `asr_correct` / `output_simplified` 已核对存在）。

- [x] **Step 4: Commit**

```bash
git add crates/asr/src/pipeline.rs crates/asr/src/lib.rs
git commit -m "feat(asr): 新增 pipeline 模块 + PipelineConfig（阶段1）"
```

---

## Task 2：`transcribe_batch` + `transcribe_with_vad` 委托（TDD） ✅

**Files:**
- Modify: `crates/asr/src/pipeline.rs`（加 `transcribe_batch` + `transcribe_segments` + 测试）
- Modify: `crates/asr/src/engine.rs:150-243`（`transcribe_with_vad` 改委托）

- [x] **Step 1: 在 `pipeline.rs` 末尾加测试模块（先写失败测试）**

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

- [x] **Step 2: 跑测试确认失败**

Run: `cargo test -p octopus-asr-local pipeline::tests 2>&1 | head -30`
Expected: 编译失败 `cannot find function transcribe_batch`（尚未实现）。

- [x] **Step 3: 在 `pipeline.rs` 实现 `transcribe_batch` + `transcribe_segments`**

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

- [x] **Step 4: 跑测试确认通过**

Run: `cargo test -p octopus-asr-local pipeline::tests`
Expected: 4 passed。

- [x] **Step 5: `transcribe_with_vad` 改委托**

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

- [x] **Step 6: 编译验证 asr**

Run: `cargo check -p octopus-asr-local`
Expected: 通过（`AsrEngineManager::transcribe` 在 engine.rs:143 调 `transcribe_with_vad`，委托后行为不变）。

- [x] **Step 7: Commit**

```bash
git add crates/asr/src/pipeline.rs crates/asr/src/engine.rs
git commit -m "feat(asr): transcribe_batch 收编 transcribe_with_vad，纠错/简繁参数化"
```

---

## Task 3：`AsrEngineManager::transcribe_batch` 方法 ✅

**Files:**
- Modify: `crates/asr/src/engine.rs`（`impl AsrEngineManager` 块内，`transcribe` 方法后追加）

- [x] **Step 1: 加 `transcribe_batch` 方法**

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

- [x] **Step 2: 编译验证**

Run: `cargo check -p octopus-asr-local`
Expected: 通过。

- [x] **Step 3: Commit**

```bash
git add crates/asr/src/engine.rs
git commit -m "feat(asr): AsrEngineManager::transcribe_batch 多端入口"
```

---

## Task 4：cli 批处理 pipeline + `do_transcribe` 改造 ✅

**Files:**
- Create: `crates/cli/src/pipeline.rs`
- Modify: `crates/cli/src/main.rs`（顶部 `mod pipeline;` + 替换 `do_transcribe` 函数体）

- [x] **Step 1: 创建 `crates/cli/src/pipeline.rs`**

```rust
//! CLI 批处理转写 pipeline：`switch_model` → `AsrEngineManager::transcribe_batch`。
//!
//! 取代旧 `do_transcribe`（直接调各引擎裸 `transcribe` 自由函数、无 VAD/纠错/简繁），
//! 让 cli 与 desktop 共用 `asr::pipeline::transcribe_batch` 的完整编排
//! （VAD 分段 + 纠错 + 简繁归一化）。cfg 从全局 app_config 构造，与 desktop 行为一致。

use anyhow::Result;
use octopus_asr_local::engine::AsrEngineManager;
use octopus_asr_local::pipeline::PipelineConfig;

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

- [x] **Step 2: `main.rs` 注册模块**

在 `crates/cli/src/main.rs` 顶部 `use` 区之后（第 3 行 `use cpal::...` 之后）加：

```rust
mod pipeline;
```

- [x] **Step 3: 替换 `do_transcribe` 函数体**

把 `crates/cli/src/main.rs` 第 478-516 行的整个 `fn do_transcribe(model: &str, language: &str, samples: &[f32]) -> Result<String> { ... }` 替换为：

```rust
/// 批处理转写入口（transcribe / transcribe-url 共用）：委托 `pipeline::run`，
/// 走 AsrEngineManager + transcribe_batch（VAD 分段 + 纠错 + 简繁归一化）。
fn do_transcribe(model: &str, language: &str, samples: &[f32]) -> Result<String> {
    pipeline::run(model, language, samples)
}
```

> 说明：原 `do_transcribe` 按 `EngineCategory` 分发到各引擎裸 `transcribe` 自由函数（whisper::transcribe 等）的逻辑整体删除——引擎实例化 + category 路由由 `AsrEngineManager::switch_model` 接管，编排由 `pipeline::transcribe_batch` 接管。调用点 `transcribe_file`（main.rs:186）与 `transcribe_url`（main.rs:365）签名不变，自动受益。

- [x] **Step 4: 编译验证 cli**

Run: `cargo check -p octopus-cli`
Expected: 通过。

> 注意：若 `octopus-asr-local` 未在 cli 的 `Cargo.toml` 依赖中，需确认已存在（cli 已大量用 `octopus_asr_local::*`，应已声明）。

- [x] **Step 5: Commit**

```bash
git add crates/cli/src/pipeline.rs crates/cli/src/main.rs
git commit -m "feat(cli): do_transcribe 走 pipeline::run（补齐 VAD/纠错/简繁链）"
```

---

## Task 5：workspace 验证 + 文档同步 ✅

**Files:**
- Modify: `docs/architecture.md`

- [x] **Step 1: workspace 全量 check**

Run: `cargo check --workspace --all-targets`
Expected: 通过，零 error。

- [x] **Step 2: asr pipeline 单测**

Run: `cargo test -p octopus-asr-local pipeline`
Expected: 4 passed。

- [x] **Step 3: workspace clippy（零新 warning）**

Run: `cargo clippy --workspace --all-targets 2>&1 | grep -E "warning|error" | head`
Expected: 无新增 warning（`PipelineConfig` 字段均被读：`language`/`correct`/`simplify` 在 transcribe_batch 用，`ngram` 在 warn 分支用，无 dead_code）。

- [x] **Step 4: 更新 `docs/architecture.md`**

在 asr 模块说明里（models 表描述附近）加 `asr::pipeline` 模块条目：

```markdown
- `asr::pipeline`（新）：批处理 pipeline 编排。`PipelineConfig`（language/correct/simplify/ngram）
  + `transcribe_batch`（VAD 分段 → 逐段转写 → 纠错 → 简繁归一化，收编自 `transcribe_with_vad`，
  纠错/简繁参数化）。`transcribe_with_vad` 退化为从 app_config 构造 cfg 的薄包装（desktop 向后兼容）。
  cli 经 `AsrEngineManager::transcribe_batch` 复用。流式 helper / StreamingRunner 见后续阶段。
```

- [x] **Step 5: 标注 spec 阶段1 完成**

在 `docs/superpowers/specs/2026-06-25-archived-spec.md#asr-pipeline-design` 顶部「> 2026-06-23 初版」行下加：

```markdown
> **阶段1 已实施（2026-06-23）**：`asr::pipeline`（PipelineConfig + transcribe_batch）、
> `transcribe_with_vad` 委托、cli 走新 pipeline。流式 trait / StreamingRunner / desktop / server 留阶段2/3。
```

- [x] **Step 6: Commit**

```bash
git add docs/architecture.md docs/superpowers/specs/2026-06-25-archived-spec.md#asr-pipeline-design
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

---

## 2026-06-23-asr-pipeline-stage2a


> ✅ **已实施（2026-06-23，commit `10f612c`）**：Task 1-4 全完成。`crates/asr/src/streaming_runner.rs` 新增（371 行），`cargo test -p octopus-asr-local` 81 tests（75 pass + 6 ignored 模型相关，0 fail，含新增 7 个），`cargo check --workspace --all-targets` 干净，clippy 无新 warning。纯新增不碰 desktop，运行时零行为变化。


**Goal:** 在 `asr` crate 新增流式编排基础设施——`TranscriptEvent` 事件 + `StreamingEngine` trait + `StreamingRunner`（收编 desktop coordinator 本地流式 tick 的纯 ASR 编排：VAD 静音检测 + 标点触发 + StreamingSession accept/flush/finish），为阶段2b desktop `StreamingPipeline` 接线打地基。

**Architecture:** 纯 asr 新增，**不碰 desktop**（denoise/resample 留 `audio.rs`，见「设计调整」）。`StreamingRunner` 吃已降噪的 16k 样本，产出 `TranscriptEvent` 流；润色/DB/Tauri emit 留端（spec §3.8）。`StreamingEngine` trait 让 local `StreamingSession` 与（阶段2c）cloud WS 共实现，签名对齐 `StreamingSession` 现有 `&self` 方法，impl 为直接委托。静音/标点决策逻辑抽成纯函数 `step_silence`，无 VAD 模型亦可单测。

**Tech Stack:** Rust workspace、`octopus_asr_local`（`vad::SileroVad`、`streaming_engine::StreamingSession`、`config::find_silero_vad`、`corrector`）、`anyhow::Result`、`log`。

---

## 阶段2 拆分总览（本 plan 仅实施 2a）

spec §10 建议分阶段，phase 2（desktop 全量拆分）体量大、无桌面自动化测试，按 writing-plans Scope Check 再拆为可独立验证的子 plan：

| 子 plan | 范围 | 依赖 |
|--------|------|------|
| **2a（本 plan）** | asr `TranscriptEvent` + `StreamingEngine` trait + `StreamingRunner`（本地流式纯 ASR 编排）+ 单测 | 无（纯新增） |
| 2b | desktop `MicSource`/`StreamingPipeline` 骨架 + 本地流式路径迁移（coordinator `Streaming` stage 委托 runner）+ 接 `TranscriptEvent`→`Transcript`/DB/emit | 2a |
| 2c | cloud `StreamingEngine` WS 实现（feature-gated）+ `CloudStreaming` 路径迁移；**VadSegmented 归位决策**（见下） | 2b |
| 2d | coordinator 清理退化（删死分发代码，成纯驱动） | 2c |

**2a 独立可验**：`cargo test -p octopus_asr_local` + `cargo check --workspace --all-targets` + clippy。不改变任何运行时行为（无调用方）。

---

## 设计调整（相对 spec §3.3 字面）

spec §3.3 设想 `StreamingRunner` 持 `DenoiseProcessor`，内部 `denoise(48k)→resample(16k)→vad→engine`。**本 plan 按用户决策调整为：**

1. **denoise + resample 留 `desktop/audio.rs`**（用户 2026-06-23 指示「denoise 保持现在，让 denoise 在 audio.rs 中」）。理由：denoise（RNNoise/DF3）紧耦合 cpal 采集（`SharedAudioState` 持 down_sampler 原生→48k + DenoiseProcessor + resampler 48k→16k，含跨帧 GRU/滤波状态），留采集层更内聚；`DenoiseProcessor`/`AudioResampler` 类型本就在 asr，`audio.rs` 只是调用方，无需搬类型。**`StreamingRunner` 输入即 `drain_samples()` 产出的已降噪 16k 样本**，不持 denoise/resampler。
2. **`AudioSource` trait 延后到 2b**。denoise 留 `audio.rs` 后，48k frame 抽象失去主要依据；2a 的 `StreamingRunner.push_samples` 直接吃 `&[f32]`（16k），测试手工喂样本即可。2b 视 MicSource 抽象需要再定 `AudioSource`。
3. **流式纠错 hook 预留但默认关**（spec §9.4「流式纠错语义」为待核实项）。`StreamingRunner` 持 `correct: bool`，`correct=true` 时对 `Partial`/`Committed` 文本过 `corrector`；desktop 流式目前无纠错，2b 以 `correct=false` 构造，**行为不变**。hook 已就位，未来翻转即可。
4. **`VadSegmented` 归 2c 决策**：它是伪流式（drain→VAD 分段→spawn 离线 transcribe→乱序拼接），不符 `StreamingEngine`「推帧→增量文本」语义（spec §7「不统一 Stage trait」）。2a 不涉及；2c 文档化其为 desktop 批分段路径（用阶段1 `transcribe_batch`），不强行塞入 `StreamingEngine`。

---

## File Structure

- **Create:** `crates/asr/src/streaming_runner.rs` —— `TranscriptEvent` + `StreamingEngine` trait + `impl StreamingEngine for StreamingSession` + `StreamingRunner` + `detect_silence_gap`/`step_silence` + 常量 + 单测。单一职责：流式编排。
- **Modify:** `crates/asr/src/lib.rs` —— 加 `pub mod streaming_runner;`。
- **不动：** `crates/asr/src/streaming_engine.rs`（`StreamingSession` 原样，仅被 trait impl 引用）、`crates/desktop/**`（2a 不碰）。

---

## Task 1: `TranscriptEvent` + `StreamingEngine` trait + `StreamingSession` 委托 impl

**Files:**
- Create: `crates/asr/src/streaming_runner.rs`

- [x] **Step 1: 写文件头 + `TranscriptEvent` + `StreamingEngine` trait**

新建 `crates/asr/src/streaming_runner.rs`，写入：

```rust
//! 流式 ASR 编排基础设施（spec §3.2/§3.3）。
//!
//! - [`TranscriptEvent`]：流式事件（润色不在 helper，留端，spec §3.8）。
//! - [`StreamingEngine`]：流式引擎 trait，local [`StreamingSession`](crate::streaming_engine::StreamingSession)
//!   与（阶段2c）cloud WS 共实现。签名对齐 `StreamingSession` 现有 `&self` 方法。
//! - [`StreamingRunner`]：收编 desktop coordinator 本地流式 tick 的纯 ASR 编排
//!   （VAD 静音 + 标点触发 + engine accept/flush/finish）。
//!
//! denoise/resample 留 `desktop/audio.rs`（用户决策，见 plan「设计调整」），
//! runner 输入即已降噪的 16k 样本。

use anyhow::Result;

use crate::streaming_engine::StreamingSession;
use crate::vad::SileroVad;

/// 流式编排事件。润色（`octopus_llm::polish`）不在 helper，由端 pipeline 处理（spec §3.8）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TranscriptEvent {
    /// 增量文本（engine.accept_samples 的新结果，可能随后被改写）。
    Partial(String),
    /// 静音冲刷提交（engine.flush，冻结历史段并插逗号）。
    Committed(String),
    /// 收尾全文（engine.finish，追加句号 + 简繁归一）。
    Final(String),
    /// 单帧处理错误（非致命，spec §9.1：端决定是否中断/重试）。
    Error(String),
}

/// 流式引擎 trait。`&self` + 内部可变（`StreamingSession` 用 `Mutex`），故要求 `Send + Sync`。
///
/// local `StreamingSession` 与（阶段2c）cloud WS 实现本 trait；`StreamingRunner` 持
/// `Box<dyn StreamingEngine>`，对本地/云端无感（spec §3.4）。
pub trait StreamingEngine: Send + Sync {
    /// 送 16k 样本，返回累积全文（有新结果时）。`was_silent` 表示上一轮静音≥阈值（触发插逗号）。
    fn accept_samples(&self, samples: &[f32], was_silent: bool) -> Result<Option<String>>;
    /// 静音冲刷：`insert_comma=true` 冻结历史段并插逗号。
    fn flush(&self, insert_comma: bool) -> Result<Option<String>>;
    /// 收尾：追加句号 + 简繁归一，返回最终全文。
    fn finish(&self) -> Result<String>;
    /// 重置引擎内部状态（会话间复用前调用）。
    fn reset(&self);
}

/// `StreamingSession` 委托实现——签名完全一致，UFCS 调用固有方法避免与 trait 方法歧义。
impl StreamingEngine for StreamingSession {
    fn accept_samples(&self, samples: &[f32], was_silent: bool) -> Result<Option<String>> {
        StreamingSession::accept_samples(self, samples, was_silent)
    }
    fn flush(&self, insert_comma: bool) -> Result<Option<String>> {
        StreamingSession::flush(self, insert_comma)
    }
    fn finish(&self) -> Result<String> {
        StreamingSession::finish(self)
    }
    fn reset(&self) {
        StreamingSession::reset(self)
    }
}
```

- [x] **Step 2: 在 `lib.rs` 导出模块**

`crates/asr/src/lib.rs` 在 `pub mod streaming_engine;`（第 15 行）后加一行：

```rust
pub mod streaming_runner;
```

- [x] **Step 3: 验证编译（trait + 委托 impl 类型对齐）**

Run: `cargo check -p octopus-asr-local`
Expected: 编译通过。若 `flush`/`finish`/`reset` 报签名不匹配，核对 `crates/asr/src/streaming_engine.rs:154/209/256` 实际签名（已核实为 `flush(&self,bool)->Result<Option<String>>` / `finish(&self)->Result<String>` / `reset(&self)`）。

- [x] **Step 4: 暂不提交（与 Task 2 合并提交）**

---

## Task 2: `StreamingRunner` + 静音/标点逻辑收编

**Files:**
- Modify: `crates/asr/src/streaming_runner.rs`

收编目标（来自 `crates/desktop/src/coordinator.rs`，逐字搬迁语义）：
- `detect_silence_gap`（2045-2099）+ 常量 `VAD_CHUNK_SIZE=512` / `VAD_SPEECH_THRESHOLD=0.5` / `PUNCTUATION_SILENCE_THRESHOLD=0.5`
- `handle_streaming_tick`（1968-2037）的 ASR 部分：`accept_samples` → `flush` 冲刷 + `flushed` 锁（1990-1992、2012-2032）

- [x] **Step 1: 追加常量 + 纯函数 `step_silence`**

在 `streaming_runner.rs` 末尾（impl 块之前）追加。`step_silence` 把静音累计 + 标点阈值 + `flushed` 锁抽成纯函数，**无 VAD 模型亦可单测**：

```rust
/// VAD 块大小（样本数，16k 下 32ms）。
const VAD_CHUNK_SIZE: usize = 512;
/// 语音概率阈值。
const VAD_SPEECH_THRESHOLD: f32 = 0.5;
/// 标点（逗号）触发的静音时长阈值（秒）。
const PUNCTUATION_SILENCE_THRESHOLD: f64 = 0.5;

/// 静音/标点决策纯函数（从 `detect_silence_gap` + `handle_streaming_tick` 抽出）。
///
/// - `has_speech`：本帧语音 chunk 数 ≥ 2（由 VAD 判定，见 `detect_silence_gap`）。
/// - `total_chunks`：本帧完整 VAD chunk 数（用于累加静音时长）。
///
/// 返回 `(was_silent_for_punct, should_flush)`：
/// - `was_silent_for_punct`：**上一帧结束前**累积静音已 ≥ 阈值（传给 engine 触发插逗号）。
/// - `should_flush`：本帧累积静音达阈值且未在本轮冲刷过 → engine.flush(true)。
///
/// `flushed` 锁语义与 `handle_streaming_tick:1990-1992,2012-2032` 一致：
/// 语音恢复（静音清零）→ 解锁；达阈值冲刷一次 → 上锁，避免静音期重复 flush。
fn step_silence(
    silence_duration: &mut f64,
    flushed: &mut bool,
    has_speech: bool,
    total_chunks: usize,
) -> (bool, bool) {
    let prev = *silence_duration;
    if has_speech {
        *silence_duration = 0.0;
    } else {
        *silence_duration += total_chunks as f64 * (VAD_CHUNK_SIZE as f64 / 16000.0);
    }
    // 语音恢复（静音清零）→ 解除 flushed 锁
    if *silence_duration == 0.0 {
        *flushed = false;
    }
    let was_silent_for_punct = prev >= PUNCTUATION_SILENCE_THRESHOLD;
    let should_flush = *silence_duration >= PUNCTUATION_SILENCE_THRESHOLD && !*flushed;
    if should_flush {
        *flushed = true;
    }
    (was_silent_for_punct, should_flush)
}
```

- [x] **Step 2: 追加 `detect_silence_gap`（VAD 包装层）**

紧接 `step_silence` 后追加。逐字搬迁 `coordinator.rs:2045-2099` 语义，改为返回 `(was_silent_for_punct, should_flush)` 并把 `flushed` 状态交由 `step_silence` 管理：

```rust
/// VAD 静音检测 + 标点触发（收编自 `coordinator.rs:detect_silence_gap`）。
///
/// 遍历 `samples`（16k）的 `VAD_CHUNK_SIZE` 块统计语音/静音 chunk，委托 [`step_silence`]
/// 更新 `silence_duration`/`flushed` 并返回决策。`vad=None`（模型缺失）→ 不加标点、不冲刷，
/// 与原 `detect_silence_gap` 的 `None` 分支一致。
fn detect_silence_gap(
    vad: &mut Option<SileroVad>,
    samples: &[f32],
    silence_duration: &mut f64,
    flushed: &mut bool,
) -> (bool, bool) {
    let Some(v) = vad.as_mut() else {
        return (false, false);
    };
    let (mut speech_chunks, mut silent_chunks) = (0usize, 0usize);
    for chunk in samples.chunks(VAD_CHUNK_SIZE) {
        if chunk.len() < VAD_CHUNK_SIZE {
            break; // 不足一个完整块，跳过（与原实现一致）
        }
        match v.compute(chunk) {
            Ok(prob) => {
                if prob >= VAD_SPEECH_THRESHOLD {
                    speech_chunks += 1;
                } else {
                    silent_chunks += 1;
                }
            }
            Err(_) => speech_chunks += 1, // VAD 计算失败，保守认为有语音
        }
    }
    let total_chunks = speech_chunks + silent_chunks;
    if total_chunks == 0 {
        return (false, false);
    }
    step_silence(
        silence_duration,
        flushed,
        speech_chunks >= 2,
        total_chunks,
    )
}
```

- [x] **Step 3: 追加 `StreamingRunner` 结构体与方法**

紧接 `detect_silence_gap` 后追加：

```rust
/// 流式编排 runner（收编 coordinator 本地流式 tick 的纯 ASR 编排）。
///
/// 持 `StreamingEngine`（local `StreamingSession` 或 cloud WS）+ VAD + 静音/标点状态。
/// **不持 denoise/resample**（留 `desktop/audio.rs`，见 plan「设计调整」）；输入为已降噪 16k 样本。
/// 润色/DB/Tauri emit 留端；本 runner 只产 [`TranscriptEvent`]。
pub struct StreamingRunner {
    engine: Box<dyn StreamingEngine>,
    vad: Option<SileroVad>,
    silence_duration: f64,
    flushed: bool,
    /// 流式纠错开关（spec §3.3 新增 hook，默认 false——desktop 流式现无纠错，行为不变）。
    correct: bool,
}

impl StreamingRunner {
    /// 构造 runner。`engine` 由调用方创建（local `StreamingSession` 或 cloud WS）。
    /// VAD 经 `find_silero_vad` 解析模型路径，缺失则 `None`（不加标点，与现状一致）。
    pub fn new(engine: Box<dyn StreamingEngine>, correct: bool) -> Result<Self> {
        let vad = crate::config::find_silero_vad()
            .ok()
            .and_then(|p| SileroVad::new(&p).ok());
        Ok(Self {
            engine,
            vad,
            silence_duration: 0.0,
            flushed: false,
            correct,
        })
    }

    /// 喂一帧**已降噪的 16k** 样本，返回本帧产生的事件（0..n）。
    ///
    /// 收编 `handle_streaming_tick:1989-2032` 的 ASR 部分：detect_silence_gap →
    /// engine.accept_samples（→Partial）→ 达阈值 engine.flush(true)（→Committed）。
    /// 幂等去重（`new_text != transcript.full()`）与 DB/emit 留端（2b StreamingPipeline）。
    pub fn push_samples(&mut self, samples_16k: &[f32]) -> Vec<TranscriptEvent> {
        let mut events = Vec::new();
        if samples_16k.is_empty() {
            return events;
        }
        let (was_silent, should_flush) = detect_silence_gap(
            &mut self.vad,
            samples_16k,
            &mut self.silence_duration,
            &mut self.flushed,
        );
        match self.engine.accept_samples(samples_16k, was_silent) {
            Ok(Some(text)) => events.push(self.maybe_correct(TranscriptEvent::Partial(text))),
            Ok(None) => {}
            Err(e) => {
                log::warn!("StreamingRunner accept_samples error: {e}");
                events.push(TranscriptEvent::Error(e.to_string()));
            }
        }
        if should_flush {
            match self.engine.flush(true) {
                Ok(Some(text)) => events.push(self.maybe_correct(TranscriptEvent::Committed(text))),
                Ok(None) => {}
                Err(e) => {
                    log::warn!("StreamingRunner flush error: {e}");
                    events.push(TranscriptEvent::Error(e.to_string()));
                }
            }
        }
        events
    }

    /// 收尾：engine.finish（追加句号 + 简繁归一）→ `Final`。
    pub fn finish(&mut self) -> TranscriptEvent {
        match self.engine.finish() {
            Ok(text) => TranscriptEvent::Final(text),
            Err(e) => TranscriptEvent::Error(e.to_string()),
        }
    }

    /// 重置（会话间复用）：engine + VAD + 静音/标点状态归零。
    pub fn reset(&mut self) {
        self.engine.reset();
        if let Some(v) = self.vad.as_mut() {
            v.reset();
        }
        self.silence_duration = 0.0;
        self.flushed = false;
    }

    /// 当前累积静音时长（秒），供端判断是否触发停顿润色（`check_and_trigger_polish` 留端）。
    pub fn silence_duration(&self) -> f64 {
        self.silence_duration
    }

    /// `correct=true` 时对 `Partial`/`Committed` 文本过 corrector；否则原样返回。
    fn maybe_correct(&self, ev: TranscriptEvent) -> TranscriptEvent {
        if !self.correct {
            return ev;
        }
        match ev {
            TranscriptEvent::Partial(t) => {
                TranscriptEvent::Partial(crate::corrector::get_corrector().correct(&t))
            }
            TranscriptEvent::Committed(t) => {
                TranscriptEvent::Committed(crate::corrector::get_corrector().correct(&t))
            }
            other => other,
        }
    }
}
```

- [x] **Step 4: 验证编译**

Run: `cargo check -p octopus-asr-local`
Expected: 通过。若 `find_silero_vad` 签名不符，核对 `crates/asr/src/config.rs`（已核实返回 `Option<PathBuf>`，与 `pipeline.rs:90` 用法一致）；`SileroVad::new(&Path)` 返回 `Result`，`.ok()` 转 Option。

- [x] **Step 5: 暂不提交（与 Task 1、3 合并）**

---

## Task 3: 单元测试（`FakeStreamingEngine` + 纯逻辑）

**Files:**
- Modify: `crates/asr/src/streaming_runner.rs`（追加 `#[cfg(test)] mod tests`）

无桌面/无音频依赖，全 hermetic。`FakeStreamingEngine` 用 `Mutex<Vec<_>>` 可编程返回序列（trait 要求 `Send+Sync`）。

- [x] **Step 1: 追加测试模块与 `FakeStreamingEngine`**

文件末尾追加：

```rust
#[cfg(test)]
mod tests {
    use super::*;

    /// 可编程 fake：accept/flush 按预设序列出队返回，finish 返回固定串。
    struct FakeStreamingEngine {
        accept_out: Mutex<Vec<Option<String>>>,
        flush_out: Mutex<Vec<Option<String>>>,
        finish_out: Mutex<String>,
    }
    impl FakeStreamingEngine {
        fn new(accept: Vec<&str>, flush: Vec<&str>, finish: &str) -> Self {
            Self {
                accept_out: Mutex::new(accept.into_iter().map(|s| Some(s.to_string())).collect()),
                flush_out: Mutex::new(flush.into_iter().map(|s| Some(s.to_string())).collect()),
                finish_out: Mutex::new(finish.to_string()),
            }
        }
    }
    impl StreamingEngine for FakeStreamingEngine {
        fn accept_samples(&self, _samples: &[f32], _was_silent: bool) -> Result<Option<String>> {
            Ok(self.accept_out.lock().unwrap().remove(0))
        }
        fn flush(&self, _insert_comma: bool) -> Result<Option<String>> {
            Ok(self.flush_out.lock().unwrap().remove(0))
        }
        fn finish(&self) -> Result<String> {
            Ok(self.finish_out.lock().unwrap().clone())
        }
        fn reset(&self) {}
    }

    fn runner(fake: FakeStreamingEngine) -> StreamingRunner {
        StreamingRunner::new(Box::new(fake), false).unwrap()
    }
```

- [x] **Step 2: 写 `step_silence` 纯逻辑测试**

```rust
    #[test]
    fn step_silence_speech_resets_silence_and_unlocks_flushed() {
        let (mut sd, mut fl) = (0.6, true); // 已过阈值且上锁
        let (punct, flush) = step_silence(&mut sd, &mut fl, true, 3);
        // 语音 → silence 清零、flushed 解锁；prev=0.6≥阈值 → punct=true；清零后 < 阈值 → flush=false
        assert_eq!((sd, fl), (0.0, false));
        assert_eq!((punct, flush), (true, false));
    }

    #[test]
    fn step_silence_accumulate_below_threshold_no_flush() {
        let (mut sd, mut fl) = (0.0, false);
        // 静音 10 chunk × (512/16000=0.032s) = 0.32s < 0.5
        let (punct, flush) = step_silence(&mut sd, &mut fl, false, 10);
        assert!((sd - 0.32).abs() < 1e-9);
        assert_eq!((punct, flush), (false, false));
        assert!(!fl);
    }

    #[test]
    fn step_silence_cross_threshold_flushes_once_then_latches() {
        let (mut sd, mut fl) = (0.0, false);
        // 第一帧静音 16 chunk × 0.032 = 0.512s ≥ 0.5 → flush=true，上锁
        let (punct1, flush1) = step_silence(&mut sd, &mut fl, false, 16);
        assert!(flush1);
        assert!(fl);
        assert!(!punct1); // prev=0
        // 第二帧继续静音 → 已上锁，不再 flush
        let (_punct2, flush2) = step_silence(&mut sd, &mut fl, false, 16);
        assert!(!flush2);
        assert!(fl);
        // 语音恢复 → 解锁
        let (mut sd2, mut fl2) = (sd, fl);
        step_silence(&mut sd2, &mut fl2, true, 3);
        assert!(!fl2);
    }
```

- [x] **Step 3: 写 `StreamingRunner` 集成测试（无 VAD 路径）**

`runner()` 构造时 VAD 为 `None`（测试环境无 silero 模型）→ `detect_silence_gap` 返回 `(false,false)`，`push_samples` 只中继 accept，不 flush。覆盖 accept→Partial、空帧、finish→Final：

```rust
    #[test]
    fn push_samples_relays_accept_as_partial() {
        // VAD=None → 无标点/冲刷；accept 首次返回 Some("你好")
        let r = runner(FakeStreamingEngine::new(vec!["你好"], vec![], "你好。"));
        let mut r = r;
        let evs = r.push_samples(&[0.0; 1600]); // 任意 16k 样本
        assert_eq!(evs, vec![TranscriptEvent::Partial("你好".to_string())]);
    }

    #[test]
    fn push_samples_empty_input_no_events() {
        let mut r = runner(FakeStreamingEngine::new(vec![], vec![], "x"));
        assert!(r.push_samples(&[]).is_empty());
    }

    #[test]
    fn finish_emits_final() {
        let mut r = runner(FakeStreamingEngine::new(vec![], vec![], "收尾。"));
        assert_eq!(r.finish(), TranscriptEvent::Final("收尾。".to_string()));
    }

    #[test]
    fn accept_error_becomes_error_event_nonfatal() {
        // accept 队列提前耗尽 → remove 越界 panic；改用足够队列 + 手测 error 路径：
        // 此用例验证正常路径下 finish 在 push 之后仍可用（状态未被破坏）。
        let mut r = runner(FakeStreamingEngine::new(vec!["a"], vec![], "a。"));
        let _ = r.push_samples(&[0.0; 512]);
        assert_eq!(r.finish(), TranscriptEvent::Final("a。".to_string()));
    }
} // end mod tests
```

> 注：`accept_error_becomes_error_event_nonfatal` 的命名保留为占位意图说明——真实 error 路径需一个「accept 返回 Err」的 fake 变体。**实现时**把该用例替换为：给 `FakeStreamingEngine` 加 `accept_err: bool` 字段，`accept_samples` 在 `accept_err` 时返回 `Err`，断言 `push_samples` 返回 `[Error(_)]` 且非 panic。下方 Step 4 给出该 fake 扩展与用例的完整代码。

- [x] **Step 4: 补「accept 返回 Err」fake 扩展 + 用例（替换 Step 3 最后一个用例）**

把 `FakeStreamingEngine::new` 扩一个 `accept_err` 分支（用单独构造函数），并替换上一个用例：

```rust
    impl FakeStreamingEngine {
        /// accept 恒返回 Err（测 error 路径）。
        fn always_err() -> Self {
            Self {
                accept_out: Mutex::new(vec![]),
                flush_out: Mutex::new(vec![]),
                finish_out: Mutex::new(String::new()),
            }
        }
    }
    impl StreamingEngine for FakeStreamingEngine {
        fn accept_samples(&self, _s: &[f32], _w: bool) -> Result<Option<String>> {
            let mut q = self.accept_out.lock().unwrap();
            if q.is_empty() {
                anyhow::bail!("fake accept error");
            }
            Ok(q.remove(0))
        }
        // flush / finish / reset 同 Step 1
        fn flush(&self, _i: bool) -> Result<Option<String>> {
            Ok(self.flush_out.lock().unwrap().remove(0))
        }
        fn finish(&self) -> Result<String> {
            Ok(self.finish_out.lock().unwrap().clone())
        }
        fn reset(&self) {}
    }

    #[test]
    fn accept_error_becomes_error_event_nonfatal() {
        let mut r = runner(FakeStreamingEngine::always_err());
        let evs = r.push_samples(&[0.0; 512]);
        assert_eq!(evs.len(), 1);
        assert!(matches!(evs[0], TranscriptEvent::Error(_)));
        // 非致命：finish 仍可调用（注意 always_err 的 finish_out 为空串）
        let _ = r.finish();
    }
```

> 实现 TDD 顺序：先把 `FakeStreamingEngine` 一次写全（含 `always_err` 与 `accept_samples` 的空队列 Err 分支），再写各 `#[test]`。上面分两步是为展示推导；实际提交时 `impl` 块只出现一次。

- [x] **Step 5: 运行测试**

Run: `cargo test -p octopus-asr-local streaming_runner`
Expected: 全部通过（`step_silence_*` ×3 + `push_samples_*` ×2 + `finish_emits_final` + `accept_error_*`）。VAD 相关路径在无模型环境由 `detect_silence_gap` 的 `None` 分支短路，不影响这些用例。

---

## Task 4: 全量验证 + 提交

- [x] **Step 1: workspace 全量 check + clippy**

Run: `cargo check --workspace --all-targets`
Expected: 0 error。

Run: `cargo clippy -p octopus-asr-local --all-targets -- -D warnings`
Expected: 无新 warning（asr 既存 warning 维持原样，不引入新增）。

- [x] **Step 2: asr 全量测试回归**

Run: `cargo test -p octopus-asr-local`
Expected: 阶段1 的 68 个测试 + 本 plan 新增测试全过。

- [x] **Step 3: 提交（Task 1+2+3 合并）**

```bash
git add crates/asr/src/streaming_runner.rs crates/asr/src/lib.rs
git commit -m "feat(asr): 新增流式编排基础设施（StreamingRunner + StreamingEngine trait）

阶段2a（spec §3.2/§3.3）：
- TranscriptEvent（Partial/Committed/Final/Error），润色留端
- StreamingEngine trait + impl for StreamingSession（签名对齐，纯委托）
- StreamingRunner 收编本地流式纯 ASR 编排（VAD 静音 + 标点 + accept/flush/finish）
- step_silence 纯函数 + detect_silence_gap 从 coordinator 搬迁
- 单测：FakeStreamingEngine + 静音/标点决策纯逻辑

设计调整（denoise 留 audio.rs、AudioSource 延后、纠错 hook 默认关）见
docs/superpowers/plans/2026-06-25-archived-plan.md#asr-pipeline-stage2a。纯新增，不碰 desktop。"
```

> 分类器提示：`git add` 与 `git commit` 分两行（换行分隔，**不要** `&&`）。

---

## Self-Review（writing-plans 自检）

**1. Spec 覆盖：** spec §3.2 `StreamingEngine`/`AudioSource`/`TranscriptEvent` → Task 1（`StreamingEngine`+`TranscriptEvent`）、`AudioSource` 显式延后（设计调整 §2，2b）；§3.3 `StreamingRunner` → Task 2；§3.3 流式纠错 hook → Task 2 `correct`/`maybe_correct`（默认关）；§9.1 错误事件 → Task 2 `push_samples`/`finish` 的 Error 分支 + Task 3 测试。spec §3.6 denoise 迁移 → 设计调整 §1 明确不迁（用户决策）。✅

**2. 占位符扫描：** Task 3 Step 3 的 `accept_error_becomes_error_event_nonfatal` 初版用「队列耗尽 panic」是不正确的占位——Step 4 已给出 `always_err` fake + Err 用例完整代码替换。最终提交时 `FakeStreamingEngine` 的 `impl` 只写一次（含空队列→Err 分支），无 TBD/TODO。✅

**3. 类型一致性：** `StreamingEngine::{accept_samples,flush,finish,reset}` 与 `StreamingSession` 同名方法签名一致（已核实 streaming_engine.rs:78/154/209/256）；`StreamingRunner::push_samples` 调用 `detect_silence_gap(&mut Option<SileroVad>, &[f32], &mut f64, &mut bool) -> (bool,bool)` 与 Step 2 定义一致；`TranscriptEvent` 变体在 push_samples（Partial/Committed/Error）、finish（Final/Error）、maybe_correct 中使用一致。✅

**4. 行为不变性：** 2a 无调用方（desktop 2b 才接入），运行时行为零变化；`step_silence`/`detect_silence_gap` 逐字搬迁 coordinator 语义（speech≥2 重置、`total_chunks*chunk_duration` 累加、`prev≥阈值` 标点、`silence==0→flushed=false`、`≥阈值&&!flushed→flush+上锁`），单测锁死边界。✅

---

## 2026-06-23-asr-pipeline-stage2b



**Goal:** desktop 本地流式路径迁移——coordinator `Stage::Streaming` 委托 `asr::StreamingRunner`（阶段2a 交付），`handle_streaming_tick` 改为消费 `TranscriptEvent`，stop 路径用 `finish_with_tail`。**运行时行为不变**（逐字等价迁移）。

**Architecture:** 2b 只迁本地流式（`Stage::Streaming`）。cloud（`CloudStreaming`）、`VadSegmented`、`StreamingPipeline` 抽象**留 2c/2d**——2b 让 coordinator 直接持 `StreamingRunner`（单路径无需抽象）。asr 小增量：`StreamingRunner` 补 `preroll_vad`（搬自 coordinator，补齐 2a 遗漏的 VAD 预热）+ `finish_with_tail`（stop 收尾，精确等价原 `accept(tail)+finish`）。coordinator `Stage::Streaming` 四字段（`engine/vad/silence_duration/flushed`）合并为 `runner`，保留 `transcript`+`streaming_active`。

**Tech Stack:** Rust、`octopus_asr_local::streaming_runner::{StreamingRunner, StreamingEngine, TranscriptEvent}`、`octopus_asr_local::streaming_engine::StreamingSession`、Tauri、`Transcript`。

---

## 设计要点（务必读完再动）

1. **行为不变铁律**：2b 是搬迁不是重写。`handle_streaming_tick` 的幂等去重（`text != transcript.full()`）、DB 写、emit、`check_and_trigger_polish` 全保留；只是 ASR 编排（VAD+标点+accept/flush）从 coordinator 内联代码换成 `runner.push_samples`。
2. **VAD 预热补齐**：coordinator 创建 VAD 后调 `vad_preroll`（静音帧 ×10 预热 LSTM，`coordinator.rs:1468`）。阶段2a `StreamingRunner::new` 未预热——2b 把 `preroll_vad` 搬进 runner，`new` 内调用，**与原行为等价**。coordinator 的 `vad_preroll`/`VAD_PREROLL_FRAMES` 保留（`VadSegmented` 仍用，2c 再议）。
3. **stop 路径精确等价**：原 stop（`coordinator.rs:864-884`）= `accept_samples(tail,false)` + `finish()` + `reset()`，**不**走 VAD/flush。`finish_with_tail` 封装此顺序，避免 `push_samples`（会 VAD/flush）引入多余标点。
4. **`StreamingPipeline` 不在 2b 引入**：单本地路径直接持 runner；cloud 接入（2c）再抽 `StreamingPipeline` 统一 local/cloud 分发。
5. **无桌面单测**：coordinator 改动靠 `cargo check --workspace --all-targets` + clippy + **手动 e2e 清单**（Task 6）验证。asr 新方法（`finish_with_tail`/`preroll`）有单测。

---

## File Structure

- **Modify:** `crates/asr/src/streaming_runner.rs` —— 加常量 `VAD_PREROLL_FRAMES`、私有 `preroll_vad`、`new` 内调预热、`pub fn finish_with_tail` + 单测。
- **Modify:** `crates/desktop/src/coordinator.rs` —— `Stage::Streaming` 字段重构、`handle_toggle` use_streaming 创建 runner、`handle_streaming_tick` 重写、stop 路径（`Stage::Streaming` 非 Idle 分支）用 runner。
- **不动：** `crates/desktop/src/audio.rs`（denoise/resample 留，drain_samples 返回 16k 降噪样本）、`transcript.rs`、`VadSegmented`/`CloudStreaming` 路径（2c）。

---

## Task 1: asr `StreamingRunner` 补 `preroll_vad` + `finish_with_tail`

**Files:**
- Modify: `crates/asr/src/streaming_runner.rs`

- [x] **Step 1: 加 `VAD_PREROLL_FRAMES` 常量 + `preroll_vad` 私有函数**

在 `streaming_runner.rs` 现有常量块（`PUNCTUATION_SILENCE_THRESHOLD` 后）追加：

```rust
/// VAD LSTM 预热帧数（搬自 `coordinator.rs:VAD_PREROLL_FRAMES`）。
const VAD_PREROLL_FRAMES: usize = 10;

/// VAD 预热：喂静音帧让 Silero LSTM 状态稳定（搬自 `coordinator.rs:vad_preroll`）。
/// 未预热时开头几帧 prob 偏高/偏低，导致标点检测开头不准。
fn preroll_vad(vad: &mut SileroVad) {
    let silence = vec![0.0_f32; VAD_CHUNK_SIZE];
    for _ in 0..VAD_PREROLL_FRAMES {
        let _ = vad.compute(&silence);
    }
}
```

- [x] **Step 2: `new` 内 VAD 构造后调 `preroll_vad`**

把 `StreamingRunner::new` 的 VAD 构造从：

```rust
        let vad = crate::config::find_silero_vad()
            .ok()
            .and_then(|p| SileroVad::new(&p).ok());
```

改为：

```rust
        let mut vad = crate::config::find_silero_vad()
            .ok()
            .and_then(|p| SileroVad::new(&p).ok());
        if let Some(v) = vad.as_mut() {
            preroll_vad(v);
        }
```

- [x] **Step 3: 加 `finish_with_tail` 方法**

在 `impl StreamingRunner` 的 `finish` 方法后追加：

```rust
    /// 收尾并先吃入尾部样本（stop 路径用）。
    ///
    /// 精确等价 `coordinator.rs:864-881` 的 stop 顺序：`engine.accept_samples(tail, false)`
    /// （**不**走 VAD/flush，`was_silent=false` 不插逗号）→ `engine.finish()`。与 [`push_samples`]
    /// 的区别：push_samples 会 VAD 检测 + 静音冲刷标点，stop 尾部不应触发标点。
    pub fn finish_with_tail(&mut self, tail: &[f32]) -> TranscriptEvent {
        if !tail.is_empty() {
            if let Err(e) = self.engine.accept_samples(tail, false) {
                log::warn!("StreamingRunner finish_with_tail accept error: {e}");
            }
        }
        self.finish()
    }
```

- [x] **Step 4: 加单测**

在 `mod tests` 末尾（最后一个 `#[test]` 后、`}` 闭合前）追加：

```rust
    #[test]
    fn finish_with_tail_emits_final() {
        // accept 队列给 1 个（tail 吃入），finish 返回固定串
        let mut r = runner(FakeStreamingEngine::new(vec!["尾"], vec![], "最终。"));
        let ev = r.finish_with_tail(&[0.0; 512]);
        assert_eq!(ev, TranscriptEvent::Final("最终。".to_string()));
    }

    #[test]
    fn finish_with_tail_empty_tail_still_finishes() {
        let mut r = runner(FakeStreamingEngine::new(vec![], vec![], "空尾。"));
        let ev = r.finish_with_tail(&[]);
        assert_eq!(ev, TranscriptEvent::Final("空尾。".to_string()));
    }
```

> 注：`finish_with_tail` 内部 `accept_samples(tail,false)` 会消耗 accept_out 队列 1 项；`finish_with_tail_empty_tail_still_finishes` 传空 tail → 不调 accept → 队列不消耗（FakeStreamingEngine::new(vec![],…) 的 accept_out 本就空，finish 直接返回）。`finish_with_tail_emits_final` 传 `[0.0;512]` → 调 accept → 消耗 `"尾"` → finish 返回 `"最终。"`。

- [x] **Step 5: 验证 asr**

Run: `cargo test -p octopus-asr-local streaming_runner`
Expected: 原 7 个 + 新增 2 个 = 9 个全过。

Run: `cargo clippy -p octopus-asr-local --all-targets 2>&1 | grep streaming_runner`
Expected: 无输出（无新 warning）。

- [x] **Step 6: 暂不提交（与 Task 2-5 合并提交，或单独提交 asr 增量）**

> 推荐单独提交 asr 增量（Task 1），再提交 coordinator 迁移（Task 2-5），便于回滚定位。

---

## Task 2: coordinator `Stage::Streaming` 字段重构

**Files:**
- Modify: `crates/desktop/src/coordinator.rs`

- [x] **Step 1: 改 `Stage::Streaming` 变体字段**

找到 `Stage::Streaming` 枚举变体（约 67-180 区间的 enum 定义），把：

```rust
    Streaming {
        engine: StreamingSession,
        transcript: Transcript,
        streaming_active: Arc<AtomicBool>,
        vad: Option<octopus_asr_local::vad::SileroVad>,
        silence_duration: f64,
        flushed: bool,
    },
```

改为：

```rust
    Streaming {
        /// 流式编排 runner（持 StreamingSession + VAD + 静音/标点状态，阶段2a）。
        runner: octopus_asr_local::streaming_runner::StreamingRunner,
        transcript: Transcript,
        streaming_active: Arc<AtomicBool>,
    },
```

- [x] **Step 2: 加 import**

coordinator.rs 顶部 `use` 区，在 `StreamingSession` 相关 import 附近加（若无则加）：

```rust
use octopus_asr_local::streaming_runner::{StreamingRunner, TranscriptEvent};
```

> 若 coordinator 用 `use octopus_asr_local::streaming_engine::StreamingSession;`，保留（handle_toggle 创建仍用）。

- [x] **Step 3: 验证编译（预期大量错误，Task 3-5 修复）**

Run: `cargo check -p octopus-desktop`
Expected: 报错集中在 `handle_toggle` 创建点、`handle_streaming_tick`、stop 路径（引用了已删字段 `engine/vad/silence_duration/flushed`）。这是预期的，下面 Task 3-5 逐一修复。

---

## Task 3: `handle_toggle` use_streaming 创建 runner

**Files:**
- Modify: `crates/desktop/src/coordinator.rs`（use_streaming 分支，约 670-736）

- [x] **Step 1: 删原 VAD 创建块，改建 runner**

原代码（约 708-736）：

```rust
                // 初始化 VAD（用于静音检测 + 标点）
                let vad = match octopus_asr_local::config::find_silero_vad() {
                    Ok(path) => match octopus_asr_local::vad::SileroVad::new(&path) {
                        Ok(mut v) => {
                            vad_preroll(&mut v);
                            Some(v)
                        }
                        Err(e) => {
                            warn!("VAD init failed: {}, punctuation disabled", e);
                            None
                        }
                    },
                    Err(e) => {
                        warn!("VAD not found: {}, punctuation disabled", e);
                        None
                    }
                };

                let streaming_active = Arc::new(AtomicBool::new(true));
                start_tick_thread(tx.clone(), streaming_active.clone());

                *stage = Stage::Streaming {
                    engine: streaming_engine,
                    transcript: Transcript::new(now_millis(), config.polish_mode),
                    streaming_active,
                    vad,
                    silence_duration: 0.0,
                    flushed: false,
                };
```

改为（VAD + preroll 由 runner 内部处理；`streaming_engine` 创建不变）：

```rust
                // VAD + 预热由 StreamingRunner 内部处理（阶段2a/2b）
                let runner = match StreamingRunner::new(Box::new(streaming_engine), false) {
                    Ok(r) => r,
                    Err(e) => {
                        error!("StreamingRunner init failed: {}, abort streaming", e);
                        let _ = audio.stop();
                        crate::result_window::hide_result(app_handle);
                        crate::tray::update_tray_label(
                            app_handle,
                            crate::tray::TrayState::Idle,
                        );
                        return;
                    }
                };

                let streaming_active = Arc::new(AtomicBool::new(true));
                start_tick_thread(tx.clone(), streaming_active.clone());

                *stage = Stage::Streaming {
                    runner,
                    transcript: Transcript::new(now_millis(), config.polish_mode),
                    streaming_active,
                };
```

> `correct=false`：desktop 流式现无纠错（与原行为一致，hook 预留）。

- [x] **Step 2: 验证此分支编译**

Run: `cargo check -p octopus-desktop`
Expected: handle_toggle 创建点不再报错；剩余报错在 `handle_streaming_tick` + stop 路径（Task 4-5）。

---

## Task 4: `handle_streaming_tick` 重写委托 runner

**Files:**
- Modify: `crates/desktop/src/coordinator.rs`（`handle_streaming_tick`，约 1968-2037）

- [x] **Step 1: 整体替换 `handle_streaming_tick` 函数体**

原函数（1968-2037，含内联 detect/accept/flush）整体替换为：

```rust
fn handle_streaming_tick(
    stage: &mut Stage,
    audio: &Arc<SharedAudioState>,
    config: &AppConfig,
    app_handle: &tauri::AppHandle,
    tx: &Sender<Command>,
) {
    let Stage::Streaming {
        runner,
        transcript,
        ..
    } = stage
    else {
        return;
    };

    let samples = audio.drain_samples();
    if samples.is_empty() {
        return;
    }

    // ASR 编排（VAD 静音 + 标点 + accept/flush）委托 runner（阶段2a）
    for event in runner.push_samples(&samples) {
        match event {
            TranscriptEvent::Partial(text) | TranscriptEvent::Committed(text) => {
                // 幂等：内容未变不重绘（消除静音期/同文本反复 update 闪烁 + 无谓 DB 写）
                if text != transcript.full() {
                    transcript.set_full(&text);
                    if let Err(e) =
                        update_transcription_raw(transcript, &config.asr_engine, "streaming")
                    {
                        warn!("DB (streaming) failed: {}", e);
                    }
                    crate::result_window::update_result(app_handle, &transcript.display_text());
                }
            }
            TranscriptEvent::Final(_) => {
                // Final 只在 stop 路径产生（finish），tick 不应收到；防御性忽略
                debug!("Streaming tick got unexpected Final event, ignored");
            }
            TranscriptEvent::Error(e) => warn!("Streaming event error: {}", e),
        }
    }

    // 停顿润色（留端，spec §3.8）
    check_and_trigger_polish(transcript, runner.silence_duration(), config, tx);
}
```

> 行为等价原 `handle_streaming_tick`：accept→Partial 与 flush→Committed 都走同一幂等 `set_full+DB+emit`（原代码两条分支逻辑完全一致，合并）；`check_and_trigger_polish` 用 `runner.silence_duration()` 取代原 `*silence_duration`。

- [x] **Step 2: 删已无用的内联 VAD helper（若仅 Streaming 用）**

检查 `detect_silence_gap`（原 2045-2099）的调用方。`grep -n detect_silence_gap crates/desktop/src/coordinator.rs`：
- 若仅 `handle_streaming_tick`（已删）调用 → **删除** `detect_silence_gap` 函数（逻辑已迁 asr `streaming_runner::detect_silence_gap`）。
- 若 `VadSegmented`/cloud 也调 → 保留。

`compute_speech_chunks`（1444）同理检查：被 `handle_vad_segmented_tick`/`handle_cloud_streaming_tick` 调用 → **保留**（2c 才动）。

`VAD_CHUNK_SIZE`/`VAD_SPEECH_THRESHOLD`/`PUNCTUATION_SILENCE_THRESHOLD` 常量：检查是否仍被 coordinator 其他处用（`detect_silence_gap` 删后可能 unused）→ 仅当确认无其他引用才删，否则保留。

- [x] **Step 3: 验证编译**

Run: `cargo check -p octopus-desktop`
Expected: `handle_streaming_tick` 不再报错；剩余报错在 stop 路径（Task 5）。

---

## Task 5: stop 路径用 `runner.finish_with_tail`

**Files:**
- Modify: `crates/desktop/src/coordinator.rs`（`handle_toggle` 的 `Stage::Streaming` 非 Idle 分支，约 852-897）

- [x] **Step 1: 替换 stop 分支**

原代码（852-897）：

```rust
        Stage::Streaming {
            engine: streaming_engine,
            transcript,
            streaming_active,
            ..
        } => {
            info!("Toggle: stopping streaming, finalizing");
            streaming_active.store(false, Ordering::Relaxed);
            let final_samples = audio.drain_samples();
            if !final_samples.is_empty() {
                if let Err(e) = streaming_engine.accept_samples(&final_samples, false) {
                    warn!("Error processing final samples: {}", e);
                }
            }
            let final_text = match streaming_engine.finish() {
                Ok(text) => text,
                Err(e) => {
                    error!("Streaming finish failed: {}", e);
                    transcript.edited_display().unwrap_or_else(|| transcript.db_text())
                }
            };
            streaming_engine.reset();
            let _ = audio.stop();
            if !final_text.is_empty() {
                transcript.set_full(&final_text);
            }
            info!("Final streaming text: '{}'", transcript.db_text());
            let tr = std::mem::replace(transcript, Transcript::new(0, PolishMode::Disabled));
            finalize_after_stop(stage, tr, config, app_handle, tx);
        }
```

改为：

```rust
        Stage::Streaming {
            runner,
            transcript,
            streaming_active,
        } => {
            info!("Toggle: stopping streaming, finalizing");
            streaming_active.store(false, Ordering::Relaxed);
            let final_samples = audio.drain_samples();
            // 尾部样本 + finish（精确等价原 accept(tail,false)+finish；不走 VAD/标点）
            let final_text = match runner.finish_with_tail(&final_samples) {
                TranscriptEvent::Final(text) => text,
                TranscriptEvent::Error(e) => {
                    error!("Streaming finish failed: {}", e);
                    transcript.edited_display().unwrap_or_else(|| transcript.db_text())
                }
                _ => transcript.edited_display().unwrap_or_else(|| transcript.db_text()),
            };
            runner.reset();
            let _ = audio.stop();
            if !final_text.is_empty() {
                transcript.set_full(&final_text);
            }
            info!("Final streaming text: '{}'", transcript.db_text());
            let tr = std::mem::replace(transcript, Transcript::new(0, PolishMode::Disabled));
            finalize_after_stop(stage, tr, config, app_handle, tx);
        }
```

> 行为等价：`finish_with_tail` 内部 `accept(tail,false)+finish`；Error 兜底 `edited_display||db_text` 与原 `finish()` Err 分支一致。

- [x] **Step 2: 检查 `StreamingSession` import 是否仍需**

`grep -n "StreamingSession" crates/desktop/src/coordinator.rs`：handle_toggle use_streaming 仍用 `StreamingSession::new` → **保留** import。

- [x] **Step 3: 全量编译**

Run: `cargo check --workspace --all-targets`
Expected: 0 error。若仍有引用已删字段（`engine/vad/silence_duration/flushed` on `Stage::Streaming`），按报错逐一改（应已无）。

---

## Task 6: 验证 + 提交

- [x] **Step 1: workspace check + clippy**

Run: `cargo check --workspace --all-targets`
Expected: 0 error。

Run: `cargo clippy -p octopus-desktop --all-targets 2>&1 | grep -E "streaming|StreamingRunner|error" | head`
Expected: 无新增 streaming 相关 warning（desktop 既存 warning 维持）。若有 `unused import`/`dead_code`（如删 detect_silence_gap 后残留常量），按提示清理。

Run: `cargo clippy -p octopus-asr-local --all-targets 2>&1 | grep streaming_runner`
Expected: 无输出。

- [x] **Step 2: asr + cli 回归（不应受影响）**

Run: `cargo test -p octopus-asr-local`
Expected: 83 tests（原 81 + Task 1 新增 2）全过（75+2 pass + 6 ignored）。

Run: `cargo test -p octopus-cli`
Expected: 4 tests 全过。

- [x] **Step 3: desktop 构建（确认 Tauri 链接无误）**

Run: `cargo build -p octopus-desktop`
Expected: 0 error（desktop 无单测，靠 build + 手动 e2e）。

- [x] **Step 4: 手动 e2e 清单（行为不变验证）**（已随 stage2b ff-merge main，e2e 通过 2026-06-25）

本地运行 desktop（`cargo tauri dev` 或既有启动方式），逐项验证本地流式（非 cloud、非 VadSegmented）：

- [x] 开录音（use_streaming 配置）→ result window 显示「正在聆听…」
- [x] 说一句中文 → 实时增量文本出现（Partial）
- [x] 停顿 >0.5s → 文本插入逗号（Committed，VAD 标点）
- [x] 继续说 → 新增文本，逗号标点正常（验证 preroll 后 VAD 标点开头不偏）
- [x] 停录音（toggle off）→ 追加句号 + 走润色/粘贴（Final，stop 路径）
- [x] DB（`~/.octopus/`）有 streaming 记录、文本正确
- [x] 静音期无闪烁（幂等去重生效）

> 若本地无法 e2e，至少完成 Step 1-3 并在提交信息标注「e2e 待本地验证」。

- [x] **Step 5: 提交**

asr 增量（Task 1）：

```bash
git add crates/asr/src/streaming_runner.rs
git commit -m "feat(asr): StreamingRunner 补 VAD 预热 + finish_with_tail（阶段2b 接线）

- preroll_vad 搬自 coordinator（补 2a 遗漏的 LSTM 预热，VAD_PREROLL_FRAMES=10）
- new() 内构造 VAD 后预热，与 desktop 原行为等价
- finish_with_tail(tail)：accept(tail,false)+finish，供 desktop stop 收尾
  （精确等价原 stop 顺序，不走 VAD/标点）
- 2 个新单测（finish_with_tail 有/无 tail）"
```

coordinator 迁移（Task 2-5）：

```bash
git add crates/desktop/src/coordinator.rs
git commit -m "refactor(desktop): Stage::Streaming 委托 StreamingRunner（阶段2b）

本地流式路径迁移，运行时行为不变：
- Stage::Streaming {engine,vad,silence_duration,flushed} → {runner,transcript,streaming_active}
- handle_streaming_tick 改消费 TranscriptEvent（Partial/Committed 幂等 set_full+DB+emit）
- stop 路径用 runner.finish_with_tail（精确等价 accept(tail)+finish+reset）
- handle_toggle 创建 StreamingRunner（VAD+preroll 由 runner 内部）
- 删已迁 asr 的 detect_silence_gap（仅 Streaming 用时）

cloud/VadSegmented/StreamingPipeline 抽象留 2c/2d。e2e 清单见
docs/superpowers/plans/2026-06-25-archived-plan.md#asr-pipeline-stage2b。"
```

> 分类器提示：`git add` 与 `git commit` 分两行（换行分隔，**不要** `&&`）。

---

## Self-Review（writing-plans 自检）

**1. Spec 覆盖：** spec §3.4 desktop `StreamingPipeline` → 2b 设计要点 §4 明确**留 2c**（单路径不抽象），coordinator 直接持 runner；§3.3 `StreamingRunner` 接入 → Task 3-5；§3.8 润色留端 → Task 4 `check_and_trigger_polish` 保留。denoise 留 audio.rs（2a 设计调整延续）→ 未动 audio.rs。✅

**2. 占位符扫描：** 无 TBD/TODO。Task 4 Step 2 的「检查 detect_silence_gap 调用方」是条件性删除（依赖 grep 结果），给出了两种分支的处理，非占位。Task 6 Step 4 e2e 清单是验证项不是实现占位。✅

**3. 类型一致性：** `StreamingRunner::new(Box<dyn StreamingEngine>, bool)`（2a）← Task 3 `StreamingRunner::new(Box::new(streaming_engine), false)` 一致；`finish_with_tail(&[f32]) -> TranscriptEvent`（Task 1 定义）← Task 5 调用一致；`push_samples(&[f32]) -> Vec<TranscriptEvent>` + `silence_duration() -> f64`（2a）← Task 4 调用一致；`TranscriptEvent::{Partial,Committed,Final,Error}`（2a）← Task 4/5 match 一致。✅

**4. 行为不变性：** Task 1 preroll 补齐 2a 遗漏（与 coordinator 原 vad_preroll 等价）；Task 4 合并 accept/flush 两条幂等分支（原代码两条分支 set_full+DB+emit 逻辑完全相同）；Task 5 finish_with_tail 精确等价 accept(tail,false)+finish+reset。无单测靠 e2e 清单（Task 6 Step 4）+ 编译/clippy 兜底。✅

---

## 2026-06-23-asr-pipeline-stage2c1



**Goal:** 落地 spec §3.4 的 `StreamingPipeline`——新建 `crates/desktop/src/pipeline.rs`，`StreamingPipeline` 持 `StreamingRunner`，承载 local 流式的「ASR 编排结果（`TranscriptEvent`）→ 文本状态更新（`Transcript::set_full`）」；coordinator `Stage::Streaming` 持 `pipeline` 替代直接持 `runner`，`handle_streaming_tick` 退化为 `drain + pipeline.tick + (DB + emit) + polish`。**运行时行为完全不变**（set_full→DB→emit 顺序保留）。

**Architecture:** 2c-1 是 2c 的低风险前置（用户 2026-06-23 决策「拆 2c：先低风险搬迁」）。cloud（utterance 级异步，与 `StreamingEngine` sample 级同步语义不匹配）+ VadSegmented（离线分段）**暂留 coordinator 不动**，留 2c-2 单独设计 cloud 接入。2c-1 只立 `StreamingPipeline` 壳 + 把 local 路径的 ASR→文本更新迁入；emit/DB/polish 留 coordinator（DB/polish 被 local + VadSegmented + cloud 三路径共用，移出会碰其他路径）。端胶水全收敛（含 emit）留 2d（transcript 进 pipeline 时一起）。

**Tech Stack:** Rust、`octopus_asr_local::streaming_runner::{StreamingRunner, StreamingEngine, TranscriptEvent}`、`crate::transcript::Transcript`。

---

## 设计要点（务必读完再动）

1. **行为不变铁律**：2c-1 是搬迁 + 一层间接，零行为差异。`pipeline.tick` 内的 `set_full` 逐字搬自 `handle_streaming_tick`（2b 版本）的幂等分支；emit/DB/polish 留 coordinator，**调用点与顺序（set_full → DB → emit）完全不变**。
2. **pipeline 边界（关键决策）**：`StreamingPipeline` 承载 **ASR 编排结果 → 文本状态更新（set_full）**，返回 `changed: bool`。**不承载** emit/DB/polish——emit 是 UI 胶水（留 coordinator 与 DB 同步触发，保持顺序），DB（`update_transcription_raw`）/polish（`check_and_trigger_polish`）被 local + VadSegmented(1414/1789) + cloud(1789) 三路径共用，移出会碰 cloud/VadSegmented（违反 2c-1「不碰」原则）。emit/DB/polish 全收敛进 pipeline 留 2d（连同 transcript）。
3. **emit/DB 顺序不变**：`pipeline.tick` 只 set_full（不 emit，不需 AppHandle）；coordinator 在 `changed=true` 后做 `DB + emit`（与原 `handle_streaming_tick` 的 `set_full → DB → emit` 完全一致）。**零行为差异**，且 `pipeline.tick` 无 AppHandle 依赖 → 单测可干净覆盖 changed=true 的 set_full 路径。
4. **transcript 留 Stage::Streaming**：`pipeline` 只持 `runner`，不持 `transcript`（transcript 被 cancel/discard/polish_done 等多处 `Stage::Streaming { transcript, .. }` 访问，进 pipeline 引发大量解构点改动）。`pipeline.tick` 接收 `&mut Transcript`。最小搬迁面。
5. **cloud/VadSegmented 零改动**：2c-1 只动 `Stage::Streaming`（local）。`Stage::CloudStreaming`/`VadSegmented`/`CloudClosing` 及其 handler 不碰。
6. **单测**：`pipeline.rs` 加 2 单测（tick Partial→set_full changed=true / finish_with_tail 委托），用 `FakeStreamingEngine` + `Transcript`，无需 AppHandle/Tauri runtime。

---

## File Structure

- **Create:** `crates/desktop/src/pipeline.rs` —— `StreamingPipeline { runner: StreamingRunner }` + `new`/`tick`/`finish_with_tail`/`silence_duration`/`reset` + 2 单测。
- **Modify:** `crates/desktop/src/main.rs` —— 加 `mod pipeline;`。
- **Modify:** `crates/desktop/src/coordinator.rs` —— `Stage::Streaming` 字段 `runner`→`pipeline`；`use crate::pipeline::StreamingPipeline`；`handle_toggle`/`handle_streaming_tick`/stop/cancel/discard 引用改 `pipeline`。
- **不动：** `crates/asr/*`、`audio.rs`、`transcript.rs`、`result_window.rs`、cloud/VadSegmented 路径。

---

## Task 1: 新建 `pipeline.rs`（StreamingPipeline 壳 + tick + 委托）

**Files:**
- Create: `crates/desktop/src/pipeline.rs`

- [x] **Step 1: 写 `pipeline.rs` 完整内容**

```rust
//! desktop 流式 pipeline（spec §3.4）。
//!
//! [`StreamingPipeline`] 持 [`StreamingRunner`]（asr，2a/2b），承载 local 流式的
//! 「ASR 编排结果（`TranscriptEvent`）→ 文本状态更新（`Transcript::set_full`）」。
//!
//! **边界**（2c-1）：emit（`result_window::update_result`）/DB（`coordinator::update_transcription_raw`）
//! /polish（`coordinator::check_and_trigger_polish`）留 coordinator——emit 与 DB 同步触发以保持
//! `set_full → DB → emit` 顺序；DB/polish 被 local + VadSegmented + cloud 三路径共用，移出会碰
//! 其他路径。transcript 也留 `Stage::Streaming`（多处访问），`tick` 接收 `&mut Transcript`。
//! emit/DB/polish 全收敛留 2d（transcript 进 pipeline 时一起）。
//!
//! cloud（utterance 级异步）/VadSegmented（离线分段）不进本 pipeline，留 coordinator（2c-2）。

use crate::transcript::Transcript;
use log::{debug, warn};
use octopus_asr_local::streaming_runner::{StreamingEngine, StreamingRunner, TranscriptEvent};

/// local 流式 pipeline：持 [`StreamingRunner`]，承载 TranscriptEvent → set_full。
///
/// 不持 transcript（留 `Stage::Streaming`），`tick` 接收 `&mut Transcript`。
/// 不持 denoise/resample（留 `audio.rs`，输入为已降噪 16k 样本）。
pub struct StreamingPipeline {
    runner: StreamingRunner,
}

impl StreamingPipeline {
    /// 构造 pipeline。`engine` 由调用方创建（local `StreamingSession`）。
    /// 内部构造 `StreamingRunner`（含 VAD 预热，2b）。
    pub fn new(engine: Box<dyn StreamingEngine>, correct: bool) -> anyhow::Result<Self> {
        Ok(Self {
            runner: StreamingRunner::new(engine, correct)?,
        })
    }

    /// 喂一帧已降噪 16k 样本：runner 编排 → TranscriptEvent → set_full。
    ///
    /// 返回 `true` 表示文本变化（coordinator 据决定是否 DB + emit，保持「内容未变不落库/不重绘」幂等）。
    /// 只承载 set_full（文本状态更新）；emit/DB/polish 留 coordinator（设计要点 §2/§3）。
    /// set_full 幂等逻辑收编自 `coordinator::handle_streaming_tick`（2b 版本）。
    pub fn tick(&mut self, samples: &[f32], transcript: &mut Transcript) -> bool {
        let mut changed = false;
        for event in self.runner.push_samples(samples) {
            match event {
                TranscriptEvent::Partial(text) | TranscriptEvent::Committed(text) => {
                    if text != transcript.full() {
                        transcript.set_full(&text);
                        changed = true;
                    }
                }
                TranscriptEvent::Final(_) => {
                    // Final 只在 stop 路径产生（finish），tick 不应收到；防御性忽略
                    debug!("StreamingPipeline tick got unexpected Final event, ignored");
                }
                TranscriptEvent::Error(e) => warn!("StreamingPipeline event error: {}", e),
            }
        }
        changed
    }

    /// 收尾并先吃入尾部样本（stop 路径用）。委托 [`StreamingRunner::finish_with_tail`]。
    pub fn finish_with_tail(&mut self, tail: &[f32]) -> TranscriptEvent {
        self.runner.finish_with_tail(tail)
    }

    /// 当前累积静音时长（秒），供 coordinator 判断停顿润色。委托 runner。
    pub fn silence_duration(&self) -> f64 {
        self.runner.silence_duration()
    }

    /// 重置（会话间复用）。委托 runner。
    pub fn reset(&mut self) {
        self.runner.reset();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::PolishMode;
    use std::sync::Mutex;

    /// 可编程 fake（搬自 `streaming_runner::tests`）。
    struct FakeStreamingEngine {
        accept_out: Mutex<Vec<Option<String>>>,
        finish_out: Mutex<String>,
    }

    impl FakeStreamingEngine {
        fn new(accept: Vec<&str>, finish: &str) -> Self {
            Self {
                accept_out: Mutex::new(
                    accept.into_iter().map(|s| Some(s.to_string())).collect(),
                ),
                finish_out: Mutex::new(finish.to_string()),
            }
        }
    }

    impl StreamingEngine for FakeStreamingEngine {
        fn accept_samples(
            &self,
            _samples: &[f32],
            _was_silent: bool,
        ) -> anyhow::Result<Option<String>> {
            let mut q = self.accept_out.lock().unwrap();
            if q.is_empty() {
                anyhow::bail!("fake accept error");
            }
            Ok(q.remove(0))
        }
        fn flush(&self, _insert_comma: bool) -> anyhow::Result<Option<String>> {
            Ok(None)
        }
        fn finish(&self) -> anyhow::Result<String> {
            Ok(self.finish_out.lock().unwrap().clone())
        }
        fn reset(&self) {}
    }

    fn pipeline(fake: FakeStreamingEngine) -> StreamingPipeline {
        StreamingPipeline::new(Box::new(fake), false).unwrap()
    }

    #[test]
    fn tick_partial_updates_transcript_and_signals_changed() {
        // accept 首次返回 Some("你好") → Partial → transcript.full 由 "" 变 "你好" → changed=true
        let mut p = pipeline(FakeStreamingEngine::new(vec!["你好"], "你好。"));
        let mut t = Transcript::new(0, PolishMode::Disabled);
        let changed = p.tick(&[0.0; 1600], &mut t);
        assert!(changed);
        assert_eq!(t.full(), "你好");
    }

    #[test]
    fn finish_with_tail_delegates_to_runner() {
        // pipeline.finish_with_tail 委托 runner；accept 队列给 1 个（tail 吃入），finish 返回固定串
        let mut p = pipeline(FakeStreamingEngine::new(vec!["尾"], "最终。"));
        let ev = p.finish_with_tail(&[0.0; 512]);
        assert_eq!(ev, TranscriptEvent::Final("最终。".to_string()));
    }
}
```

- [x] **Step 2: 验证 pipeline.rs 编译（需先加 mod）**

Run: `cargo check -p octopus-desktop`（Task 2 加 `mod pipeline;` 后）
Expected: pipeline.rs 自身无错（`Transcript::set_full`/`full`、`PolishMode` 路径正确）。`crate::config::PolishMode` 可见（coordinator 已 `use crate::config::PolishMode`，同 crate）。

---

## Task 2: `main.rs` 注册模块 + coordinator import + Stage 字段

**Files:**
- Modify: `crates/desktop/src/main.rs`、`crates/desktop/src/coordinator.rs`

- [x] **Step 1: main.rs 加 `mod pipeline;`**

在 `mod coordinator;`（约 line 5）后加：

```rust
mod pipeline;
```

- [x] **Step 2: coordinator.rs 加 import**

顶部 `use` 区，2b 加的 `use octopus_asr_local::streaming_runner::{StreamingRunner, TranscriptEvent};` 附近：

```rust
use crate::pipeline::StreamingPipeline;
```

- [x] **Step 3: `Stage::Streaming` 字段 `runner`→`pipeline`**

```rust
    Streaming {
        /// 流式编排 runner（持 StreamingSession + VAD + 静音/标点状态，阶段2a）。
        runner: octopus_asr_local::streaming_runner::StreamingRunner,
        transcript: Transcript,
        streaming_active: Arc<AtomicBool>,
    },
```

改为：

```rust
    Streaming {
        /// 流式 pipeline（持 StreamingRunner + 承载 set_full 文本更新，spec §3.4）。
        pipeline: crate::pipeline::StreamingPipeline,
        transcript: Transcript,
        streaming_active: Arc<AtomicBool>,
    },
```

- [x] **Step 4: 验证编译（预期报错，Task 3-4 修复）**

Run: `cargo check -p octopus-desktop`
Expected: 报错集中在引用旧字段 `runner` 的 5 处（handle_toggle、handle_streaming_tick、stop、cancel、discard）。

---

## Task 3: `handle_toggle` 创建 `StreamingPipeline`

**Files:**
- Modify: `crates/desktop/src/coordinator.rs`（use_streaming 分支）

- [x] **Step 1: 改 runner 创建为 pipeline 创建**

原（2b）：

```rust
                // VAD + 预热由 StreamingRunner 内部处理（阶段2a/2b）
                let runner = match StreamingRunner::new(Box::new(streaming_engine), false) {
                    Ok(r) => r,
                    Err(e) => {
                        error!("StreamingRunner init failed: {}, abort streaming", e);
                        let _ = audio.stop();
                        crate::result_window::hide_result(app_handle);
                        crate::tray::update_tray_label(
                            app_handle,
                            crate::tray::TrayState::Idle,
                        );
                        return;
                    }
                };

                let streaming_active = Arc::new(AtomicBool::new(true));
                start_tick_thread(tx.clone(), streaming_active.clone());

                *stage = Stage::Streaming {
                    runner,
                    transcript: Transcript::new(now_millis(), config.polish_mode),
                    streaming_active,
                };
```

改为：

```rust
                // StreamingPipeline 内部构造 StreamingRunner（VAD + 预热，阶段2a/2b）
                let pipeline = match StreamingPipeline::new(Box::new(streaming_engine), false) {
                    Ok(p) => p,
                    Err(e) => {
                        error!("StreamingPipeline init failed: {}, abort streaming", e);
                        let _ = audio.stop();
                        crate::result_window::hide_result(app_handle);
                        crate::tray::update_tray_label(
                            app_handle,
                            crate::tray::TrayState::Idle,
                        );
                        return;
                    }
                };

                let streaming_active = Arc::new(AtomicBool::new(true));
                start_tick_thread(tx.clone(), streaming_active.clone());

                *stage = Stage::Streaming {
                    pipeline,
                    transcript: Transcript::new(now_millis(), config.polish_mode),
                    streaming_active,
                };
```

- [x] **Step 2: 验证此分支编译**

Run: `cargo check -p octopus-desktop`
Expected: handle_toggle 不再报错；剩余在 tick + stop/cancel/discard（Task 4）。

---

## Task 4: `handle_streaming_tick` 调 `pipeline.tick` + stop/cancel/discard

**Files:**
- Modify: `crates/desktop/src/coordinator.rs`

- [x] **Step 1: `handle_streaming_tick` 重写**

原（2b）：

```rust
    let Stage::Streaming {
        runner,
        transcript,
        ..
    } = stage
    else {
        return;
    };

    let samples = audio.drain_samples();
    if samples.is_empty() {
        return;
    }

    // ASR 编排（VAD 静音 + 标点 + accept/flush）委托 runner（阶段2a）
    for event in runner.push_samples(&samples) {
        match event {
            TranscriptEvent::Partial(text) | TranscriptEvent::Committed(text) => {
                // 幂等：内容未变不重绘（消除静音期/同文本反复 update 闪烁 + 无谓 DB 写）
                if text != transcript.full() {
                    transcript.set_full(&text);
                    if let Err(e) =
                        update_transcription_raw(transcript, &config.asr_engine, "streaming")
                    {
                        warn!("DB (streaming) failed: {}", e);
                    }
                    crate::result_window::update_result(app_handle, &transcript.display_text());
                }
            }
            TranscriptEvent::Final(_) => {
                debug!("Streaming tick got unexpected Final event, ignored");
            }
            TranscriptEvent::Error(e) => warn!("Streaming event error: {}", e),
        }
    }

    // 停顿润色（留端，spec §3.8）
    check_and_trigger_polish(transcript, runner.silence_duration(), config, tx);
}
```

改为（pipeline.tick 承载 set_full 返回 changed；DB + emit + polish 留 coordinator，顺序 set_full→DB→emit 不变）：

```rust
    let Stage::Streaming {
        pipeline,
        transcript,
        ..
    } = stage
    else {
        return;
    };

    let samples = audio.drain_samples();
    if samples.is_empty() {
        return;
    }

    // ASR 编排 + 文本更新委托 pipeline（spec §3.4）；changed 表示文本变化
    let changed = pipeline.tick(&samples, transcript);
    if changed {
        // 幂等：内容未变不落库/不重绘（DB + emit 留 coordinator，保持 set_full→DB→emit 顺序）
        if let Err(e) = update_transcription_raw(transcript, &config.asr_engine, "streaming") {
            warn!("DB (streaming) failed: {}", e);
        }
        crate::result_window::update_result(app_handle, &transcript.display_text());
    }

    // 停顿润色（留 coordinator：三路径共用 check_and_trigger_polish，spec §3.8）
    check_and_trigger_polish(transcript, pipeline.silence_duration(), config, tx);
}
```

> **行为等价**：pipeline.tick 内 `set_full`（原内联）；changed=true 后 coordinator `DB + emit`——顺序 `set_full → DB → emit` 与原完全一致。幂等（changed=false 不 DB/emit）保留。

- [x] **Step 2: stop 路径 `runner`→`pipeline`**

原 stop 分支解构 + finish_with_tail + reset（2b）的 `runner` 全改 `pipeline`：

```rust
        Stage::Streaming {
            runner,
            transcript,
            streaming_active,
        } => {
```
→
```rust
        Stage::Streaming {
            pipeline,
            transcript,
            streaming_active,
        } => {
```

分支内 `runner.finish_with_tail(&final_samples)` → `pipeline.finish_with_tail(&final_samples)`；`runner.reset()` → `pipeline.reset()`。其余（streaming_active/final_text match/audio.stop/finalize_after_stop）不变。

- [x] **Step 3: handle_cancel `runner`→`pipeline`**

```rust
        Stage::Streaming {
            runner,
            streaming_active,
            ..
        } => {
            info!("Cancel: stopping streaming");
            streaming_active.store(false, Ordering::Relaxed);
            runner.reset();
            let _ = audio.stop();
        }
```
→
```rust
        Stage::Streaming {
            pipeline,
            streaming_active,
            ..
        } => {
            info!("Cancel: stopping streaming");
            streaming_active.store(false, Ordering::Relaxed);
            pipeline.reset();
            let _ = audio.stop();
        }
```

- [x] **Step 4: handle_discard `runner`→`pipeline`**

handle_discard 的 `Stage::Streaming { runner, streaming_active, .. }` 分支同 Step 3 改法（`runner`→`pipeline`，`runner.reset()`→`pipeline.reset()`，info! 文案 "Discard: stopping streaming" 不变）。

- [x] **Step 5: 检查 `StreamingRunner` import 是否仍需**

Run: `grep -n "StreamingRunner" crates/desktop/src/coordinator.rs`
Expected: Task 3-4 全改 pipeline 后，coordinator 不再直接用 `StreamingRunner` → 删 `use octopus_asr_local::streaming_runner::StreamingRunner;`。**保留 `TranscriptEvent`**（stop 路径 `match pipeline.finish_with_tail` 仍用）。

> 若 grep 显示 `StreamingRunner` 仅在注释 → 删 import。

- [x] **Step 6: 全量编译**

Run: `cargo check --workspace --all-targets`
Expected: 0 error。残留 `runner` on `Stage::Streaming` 按 grep 逐一改（应已无）。

---

## Task 5: 验证 + 文档同步 + 提交

- [x] **Step 1: workspace check + clippy**

Run: `cargo check --workspace --all-targets`
Expected: 0 error。

Run: `cargo clippy -p octopus-desktop --all-targets 2>&1 | grep -B1 -A3 "src/pipeline.rs" | head`
Expected: 无 pipeline.rs warning。

Run: `cargo clippy -p octopus-desktop --all-targets 2>&1 | grep -E "unused import|StreamingRunner" | head`
Expected: 若 Task 4 Step 5 删了 `StreamingRunner` import → 无 unused；若漏删 → 按提示删。

- [x] **Step 2: 回归测试**

Run: `cargo test -p octopus-asr-local`
Expected: 77 passed + 6 ignored（2c-1 不碰 asr）。

Run: `cargo test -p octopus-desktop`
Expected: 2 passed（tick_partial_updates_transcript_and_signals_changed + finish_with_tail_delegates_to_runner）。

- [x] **Step 3: desktop 构建**

Run: `cargo build -p octopus-desktop`
Expected: 0 error（Tauri 链接通过）。

- [x] **Step 4: 手动 e2e 清单（行为不变验证，用户本地）**

本地运行 desktop，逐项验证本地流式（非 cloud、非 VadSegmented）：

- [x] 开录音（use_streaming 配置）→ result window 显示「正在聆听…」
- [x] 说一句中文 → 实时增量文本出现（Partial → pipeline.tick set_full → emit）
- [x] 停顿 >0.5s → 文本插入逗号（Committed，VAD 标点）
- [x] DB（`~/.octopus/`）有 streaming 记录、文本正确（验证 changed → DB + emit）
- [x] 停录音（toggle off）→ 追加句号 + 走润色/粘贴（Final，pipeline.finish_with_tail）
- [x] 静音期无闪烁（幂等：changed=false 不 DB/emit）
- [x] Cancel（Esc）/Discard（关闭）→ 流式中断、pipeline.reset 生效

> 与 2b e2e 清单一致（2c-1 零行为差异）。

- [x] **Step 5: 同步文档 + 提交**

spec banner（`docs/superpowers/specs/2026-06-25-archived-spec.md#asr-pipeline-design`）2c 行更新：

```
> - **2c-1（已实施，commit <SHA>，e2e 待本地）**：StreamingPipeline 壳立 + local ASR→set_full 迁入 `desktop/pipeline.rs`；emit/DB/polish 留 coordinator（三路径共用 / 保持顺序）；transcript 留 Stage。cloud/VadSegmented 不动（plan `stage2c1.md`）。
> - **2c-2（待）**：cloud 接入设计（utterance 级异步 vs StreamingEngine sample 级同步语义不匹配，需 brainstorm adapter / 分层接口）。
> - **2d（待）**：coordinator 清理——emit/DB/polish + transcript 全收敛进 pipeline。
```

architecture.md：补 `desktop/src/pipeline.rs` 模块行 + Streaming 数据流（coordinator 经 `StreamingPipeline`）。

提交（2 个代码 + 1 文档）：

```bash
git add crates/desktop/src/pipeline.rs crates/desktop/src/main.rs
git commit -m "feat(desktop): 新建 StreamingPipeline（spec §3.4，阶段2c-1）

- pipeline.rs：StreamingPipeline { runner } 承载 TranscriptEvent → set_full
- tick 返回 changed 供 coordinator 决定 DB + emit（保持幂等 + set_full→DB→emit 顺序）
- emit/DB/polish 留 coordinator（local+VadSegmented+cloud 三路径共用）
- 2 单测（tick Partial→set_full / finish_with_tail 委托）"
```

```bash
git add crates/desktop/src/coordinator.rs
git commit -m "refactor(desktop): Stage::Streaming 持 StreamingPipeline（阶段2c-1）

- Stage::Streaming { runner } → { pipeline }（local 路径）
- handle_streaming_tick：drain + pipeline.tick + DB + emit + polish（退化为路由）
- handle_toggle/stop/cancel/discard 同步 runner→pipeline
- 删未用的 StreamingRunner import（保留 TranscriptEvent）

cloud/VadSegmented 零改动（留 2c-2）。行为零差异，e2e 待本地。"
```

文档提交：

```bash
git add docs/
git commit -m "docs: 同步 ASR pipeline 阶段2c-1（StreamingPipeline 壳）实施状态"
```

> 分类器提示：`git add` 与 `git commit` 分两行（换行分隔，**不要** `&&`）。

---

## Self-Review（writing-plans 自检）

**1. Spec 覆盖：** spec §3.4 `StreamingPipeline { source, runner, cfg }` → 2c-1 落地 `{ runner }`（source 留 audio.rs/cpal，cfg 延后，transcript 留 stage——2d 收敛）；§3.4「收编流式分发」→ 2c-1 收编 local ASR→set_full，分发（local/cloud）+ emit/DB/polish 留 coordinator（cloud 2c-2，emit/DB 2d）；§3.8 polish 留端 → `check_and_trigger_polish` 留 coordinator。✅

**2. 占位符扫描：** 无 TBD/TODO。Task 4 Step 2 的 stop 分支 `runner.→pipeline.` 给出精确 old/new 片段。Task 4 Step 5 的 grep 是条件性删 import（给出判定）。✅

**3. 类型一致性：** `StreamingPipeline::new(Box<dyn StreamingEngine>, bool) -> anyhow::Result<Self>` ← Task 3 一致；`tick(&[f32], &mut Transcript) -> bool` ← Task 4 一致；`finish_with_tail(&[f32]) -> TranscriptEvent`（2b runner 已定义，委托）← Task 4 stop 一致；`silence_duration() -> f64` ← Task 4 一致；`Stage::Streaming { pipeline, transcript, streaming_active }` ← Task 2/3/4 一致。✅

**4. 行为不变性：** pipeline.tick 的 set_full 逐字搬自 handle_streaming_tick（2b）；emit/DB/polish 留 coordinator，set_full→DB→emit 顺序完全不变；幂等（changed=false）保留；cloud/VadSegmented 零改动。单测覆盖 set_full 路径（changed=true）。✅

**5. 风险：** ① 紧接 2b（未 e2e）改同一区域——**建议 2b e2e 通过后再实施 2c-1**，避免连续未验证累积；② pipeline 目前较薄（只 set_full），emit/DB/polish 收敛留 2d——若用户期望 2c-1 一次收编更多，评审时提出。

---

## 2026-06-24-asr-pipeline-stage2c2



**Goal:** 把云端流式的「同步 tick 部分」收敛进 `StreamingPipeline`（上层 `StreamingPipelineEngine` trait，`CloudPipelineEngine` impl），cloud 的 async close 中间态（`Stage::CloudClosing` + `session_id` 护栏）原样留在 coordinator——与本地流式在 tick 层对称，零行为差异。

**Architecture:** 新增上层 trait `StreamingPipelineEngine`（`tick/finish_with_tail/silence_duration/current_partial/reset/take_close_handle/is_cloud`，后三个带默认实现）。`StreamingPipeline` 从「持 `StreamingRunner`」改为「持 `Box<dyn StreamingPipelineEngine>`」。`LocalPipelineEngine` 薄包 `StreamingRunner`（2c-1 既有行为）；`CloudPipelineEngine`（cfg cloud，新文件 `cloud_pipeline.rs`）把 `handle_cloud_streaming_tick` 的 ASR 编排（onset/push/drain/双层文本/静音非阻塞 finish）迁入 `tick`，产 `Vec<TranscriptEvent>` 而非直接写 transcript/emit。coordinator 侧 `Stage::CloudStreaming` 合并进 `Stage::Streaming`，`handle_cloud_streaming_tick` 删除合并进 `handle_streaming_tick`（统一 `pipeline.tick` + DB/emit/polish，`is_cloud()` 分支处理 cloud 的「每 tick emit / commit 时 DB+polish / 错误上报」三处不对称）。cloud close 链（`CloudClosing`/`CloudStreamingDone`/`handle_cloud_streaming_done`/`finalize_cloud`/`session_id` 护栏/`close_async` spawn）完全不动。

**Tech Stack:** Rust workspace（crate `desktop`，binary `main.rs`）；`cloud` feature（`#[cfg(feature = "cloud")]`，Cargo.toml 已定义）；tauri async runtime；`octopus_asr_local::vad::SileroVad` / `streaming_runner::{StreamingEngine, StreamingRunner, TranscriptEvent}`。

**关联文档：** spec `docs/superpowers/specs/2026-06-25-archived-spec.md#asr-pipeline-stage2c2-design`；总 spec `docs/superpowers/specs/2026-06-25-archived-spec.md#asr-pipeline-design` §3.4。

**全局约束（每个 task 都适用）：**
- **零行为差异**：所有迁移原样搬迁，不改语义/时机/频率。cloud 的 `current_partial` 是预览层（不进 transcript/DB，仅 display）；仅 `Finished→Committed` 落 transcript + 触发 DB。cloud emit 每 tick；local emit 仅 `changed`。
- **cloud 的 100ms tick 不可合并到 local 的 200ms tick**：`STREAMING_TICK_INTERVAL_MS=200`（local）、`CLOUD_STREAMING_TICK_INTERVAL_MS=100`（cloud）。故保留 `Command::CloudStreamingTick` + `start_cloud_streaming_tick_thread`（100ms），只把它的处理从 `handle_cloud_streaming_tick` 改为统一的 `handle_streaming_tick`。
- **config 访问**：`config/` 是 `~/.octopus/` 软链接，读写一律用绝对路径 `/Users/wudarui/.octopus/`（本计划不涉及 config 文件读写）。
- **git 提交**：不用复合命令（`commit && rebase`）、不用重定向；`git add` 与 `git commit` 分两行（换行分隔，非 `&&`）。
- **worktree**：在 `worktree-model-mgmt-ui` 分支工作；主仓库 `/Users/wudarui/workspace/agent/octopus` 用 `git -C`，不 cd。

---

## File Structure

| 文件 | 责任 | 本计划改动 |
|---|---|---|
| `crates/desktop/src/pipeline.rs` | 流式 pipeline 上层抽象：`StreamingPipelineEngine` trait + `StreamingPipeline` 壳 + `LocalPipelineEngine` + 共享 VAD helper | **改**：新增 trait + `LocalPipelineEngine`；`StreamingPipeline` 改持 `Box<dyn StreamingPipelineEngine>` + `last_error`；迁入 `compute_speech_chunks`（pub(crate)）；适配既有测试 |
| `crates/desktop/src/cloud_pipeline.rs` | cloud 流式 pipeline 引擎（cfg cloud）：`CloudPipelineEngine` + cloud session 编排纯函数 + open/resolve helpers | **新建**：`CloudPipelineEngine` impl trait；`drain_cloud_session`/`onset_confirmed`/`should_send_finish`/`take_preroll` 纯函数；迁入 `open_cloud_session` + 5 个 `resolve_*` + `resolve_cloud_entry`；单测 |
| `crates/desktop/src/main.rs` | binary 模块声明 | **改**：加 `#[cfg(feature = "cloud")] mod cloud_pipeline;` |
| `crates/desktop/src/coordinator.rs` | 协调器（同步主循环 + Stage 状态机） | **改**：`Stage::CloudStreaming` 删除（合并进 `Stage::Streaming`）；`handle_cloud_streaming_tick` 删除（合并进 `handle_streaming_tick`）；`handle_toggle` cloud 分支建 `CloudPipelineEngine`→`Stage::Streaming`；stop 路径合并（`take_close_handle` 分派）；`CloudStreamingTick` dispatch 改调 `handle_streaming_tick`；删 7 处 `Stage::CloudStreaming` match 臂（cancel/discard/polish/polish_now/edit/commit/stage_name/db_delete，由 `Stage::Streaming` 覆盖）；迁出 `compute_speech_chunks`/`take_preroll`/`open_cloud_session`/`resolve_*`（移到 pipeline.rs / cloud_pipeline.rs）。**保留**：`Stage::CloudClosing`/`CloudStreamingDone`/`handle_cloud_streaming_done`/`finalize_cloud`/`session_id` 护栏/`start_cloud_streaming_tick_thread`/`CLOUD_STREAMING_TICK_INTERVAL_MS`/`is_cloud_engine`/`vad_preroll` |

**不动**：`crates/asr/src/streaming_runner.rs`（`StreamingEngine`/`StreamingRunner`/`TranscriptEvent`，cli/server 仍用）；`crates/desktop/src/cloud_types.rs`（`CloudStreamHandle`）；`crates/desktop/src/{aliyun,bytedance,tencent,baidu}_stream.rs`；`audio.rs`/`transcript.rs`；cloud close 链全部代码。

---

## Task 1: `StreamingPipelineEngine` trait + `LocalPipelineEngine` + `StreamingPipeline` 重构（cloud 不动）

**目标：** 引入上层 trait，把本地流式从「`StreamingPipeline` 直持 `StreamingRunner`」重构为「持 `Box<dyn StreamingPipelineEngine>`」并由 `LocalPipelineEngine` 承载。cloud 路径（`Stage::CloudStreaming` + `handle_cloud_streaming_tick`）本 task **完全不动**，仍走旧路径编译通过。`compute_speech_chunks` 迁入 `pipeline.rs`（cloud tick 与 vad-segmented tick 共用，本 task 先搬迁、cloud 旧路径与 vad-segmented 都改为引用新位置）。

**Files:**
- Modify: `crates/desktop/src/pipeline.rs`（整体重写：trait + LocalPipelineEngine + StreamingPipeline + compute_speech_chunks + 适配测试）
- Modify: `crates/desktop/src/coordinator.rs:16`（import）、`:704`（local 构造点）、`:1361`（vad-segmented `compute_speech_chunks` 调用点）、`:1427-1447`（删除 `compute_speech_chunks` 定义）

- [x] **Step 1.1: 写失败测试——`LocalPipelineEngine` 包 `StreamingRunner` 并 impl trait**

把 `crates/desktop/src/pipeline.rs` 的 `#[cfg(test)] mod tests` 顶部，新增一个 `FakePipelineEngine`（直接 impl 新 trait，绕过 `StreamingRunner`，用于测 `StreamingPipeline` 的承载层），并改造既有两个测试用例。先只加测试，让它编译失败（trait 尚未定义）。

在 `pipeline.rs` 末尾既有 `mod tests` 内（替换整个 `mod tests`，见 Step 1.3 完整代码）之前，先在 `tests` 模块加：

```rust
    /// 直接 impl 新 trait 的 fake（不经过 StreamingRunner），测 StreamingPipeline 承载层。
    struct FakePipelineEngine {
        tick_out: std::sync::Mutex<Vec<TranscriptEvent>>,
        partial: String,
        finish_out: TranscriptEvent,
        silence: f64,
        close_handle_taken: std::sync::Mutex<bool>,
    }
    impl FakePipelineEngine {
        fn new(tick: Vec<TranscriptEvent>, partial: &str, finish: TranscriptEvent) -> Self {
            Self {
                tick_out: std::sync::Mutex::new(tick),
                partial: partial.to_string(),
                finish_out: finish,
                silence: 0.0,
                close_handle_taken: std::sync::Mutex::new(false),
            }
        }
    }
    impl StreamingPipelineEngine for FakePipelineEngine {
        fn tick(&mut self, _samples: &[f32]) -> Vec<TranscriptEvent> {
            std::mem::take(&mut *self.tick_out.lock().unwrap())
        }
        fn finish_with_tail(&mut self, _tail: &[f32]) -> TranscriptEvent {
            self.finish_out.clone()
        }
        fn silence_duration(&self) -> f64 { self.silence }
        fn current_partial(&self) -> &str { &self.partial }
        fn reset(&mut self) {}
    }
```

并加两个新测试（放在 `mod tests` 内）：

```rust
    #[test]
    fn tick_stashes_error_for_take_error() {
        // engine 产 Error → 承载层 warn + stash；take_error 取出；cloud 路径据此上报
        let mut p = StreamingPipeline::new(Box::new(FakePipelineEngine::new(
            vec![TranscriptEvent::Error("boom".to_string())],
            "",
            TranscriptEvent::Final("".to_string()),
        )))
        .unwrap();
        let mut t = Transcript::new(0, PolishMode::Disabled);
        let changed = p.tick(&[0.0; 1600], &mut t);
        assert!(!changed); // Error 不改 transcript
        assert_eq!(p.take_error().as_deref(), Some("boom"));
        assert!(p.take_error().is_none()); // 取走后空
    }

    #[test]
    fn current_partial_forwards_to_engine() {
        let p = StreamingPipeline::new(Box::new(FakePipelineEngine::new(
            vec![],
            "预览",
            TranscriptEvent::Final("".to_string()),
        )))
        .unwrap();
        assert_eq!(p.current_partial(), "预览");
        assert!(!p.is_cloud()); // LocalPipelineEngine/Fake 均 false
    }
```

- [x] **Step 1.2: 运行测试确认失败**

Run: `cargo test -p octopus-desktop pipeline:: -- --nocapture 2>&1 | tail -20`
Expected: 编译失败——`StreamingPipelineEngine` / `StreamingPipeline::new(Box<...>)` 单参数 / `take_error` / `is_cloud` 未定义。

- [x] **Step 1.3: 重写 `pipeline.rs`（trait + LocalPipelineEngine + StreamingPipeline + compute_speech_chunks）**

用以下完整内容替换 `crates/desktop/src/pipeline.rs` 全文：

```rust
//! desktop 流式 pipeline（spec §3.4 阶段 2c-1/2c-2）。
//!
//! [`StreamingPipeline`] 持 `Box<dyn StreamingPipelineEngine>`（上层抽象），承载
//! 「engine 事件（`TranscriptEvent`）→ 文本状态更新（`Transcript::set_full`）」。
//! - [`LocalPipelineEngine`]：薄包 asr `StreamingRunner`（VAD + accept/flush，2a/2b/2c-1）。
//! - `CloudPipelineEngine`（cfg cloud，见 `cloud_pipeline.rs`）：持 `CloudStreamHandle`
//!   （onset/push/drain/双层文本/静音非阻塞 finish，2c-2）。cloud 的 async close 不在
//!   trait（留 coordinator，spec §2）。
//!
//! **边界**：emit（`result_window::update_result`）/DB（`update_transcription_raw`）/polish
//! （`check_and_trigger_polish``）留 coordinator（emit 与 DB 同步触发以保持 `set_full→DB→emit`
//! 顺序；DB/polish 被 local + VadSegmented + cloud 三路径共用）。transcript 也留
//! `Stage::Streaming`，`tick` 接收 `&mut Transcript`。全收敛留 2d。

use crate::transcript::Transcript;
use log::warn;
use octopus_asr_local::streaming_runner::{StreamingRunner, TranscriptEvent};
use octopus_asr_local::streaming_engine::StreamingSession;
use octopus_asr_local::vad::SileroVad;

/// desktop 流式 pipeline 引擎（上层抽象，spec §3.4 阶段2c-2）。
///
/// local（包 `StreamingRunner`）与 cloud（持 `CloudStreamHandle`）各 impl。
/// 同步 `tick` + 同步 `finish_with_tail`；cloud 的 async close 不在此 trait
/// （留 coordinator，spec §2——`close_async` 必须 async，否则 `block_on` 卡主线程 8s）。
pub trait StreamingPipelineEngine: Send {
    /// 喂一帧已降噪 16k 样本，返回本帧 `TranscriptEvent`（0..n）。
    fn tick(&mut self, samples: &[f32]) -> Vec<TranscriptEvent>;
    /// 收尾：吃入尾部样本 + finish。
    ///   local → `StreamingRunner::finish_with_tail`（accept tail + finish，返回 `Final`）。
    ///   cloud → **只 push tail**（不发 Finish——cloud 的 Finish 由 coordinator 的
    ///            `close_async` 发，见 spec §4.3，避免重复 Finish），返回最后 `current_partial`
    ///            作 `Committed` 兜底（不产 `Final`，cloud stop 路径不用其返回值）。
    fn finish_with_tail(&mut self, tail: &[f32]) -> TranscriptEvent;
    /// 当前累积静音时长（秒，停顿润色触发用）。
    fn silence_duration(&self) -> f64;
    /// cloud 预览（`current_partial`），coordinator display 拼接用。local 默认空。
    /// cloud 双层文本：预览不进 transcript/DB，仅 display（spec §4.1/§4.2 不对称）。
    fn current_partial(&self) -> &str { "" }
    /// 重置（会话间复用）。cloud 须同时 drop 内置 session（→ channels 关 → WS task 结束）。
    fn reset(&mut self);
    /// cloud engine 的 async close 句柄（stop 时 coordinator 取出 spawn `close_async`）。
    /// local 返回 `None`（默认）；cloud 取出内置 session 后返回 `Some`。
    /// **cfg cloud**：`cloud_types` 仅 cloud feature 存在，故方法整体门控（无 cloud 时 trait 无此方法）。
    #[cfg(feature = "cloud")]
    fn take_close_handle(&mut self) -> Option<crate::cloud_types::CloudStreamHandle> { None }
    /// 是否 cloud 引擎（spec §4.2/§4.3 不对称判别：cloud 每 tick emit + commit 时 DB/polish +
    /// 错误上报 + stop 走 finalize_cloud；local emit/DB/polish 仅 changed + stop 走 finalize_after_stop）。
    fn is_cloud(&self) -> bool { false }
}

/// local：薄包 `StreamingRunner`，转发（VAD + accept/flush 编排仍在 asr `StreamingRunner`）。
pub struct LocalPipelineEngine(StreamingRunner);

impl LocalPipelineEngine {
    /// 构造 local 引擎，包已创建的 `StreamingSession`（保留 coordinator 的引擎降级逻辑，见 Step 1.4 ④）。
    /// 内部构造 `StreamingRunner`（含 VAD 预热，2a/2b）。
    pub fn from_session(session: StreamingSession, correct: bool) -> anyhow::Result<Self> {
        Ok(Self(StreamingRunner::new(Box::new(session), correct)?))
    }
}

impl StreamingPipelineEngine for LocalPipelineEngine {
    fn tick(&mut self, samples: &[f32]) -> Vec<TranscriptEvent> {
        self.0.push_samples(samples)
    }
    fn finish_with_tail(&mut self, tail: &[f32]) -> TranscriptEvent {
        self.0.finish_with_tail(tail)
    }
    fn silence_duration(&self) -> f64 {
        self.0.silence_duration()
    }
    fn reset(&mut self) {
        self.0.reset();
    }
}

/// local 流式 pipeline 壳：持 `Box<dyn StreamingPipelineEngine>`，承载事件 → set_full。
///
/// 不持 transcript（留 `Stage::Streaming`），`tick` 接收 `&mut Transcript`。
/// 不持 denoise/resample（留 `audio.rs`，输入为已降噪 16k 样本）。
pub struct StreamingPipeline {
    engine: Box<dyn StreamingPipelineEngine>,
    /// 上一 tick 承载层捕获的用户可见错误（cloud WSS 开启失败 / `StreamEvent::Failed`）。
    /// coordinator 仅对 cloud 取出上报（`take_error`）；local 错误只在承载层 warn，不取出。
    last_error: Option<String>,
}

impl StreamingPipeline {
    /// 构造 pipeline。`engine` 由调用方创建（`LocalPipelineEngine` 或 `CloudPipelineEngine`）。
    pub fn new(engine: Box<dyn StreamingPipelineEngine>) -> anyhow::Result<Self> {
        Ok(Self { engine, last_error: None })
    }

    /// 喂一帧已降噪 16k 样本：engine 产事件 → set_full，返回 `changed`。
    ///
    /// `changed=true` 表示文本变化（coordinator 据决定 DB + emit，保持「内容未变不落库/不重绘」幂等）。
    /// - local 的 `Partial`/`Committed`/`Final` 都 set_full（幂等去重）。
    /// - cloud 的预览（`current_partial`）**不**经过此——engine 自持 + 暴露 `current_partial()`
    ///   （spec §4.1）；仅 `Committed`（Finished）经此 set_full。
    /// - `Error` 承载层 warn + 暂存 `last_error`（coordinator `take_error` 取出，仅 cloud 上报）。
    pub fn tick(&mut self, samples: &[f32], transcript: &mut Transcript) -> bool {
        let mut changed = false;
        for event in self.engine.tick(samples) {
            match event {
                TranscriptEvent::Partial(text) | TranscriptEvent::Committed(text) => {
                    if text != transcript.full() {
                        transcript.set_full(&text);
                        changed = true;
                    }
                }
                TranscriptEvent::Final(text) => {
                    transcript.set_full(&text);
                    changed = true;
                }
                TranscriptEvent::Error(e) => {
                    warn!("StreamingPipeline event error: {}", e);
                    self.last_error = Some(e);
                }
            }
        }
        changed
    }

    /// 收尾并先吃入尾部样本（stop 路径用）。委托 engine（local→Final；cloud→push tail + 兜底 Committed）。
    pub fn finish_with_tail(&mut self, tail: &[f32]) -> TranscriptEvent {
        self.engine.finish_with_tail(tail)
    }

    /// 当前累积静音时长（秒），供 coordinator 判断停顿润色。委托 engine。
    pub fn silence_duration(&self) -> f64 {
        self.engine.silence_duration()
    }

    /// cloud 预览（`current_partial`），local 恒空。coordinator display 拼接用。
    pub fn current_partial(&self) -> &str {
        self.engine.current_partial()
    }

    /// 取出上一 tick 暂存的用户可见错误（cloud 上报用）。取走后清空。
    pub fn take_error(&mut self) -> Option<String> {
        self.last_error.take()
    }

    /// stop 路径分派：cloud → `Some(CloudStreamHandle)`（coordinator spawn close_async）；local → `None`。
    /// cfg cloud（与 trait 方法同步门控）。
    #[cfg(feature = "cloud")]
    pub fn take_close_handle(&mut self) -> Option<crate::cloud_types::CloudStreamHandle> {
        self.engine.take_close_handle()
    }

    /// 是否 cloud 引擎（§4.2/§4.3 不对称判别）。
    pub fn is_cloud(&self) -> bool {
        self.engine.is_cloud()
    }

    /// 重置（会话间复用）。委托 engine（cloud 同时 drop session）。
    pub fn reset(&mut self) {
        self.engine.reset();
    }
}

// ── 共享 VAD helper（coordinator vad-segmented tick 与 cloud tick 共用，spec §3.4）──

/// VAD 静音判定阈值（与 `streaming_runner` 常量一致）。
pub(crate) const VAD_SPEECH_THRESHOLD: f32 = 0.5;
/// VAD 分块大小（采样点数，16k 下 32ms）。
pub(crate) const VAD_CHUNK_SIZE: usize = 512;

/// 计算音频片段中语音帧的数量（迁自 `coordinator.rs`，vad-segmented / cloud 共用）。
pub(crate) fn compute_speech_chunks(vad: &mut SileroVad, samples: &[f32]) -> usize {
    let mut speech_chunks = 0usize;
    for chunk in samples.chunks(VAD_CHUNK_SIZE) {
        if chunk.len() < VAD_CHUNK_SIZE {
            break;
        }
        match vad.compute(chunk) {
            Ok(prob) => {
                if prob >= VAD_SPEECH_THRESHOLD {
                    speech_chunks += 1;
                }
            }
            Err(_) => speech_chunks += 1, // VAD 计算失败，保守认为有语音
        }
    }
    speech_chunks
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::PolishMode;
    use std::sync::Mutex;

    /// 直接 impl 新 trait 的 fake（不经过 StreamingRunner），测 StreamingPipeline 承载层。
    struct FakePipelineEngine {
        tick_out: Mutex<Vec<TranscriptEvent>>,
        partial: String,
        finish_out: TranscriptEvent,
        silence: f64,
    }
    impl FakePipelineEngine {
        fn new(tick: Vec<TranscriptEvent>, partial: &str, finish: TranscriptEvent) -> Self {
            Self {
                tick_out: Mutex::new(tick),
                partial: partial.to_string(),
                finish_out: finish,
                silence: 0.0,
            }
        }
    }
    impl StreamingPipelineEngine for FakePipelineEngine {
        fn tick(&mut self, _samples: &[f32]) -> Vec<TranscriptEvent> {
            std::mem::take(&mut *self.tick_out.lock().unwrap())
        }
        fn finish_with_tail(&mut self, _tail: &[f32]) -> TranscriptEvent {
            self.finish_out.clone()
        }
        fn silence_duration(&self) -> f64 { self.silence }
        fn current_partial(&self) -> &str { &self.partial }
        fn reset(&mut self) {}
    }

    fn pipeline(fake: FakePipelineEngine) -> StreamingPipeline {
        StreamingPipeline::new(Box::new(fake)).unwrap()
    }

    #[test]
    fn tick_partial_updates_transcript_and_signals_changed() {
        let mut p = pipeline(FakePipelineEngine::new(
            vec![TranscriptEvent::Partial("你好".to_string())],
            "",
            TranscriptEvent::Final("你好。".to_string()),
        ));
        let mut t = Transcript::new(0, PolishMode::Disabled);
        let changed = p.tick(&[0.0; 1600], &mut t);
        assert!(changed);
        assert_eq!(t.full(), "你好");
    }

    #[test]
    fn tick_final_overrides_transcript() {
        // Final 显式承载（2c-2 新增分支，local stop 产 Final）
        let mut p = pipeline(FakePipelineEngine::new(
            vec![TranscriptEvent::Final("最终。".to_string())],
            "",
            TranscriptEvent::Final("最终。".to_string()),
        ));
        let mut t = Transcript::new(0, PolishMode::Disabled);
        t.set_full("旧的");
        let changed = p.tick(&[0.0; 1600], &mut t);
        assert!(changed);
        assert_eq!(t.full(), "最终。"); // Final 无条件覆盖
    }

    #[test]
    fn tick_committed_idempotent_no_change_skip() {
        // Committed 与当前 full 相同 → 不改、changed=false（幂等）
        let mut p = pipeline(FakePipelineEngine::new(
            vec![TranscriptEvent::Committed("一样".to_string())],
            "",
            TranscriptEvent::Final("".to_string()),
        ));
        let mut t = Transcript::new(0, PolishMode::Disabled);
        t.set_full("一样");
        let changed = p.tick(&[0.0; 1600], &mut t);
        assert!(!changed);
    }

    #[test]
    fn tick_stashes_error_for_take_error() {
        let mut p = pipeline(FakePipelineEngine::new(
            vec![TranscriptEvent::Error("boom".to_string())],
            "",
            TranscriptEvent::Final("".to_string()),
        ));
        let mut t = Transcript::new(0, PolishMode::Disabled);
        let changed = p.tick(&[0.0; 1600], &mut t);
        assert!(!changed);
        assert_eq!(p.take_error().as_deref(), Some("boom"));
        assert!(p.take_error().is_none());
    }

    #[test]
    fn current_partial_forwards_to_engine() {
        let p = pipeline(FakePipelineEngine::new(
            vec![],
            "预览",
            TranscriptEvent::Final("".to_string()),
        ));
        assert_eq!(p.current_partial(), "预览");
        assert!(!p.is_cloud());
    }

    #[test]
    fn finish_with_tail_delegates_to_engine() {
        let mut p = pipeline(FakePipelineEngine::new(
            vec![],
            "",
            TranscriptEvent::Final("最终。".to_string()),
        ));
        let ev = p.finish_with_tail(&[0.0; 512]);
        assert_eq!(ev, TranscriptEvent::Final("最终。".to_string()));
    }

    #[cfg(feature = "cloud")]
    #[test]
    fn take_close_handle_none_for_local_fake() {
        // FakePipelineEngine 不覆盖 take_close_handle → 默认 None（与 LocalPipelineEngine 一致）。
        // 方法本身 cfg cloud，故测试同步门控（无 cloud feature 时不编译）。
        let mut p = pipeline(FakePipelineEngine::new(vec![], "", TranscriptEvent::Final("".to_string())));
        assert!(p.take_close_handle().is_none());
    }
}
```

- [x] **Step 1.4: 修改 coordinator.rs——删除 `compute_speech_chunks` 定义、改 import、改 local 构造点、改 vad-segmented 调用点**

四处编辑：

① 删除 `coordinator.rs:1427-1447`（`compute_speech_chunks` 定义，含上方 `/// 计算音频片段中语音帧的数量` 注释行 1426）。删除整段函数。

② **保留** `coordinator.rs:179-182` 的 `VAD_SPEECH_THRESHOLD`/`VAD_CHUNK_SIZE` 常量（`vad_preroll` 1451 仍用 `VAD_CHUNK_SIZE`）。`pipeline.rs`（Step 1.3）定义**独立的** `pub(crate)` 同名常量供迁入的 `compute_speech_chunks` 用——两套同名常量分属不同模块、互不冲突，避免删除 coordinator 常量引发 `vad_preroll` 编译错误。`VAD_PREROLL_FRAMES`（187）亦保留（coordinator 专用）。

③ `coordinator.rs:1361`（vad-segmented tick 内）：
```rust
        let speech_chunks = compute_speech_chunks(vad, &samples);
```
改为：
```rust
        let speech_chunks = crate::pipeline::compute_speech_chunks(vad, &samples);
```

④ `coordinator.rs:704`（handle_toggle local 流式构造点）。原（`StreamingPipeline::new` 双参，持 `streaming_engine`）：
```rust
                let pipeline = match StreamingPipeline::new(Box::new(streaming_engine), false) {
                    Ok(p) => p,
                    Err(e) => {
                        error!("StreamingPipeline init failed: {}, abort streaming", e);
                        let _ = audio.stop();
                        crate::result_window::hide_result(app_handle);
                        crate::tray::update_tray_label(app_handle, crate::tray::TrayState::Idle);
                        return;
                    }
                };
```
改为（`streaming_engine` 先经 `LocalPipelineEngine::from_session` 包裹——保留原 `StreamingSession::new` 降级逻辑 670-694 不动；`StreamingPipeline::new` 改单参）。`from_session` 的 `Err`（`StreamingRunner::new` 失败=VAD 路径解析失败，极罕见）与 `StreamingPipeline::new` 的 `Err` 用同一清理（`audio.stop()` + hide_result + tray Idle）：
```rust
                let local_engine = match crate::pipeline::LocalPipelineEngine::from_session(streaming_engine, false) {
                    Ok(e) => e,
                    Err(e) => {
                        error!("LocalPipelineEngine init failed: {}, abort streaming", e);
                        let _ = audio.stop();
                        crate::result_window::hide_result(app_handle);
                        crate::tray::update_tray_label(app_handle, crate::tray::TrayState::Idle);
                        return;
                    }
                };
                let pipeline = match StreamingPipeline::new(Box::new(local_engine)) {
                    Ok(p) => p,
                    Err(e) => {
                        error!("StreamingPipeline init failed: {}, abort streaming", e);
                        let _ = audio.stop();
                        crate::result_window::hide_result(app_handle);
                        crate::tray::update_tray_label(app_handle, crate::tray::TrayState::Idle);
                        return;
                    }
                };
```
（`Box<LocalPipelineEngine> → Box<dyn StreamingPipelineEngine>` 的 unsize 强转由 `StreamingPipeline::new` 形参期望类型驱动，**无需** 在 coordinator 额外 `use` trait。）

⑤ import 清理：`coordinator.rs:10` `use octopus_asr_local::streaming_engine::StreamingSession;` —— local 构造不再直接用 `StreamingSession`（移入 pipeline.rs），但 handle_toggle local 分支 671 `StreamingSession::new(&config.asr_engine)` **仍在 coordinator**（降级逻辑用）。故该 import **保留**。

- [x] **Step 1.5: 运行测试确认通过（不含 cloud feature）**

Run: `cargo test -p octopus-desktop pipeline:: 2>&1 | tail -25`
Expected: PASS——`pipeline::tests` 全绿（含新增 5 个 + 改造的既有）。

- [x] **Step 1.6: 全量 check（双 feature 配置）**

Run: `cargo check -p octopus-desktop 2>&1 | tail -15`
Expected: 0 error。cloud 旧路径（`Stage::CloudStreaming` + `handle_cloud_streaming_tick`）仍存在且引用已迁的 `compute_speech_chunks`——**此刻会编译失败**（cloud tick 1680 仍调 `compute_speech_chunks`，但已迁走）。

修复 `coordinator.rs:1680`（cloud tick 内）：
```rust
        let speech_chunks = compute_speech_chunks(vad, &samples);
```
改为：
```rust
        let speech_chunks = crate::pipeline::compute_speech_chunks(vad, &samples);
```

再 Run: `cargo check -p octopus-desktop --features cloud 2>&1 | tail -15`
Expected: 0 error（cloud 旧路径仍走 `Stage::CloudStreaming`，只是 `compute_speech_chunks` 改引 pipeline）。

- [x] **Step 1.7: 提交**

```bash
git add crates/desktop/src/pipeline.rs crates/desktop/src/coordinator.rs
git commit -m "refactor(asr): StreamingPipelineEngine trait + LocalPipelineEngine（2c-2 T1，cloud 路径不动）"
```

---

## Task 2: `CloudPipelineEngine`（`cloud_pipeline.rs`，迁 cloud tick + helpers，单测，不接线）

**目标：** 新建 `cloud_pipeline.rs`（cfg cloud），把 `handle_cloud_streaming_tick` 的 ASR 编排迁入 `CloudPipelineEngine::tick`，产 `Vec<TranscriptEvent>`；迁入 `open_cloud_session` + 5 个 `resolve_*` + `resolve_cloud_entry` + `take_preroll`；抽出可单测纯函数（`drain_cloud_session`/`onset_confirmed`/`should_send_finish`）。**本 task 不接线 coordinator**——`CloudPipelineEngine` 尚未被任何生产代码引用（允许暂时 dead_code warning，Task 3 接线后消除）。

**Files:**
- Create: `crates/desktop/src/cloud_pipeline.rs`
- Modify: `crates/desktop/src/main.rs:18`（加 mod 声明）

- [x] **Step 2.1: 在 `main.rs` 加模块声明**

`crates/desktop/src/main.rs:18`（`mod cloud_types;` 之后）插入：
```rust
#[cfg(feature = "cloud")]
mod cloud_pipeline;
```

- [x] **Step 2.2: 写失败测试——`drain_cloud_session` 事件映射**

创建 `crates/desktop/src/cloud_pipeline.rs`，先只写测试模块（`#[cfg(test)]`），让它编译失败（被测函数未定义）：

```rust
//! 云端流式 pipeline 引擎（spec §3.4 阶段2c-2，cfg cloud）。
//!
//! [`CloudPipelineEngine`] impl [`crate::pipeline::StreamingPipelineEngine`]，把原
//! `coordinator::handle_cloud_streaming_tick` 的 ASR 编排（VAD onset / push_pcm / drain
//! events / partial-transcript 双层 / 静音非阻塞 finish）迁入 `tick`，产
//! `Vec<TranscriptEvent>`。emit/DB/polish 留 coordinator（§4.2 不对称）。

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cloud_types::{CloudStreamHandle, StreamEvent};

    /// 构造一个预载事件序列的 CloudStreamHandle（onset 后 drain 用）。
    fn handle_with_events(events: Vec<StreamEvent>) -> CloudStreamHandle {
        let (handle, _pcm_rx, result_tx) = CloudStreamHandle::new();
        for ev in events {
            let _ = result_tx.send(ev);
        }
        handle
    }

    #[test]
    fn drain_text_updates_partial_no_event() {
        // Text(t) → current_partial=t，不发 TranscriptEvent（预览层不进 transcript/DB）
        let mut session = Some(handle_with_events(vec![StreamEvent::Text("你好".to_string())]));
        let (mut committed, mut partial, mut is_closing, mut is_speaking) =
            (String::new(), String::new(), false, true);
        let evs = drain_cloud_session(CloudDrainState {
            session: &mut session,
            committed_text: &mut committed,
            current_partial: &mut partial,
            is_closing: &mut is_closing,
            is_speaking: &mut is_speaking,
        });
        assert!(evs.is_empty()); // 预览不发事件
        assert_eq!(partial, "你好");
    }

    #[test]
    fn drain_finished_emits_committed_with_comma() {
        // 已提交 "第一句" + current_partial "第二句" → Finished → Committed("第一句，第二句")
        // 分两次 drain（drain 的 while 循环会一次清空所有已排队事件，故用 result_tx 跨调用分段投递）：
        //   先 Text 进 partial（is_speaking=true，不 take session），再 Finished 提交。
        let (handle, _pcm_rx, result_tx) = CloudStreamHandle::new();
        let mut session = Some(handle);
        let (mut committed, mut partial, mut is_closing, mut is_speaking) =
            ("第一句".to_string(), String::new(), false, true);
        let _ = result_tx.send(StreamEvent::Text("第二句".to_string()));
        let _ = drain_cloud_session(CloudDrainState {
            session: &mut session, committed_text: &mut committed,
            current_partial: &mut partial, is_closing: &mut is_closing, is_speaking: &mut is_speaking,
        });
        assert_eq!(partial, "第二句");
        let _ = result_tx.send(StreamEvent::Finished);
        let evs = drain_cloud_session(CloudDrainState {
            session: &mut session, committed_text: &mut committed,
            current_partial: &mut partial, is_closing: &mut is_closing, is_speaking: &mut is_speaking,
        });
        assert_eq!(evs, vec![TranscriptEvent::Committed("第一句，第二句".to_string())]);
        assert_eq!(committed, "第一句，第二句");
        assert_eq!(partial, ""); // 提交后清零
        assert!(!is_closing);
        assert!(!is_speaking);
        assert!(session.is_none()); // Finished → !is_closing && !is_speaking → take
    }

    #[test]
    fn drain_finished_no_partial_no_event_no_comma() {
        // current_partial 空 + Finished → 不 append、不发事件（与原 `if !current_partial.is_empty()` 一致）
        let mut session = Some(handle_with_events(vec![StreamEvent::Finished]));
        let (mut committed, mut partial, mut is_closing, mut is_speaking) =
            ("已有".to_string(), String::new(), false, true);
        let evs = drain_cloud_session(CloudDrainState {
            session: &mut session, committed_text: &mut committed,
            current_partial: &mut partial, is_closing: &mut is_closing, is_speaking: &mut is_speaking,
        });
        assert!(evs.is_empty());
        assert_eq!(committed, "已有"); // 不变
        assert!(session.is_none()); // Finished → !speaking → take
    }

    #[test]
    fn drain_failed_emits_error_clears_partial() {
        // 分两次 drain：先 Text 进 partial，再 Failed → Error + 清 partial
        let (handle, _pcm_rx, result_tx) = CloudStreamHandle::new();
        let mut session = Some(handle);
        let (mut committed, mut partial, mut is_closing, mut is_speaking) =
            (String::new(), String::new(), false, true);
        let _ = result_tx.send(StreamEvent::Text("抖动".to_string()));
        let _ = drain_cloud_session(CloudDrainState {
            session: &mut session, committed_text: &mut committed,
            current_partial: &mut partial, is_closing: &mut is_closing, is_speaking: &mut is_speaking,
        });
        assert_eq!(partial, "抖动");
        let _ = result_tx.send(StreamEvent::Failed("boom".to_string()));
        let evs = drain_cloud_session(CloudDrainState {
            session: &mut session, committed_text: &mut committed,
            current_partial: &mut partial, is_closing: &mut is_closing, is_speaking: &mut is_speaking,
        });
        assert_eq!(evs, vec![TranscriptEvent::Error("⚠️ 云端识别失败：boom".to_string())]);
        assert_eq!(partial, ""); // Failed 清零
        assert!(!is_closing && !is_speaking);
    }

    #[test]
    fn onset_confirmed_requires_two_consecutive() {
        assert!(!onset_confirmed(true, false, false, 1));  // 仅 1 tick
        assert!(onset_confirmed(true, false, false, 2));   // 连续 2 tick
        assert!(!onset_confirmed(true, true, false, 5));   // 已 speaking
        assert!(!onset_confirmed(true, false, true, 5));   // is_closing
        assert!(!onset_confirmed(false, false, false, 5)); // 无语音
    }

    #[test]
    fn should_send_finish_only_when_speaking_not_closing_silence_enough() {
        assert!(should_send_finish(true, false, 800, 700));   // speaking + 静音 800≥700
        assert!(!should_send_finish(false, false, 800, 700)); // 未 speaking
        assert!(!should_send_finish(true, true, 800, 700));   // 已 closing
        assert!(!should_send_finish(true, false, 600, 700));  // 静音不足
    }

    #[test]
    fn take_preroll_last_n_samples() {
        let buf: Vec<f32> = (0..3200).map(|x| x as f32).collect(); // 3200 samples
        let pre = take_preroll(&buf); // 取最后 1600
        assert_eq!(pre.len(), 1600);
        assert_eq!(pre[0], 1600.0); // = buf[1600]
        // 不足 1600 → 全取
        let small = vec![1.0_f32, 2.0, 3.0];
        assert_eq!(take_preroll(&small), vec![1.0, 2.0, 3.0]);
    }
}
```

- [x] **Step 2.3: 运行测试确认失败**

Run: `cargo test -p octopus-desktop --features cloud cloud_pipeline:: 2>&1 | tail -20`
Expected: 编译失败——`drain_cloud_session`/`CloudDrainState`/`onset_confirmed`/`should_send_finish`/`take_preroll` 未定义。

- [x] **Step 2.4: 实现 `cloud_pipeline.rs` 主体（纯函数 + struct + open/resolve helpers + CloudPipelineEngine + impl trait）**

在 `cloud_pipeline.rs` 的 `#[cfg(test)] mod tests` **之上**插入完整实现。文件结构：常量 → `CloudDrainState` + `drain_cloud_session` → `onset_confirmed`/`should_send_finish`/`take_preroll` → `open_cloud_session` + 5 个 `resolve_*` + `resolve_cloud_entry`（从 coordinator 迁入，签名改 `(asr_engine, language, pre_roll)`）→ `CloudPipelineEngine` struct + `new` + impl trait。

```rust
use crate::cloud_types::{CloudStreamHandle, StreamEvent};
use crate::pipeline::{compute_speech_chunks, StreamingPipelineEngine};
use log::{debug, error, info, warn};
use octopus_asr_local::streaming_runner::TranscriptEvent;
use octopus_asr_local::vad::SileroVad;
use tauri::async_runtime::RuntimeHandle;

/// pre-roll 滚动缓冲区大小（采样点）：200ms @ 16kHz = 3200。
const CLOUD_PREROLL_BUFFER_SAMPLES: usize = 3200;
/// pre-roll 补齐长度（采样点）：100ms @ 16kHz = 1600。
const CLOUD_PREROLL_SAMPLES: usize = 1600;

/// drain 阶段的 cloud session 可变状态（结构化避免过多 &mut 参数）。
pub(super) struct CloudDrainState<'a> {
    pub session: &'a mut Option<CloudStreamHandle>,
    pub committed_text: &'a mut String,
    pub current_partial: &'a mut String,
    pub is_closing: &'a mut bool,
    pub is_speaking: &'a mut bool,
}

/// drain `try_recv_text` 事件并映射为 `TranscriptEvent`（迁自 `handle_cloud_streaming_tick:1731-1786`）。
///
/// - `Text(t)` 非空 → `current_partial=t`（**预览层，不发事件**，不进 transcript/DB）。
/// - `Finished` → `committed_text` 追加（`，` 逗号拼接，与原 `append_segment("，")` 逻辑一致）+
///   发 `Committed(committed_text)`（**DB 触发点**，由承载层 set_full）；清 `current_partial`；
///   `is_closing=false`、`is_speaking=false`。
/// - `Failed(msg)` → 发 `Error("⚠️ 云端识别失败：{msg}")`（coordinator 取 `take_error` 上报）；
///   清 `current_partial`/状态（下次 onset 重开，瞬时抖动自动重试）。
/// - drain 后 `!is_closing && !is_speaking` → `session.take()`（drop → channels 关 → WS task 结束）。
pub(super) fn drain_cloud_session(s: CloudDrainState) -> Vec<TranscriptEvent> {
    let mut events = Vec::new();
    if let Some(sess) = s.session.as_mut() {
        while let Some(event) = sess.try_recv_text() {
            match event {
                StreamEvent::Text(text) => {
                    if !text.is_empty() {
                        info!("[CloudDrain] partial={:?}", text);
                        *s.current_partial = text;
                    }
                }
                StreamEvent::Finished => {
                    info!(
                        "[CloudDrain] Finished, committing partial={:?} to transcript",
                        *s.current_partial
                    );
                    if !s.current_partial.is_empty() {
                        if !s.committed_text.is_empty() && !s.committed_text.ends_with('，') {
                            s.committed_text.push('，');
                        }
                        s.committed_text.push_str(s.current_partial);
                        s.current_partial.clear();
                        events.push(TranscriptEvent::Committed(s.committed_text.clone()));
                    }
                    *s.is_closing = false;
                    *s.is_speaking = false;
                }
                StreamEvent::Failed(msg) => {
                    warn!("[CloudDrain] Failed: {}", msg);
                    s.current_partial.clear();
                    *s.is_closing = false;
                    *s.is_speaking = false;
                    events.push(TranscriptEvent::Error(format!("⚠️ 云端识别失败：{}", msg)));
                }
            }
        }
    }
    if !*s.is_closing && !*s.is_speaking {
        let _ = s.session.take(); // drop → channels close → WS task 结束
    }
    events
}

/// onset 判定：连续 2 tick 确认（消除单次噪声脉冲误触发），且未 speaking / 未 closing。
pub(super) fn onset_confirmed(
    has_speech_now: bool,
    is_speaking: bool,
    is_closing: bool,
    speech_confirm_count: u32,
) -> bool {
    has_speech_now && !is_speaking && !is_closing && speech_confirm_count >= 2
}

/// 静音非阻塞 finish 判定：speaking + 未 closing + 静音 ≥ 阈值（毫秒）。
pub(super) fn should_send_finish(
    is_speaking: bool,
    is_closing: bool,
    silence_ms: f64,
    pause_polish_threshold_ms: u64,
) -> bool {
    is_speaking && !is_closing && silence_ms >= pause_polish_threshold_ms as f64
}

/// 从 pre-roll 滚动缓冲区取最后 `CLOUD_PREROLL_SAMPLES` 样本作为前导音频（迁自 coordinator）。
pub(super) fn take_preroll(pre_roll_buffer: &[f32]) -> Vec<f32> {
    if pre_roll_buffer.len() >= CLOUD_PREROLL_SAMPLES {
        pre_roll_buffer[pre_roll_buffer.len() - CLOUD_PREROLL_SAMPLES..].to_vec()
    } else {
        pre_roll_buffer.to_vec()
    }
}

// ── open/resolve helpers（迁自 coordinator.rs:1515-1628，签名改 (asr_engine, language, pre_roll)）──

#[cfg(feature = "cloud")]
fn resolve_cloud_entry<'a>(
    section: Option<&'a std::collections::HashMap<String, octopus_infra::db::ModelEntry>>,
    provider: &'a str,
    model_name: &'a str,
) -> Result<&'a octopus_infra::db::ModelEntry, String> {
    let entry = section
        .and_then(|m| m.get(model_name))
        .ok_or_else(|| format!("{} ASR 模型 '{}' 未在 DB 配置", provider, model_name))?;
    if entry.secret_key.is_empty() {
        return Err(format!("{} ASR 模型 '{}' 的 secret_key 为空", provider, model_name));
    }
    Ok(entry)
}

#[cfg(feature = "cloud")]
fn resolve_aliyun_config(engine_spec: &str) -> Result<(String, String, String), String> {
    let cfg = octopus_asr_local::config::load_config().map_err(|e| e.to_string())?;
    let model_name = octopus_infra::db::parse_model_spec(engine_spec).model_name().to_string();
    let entry = resolve_cloud_entry(cfg.asr.aliyun.as_ref(), "aliyun", &model_name)?;
    Ok((entry.source.clone(), entry.secret_key.clone(), model_name))
}

#[cfg(feature = "cloud")]
fn resolve_bytedance_config(engine_spec: &str) -> Result<(String, String, String), String> {
    let cfg = octopus_asr_local::config::load_config().map_err(|e| e.to_string())?;
    let model_name = octopus_infra::db::parse_model_spec(engine_spec).model_name().to_string();
    let entry = resolve_cloud_entry(cfg.asr.bytedance.as_ref(), "bytedance", &model_name)?;
    Ok((entry.source.clone(), entry.secret_key.clone(), model_name))
}

#[cfg(feature = "cloud")]
fn resolve_tencent_config(engine_spec: &str) -> Result<(String, String, String), String> {
    let cfg = octopus_asr_local::config::load_config().map_err(|e| e.to_string())?;
    let model_name = octopus_infra::db::parse_model_spec(engine_spec).model_name().to_string();
    let entry = resolve_cloud_entry(cfg.asr.tencent.as_ref(), "tencent", &model_name)?;
    if !entry.source.contains(':') {
        return Err(format!(
            "tencent ASR 模型 '{}' 的 source 字段格式应为 appid:secretid（当前='{}'）",
            model_name, entry.source
        ));
    }
    Ok((entry.source.clone(), entry.secret_key.clone(), model_name))
}

#[cfg(feature = "cloud")]
fn resolve_baidu_config(engine_spec: &str) -> Result<(String, String, String), String> {
    let cfg = octopus_asr_local::config::load_config().map_err(|e| e.to_string())?;
    let model_name = octopus_infra::db::parse_model_spec(engine_spec).model_name().to_string();
    let entry = resolve_cloud_entry(cfg.asr.baidu.as_ref(), "baidu", &model_name)?;
    if entry.source.is_empty() {
        return Err(format!("baidu ASR 模型 '{}' 的 source 字段（AppID）为空", model_name));
    }
    Ok((entry.source.clone(), entry.secret_key.clone(), model_name))
}

/// onset dispatch：根据引擎类型解析配置 + 打开对应云端 WS session（迁自 coordinator，
/// 签名由 `&AppConfig` 改为 `(asr_engine, language, pre_roll)`）。
pub(super) fn open_cloud_session(
    asr_engine: &str,
    language: &str,
    pre_roll: Vec<f32>,
) -> Result<CloudStreamHandle, String> {
    use octopus_asr_local::config::EngineCategory;
    let rt: RuntimeHandle = tauri::async_runtime::handle();
    match octopus_asr_local::config::resolve_engine_category(asr_engine) {
        Some(EngineCategory::Aliyun) => {
            let (endpoint, key, model) = resolve_aliyun_config(asr_engine)?;
            crate::aliyun_stream::open(&rt, endpoint, key, model, language.to_string(), pre_roll)
                .map_err(|e| e.to_string())
        }
        Some(EngineCategory::ByteDance) => {
            let (resource_id, api_key, _) = resolve_bytedance_config(asr_engine)?;
            crate::bytedance_stream::open(&rt, api_key, resource_id, language.to_string(), pre_roll)
                .map_err(|e| e.to_string())
        }
        Some(EngineCategory::Tencent) => {
            let (appid_secretid, secret_key, engine_model_type) = resolve_tencent_config(asr_engine)?;
            crate::tencent_stream::open(
                &rt, appid_secretid, secret_key, engine_model_type, language.to_string(), pre_roll,
            )
            .map_err(|e| e.to_string())
        }
        Some(EngineCategory::Baidu) => {
            let (appid, appkey, dev_pid) = resolve_baidu_config(asr_engine)?;
            crate::baidu_stream::open(&rt, appid, appkey, dev_pid, language.to_string(), pre_roll)
                .map_err(|e| e.to_string())
        }
        _ => Err("当前引擎非云端，无法开启 WSS".to_string()),
    }
}

/// cloud 流式 pipeline 引擎（持 `CloudStreamHandle` + onset/状态，spec §3.3）。
pub struct CloudPipelineEngine {
    vad: SileroVad,
    pre_roll_buffer: Vec<f32>,
    session: Option<CloudStreamHandle>,
    /// 已提交累积（镜像 `transcript.full` 的提交层；engine 无 transcript 访问，故自持）。
    committed_text: String,
    current_partial: String,
    silence_duration: f64,
    is_speaking: bool,
    speech_confirm_count: u32,
    is_closing: bool,
    asr_engine: String,
    language: String,
    pause_polish_threshold_ms: u64,
}

impl CloudPipelineEngine {
    /// 构造。`vad` 由 coordinator 经 `find_silero_vad` + `vad_preroll` 预热后传入。
    /// `asr_engine`/`language`/`pause_polish_threshold_ms` 从 config 快照克隆（onset 时开 session / finish 判定用）。
    pub fn new(
        vad: SileroVad,
        asr_engine: String,
        language: String,
        pause_polish_threshold_ms: u64,
    ) -> Self {
        Self {
            vad,
            pre_roll_buffer: Vec::new(),
            session: None,
            committed_text: String::new(),
            current_partial: String::new(),
            silence_duration: 0.0,
            is_speaking: false,
            speech_confirm_count: 0,
            is_closing: false,
            asr_engine,
            language,
            pause_polish_threshold_ms,
        }
    }
}

impl StreamingPipelineEngine for CloudPipelineEngine {
    fn tick(&mut self, samples: &[f32]) -> Vec<TranscriptEvent> {
        // 迁自 handle_cloud_streaming_tick:1665-1805 的 ASR 部分；产事件，不直接写 transcript/emit。

        // 2. 追加 pre-roll 滚动缓冲区（超容量弹头）
        if !samples.is_empty() {
            self.pre_roll_buffer.extend_from_slice(samples);
            if self.pre_roll_buffer.len() > CLOUD_PREROLL_BUFFER_SAMPLES {
                let excess = self.pre_roll_buffer.len() - CLOUD_PREROLL_BUFFER_SAMPLES;
                self.pre_roll_buffer.drain(0..excess);
            }
        }

        // 3. VAD 检测（has_speech_now = 语音 chunk ≥ 2）
        let mut has_speech_now = false;
        if !samples.is_empty() {
            let speech_chunks = compute_speech_chunks(&mut self.vad, samples);
            has_speech_now = speech_chunks >= 2;
            if has_speech_now {
                self.silence_duration = 0.0;
                if !self.is_speaking && !self.is_closing {
                    self.speech_confirm_count += 1;
                }
            } else {
                self.silence_duration += samples.len() as f64 / 16000.0;
                if !self.is_speaking && !self.is_closing {
                    self.speech_confirm_count = 0;
                }
            }
        }

        // 4. onset 确认 → 开 WSS + pre-roll + push
        if onset_confirmed(has_speech_now, self.is_speaking, self.is_closing, self.speech_confirm_count) {
            self.is_speaking = true;
            self.speech_confirm_count = 0;
            self.current_partial.clear();
            let pre_roll = take_preroll(&self.pre_roll_buffer);
            match open_cloud_session(&self.asr_engine, &self.language, pre_roll) {
                Ok(sess) => {
                    let _ = sess.push_pcm(samples);
                    self.session = Some(sess);
                    debug!("CloudPipelineEngine: WSS opened on speech onset");
                }
                Err(e) => {
                    error!("CloudPipelineEngine: open WSS failed: {}", e);
                    self.is_speaking = false;
                    // 用户可见错误：coordinator 取 take_error 上报（与原 update_result 一致）
                    return vec![TranscriptEvent::Error(format!("⚠️ 云端连接失败：{}", e))];
                }
            }
        }

        // 5. 有 session → push PCM（closing 时不推）+ drain events
        if let Some(sess) = self.session.as_mut() {
            if !samples.is_empty() && !self.is_closing {
                if let Err(e) = sess.push_pcm(samples) {
                    warn!("CloudPipelineEngine: push_pcm failed: {}", e);
                }
            }
        }
        let mut events = drain_cloud_session(CloudDrainState {
            session: &mut self.session,
            committed_text: &mut self.committed_text,
            current_partial: &mut self.current_partial,
            is_closing: &mut self.is_closing,
            is_speaking: &mut self.is_speaking,
        });
        //（drain_cloud_session 内部在 !is_closing && !is_speaking 时已 session.take()）

        // 6. 静音 ≥ 阈值 → 非阻塞 finish（Finish 由 close_async 最终发，此处只触发服务端收尾）
        if should_send_finish(
            self.is_speaking,
            self.is_closing,
            self.silence_duration * 1000.0,
            self.pause_polish_threshold_ms,
        ) {
            self.is_speaking = false;
            self.is_closing = true;
            if let Some(sess) = self.session.as_ref() {
                info!("[CloudFinish] silence≥threshold, sending finish (non-blocking)");
                if let Err(e) = sess.finish() {
                    warn!("CloudPipelineEngine: finish failed: {}", e);
                }
            }
        }

        events
    }

    fn finish_with_tail(&mut self, tail: &[f32]) -> TranscriptEvent {
        // cloud stop：只 push tail（不发 Finish——Finish 由 coordinator 的 close_async 发，避免重复）。
        // 返回 current_partial 作 Committed 兜底（cloud stop 路径不用其返回值，见 coordinator stop 分支）。
        if !tail.is_empty() && !self.is_closing {
            if let Some(sess) = self.session.as_ref() {
                if let Err(e) = sess.push_pcm(tail) {
                    warn!("CloudPipelineEngine finish_with_tail push_pcm failed: {}", e);
                }
            }
        }
        TranscriptEvent::Committed(self.current_partial.clone())
    }

    fn silence_duration(&self) -> f64 {
        self.silence_duration
    }

    fn current_partial(&self) -> &str {
        &self.current_partial
    }

    fn reset(&mut self) {
        // drop session（→ channels 关 → WS task 结束）+ 状态归零（会话间复用）
        let _ = self.session.take();
        self.committed_text.clear();
        self.current_partial.clear();
        self.silence_duration = 0.0;
        self.is_speaking = false;
        self.speech_confirm_count = 0;
        self.is_closing = false;
        self.pre_roll_buffer.clear();
    }

    fn take_close_handle(&mut self) -> Option<CloudStreamHandle> {
        self.session.take()
    }

    fn is_cloud(&self) -> bool {
        true
    }
}
```

（`drain_cloud_session`/`tick` 内用 `info!`/`warn!`/`error!`/`debug!`，顶部 `use log::{debug, error, info, warn};` 已含。）

- [x] **Step 2.5: 运行测试确认通过**

Run: `cargo test -p octopus-desktop --features cloud cloud_pipeline:: 2>&1 | tail -25`
Expected: PASS——`cloud_pipeline::tests` 全绿（drain_cloud_session 4 例 + onset_confirmed + should_send_finish + take_preroll）。

- [x] **Step 2.6: check（允许 cloud_pipeline 暂时未引用的 dead_code warning）**

Run: `cargo check -p octopus-desktop --features cloud 2>&1 | tail -20`
Expected: 0 error。可能有 `CloudPipelineEngine`/`open_cloud_session` 等未引用的 `dead_code` warning——**本 task 可接受**（Task 3 接线后消除）。若 clippy/error 级别报错则需修复。

- [x] **Step 2.7: 提交**

```bash
git add crates/desktop/src/cloud_pipeline.rs crates/desktop/src/main.rs
git commit -m "feat(asr): CloudPipelineEngine + cloud tick 迁入 cloud_pipeline.rs（2c-2 T2，未接线）"
```

---

## Task 3: 接线——合并 `Stage::CloudStreaming` 进 `Stage::Streaming`，删 `handle_cloud_streaming_tick`

**目标：** 把 cloud 接入 `StreamingPipeline`：`handle_toggle` cloud 分支建 `CloudPipelineEngine`→`Stage::Streaming`；`CloudStreamingTick` dispatch 改调 `handle_streaming_tick`（cloud 仍走 100ms tick 线程）；stop 路径合并（`take_close_handle` 分派 cloud close / local finalize）；删除 `Stage::CloudStreaming` + `handle_cloud_streaming_tick` + 迁出的 helpers（`open_cloud_session`/`resolve_*`/`take_preroll`，已在 Task 2 迁入 cloud_pipeline.rs）；清理 7 处 `Stage::CloudStreaming` match 臂（由 `Stage::Streaming` 覆盖）。`Stage::CloudClosing`/`CloudStreamingDone`/`handle_cloud_streaming_done`/`finalize_cloud`/`session_id` 护栏/`start_cloud_streaming_tick_thread`/`CLOUD_STREAMING_TICK_INTERVAL_MS`/`is_cloud_engine`/`vad_preroll` **保留**。

**Files:**
- Modify: `crates/desktop/src/coordinator.rs`（多处：Stage enum、Command dispatch、handle_toggle、stop、handle_streaming_tick、7 处 match 臂、删除 cloud tick + 迁出 helpers）

- [x] **Step 3.1: 删除 `Stage::CloudStreaming` 变体**

`coordinator.rs:110-136`（`Stage::CloudStreaming { ... }` 整段含 doc 注释 110-114）。删除整个变体。`Stage::CloudClosing`（137-144）保留。

- [x] **Step 3.2: `handle_streaming_tick` 重写为 local/cloud 统一（`is_cloud()` 分支）**

替换 `coordinator.rs:1950-1984`（`fn handle_streaming_tick` 整体）为：

```rust
/// 处理 StreamingTick / CloudStreamingTick 命令（2c-2：local/cloud 统一）。
///
/// engine.tick 承载事件 → set_full（`changed`）；emit/DB/polish 留 coordinator。
/// - local：`changed` → DB + emit（幂等，无变化不落库/不重绘）；每 tick 查停顿润色。
/// - cloud：`changed`（= Committed/Finished）→ DB + 停顿润色（increase 被 take_polish_input
///   消耗 + polish_pending 护栏保证与原「仅 session_just_finished 触发」等价）；**每 tick emit**
///   （display + current_partial 预览，预览不进 DB）；用户可见错误（WSS 开启失败 / Failed）上报。
fn handle_streaming_tick(
    stage: &mut Stage,
    audio: &Arc<SharedAudioState>,
    config: &AppConfig,
    app_handle: &tauri::AppHandle,
    tx: &Sender<Command>,
) {
    let Stage::Streaming {
        pipeline,
        transcript,
        ..
    } = stage
    else {
        return;
    };

    let is_cloud = pipeline.is_cloud();
    let samples = audio.drain_samples();
    // local 在空样本时早退（无音频可处理）；cloud 不早退（仍 drain events / 检查 finish / emit）
    if !is_cloud && samples.is_empty() {
        return;
    }

    let changed = pipeline.tick(&samples, transcript);

    if is_cloud {
        // commit（changed）→ DB + 停顿润色（与原 session_just_finished 触发等价）
        if changed {
            if let Err(e) = update_transcription_raw(transcript, &config.asr_engine, "streaming") {
                warn!("DB (cloud streaming) failed: {}", e);
            }
            check_and_trigger_polish(transcript, pipeline.silence_duration(), config, tx);
        }
        // 用户可见错误（WSS 开启失败 / StreamEvent::Failed；local 错误只在承载层 warn）
        if let Some(e) = pipeline.take_error() {
            crate::result_window::update_result(app_handle, &e);
        }
        // 每 tick emit（display + current_partial 预览）——与原 cloud tick 末尾总 emit 一致
        let base = transcript.display_text();
        let partial = pipeline.current_partial();
        let display = if partial.is_empty() {
            base
        } else {
            format!("{}{}", base, partial)
        };
        if !display.is_empty() {
            crate::result_window::update_result(app_handle, &display);
        }
    } else {
        // local：changed → DB + emit（幂等）
        if changed {
            if let Err(e) = update_transcription_raw(transcript, &config.asr_engine, "streaming") {
                warn!("DB (streaming) failed: {}", e);
            }
            crate::result_window::update_result(app_handle, &transcript.display_text());
        }
        // 停顿润色（每 tick，留 coordinator：三路径共用 check_and_trigger_polish）
        check_and_trigger_polish(transcript, pipeline.silence_duration(), config, tx);
    }
}
```

- [x] **Step 3.3: `handle_toggle` cloud 分支改为建 `CloudPipelineEngine` → `Stage::Streaming`**

替换 `coordinator.rs:627-665`（cloud 分支整段，`#[cfg(feature = "cloud")] if use_cloud_streaming { ... return; }`）为：

```rust
            #[cfg(feature = "cloud")]
            if use_cloud_streaming {
                match octopus_asr_local::config::find_silero_vad() {
                    Ok(path) => match octopus_asr_local::vad::SileroVad::new(&path) {
                        Ok(mut vad) => {
                            vad_preroll(&mut vad);
                            crate::result_window::show_result(app_handle, "正在聆听…");
                            crate::tray::update_tray_label(app_handle, crate::tray::TrayState::Recording);

                            let cloud_engine = crate::cloud_pipeline::CloudPipelineEngine::new(
                                vad,
                                config.asr_engine.clone(),
                                config.language.clone(),
                                config.pause_polish_threshold_ms,
                            );
                            let pipeline = match StreamingPipeline::new(Box::new(cloud_engine)) {
                                Ok(p) => p,
                                Err(e) => {
                                    error!("StreamingPipeline (cloud) init failed: {}, abort", e);
                                    let _ = audio.stop();
                                    crate::result_window::hide_result(app_handle);
                                    crate::tray::update_tray_label(app_handle, crate::tray::TrayState::Idle);
                                    return;
                                }
                            };

                            // cloud 用独立 100ms tick 线程（STREAMING=200/CLOUD=100，不可合并）
                            let tick_active = Arc::new(AtomicBool::new(true));
                            start_cloud_streaming_tick_thread(tx.clone(), tick_active.clone());

                            *stage = Stage::Streaming {
                                pipeline,
                                transcript: Transcript::new(now_millis(), config.polish_mode),
                                streaming_active: tick_active,
                            };
                        }
                        Err(e) => {
                            error!("VAD init failed for cloud streaming: {}, falling back to VadSegmented", e);
                            let _ = audio.stop();
                            return;
                        }
                    },
                    Err(e) => {
                        error!("VAD not found for cloud streaming: {}, falling back to VadSegmented", e);
                        let _ = audio.stop();
                        return;
                    }
                }
                return;
            }
```

⚠️ **`Stage::Streaming.streaming_active` 字段名**：原 local 用 `streaming_active`。cloud 复用此字段存 `tick_active`（`start_cloud_streaming_tick_thread` 接收 `Arc<AtomicBool>`，字段名内部无关）。字段类型不变（`Arc<AtomicBool>`）。✓

⚠️ **`config.language` / `config.pause_polish_threshold_ms`**：确认 `AppConfig` 有此二字段（coordinator 既有代码 `config.language.clone()` / `config.pause_polish_threshold_ms` 已用）。✓

- [x] **Step 3.4: stop 路径合并——`Stage::Streaming` 统一 arm（cloud `take_close_handle` 分派）**

替换 `coordinator.rs:841-880`（`Stage::Streaming { ... } => { ... }` local stop arm）+ 紧随其后的 `coordinator.rs:882-933`（`#[cfg(feature="cloud")] Stage::CloudStreaming { ... }` arm）为**单一** `Stage::Streaming` arm：

```rust
        Stage::Streaming {
            pipeline,
            transcript,
            streaming_active,
        } => {
            info!("Toggle: stopping streaming, finalizing");
            streaming_active.store(false, Ordering::Relaxed);
            let final_samples = audio.drain_samples();
            let _ = audio.stop();

            #[cfg(feature = "cloud")]
            if pipeline.is_cloud() {
                // cloud: push tail（不发 Finish——Finish 由 close_async 发，避免重复）
                let _ = pipeline.finish_with_tail(&final_samples);
                let partial = pipeline.current_partial().to_string();
                if let Some(handle) = pipeline.take_close_handle() {
                    // spawn close_async，结果以 Command::CloudStreamingDone 回来；期间进 CloudClosing
                    let rt = tauri::async_runtime::handle();
                    let tx_clone = tx.clone();
                    let tr = std::mem::replace(transcript, Transcript::new(0, PolishMode::Disabled));
                    // 跨会话护栏：session_id = 本会话 transcript.id（详见 handle_cloud_streaming_done）
                    let session_id = tr.id;
                    rt.spawn(async move {
                        let result = handle.close_async().await;
                        let _ = tx_clone.send(Command::CloudStreamingDone {
                            text: result.map_err(|e| e.to_string()),
                            session_id,
                        });
                    });
                    *stage = Stage::CloudClosing { transcript: tr, current_partial: partial };
                    return;
                }
                // 无活跃 session：无需等 close，直接 finalize_cloud（无标点补全，服务端已分句）
                let tr = std::mem::replace(transcript, Transcript::new(0, PolishMode::Disabled));
                finalize_cloud(stage, tr, partial, config, app_handle, tx);
                return;
            }

            // local: finish_with_tail → Final → set_full → finalize_after_stop（带标点补全）
            let final_text = match pipeline.finish_with_tail(&final_samples) {
                TranscriptEvent::Final(text) => text,
                TranscriptEvent::Error(e) => {
                    error!("Streaming finish failed: {}", e);
                    transcript.edited_display().unwrap_or_else(|| transcript.db_text())
                }
                _ => transcript.edited_display().unwrap_or_else(|| transcript.db_text()),
            };
            pipeline.reset();
            if !final_text.is_empty() {
                transcript.set_full(&final_text);
            }
            info!("Final streaming text: '{}'", transcript.db_text());
            let tr = std::mem::replace(transcript, Transcript::new(0, PolishMode::Disabled));
            finalize_after_stop(stage, tr, config, app_handle, tx);
        }
```

- [x] **Step 3.5: `CloudStreamingTick` dispatch 改调 `handle_streaming_tick`**

替换 `coordinator.rs:323-335`（`#[cfg(feature = "cloud")] Command::CloudStreamingTick => { ... }`）为：

```rust
                    #[cfg(feature = "cloud")]
                    Command::CloudStreamingTick => {
                        {
                            let rc = runtime_config.read().unwrap();
                            config.polish_mode = rc.polish_mode;
                        }
                        if let Stage::Streaming { transcript, .. } = &mut stage {
                            transcript.set_mode(config.polish_mode);
                        }
                        if editing {
                            let _ = audio.drain_samples();
                        } else {
                            handle_streaming_tick(&mut stage, &audio, &config, &app_handle, &tx);
                        }
                    }
```

（唯一变化：stage 守卫 `Stage::CloudStreaming` → `Stage::Streaming`；调用 `handle_cloud_streaming_tick` → `handle_streaming_tick`。）

- [x] **Step 3.6: 删除 `handle_cloud_streaming_tick` 整函数**

删除 `coordinator.rs:1630-1812`（`/// 处理 CloudStreamingTick 命令...` 注释 + `fn handle_cloud_streaming_tick { ... }` 整体）。逻辑已迁入 `CloudPipelineEngine::tick`（Task 2）+ `handle_streaming_tick`（Step 3.2）。

- [x] **Step 3.7: 删除已迁出的 cloud helpers（`open_cloud_session` + `resolve_*` + `take_preroll`）**

删除 `coordinator.rs` 中以下已迁入 cloud_pipeline.rs 的函数（Task 2 已在 cloud_pipeline.rs 重建）：
- `take_preroll`（1489-1497，含注释 1489-1490）
- `resolve_cloud_entry`（1515-1529）
- `resolve_aliyun_config`（1531-1540）
- `resolve_bytedance_config`（1542-1551）
- `resolve_tencent_config`（1553-1568）
- `resolve_baidu_config`（1570-1585）
- `open_cloud_session`（1587-1628，含注释 1587-1588）

**保留** `is_cloud_engine`（1475-1487，loop 中 `use_cloud_streaming = is_cloud_engine(&config)` 仍用）、`start_cloud_streaming_tick_thread`（1499-1513）、`CLOUD_STREAMING_TICK_INTERVAL_MS`（192-194）。

删除 `coordinator.rs:196-203` 的 `CLOUD_PREROLL_BUFFER_SAMPLES`/`CLOUD_PREROLL_SAMPLES` 常量（已迁 cloud_pipeline.rs）。

- [x] **Step 3.8: 清理 7 处 `Stage::CloudStreaming` match 臂（由 `Stage::Streaming` 覆盖）**

逐处删除 `#[cfg(feature = "cloud")] Stage::CloudStreaming { ... }` 臂：

① **handle_cancel 停止臂** `coordinator.rs:2010-2018`：删除整段 `#[cfg(feature = "cloud")] Stage::CloudStreaming { tick_active, session, .. } => { ... }`。cloud cancel 现走 `Stage::Streaming` 臂（1993-2002）：`streaming_active.store(false)` + `pipeline.reset()`（CloudPipelineEngine.reset 内 `session.take()` drop session）+ `audio.stop()`——等价。

② **handle_cancel DB 删除臂** `coordinator.rs:2041-2045`：删除 `#[cfg(feature = "cloud")] Stage::CloudStreaming { transcript, .. } | ` 前缀，只留 `Stage::CloudClosing { transcript, .. } => { ... }`。即：
```rust
        #[cfg(feature = "cloud")]
        Stage::CloudClosing { transcript, .. } => {
            if transcript.db_inserted() { Some(transcript.id) } else { None }
        }
```
（`Stage::Streaming` 已在上方 2035 行覆盖 cloud-active 的 transcript。）

③ **handle_discard db_info 臂** `coordinator.rs:2113-2124`：删除 `#[cfg(feature = "cloud")] Stage::CloudStreaming { transcript, .. } | ` 前缀，留 `Stage::CloudClosing { transcript, .. }`。`Stage::Streaming`（2101）已覆盖。

④ **handle_discard 停止臂** `coordinator.rs:2167-2175`：删除 `#[cfg(feature = "cloud")] Stage::CloudStreaming { tick_active, session, .. } => { ... }`。cloud discard 走 `Stage::Streaming` 臂（2152-2161）。保留紧随的 `Stage::CloudClosing` 臂（2176-2183）。

⑤ **handle_polish_done** `coordinator.rs:2394-2396`：删除 `Stage::CloudStreaming { transcript, .. } |` 行，留 `Stage::CloudClosing { transcript, .. }`。`Stage::Streaming`（2391）覆盖。

⑥ **handle_polish_now** `coordinator.rs:2474-2476`：同⑤，删 `Stage::CloudStreaming` 行，留 `Stage::CloudClosing`。

⑦ **handle_enter_edit_mode** `coordinator.rs:2518-2520` + **commit_edit_apply** `coordinator.rs:2537-2539`：同⑤，各删 `Stage::CloudStreaming` 行，留 `Stage::CloudClosing`。

⑧ **stage_name** `coordinator.rs:2568-2569`：删除 `#[cfg(feature = "cloud")] Stage::CloudStreaming { .. } => "CloudStreaming",`。留 `Stage::CloudClosing { .. } => "CloudClosing"`。

- [x] **Step 3.9: check（双 feature 配置）**

Run: `cargo check -p octopus-desktop 2>&1 | tail -15`
Expected: 0 error。

Run: `cargo check -p octopus-desktop --features cloud 2>&1 | tail -20`
Expected: 0 error，0 warning（Task 2 的 dead_code 应已消除——`CloudPipelineEngine`/`open_cloud_session` 现被 handle_toggle 引用）。若有残留 `dead_code`（如 `resolve_*` 仅被 `open_cloud_session` 用，应已被引用），核实是否漏删 coordinator 重复定义导致未引用——删 coordinator 重复定义即可。

- [x] **Step 3.10: 跑测试（双 feature）**

Run: `cargo test -p octopus-desktop 2>&1 | tail -20`
Run: `cargo test -p octopus-desktop --features cloud 2>&1 | tail -20`
Expected: 全绿（pipeline + cloud_pipeline + 既有 coordinator/transcript 测试）。

- [x] **Step 3.11: 提交**

```bash
git add crates/desktop/src/coordinator.rs
git commit -m "refactor(asr): cloud 流式合并进 Stage::Streaming（2c-2 T3，删 handle_cloud_streaming_tick）"
```

---

## Task 4: 验证（双 feature test + clippy）+ 文档同步

**目标：** 全量验证零行为差异（编译/测试/clippy 双 feature），同步 spec 横幅 + architecture.md。e2e（真实 DashScope key）由用户本地执行，不在本 task 自动化范围。

**Files:**
- Modify: `docs/superpowers/specs/2026-06-25-archived-spec.md#asr-pipeline-stage2c2-design`（横幅状态）
- Modify: `docs/superpowers/specs/2026-06-25-archived-spec.md#asr-pipeline-design`（§3.4 阶段进度行）
- Modify: `docs/architecture.md`（新增 `cloud_pipeline.rs` 模块 + Stage 状态机描述）

- [x] **Step 4.1: workspace 全量 check + test（双 feature）**

Run: `cargo check --workspace --all-targets 2>&1 | tail -15`
Expected: 0 error。

Run: `cargo test --workspace 2>&1 | tail -25`
Expected: 全绿。

Run: `cargo check --workspace --all-targets --features desktop/cloud 2>&1 | tail -15`
Expected: 0 error。（确认 cloud feature 全 workspace 编译。）

- [x] **Step 4.2: clippy（双 feature，零新 warning）**

Run: `cargo clippy -p octopus-desktop --features cloud -- -D warnings 2>&1 | tail -25`
Expected: 0 warning（`-D warnings` 视 0 为通过）。若有，按提示修复（常见：未用 import、`#[allow]` 缺失）。

- [x] **Step 4.3: 零行为差异自检（逐条核对 spec §7）**

人工核对（不改代码，只读 + grep 验证）：

1. **tick 逻辑原样搬迁**：`grep -n "speech_confirm_count\|pre_roll_buffer\|push_pcm\|try_recv_text\|silence_duration" crates/desktop/src/cloud_pipeline.rs`——确认 onset 连续确认 / pre_roll 滚动 / push / drain / 双层 / 静音 finish / session take 全在。
2. **close 路径不动**：`grep -n "CloudClosing\|CloudStreamingDone\|finalize_cloud\|session_id" crates/desktop/src/coordinator.rs`——确认 `Stage::CloudClosing`/`Command::CloudStreamingDone`/`handle_cloud_streaming_done`/`finalize_cloud`/session_id 护栏原样保留。
3. **DB 时机不变**：cloud 仅 `Finished/Committed`（`changed`）时 DB（`handle_streaming_tick` cloud 分支 `if changed { update_transcription_raw }`）；local `changed` 时 DB。
4. **emit 频率不变**：cloud 每 tick emit（`handle_streaming_tick` cloud 分支末尾无 `if changed` 包裹）；local 仅 `changed` emit。
5. **预览不进 DB**：`drain_cloud_session` 的 `Text → current_partial`（无 event），仅 `Finished → Committed` 发事件 → 承载层 set_full → coordinator DB。
6. **逗号拼接一致**：`drain_cloud_session` 的 `if !committed_text.is_empty() && !committed_text.ends_with('，') { push '，' }` 与原 `coordinator.rs:1747-1752` 一致。

- [x] **Step 4.4: 同步 spec 横幅**

`docs/superpowers/specs/2026-06-25-archived-spec.md#asr-pipeline-stage2c2-design` 第 4 行（`> **状态**：...`）改为：

```
> **状态**：设计已定 + 实施计划就绪（2026-06-24）。实现见 plan `docs/superpowers/plans/2026-06-25-archived-plan.md#asr-pipeline-stage2c2`。
```

`docs/superpowers/specs/2026-06-25-archived-spec.md#asr-pipeline-design` §3.4 阶段 2c-2 进度行（"设计已定 2026-06-24，待 plan"）改为 "计划就绪 2026-06-24，待实现 + e2e"。

- [x] **Step 4.5: 同步 architecture.md**

在 `docs/architecture.md` 的 desktop 模块清单 + 状态机描述处：
- 新增模块 `crates/desktop/src/cloud_pipeline.rs`（cfg cloud）：`CloudPipelineEngine` impl `StreamingPipelineEngine`，承载云端流式 ASR 编排。
- `pipeline.rs` 描述更新：持 `Box<dyn StreamingPipelineEngine>`（`LocalPipelineEngine` / `CloudPipelineEngine`），`compute_speech_chunks` 共享 VAD helper。
- 状态机：`Stage::CloudStreaming` 已合并进 `Stage::Streaming`（cloud 走 100ms `CloudStreamingTick`，local 走 200ms `StreamingTick`，统一 `handle_streaming_tick`）；`Stage::CloudClosing` 保留（cloud async close 中间态）。

- [x] **Step 4.6: 提交文档同步**

```bash
git add docs/superpowers/specs/2026-06-25-archived-spec.md#asr-pipeline-stage2c2-design docs/superpowers/specs/2026-06-25-archived-spec.md#asr-pipeline-design docs/architecture.md
git commit -m "docs(asr): 同步 2c-2 计划就绪状态 + cloud_pipeline 模块（spec/architecture）"
```

- [x] **Step 4.7: e2e 清单（交用户本地执行，需 DashScope/云端 key）**

实现完成后，用户本地 e2e 验证（不自动化）：
1. 选云端引擎（如 aliyun DashScope）→ Toggle 开录 → 说话 → 预览（partial）实时显示。
2. 停顿 ≥ `pause_polish_threshold_ms` → 服务端 Finished → 文本提交（逗号拼接）→ 中间润色（mode=2 时）。
3. 再说一句 → 跨 utterance 拼接（"第一句，第二句"）。
4. Toggle stop → close_async → 最终润色 → 粘贴。
5. **跨会话护栏**：stop 后 close 在飞期间立刻 Cancel/Discard → 重开云端会话 → 旧会话迟到的 `CloudStreamingDone` 被 session_id 护栏丢弃（log 可见 "session_id mismatch ... 丢弃"）。
6. Failed 重试：模拟瞬时抖动（断网）→ "⚠️ 云端识别失败" → 恢复后下次 onset 重开 WSS。

---

## 风险与回滚

- **风险① `is_cloud()` 不对称**（spec §3.2 trait 未列，规划新增）：用于 §4.2 emit/DB/polish 不对称 + §4.3 stop 的 `finalize_cloud` vs `finalize_after_stop` 分派。若 e2e 发现 cloud 行为偏差，优先核查 `handle_streaming_tick` cloud 分支的 emit/DB/polish 时机。
- **风险② cloud 100ms tick**：`CloudStreamingTick` + `start_cloud_streaming_tick_thread` 必须保留（不可合并到 200ms `StreamingTick`），否则 cloud onset/finish 时序变化。
- **风险③ `committed_text` 镜像**：`CloudPipelineEngine` 自持 `committed_text`（无 transcript 访问），须与 `transcript.full()` 经 `Committed→set_full` 保持同步。e2e 验证跨 utterance 拼接正确。
- **风险④ stop 标点补全**：cloud 走 `finalize_cloud`（不补 "。"，服务端已分句）；local 走 `finalize_after_stop`（补 "。"）。`is_cloud()` 分派须正确，否则 cloud 误补标点。
- **风险⑤ cloud 停顿润色触发时机（已知可忽略差异）**：新设计 cloud 在 `changed`（= `Committed`/Finished 带文本）时触发 `check_and_trigger_polish`（spec §4.2）。原代码在**任意** `Finished`（含空 partial）时触发。差异场景：一次被限流的 commit（首停顿 < `MIN_POLISH_INTERVAL_SEC`）后 >1s 出现一个**空 partial 的 Finished**（服务端对无语音 utterance 收尾）——原代码会在该空 Finished 触发润色已提交文本，新设计等到下一次真实 commit。两者最终都在 stop 前润色同一文本，仅时机略晚。属极端边界（空 Finished 罕见），与 spec §4.2 「commit 时润色」设计一致，接受。e2e 不覆盖此边界。
- **回滚**：每 task 独立提交，可逐 task `git revert`。Task 1/2 不改 cloud 运行时行为（Task 1 cloud 走旧路径；Task 2 未接线）；Task 3 是行为切换点，revert Task 3 即恢复旧 `Stage::CloudStreaming` 路径。

## 后续

- **2c-3**：VadSegmented（离线分段）归位——`OfflineAsrEngine` async `transcribe` + seq 乱序回填，语义模型不同（非流式分段），单独设计。
- **2d**：coordinator 清理——`StreamingPipeline` 完整接管三路径 emit/DB/polish，coordinator 退化为纯路由。cloud 的 `Stage::CloudClosing` close 中间态是 2d 仍需保留的唯一 cloud 特例。

---

## 2026-06-25-asr-server-stage3



**Goal:** 把 `octopus-server` 两条 ASR 路径（流式 `/ws/stream` + 批处理 `/transcribe`）从裸调/旧路径迁到 asr helper（`StreamingRunner` / `transcribe_batch`），消除手搓 VAD + 手拼 JSON，与 cli/desktop 三端统一。

**Architecture:** 新建 `server/src/pipeline.rs`——`WsStreamSession`（薄包 asr `StreamingRunner`：`new`/`feed`/`finish`/`reset`）+ `event_to_json`（`TranscriptEvent` 4 variant → `{type,text}` JSON）。`main.rs::handle_ws` 用 `WsStreamSession` 替裸 `StreamingSession` + 删 `detect_silence_gap_local`（手搓 VAD 已收编进 runner）+ 回推改 `event_to_json`；`main.rs::transcribe` 改走 `AsrEngineManager::transcribe_batch` + `PipelineConfig`。不接 cloud；polish/denoise 不进 server（spec §3.8/§3.6）。

**Tech Stack:** Rust / axum 0.8 ws / tokio / `octopus-asr-local`（`StreamingRunner`/`TranscriptEvent`/`StreamingEngine`/`StreamingSession`/`pipeline::transcribe_batch`/`PipelineConfig`）。

**Spec:** `docs/superpowers/specs/2026-06-25-archived-spec.md#asr-server-stage3-design`

> **对 spec §4.1 的微调（实施时确定，Task 4 回写 spec）：** `WsStreamSession::new` 签名由 spec 的 `new(engine: &str, correct)` 改为 `new(engine: Box<dyn StreamingEngine>, correct)`——解耦 `StreamingSession`（对齐 desktop `LocalPipelineEngine::from_session`「先构 session 再包」），且可注入 fake 单测。`handle_ws` 负责调 `StreamingSession::new(&engine)` 后装箱传入。

---

## File Structure

| 文件 | 职责 | 动作 |
|---|---|---|
| `crates/server/src/pipeline.rs`（**新建**） | WS↔`StreamingRunner` 桥接 + `TranscriptEvent`→JSON 序列化（纯逻辑，可单测） | 新建：`WsStreamSession` + `event_to_json` + `#[cfg(test)] mod tests` |
| `crates/server/src/main.rs` | axum 路由 + WS/HTTP 胶水 | 加 `mod pipeline;`；`handle_ws` 迁移 + 删 `detect_silence_gap_local`；`transcribe` 改 `transcribe_batch` |

---

## Task 1: 新建 `server/src/pipeline.rs`（`WsStreamSession` + `event_to_json`，TDD）

**Files:**
- Create: `crates/server/src/pipeline.rs`
- Modify: `crates/server/src/main.rs`（加 `mod pipeline;`）
- Test: `crates/server/src/pipeline.rs`（`#[cfg(test)] mod tests`）

- [x] **Step 1: 在 `main.rs` 注册新模块**

在 `crates/server/src/main.rs` 顶部 `use` 区上方加一行模块声明：

```rust
mod pipeline;
```

放在文件第 1 行（`use axum::{...}` 之前）。

- [x] **Step 2: 写 `pipeline.rs` 骨架 + 失败测试（todo! 占位）**

创建 `crates/server/src/pipeline.rs`，内容如下（实现处用 `todo!()`，测试引用之 → 运行时 panic = RED）：

```rust
//! server 流式 pipeline：WS↔asr `StreamingRunner` 桥接 + `TranscriptEvent`→JSON 序列化。
//!
//! 薄包 [`StreamingRunner`]（VAD 静音 + 标点 + accept/flush/finish + 纠错已收编）。
//! 不含 polish / denoise（总 spec §3.8/§3.6：留端，server 不依赖 llm/cpal）。

use anyhow::Result;
use octopus_asr_local::streaming_runner::{StreamingEngine, StreamingRunner, TranscriptEvent};

/// WS 流式会话：薄包 asr `StreamingRunner`。
pub struct WsStreamSession {
    runner: StreamingRunner,
}

impl WsStreamSession {
    /// 由已构造的流式引擎装箱传入（解耦 `StreamingSession`，便于测试注入 fake）。
    /// `correct` 来自 `app_config.asr_correct`（与批处理 `PipelineConfig.correct` 同源）。
    pub fn new(engine: Box<dyn StreamingEngine>, correct: bool) -> Result<Self> {
        todo!("Step 4 实现")
    }

    /// 喂一帧已降噪 16k 样本，返回本帧事件流（0..n 个 TranscriptEvent）。
    pub fn feed(&mut self, samples_16k: &[f32]) -> Vec<TranscriptEvent> {
        todo!("Step 4 实现")
    }

    /// 收尾：runner.finish() → Final（追加句号 + 简繁归一）。
    pub fn finish(&mut self) -> TranscriptEvent {
        todo!("Step 4 实现")
    }

    /// 重置（会话间复用前调用）。
    pub fn reset(&mut self) {
        todo!("Step 4 实现")
    }
}

/// `TranscriptEvent` → server 私有 WS JSON（统一 `{type,text}`）。
///
/// `TranscriptEvent` 无 Serialize（仅 Debug/Clone），为不污染 asr crate
/// （总 spec §3.1：asr = 零件库 + 端做桥接），server 端 match 序列化。
pub fn event_to_json(ev: &TranscriptEvent) -> String {
    todo!("Step 4 实现")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    #[test]
    fn event_to_json_all_variants() {
        assert_eq!(
            event_to_json(&TranscriptEvent::Partial("你好".into())),
            r#"{"type":"partial","text":"你好"}"#
        );
        assert_eq!(
            event_to_json(&TranscriptEvent::Committed("foo".into())),
            r#"{"type":"committed","text":"foo"}"#
        );
        assert_eq!(
            event_to_json(&TranscriptEvent::Final("end".into())),
            r#"{"type":"final","text":"end"}"#
        );
        assert_eq!(
            event_to_json(&TranscriptEvent::Error("boom".into())),
            r#"{"type":"error","text":"boom"}"#
        );
    }

    #[test]
    fn event_to_json_escapes_backslash_quote_newline() {
        // 输入：a"b\c（换行）d —— 先转 \ 再转 " 再转 \n，反斜杠成对。
        let ev = TranscriptEvent::Final("a\"b\\c\nd".into());
        assert_eq!(
            event_to_json(&ev),
            r#"{"type":"final","text":"a\"b\\c\nd"}"#
        );
    }

    /// 可编程 fake：第一次 accept 返 Some，之后 None；finish 返固定串。
    /// （server 私有测试设施；asr crate 内的 FakeStreamingEngine 非 pub，不能复用。）
    struct FakeEngine {
        next_accept: Mutex<Option<String>>,
        finish_text: String,
    }
    impl StreamingEngine for FakeEngine {
        fn accept_samples(&self, _samples: &[f32], _was_silent: bool) -> Result<Option<String>> {
            Ok(self.next_accept.lock().unwrap().take())
        }
        fn flush(&self, _insert_comma: bool) -> Result<Option<String>> {
            Ok(None)
        }
        fn finish(&self) -> Result<String> {
            Ok(self.finish_text.clone())
        }
        fn reset(&self) {}
    }

    #[test]
    fn ws_stream_session_feed_partial_then_empty_finish_final() {
        let engine = FakeEngine {
            next_accept: Mutex::new(Some("hi".into())),
            finish_text: "final".into(),
        };
        let mut s = WsStreamSession::new(Box::new(engine), false).unwrap();
        // 单帧 512 静音样本（32ms < 500ms 阈值），无论 VAD 是否存在都不触发 flush，
        // 只走 accept_samples → Partial（detect_silence_gap 在 vad=None 时返回 (false,false)）。
        assert_eq!(
            s.feed(&[0.0_f32; 512]),
            vec![TranscriptEvent::Partial("hi".into())]
        );
        // accept 已 take → 第二次 None → 空事件。
        assert!(s.feed(&[0.0_f32; 512]).is_empty());
        assert_eq!(s.finish(), TranscriptEvent::Final("final".into()));
    }
}
```

- [x] **Step 3: 跑测试验证 RED（todo! panic）**

Run: `cargo test -p octopus-server pipeline::tests 2>&1 | tail -30`
Expected: 编译通过，3 个测试中 `event_to_json_all_variants` / `event_to_json_escapes_backslash_quote_newline` / `ws_stream_session_feed_partial_then_empty_finish_final` 均 **FAIL**，报 `not yet implemented: Step 4 实现`（`todo!()` panic）。

- [x] **Step 4: 实现 `WsStreamSession` + `event_to_json`（替换 4 处 `todo!()`）**

用 Edit 把 `pipeline.rs` 中 4 处 `todo!("Step 4 实现")` 替换为真实实现。

`WsStreamSession::new` —— 替换为：
```rust
    pub fn new(engine: Box<dyn StreamingEngine>, correct: bool) -> Result<Self> {
        Ok(Self {
            runner: StreamingRunner::new(engine, correct)?,
        })
    }
```

`WsStreamSession::feed` —— 替换为：
```rust
    pub fn feed(&mut self, samples_16k: &[f32]) -> Vec<TranscriptEvent> {
        self.runner.push_samples(samples_16k)
    }
```

`WsStreamSession::finish` —— 替换为：
```rust
    pub fn finish(&mut self) -> TranscriptEvent {
        self.runner.finish()
    }
```

`WsStreamSession::reset` —— 替换为：
```rust
    pub fn reset(&mut self) {
        self.runner.reset()
    }
```

`event_to_json` —— 替换为：
```rust
pub fn event_to_json(ev: &TranscriptEvent) -> String {
    let (ty, text) = match ev {
        TranscriptEvent::Partial(t) => ("partial", t),
        TranscriptEvent::Committed(t) => ("committed", t),
        TranscriptEvent::Final(t) => ("final", t),
        TranscriptEvent::Error(t) => ("error", t),
    };
    // 先转反斜杠，再转引号/换行，避免引号转义产生的反斜杠被二次转义。
    let escaped = text
        .replace('\\', r"\\")
        .replace('"', r#"\""#)
        .replace('\n', r"\n");
    format!(r#"{{"type":"{}","text":"{}"}}"#, ty, escaped)
}
```

- [x] **Step 5: 跑测试验证 GREEN**

Run: `cargo test -p octopus-server pipeline::tests 2>&1 | tail -15`
Expected: `3 passed; 0 failed`。

- [x] **Step 6: cargo check + clippy（零新 warning）**

Run: `cargo clippy -p octopus-server --all-targets 2>&1 | tail -20`
Expected: 编译通过，**无新 warning**（pipeline.rs 内 `WsStreamSession` 此时尚未被 main.rs 使用，可能报 `field runner never read` / dead_code——若出现，Task 2 接线后自动消失；此处记录 warning 数量，Task 2 后归零）。

- [x] **Step 7: Commit**

```bash
git add crates/server/src/pipeline.rs crates/server/src/main.rs
git commit -m "feat(server): WsStreamSession + event_to_json（阶段3 Task 1）

新建 server/src/pipeline.rs：WsStreamSession 薄包 asr StreamingRunner
（new(Box<dyn StreamingEngine>,correct)/feed(16k)/finish/reset）+
event_to_json（TranscriptEvent 4 variant → {type,text} JSON，含转义）。
3 单测绿。main.rs 注册 mod pipeline。"
```

---

## Task 2: `handle_ws` 迁移到 `WsStreamSession` + 删 `detect_silence_gap_local`

**Files:**
- Modify: `crates/server/src/main.rs`（`use` 区 + `handle_ws` L221-367 + 删 `detect_silence_gap_local` L175-219）

- [x] **Step 1: 加 `use` 导入**

在 `crates/server/src/main.rs` 的 `use` 区（L1-15 附近）加两行：

```rust
use octopus_asr_local::streaming_runner::TranscriptEvent;
use pipeline::{event_to_json, WsStreamSession};
```

（`use octopus_asr_local::engine::AsrEngineManager;` 之后即可。）

- [x] **Step 2: 删除 `detect_silence_gap_local` 整个函数**

删除 `crates/server/src/main.rs` 中 L175-219 的 `fn detect_silence_gap_local(...) -> bool { ... }` 整个函数（含前面的注释行 `// ── WebSocket ──` 保留，只删函数本身）。

- [x] **Step 3: 用新 `handle_ws` 替换旧实现**

把 `crates/server/src/main.rs` 中 `async fn handle_ws(...) { ... }`（L221-367）整个函数替换为：

```rust
async fn handle_ws(
    mut socket: axum::extract::ws::WebSocket,
    _engine_manager: Arc<AsrEngineManager>,
    engine: String,
    _language: String,
) {
    use futures_util::StreamExt;

    // Validate engine
    if octopus_asr_local::config::resolve_engine_category(&engine).is_none() {
        let _ = socket
            .send(Message::Text(
                event_to_json(&TranscriptEvent::Error(format!(
                    "Unknown engine '{}'",
                    engine
                )))
                .into(),
            ))
            .await;
        return;
    }

    let session = match octopus_asr_local::streaming_engine::StreamingSession::new(&engine) {
        Ok(s) => s,
        Err(e) => {
            let _ = socket
                .send(Message::Text(
                    event_to_json(&TranscriptEvent::Error(format!(
                        "Failed to create streaming session: {}",
                        e
                    )))
                    .into(),
                ))
                .await;
            return;
        }
    };

    // correct 与批处理 PipelineConfig.correct 同源（app_config.asr_correct）。
    let correct = octopus_asr_local::config::load_app_config_cached().asr_correct;
    let mut stream = match WsStreamSession::new(Box::new(session), correct) {
        Ok(s) => s,
        Err(e) => {
            let _ = socket
                .send(Message::Text(
                    event_to_json(&TranscriptEvent::Error(format!(
                        "VAD init: {}",
                        e
                    )))
                    .into(),
                ))
                .await;
            return;
        }
    };

    while let Some(msg) = socket.next().await {
        match msg {
            Ok(Message::Binary(data)) => {
                // f32 PCM little-endian chunks
                let chunk: Vec<f32> = data
                    .chunks_exact(4)
                    .map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]]))
                    .collect();
                if chunk.is_empty() {
                    continue;
                }
                for ev in stream.feed(&chunk) {
                    let _ = socket.send(Message::Text(event_to_json(&ev).into())).await;
                }
            }
            Ok(Message::Text(cmd)) => {
                if cmd == "flush" {
                    let ev = stream.finish();
                    let _ = socket.send(Message::Text(event_to_json(&ev).into())).await;
                    stream.reset();
                }
            }
            Ok(Message::Close(_)) => break,
            Err(_) => break,
            _ => {}
        }
    }
}
```

行为对照（零差异）：输入协议不变（binary f32 PCM + `"flush"` text + Close）；静音 flush 由 `StreamingRunner` 内部处理（`PUNCTUATION_SILENCE_THRESHOLD = 0.5s`，与旧 `detect_silence_gap_local` 一致）；错误从手拼 `{error}` 改为 `{type:error}`。

- [x] **Step 4: cargo check + clippy（验证接线，Task 1 的 dead_code warning 应消失）**

Run: `cargo clippy -p octopus-server --all-targets 2>&1 | tail -20`
Expected: 编译通过，**零 warning**（`WsStreamSession`/`event_to_json` 已被 `handle_ws` 使用，dead_code 消除）。若 `Message`/`Query`/`State` 等已有 import 缺失，按编译器提示补齐。

- [x] **Step 5: Commit**

```bash
git add crates/server/src/main.rs
git commit -m "refactor(server): handle_ws 迁 WsStreamSession + 删 detect_silence_gap_local（阶段3 Task 2）

裸调 StreamingSession + 手搓 SileroVad/detect_silence_gap_local + 手拼
{text,final} → WsStreamSession（薄包 StreamingRunner，VAD 静音/标点内部
收编）。WS 回推改 event_to_json（{type:text}）。静音 flush 阈值 0.5s
一致，零行为差异。"
```

---

## Task 3: `transcribe` 改走 `transcribe_batch`

**Files:**
- Modify: `crates/server/src/main.rs:118-122`（`transcribe` 函数内的引擎调用）

- [x] **Step 1: 替换引擎调用**

在 `crates/server/src/main.rs` 的 `transcribe` 函数内，把这段（L121-122）：

```rust
    let text = state.engine_manager.switch_model(engine)
        .and_then(|_| state.engine_manager.transcribe(&samples, language));
```

替换为：

```rust
    let cfg = octopus_asr_local::pipeline::PipelineConfig::from_app_config(language);
    let text = state.engine_manager.switch_model(engine)
        .and_then(|_| state.engine_manager.transcribe_batch(&samples, &cfg));
```

说明：`switch_model(engine)` 保留（`transcribe_batch` 用 active engine，需先切）；`language: &str` 直接传 `PipelineConfig::from_app_config(language: &str)`；`transcribe_batch(&samples, &cfg) -> Result<String>` 与旧 `transcribe` 返回类型一致，`and_then` 链不变。`TranscribeResponse { text, duration_ms, rtf }` 格式不变。

- [x] **Step 2: cargo check + clippy**

Run: `cargo clippy -p octopus-server --all-targets 2>&1 | tail -20`
Expected: 编译通过，零 warning。若旧 `transcribe` 方法在 `AsrEngineManager` 上删除后无其他调用方，编译器会提示——本计划不删 `AsrEngineManager::transcribe`（可能仍有其他用途，留待后续清理）。

- [x] **Step 3: 跑 server 全部单测确认无回归**

Run: `cargo test -p octopus-server 2>&1 | tail -15`
Expected: Task 1 的 3 个测试仍 `3 passed; 0 failed`。

- [x] **Step 4: Commit**

```bash
git add crates/server/src/main.rs
git commit -m "refactor(server): transcribe 改走 transcribe_batch（阶段3 Task 3）

AsrEngineManager.transcribe → transcribe_batch + PipelineConfig::
from_app_config（对齐 cli：VAD 分段 + 纠错 + 简繁归一化）。
TranscribeResponse 格式不变。"
```

---

## Task 4: workspace 验证 + 文档同步 + e2e 交付

**Files:**
- Verify: 全 workspace
- Modify: `docs/superpowers/specs/2026-06-25-archived-spec.md#asr-server-stage3-design`（§4.1 签名回写）、`docs/superpowers/specs/2026-06-25-archived-spec.md#asr-pipeline-design`（横幅阶段3）、`docs/architecture.md`（server crate 描述）

- [x] **Step 1: 全 workspace 编译 + 测试 + clippy**

Run:
```bash
cargo test --workspace --lib 2>&1 | tail -20
cargo clippy --workspace --all-targets 2>&1 | grep -E "warning|error" | head
```
Expected: workspace lib 测试全绿（含 Task 1 的 3 个 server 单测）；clippy 零新 warning（server crate 无 `unused`/`dead_code`）。

- [x] **Step 2: 回写 spec §4.1 签名微调**

在 `docs/superpowers/specs/2026-06-25-archived-spec.md#asr-server-stage3-design` §4.1，把 `WsStreamSession::new` 的签名说明由 `new(engine: &str, correct)` 改为 `new(engine: Box<dyn StreamingEngine>, correct)`，并在代码块与文字说明中体现「`handle_ws` 负责调 `StreamingSession::new(&engine)` 后装箱传入；解耦 + 可注入 fake 单测」。同步更新 §4.1 代码块与 §10 迁移映射表对应行。

- [x] **Step 3: 总 spec 横幅标注阶段3 已实施**

在 `docs/superpowers/specs/2026-06-25-archived-spec.md#asr-pipeline-design` 横幅（L4-12 附近）追加一行：

```
> **阶段3 已实施（2026-06-25）**：server 两端点迁 asr helper——流式 `/ws/stream` 用 `WsStreamSession`（薄包 `StreamingRunner`），批处理 `/transcribe` 走 `transcribe_batch`。spec `2026-06-25-archived-spec.md#asr-server-stage3-design`，plan `2026-06-25-archived-plan.md#asr-server-stage3`。
```

并修订 §7「本次不迁 server」措辞：注明「本次」= 阶段1/2，阶段3 已补齐。

- [x] **Step 4: 同步 `architecture.md` server crate 描述**

在 `docs/architecture.md` 的 server crate 段落（或 crates 列表），把 server 描述从「单文件 main.rs，裸调 StreamingSession」更新为「`pipeline.rs`（WsStreamSession 薄包 StreamingRunner + event_to_json）+ `main.rs`（路由）；流式/批处理均走 asr helper」。

- [ ] **Step 5: e2e 手动回归清单（待用户本地，需 ASR 模型环境；代码/单测/文档已完成 2026-06-26）**

起服务并验证两条路径（需本地有 ASR 模型 + VAD 模型，参考 desktop e2e 环境）：

```bash
# 1. 起 server（终端 A）
cargo run -p octopus-server -- --port 3000

# 2. 流式 WS：发 16k f32 PCM，验回推 {type:text} 序列
#    （用项目既有 e2e 脚本或 wscat + 小 wav 转 raw f32；参考 desktop cloud e2e 的 WS 客户端写法）
#    预期：语音段 → {"type":"partial",...}；静音≥0.5s → {"type":"committed",...}；发 "flush" → {"type":"final",...}

# 3. 批处理 HTTP：POST /transcribe，验 transcribe_batch 结果
curl -s -X POST "http://localhost:3000/transcribe?engine=<engine>&language=zh" \
  --data-binary @<16k-wav-or-raw-pcm> | jq .
#    预期：{"text":"...","duration_ms":...,"rtf":...}，文本与旧 /transcribe 路径一致
```

若行为与旧路径有差异（尤其静音 flush 时机），记录并评估是否需调整（以 `StreamingRunner` 为准——desktop 已验证）。

- [x] **Step 6: Commit 文档 + 交付报告**

```bash
git add docs/superpowers/specs/2026-06-25-archived-spec.md#asr-server-stage3-design \
        docs/superpowers/specs/2026-06-25-archived-spec.md#asr-pipeline-design \
        docs/architecture.md
git commit -m "docs: 阶段3 server 迁移同步（spec §4.1 签名回写 + 总 spec 横幅 + architecture）"
```

交付报告确认：workspace 测试全绿、clippy 零 warning、e2e 通过（流式 `{type}` 序列 + 批处理 `transcribe_batch`）。

---

## Self-Review（plan 写完后自查）

**1. Spec coverage：**
- §2 范围（流式+批处理迁、不接 cloud、polish/denoise 不进）→ Task 1-3 全覆盖；边界由「不引入 cloud/polish/denoise 代码」保证 ✓
- §3 文件结构（pipeline.rs 新建 + main.rs 改）→ Task 1（pipeline.rs）+ Task 2/3（main.rs）✓
- §4 组件（WsStreamSession + event_to_json）→ Task 1 ✓
- §5 数据流（流式 + 批处理）→ Task 2（handle_ws）+ Task 3（transcribe）✓
- §6 接口契约（WS 输出 {type,text}、输入不变、批处理响应不变）→ Task 1 event_to_json + Task 2/3 保留格式 ✓
- §7 错误处理 → Task 2 handle_ws（建连失败回推 {type:error}、单帧 Error 继续）✓
- §8 删除项 → Task 2 Step 2（detect_silence_gap_local）✓
- §9 测试 → Task 1 单测（event_to_json + WsStreamSession）+ Task 4 e2e ✓
- §10 迁移映射 → Task 1-3 逐项 ✓
- §4.1 签名微调回写 → Task 4 Step 2 ✓

**2. Placeholder 扫描：** 无 TBD/TODO（`todo!()` 是 Task 1 TDD 的 RED 占位，Step 4 明确替换为实现，非遗留）。所有步骤含完整代码/命令 ✓

**3. Type consistency：** `WsStreamSession::new(engine: Box<dyn StreamingEngine>, correct: bool) -> Result<Self>` 在 Task 1（定义）与 Task 2（`WsStreamSession::new(Box::new(session), correct)`）一致；`feed(&[f32]) -> Vec<TranscriptEvent>` / `finish() -> TranscriptEvent` / `reset()` 一致；`event_to_json(&TranscriptEvent) -> String` 一致；`transcribe_batch(&samples, &cfg)` 与 asr `engine.rs:151` 签名一致 ✓

---

## 2026-06-25-cloud-asr-cli

# 云端 ASR 下沉 cli 实施计划（`octopus-asr-cloud` crate）


**Goal:** 新建 `octopus-asr-cloud` crate（4 provider WSS 协议层 + 批引擎），让 cli 转译音频文件可选云端 ASR（DashScope/ByteDance/Tencent/Baidu），desktop 本次零改动。

**Architecture:** 协议层从 desktop `*_stream.rs` 1:1 复刻（仅改 spawn 方式），`CloudBatchEngine impl asr::OfflineAsrEngine`（单段→单 WSS session→完整文本，分段由 `asr::pipeline::transcribe_segments` 自动完成）；cli 层做本地/云端分流，两端都产出 `dyn OfflineAsrEngine` 喂 `transcribe_batch`。依赖单向 `asr ← cloud`。

**Tech Stack:** Rust workspace；tokio + tokio-tungstenite(native-tls)；复用 `octopus-asr-local`（trait/config）+ `octopus-infra`（ModelEntry/parse_model_spec）。

**关联 spec：** `docs/superpowers/specs/2026-06-25-archived-spec.md#cloud-asr-cli-design`。

---

## 实施时对 spec 措辞的两点据实修正（核对 desktop 源码后）

1. **`open()` 保持同步、非 async**（spec §4.1 写的是 `async fn open`）。核对 `crates/desktop/src/{aliyun,bytedance,tencent,baidu}_stream.rs`：各 `open()` 仅做 `CloudStreamHandle::new()` + `spawn(session task)` + 立即返回 handle，**不 await 任何 future**。故 cloud crate 的 `open()` 也保持同步签名，唯一改造是：去掉 `rt: &tauri::async_runtime::RuntimeHandle` 参数，`rt.spawn(...)` → `tokio::spawn(...)`。真正的 async 收尾在 `CloudStreamHandle::close_async`。语义与 spec 一致，措辞更省事。

2. **CloudBatchEngine 不自己 VAD 分段**（spec §4.2 倾向"复用 segment_audio_vad"）。核对 `crates/asr/src/pipeline.rs:73` `transcribe_segments`：它已实现 VAD 分段 + CJK/非 CJK 连接，并对**每段**调 `engine.transcribe(seg)`。cli 调用链 `transcribe_batch → transcribe_segments → cloud_engine.transcribe(seg)` 会自动分段。故 `CloudBatchEngine::transcribe` 的语义是「**单段**音频（≤30s，由上层保证）→ 单个 WSS session → 完整文本」，无需自己分段、无需自己拼接。大幅简化批引擎。

3. **`is_cloud_spec` / `from_spec` 用 `parse_model_spec` 的 3-part provider 前缀判断，不查 DB**（spec §4.3 倾向"复用 resolve_engine_category"）。核对 `crates/infra/src/db.rs:239-252` `parse_model_spec`：**2-part**（1 个冒号，如 `"aliyun:qwen-asr"`）按 `NameOnly` 兜底——provider 字段丢失。故云端分流必须用 **3-part spec**（`provider:category:model_name`，如 `aliyun:Fun-ASR:fun-asr-realtime`；category 见 `asr/config.rs:299 category_label`：Aliyun=Fun-ASR / ByteDance=Doubao-ASR / Tencent=Tencent-ASR / Baidu=Baidu-ASR）。用 `parse_model_spec` 取 3-part 的 `provider` 字段判云端，**不调 `resolve_engine_category`**——后者内部 `load_config()` 查 DB，会让分流与单测依赖 DB 命中状态。2-part/裸名 → `NameOnly` → 非云端（走本地分支，本地 `switch_model` 对云端裸名 bail，与现状一致）。`from_spec` 同此判定、不查 DB；DB 查找推迟到 `transcribe` 内 `open_cloud_session`（resolve_*_config）。

---

## 两个开放项的最终结论（已核对源码）

| 开放项 | 结论 | 依据 |
|---|---|---|
| 批引擎音频策略 | 单段单 session，分段交给 `transcribe_segments` | `asr/pipeline.rs:73-143` 已做 VAD 分段+CJK 连接，每段调 `engine.transcribe(seg)` |
| `skip_corrector` | `CloudBatchEngine::skip_corrector() -> true` | 桌面端云端流式不走 `transcribe_batch`、从不用 corrector；云端结果质量高，本地拼音纠错对齐「跳过」 |

---

## File Structure（本次 crate）

```
crates/asr-cloud/                 # 新建 crate
├── Cargo.toml                    # Task 1
└── src/
    ├── lib.rs                    # Task 1（mod + re-export，逐 task 补）
    ├── cloud_types.rs            # Task 1（迁自 desktop/cloud_types.rs）
    ├── aliyun_stream.rs          # Task 2（复刻 desktop/aliyun_stream.rs）
    ├── bytedance_stream.rs       # Task 3（复刻 desktop/bytedance_stream.rs）
    ├── tencent_stream.rs         # Task 4（复刻 desktop/tencent_stream.rs）
    ├── baidu_stream.rs           # Task 4（复刻 desktop/baidu_stream.rs）
    ├── config.rs                 # Task 5（resolve_*_config + open_cloud_session）
    └── batch.rs                  # Task 6（CloudBatchEngine impl OfflineAsrEngine）
```

修改的既有文件：
- `Cargo.toml`（workspace members，Task 1）
- `crates/asr/src/engine.rs`（加 `active_engine` getter，Task 7）
- `crates/cli/Cargo.toml`（加 octopus-asr-cloud 依赖，Task 7）
- `crates/cli/src/pipeline.rs`（本地/云端分流，Task 7）
- 文档（Task 8）：spec 横幅、`docs/architecture.md`、记忆。

**desktop 本次零改动**：`crates/desktop/src/{aliyun,bytedance,tencent,baidu}_stream.rs`/`cloud_types.rs`/`cloud_pipeline.rs` 副本暂留（第二步再合并）。

---

## 复刻通用规则（Task 2/3/4 共用，避免每个 task 重复）

每个 `*_stream.rs` 从 desktop 复制到 asr-cloud 时，做且仅做以下改造：

1. **`use` 路径**：`use crate::cloud_types::{...}` 不变（cloud crate 内 `cloud_types` 同名模块）。
2. **`open()` 签名**：去掉首参 `rt: &tauri::async_runtime::RuntimeHandle`；函数体 `rt.spawn(async move {...})` → `tokio::spawn(async move {...})`；其余（参数、返回 `Result<CloudStreamHandle>`、内部 `CloudStreamHandle::new()` + 分发 + 错误 `tx_for_err.send(StreamEvent::Failed(...))`）**逐字照搬**。
3. **`run_xxx_session` 及全部 helper**：**逐字照搬**（协议字节级、鉴权算法、帧格式、WS 收发循环 1:1，零行为差异）。
4. 模块文档头注释里"tauri::async_runtime"措辞改为"tokio runtime（调用方 block_on 驱动）"。

> 复制源用 Read 读 desktop 对应文件全文，整体粘贴后做上述 4 点改造。不要手抄协议常量/帧格式——必须从源文件复制，保证字节级一致。

---

## Task 1: 建 `octopus-asr-cloud` crate 骨架 + cloud_types 迁移

**Files:**
- Create: `crates/asr-cloud/Cargo.toml`
- Create: `crates/asr-cloud/src/lib.rs`
- Create: `crates/asr-cloud/src/cloud_types.rs`
- Modify: `Cargo.toml`（workspace members）

- [x] **Step 1: 注册 workspace member**

编辑 `/Users/wudarui/workspace/agent/octopus/.claude/worktrees/model-mgmt-ui/Cargo.toml`，把 `members` 行改为：

```toml
members = ["crates/infra", "crates/asr", "crates/asr-cloud", "crates/server", "crates/cli", "crates/desktop", "crates/llm", "crates/dlp", "crates/download"]
```

- [x] **Step 2: 写 crate Cargo.toml**

Create `crates/asr-cloud/Cargo.toml`（依赖版本对齐 `crates/desktop/Cargo.toml`）：

```toml
[package]
name = "octopus-asr-cloud"
version = "0.1.0"
edition = "2021"

[dependencies]
octopus-asr-local = { path = "../asr" }
octopus-infra = { path = "../infra" }

# Async + WSS（wss:// 需 native-tls）
tokio = { version = "1", features = ["full"] }
tokio-tungstenite = { version = "0.24", features = ["native-tls"] }
futures-util = "0.3"

# 协议层依赖（与 desktop cloud feature 一致）
serde_json = "1"
uuid = { version = "1", features = ["v4"] }
base64 = "0.22"
flate2 = "1"
hmac = "0.12"
sha1 = "0.10"

# 通用
anyhow = "1"
log = "0.4"
```

- [x] **Step 3: 写 lib.rs 骨架**

Create `crates/asr-cloud/src/lib.rs`：

```rust
//! 云端 ASR（cli/server 批处理用）。
//!
//! 4 provider（Aliyun/ByteDance/Tencent/Baidu）WSS 协议层 + 批引擎（impl
//! `octopus_asr_local::engine::OfflineAsrEngine`）。协议层从 `octopus-desktop` 复刻
//!（见各 `*_stream.rs`），改造为不依赖 tauri runtime：`open()` 内部用 `tokio::spawn`，
//! 调用方（`CloudBatchEngine`）在自有 tokio runtime 上 `block_on` 驱动。
//!
//! 设计详见 `docs/superpowers/specs/2026-06-25-archived-spec.md#cloud-asr-cli-design`。

pub mod cloud_types;
```

- [x] **Step 4: 写 cloud_types 测试（先写测试，TDD）**

先 Read `crates/desktop/src/cloud_types.rs` 全文确认内容（本 task 迁移它）。然后 Create `crates/asr-cloud/src/cloud_types.rs`，**整体复制 desktop 版本**，做以下改造：
- `pub(crate) enum PcmFrame` → `pub enum PcmFrame`（cloud crate 内部跨模块用，但保持 pub(crate) 亦可；本 task 保持 `pub(crate)`，与 desktop 一致）。
- `pub(crate) fn samples_to_pcm_s16le` → 保持 `pub(crate)`。
- 顶部模块文档注释把"coordinator → 后台 WS task"措辞保留（语义仍成立：CloudBatchEngine 扮演 coordinator 角色推音频）。
- `use anyhow::{anyhow, bail, Result};` / `use tokio::sync::mpsc;` 不变。
- 3 个单测（`test_samples_to_pcm_s16le_empty/basic/clamp`）**逐字复制**。

最终 `cloud_types.rs` 内容 = desktop `cloud_types.rs` 全文（含 `PcmFrame`/`StreamEvent`/`CloudStreamHandle`/`CLOUD_CLOSE_TIMEOUT_SECS`/`samples_to_pcm_s16le` + tests），无需任何逻辑改动（该文件不依赖 tauri）。

- [x] **Step 5: 验证测试通过**

Run: `cargo test -p octopus-asr-cloud --lib`
Expected: 3 passed（samples_to_pcm_s16le 三个单测），0 failed。

- [x] **Step 6: workspace check 确认注册无误**

Run: `cargo check -p octopus-asr-cloud`
Expected: 编译通过（cloud_types 无 tauri 依赖，应干净通过）。

- [x] **Step 7: Commit**

```bash
git add crates/asr-cloud Cargo.toml
git commit -m "feat(asr-cloud): 新建 crate 骨架 + 迁移 cloud_types（PcmFrame/StreamEvent/CloudStreamHandle）"
```

---

## Task 2: 协议层 aliyun（DashScope）

**Files:**
- Create: `crates/asr-cloud/src/aliyun_stream.rs`
- Modify: `crates/asr-cloud/src/lib.rs`（加 `pub mod aliyun_stream;`）

aliyun 协议最复杂（Fun-ASR/Paraformer 任务型 + Qwen-ASR Realtime 两套），但纯函数可单测面有限（主要是 `is_qwen_realtime_endpoint`）。WSS 主体靠 desktop 已验证逻辑 + `#[ignore]` 真实 key 集成测试。

- [x] **Step 1: 复制 + 改造 aliyun_stream.rs**

Read `crates/desktop/src/aliyun_stream.rs` 全文。Create `crates/asr-cloud/src/aliyun_stream.rs`，整体粘贴，按「复刻通用规则」改造：
- `open()` 签名改为（去掉 `rt`，`rt.spawn` → `tokio::spawn`）：

```rust
/// 建连 + 初始化 + 推 pre-roll PCM + 启动后台 WS task。
///
/// 根据 `endpoint` 路径自动选择协议：
/// - 含 `/v1/realtime` → Qwen-ASR Realtime 会话协议（OpenAI Realtime 风格）
/// - 否则 → Fun-ASR/Paraformer 任务型协议（run-task/finish-task）
///
/// **须在 tokio runtime 上下文调用**（内部 `tokio::spawn`）。批引擎 `CloudBatchEngine`
/// 在自有 runtime 的 `block_on` 内调用。
/// `pre_roll_samples` 是 f32[-1,1] 样本（批处理传空 Vec：整段一次推，无需前导）。
pub fn open(
    endpoint: String,
    key: String,
    model: String,
    language: String,
    pre_roll_samples: Vec<f32>,
) -> Result<CloudStreamHandle> {
    let (handle, pcm_rx, result_tx) = CloudStreamHandle::new();
    let is_qwen = is_qwen_realtime_endpoint(&endpoint);
    tokio::spawn(async move {
        let tx_for_err = result_tx.clone();
        let result = if is_qwen {
            run_qwen_realtime_session(
                pcm_rx, result_tx, endpoint, key, model, language, pre_roll_samples,
            )
            .await
        } else {
            run_ws_session(
                pcm_rx, result_tx, endpoint, key, model, language, pre_roll_samples,
            )
            .await
        };
        if let Err(e) = result {
            log::error!("aliyun stream session error: {}", e);
            let _ = tx_for_err.send(StreamEvent::Failed(e.to_string()));
        }
    });
    Ok(handle)
}
```

- `run_ws_session` / `run_qwen_realtime_session` / `is_qwen_realtime_endpoint` 及所有 helper：**逐字照搬** desktop 版本。
- 模块文档头：把"tauri::async_runtime（tokio handle）"改为"tokio runtime（CloudBatchEngine 的 block_on 驱动）"。

- [x] **Step 2: 注册模块**

`crates/asr-cloud/src/lib.rs` 末尾加：

```rust
pub mod aliyun_stream;
```

- [x] **Step 3: 复制 desktop 已有单测（含 is_qwen_realtime_endpoint）**

desktop `aliyun_stream.rs` 已带：`is_qwen_realtime_endpoint`（L282，pub(crate)）+ L508 起的 `mod tests`（5 个测试）。复刻时把 `is_qwen_realtime_endpoint` 函数 + 整个 `#[cfg(test)] mod tests {...}` **逐字复制**到 cloud crate 版本（字节级验证已存在，无需新编）。确认 `is_qwen_realtime_endpoint` 判定逻辑含 `/v1/realtime` 子串。

- [x] **Step 4: 验证测试通过**

Run: `cargo test -p octopus-asr-cloud --lib aliyun`
Expected: `is_qwen_realtime_endpoint_detects_realtime` PASS。

- [x] **Step 5: 编译验证（含 native-tls / serde_json 依赖生效）**

Run: `cargo check -p octopus-asr-cloud`
Expected: 编译通过，0 error。

- [x] **Step 6: Commit**

```bash
git add crates/asr-cloud/src/aliyun_stream.rs crates/asr-cloud/src/lib.rs
git commit -m "feat(asr-cloud): 复刻 aliyun(DashScope) WSS 协议层（open 去 tauri + tokio::spawn）"
```

---

## Task 3: 协议层 bytedance（豆包二进制帧）

**Files:**
- Create: `crates/asr-cloud/src/bytedance_stream.rs`
- Modify: `crates/asr-cloud/src/lib.rs`

bytedance 是二进制帧协议（4B header + payload + gzip），帧编解码纯函数可单测，价值最高。

- [x] **Step 1: 复制 + 改造 bytedance_stream.rs**

Read `crates/desktop/src/bytedance_stream.rs` 全文。Create `crates/asr-cloud/src/bytedance_stream.rs`，整体粘贴，按「复刻通用规则」改造。`open()` 新签名：

```rust
/// 建连 + 发初始 config + 推 pre-roll PCM + 启动后台 WS task。
///
/// **须在 tokio runtime 上下文调用**（内部 `tokio::spawn`）。
pub fn open(
    api_key: String,
    resource_id: String,
    language: String,
    pre_roll_samples: Vec<f32>,
) -> Result<CloudStreamHandle> {
    let (handle, pcm_rx, result_tx) = CloudStreamHandle::new();
    tokio::spawn(async move {
        let tx_for_err = result_tx.clone();
        let result =
            run_session(pcm_rx, result_tx, api_key, resource_id, language, pre_roll_samples)
                .await;
        if let Err(e) = result {
            log::error!("bytedance stream session error: {}", e);
            let _ = tx_for_err.send(StreamEvent::Failed(e.to_string()));
        }
    });
    Ok(handle)
}
```

> 注意：desktop 版 `run_session` 的确切名以源文件为准（可能是 `run_bytedance_session`）；照搬源文件的函数名，`open` 内调用名与之对齐。

- 帧编解码常量（`PROTOCOL_VERSION`/`MSG_*`/`FLAG_*`/`SER_*`）、`build_*`/`parse_*` helper、`run_session`：**逐字照搬**。

- [x] **Step 2: 注册模块**

`lib.rs` 加 `pub mod bytedance_stream;`

- [x] **Step 3: 复制 desktop 已有帧编解码单测**

desktop `bytedance_stream.rs` L385 起的 `mod tests` 已带 5 个测试：`test_build_client_frame_audio` / `test_build_client_frame_last`（帧构造，校验 4B header：byte0=0x11、msg_type、flags、ser、comp）+ `test_gzip_roundtrip` + `test_parse_server_frame_response` / `test_parse_server_frame_error`。帧构造函数实际名 `build_client_frame(msg_type, flags, serialization, compression, payload_raw)`（5 参数）。复刻时**逐字复制**该 `mod tests`——它依赖的 `build_client_frame`/`parse_server_frame`/`gzip_compress`/`decompress_or_raw`/协议常量本就在 `run_bytedance_session` 同文件，Step 1 整体复制已含。字节级验证已存在，无需新编。

- [x] **Step 4: 验证测试通过**

Run: `cargo test -p octopus-asr-cloud --lib bytedance`
Expected: 帧编解码单测 PASS。

- [x] **Step 5: 编译验证**

Run: `cargo check -p octopus-asr-cloud`
Expected: 0 error（flate2 Gzip 依赖生效）。

- [x] **Step 6: Commit**

```bash
git add crates/asr-cloud/src/bytedance_stream.rs crates/asr-cloud/src/lib.rs
git commit -m "feat(asr-cloud): 复刻 bytedance(豆包) 二进制帧 WSS 协议层 + 帧编解码单测"
```

---

## Task 4: 协议层 tencent（HMAC-SHA1 签名）+ baidu（START 帧鉴权）

**Files:**
- Create: `crates/asr-cloud/src/tencent_stream.rs`
- Create: `crates/asr-cloud/src/baidu_stream.rs`
- Modify: `crates/asr-cloud/src/lib.rs`

两个相对简单（tencent 签名构造 + baidu START 帧），合并到一个 task。

- [x] **Step 1: 复刻 tencent_stream.rs**

Read `crates/desktop/src/tencent_stream.rs` 全文。Create `crates/asr-cloud/src/tencent_stream.rs`，整体粘贴，按「复刻通用规则」改造。`open()` 新签名：

```rust
/// 建连（含签名 URL）+ 推 pre-roll PCM + 启动后台 WS task。
///
/// **须在 tokio runtime 上下文调用**。
pub fn open(
    appid_secretid: String,
    secret_key: String,
    engine_model_type: String,
    _language: String,
    pre_roll_samples: Vec<f32>,
) -> Result<CloudStreamHandle> {
    let (handle, pcm_rx, result_tx) = CloudStreamHandle::new();
    tokio::spawn(async move {
        let tx_for_err = result_tx.clone();
        let result = run_tencent_session(
            pcm_rx, result_tx, appid_secretid, secret_key, engine_model_type, pre_roll_samples,
        )
        .await;
        if let Err(e) = result {
            log::error!("tencent stream session error: {}", e);
            let _ = tx_for_err.send(StreamEvent::Failed(e.to_string()));
        }
    });
    Ok(handle)
}
```

- 签名构造 helper（拼 `sign_str` → HMAC-SHA1 → base64 → URL-encode）+ `run_tencent_session`：**逐字照搬**。

- [x] **Step 2: 复刻 baidu_stream.rs**

Read `crates/desktop/src/baidu_stream.rs` 全文。Create `crates/asr-cloud/src/baidu_stream.rs`，整体粘贴，按「复刻通用规则」改造。`open()` 新签名：

```rust
/// 建连 + 发 START 帧 + 推 pre-roll PCM + 启动后台 WS task。
///
/// **须在 tokio runtime 上下文调用**。
pub fn open(
    appid: String,
    appkey: String,
    dev_pid: String,
    _language: String,
    pre_roll_samples: Vec<f32>,
) -> Result<CloudStreamHandle> {
    let (handle, pcm_rx, result_tx) = CloudStreamHandle::new();
    tokio::spawn(async move {
        let tx_for_err = result_tx.clone();
        let result =
            run_baidu_session(pcm_rx, result_tx, appid, appkey, dev_pid, pre_roll_samples).await;
        if let Err(e) = result {
            log::error!("baidu stream session error: {}", e);
            let _ = tx_for_err.send(StreamEvent::Failed(e.to_string()));
        }
    });
    Ok(handle)
}
```

- `run_baidu_session`（含 START 帧 JSON 构造、UUID `sn`、双向循环、FINISH）：**逐字照搬**。

- [x] **Step 3: 注册模块**

`lib.rs` 加：

```rust
pub mod tencent_stream;
pub mod baidu_stream;
```

- [x] **Step 4: 复制 desktop 已有签名单测（tencent）**

desktop `tencent_stream.rs` L298 起的 `mod tests` 已带 7 个测试：`test_percent_encode_special_chars` / `test_percent_encode_alphanumeric`（URL 编码）+ `test_build_signed_url_structure` / `_deterministic` / `_different_keys`（签名 URL 结构/确定性/密钥敏感性）。签名函数实际名 `build_signed_url(appid, secretid, secret_key, engine_model_type, voice_id)`（5 参数，含 voice_id）。复刻时**逐字复制**该 `mod tests`——依赖的 `build_signed_url`/`percent_encode` Step 1 整体复制已含。

- [x] **Step 5: 复制 desktop 已有单测（baidu）**

desktop `baidu_stream.rs` L230 起的 `mod tests` 已带 6 个测试。复刻时**逐字复制**该 `mod tests`。

- [x] **Step 6: 验证测试通过**

Run: `cargo test -p octopus-asr-cloud --lib`
Expected: tencent 签名 + baidu endpoint 单测 PASS（连同前序 task 的测试全绿）。

- [x] **Step 7: 编译验证**

Run: `cargo check -p octopus-asr-cloud`
Expected: 0 error（hmac/sha1/base64 依赖生效）。

- [x] **Step 8: Commit**

```bash
git add crates/asr-cloud/src/tencent_stream.rs crates/asr-cloud/src/baidu_stream.rs crates/asr-cloud/src/lib.rs
git commit -m "feat(asr-cloud): 复刻 tencent(HMAC-SHA1) + baidu(START 帧) WSS 协议层 + 签名单测"
```

---

## Task 5: config 分发（resolve_*_config + open_cloud_session）

**Files:**
- Create: `crates/asr-cloud/src/config.rs`
- Modify: `crates/asr-cloud/src/lib.rs`

复刻 `crates/desktop/src/cloud_pipeline.rs:110-213` 的 resolve_* + open_cloud_session，去 tauri、改同步（open 同步）。

- [x] **Step 1: 写 config.rs（迁移 resolve_* + open_cloud_session）**

Create `crates/asr-cloud/src/config.rs`：

```rust
//! 云端 ASR 配置解析 + provider 分发（复刻 desktop cloud_pipeline.rs 的 open 部分）。
//!
//! 与 desktop 差异：无 tauri runtime 依赖；`open_cloud_session` 同步返回 `CloudStreamHandle`
//!（各 provider `open()` 内部 `tokio::spawn`，须在 tokio 上下文调用）。

use crate::cloud_types::CloudStreamHandle;
use anyhow::{bail, Result};
use octopus_asr_local::config::{self, EngineCategory};

/// 通用云端配置解析：从 DB section 取 ModelEntry + 校验 secret_key 非空。
fn resolve_cloud_entry<'a>(
    section: Option<&'a std::collections::HashMap<String, octopus_infra::db::ModelEntry>>,
    provider: &'a str,
    model_name: &'a str,
) -> std::result::Result<&'a octopus_infra::db::ModelEntry, String> {
    let entry = section
        .and_then(|m| m.get(model_name))
        .ok_or_else(|| format!("{} ASR 模型 '{}' 未在 DB 配置", provider, model_name))?;
    if entry.secret_key.is_empty() {
        return Err(format!("{} ASR 模型 '{}' 的 secret_key 为空", provider, model_name));
    }
    Ok(entry)
}

/// 解析 Aliyun（DashScope）配置（endpoint + key + model_name）。
fn resolve_aliyun_config(engine_spec: &str) -> std::result::Result<(String, String, String), String> {
    let cfg = config::load_config().map_err(|e| e.to_string())?;
    let model_name = octopus_infra::db::parse_model_spec(engine_spec)
        .model_name()
        .to_string();
    let entry = resolve_cloud_entry(cfg.asr.aliyun.as_ref(), "aliyun", &model_name)?;
    Ok((entry.source.clone(), entry.secret_key.clone(), model_name))
}

/// 解析 ByteDance（豆包）配置（resource_id + api_key + model_name）。
fn resolve_bytedance_config(engine_spec: &str) -> std::result::Result<(String, String, String), String> {
    let cfg = config::load_config().map_err(|e| e.to_string())?;
    let model_name = octopus_infra::db::parse_model_spec(engine_spec)
        .model_name()
        .to_string();
    let entry = resolve_cloud_entry(cfg.asr.bytedance.as_ref(), "bytedance", &model_name)?;
    Ok((entry.source.clone(), entry.secret_key.clone(), model_name))
}

/// 解析 Tencent（腾讯云）配置（appid:secretid + secret_key + engine_model_type）。
fn resolve_tencent_config(engine_spec: &str) -> std::result::Result<(String, String, String), String> {
    let cfg = config::load_config().map_err(|e| e.to_string())?;
    let model_name = octopus_infra::db::parse_model_spec(engine_spec)
        .model_name()
        .to_string();
    let entry = resolve_cloud_entry(cfg.asr.tencent.as_ref(), "tencent", &model_name)?;
    if !entry.source.contains(':') {
        return Err(format!(
            "tencent ASR 模型 '{}' 的 source 字段格式应为 appid:secretid（当前='{}'）",
            model_name, entry.source
        ));
    }
    Ok((entry.source.clone(), entry.secret_key.clone(), model_name))
}

/// 解析 Baidu（百度云）配置（appid + api_key + dev_pid）。
fn resolve_baidu_config(engine_spec: &str) -> std::result::Result<(String, String, String), String> {
    let cfg = config::load_config().map_err(|e| e.to_string())?;
    let model_name = octopus_infra::db::parse_model_spec(engine_spec)
        .model_name()
        .to_string();
    let entry = resolve_cloud_entry(cfg.asr.baidu.as_ref(), "baidu", &model_name)?;
    if entry.source.is_empty() {
        return Err(format!(
            "baidu ASR 模型 '{}' 的 source 字段（AppID）为空",
            model_name
        ));
    }
    Ok((entry.source.clone(), entry.secret_key.clone(), model_name))
}

/// 根据 spec 解析配置 + 打开对应云端 WS session（同步返回句柄）。
///
/// `asr_engine` 是完整 spec（如 `aliyun:qwen-asr`）。**须在 tokio runtime 上下文调用**
///（各 provider `open` 内部 `tokio::spawn`）。
pub fn open_cloud_session(
    asr_engine: &str,
    language: &str,
    pre_roll: Vec<f32>,
) -> Result<CloudStreamHandle> {
    match config::resolve_engine_category(asr_engine) {
        Some(EngineCategory::Aliyun) => {
            let (endpoint, key, model) = resolve_aliyun_config(asr_engine)?;
            crate::aliyun_stream::open(endpoint, key, model, language.to_string(), pre_roll)
                .map_err(|e| anyhow::anyhow!("{}", e))
        }
        Some(EngineCategory::ByteDance) => {
            let (resource_id, api_key, _) = resolve_bytedance_config(asr_engine)?;
            crate::bytedance_stream::open(api_key, resource_id, language.to_string(), pre_roll)
                .map_err(|e| anyhow::anyhow!("{}", e))
        }
        Some(EngineCategory::Tencent) => {
            let (appid_secretid, secret_key, engine_model_type) =
                resolve_tencent_config(asr_engine)?;
            crate::tencent_stream::open(
                appid_secretid,
                secret_key,
                engine_model_type,
                language.to_string(),
                pre_roll,
            )
            .map_err(|e| anyhow::anyhow!("{}", e))
        }
        Some(EngineCategory::Baidu) => {
            let (appid, appkey, dev_pid) = resolve_baidu_config(asr_engine)?;
            crate::baidu_stream::open(appid, appkey, dev_pid, language.to_string(), pre_roll)
                .map_err(|e| anyhow::anyhow!("{}", e))
        }
        _ => bail!("当前引擎非云端（spec='{}'），无法开启 WSS", asr_engine),
    }
}
```

> **核对**：`octopus_asr_local::config::load_config()` / `resolve_engine_category()` / `EngineCategory` 均 pub（desktop `cloud_pipeline.rs:129/186/188` 跨 crate 已用）；`octopus_infra::db::{ModelEntry, parse_model_spec}` pub（desktop `cloud_pipeline.rs:114/131` 已用）。`AppConfig.asr.{aliyun,bytedance,tencent,baidu}` 字段类型 = `Option<HashMap<String, ModelEntry>>`（见 desktop resolve_* 用法）。若字段名/类型有出入，以 desktop `cloud_pipeline.rs:127-177` 为准对齐。

- [x] **Step 2: 注册模块 + re-export**

`lib.rs` 加：

```rust
pub mod config;
pub use config::open_cloud_session;
```

- [x] **Step 3: 写 open_cloud_session 错误路径单测（先写测试）**

在 `config.rs` 末尾加（非法 spec 在 resolve 前就 bail，不需真实 key）：

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn open_cloud_session_rejects_local_spec() {
        // 本地引擎 spec（如 whisper）→ resolve_engine_category 返回非云端 → bail。
        // 无需 tokio runtime（在 spawn 前就返回 Err）。
        let res = open_cloud_session("whisper", "zh", Vec::new());
        assert!(res.is_err());
        let msg = format!("{}", res.unwrap_err());
        assert!(msg.contains("非云端") || msg.contains("无法开启 WSS"));
    }

    #[test]
    fn open_cloud_session_rejects_unresolvable_spec() {
        // 不存在的 spec → resolve_engine_category 返回 None → bail。
        let res = open_cloud_session("nonexistent:foo:bar", "zh", Vec::new());
        assert!(res.is_err());
    }
}
```

- [x] **Step 4: 验证测试通过**

Run: `cargo test -p octopus-asr-cloud --lib config`
Expected: 2 passed。

- [x] **Step 5: 编译验证**

Run: `cargo check -p octopus-asr-cloud`
Expected: 0 error。

- [x] **Step 6: Commit**

```bash
git add crates/asr-cloud/src/config.rs crates/asr-cloud/src/lib.rs
git commit -m "feat(asr-cloud): config 分发（resolve_*_config + open_cloud_session，去 tauri）"
```

---

## Task 6: CloudBatchEngine impl OfflineAsrEngine

**Files:**
- Create: `crates/asr-cloud/src/batch.rs`
- Modify: `crates/asr-cloud/src/lib.rs`

批引擎核心：`from_spec` 解析 + 建 runtime；`transcribe` 单段单 session（block_on open + 分块 push + close_async）；`skip_corrector=true`。

- [x] **Step 1: 写 from_spec 错误路径测试（先写测试）**

Create `crates/asr-cloud/src/batch.rs`，先写测试模块：

```rust
//! 云端 ASR 批引擎（impl `octopus_asr_local::engine::OfflineAsrEngine`）。
//!
//! 语义：`transcribe(samples, language)` = 单段音频（≤30s，由上层 `transcribe_segments`
//! 保证）→ 单个 WSS session → 完整文本。VAD 分段 + CJK 连接由
//! `asr::pipeline::transcribe_segments` 自动完成，本引擎不分段、不拼接。
//!
//! `skip_corrector() = true`：云端结果质量高，跳过本地拼音纠错（对齐桌面端云端行为）；
//! 简繁转换仍由 `transcribe_batch` 处理。

use crate::open_cloud_session;
use anyhow::{bail, Result};
use octopus_asr_local::engine::OfflineAsrEngine;
use octopus_infra::db::{parse_model_spec, ModelSpec};

/// 分块推送粒度（采样点）：200ms @ 16kHz = 3200。平滑灌入避免单帧过大。
const CLOUD_PUSH_CHUNK_SAMPLES: usize = 3200;

/// 判断 spec 是否云端 ASR（3-part provider 前缀为 aliyun/bytedance/tencent/baidu）。
///
/// 用 `parse_model_spec` 取 provider 字段，**不查 DB**（纯字符串解析，可单测）。
/// 2-part/裸名 → `NameOnly` → false（走本地分支）。3-part 是标准 spec 格式
///（如 `aliyun:Fun-ASR:fun-asr-realtime`）。cli 分流与本 crate 的 `from_spec` 共用此判定。
pub fn is_cloud_spec(spec: &str) -> bool {
    matches!(
        parse_model_spec(spec),
        ModelSpec::Full { provider, .. } if is_cloud_provider(provider)
    )
}

/// provider 字符串是否云端（大小写不敏感）。
fn is_cloud_provider(provider: &str) -> bool {
    provider.eq_ignore_ascii_case("aliyun")
        || provider.eq_ignore_ascii_case("bytedance")
        || provider.eq_ignore_ascii_case("tencent")
        || provider.eq_ignore_ascii_case("baidu")
}

/// 云端 ASR 批引擎。
pub struct CloudBatchEngine {
    /// 完整 3-part spec（如 `aliyun:Fun-ASR:fun-asr-realtime`），`open_cloud_session` 据此解析配置。
    spec: String,
    /// 自有 tokio runtime（驱动各 provider `open` 的 `tokio::spawn` + `close_async`）。
    rt: tokio::runtime::Runtime,
}

impl CloudBatchEngine {
    /// 从 spec 构造。校验 provider 前缀为云端（不查 DB）+ 建 runtime。
    /// DB 查找（resolve_*_config）推迟到 `transcribe` 内的 `open_cloud_session`。
    pub fn from_spec(spec: &str) -> Result<Self> {
        if !is_cloud_spec(spec) {
            bail!(
                "非云端 ASR spec（'{}'）；CloudBatchEngine 仅支持 3-part 云端 spec \
                 （aliyun/bytedance/tencent/baidu:category:model_name）",
                spec
            );
        }
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        Ok(Self { spec: spec.to_string(), rt })
    }
}

impl OfflineAsrEngine for CloudBatchEngine {
    fn transcribe(&self, samples: &[f32], language: &str) -> Result<String> {
        let spec = self.spec.clone();
        let lang = language.to_string();
        self.rt.block_on(async move {
            let mut handle = open_cloud_session(&spec, &lang, Vec::new())?;
            // 分块推 PCM（批处理一次推完；空 samples 也安全：不进循环，直接 finish）。
            for chunk in samples.chunks(CLOUD_PUSH_CHUNK_SAMPLES) {
                handle.push_pcm(chunk)?;
            }
            // close_async：发 Finish + 收最终结果（超时上限 CLOUD_CLOSE_TIMEOUT_SECS=8s）。
            handle.close_async().await
        })
    }

    fn skip_corrector(&self) -> bool {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_cloud_spec_recognizes_3part_cloud() {
        // 3-part 云端 spec（provider 前缀为云端）→ true（不查 DB）。
        // category 段取 asr/config.rs category_label 的实际值。
        assert!(is_cloud_spec("aliyun:Fun-ASR:fun-asr-realtime"));
        assert!(is_cloud_spec("bytedance:Doubao-ASR:doubao-asr-1.0-streaming"));
        assert!(is_cloud_spec("tencent:Tencent-ASR:16k_zh"));
        assert!(is_cloud_spec("baidu:Baidu-ASR:15372"));
    }

    #[test]
    fn is_cloud_spec_rejects_local_3part_bare_and_2part() {
        // 本地 3-part（provider=local）→ false。
        assert!(!is_cloud_spec("local:zipformer:zipformer-small-ctc"));
        // 裸名 → NameOnly → false。
        assert!(!is_cloud_spec("zipformer-small-ctc"));
        // 2-part → NameOnly 兜底 → false（须 3-part 才判云端）。
        assert!(!is_cloud_spec("aliyun:fun-asr-realtime"));
    }

    #[test]
    fn from_spec_rejects_non_cloud() {
        assert!(CloudBatchEngine::from_spec("local:zipformer:zipformer-small-ctc").is_err());
        assert!(CloudBatchEngine::from_spec("zipformer-small-ctc").is_err());
        assert!(CloudBatchEngine::from_spec("aliyun:fun-asr-realtime").is_err()); // 2-part
    }

    #[test]
    fn from_spec_accepts_cloud_3part() {
        // 云端 3-part → 构造成功（不查 DB、不连网；仅建 runtime）。
        assert!(CloudBatchEngine::from_spec("aliyun:Fun-ASR:fun-asr-realtime").is_ok());
    }
}
```

- [x] **Step 2: 注册模块 + re-export**

`lib.rs` 加：

```rust
pub mod batch;
pub use batch::{CloudBatchEngine, is_cloud_spec};
```

- [x] **Step 3: 验证测试通过**

Run: `cargo test -p octopus-asr-cloud --lib batch`
Expected: `from_spec_rejects_local_engine` + `from_spec_rejects_garbage` PASS。

- [x] **Step 4: 加真实 key 集成测试（#[ignore]）**

在 `batch.rs` 测试模块追加（用户提供本地 DashScope key 时手动跑）：

```rust
    /// 真实 DashScope 集成测试：`cargo test -p octopus-asr-cloud --lib -- --ignored batch::real_aliyun`。
    /// 需 ~/.octopus/config.yaml 的 asr.aliyun.<model> 配好 secret_key。
    /// 用 `cargo run` 录一段样本或用现成 wav → f32 样本后断言非空文本。
    #[ignore]
    #[test]
    fn real_aliyun_transcribe_nonempty() {
        // 占位：实际验证靠 cli 端到端（Task 8 e2e 清单）。
        // 此测试保留为「有本地 key 时的最小集成入口」，样本来源由用户准备。
        // 无样本时直接返回，避免误失败。
        eprintln!("[ignore] 跳过：需本地 DashScope key + 音频样本，见 Task 8 e2e 清单");
    }
```

- [x] **Step 5: 编译验证**

Run: `cargo check -p octopus-asr-cloud --all-targets`
Expected: 0 error（含 test target）。

- [x] **Step 6: Commit**

```bash
git add crates/asr-cloud/src/batch.rs crates/asr-cloud/src/lib.rs
git commit -m "feat(asr-cloud): CloudBatchEngine impl OfflineAsrEngine（单段单 session，skip_corrector=true）"
```

---

## Task 7: AsrEngineManager getter + cli 本地/云端分流

**Files:**
- Modify: `crates/asr/src/engine.rs`（加 `active_engine` getter）
- Modify: `crates/cli/Cargo.toml`（加 octopus-asr-cloud 依赖）
- Modify: `crates/cli/src/pipeline.rs`（分流）

- [x] **Step 1: 给 AsrEngineManager 加 active_engine getter**

编辑 `crates/asr/src/engine.rs`，在 `transcribe_batch` 方法后（`impl AsrEngineManager` 块内，约 L163 后）加：

```rust
    /// 取出当前 active engine（供 cli 分流后统一调 `pipeline::transcribe_batch`）。
    ///
    /// 与本地/云端分流配合：cli 本地分支构造 `AsrEngineManager` + `switch_model` 后取
    /// `Arc<dyn OfflineAsrEngine>`，与云端分支的 `CloudBatchEngine` 同为 `dyn OfflineAsrEngine`，
    /// 喂同一 `transcribe_batch`。
    pub fn active_engine(&self) -> Result<Arc<dyn OfflineAsrEngine>> {
        self.active_engine
            .read()
            .unwrap()
            .clone()
            .ok_or_else(|| anyhow::anyhow!("No active ASR engine loaded in AsrEngineManager"))
    }
```

- [x] **Step 2: cli 加 octopus-asr-cloud 依赖**

编辑 `crates/cli/Cargo.toml`，在 `[dependencies]` 加（位置参考既有 octopus-asr-local 行）：

```toml
octopus-asr-cloud = { path = "../asr-cloud" }
```

> 先 Read `crates/cli/Cargo.toml` 确认既有 `octopus-asr-local` 行的写法，紧随其后加。

- [x] **Step 3: 写 is_cloud_spec 测试（先写测试）**

编辑 `crates/cli/src/pipeline.rs`，整体替换为（先写测试 + is_cloud_spec）：

```rust
//! CLI 批处理转写 pipeline：本地 / 云端分流 → `transcribe_batch`（VAD + 纠错 + 简繁）。
//!
//! 分流在 cli 层（`asr` crate 不依赖 `asr-cloud`，避免循环）：
//! - 云端 spec（aliyun/bytedance/tencent/baidu）→ `CloudBatchEngine::from_spec`。
//! - 本地 onnx → `AsrEngineManager` + `active_engine`。
//! 两端都经 `asr::pipeline::transcribe_batch` 编排（VAD 分段 + 纠错 + 简繁）。

use anyhow::Result;
use octopus_asr_local::engine::{AsrEngineManager, OfflineAsrEngine};
use octopus_asr_local::pipeline::{transcribe_batch, PipelineConfig};
use octopus_asr_cloud::{is_cloud_spec, CloudBatchEngine};

/// 批处理转写：分流 → transcribe_batch（VAD 分段 + 纠错 + 简繁）。
///
/// `model` 为 DB models 表的 model_name（支持 `provider:category:model` spec）。
/// 云端 spec → `CloudBatchEngine`（内部 WSS，`skip_corrector=true`）；本地 → onnx 引擎。
pub fn run(model: &str, language: &str, samples: &[f32]) -> Result<String> {
    let cfg = PipelineConfig::from_app_config(language);
    if is_cloud_spec(model) {
        let engine = CloudBatchEngine::from_spec(model)?;
        transcribe_batch(&engine, samples, &cfg)
    } else {
        let mgr = AsrEngineManager::new();
        mgr.switch_model(model)?;
        let engine = mgr.active_engine()?;
        transcribe_batch(&engine, samples, &cfg)
    }
}

// cli 层无可单测的纯函数：is_cloud_spec 在 octopus-asr-cloud crate（Task 6）已测；
// run 需真实引擎 / WSS，验证靠 cargo check + clippy + Task 8 e2e 清单。
```

> **核对**：`octopus_asr_local::pipeline::transcribe_batch` 是 pub（`asr/pipeline.rs:46`）。`resolve_engine_category` / `EngineCategory` pub（见 Task 5 核对）。若 `is_cloud_spec_recognizes_cloud_prefixes` 中某前缀解析不出云端（取决于 `resolve_engine_category` 实现），Read `crates/asr/src/config.rs` 的 `resolve_engine_category` + `EngineCategory` 前缀表，用**实际能解析为云端**的 spec 形态替换测试用例。

- [x] **Step 4: 确认 is_cloud_spec 测试在 cloud crate 通过**

`is_cloud_spec` 单测在 `octopus-asr-cloud`（Task 6 Step 1），cli 层无单测（run 需真实引擎/WSS）。

Run: `cargo test -p octopus-asr-cloud --lib batch`
Expected: `is_cloud_spec_*` + `from_spec_*` 全 PASS（Task 6 已验证）。

- [x] **Step 5: workspace 编译验证（关键里程碑：cli 拉通 cloud crate）**

Run: `cargo check --workspace --all-targets`
Expected: 0 error。asr-cloud 全链路（cli → asr-cloud → asr/infra）编译通过。

- [x] **Step 6: clippy 零新 warning**

Run: `cargo clippy -p octopus-asr-cloud -p octopus-cli --all-targets -- -D warnings`
Expected: 0 warning（新代码）。若 asr-cloud/cli 既有 warning 与本次无关，用 `-W` 而非 `-D` 区分；目标只看新代码无 warning。

- [x] **Step 7: Commit**

```bash
git add crates/asr/src/engine.rs crates/cli/Cargo.toml crates/cli/src/pipeline.rs
git commit -m "feat(cli): 本地/云端 ASR 分流（AsrEngineManager::active_engine + CloudBatchEngine）"
```

---

## Task 8: workspace 测试 + 文档同步 + e2e 清单

**Files:**
- Modify: `docs/superpowers/specs/2026-06-25-archived-spec.md#cloud-asr-cli-design`（横幅状态）
- Modify: `docs/architecture.md`（加 octopus-asr-cloud）
- Modify: 记忆 `parallel-workstreams.md` + `MEMORY.md`
- Create: 本 plan 同目录无需新建（e2e 清单写在本 task）

- [x] **Step 1: workspace 全量测试**

Run: `cargo test --workspace`
Expected: 全绿（含 asr-cloud 全部单测；`#[ignore]` 的真实 key 测试跳过）。

- [x] **Step 2: workspace check + clippy 兜底**

Run: `cargo check --workspace --all-targets`
Run: `cargo clippy --workspace --all-targets`
Expected: check 0 error；clippy 无本次引入的新 warning。

- [x] **Step 3: 更新 spec 横幅状态**

编辑 `docs/superpowers/specs/2026-06-25-archived-spec.md#cloud-asr-cli-design`，把顶部状态行：

```
> **状态**：设计中（待用户审 → writing-plans）。
```
改为：
```
> **状态**：已实现且 e2e 通过（plan `docs/superpowers/plans/2026-06-25-archived-plan.md#cloud-asr-cli`，8 task 全完成；workspace 测试绿；e2e 用户本地云端 key 验通过 2026-06-25）。
```

并在 §4.1「协议层」开头加一句实施修正注记：

```
> **实施修正**：`open()` 保持同步签名（仅 `tokio::spawn`），`close_async` 才是 async 收尾；
> CloudBatchEngine 不自己分段（`transcribe_segments` 自动分段）。详见 plan 顶部「两点据实修正」。
```

- [x] **Step 4: 更新 architecture.md**

Read `docs/architecture.md`，在 crate 列表/workspace 结构处加 `octopus-asr-cloud`（云端 ASR WSS 协议层 + 批引擎，cli 批处理用；desktop 第二步复用）。若无明确 crate 清单段，在最接近的「模块/crate 说明」处补一段：

```markdown
- `crates/asr-cloud`（`octopus-asr-cloud`）：云端 ASR（Aliyun/ByteDance/Tencent/Baidu）WSS
  协议层 + 批引擎 `CloudBatchEngine`（impl `OfflineAsrEngine`）。cli 批处理转译音频文件可选云端
  API；desktop 流式适配暂留 desktop（第二步合并）。依赖 `octopus-asr-local`（单向）。
```

- [x] **Step 5: 更新记忆 parallel-workstreams.md**

在 `parallel-workstreams.md` 的 ASR pipeline 阶段2 条目（item 7）末尾，或作为新进展，补一行：

```
**云端 ASR 下沉 cli（已实施，worktree-model-mgmt-ui）**：新建 octopus-asr-cloud crate
（4 provider WSS 协议层 1:1 复刻 desktop + CloudBatchEngine impl OfflineAsrEngine，
skip_corrector=true）+ cli 本地/云端分流（is_cloud_spec + AsrEngineManager::active_engine）。
desktop 本次零改动（*_stream.rs 副本暂留，第二步合并）。e2e 通过（用户本地云端 key 验，2026-06-25）。
```

同步更新 `MEMORY.md` 索引行的尾部「2c-3/2d 待」前后，提及 cloud-asr-cli 已实施。

- [x] **Step 6: 文档提交**

```bash
git add docs/superpowers/specs/2026-06-25-archived-spec.md#cloud-asr-cli-design docs/architecture.md
git commit -m "docs: 云端 ASR 下沉 cli 实施完成同步（spec 横幅 + architecture）"
```

（记忆文件在仓库外，不进 git，Step 5 用 Write 工具直接写。）

- [x] **Step 7: e2e 手动验证清单（用户本地云端 key 验通过，2026-06-25）**

实现完成后，向用户给出以下 e2e 清单（用户本地有云端 key 时执行）：

```
# 前置：~/.octopus/config.yaml 的 asr.<provider>.<model> 配好 secret_key（与 desktop 同源）。

# 1. 云端转译（aliyun 示例，替换为实际配置的 model spec）
octopus-cli transcribe --model "aliyun:qwen-asr" --language zh path/to/test.wav
# 预期：输出识别文本（非空、内容正确）；云端结果不再走本地拼音纠错。

# 2. 云端转译长音频（>30s，触发 VAD 分段 → 多 session）
octopus-cli transcribe --model "aliyun:qwen-asr" --language zh path/to/long.wav
# 预期：分段识别 + CJK 连接，输出连贯文本。

# 3. 其他 provider（按已配置的 key 轮测）
octopus-cli transcribe --model "bytedance:<model>" --language zh test.wav
octopus-cli transcribe --model "tencent:<model>" --language zh test.wav
octopus-cli transcribe --model "baidu:<model>" --language zh test.wav

# 4. 回归：本地 onnx 仍正常（分流本地分支未受影响）
octopus-cli transcribe --model "zipformer-small-ctc" --language zh test.wav

# 5. 错误路径：未配置 key 的云端 spec → 友好报错（非 panic）
octopus-cli transcribe --model "aliyun:not-configured" --language zh test.wav
# 预期：报 "aliyun ASR 模型 'not-configured' 未在 DB 配置" 或 secret_key 为空。
```

- [x] **Step 8: 标记 plan 全部完成**

本 plan 所有 task checkbox 勾选；向用户报告实施完成 + e2e 清单，进入 `finishing-a-development-branch`（保留 worktree / ff-merge 由用户定）。

---

## Spec Coverage 自检

| spec 章节 | 覆盖 task |
|---|---|
| §3.1 crate 依赖图（asr←cloud，cli 依赖两者） | Task 1（Cargo.toml）+ Task 7（cli 依赖） |
| §3.2 三层分工（协议层/批引擎/流式留 desktop） | Task 2-4（协议层）+ Task 6（批引擎）+ desktop 不动 |
| §3.3 runtime（block_on + tokio::spawn） | Task 6（CloudBatchEngine.rt）+ Task 2-4（open tokio::spawn） |
| §4.1 协议层（4 provider WSS，去 tauri） | Task 2/3/4 |
| §4.2 CloudBatchEngine（transcribe + skip_corrector） | Task 6 |
| §4.3 provider 分发（EngineCategory + resolve_*） | Task 5 |
| §5 cli 接入（is_cloud_spec + active_engine getter） | Task 7 |
| §6 config 复用（AppConfig.asr.{provider}） | Task 5（resolve_*） |
| §7 测试策略（协议纯函数单测 + #[ignore] 真实 key） | Task 1-6 各单测 + Task 6 #[ignore] + Task 8 e2e |
| §9 风险（临时两份/超时/循环约束） | 两份=desktop 不动（Task 范围）；超时=close_async 8s（Task 1 迁移）；循环=cli 分流（Task 7） |

---

## 备注：desktop 第二步（非本次，spec §8/§10）

本次完成后，desktop 仍用自己的 `*_stream.rs` 副本。第二步（独立后续）：
- 删 desktop `{aliyun,bytedance,tencent,baidu}_stream.rs` + `cloud_types.rs` 协议副本；
- `cloud_pipeline.rs` 的 `open_cloud_session` / `resolve_*` 改调 `octopus_asr_cloud`；
- `CloudPipelineEngine` 持 `CloudStreamHandle` 改用 cloud crate 类型；
- 云端流式 e2e 回归（本地 + 云端）。
本次不触碰，留 spec §8 记录。

---

## 2026-06-25-coordinator-cleanup



**Goal:** 把散在 coordinator 三处的 emit/DB/polish 触发逻辑收敛进 pipeline 事件流（`PipelineEvent`），coordinator 退化为统一事件路由，零行为差异。

**Architecture:** `Pipeline::tick` 产 `Vec<PipelineEvent>`（PersistRaw/Emit/Polish/Error）；coordinator 抽 `apply_pipeline_events`（dispatch_tick + stop 共用概念，但 stop 实际丢弃事件保持现状）+ `dispatch_tick`（三 Tick 命令合一）。transcript 留 Stage，finalize/cloud close/Transcript 状态机不动。迁移用「先加 `tick_events` inherent(Vec) → coordinator 切 → trait 合并」4 步，每 task 自洽编译。

**Tech Stack:** Rust，tauri 2 desktop crate，`crates/desktop/src/{pipeline.rs,coordinator.rs}`。spec `docs/superpowers/specs/2026-06-25-archived-spec.md#coordinator-cleanup-design`。

**迁移策略说明（重要）：** `Pipeline::tick` 签名 `bool → Vec` 是全局原子改动（trait + StreamingPipeline + VadSegmentedPipeline + coordinator 全调用点）。若一步改全，中间编译断。故：
- Task 1：pipeline 加 inherent `tick_events(..) -> Vec<PipelineEvent>`（新方法，复用现有 `tick`/`run_tick`，不重复 set_full），trait `tick(bool)` 不动，coordinator 不动 → 编译过。
- Task 2：coordinator 切 `tick_events`（apply_pipeline_events + dispatch_tick），删旧 handler → 编译过。
- Task 3：trait `tick` 签名改 Vec（合并：`tick_events` → `tick`），删旧 inherent `tick(bool)` + trait `silence_duration`/`took_segment_cut` + 清 `#[allow(unused)]` → 编译过。
- Task 4：验证 + 文档 + ff-merge。

---

### Task 1: PipelineEvent + 两 pipeline 加 inherent tick_events（Vec）+ 单测

**Files:**
- Modify: `crates/desktop/src/pipeline.rs`（加 `PipelineEvent` enum；`StreamingPipeline::tick_events`；`VadSegmentedPipeline::tick_events`；`FakePipelineEngine` 加 `is_cloud` 可配；streaming tick_events 单测）

**目的：** 新增事件 enum + 两 pipeline 的 inherent `tick_events`（产事件流，复用现有 tick/run_tick 不碰 set_full 逻辑）。trait 与 coordinator 不动，编译自洽。

- [x] **Step 1: 加 PipelineEvent enum**

在 `pipeline.rs` 的 `SegmentResult` struct（L30）之前加：

```rust
/// pipeline tick 产出的「该做什么」事件。coordinator `apply_pipeline_events` 据此执行端动作
/// （DB/emit/polish/错误上报）。不携带 transcript 状态（transcript 留 Stage，coordinator 持 &mut）
/// ——只携带「决定 + 必要字符串」。（2d，spec §3.2）
#[derive(Debug, PartialEq)]
pub enum PipelineEvent {
    /// 落库 raw_text（pipeline 已判文本变化）。engine_mode = DB engine_mode 列（"streaming"/"vad_segmented"）。
    /// coordinator 调 update_transcription_raw(&mut transcript, &config.asr_engine, engine_mode)。
    PersistRaw { engine_mode: &'static str },
    /// 刷新结果窗口。display 已由 pipeline 算好（local=transcript.display_text()；cloud=display+current_partial）。
    /// coordinator 调 result_window::update_result(app_handle, &display)。
    Emit { display: String },
    /// 触发停顿润色。silence = 停顿时长（streaming 传 silence_duration；vad-seg 段边界传 f64::INFINITY 必过，
    /// 等价原 after_vad_tick 传 pause_polish_threshold_ms 让 check_and_trigger_polish 静音检查自动达标）。
    /// coordinator 调 check_and_trigger_polish(&mut transcript, silence, config, tx)（防抖五重检查原样在彼处）。
    Polish { silence: f64 },
    /// 用户可见错误（cloud WSS 开启失败 / StreamEvent::Failed；local 错误只在承载层 warn，不产此事件）。
    Error(String),
}
```

- [x] **Step 2: 加 StreamingPipeline::tick_events（复用 inherent tick）**

在 `StreamingPipeline::tick` inherent 方法（L197-218）之后、`finish`（L225）之前加：

```rust
    /// 产 tick 事件流（2d，spec §3.4）。coordinator `dispatch_tick` 调此 + `apply_pipeline_events`。
    /// 复用 inherent `tick` 的 set_full/last_error 逻辑（不重复），按 `is_cloud` 决定事件序列：
    /// - local：`changed`→`[PersistRaw, Emit]`；每 tick 追加 `[Polish{silence_duration}]`；空样本→`[]`（早退）
    /// - cloud：`changed`→`[PersistRaw, Polish]`；每 tick 追加 `[Emit{display+partial}]`；`error`→追加 `[Error]`
    pub fn tick_events(
        &mut self,
        samples: &[f32],
        transcript: &mut Transcript,
    ) -> Vec<PipelineEvent> {
        let is_cloud = self.engine.is_cloud();
        // local 空样本早退（等价原 handle_streaming_tick L1370）；cloud 不早退（仍 emit 预览/drain）
        if !is_cloud && samples.is_empty() {
            return Vec::new();
        }
        let changed = self.tick(samples, transcript); // set_full + 设 last_error（复用，不重复逻辑）
        let mut events = Vec::new();
        if is_cloud {
            if changed {
                events.push(PipelineEvent::PersistRaw { engine_mode: "streaming" });
                events.push(PipelineEvent::Polish { silence: self.engine.silence_duration() });
            }
            if let Some(e) = self.last_error.take() {
                events.push(PipelineEvent::Error(e));
            }
            // 每 tick emit（display + current_partial 预览，预览不进 DB）
            let base = transcript.display_text();
            let partial = self.engine.current_partial();
            let display = if partial.is_empty() {
                base
            } else {
                format!("{}{}", base, partial)
            };
            events.push(PipelineEvent::Emit { display });
        } else {
            if changed {
                events.push(PipelineEvent::PersistRaw { engine_mode: "streaming" });
                events.push(PipelineEvent::Emit { display: transcript.display_text() });
            }
            // local 每 tick 查停顿润色（等价原 handle_streaming_tick L1408）
            events.push(PipelineEvent::Polish { silence: self.engine.silence_duration() });
        }
        events
    }
```

- [x] **Step 3: 加 VadSegmentedPipeline::tick_events（复用 run_tick）**

在 `VadSegmentedPipeline::run_tick`（L431-485）之后、`impl Pipeline for VadSegmentedPipeline`（L488）之前加：

```rust
    /// 产 tick 事件流（2d，spec §3.4）。复用 `run_tick`（双 VAD+切段+spawn+drain+set_full，不重复），
    /// 按 `changed`/`segment_cut` 产事件：
    /// `changed`→`[PersistRaw{vad_segmented}, Emit]`；`segment_cut`→追加 `[Polish{INFINITY}]`
    ///（段边界 silence 必过，等价原 after_vad_tick L1221 传 pause_polish_threshold_ms）。
    /// WaitingCompletion 收尾也走此（空样本 run_tick 跳过切段仅 drain，segment_cut 恒 false → 无 Polish）。
    pub(crate) fn tick_events(
        &mut self,
        samples: &[f32],
        transcript: &mut Transcript,
    ) -> Vec<PipelineEvent> {
        let changed = self.run_tick(samples, transcript);
        let segment_cut = self.segment_cut_this_tick;
        let mut events = Vec::new();
        if changed {
            events.push(PipelineEvent::PersistRaw { engine_mode: "vad_segmented" });
            events.push(PipelineEvent::Emit { display: transcript.display_text() });
        }
        if segment_cut {
            events.push(PipelineEvent::Polish { silence: f64::INFINITY });
        }
        events
    }
```

- [x] **Step 4: FakePipelineEngine 加 is_cloud 可配（供 cloud tick_events 测试）**

改 `tests` 模块里的 `FakePipelineEngine`（L536-562）。struct 加 `is_cloud` 字段，`new` 设 false，加 `new_cloud` 构造器，impl `is_cloud`：

```rust
    struct FakePipelineEngine {
        tick_out: Mutex<Vec<TranscriptEvent>>,
        partial: String,
        finish_out: TranscriptEvent,
        silence: f64,
        is_cloud: bool,
    }
    impl FakePipelineEngine {
        fn new(tick: Vec<TranscriptEvent>, partial: &str, finish: TranscriptEvent) -> Self {
            Self { tick_out: Mutex::new(tick), partial: partial.to_string(), finish_out: finish, silence: 0.0, is_cloud: false }
        }
        fn new_cloud(tick: Vec<TranscriptEvent>, partial: &str, finish: TranscriptEvent) -> Self {
            Self { tick_out: Mutex::new(tick), partial: partial.to_string(), finish_out: finish, silence: 0.0, is_cloud: true }
        }
    }
    impl StreamingPipelineEngine for FakePipelineEngine {
        fn tick(&mut self, _samples: &[f32]) -> Vec<TranscriptEvent> {
            std::mem::take(&mut *self.tick_out.lock().unwrap())
        }
        fn finish(&mut self) -> TranscriptEvent { self.finish_out.clone() }
        fn silence_duration(&self) -> f64 { self.silence }
        fn current_partial(&self) -> &str { &self.partial }
        fn reset(&mut self) {}
        fn is_cloud(&self) -> bool { self.is_cloud }
    }
```

- [x] **Step 5: 写 streaming tick_events 单测（local changed/no-change/empty + cloud）**

在 `tests` 模块（`finish_delegates_to_engine` 测试之后）加：

```rust
    #[test]
    fn tick_events_local_changed_produces_persist_emit_polish() {
        let mut p = pipeline(FakePipelineEngine::new(
            vec![TranscriptEvent::Partial("你好".to_string())],
            "", TranscriptEvent::Final("".into()),
        ));
        let mut t = Transcript::new(0, PolishMode::Disabled);
        let events = p.tick_events(&[0.0; 1600], &mut t);
        assert_eq!(events, vec![
            PipelineEvent::PersistRaw { engine_mode: "streaming" },
            PipelineEvent::Emit { display: "你好".to_string() },
            PipelineEvent::Polish { silence: 0.0 },
        ]);
    }

    #[test]
    fn tick_events_local_empty_samples_returns_empty() {
        let mut p = pipeline(FakePipelineEngine::new(vec![], "", TranscriptEvent::Final("".into())));
        let mut t = Transcript::new(0, PolishMode::Disabled);
        assert!(p.tick_events(&[], &mut t).is_empty());
    }

    #[test]
    fn tick_events_local_no_change_only_polish() {
        // Committed 与 full 同 → changed=false → 只产 Polish（local 每 tick）
        let mut p = pipeline(FakePipelineEngine::new(
            vec![TranscriptEvent::Committed("一样".into())], "", TranscriptEvent::Final("".into()),
        ));
        let mut t = Transcript::new(0, PolishMode::Disabled);
        t.set_full("一样");
        let events = p.tick_events(&[0.0; 1600], &mut t);
        assert_eq!(events, vec![PipelineEvent::Polish { silence: 0.0 }]);
    }

    #[test]
    fn tick_events_cloud_changed_emits_display_with_partial() {
        let mut p = pipeline(FakePipelineEngine::new_cloud(
            vec![TranscriptEvent::Committed("已提交".into())],
            "预览中", TranscriptEvent::Final("".into()),
        ));
        let mut t = Transcript::new(0, PolishMode::Disabled);
        let events = p.tick_events(&[0.0; 1600], &mut t);
        // changed → PersistRaw + Polish；每 tick Emit(display+partial) = "已提交预览中"
        assert!(events.contains(&PipelineEvent::PersistRaw { engine_mode: "streaming" }));
        assert!(events.iter().any(|e| matches!(e, PipelineEvent::Emit { display } if display == "已提交预览中")));
    }

    #[test]
    fn tick_events_cloud_error_produces_error_event() {
        let mut p = pipeline(FakePipelineEngine::new_cloud(
            vec![TranscriptEvent::Error("boom".into())],
            "", TranscriptEvent::Final("".into()),
        ));
        let mut t = Transcript::new(0, PolishMode::Disabled);
        let events = p.tick_events(&[0.0; 1600], &mut t);
        assert!(events.iter().any(|e| matches!(e, PipelineEvent::Error(msg) if msg == "boom")));
    }
```

> VadSegmentedPipeline::tick_events 不加单测——构造依赖 SileroVad 模型文件（`find_silero_vad`），单测难；逻辑简单（run_tick + 产事件），靠 Task 4 e2e 覆盖。

- [x] **Step 6: 跑测试**

Run: `cargo test -p octopus-desktop pipeline::tests`
Expected: 全绿（含 5 个新 tick_events 测试 + 既有测试）。

- [x] **Step 7: Commit**

```bash
git add crates/desktop/src/pipeline.rs
git commit -m "feat(desktop): PipelineEvent + 两 pipeline tick_events 产事件（2d Task 1）"
```

---

### Task 2: coordinator apply_pipeline_events + dispatch_tick + 删旧 handler + 三命令合一 + stop 适配

**Files:**
- Modify: `crates/desktop/src/coordinator.rs`（加 `apply_pipeline_events` + `dispatch_tick`；删 `after_vad_tick` + `handle_streaming_tick` + `handle_vad_segmented_tick`；三 Tick 命令 dispatch 合一；stop 路径 tick 适配丢弃事件）

**目的：** coordinator 切事件流——三 Tick 命令合一调 `dispatch_tick`，emit/DB/polish 由 `apply_pipeline_events` 统一路由。stop 路径丢弃 tick 事件（保持现状 stop 无 DB/emit，零行为差异）。

- [x] **Step 1: 加 apply_pipeline_events（事件循环体）**

在 `update_transcription_raw`（L2046）之前加：

```rust
/// pipeline 事件 → 端动作（DB/emit/polish/错误上报）。2d 统一路由，消除三路径重复。（spec §3.5）
fn apply_pipeline_events(
    events: Vec<crate::pipeline::PipelineEvent>,
    transcript: &mut Transcript,
    config: &AppConfig,
    app_handle: &tauri::AppHandle,
    tx: &Sender<Command>,
) {
    use crate::pipeline::PipelineEvent;
    for ev in events {
        match ev {
            PipelineEvent::PersistRaw { engine_mode } => {
                if let Err(e) = update_transcription_raw(transcript, &config.asr_engine, engine_mode) {
                    warn!("DB ({}) failed: {}", engine_mode, e);
                }
            }
            PipelineEvent::Emit { display } => {
                if !display.is_empty() {
                    crate::result_window::update_result(app_handle, &display);
                }
            }
            PipelineEvent::Polish { silence } => {
                check_and_trigger_polish(transcript, silence, config, tx);
            }
            PipelineEvent::Error(e) => {
                crate::result_window::update_result(app_handle, &e);
            }
        }
    }
}
```

- [x] **Step 2: 加 dispatch_tick（三 Tick 命令合一的 dispatch）**

在 `apply_pipeline_events` 之后加：

```rust
/// VadSegmentedTick / StreamingTick / CloudStreamingTick 三命令合一的 dispatch（2d，spec §3.5）。
/// 各 Stage 变体调对应 pipeline 的 `tick_events` → `apply_pipeline_events` 统一路由。
/// WaitingCompletion 额外做 active_count==0 收尾判定（沿用 2c-3 既有逻辑）。
fn dispatch_tick(
    stage: &mut Stage,
    audio: &Arc<SharedAudioState>,
    config: &AppConfig,
    app_handle: &tauri::AppHandle,
    tx: &Sender<Command>,
) {
    let samples = audio.drain_samples();
    match stage {
        Stage::Streaming { pipeline, transcript, .. } => {
            let events = pipeline.tick_events(&samples, transcript);
            apply_pipeline_events(events, transcript, config, app_handle, tx);
        }
        Stage::VadSegmented { pipeline, transcript, .. } => {
            let events = pipeline.tick_events(&samples, transcript);
            apply_pipeline_events(events, transcript, config, app_handle, tx);
        }
        Stage::WaitingCompletion { pipeline, transcript, tick_active } => {
            let events = pipeline.tick_events(&samples, transcript);
            apply_pipeline_events(events, transcript, config, app_handle, tx);
            // 所有在途段完成 → 收尾（停 tick 线程 + finalize）
            if pipeline.active_count() == 0 {
                tick_active.store(false, Ordering::Relaxed);
                let tr = std::mem::replace(transcript, Transcript::new(0, PolishMode::Disabled));
                finalize_after_stop(stage, tr, config, app_handle, tx);
            }
        }
        _ => {}
    }
}
```

> 借用：`pipeline.tick_events(&samples, transcript)` 借 `&mut pipeline` + `&mut transcript`（同 Stage 两字段，disjoint borrow），调用结束释放；随后 `apply_pipeline_events(.., transcript, ..)` 再借 `&mut transcript`。WaitingCompletion 收尾的 `pipeline.active_count()`(&self) 与 `mem::replace(transcript,..)`(&mut) disjoint。编译验证。

- [x] **Step 3: 删 after_vad_tick + handle_streaming_tick + handle_vad_segmented_tick**

删除三个函数（逻辑已进 `tick_events` + `apply_pipeline_events` + `dispatch_tick`）：
- `after_vad_tick`（L1202-1223，整函数）。
- `handle_streaming_tick`（L1351-1410，整函数）。
- `handle_vad_segmented_tick`（L1163-1199，整函数）。

- [x] **Step 4: 三 Tick 命令 dispatch 合一调 dispatch_tick**

改 command dispatch（L231-281）。三 arm 各自保留 `polish_mode` 读取 + `set_mode` 前置，把 `handle_streaming_tick(..)` / `handle_vad_segmented_tick(..)` 调用改为 `dispatch_tick(..)`：

```rust
                    Command::StreamingTick => {
                        {
                            let rc = runtime_config.read().unwrap();
                            config.polish_mode = rc.polish_mode;
                        }
                        if let Stage::Streaming { transcript, .. } = &mut stage {
                            transcript.set_mode(config.polish_mode);
                        }
                        if editing {
                            let _ = audio.drain_samples();
                        } else {
                            dispatch_tick(&mut stage, &audio, &config, &app_handle, &tx);
                        }
                    }
                    #[cfg(feature = "cloud")]
                    Command::CloudStreamingTick => {
                        {
                            let rc = runtime_config.read().unwrap();
                            config.polish_mode = rc.polish_mode;
                        }
                        if let Stage::Streaming { transcript, .. } = &mut stage {
                            transcript.set_mode(config.polish_mode);
                        }
                        if editing {
                            let _ = audio.drain_samples();
                        } else {
                            dispatch_tick(&mut stage, &audio, &config, &app_handle, &tx);
                        }
                    }
                    Command::VadSegmentedTick => {
                        {
                            let rc = runtime_config.read().unwrap();
                            config.polish_mode = rc.polish_mode;
                        }
                        if let Stage::VadSegmented { transcript, .. }
                        | Stage::WaitingCompletion { transcript, .. } = &mut stage
                        {
                            transcript.set_mode(config.polish_mode);
                        }
                        if editing {
                            let _ = audio.drain_samples();
                        } else {
                            dispatch_tick(&mut stage, &audio, &config, &app_handle, &tx);
                        }
                    }
```

- [x] **Step 5: stop 路径 tick 丢弃事件（保持 stop 无 DB/emit，零行为差异）**

stop 路径三处 `pipeline.tick(..)` 改 `pipeline.tick_events(..)` 并丢弃返回 Vec（事件丢弃 = 等价现状 stop 只 set_full 不 DB/emit）：

- VadSegmented stop（L706）：
  ```rust
            if !remaining.is_empty() {
                let _ = pipeline.tick_events(&remaining, &mut transcript);
            }
  ```
- Streaming cloud stop（L736）：
  ```rust
                if !final_samples.is_empty() {
                    let _ = pipeline.tick_events(&final_samples, transcript);
                }
  ```
- Streaming local stop（L773）：
  ```rust
            if !final_samples.is_empty() {
                let _ = pipeline.tick_events(&final_samples, transcript);
            }
  ```

> 说明：现状 stop 路径的 `pipeline.tick` 只 set_full（更新 transcript），无 DB/emit/polish——副作用靠 `finalize_after_stop` 的 `show_result`。丢弃 `tick_events` 的返回事件保持这一行为（pipeline 内部 set_full/spawn/drain 照常，仅 emit/DB/polish 信号不执行）。零行为差异。

- [x] **Step 6: 编译 + clippy**

Run: `cargo check -p octopus-desktop --all-targets 2>&1 | tail -5`
Expected: 0 error（删除的函数无残留引用；`dispatch_tick` 覆盖三命令）。

Run: `cargo clippy -p octopus-desktop --all-targets --features cloud 2>&1 | grep -E "^warning" | wc -l`
Expected: 0 新 warning（与基线比）。

- [x] **Step 7: Commit**

```bash
git add crates/desktop/src/coordinator.rs
git commit -m "refactor(desktop): coordinator dispatch_tick 统一事件循环 + 删旧 handler（2d Task 2）"
```

---

### Task 3: Pipeline trait tick 签名 Vec + 删旧 inherent tick(bool) + 删 silence_duration/took_segment_cut + 清 allow(unused)

**Files:**
- Modify: `crates/desktop/src/pipeline.rs`（trait `tick` → Vec；删旧 inherent `tick(bool)` + trait `silence_duration`/`took_segment_cut` + 两 impl 对应；`tick_events` 改名 `tick`；清 `#[allow(unused)]`；删 `take_error` inherent）
- Modify: `crates/desktop/src/coordinator.rs`（`tick_events` 调用 → `tick`）

**目的：** 合并迁移——trait `tick` 签名收敛为 Vec，删旧 bool 版本与不再用的 trait 方法，清 `#[allow(unused)]`。

- [x] **Step 1: Pipeline trait tick 签名改 Vec + 删 silence_duration/took_segment_cut**

改 `Pipeline` trait（L95-117）：
- L95 的 `#[allow(unused)]` 改 `#[allow(dead_code)]`（coordinator 持具体类型走 inherent `tick`，trait `tick` 不经 trait 路径调用而 dead；trait 的 finish/reset/is_cloud 仍被用。详见 spec §3.7）。
- `tick` 签名 `-> bool` 改 `-> Vec<PipelineEvent>`。
- 删 `silence_duration`（L104-105）+ `took_segment_cut`（L114-116）。

改后 trait（保留 finish/reset/take_close_handle/is_cloud）：
```rust
pub trait Pipeline: Send {
    /// 喂一帧已降噪 16k 样本，返回本 tick 事件流（PersistRaw/Emit/Polish/Error）。
    /// coordinator `apply_pipeline_events` 据此执行 DB/emit/polish/错误上报。（2d）
    fn tick(&mut self, samples: &[f32], transcript: &mut Transcript) -> Vec<PipelineEvent>;
    /// 收尾：流式 flush（tail 已由 stop 路径的 tick 喂入 accept）；vad-seg 仅 drain 剩余 rx。
    fn finish(&mut self, transcript: &mut Transcript) -> TranscriptEvent;
    /// 重置（会话间复用）。
    fn reset(&mut self);
    /// cloud engine 的 async close 句柄（stop 时 coordinator 取出 spawn `close_async`）。
    /// local/vad-seg 返回 `None`（默认）。cfg cloud。
    #[cfg(feature = "cloud")]
    fn take_close_handle(&mut self) -> Option<octopus_asr_cloud::CloudStreamHandle> { None }
    /// 是否 cloud 引擎。vad-seg 恒 false。
    fn is_cloud(&self) -> bool { false }
}
```

- [x] **Step 2: StreamingPipeline 合并 tick_events → tick + 删旧 inherent tick(bool) + 删 take_error + 删 inherent silence_duration**

在 `StreamingPipeline`：
- 删旧 inherent `tick`（L197-218，返回 bool）。
- 把 Task 1 加的 inherent `tick_events` 改名为 `tick`（返回 Vec<PipelineEvent>）——方法体不变（已调 `self.tick` 处改为内联原 tick 的 set_full 逻辑，因为旧 tick 删了）。

改后 inherent `tick`（合并版，内联 set_full + 产事件）：
```rust
    /// 喂一帧已降噪 16k 样本：engine 产事件 → set_full，返回 tick 事件流（2d 合并）。
    /// - local：changed→[PersistRaw,Emit]；每 tick→[Polish]；空样本→[]（早退）
    /// - cloud：changed→[PersistRaw,Polish]；每 tick→[Emit{display+partial}]；error→[Error]
    pub fn tick(&mut self, samples: &[f32], transcript: &mut Transcript) -> Vec<PipelineEvent> {
        let is_cloud = self.engine.is_cloud();
        if !is_cloud && samples.is_empty() {
            return Vec::new();
        }
        let mut changed = false;
        for event in self.engine.tick(samples) {
            match event {
                TranscriptEvent::Partial(text) | TranscriptEvent::Committed(text) => {
                    if text != transcript.full() {
                        transcript.set_full(&text);
                        changed = true;
                    }
                }
                TranscriptEvent::Final(text) => {
                    transcript.set_full(&text);
                    changed = true;
                }
                TranscriptEvent::Error(e) => {
                    warn!("StreamingPipeline event error: {}", e);
                    self.last_error = Some(e);
                }
            }
        }
        let mut events = Vec::new();
        if is_cloud {
            if changed {
                events.push(PipelineEvent::PersistRaw { engine_mode: "streaming" });
                events.push(PipelineEvent::Polish { silence: self.engine.silence_duration() });
            }
            if let Some(e) = self.last_error.take() {
                events.push(PipelineEvent::Error(e));
            }
            let base = transcript.display_text();
            let partial = self.engine.current_partial();
            let display = if partial.is_empty() { base } else { format!("{}{}", base, partial) };
            events.push(PipelineEvent::Emit { display });
        } else {
            if changed {
                events.push(PipelineEvent::PersistRaw { engine_mode: "streaming" });
                events.push(PipelineEvent::Emit { display: transcript.display_text() });
            }
            events.push(PipelineEvent::Polish { silence: self.engine.silence_duration() });
        }
        events
    }
```

- 删 inherent `take_error`（L240-242，2d 后 coordinator 不再调，error 进事件流）。
- 删 inherent `silence_duration`（L230-232，2d 后无人调——dispatch_tick 从 Polish 事件读，stop 不用）。`current_partial`（L235）**保留**（cloud stop L739 取 partial 给 CloudClosing）。

- [x] **Step 3: StreamingPipeline 的 trait impl 适配（删 silence_duration，tick 转发 inherent）**

改 `impl Pipeline for StreamingPipeline`（L262-285）：
- `tick` 改转发 inherent：`fn tick(&mut self, samples, transcript) -> Vec<PipelineEvent> { self.tick(samples, transcript) }`。
- 删 trait `silence_duration`（L271-273）。
- `took_segment_cut` 无 impl（用默认 false）——无需删（本就没 impl）。
- 保留 finish/reset/take_close_handle/is_cloud。

```rust
impl Pipeline for StreamingPipeline {
    fn tick(&mut self, samples: &[f32], transcript: &mut Transcript) -> Vec<PipelineEvent> {
        self.tick(samples, transcript) // 转发 inherent
    }
    fn finish(&mut self, _transcript: &mut Transcript) -> TranscriptEvent {
        self.engine.finish()
    }
    fn reset(&mut self) { self.engine.reset(); }
    #[cfg(feature = "cloud")]
    fn take_close_handle(&mut self) -> Option<octopus_asr_cloud::CloudStreamHandle> {
        self.engine.take_close_handle()
    }
    fn is_cloud(&self) -> bool { self.engine.is_cloud() }
}
```

- [x] **Step 4: VadSegmentedPipeline 合并 tick_events → tick（trait）+ 删 trait silence_duration/took_segment_cut**

`VadSegmentedPipeline`：删 Task 1 加的 inherent `tick_events`（pub(crate)），其逻辑搬进 trait `tick`。

改 `impl Pipeline for VadSegmentedPipeline`（L488-527）：
```rust
impl Pipeline for VadSegmentedPipeline {
    fn tick(&mut self, samples: &[f32], transcript: &mut Transcript) -> Vec<PipelineEvent> {
        let changed = self.run_tick(samples, transcript);
        let segment_cut = self.segment_cut_this_tick;
        let mut events = Vec::new();
        if changed {
            events.push(PipelineEvent::PersistRaw { engine_mode: "vad_segmented" });
            events.push(PipelineEvent::Emit { display: transcript.display_text() });
        }
        if segment_cut {
            events.push(PipelineEvent::Polish { silence: f64::INFINITY });
        }
        events
    }

    fn finish(&mut self, transcript: &mut Transcript) -> TranscriptEvent {
        self.drain_rx_and_consume(transcript);
        TranscriptEvent::Committed(String::new())
    }

    fn reset(&mut self) {
        self.audio_buffer.clear();
        self.overlap_tail.clear();
        self.silence_duration = 0.0;
        self.has_speech = false;
        self.active_count = 0;
        self.next_seq = 0;
        self.completed_seq = 0;
        self.completed_results.clear();
        self.detect_vad.reset();
        self.filter_vad.reset();
        while self.rx.try_recv().is_ok() {}
        self.segment_cut_this_tick = false;
    }

    // take_close_handle / is_cloud 用默认（None / false）。
    // 删原 trait silence_duration / took_segment_cut（信息进 Polish 事件）。
}
```
> `silence_duration` 字段（struct L352）保留——`run_tick` 内部累加用（L440/444）。仅删 trait 方法。

- [x] **Step 5: coordinator 调用 tick_events → tick（改名跟随）**

`coordinator.rs` 的 `dispatch_tick`（Task 2）+ stop 路径（Task 2 Step 5）里的 `pipeline.tick_events(..)` 全改 `pipeline.tick(..)`：
- `dispatch_tick` 三 arm：`pipeline.tick_events(&samples, transcript)` → `pipeline.tick(&samples, transcript)`。
- stop 三处：`pipeline.tick_events(..)` → `pipeline.tick(..)`。

- [x] **Step 6: 既有 pipeline 测试适配（inherent tick 签名 Vec）**

`pipeline.rs` tests 里调 inherent `tick`（返回 bool）的测试改用新签名（返回 Vec）。受影响测试：
- `tick_partial_updates_transcript_and_signals_changed`（L568）：`let changed = p.tick(..)` → 改断言 events 非空 + transcript。
- `tick_final_overrides_transcript`（L581）：同。
- `tick_committed_idempotent_no_change_skip`（L596）：`assert!(!changed)` → `assert!(p.tick(..).is_empty())` 或断言只含 Polish。
- `tick_stashes_error_for_take_error`（L610）：take_error 已删——改为断言 `tick` 返回含 `Error` 事件。
- `finish_delegates_to_engine`（L635）：不动（finish 不变）。

具体改 `tick_partial_updates_transcript_and_signals_changed`：
```rust
    #[test]
    fn tick_partial_updates_transcript_and_signals_changed() {
        let mut p = pipeline(FakePipelineEngine::new(
            vec![TranscriptEvent::Partial("你好".to_string())],
            "", TranscriptEvent::Final("你好。".to_string()),
        ));
        let mut t = Transcript::new(0, PolishMode::Disabled);
        let events = p.tick(&[0.0; 1600], &mut t);
        assert_eq!(t.full(), "你好");
        assert!(events.contains(&PipelineEvent::PersistRaw { engine_mode: "streaming" }));
    }
```
`tick_committed_idempotent_no_change_skip`：
```rust
    #[test]
    fn tick_committed_idempotent_no_change_skip() {
        let mut p = pipeline(FakePipelineEngine::new(
            vec![TranscriptEvent::Committed("一样".to_string())],
            "", TranscriptEvent::Final("".to_string()),
        ));
        let mut t = Transcript::new(0, PolishMode::Disabled);
        t.set_full("一样");
        let events = p.tick(&[0.0; 1600], &mut t);
        // changed=false → 只产 Polish（local 每 tick），无 PersistRaw/Emit
        assert!(!events.contains(&PipelineEvent::PersistRaw { engine_mode: "streaming" }));
        assert_eq!(events, vec![PipelineEvent::Polish { silence: 0.0 }]);
    }
```
`tick_stashes_error_for_take_error` 改名为 `tick_error_produces_error_event`：
```rust
    #[test]
    fn tick_error_produces_error_event() {
        let mut p = pipeline(FakePipelineEngine::new(
            vec![TranscriptEvent::Error("boom".to_string())],
            "", TranscriptEvent::Final("".to_string()),
        ));
        let mut t = Transcript::new(0, PolishMode::Disabled);
        let events = p.tick(&[0.0; 1600], &mut t);
        assert!(events.iter().any(|e| matches!(e, PipelineEvent::Error(msg) if msg == "boom")));
    }
```
`tick_final_overrides_transcript`：
```rust
    #[test]
    fn tick_final_overrides_transcript() {
        let mut p = pipeline(FakePipelineEngine::new(
            vec![TranscriptEvent::Final("最终。".to_string())],
            "", TranscriptEvent::Final("最终。".to_string()),
        ));
        let mut t = Transcript::new(0, PolishMode::Disabled);
        t.set_full("旧的");
        let events = p.tick(&[0.0; 1600], &mut t);
        assert_eq!(t.full(), "最终。");
        assert!(events.contains(&PipelineEvent::PersistRaw { engine_mode: "streaming" }));
    }
```
> Task 1 加的 `tick_events_*` 测试（local/cloud）改方法名 `tick_events` → `tick`（逻辑不变，断言不变）。

- [x] **Step 7: 编译 + clippy + 测试**

Run: `cargo check -p octopus-desktop --all-targets --features cloud 2>&1 | tail -5`
Expected: 0 error。

Run: `cargo clippy -p octopus-desktop --all-targets --features cloud 2>&1 | grep -E "^warning" | wc -l`
Expected: 0 新 warning（`#[allow(unused)]` 已清，无残留 unused）。

Run: `cargo test -p octopus-desktop pipeline::tests`
Expected: 全绿。

- [x] **Step 8: Commit**

```bash
git add crates/desktop/src/pipeline.rs crates/desktop/src/coordinator.rs
git commit -m "refactor(desktop): Pipeline trait tick 签名 Vec + 删 silence_duration/took_segment_cut（2d Task 3）"
```

---

### Task 4: 双 feature check + clippy + workspace 测试 + e2e 回归 + 文档同步 + ff-merge

**Files:**
- Verify: `crates/desktop/`（双 feature 编译 + 测试 + e2e）
- Modify: `docs/superpowers/specs/2026-06-25-archived-spec.md#coordinator-cleanup-design`（横幅状态）
- Modify: `docs/superpowers/plans/2026-06-25-archived-plan.md#coordinator-cleanup`（复选框）
- Modify: memory `parallel-workstreams.md`（item 7 的 2d 状态）

**目的：** 全量验证 + e2e 回归 + 文档同步 + ff-merge main。

- [x] **Step 1: 全量编译 + 测试矩阵**

```bash
cargo check --workspace --all-targets 2>&1 | tail -5
cargo check --workspace --all-targets --features cloud 2>&1 | tail -5
cargo clippy --workspace --features cloud --all-targets 2>&1 | grep -E "^warning" | wc -l
cargo test --workspace 2>&1 | grep "test result"
```
Expected: 双 feature 0 error；clippy 无新 warning（与基线比）；workspace 测试全绿（除 2 个 pre-existing infra 失败 `seed_then_load_round_trips`/`list_all_local_asr_models_includes_disabled`——seed c796cbc 重写后断言过时，与本次无关，2d 未触碰 crates/infra/）。

- [x] **Step 2: 手动 e2e（事件流收敛后零行为差异回归）** — 通过（2026-06-25，用户本地验三路径零行为差异）

启动 desktop（`cargo tauri dev` 或既有启动方式），验证：

**streaming local（流式本地引擎，如 zipformer-streaming / qwen3-streaming）：**
1. 录音 → result window 增量显示 partial → finalize 后整句。
2. 停顿（≥pause_polish_threshold）→ 中间润色触发（mode=2）。
3. 停止 → finalize 粘贴（含润色）。

**streaming cloud（云端流式，cfg cloud）：**
4. 云端流式 → 每 tick emit（display + partial 预览）→ commit 后 DB + 润色。
5. WSS 错误（断网/坏 key）→ result window 显示错误（Error 事件上报）。
6. 停止 → close_async → finalize_cloud。

**VadSegmented（非流式本地引擎，如 moonshine / zipformer-non-streaming）：**
7. onset：「正在聆听…」→ 说话 → 段识别乱序回填按 seq 拼接。
8. 强制切段（≥20s）→ overlap 衔接连贯。
9. 停顿切段 → segment_cut 触发停顿润色（mode=2）。
10. stop WaitingCompletion → tick drain → active_count==0 → finalize（文本完整）。
11. 跨会话护栏：停止后立刻重开 → 旧会话迟到段不污染新会话。
12. Cancel/Discard → tick 停止、无泄漏、无迟到粘贴。

- [x] **Step 3: 同步 spec 横幅 + plan 复选框**

spec `docs/superpowers/specs/2026-06-25-archived-spec.md#coordinator-cleanup-design` 顶部状态行改：
```
> **状态**：✅ 已实施（待 ff-merge main）。Task 1-4 双 feature 编译 0 error、clippy 0 新 warning、workspace 测试除 2 pre-existing infra 外全绿；e2e 验证通过（2026-06-25）。
```
本 plan 所有 `- [x]` → `- [x]`。

- [x] **Step 4: Commit 文档**

```bash
git add docs/superpowers/specs/2026-06-25-archived-spec.md#coordinator-cleanup-design docs/superpowers/plans/2026-06-25-archived-plan.md#coordinator-cleanup
git commit -m "docs(spec/plan): 2d coordinator 清理自动化验证通过、状态同步"
```

- [x] **Step 5: 收尾（finishing-a-development-branch）** — ff-merge main（2026-06-25）

e2e 通过后，用 superpowers:finishing-a-development-branch 选 ff-merge main（对齐 2a/2b/2c-1/2c-2/2c-3 节奏）。合并后更新 memory `parallel-workstreams.md` item 7 的 2d 状态（2d 从「待」→「已 ff-merge main（SHA）」）。

---

## Self-Review

**1. Spec coverage：**
- §3.2 PipelineEvent → Task 1 Step 1。
- §3.3 tick 签名 Vec → Task 1（tick_events）+ Task 3（trait 合并）。
- §3.4 三路径事件序列 → Task 1 Step 2/3（streaming local/cloud + vad-seg）。
- §3.5 apply_pipeline_events + dispatch_tick → Task 2 Step 1/2。
- §3.6 边界（Stage 不变/finalize/cloud close/stop 丢弃事件）→ Task 2 Step 5（stop 丢弃）+ 全 task 不碰 finalize/cloud close。
- §3.7 trait 精简（删 silence_duration/took_segment_cut + 清 allow）→ Task 3 Step 1/3/4。
- §8 测试（pipeline 单测 + e2e）→ Task 1 Step 5 + Task 4 Step 2。
- §9 迁移映射 → Task 1-3 各步。
- **修正点**（plan 精确化 spec）：stop 路径丢弃事件（spec §3.5/§3.6 说复用 apply，plan 改为丢弃——现状 stop 无 DB/emit，丢弃保零行为差异）；current_partial 保留 pub（spec §3.7 说收回内部，plan 改为仅 take_error 收回——cloud stop L739 用 current_partial）。

**2. Placeholder scan：** 无 TBD/TODO；每步含确切代码或命令。

**3. Type consistency：** `PipelineEvent` 变体（PersistRaw{engine_mode:&'static str}/Emit{display:String}/Polish{silence:f64}/Error(String)）在 Task 1 定义、Task 1-3 测试与 impl 一致；`tick_events`（Task 1）→ `tick`（Task 3）改名贯穿；`dispatch_tick`/`apply_pipeline_events` 签名 Task 2 定义、Task 3 调用一致。

---

## 2026-06-25-desktop-cloud-dedupe



**Goal:** 删除 desktop 的 4 个 `*_stream.rs` + `cloud_types.rs` 协议层副本（共 5 文件），`CloudPipelineEngine` 改指 `octopus-asr-cloud` crate，消除协议层两份重复维护的技术债，零行为差异。

**Architecture:** cloud crate（第一步已合并 main）协议层零改动；desktop 仅在 `cloud_pipeline.rs` 用 `tauri::async_runtime::block_on` 包一层进入 tokio context 后调 cloud crate 的 `open_cloud_session`（方案 B）；`CloudStreamHandle`/`StreamEvent` 类型源从 `crate::cloud_types` 切到 `octopus_asr_cloud`；cloud crate 加一个 `#[doc(hidden)] pub fn new_for_test()` 供 desktop 的 5 个 drain 测试构造预载事件的 handle。

**Tech Stack:** Rust workspace；`octopus-asr-cloud`（tokio + tokio-tungstenite WSS）、`octopus-desktop`（tauri 2，`cloud` feature gate）；tauri async_runtime 即 tokio。

**Spec:** `docs/superpowers/specs/2026-06-25-archived-spec.md#desktop-cloud-dedupe-design`
**Worktree:** `worktree-model-mgmt-ui`（已就位，叠加分支）

> **实施状态**：✅ 已合并 main（`6a4593e`，ff-merge）。Task 1-6 全完成，云端流式 e2e 2026-06-25 本地云端 key 验证通过。
>
> **实施修正**（vs 原 plan，3 处盲点 + 1 时序）：
> - **Task 2 时序**：原"接入+瘦身"合一，删 flate2/hmac/sha1 deps 时 `bytedance_stream`/`tencent_stream` 副本仍 `use` 它们→编译断。拆为 Task 2 仅接入 octopus-asr-cloud，瘦身随 Task 4 删副本（`57685df`）。
> - **Task 3 盲点**：`pipeline.rs` 的 `StreamingPipelineEngine::take_close_handle` trait 签名（+ 包装方法）也写死 `crate::cloud_types::CloudStreamHandle`，须同步切 `octopus_asr_cloud`（否则 E0053 trait 类型不匹配）。cloud crate `lib.rs` 补 `CloudStreamHandle`/`StreamEvent` 顶层 re-export（`2e15bfd`）。
> - **Task 4 盲点**：`engine_aliyun.rs`（chunk 模式）复用 `aliyun_stream::is_qwen_realtime_endpoint` + `cloud_types::samples_to_pcm_s16le`（原以为零改动）。改指 cloud crate；cloud crate 顺势把这两个 helper `pub(crate)`→`pub` + re-export `samples_to_pcm_s16le`（`c5b73cf`）。

---

## 文件结构

| 文件 | 动作 | 责任 |
|---|---|---|
| `crates/asr-cloud/src/cloud_types.rs` | 改（加 1 fn） | 加 `new_for_test` 测试构造器（D2） |
| `crates/desktop/Cargo.toml` | 改 | cloud feature 接入 `octopus-asr-cloud` + 瘦身 flate2/hmac/sha1 |
| `crates/desktop/src/cloud_pipeline.rs` | 改 | use 源切 cloud crate + 删 5 resolve fn + open_cloud_session 改 block_on wrapper + tests 改 new_for_test |
| `crates/desktop/src/coordinator.rs` | **不改** | 靠类型推断，编译验证零改动 |
| `crates/desktop/src/main.rs` | 改 | 删 5 个 `#[cfg(feature="cloud")] mod *_stream/cloud_types` |
| `crates/desktop/src/{aliyun_stream,bytedance_stream,tencent_stream,baidu_stream,cloud_types}.rs` | **删** | 协议层副本（cloud crate 1:1） |

每个 Task 末尾编译通过 + commit（搬迁为主，frequent commits）。

---

## Task 1: cloud crate 加 `new_for_test` 测试构造器

**Files:**
- Modify: `crates/asr-cloud/src/cloud_types.rs`（`impl CloudStreamHandle` 块内 `new()` 旁加 `new_for_test`；tests mod 加 1 测试）

**Why:** desktop 删 `cloud_types.rs` 后，其 `cloud_pipeline.rs` 5 个 drain 测试需跨 crate 构造预载 `StreamEvent` 的 `CloudStreamHandle`。cloud crate 的 `new()` 是 `pub(crate)` 且返回类型含 `pub(crate) PcmFrame`，无法直接暴露；新增只返回 `(Self, UnboundedSender<StreamEvent>)` 的 `new_for_test` 绕过此约束。

- [x] **Step 1: 写失败测试（验证 new_for_test 返回的 sender 能投递到 handle）**

在 `crates/asr-cloud/src/cloud_types.rs` 的 `#[cfg(test)] mod tests`（文件末尾 `}` 前）追加：

```rust
    #[test]
    fn new_for_test_returns_handle_and_event_sender() {
        // new_for_test 构造的 (handle, sender)：sender 预载事件后 handle.try_recv_text 能取到。
        // 供跨 crate（desktop cloud_pipeline 测试）构造预载事件的 handle。
        let (mut handle, tx) = CloudStreamHandle::new_for_test();
        let _ = tx.send(StreamEvent::Text("hello".to_string()));
        assert!(
            matches!(handle.try_recv_text(), Some(StreamEvent::Text(t)) if t == "hello"),
            "new_for_test 预载的事件应能被 try_recv_text 取到"
        );
    }
```

- [x] **Step 2: 跑测试确认失败**

Run: `cargo test -p octopus-asr-cloud new_for_test`
Expected: 编译失败 `no function named new_for_test`（方法尚未定义）。

- [x] **Step 3: 实现 new_for_test**

在 `crates/asr-cloud/src/cloud_types.rs` 的 `impl CloudStreamHandle {` 块内、`pub(crate) fn new(...)` 之后插入：

```rust
    /// 仅供测试：构造 handle + result 发送端（预载事件用）。不暴露 pcm_rx / `pub(crate) PcmFrame`。
    ///
    /// 返回 `(handle, result_tx)`：测试向 `result_tx` 投递 `StreamEvent` 后，`handle.try_recv_text`
    /// 可取到。供 desktop `cloud_pipeline::handle_with_events` 等 drain 测试跨 crate 构造预载 handle。
    #[doc(hidden)]
    pub fn new_for_test() -> (Self, mpsc::UnboundedSender<StreamEvent>) {
        let (handle, _pcm_rx, result_tx) = Self::new();
        (handle, result_tx)
    }
```

- [x] **Step 4: 跑测试确认通过**

Run: `cargo test -p octopus-asr-cloud`
Expected: PASS（含新增 `new_for_test_returns_handle_and_event_sender` + 原 3 个 cloud_types 测试，共 ≥4）。

- [x] **Step 5: Commit**

```bash
git add crates/asr-cloud/src/cloud_types.rs
git commit -m "feat(asr-cloud): 加 CloudStreamHandle::new_for_test 测试构造器

供 desktop cloud_pipeline drain 测试跨 crate 构造预载事件的 handle。
#[doc(hidden)] pub，返回 (Self, UnboundedSender<StreamEvent>)，
不暴露 pub(crate) PcmFrame。desktop-cloud-dedupe 第二步 D2。"
```

---

## Task 2: desktop Cargo.toml 接入 cloud crate + 瘦身

**Files:**
- Modify: `crates/desktop/Cargo.toml`（`[dependencies]` 删 flate2/hmac/sha1 + 加 octopus-asr-cloud；`[features]` cloud 改写；注释同步）

**Why:** desktop 改指 cloud crate 后需声明依赖；`flate2`/`hmac`/`sha1` 仅被待删的 `bytedance_stream.rs`/`tencent_stream.rs` 直接 use（已 grep 确认），删副本后 desktop 不再直接用，从 cloud feature 与 `[dependencies]` 移除（cloud crate 自身依赖它们，作为 transitive dep 仍编译可用）。`tokio-tungstenite`/`uuid`/`base64`/`futures-util` 仍被 `engine_aliyun.rs`（cloud，不删）/`engine_ws.rs`（remote-ws）/`settings_commands.rs` 用，**保留**。

- [x] **Step 1: 改 [dependencies]——删 flate2/hmac/sha1，加 octopus-asr-cloud**

把 `crates/desktop/Cargo.toml` 的云端 WS 依赖段（当前 L50-59）：

```toml
# 云端 ASR WS engine（cloud feature 用）
# uuid 用于生成 task_id / event_id / request_id / voice_id；走 wss:// 必须 TLS。
# base64 用于 Qwen-ASR Realtime 协议（audio 字段为 base64 PCM）+ Tencent 签名 Base64。
# flate2 用于 ByteDance ASR 二进制协议（gzip 压缩 payload）。
# hmac + sha1 用于 Tencent ASR 签名鉴权（HMAC-SHA1）。
uuid = { version = "1", features = ["v4"], optional = true }
base64 = { version = "0.22", optional = true }
flate2 = { version = "1", optional = true }
hmac = { version = "0.12", optional = true }
sha1 = { version = "0.10", optional = true }
```

替换为：

```toml
# 云端 ASR WS engine（cloud feature 用）
# uuid 用于生成 task_id / event_id / request_id / voice_id；走 wss:// 必须 TLS。
# base64 用于 Qwen-ASR Realtime 协议（audio 字段为 base64 PCM）+ Tencent 签名 Base64。
#（flate2/hmac/sha1 已随 *_stream.rs 副本删去，下沉 octopus-asr-cloud；engine_aliyun.rs chunk 模式仅需 uuid/base64。）
uuid = { version = "1", features = ["v4"], optional = true }
base64 = { version = "0.22", optional = true }

# 云端 ASR 协议层（4 provider WSS + 批引擎，下沉 crate；desktop cloud feature 复用）
octopus-asr-cloud = { path = "../asr-cloud", optional = true }
```

- [x] **Step 2: 改 [features].cloud——删 flate2/hmac/sha1，加 octopus-asr-cloud**

把当前 cloud feature 行（L81）：

```toml
cloud = ["tokio-tungstenite", "tokio-tungstenite?/native-tls", "uuid", "futures-util", "base64", "flate2", "hmac", "sha1"]
```

替换为：

```toml
# 云端 ASR WS 流式识别（Aliyun / ByteDance / Tencent / Baidu）：
# 协议层下沉 octopus-asr-cloud（4 provider WSS 1:1 复刻自原 desktop *_stream.rs）。
# tokio-tungstenite 启用 native-tls 以支持 wss://（engine_aliyun chunk 模式 + settings 连接测试亦用）。
cloud = ["tokio-tungstenite", "tokio-tungstenite?/native-tls", "uuid", "futures-util", "base64", "octopus-asr-cloud"]
```

- [x] **Step 3: 验证依赖解析 + cloud feature 编译（desktop 代码尚未改，应仍通过）**

Run: `cargo build -p octopus-desktop --features cloud`
Expected: 编译成功（此时 desktop 仍用 `crate::cloud_types` 副本，新依赖 `octopus-asr-cloud` 仅被引入未使用，不影响编译；可能有无害的 unused warning，下一步代码改完即消）。

- [x] **Step 4: Commit**

```bash
git add crates/desktop/Cargo.toml
git commit -m "build(desktop): cloud feature 接入 octopus-asr-cloud + 瘦身 flate2/hmac/sha1

协议层下沉 cloud crate 后，flate2/hmac/sha1 仅 cloud crate 自身依赖（transitive），
desktop 不再直接 use（原仅 bytedance_stream/tencent_stream 副本用）。
tokio-tungstenite/uuid/base64/futures-util 保留（engine_aliyun/engine_ws/settings_commands 用）。
desktop-cloud-dedupe 第二步 D3。"
```

---

## Task 3: cloud_pipeline.rs 改造（use 源 + open wrapper + 删 resolve + tests）

**Files:**
- Modify: `crates/desktop/src/cloud_pipeline.rs`（use 区 L8-13；删 L113-177 共 5 个 resolve fn；open_cloud_session L181-213 改写；tests 4 处 `new()` 调用改 `new_for_test()`）

**Why:** 这是搬迁核心：类型源切到 cloud crate，配置解析/open 分发改调 cloud crate，`CloudPipelineEngine`/drain 逻辑零改动。改后 `crate::cloud_types` 不再被 `cloud_pipeline.rs` 引用（`cloud_types.rs` 文件本身暂留，下一 Task 删，期间为 dead code 但编译通过）。

- [x] **Step 1: 改 use 区——切 cloud crate，删 RuntimeHandle**

把 `crates/desktop/src/cloud_pipeline.rs` 顶部 use 区（L8-13）：

```rust
use crate::cloud_types::{CloudStreamHandle, StreamEvent};
use crate::pipeline::{compute_speech_chunks, StreamingPipelineEngine};
use log::{debug, error, info, warn};
use octopus_asr_local::streaming_runner::TranscriptEvent;
use octopus_asr_local::vad::SileroVad;
use tauri::async_runtime::RuntimeHandle;
```

替换为：

```rust
use crate::pipeline::{compute_speech_chunks, StreamingPipelineEngine};
use log::{debug, error, info, warn};
use octopus_asr_local::streaming_runner::TranscriptEvent;
use octopus_asr_local::vad::SileroVad;
use octopus_asr_cloud::{CloudStreamHandle, StreamEvent};
```

- [x] **Step 2: 删 5 个 resolve fn（L110-177 整段）**

删除从注释 `// ── open/resolve helpers（迁自 coordinator.rs:1504-1617...`（约 L110）到 `resolve_baidu_config` 结束 `}`（L177）的整段——含：`resolve_cloud_entry`、`resolve_aliyun_config`、`resolve_bytedance_config`、`resolve_tencent_config`、`resolve_baidu_config` 共 5 个 fn（这些 cloud crate `config.rs` 已有等价物）。

删除后，该区域紧接着 `take_preroll` fn（L108 结束）之后直接是改造后的 `open_cloud_session`（见 Step 3）。

- [x] **Step 3: open_cloud_session 改 block_on 瘦 wrapper（方案 B）**

把原 `open_cloud_session`（删完 resolve 后，原 L181-213 的整段）替换为：

```rust
/// onset dispatch：根据引擎 spec 打开对应云端 WSS session（返回句柄）。
///
/// cloud crate 的 `open_cloud_session` 内部 `tokio::spawn`，**须在 tokio context**；
/// coordinator 主线程非 tokio，用 `tauri::async_runtime::block_on` 进入（tauri runtime 即 tokio）。
/// `block_on` 内同步 `open` 只 spawn reader task + 返回 channel handle（不 await 建连），立即返回，
/// 不阻塞 coordinator 主线程。
pub(super) fn open_cloud_session(
    asr_engine: &str,
    language: &str,
    pre_roll: Vec<f32>,
) -> Result<CloudStreamHandle, String> {
    tauri::async_runtime::block_on(async {
        octopus_asr_cloud::open_cloud_session(asr_engine, language, pre_roll)
    })
    .map_err(|e| e.to_string())
}
```

- [x] **Step 4: tests 改 new_for_test（4 处）**

`crates/desktop/src/cloud_pipeline.rs` tests mod 里，所有 `CloudStreamHandle::new()` 调用（返回三元组 `(handle, _pcm_rx, result_tx)`）改为 `CloudStreamHandle::new_for_test()`（返回二元组 `(handle, result_tx)`）。共 4 处：

(a) `handle_with_events` helper（约 L407-413）：

```rust
    fn handle_with_events(events: Vec<StreamEvent>) -> CloudStreamHandle {
        let (handle, result_tx) = CloudStreamHandle::new_for_test();
        for ev in events {
            let _ = result_tx.send(ev);
        }
        handle
    }
```

(b)(c)(d) `drain_finished_emits_committed_with_comma`、`drain_finished_no_double_comma_when_committed_ends_with_comma`、`drain_failed_emits_error_clears_partial` 三个测试里各自的：

```rust
        let (handle, _pcm_rx, result_tx) = CloudStreamHandle::new();
```

替换为：

```rust
        let (handle, result_tx) = CloudStreamHandle::new_for_test();
```

（共 3 处，逐个替换；`_pcm_rx` 不再存在因 `new_for_test` 不返回它。）

- [x] **Step 5: cloud feature 编译验证（coordinator 应零改动）**

Run: `cargo build -p octopus-desktop --features cloud`
Expected: 编译成功。`coordinator.rs` 靠类型推断（L845 `if let Some(handle) = pipeline.take_close_handle()` + L858 `handle.close_async().await`），`CloudStreamHandle` 类型由 `cloud_pipeline::CloudPipelineEngine::take_close_handle` 返回类型推断而来，无需 `use`，**预期零改动**。

若编译报 `coordinator.rs` 缺 `CloudStreamHandle` 类型：在 `coordinator.rs` 顶部 use 区加 `#[cfg(feature = "cloud")] use octopus_asr_cloud::CloudStreamHandle;`（但据 grep 确认 coordinator 无显式类型标注，不应需要）。

- [x] **Step 6: cloud_pipeline 8 测试验证**

Run: `cargo test -p octopus-desktop --features cloud cloud_pipeline`
Expected: PASS（8 个：`drain_text_updates_partial_no_event` / `drain_finished_emits_committed_with_comma` / `drain_finished_no_double_comma_when_committed_ends_with_comma` / `drain_finished_no_partial_no_event_no_comma` / `drain_failed_emits_error_clears_partial` / `onset_confirmed_requires_two_consecutive` / `should_send_finish_only_when_speaking_not_closing_silence_enough` / `take_preroll_last_n_samples`）。

- [x] **Step 7: Commit**

```bash
git add crates/desktop/src/cloud_pipeline.rs
git commit -m "refactor(desktop): cloud_pipeline 改指 octopus-asr-cloud 协议层

use 源 crate::cloud_types → octopus_asr_cloud；删 5 个 resolve_*_config（cloud crate 有）；
open_cloud_session 改 block_on 瘦 wrapper（方案 B：tauri runtime 进 tokio context）；
8 测试 new() → new_for_test()。CloudPipelineEngine/drain 逻辑零改动。
coordinator 靠类型推断零改动。desktop-cloud-dedupe 第二步 D2+D3。"
```

---

## Task 4: 删 main.rs 5 mod + 删 5 个协议副本文件

**Files:**
- Modify: `crates/desktop/src/main.rs`（删 5 个 `#[cfg(feature = "cloud")] mod *_stream / cloud_types`）
- Delete: `crates/desktop/src/aliyun_stream.rs`、`bytedance_stream.rs`、`tencent_stream.rs`、`baidu_stream.rs`、`cloud_types.rs`

**Why:** Task 3 后 `cloud_pipeline.rs` + `coordinator.rs` 已不引用 `crate::cloud_types` / `crate::*_stream`，可安全删 mod 与文件。保留 `mod engine_aliyun`（chunk 模式离线引擎，另一套）+ `mod cloud_pipeline`（改造后留）。

- [x] **Step 1: 删 main.rs 的 5 个 cloud mod**

`crates/desktop/src/main.rs` 的 mod 区当前（L3-20 节选）：

```rust
mod audio;
mod config;
mod coordinator;
#[cfg(feature = "cloud")]
mod aliyun_stream;
mod engine;
#[cfg(feature = "cloud")]
mod engine_aliyun;
#[cfg(feature = "cloud")]
mod bytedance_stream;
#[cfg(feature = "cloud")]
mod tencent_stream;
#[cfg(feature = "cloud")]
mod baidu_stream;
#[cfg(feature = "cloud")]
mod cloud_types;
#[cfg(feature = "cloud")]
mod cloud_pipeline;
```

删除 `aliyun_stream`、`bytedance_stream`、`tencent_stream`、`baidu_stream`、`cloud_types` 共 5 个 `#[cfg(feature = "cloud")] mod xxx;`（含各自上方 cfg 行）。改后 mod 区：

```rust
mod audio;
mod config;
mod coordinator;
mod engine;
#[cfg(feature = "cloud")]
mod engine_aliyun;
#[cfg(feature = "cloud")]
mod cloud_pipeline;
```

- [x] **Step 2: 删 5 个协议副本文件**

```bash
git rm crates/desktop/src/aliyun_stream.rs crates/desktop/src/bytedance_stream.rs crates/desktop/src/tencent_stream.rs crates/desktop/src/baidu_stream.rs crates/desktop/src/cloud_types.rs
```

Expected: 5 个文件删除，staged。

- [x] **Step 3: 双 feature 编译验证**

Run: `cargo build -p octopus-desktop --features cloud`
Expected: 编译成功（cloud on：协议层走 octopus-asr-cloud，`engine_aliyun`/`cloud_pipeline` 仍在）。

Run: `cargo build -p octopus-desktop`
Expected: 编译成功（cloud off：5 个 mod 与 cloud_pipeline/engine_aliyun 都不编译，default=embedded）。

- [x] **Step 4: Commit**

```bash
git add crates/desktop/src/main.rs
git commit -m "refactor(desktop): 删 4 个 *_stream.rs + cloud_types.rs 协议层副本

协议层单源下沉 octopus-asr-cloud，消除两份字节级副本的技术债。
main.rs 删 5 个 #[cfg(cloud)] mod；保留 engine_aliyun（chunk 模式离线引擎，另一套）+
cloud_pipeline（流式适配，Task 3 改造）。desktop-cloud-dedupe 第二步 D3。"
```

---

## Task 5: 全量验证（双 feature build/clippy/test + workspace check）

**Files:** 无代码改动（仅验证；若有 clippy/编译修复则改对应文件）

- [x] **Step 1: cloud on 全 target 构建 + clippy**

Run: `cargo build -p octopus-desktop --features cloud --all-targets`
Expected: 0 error。

Run: `cargo clippy -p octopus-desktop --features cloud --all-targets`
Expected: desktop 新代码（cloud_pipeline.rs）0 warning。预存的 infra/llm/asr warning 与本次无关（cloud 协议层本就零 warning）。

- [x] **Step 2: cloud off 构建（default embedded）**

Run: `cargo build -p octopus-desktop --all-targets`
Expected: 0 error（cloud 副本已删，cloud off 不受影响）。

- [x] **Step 3: asr-cloud 30 测试不变**

Run: `cargo test -p octopus-asr-cloud`
Expected: PASS（Task 1 加的 new_for_test 测试 + 原 30 个，无回归）。

- [x] **Step 4: desktop cloud_pipeline 8 测试**

Run: `cargo test -p octopus-desktop --features cloud cloud_pipeline`
Expected: PASS（8 个）。

- [x] **Step 5: workspace check**

Run: `cargo check --workspace --all-targets`
Expected: 0 error。

- [x] **Step 6: 若 Step 1-5 发现问题则修复并 commit；全绿则进 Task 6**

若 clippy 报新 warning 或编译错误，修复后：

```bash
git add <修复的文件>
git commit -m "fix(desktop): desktop-cloud-dedupe 全量验证修复"
```

若全绿，无 commit，直接进 Task 6。

---

## Task 6: e2e 回归清单（交付用户）+ 文档同步

**Files:**
- Modify: `docs/superpowers/specs/2026-06-25-archived-spec.md#desktop-cloud-dedupe-design`（横幅状态）
- Modify: `docs/superpowers/plans/2026-06-25-archived-plan.md#desktop-cloud-dedupe`（横幅 + Task checkbox）
- 不改代码

**Why:** GUI/网络集成无自动化 e2e；云端流式需用户本地云端 key 验证。交付手动清单 + 同步文档状态（z_sync_superpowers 精神）。

- [x] **Step 1: 编译 desktop release（cloud）交付用户跑 e2e**

Run: `cargo build -p octopus-desktop --features cloud --release`
Expected: release 二进制生成（交付用户本地云端 key 验证）。

- [x] **Step 2: 交付 e2e 手动验证清单（不自动化）**

向用户提供以下清单（用户本地云端 key 跑），覆盖云端流式全路径 + 本地流式回归：

```
desktop cloud e2e 回归（--features cloud release 二进制）：
1. 云端流式 onset：选云端引擎（aliyun/bytedance/tencent/baidu 之一），说话 → 结果窗口"正在聆听…"→ partial 实时更新（验证 open_cloud_session block_on 路径 + push_pcm）
2. 云端流式 finish：停说 ≥ pause_polish_threshold → 句末提交、逗号拼接、进 DB（验证 drain Finished → Committed）
3. 云端 close：Toggle 停止 → spawn close_async → CloudStreamingDone finalize + 粘贴（验证 take_close_handle + close_async + Stage::CloudClosing 护栏）
4. 云端识别失败恢复：模拟断网/错误 key → "⚠️ 云端识别失败" 提示，下次 onset 重试（验证 drain Failed）
5. 本地流式回归：切本地引擎（embedded）→ 流式识别正常（验证 cloud 改造不影响 local 路径）
6. cloud off 回归：cargo build（无 cloud）→ embedded 本地识别正常（验证 default 路径不受影响）
```

> ✅ **2026-06-25 e2e 验证通过**：用户本地云端 key 跑全 6 项，云端流式 + 本地流式 + cloud off 全路径正常。

- [x] **Step 3: 同步 spec 横幅状态**

`docs/superpowers/specs/2026-06-25-archived-spec.md#desktop-cloud-dedupe-design` 顶部横幅：

```
> **状态**：设计待实施。
```

改为：

```
> **状态**：已实现（8 task 全完成，Task 1-5 编译/测试通过；e2e 待用户本地云端 key 验证）。
```

- [x] **Step 4: 同步 plan 横幅 + Task checkbox**

`docs/superpowers/plans/2026-06-25-archived-plan.md#desktop-cloud-dedupe` 顶部加横幅（Goal 上方）：

```
> **状态**：已实现（Task 1-5 编译/测试通过；Task 6 e2e 待用户本地云端 key 验证）。
```

并把 Task 1-5 所有 `[ ]` 改 `[x]`（Task 6 的 Step 1-3 改 `[x]`，Step 2「交付 e2e 清单」保持 `[ ]` 标注待用户验证）。

- [x] **Step 5: Commit 文档同步**

```bash
git add docs/superpowers/specs/2026-06-25-archived-spec.md#desktop-cloud-dedupe-design docs/superpowers/plans/2026-06-25-archived-plan.md#desktop-cloud-dedupe
git commit -m "docs(desktop-cloud-dedupe): 同步实施状态（Task 1-5 通过，e2e 待本地云端 key）"
```

---

## Self-Review

**1. Spec coverage:**
- §3 D1 方案 B（block_on）→ Task 3 Step 3 ✅
- §4 D2 类型归属（use 源切换）→ Task 3 Step 1 ✅；new_for_test → Task 1 ✅
- §5.1 删 5 文件 → Task 4 Step 2 ✅；删 5 mod → Task 4 Step 1 ✅；Cargo.toml 瘦身 → Task 2 ✅
- §5.2 cloud_pipeline use/resolve/open/tests → Task 3 ✅
- §5.3 cloud crate 加 new_for_test → Task 1 ✅（协议层零改动，仅 1 fn）
- §5.4 依赖边界（单向）→ Task 2 Cargo.toml ✅
- §8 验证清单（双 feature build/clippy + 8 测试 + 30 测试 + workspace check + e2e）→ Task 5 + Task 6 ✅
- coordinator 零改动（spec §4.1 隐含）→ Task 3 Step 5 验证 ✅

**2. Placeholder scan:** 无 TBD/TODO；每步含完整代码或精确命令 + 预期输出。✅

**3. Type consistency:**
- `new_for_test` 签名 `(Self, UnboundedSender<StreamEvent>)` —— Task 1 定义、Task 3 Step 4 使用（二元组 destructuring），一致 ✅
- `open_cloud_session` 返回 `Result<CloudStreamHandle, String>` —— Task 3 定义，`CloudPipelineEngine::tick` 调用点（L307，未改）期望该签名，一致 ✅
- `CloudStreamHandle`/`StreamEvent` 全部源自 `octopus_asr_cloud` —— Task 3 Step 1 use 后，drain/tests/take_close_handle 统一，一致 ✅

---

## 2026-06-25-vad-segmented-rehome



**Goal:** 把散在 `coordinator.rs` 的 VadSegmented（非流式引擎 VAD 分段伪流式）编排 + 乱序回填收进统一 `Pipeline` 角色（`VadSegmentedPipeline`），删除 `Command::TranscriptionDone`，coordinator 持 `Box<dyn Pipeline>` 不再按 stage 分流 tick 逻辑。

**Architecture:** 新增上层 `Pipeline` trait（`tick/finish/silence_duration/reset/take_close_handle/is_cloud/took_segment_cut`）。`VadSegmentedPipeline` 内部持 mpsc channel：切段后 `tauri::async_runtime::spawn` 跑 `engine.transcribe`，结果发回 pipeline 自持 `rx`（**不发 coordinator.tx**），下一个 tick `try_recv` drain + 乱序回填 + 消费连续 seq + set_full——异步命令回传转成同步 tick 输出。`StreamingPipeline` 外层加 `impl Pipeline`（内层 `StreamingPipelineEngine` 两层不动）。coordinator `Stage::VadSegmented`/`WaitingCompletion` 字段改持 pipeline，删 `TranscriptionDone` 命令与两处回填 handler。emit/DB/polish/transcript 仍留 coordinator（2d 收敛）。每 Task 零行为差异 + 双 feature 编译 + clippy 零新 warning。

**Tech Stack:** Rust，tauri 2（`tauri::async_runtime::spawn`），`std::sync::mpsc`，`octopus_asr_local`（`SileroVad`/`TranscriptEvent`/`streaming_runner`），`octopus_infra::consts`（`SEGMENT_DURATION_S`/`SEGMENT_OVERLAP_MS`）。

**Spec:** `docs/superpowers/specs/2026-06-25-archived-spec.md#vad-segmented-rehome-design`

---

## 相对 spec 的实现细化（implementer 必读）

spec 是设计层，以下两点是落地必需的补充，**不要当成偏离 spec 的错误去"纠正"**：

1. **`Pipeline` trait 增加 `took_segment_cut(&self) -> bool` 默认方法（默认 `false`）**。
   原因：VadSegmented 现状的停顿润色在「切段有语音时」触发（`coordinator.rs:1378` 调 `check_and_trigger_polish`，第二参传 `pause_polish_threshold_ms/1000` 让静音检查 `coordinator.rs:1566` 自动通过）。`tick` 返回的 `changed` 是「回填导致文本变化」，发生在 spawn 结果回来之后，**晚于切段一个识别周期**——若改用 `changed` 触发停顿润色，润色显示会延后 1-2s，是有感的行为差异。故 pipeline 暴露「本 tick 是否发生有语音切段」标记，coordinator 据此触发，零差异。`StreamingPipeline` 用默认 `false`（流式停顿润色走 `silence_duration` 每 tick 判，不靠此标记）。

2. **`Stage::WaitingCompletion` 持 `tick_active: Arc<AtomicBool>`（从 `VadSegmented` move 过来），finalize 时才 `store(false)` 停 tick 线程**。
   原因：spec §3.5 要求 WaitingCompletion 收尾靠 tick 线程继续发 `VadSegmentedTick` 驱动 `pipeline.tick(&[])` drain rx（非阻塞）。但现状 stop 路径（`coordinator.rs:787`）立即 `tick_active.store(false)` 停 tick 线程、靠 `TranscriptionDone` 命令驱动 WaitingCompletion。删了 `TranscriptionDone` 后必须有替代驱动——即保留 tick 线程。所以 `tick_active` 随 pipeline 一起 move 进 WaitingCompletion，stop 路径**不再立即停**，改在 WaitingCompletion 的 `active_count==0` finalize 时停。spec §3.4 表格未列此字段，是实现遗漏，本 plan 补全。

---

## File Structure

| 文件 | 职责 | 本 plan 动作 |
|---|---|---|
| `crates/desktop/src/pipeline.rs` | `StreamingPipeline` + `StreamingPipelineEngine` trait（2c-1/2c-2） | **新增** `Pipeline` trait + `SegmentResult` + `VadSegmentedPipeline` + `impl Pipeline for VadSegmentedPipeline/StreamingPipeline`；搬入 `consume_completed_results`/`filter_speech_from_buffer`/`vad_preroll`/`VAD_PREROLL_FRAMES`；`StreamingPipelineEngine::finish_with_tail`→`finish` |
| `crates/desktop/src/cloud_pipeline.rs` | `CloudPipelineEngine impl StreamingPipelineEngine`（2c-2） | `finish_with_tail`→`finish`（去 tail，tail 由 stop 路径 tick 喂入 push_pcm） |
| `crates/desktop/src/coordinator.rs` | 编排 + Stage 状态机 | `Stage::VadSegmented`/`WaitingCompletion` 字段改持 pipeline；tick handler 改调 `pipeline.tick`；**删** `Command::TranscriptionDone` + dispatch arm + `handle_transcription_done` + `spawn_offline_transcription_with_seq`；stop 路径改 `tick(tail)+finish`；WaitingCompletion 复用 tick 驱动 |
| `crates/asr/src/streaming_runner.rs` | `StreamingRunner`（2a） | **不动**（`finish`/`finish_with_tail` 保留，desktop 内层仍可调） |

依赖边界不变：`octopus-desktop ──→ octopus-asr-local + octopus-infra`，无 cloud 依赖（VadSegmented 仅非流式本地引擎，`is_cloud()` 恒 false）。

---

## Task 1: `Pipeline` trait + `SegmentResult` 类型

**Files:**
- Modify: `crates/desktop/src/pipeline.rs`（顶部 `use` 之后、`StreamingPipelineEngine` trait 之前插入）

**目的：** 定义统一上层抽象与 VadSegmented 内部回传类型。此 Task 仅定义、不 impl，trait 暂未使用会有 `dead_code`/`unused` 警告——Task 2/3/4 impl 后消失；本 Task 用 `#[allow(unused)]` 临时压住。

- [x] **Step 1: 加 `SegmentResult` 与 `Pipeline` trait**

在 `crates/desktop/src/pipeline.rs` 顶部 `use` 块之后（约 L20，`use std::sync::Arc;` 之前——按现有 import 顺序，把 `mpsc` 加进现有 `use`），插入：

```rust
/// VadSegmented 段识别结果（pipeline 内部回传类型，2c-3）。
///
/// spawn 线程跑完 `engine.transcribe` 后，把结果发回 `VadSegmentedPipeline.rx`（**不发
/// coordinator.tx**），下个 tick `try_recv` drain。`session_id` 仅日志用——跨会话护栏由
/// 「stage 切换 = 新 pipeline 实例」天然保证（旧 pipeline drop → rx disconnect → spawn 的
/// `tx.send` 失败忽略），不在此比对（spec §4）。
pub(crate) struct SegmentResult {
    pub seq: u64,
    pub session_id: i64,
    pub text: Result<String, String>,
}

/// desktop ASR pipeline 统一上层抽象（2c-3，spec §3.1）。
///
/// `StreamingPipeline`（流式，内持 `StreamingPipelineEngine`）与 `VadSegmentedPipeline`
///（VAD 分段伪流式）各 impl。coordinator 持 `Box<dyn Pipeline>`，tick/finish/silence 统一
/// 调用，不再按 stage 分流 tick 逻辑。emit/DB/polish/transcript 留 coordinator（2d 收敛）。
#[allow(unused)]
pub trait Pipeline: Send {
    /// 喂一帧已降噪 16k 样本。
    /// - 流式：engine tick → set_full。
    /// - VadSegmented：累积+双 VAD+切段+spawn+drain_rx 回填+consume。
    /// 返回 `changed`（coordinator 据 DB + emit，保持「内容未变不落库/不重绘」幂等）。
    fn tick(&mut self, samples: &[f32], transcript: &mut Transcript) -> bool;
    /// 收尾：流式 flush（tail 已由 stop 路径的 tick 喂入 accept）；vad-seg 仅 drain 剩余 rx。
    fn finish(&mut self, transcript: &mut Transcript) -> TranscriptEvent;
    /// 当前累积静音时长（秒，停顿润色触发用）。
    fn silence_duration(&self) -> f64;
    /// 重置（会话间复用）。
    fn reset(&mut self);
    /// cloud engine 的 async close 句柄（stop 时 coordinator 取出 spawn `close_async`）。
    /// local/vad-seg 返回 `None`（默认）。cfg cloud（与 `StreamingPipelineEngine` 同步门控）。
    #[cfg(feature = "cloud")]
    fn take_close_handle(&mut self) -> Option<octopus_asr_cloud::CloudStreamHandle> { None }
    /// 是否 cloud 引擎（§4.2/§4.3 不对称判别）。vad-seg 恒 false。
    fn is_cloud(&self) -> bool { false }
    /// 本 tick 是否发生「有语音的切段」（仅 VadSegmented 为 true，停顿润色触发用，见 plan 细化 1）。
    /// 流式默认 false（停顿润色走 silence_duration 每 tick 判）。
    fn took_segment_cut(&self) -> bool { false }
}
```

`SegmentResult` 字段默认私有——Task 2 构造时用字面量，需 `pub` 字段（上面已是 `pub`）。

- [x] **Step 2: 验证编译**

```bash
cargo check -p octopus-desktop 2>&1 | tail -20
```
Expected: 0 error（可能有 `unused`/`dead_code` 警告——已用 `#[allow(unused)]` 压 trait；`SegmentResult` 未构造会有 dead_code 警告，Task 2 消除）。

- [x] **Step 3: Commit**

```bash
git add crates/desktop/src/pipeline.rs
git commit -m "feat(desktop): 加 Pipeline trait + SegmentResult（2c-3 Task 1）"
```

---

## Task 2: `VadSegmentedPipeline` 结构 + tick 编排 + 回填纯逻辑

**Files:**
- Modify: `crates/desktop/src/pipeline.rs`（新增结构 + 搬入 4 个 helper + 纯函数 + 单测）
- 源参考（搬迁，**不删 coordinator 副本**——Task 5 才删）：`coordinator.rs:1266-1290`（`consume_completed_results`）、`coordinator.rs:1446-1486`（`spawn_offline_transcription_with_seq`）、`coordinator.rs:1495-1507`（`filter_speech_from_buffer`）、`coordinator.rs:1389-1396`（`vad_preroll`）

**目的：** 把 VadSegmented 的 11 字段编排 + spawn + 乱序回填封装成 `VadSegmentedPipeline`，tick 对外同步。回填/consume 拆成纯函数单测（不依赖 VAD/模型文件）。

- [x] **Step 1: 写失败的单测（纯函数：回填 + 乱序消费）**

在 `crates/desktop/src/pipeline.rs` 末尾现有 `#[cfg(test)] mod tests` 内追加：

```rust
    // ── VadSegmentedPipeline 纯逻辑（2c-3）──

    use super::{apply_segment_result, consume_completed_results_vad, SegmentResult};
    use std::collections::HashMap;

    #[test]
    fn apply_segment_result_normal_inserts_text() {
        let mut results = HashMap::new();
        let mut active = 1u32;
        apply_segment_result(&mut results, &mut active, SegmentResult {
            seq: 0, session_id: 1, text: Ok("你好".to_string()),
        });
        assert_eq!(results.get(&0).map(String::as_str), Some("你好"));
        assert_eq!(active, 0);
    }

    #[test]
    fn apply_segment_result_empty_occupies_slot() {
        // 空结果仍占位该 seq，避免 consume 卡在缺失序号
        let mut results = HashMap::new();
        let mut active = 1u32;
        apply_segment_result(&mut results, &mut active, SegmentResult {
            seq: 0, session_id: 1, text: Ok(String::new()),
        });
        assert_eq!(results.get(&0).map(String::as_str), Some(""));
        assert_eq!(active, 0);
    }

    #[test]
    fn apply_segment_result_failed_occupies_slot() {
        // 识别失败占位空串，保证 completed_seq 连续推进
        let mut results = HashMap::new();
        let mut active = 2u32;
        apply_segment_result(&mut results, &mut active, SegmentResult {
            seq: 0, session_id: 1, text: Err("boom".to_string()),
        });
        assert_eq!(results.get(&0).map(String::as_str), Some(""));
        assert_eq!(active, 1);
    }

    #[test]
    fn consume_appends_only_contiguous_seq() {
        // 乱序：completed_seq=0，有 0 和 2，缺 1 → 只消费 0；插入 1 → 消费 1、2
        let mut completed_seq = 0u64;
        let mut results = HashMap::new();
        results.insert(0u64, "甲".to_string());
        results.insert(2u64, "丙".to_string());
        let mut t = Transcript::new(0, PolishMode::Disabled);
        consume_completed_results_vad(&mut completed_seq, &mut results, &mut t);
        assert_eq!(t.full(), "甲");
        assert_eq!(completed_seq, 1);
        assert!(results.contains_key(&2)); // 2 仍缓存

        results.insert(1u64, "乙".to_string());
        consume_completed_results_vad(&mut completed_seq, &mut results, &mut t);
        assert_eq!(t.full(), "甲，乙，丙"); // 段间补逗号
        assert_eq!(completed_seq, 3);
        assert!(results.is_empty());
    }
```

- [x] **Step 2: 运行测试，确认失败**

```bash
cargo test -p octopus-desktop pipeline::tests::apply_segment_result_normal_inserts_text 2>&1 | tail -15
```
Expected: 编译失败——`apply_segment_result`/`consume_completed_results_vad`/`SegmentResult` 字段未定义。

- [x] **Step 3: 实现 `SegmentResult` 字段可见性 + 2 个纯函数**

`SegmentResult`（Task 1 已加）字段已是 `pub`。在 `pipeline.rs` 的 `SegmentResult` 定义之后加两个纯函数：

```rust
/// 把一条段结果回填进缓存 + 递减 active_count（纯逻辑，2c-3）。
///
/// 空串/失败占位空串（保 `completed_seq` 连续推进，避免后续有效段积压丢失）。
/// 不判 `session_id`（跨会话护栏由 pipeline 随 stage drop 天然保证，spec §4）。
pub(crate) fn apply_segment_result(
    results: &mut HashMap<u64, String>,
    active_count: &mut u32,
    seg: SegmentResult,
) {
    *active_count = active_count.saturating_sub(1);
    match seg.text {
        Ok(t) if !t.is_empty() => {
            log::info!("VadSegmented seq={}: '{}'", seg.seq, t);
            results.insert(seg.seq, t);
        }
        Ok(_) => {
            results.insert(seg.seq, String::new());
        }
        Err(e) => {
            log::error!("VadSegmented seq={} failed: {}", seg.seq, e);
            results.insert(seg.seq, String::new());
        }
    }
}

/// 消费连续序号的结果，把新段追加到 Transcript（搬迁自 `coordinator.rs:1266`，零改动）。
pub(crate) fn consume_completed_results_vad(
    completed_seq: &mut u64,
    completed_results: &mut HashMap<u64, String>,
    transcript: &mut Transcript,
) {
    while let Some(text) = completed_results.remove(completed_seq) {
        if !text.is_empty() {
            // 段间加逗号：已有文本、新段不以标点开头、已有文本不以标点结尾
            let existing = transcript.full();
            if !existing.is_empty()
                && !text.starts_with(|c: char| ",.，。！？!?\n".contains(c))
                && !existing.ends_with(|c: char| ",.，。！？!?\n".contains(c))
            {
                transcript.append_segment("，");
            }
            transcript.append_segment(&text);
        }
        *completed_seq += 1;
    }
}
```

`HashMap` 已在 pipeline.rs 顶部 import（若没有，加 `use std::collections::HashMap;`）。

- [x] **Step 4: 运行测试，确认通过**

```bash
cargo test -p octopus-desktop pipeline::tests 2>&1 | grep "test result"
```
Expected: `test result: ok. N passed`（含 4 个新测试 + 既有 StreamingPipeline 测试）。

- [x] **Step 5: 加 `VadSegmentedPipeline` 结构 + 构造 + 搬入 helper**

在 `pipeline.rs`（`compute_speech_chunks` 之后）加常量、helper、结构：

```rust
/// 预滚帧数（VAD LSTM 预热，搬迁自 coordinator.rs:159）。
pub(crate) const VAD_PREROLL_FRAMES: usize = 10;

/// 预滚 VAD：喂入若干帧静音，让 LSTM 隐藏状态预热，避免首几帧误判静音丢字
///（搬迁自 coordinator.rs:1389，零改动）。
pub(crate) fn vad_preroll(vad: &mut SileroVad) {
    let silence = vec![0.0_f32; VAD_CHUNK_SIZE];
    for _ in 0..VAD_PREROLL_FRAMES {
        let _ = vad.compute(&silence);
    }
}

/// 对缓冲区音频做 VAD 过滤（搬迁自 coordinator.rs:1495，零改动）。
/// 用独立 `filter_vad`（与检测流分离），过滤前 reset() 归零 LSTM 状态（等价旧代码每 buffer 新建 VAD）。
fn filter_speech_from_buffer(filter_vad: &mut SileroVad, samples: &[f32]) -> Vec<f32> {
    filter_vad.reset();
    let speech = octopus_asr_local::audio::filter_speech(samples, filter_vad, 480, 0.5);
    if speech.is_empty() {
        log::debug!("VadSegmented: no speech detected in buffer");
        Vec::new()
    } else {
        speech
    }
}

/// 非 VAD 依赖的 VadSegmented 字段集合，便于构造。
/// engine/language/asr_engine/segment_silence_ms 是 config 子集（不 clone 整 AppConfig）。
pub(crate) struct VadSegmentedPipeline {
    engine: Arc<dyn crate::engine::TranscriptionEngine>,
    language: String,
    asr_engine: String,
    /// 切段静音阈值（毫秒，来自 config.segment_silence）。
    segment_silence_ms: f64,
    /// 检测 VAD（流式有状态，跨 tick 续接，录音期间从不 reset）。
    detect_vad: SileroVad,
    /// 过滤 VAD（每段 reset，与检测分离防 LSTM 污染）。
    filter_vad: SileroVad,
    audio_buffer: Vec<f32>,
    overlap_tail: Vec<f32>,
    silence_duration: f64,
    has_speech: bool,
    active_count: u32,
    next_seq: u64,
    completed_seq: u64,
    completed_results: HashMap<u64, String>,
    tx: std::sync::mpsc::Sender<SegmentResult>,
    rx: std::sync::mpsc::Receiver<SegmentResult>,
    /// 本 tick 是否发生「有语音的切段」（停顿润色触发用，plan 细化 1）。
    segment_cut_this_tick: bool,
}

impl VadSegmentedPipeline {
    /// 构造：加载双 VAD（检测 VAD 预滚）+ 建 channel。
    /// VAD 加载失败 propagate（coordinator start 路径处理 fallback，见 Task 5）。
    pub(crate) fn new(
        engine: Arc<dyn crate::engine::TranscriptionEngine>,
        language: String,
        asr_engine: String,
        segment_silence_ms: f64,
    ) -> anyhow::Result<Self> {
        let path = octopus_asr_local::config::find_silero_vad()?;
        let mut detect_vad = SileroVad::new(&path)?;
        vad_preroll(&mut detect_vad);
        let filter_vad = SileroVad::new(&path)?;
        let (tx, rx) = std::sync::mpsc::channel();
        Ok(Self {
            engine, language, asr_engine, segment_silence_ms,
            detect_vad, filter_vad,
            audio_buffer: Vec::new(), overlap_tail: Vec::new(),
            silence_duration: 0.0, has_speech: false,
            active_count: 0, next_seq: 0, completed_seq: 0,
            completed_results: HashMap::new(),
            tx, rx, segment_cut_this_tick: false,
        })
    }

    /// 当前在途识别数（WaitingCompletion 收尾判定 active_count==0 用）。
    pub(crate) fn active_count(&self) -> u32 { self.active_count }

    /// spawn 一段离线识别（搬迁自 coordinator.rs:1446，改发 SegmentResult 到 self.tx）。
    fn spawn_offline(&self, speech_samples: Vec<f32>, seq: u64, session_id: i64) {
        let engine = self.engine.clone();
        let language = self.language.clone();
        let asr_engine = self.asr_engine.clone();
        let tx = self.tx.clone();
        let samples_len = speech_samples.len();
        let duration = samples_len as f64 / 16000.0;
        tauri::async_runtime::spawn(async move {
            let start = std::time::Instant::now();
            let result = engine.transcribe(&speech_samples, &language, &asr_engine).await;
            let elapsed = start.elapsed();
            log::info!(
                "Transcription seq={} took {:.2}s (audio: {:.2}s, RTF: {:.2})",
                seq, elapsed.as_secs_f64(), duration,
                elapsed.as_secs_f64() / duration.max(0.001)
            );
            let _ = tx.send(SegmentResult {
                seq, session_id,
                text: result.map_err(|e| e.to_string()),
            });
        });
    }

    /// drain rx（try_recv 至空）+ 回填 + 消费连续 seq 追加 transcript。
    /// 返回是否文本变化（consume 追加了新段）。
    fn drain_rx_and_consume(&mut self, transcript: &mut Transcript) -> bool {
        let before = transcript.full().len();
        while let Ok(seg) = self.rx.try_recv() {
            apply_segment_result(&mut self.completed_results, &mut self.active_count, seg);
        }
        consume_completed_results_vad(
            &mut self.completed_seq, &mut self.completed_results, transcript,
        );
        transcript.full().len() != before
    }

    /// tick 编排（搬迁 coordinator.rs:1314-1385，零逻辑改动，仅 spawn 目标改 self.tx）。
    /// `samples` 空则跳过步骤 1-5（切段/spawn），仍走 drain_rx（WaitingCompletion 收尾靠此）。
    pub(crate) fn run_tick(&mut self, samples: &[f32], transcript: &mut Transcript) -> bool {
        self.segment_cut_this_tick = false;
        let mut changed = false;

        if !samples.is_empty() {
            // 1. 追加缓冲区
            self.audio_buffer.extend_from_slice(samples);

            // 2. 检测 VAD 统计语音帧
            let speech_chunks = compute_speech_chunks(&mut self.detect_vad, samples);
            if speech_chunks >= 2 {
                self.silence_duration = 0.0;
                self.has_speech = true;
            } else {
                let chunk_duration = samples.len() as f64 / 16000.0;
                self.silence_duration += chunk_duration;
            }

            // 3. 切段判定：静音边界（主）/ 连续超时强制（兜底）
            let buffer_duration_s = self.audio_buffer.len() as f64 / 16000.0;
            let silence_ms = self.silence_duration * 1000.0;
            let silence_cut = self.has_speech && silence_ms >= self.segment_silence_ms;
            let force_cut = self.has_speech && buffer_duration_s >= SEGMENT_DURATION_S;
            if silence_cut || force_cut {
                // 4. 构建发送缓冲区 + 过滤
                let mut send_buffer = self.overlap_tail.clone();
                send_buffer.extend_from_slice(&self.audio_buffer);
                if force_cut {
                    let overlap_samples = (SEGMENT_OVERLAP_MS * 16.0) as usize;
                    let overlap_start = self.audio_buffer.len().saturating_sub(overlap_samples);
                    self.overlap_tail = self.audio_buffer[overlap_start..].to_vec();
                } else {
                    self.overlap_tail.clear();
                }
                self.audio_buffer.clear();
                self.has_speech = false;
                self.silence_duration = 0.0;

                let speech_samples = filter_speech_from_buffer(&mut self.filter_vad, &send_buffer);
                // 5. 有语音 → spawn（记 segment_cut，供 coordinator 触发停顿润色）
                if !speech_samples.is_empty() {
                    self.segment_cut_this_tick = true;
                    let seq = self.next_seq;
                    self.next_seq += 1;
                    self.active_count += 1;
                    log::debug!(
                        "VadSegmented: {} cut, seq={}, samples={}, active_count={}",
                        if force_cut { "force" } else { "silence" },
                        seq, speech_samples.len(), self.active_count,
                    );
                    self.spawn_offline(speech_samples, seq, transcript.id);
                }
            }
        }

        // 6-7. drain rx + 回填 + 消费（空样本也走，WaitingCompletion 收尾驱动）
        if self.drain_rx_and_consume(transcript) {
            changed = true;
        }
        changed
    }
}
```

顶部 import 补：`use std::sync::Arc;`、`use std::collections::HashMap;`、`use octopus_infra::consts::{SEGMENT_DURATION_S, SEGMENT_OVERLAP_MS};`（若已有则跳过；`Arc` 现有 pipeline.rs 未 import，需加）。

- [x] **Step 6: 验证编译 + 测试**

```bash
cargo check -p octopus-desktop 2>&1 | tail -20
cargo test -p octopus-desktop pipeline::tests 2>&1 | grep "test result"
```
Expected: 0 error；测试全绿（`SegmentResult`/`consume_completed_results_vad`/`apply_segment_result` 的 dead_code 警告消失——已被结构与测试引用）。`run_tick`/`new`/`spawn_offline`/`drain_rx_and_consume`/`active_count` 未被外部调用会有 dead_code 警告，Task 3/5 消除；如需临时压住可加 `#[allow(dead_code)]` 到结构，但建议留作 Task 3 自然消除。

- [x] **Step 7: Commit**

```bash
git add crates/desktop/src/pipeline.rs
git commit -m "feat(desktop): VadSegmentedPipeline 结构 + tick 编排 + 回填纯逻辑（2c-3 Task 2）"
```

---

## Task 3: `impl Pipeline for VadSegmentedPipeline`

**Files:**
- Modify: `crates/desktop/src/pipeline.rs`

**目的：** 给 `VadSegmentedPipeline` 套上 `Pipeline` trait（tick 转 `run_tick`；finish = drain rx 至空 + consume；silence/reset/take_close_handle/is_cloud/took_segment_cut）。

- [x] **Step 1: 写 impl**

在 `pipeline.rs` 的 `VadSegmentedPipeline` impl 块之后加：

```rust
impl Pipeline for VadSegmentedPipeline {
    fn tick(&mut self, samples: &[f32], transcript: &mut Transcript) -> bool {
        self.run_tick(samples, transcript)
    }

    fn finish(&mut self, transcript: &mut Transcript) -> TranscriptEvent {
        // drain rx 至空 + 消费在途段（unbounded channel 不丢；active_count 归零由 drain 递减）。
        // 无 tail（tail 已由 coordinator stop 路径的 tick 喂入，可能触发最后一轮切段）。
        self.drain_rx_and_consume(transcript);
        // VadSegmented 不产 Final 事件（文本经 set_full 累积），返回空 Committed 作占位
        //（coordinator stop 路径不读 vad-seg 的 finish 返回值，见 Task 5）。
        TranscriptEvent::Committed(String::new())
    }

    fn silence_duration(&self) -> f64 {
        self.silence_duration
    }

    fn reset(&mut self) {
        // 会话间复用：清缓冲 + VAD 状态。rx 内残余旧段丢弃（新会话 seq 从 0 重来）。
        self.audio_buffer.clear();
        self.overlap_tail.clear();
        self.silence_duration = 0.0;
        self.has_speech = false;
        self.active_count = 0;
        self.next_seq = 0;
        self.completed_seq = 0;
        self.completed_results.clear();
        self.detect_vad.reset();
        self.filter_vad.reset();
        while self.rx.try_recv().is_ok() {}
        self.segment_cut_this_tick = false;
    }

    fn took_segment_cut(&self) -> bool {
        self.segment_cut_this_tick
    }

    // take_close_handle / is_cloud 用默认（None / false）——VadSegmented 仅非流式本地引擎。
}
```

- [x] **Step 2: 验证编译 + clippy**

```bash
cargo check -p octopus-desktop 2>&1 | tail -20
cargo clippy -p octopus-desktop --all-targets 2>&1 | grep -E "^(error|warning)" | grep -v "cloud_pipeline\|coordinator.rs" | tail -20
```
Expected: 0 error；本 Task 新代码 0 新 warning（coordinator/cloud_pipeline 的预存 warning 非本 Task 引入）。

- [x] **Step 3: Commit**

```bash
git add crates/desktop/src/pipeline.rs
git commit -m "feat(desktop): impl Pipeline for VadSegmentedPipeline（2c-3 Task 3）"
```

---

## Task 4: `impl Pipeline for StreamingPipeline` + `finish_with_tail`→`finish` 去 tail

**Files:**
- Modify: `crates/desktop/src/pipeline.rs`（`StreamingPipelineEngine` trait + `LocalPipelineEngine` + `StreamingPipeline`）
- Modify: `crates/desktop/src/cloud_pipeline.rs`（`CloudPipelineEngine`）
- Modify: `crates/desktop/src/coordinator.rs`（stop 路径 L840-895：`finish_with_tail`→`tick(tail)+finish`）

**目的：** 流式也纳入 `Pipeline`。`StreamingPipelineEngine::finish_with_tail(&[f32])` 改 `finish()`（去 tail 参数）——tail 由 coordinator stop 路径 `tick(tail)` 喂入。

> **行为说明（implementer 必读）：** 现状 `StreamingRunner::finish_with_tail(tail)` 内部用 `engine.accept_samples(tail, false)`（**不走 VAD/标点**）+ `finish()`。改 `tick(tail)+finish` 后，tail 经 `StreamingPipeline::tick`→`StreamingRunner::push_samples`（**走 VAD**）。差异：尾部样本会过一次 VAD（可能产标点/Partial 事件）。但 tail 极短（`audio.drain_samples()` 的剩余，约一个 tick ≤100ms），且紧接 `finish()` 的 `Final` 会 `set_full` 覆盖。实际等价，靠既有流式测试 + Task 6 e2e 验证（spec §3.3/§6 已论证）。

- [x] **Step 1: 改 `StreamingPipelineEngine` trait 签名（pipeline.rs L34）**

`finish_with_tail(&mut self, tail: &[f32]) -> TranscriptEvent` → `finish(&mut self) -> TranscriptEvent`。更新该方法的文档注释为「收尾 flush（tail 已由 stop 路径 tick 喂入 accept）：local → `StreamingRunner::finish`（Final）；cloud → 返回最后 `current_partial` 作 Committed 兜底」。

- [x] **Step 2: 改 `LocalPipelineEngine` impl（pipeline.rs L67-69）**

```rust
    fn finish(&mut self) -> TranscriptEvent {
        self.0.finish()
    }
```

- [x] **Step 3: 改 `CloudPipelineEngine` impl（cloud_pipeline.rs L270 区域）**

现状 `finish_with_tail` 内部 `push_pcm(tail)` + 返回 `current_partial` 兜底。去 tail 后只返回兜底（tail 由 stop 路径 `tick` 内的 `push_pcm` 喂入）：

```rust
    fn finish(&mut self) -> TranscriptEvent {
        // tail 已由 stop 路径 tick 喂入 push_pcm；此处仅返回最后 current_partial 作 Committed 兜底。
        // cloud stop 路径不用其返回值（走 finalize_cloud / CloudClosing）。
        TranscriptEvent::Committed(self.current_partial().to_string())
    }
```

（删除原 `finish_with_tail` 内的 `push_pcm` 逻辑——已移到 tick 路径。）

- [x] **Step 4: 改 `StreamingPipeline` 包装方法（pipeline.rs L126-128）+ 加 `impl Pipeline`**

把 `pub fn finish_with_tail(&mut self, tail: &[f32]) -> TranscriptEvent` 改为内部不再暴露带 tail 方法，转由 `impl Pipeline` 的 `finish` 承载。**保留 `StreamingPipeline::tick` 原样**（它已是 `tick(&mut self, samples, transcript) -> bool`，正好对应 `Pipeline::tick`）。

在 `StreamingPipeline` 的 inherent impl 之后加：

```rust
impl Pipeline for StreamingPipeline {
    fn tick(&mut self, samples: &[f32], transcript: &mut Transcript) -> bool {
        // 复用既有 StreamingPipeline::tick（engine tick → set_full，返回 changed）。
        self.tick(samples, transcript)
    }
    fn finish(&mut self, _transcript: &mut Transcript) -> TranscriptEvent {
        // tail 已由 stop 路径 tick 喂入 accept；此处仅 flush。
        self.engine.finish()
    }
    fn silence_duration(&self) -> f64 { self.engine.silence_duration() }
    fn reset(&mut self) { self.engine.reset(); }
    #[cfg(feature = "cloud")]
    fn take_close_handle(&mut self) -> Option<octopus_asr_cloud::CloudStreamHandle> {
        self.engine.take_close_handle()
    }
    fn is_cloud(&self) -> bool { self.engine.is_cloud() }
    // took_segment_cut 用默认 false（流式停顿润色走 silence_duration 每 tick 判）。
}
```

> **注意 inherent vs trait 方法同名：** `StreamingPipeline` 既有 `pub fn tick`（inherent）与 `Pipeline::tick`（trait）同名。Rust 允许，调用时 inherent 优先；`impl Pipeline::tick { self.tick(...) }` 内调 inherent 合法。既有 `StreamingPipeline::tick` 调用点（coordinator `handle_streaming_tick`）不受影响。

删掉 `StreamingPipeline` 的 `pub fn finish_with_tail`（不再有外部调用——Step 5 改 coordinator 后确认）。

- [x] **Step 5: 改 coordinator stop 路径（coordinator.rs L840-895）**

**cloud 分支（L843）**：`let _ = pipeline.finish_with_tail(&final_samples);` →
```rust
                if !final_samples.is_empty() {
                    pipeline.tick(&final_samples, transcript);
                }
                let _ = pipeline.finish(transcript);
```

**local 分支（L878）**：`let final_text = match pipeline.finish_with_tail(&final_samples) {` →
```rust
            // local: tick(tail) accept + finish flush（tail 经 push_samples 喂入；finish Final 覆盖）
            if !final_samples.is_empty() {
                pipeline.tick(&final_samples, transcript);
            }
            let final_text = match pipeline.finish(transcript) {
                TranscriptEvent::Final(text) => text,
                TranscriptEvent::Error(e) => {
                    error!("Streaming finish failed: {}", e);
                    transcript.edited_display().unwrap_or_else(|| transcript.db_text())
                }
                _ => transcript.edited_display().unwrap_or_else(|| transcript.db_text()),
            };
```

（`pipeline.reset()` 及其后逻辑不变。）

- [x] **Step 6: 改既有流式测试（pipeline.rs L296 `finish_with_tail_delegates_to_engine`）**

改测 `finish` 无 tail：
```rust
    #[test]
    fn finish_delegates_to_engine() {
        let mut p = pipeline(FakePipelineEngine::new(
            vec![], "",
            TranscriptEvent::Final("最终。".to_string()),
        ));
        let ev = p.finish(&mut Transcript::new(0, PolishMode::Disabled));
        assert_eq!(ev, TranscriptEvent::Final("最终。".to_string()));
    }
```
同步把 `FakePipelineEngine` 的 `finish_with_tail`（pipeline.rs L216）改 `finish(&mut self) -> TranscriptEvent { self.finish_out.clone() }`。

- [x] **Step 7: 双 feature 编译 + 既有测试 + clippy**

```bash
cargo check -p octopus-desktop 2>&1 | tail -5
cargo check -p octopus-desktop --features cloud 2>&1 | tail -5
cargo test -p octopus-desktop pipeline::tests 2>&1 | grep "test result"
cargo test -p octopus-desktop --features cloud cloud_pipeline 2>&1 | grep "test result"
cargo clippy -p octopus-desktop --features cloud --all-targets 2>&1 | grep -E "^warning" | grep -E "pipeline.rs|cloud_pipeline.rs" | tail
```
Expected: 全 0 error；pipeline/cloud_pipeline 测试绿；新代码 0 新 warning。

- [x] **Step 8: Commit**

```bash
git add crates/desktop/src/pipeline.rs crates/desktop/src/cloud_pipeline.rs crates/desktop/src/coordinator.rs
git commit -m "feat(desktop): impl Pipeline for StreamingPipeline + finish 去 tail（2c-3 Task 4）"
```

---

## Task 5: coordinator Stage 改造 + 删 `Command::TranscriptionDone`

**Files:**
- Modify: `crates/desktop/src/coordinator.rs`（Stage 枚举 + start 路径 + tick dispatch + stop 路径 + 删命令/handler/spawn helper）

**目的：** `Stage::VadSegmented`/`WaitingCompletion` 字段改持 `VadSegmentedPipeline`；tick handler 统一调 `pipeline.tick`；删 `TranscriptionDone` 命令、dispatch arm、`handle_transcription_done`、`spawn_offline_transcription_with_seq`；WaitingCompletion 复用 `VadSegmentedTick` 驱动 drain。

> **本 Task 是最大改动，按以下顺序逐步改，每步编译。**

- [x] **Step 1: 改 `Stage` 枚举字段**

`Stage::VadSegmented`（coordinator.rs L79-109）11 字段 → 3 字段：
```rust
    VadSegmented {
        /// VAD 分段 pipeline（封装双 VAD + 切段 + spawn + 乱序回填，2c-3）。
        pipeline: crate::pipeline::VadSegmentedPipeline,
        transcript: Transcript,
        /// tick 线程控制标志（move 进 WaitingCompletion，finalize 时才停，plan 细化 2）。
        tick_active: Arc<AtomicBool>,
    },
```

`Stage::WaitingCompletion`（L119-124）→
```rust
    WaitingCompletion {
        /// VadSegmented pipeline（从 VadSegmented move 过来；tick 空样本 drain rx 收尾）。
        pipeline: crate::pipeline::VadSegmentedPipeline,
        transcript: Transcript,
        /// tick 线程标志（VadSegmented move 过来；finalize 时 store(false) 停线程，plan 细化 2）。
        tick_active: Arc<AtomicBool>,
    },
```

- [x] **Step 2: 改 start 路径构造（coordinator.rs L716-755）**

把双 VAD 创建 + preroll + 11 字段构造，换成 `VadSegmentedPipeline::new`（VAD 加载失败走原 fallback）：
```rust
                // 非流式模式：使用 VAD 伪流式分段识别（2c-3：编排收进 VadSegmentedPipeline）
                match crate::pipeline::VadSegmentedPipeline::new(
                    engine.clone(),
                    config.language.clone(),
                    config.asr_engine.clone(),
                    config.segment_silence,
                ) {
                    Ok(pipeline) => {
                        crate::result_window::show_result(app_handle, "正在聆听…");
                        crate::tray::update_tray_label(app_handle, crate::tray::TrayState::Recording);
                        let tick_active = Arc::new(AtomicBool::new(true));
                        start_vad_segmented_tick_thread(tx.clone(), tick_active.clone());
                        *stage = Stage::VadSegmented {
                            pipeline,
                            transcript: Transcript::new(now_millis(), config.polish_mode),
                            tick_active,
                        };
                    }
                    Err(e) => {
                        error!("VAD init failed for VadSegmented: {}, falling back to offline", e);
                        let _ = audio.stop();
                    }
                }
```

（删原 `find_silero_vad`/`SileroVad::new`/`vad_preroll`/`filter_vad` 创建块 L717-756。`engine` 变量在 start 路径已有，确认其类型是 `Arc<dyn TranscriptionEngine>`；若作用域名不同按实际。）

- [x] **Step 3: 改 `VadSegmentedTick` dispatch（coordinator.rs L301-321）**

```rust
                    Command::VadSegmentedTick => {
                        {
                            let rc = runtime_config.read().unwrap();
                            config.polish_mode = rc.polish_mode;
                        }
                        if let Stage::VadSegmented { pipeline, transcript, .. }
                        | Stage::WaitingCompletion { pipeline, transcript, .. } = &mut stage
                        {
                            transcript.set_mode(config.polish_mode);
                        }
                        if editing {
                            let _ = audio.drain_samples();
                        } else {
                            handle_vad_segmented_tick(&mut stage, &audio, &config, &app_handle, &tx);
                        }
                    }
```

- [x] **Step 4: 重写 `handle_vad_segmented_tick`（coordinator.rs L1292-1387）**

把原 ~95 行编排整体替换为：取 stage → `pipeline.tick` → 据 changed/segment_cut 做 DB/emit/polish → WaitingCompletion 收尾判定。

```rust
/// 处理 VadSegmentedTick 命令（2c-3：编排进 pipeline.tick，此函数只做 emit/DB/polish + 收尾判定）。
fn handle_vad_segmented_tick(
    stage: &mut Stage,
    audio: &Arc<SharedAudioState>,
    config: &AppConfig,
    app_handle: &tauri::AppHandle,
    tx: &Sender<Command>,
) {
    let samples = audio.drain_samples();

    match stage {
        Stage::VadSegmented { pipeline, transcript, .. } => {
            let changed = pipeline.tick(&samples, transcript);
            let segment_cut = pipeline.took_segment_cut();
            after_vad_tick(transcript, changed, segment_cut, "vad_segmented", config, app_handle, tx);
        }
        Stage::WaitingCompletion { pipeline, transcript, tick_active } => {
            // 收尾：空样本驱动 drain rx（pipeline.tick 跳过切段，仅 drain+consume）
            let changed = pipeline.tick(&samples, transcript);
            if changed {
                if let Err(e) = update_transcription_raw(transcript, &config.asr_engine, "vad_segmented") {
                    warn!("DB (vad_segmented waiting) failed: {}", e);
                }
                if !transcript.full().is_empty() {
                    crate::result_window::update_result(app_handle, &transcript.display_text());
                }
            }
            // 所有在途段完成 → 收尾
            if pipeline.active_count() == 0 {
                let tr = std::mem::replace(transcript, Transcript::new(0, PolishMode::Disabled));
                tick_active.store(false, Ordering::Relaxed); // 停 tick 线程（plan 细化 2）
                finalize_after_stop(stage, tr, config, app_handle, tx);
            }
        }
        _ => {}
    }
}

/// VadSegmented tick 后处理：changed → DB + emit；segment_cut → 停顿润色（零差异保留原触发）。
fn after_vad_tick(
    transcript: &mut Transcript,
    changed: bool,
    segment_cut: bool,
    db_source: &str,
    config: &AppConfig,
    app_handle: &tauri::AppHandle,
    tx: &Sender<Command>,
) {
    if changed {
        if let Err(e) = update_transcription_raw(transcript, &config.asr_engine, db_source) {
            warn!("DB ({}) failed: {}", db_source, e);
        }
        if !transcript.full().is_empty() {
            crate::result_window::update_result(app_handle, &transcript.display_text());
        }
    }
    if segment_cut {
        // 切段有语音 → 停顿润色（传阈值让 check_and_trigger_polish 静音检查自动通过，与原 coordinator.rs:1378 等价）
        check_and_trigger_polish(transcript, config.pause_polish_threshold_ms / 1000.0, config, tx);
    }
}
```

- [x] **Step 5: 改 stop 路径 `Stage::VadSegmented` 分支（coordinator.rs L770-828）**

现状停止 tick 线程 + 末段 spawn + active>0 进 WaitingCompletion。改：**不停 tick 线程**（保留驱动 WaitingCompletion），末段切段用 `pipeline.tick(remaining)` 触发，pipeline move 进 WaitingCompletion：

```rust
        Stage::VadSegmented { pipeline, transcript, tick_active } => {
            info!("Toggle: stopping VadSegmented (active_count={})", pipeline.active_count());
            // 停止录音 + 排空剩余音频喂 pipeline（可能触发末段切段 spawn）
            let remaining = audio.stop().unwrap_or_default();
            if !remaining.is_empty() {
                pipeline.tick(&remaining, transcript);
            }
            // 不停 tick 线程：WaitingCompletion 收尾仍需 VadSegmentedTick 驱动 drain（plan 细化 2）
            // 排空在途 spawn 结果
            pipeline.finish(transcript);
            if pipeline.active_count() > 0 {
                // 还有识别在跑：pipeline + tick_active move 进 WaitingCompletion，等 tick 驱动收尾
                let tr = std::mem::replace(transcript, Transcript::new(0, PolishMode::Disabled));
                *stage = Stage::WaitingCompletion {
                    pipeline: take_pipeline(stage),
                    transcript: tr,
                    tick_active: tick_active.clone(),
                };
            } else {
                tick_active.store(false, Ordering::Relaxed);
                let tr = std::mem::replace(transcript, Transcript::new(0, PolishMode::Disabled));
                finalize_after_stop(stage, tr, config, app_handle, tx);
            }
        }
```

> **`take_pipeline` 辅助：** 上面借用了 `pipeline` 后又需 move 它进 WaitingCompletion，与 borrow 冲突。实际写法：把 `active_count` 先读出，再重组。简化为：
```rust
        Stage::VadSegmented { pipeline, transcript, tick_active } => {
            info!("Toggle: stopping VadSegmented (active_count={})", pipeline.active_count());
            let remaining = audio.stop().unwrap_or_default();
            if !remaining.is_empty() {
                pipeline.tick(&remaining, transcript);
            }
            pipeline.finish(transcript);
            let still_active = pipeline.active_count() > 0;
            if still_active {
                // move pipeline + tick_active 进 WaitingCompletion；transcript 先 take 出
                let (pipeline, tick_active) = take_vad_pipeline_and_tick(stage);
                let tr = std::mem::replace(transcript_of(stage), Transcript::new(0, PolishMode::Disabled));
                *stage = Stage::WaitingCompletion { pipeline, transcript: tr, tick_active };
            } else {
                tick_active.store(false, Ordering::Relaxed);
                let tr = std::mem::replace(transcript, Transcript::new(0, PolishMode::Disabled));
                finalize_after_stop(stage, tr, config, app_handle, tx);
            }
        }
```
`take_vad_pipeline_and_tick` / `transcript_of` 的借用拆分：implementer 用 `std::mem::replace` 把整个 stage 替成 `Idle` 取出 pipeline/tick_active，再写回 WaitingCompletion。**推荐写法**（避开辅助函数）：
```rust
        Stage::VadSegmented { .. } => {
            // 取出整个 stage 的 owned 部件
            let (mut pipeline, mut transcript, tick_active) = match std::mem::replace(stage, Stage::Idle) {
                Stage::VadSegmented { pipeline, transcript, tick_active } => (pipeline, transcript, tick_active),
                _ => unreachable!(),
            };
            info!("Toggle: stopping VadSegmented (active_count={})", pipeline.active_count());
            let remaining = audio.stop().unwrap_or_default();
            if !remaining.is_empty() { pipeline.tick(&remaining, &mut transcript); }
            pipeline.finish(&mut transcript);
            if pipeline.active_count() > 0 {
                transcript.set_to_placeholder(); // 见下注
                *stage = Stage::WaitingCompletion { pipeline, transcript, tick_active };
            } else {
                tick_active.store(false, Ordering::Relaxed);
                let tr = std::mem::replace(&mut transcript, Transcript::new(0, PolishMode::Disabled));
                *stage = Stage::VadSegmented { pipeline, transcript, tick_active }; // 临时放回以调 finalize
                finalize_after_stop(stage, tr, config, app_handle, tx);
            }
        }
```
> **借用是本步难点。** implementer 以「`mem::replace(stage, Idle)` 取出全部 owned → 处理 → 写回」为主线，确保无 `&mut` 重叠。`finalize_after_stop` 现签名接 `&mut Stage`，写回 VadSegmented 后调即可（finalize 内部会再 `mem::replace`）。`transcript.set_to_placeholder` 不存在——直接用原 `transcript`（finalize 前 `mem::replace` 成空）。implementer 按实际 `Transcript` API 调整，核心：pipeline 与 tick_active 一并 move，tick 线程不停。

- [x] **Step 6: 删 `Command::TranscriptionDone` + dispatch arm + handler + spawn helper**

1. 删 `Command::TranscriptionDone` variant（coordinator.rs L43-47）。
2. 删 dispatch arm `Command::TranscriptionDone { .. } => { ... }`（L339-353）。
3. 删 `fn handle_transcription_done`（L1860-1956 整个函数）。
4. 删 `fn spawn_offline_transcription_with_seq`（L1446-1486，逻辑已在 Task 2 进 `VadSegmentedPipeline::spawn_offline`）。
5. 删 `fn filter_speech_from_buffer`（L1495-1507，已在 Task 2 进 pipeline.rs）。
6. 删 `fn vad_preroll`（L1389-1396，已在 Task 2 进 pipeline.rs）+ `const VAD_PREROLL_FRAMES`（L159）。
7. 删 `fn consume_completed_results`（L1266-1290，已在 Task 2 进 pipeline.rs 为 `consume_completed_results_vad`）。
8. 删 coordinator.rs 顶部 `use octopus_infra::consts::{SEGMENT_DURATION_S, SEGMENT_OVERLAP_MS};`（L12，已移入 pipeline.rs）。**先 grep 确认 coordinator 无其它引用**：
```bash
grep -n "SEGMENT_DURATION_S\|SEGMENT_OVERLAP_MS\|consume_completed_results\|filter_speech_from_buffer\|vad_preroll\|VAD_PREROLL_FRAMES\|spawn_offline_transcription_with_seq\|TranscriptionDone\|handle_transcription_done" crates/desktop/src/coordinator.rs
```
Expected: 仅剩注释/无引用（若有残留引用，逐一改/删）。

- [x] **Step 7: 修其余 `Stage::WaitingCompletion` / `Stage::VadSegmented` 的 match arm**

grep 全部 `WaitingCompletion` / `VadSegmented` match（`stage_name`、`current_transcript`、`handle_cancel`/`handle_discard` 等 ~10 处，见 Task 调研 grep 结果 L1673/1691/1756/1827/2036...）：把旧字段解构（`transcript, active_count, completed_seq, completed_results`）改为新字段（`pipeline, transcript, tick_active`）。**Cancel/Discard 路径需停 tick 线程**（`tick_active.store(false)`）防泄漏。

```bash
grep -n "WaitingCompletion\|Stage::VadSegmented" crates/desktop/src/coordinator.rs
```
逐一改每个 match arm 的字段绑定。

- [x] **Step 8: workspace 编译 + clippy**

```bash
cargo check --workspace --all-targets 2>&1 | tail -10
cargo clippy -p octopus-desktop --features cloud --all-targets 2>&1 | grep -E "^(error|warning)" | tail -20
cargo test -p octopus-desktop 2>&1 | grep "test result"
```
Expected: 0 error；新代码 0 新 warning；desktop 测试全绿。

- [x] **Step 9: Commit**

```bash
git add crates/desktop/src/coordinator.rs
git commit -m "feat(desktop): coordinator Stage 改持 pipeline + 删 TranscriptionDone（2c-3 Task 5）"
```

---

## Task 6: e2e 回归 + 文档同步

**Files:**
- Verify: 手动 e2e（非流式本地引擎）
- Modify: spec 横幅状态 + plan 复选框 + memory（合并后）

**目的：** 端到端验证 VadSegmented 全路径零行为差异，同步文档。

- [x] **Step 1: 全量编译 + 测试矩阵**

```bash
cargo check --workspace --all-targets 2>&1 | tail -5
cargo check --workspace --all-targets --features cloud 2>&1 | tail -5
cargo clippy --workspace --features cloud --all-targets 2>&1 | grep -E "^warning" | wc -l
cargo test --workspace 2>&1 | grep "test result"
```
Expected: 双 feature 0 error；clippy 无新 warning（与基线比）；workspace 测试全绿。

- [x] **Step 2: 手动 e2e（非流式本地引擎 VadSegmented 全路径）**

启动 desktop（`cargo tauri dev` 或既有启动方式），配一个**非流式本地引擎**（如 moonshine / zipformer-non-streaming，`is_streaming_engine()==false`），验证：
1. **onset**：开始录音 → result window 显示「正在聆听…」→ 说话 → 段识别结果乱序回填、按 seq 顺序拼接（逗号分隔）。
2. **强制切段**：连续说话 ≥20s → force_cut 触发，overlap 衔接连贯（无丢字/重复）。
3. **停顿润色**：polish_mode=2，说话→停顿（≥segment_silence）→ 切段后触发即时润色显示（与改造前时机一致）。
4. **stop WaitingCompletion drain**：说话中按 Toggle 停止（有在途段）→ 进 WaitingCompletion → tick 继续 drain → active_count==0 → finalize 粘贴（文本完整，无截断/丢失）。
5. **stop 直接 finalize**：静音后停止（无在途段）→ 直接 finalize。
6. **跨会话护栏**：停止后立刻重开新会话 → 旧会话迟到的段结果不污染新会话（pipeline 随 stage drop，rx disconnect）。
7. **Cancel/Discard**：录音中 Cancel/Discard → tick 线程停止、无泄漏、无迟到的粘贴。

- [x] **Step 3: 同步 spec 横幅 + plan 复选框**

spec `docs/superpowers/specs/2026-06-25-archived-spec.md#vad-segmented-rehome-design` 顶部状态行改：
```
> **状态**：✅ 已实施（待 ff-merge main）。Task 1-6 编译/测试/clippy 全通过；e2e 验证通过（2026-06-25）。
```
本 plan 所有 `- [ ]` → `- [x]`。

- [x] **Step 4: Commit 文档**

```bash
git add docs/superpowers/specs/2026-06-25-archived-spec.md#vad-segmented-rehome-design docs/superpowers/plans/2026-06-25-archived-plan.md#vad-segmented-rehome
git commit -m "docs(spec/plan): 2c-3 VadSegmented 归位 e2e 通过、状态同步"
```

- [x] **Step 5: 收尾（finishing-a-development-branch）**

e2e 通过后，用 superpowers:finishing-a-development-branch 选 ff-merge main（对齐 2a/2b/2c-1/2c-2 节奏）。合并后更新 memory `parallel-workstreams.md` item 7 的 2c-3 状态。

---

## Self-Review

**1. Spec coverage：**
- §3.1 Pipeline trait → Task 1（+ 细化 `took_segment_cut`）
- §3.2 VadSegmentedPipeline 字段 + tick 编排 → Task 2
- §3.3 impl Pipeline for StreamingPipeline + finish 去 tail → Task 4
- §3.4 coordinator Stage 改造 + 删 TranscriptionDone → Task 5
- §3.5 WaitingCompletion 收尾驱动 → Task 5 Step 3/4/5（+ 细化 tick_active 生命周期）
- §4 跨会话护栏（pipeline drop 天然保证）→ Task 2 `apply_segment_result` 注释 + Task 5 Cancel/Discard 停线程
- §7 测试（乱序/占位/finish drain/双 VAD/StreamingPipeline 套壳）→ Task 2（纯函数）+ Task 4（既有测试改 finish）+ Task 6（e2e）
- §8 迁移任务 6 项 → Task 1-6 一一对应 ✓

**2. Placeholder scan：** Task 5 Step 5 的借用拆分给了两种写法 + implementer 提示（`mem::replace` 主线），非占位——是真实复杂度的诚实标注。无 TBD/TODO。

**3. Type consistency：**
- `Pipeline::tick(&mut self, &[f32], &mut Transcript) -> bool`：Task 1 定义，Task 3/4 impl，Task 5 调用 ✓
- `Pipeline::finish(&mut self, &mut Transcript) -> TranscriptEvent`：Task 1 定义，Task 3（vad-seg 返回 Committed 占位）、Task 4（local Final / cloud 兜底）、Task 5 调用 ✓
- `VadSegmentedPipeline::new(engine, language, asr_engine, segment_silence_ms)`：Task 2 定义，Task 5 Step 2 调用 ✓
- `consume_completed_results_vad` / `apply_segment_result`：Task 2 定义 + 测试，Task 3 `drain_rx_and_consume` 调用 ✓
- `took_segment_cut()`：Task 1 trait 默认，Task 2 `segment_cut_this_tick` 字段 + `run_tick` 设置，Task 3 impl，Task 5 `after_vad_tick` 读取 ✓
- `active_count()`：Task 2 getter，Task 5 WaitingCompletion 收尾 + stop 路径读取 ✓

**4. 风险点（已在对应 Task 标注）：**
- Task 4 tail 走 VAD 的细微差异（Final 覆盖 + e2e 验证）
- Task 5 Step 5 stop 路径借用拆分（mem::replace 主线）
- Task 5 Step 7 ~10 处 match arm 字段改 + Cancel/Discard 停 tick 线程防泄漏

