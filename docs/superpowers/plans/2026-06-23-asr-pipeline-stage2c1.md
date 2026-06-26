# ASR Pipeline 阶段2c-1：StreamingPipeline 壳 + local 迁入 pipeline.rs 实施计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development 或 superpowers:executing-plans 按任务实施。Steps 用 checkbox（`- [ ]`）跟踪。

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

spec banner（`docs/superpowers/specs/2026-06-23-asr-pipeline-design.md`）2c 行更新：

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
