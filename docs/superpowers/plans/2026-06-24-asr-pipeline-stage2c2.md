# ASR Pipeline 阶段 2c-2：云端流式接入 StreamingPipeline 实施计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 把云端流式的「同步 tick 部分」收敛进 `StreamingPipeline`（上层 `StreamingPipelineEngine` trait，`CloudPipelineEngine` impl），cloud 的 async close 中间态（`Stage::CloudClosing` + `session_id` 护栏）原样留在 coordinator——与本地流式在 tick 层对称，零行为差异。

**Architecture:** 新增上层 trait `StreamingPipelineEngine`（`tick/finish_with_tail/silence_duration/current_partial/reset/take_close_handle/is_cloud`，后三个带默认实现）。`StreamingPipeline` 从「持 `StreamingRunner`」改为「持 `Box<dyn StreamingPipelineEngine>`」。`LocalPipelineEngine` 薄包 `StreamingRunner`（2c-1 既有行为）；`CloudPipelineEngine`（cfg cloud，新文件 `cloud_pipeline.rs`）把 `handle_cloud_streaming_tick` 的 ASR 编排（onset/push/drain/双层文本/静音非阻塞 finish）迁入 `tick`，产 `Vec<TranscriptEvent>` 而非直接写 transcript/emit。coordinator 侧 `Stage::CloudStreaming` 合并进 `Stage::Streaming`，`handle_cloud_streaming_tick` 删除合并进 `handle_streaming_tick`（统一 `pipeline.tick` + DB/emit/polish，`is_cloud()` 分支处理 cloud 的「每 tick emit / commit 时 DB+polish / 错误上报」三处不对称）。cloud close 链（`CloudClosing`/`CloudStreamingDone`/`handle_cloud_streaming_done`/`finalize_cloud`/`session_id` 护栏/`close_async` spawn）完全不动。

**Tech Stack:** Rust workspace（crate `desktop`，binary `main.rs`）；`cloud` feature（`#[cfg(feature = "cloud")]`，Cargo.toml 已定义）；tauri async runtime；`octopus_asr::vad::SileroVad` / `streaming_runner::{StreamingEngine, StreamingRunner, TranscriptEvent}`。

**关联文档：** spec `docs/superpowers/specs/2026-06-24-asr-pipeline-stage2c2-design.md`；总 spec `docs/superpowers/specs/2026-06-23-asr-pipeline-design.md` §3.4。

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

- [ ] **Step 1.1: 写失败测试——`LocalPipelineEngine` 包 `StreamingRunner` 并 impl trait**

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

- [ ] **Step 1.2: 运行测试确认失败**

Run: `cargo test -p octopus-desktop pipeline:: -- --nocapture 2>&1 | tail -20`
Expected: 编译失败——`StreamingPipelineEngine` / `StreamingPipeline::new(Box<...>)` 单参数 / `take_error` / `is_cloud` 未定义。

- [ ] **Step 1.3: 重写 `pipeline.rs`（trait + LocalPipelineEngine + StreamingPipeline + compute_speech_chunks）**

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
use octopus_asr::streaming_runner::{StreamingRunner, TranscriptEvent};
use octopus_asr::streaming_engine::StreamingSession;
use octopus_asr::vad::SileroVad;

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

- [ ] **Step 1.4: 修改 coordinator.rs——删除 `compute_speech_chunks` 定义、改 import、改 local 构造点、改 vad-segmented 调用点**

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

⑤ import 清理：`coordinator.rs:10` `use octopus_asr::streaming_engine::StreamingSession;` —— local 构造不再直接用 `StreamingSession`（移入 pipeline.rs），但 handle_toggle local 分支 671 `StreamingSession::new(&config.asr_engine)` **仍在 coordinator**（降级逻辑用）。故该 import **保留**。

- [ ] **Step 1.5: 运行测试确认通过（不含 cloud feature）**

Run: `cargo test -p octopus-desktop pipeline:: 2>&1 | tail -25`
Expected: PASS——`pipeline::tests` 全绿（含新增 5 个 + 改造的既有）。

- [ ] **Step 1.6: 全量 check（双 feature 配置）**

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

- [ ] **Step 1.7: 提交**

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

- [ ] **Step 2.1: 在 `main.rs` 加模块声明**

`crates/desktop/src/main.rs:18`（`mod cloud_types;` 之后）插入：
```rust
#[cfg(feature = "cloud")]
mod cloud_pipeline;
```

- [ ] **Step 2.2: 写失败测试——`drain_cloud_session` 事件映射**

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

- [ ] **Step 2.3: 运行测试确认失败**

Run: `cargo test -p octopus-desktop --features cloud cloud_pipeline:: 2>&1 | tail -20`
Expected: 编译失败——`drain_cloud_session`/`CloudDrainState`/`onset_confirmed`/`should_send_finish`/`take_preroll` 未定义。

- [ ] **Step 2.4: 实现 `cloud_pipeline.rs` 主体（纯函数 + struct + open/resolve helpers + CloudPipelineEngine + impl trait）**

在 `cloud_pipeline.rs` 的 `#[cfg(test)] mod tests` **之上**插入完整实现。文件结构：常量 → `CloudDrainState` + `drain_cloud_session` → `onset_confirmed`/`should_send_finish`/`take_preroll` → `open_cloud_session` + 5 个 `resolve_*` + `resolve_cloud_entry`（从 coordinator 迁入，签名改 `(asr_engine, language, pre_roll)`）→ `CloudPipelineEngine` struct + `new` + impl trait。

```rust
use crate::cloud_types::{CloudStreamHandle, StreamEvent};
use crate::pipeline::{compute_speech_chunks, StreamingPipelineEngine};
use log::{debug, error, info, warn};
use octopus_asr::streaming_runner::TranscriptEvent;
use octopus_asr::vad::SileroVad;
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
    let cfg = octopus_asr::config::load_config().map_err(|e| e.to_string())?;
    let model_name = octopus_infra::db::parse_model_spec(engine_spec).model_name().to_string();
    let entry = resolve_cloud_entry(cfg.asr.aliyun.as_ref(), "aliyun", &model_name)?;
    Ok((entry.source.clone(), entry.secret_key.clone(), model_name))
}

#[cfg(feature = "cloud")]
fn resolve_bytedance_config(engine_spec: &str) -> Result<(String, String, String), String> {
    let cfg = octopus_asr::config::load_config().map_err(|e| e.to_string())?;
    let model_name = octopus_infra::db::parse_model_spec(engine_spec).model_name().to_string();
    let entry = resolve_cloud_entry(cfg.asr.bytedance.as_ref(), "bytedance", &model_name)?;
    Ok((entry.source.clone(), entry.secret_key.clone(), model_name))
}

#[cfg(feature = "cloud")]
fn resolve_tencent_config(engine_spec: &str) -> Result<(String, String, String), String> {
    let cfg = octopus_asr::config::load_config().map_err(|e| e.to_string())?;
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
    let cfg = octopus_asr::config::load_config().map_err(|e| e.to_string())?;
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
    use octopus_asr::config::EngineCategory;
    let rt: RuntimeHandle = tauri::async_runtime::handle();
    match octopus_asr::config::resolve_engine_category(asr_engine) {
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

- [ ] **Step 2.5: 运行测试确认通过**

Run: `cargo test -p octopus-desktop --features cloud cloud_pipeline:: 2>&1 | tail -25`
Expected: PASS——`cloud_pipeline::tests` 全绿（drain_cloud_session 4 例 + onset_confirmed + should_send_finish + take_preroll）。

- [ ] **Step 2.6: check（允许 cloud_pipeline 暂时未引用的 dead_code warning）**

Run: `cargo check -p octopus-desktop --features cloud 2>&1 | tail -20`
Expected: 0 error。可能有 `CloudPipelineEngine`/`open_cloud_session` 等未引用的 `dead_code` warning——**本 task 可接受**（Task 3 接线后消除）。若 clippy/error 级别报错则需修复。

- [ ] **Step 2.7: 提交**

```bash
git add crates/desktop/src/cloud_pipeline.rs crates/desktop/src/main.rs
git commit -m "feat(asr): CloudPipelineEngine + cloud tick 迁入 cloud_pipeline.rs（2c-2 T2，未接线）"
```

---

## Task 3: 接线——合并 `Stage::CloudStreaming` 进 `Stage::Streaming`，删 `handle_cloud_streaming_tick`

**目标：** 把 cloud 接入 `StreamingPipeline`：`handle_toggle` cloud 分支建 `CloudPipelineEngine`→`Stage::Streaming`；`CloudStreamingTick` dispatch 改调 `handle_streaming_tick`（cloud 仍走 100ms tick 线程）；stop 路径合并（`take_close_handle` 分派 cloud close / local finalize）；删除 `Stage::CloudStreaming` + `handle_cloud_streaming_tick` + 迁出的 helpers（`open_cloud_session`/`resolve_*`/`take_preroll`，已在 Task 2 迁入 cloud_pipeline.rs）；清理 7 处 `Stage::CloudStreaming` match 臂（由 `Stage::Streaming` 覆盖）。`Stage::CloudClosing`/`CloudStreamingDone`/`handle_cloud_streaming_done`/`finalize_cloud`/`session_id` 护栏/`start_cloud_streaming_tick_thread`/`CLOUD_STREAMING_TICK_INTERVAL_MS`/`is_cloud_engine`/`vad_preroll` **保留**。

**Files:**
- Modify: `crates/desktop/src/coordinator.rs`（多处：Stage enum、Command dispatch、handle_toggle、stop、handle_streaming_tick、7 处 match 臂、删除 cloud tick + 迁出 helpers）

- [ ] **Step 3.1: 删除 `Stage::CloudStreaming` 变体**

`coordinator.rs:110-136`（`Stage::CloudStreaming { ... }` 整段含 doc 注释 110-114）。删除整个变体。`Stage::CloudClosing`（137-144）保留。

- [ ] **Step 3.2: `handle_streaming_tick` 重写为 local/cloud 统一（`is_cloud()` 分支）**

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

- [ ] **Step 3.3: `handle_toggle` cloud 分支改为建 `CloudPipelineEngine` → `Stage::Streaming`**

替换 `coordinator.rs:627-665`（cloud 分支整段，`#[cfg(feature = "cloud")] if use_cloud_streaming { ... return; }`）为：

```rust
            #[cfg(feature = "cloud")]
            if use_cloud_streaming {
                match octopus_asr::config::find_silero_vad() {
                    Ok(path) => match octopus_asr::vad::SileroVad::new(&path) {
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

- [ ] **Step 3.4: stop 路径合并——`Stage::Streaming` 统一 arm（cloud `take_close_handle` 分派）**

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

- [ ] **Step 3.5: `CloudStreamingTick` dispatch 改调 `handle_streaming_tick`**

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

- [ ] **Step 3.6: 删除 `handle_cloud_streaming_tick` 整函数**

删除 `coordinator.rs:1630-1812`（`/// 处理 CloudStreamingTick 命令...` 注释 + `fn handle_cloud_streaming_tick { ... }` 整体）。逻辑已迁入 `CloudPipelineEngine::tick`（Task 2）+ `handle_streaming_tick`（Step 3.2）。

- [ ] **Step 3.7: 删除已迁出的 cloud helpers（`open_cloud_session` + `resolve_*` + `take_preroll`）**

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

- [ ] **Step 3.8: 清理 7 处 `Stage::CloudStreaming` match 臂（由 `Stage::Streaming` 覆盖）**

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

- [ ] **Step 3.9: check（双 feature 配置）**

Run: `cargo check -p octopus-desktop 2>&1 | tail -15`
Expected: 0 error。

Run: `cargo check -p octopus-desktop --features cloud 2>&1 | tail -20`
Expected: 0 error，0 warning（Task 2 的 dead_code 应已消除——`CloudPipelineEngine`/`open_cloud_session` 现被 handle_toggle 引用）。若有残留 `dead_code`（如 `resolve_*` 仅被 `open_cloud_session` 用，应已被引用），核实是否漏删 coordinator 重复定义导致未引用——删 coordinator 重复定义即可。

- [ ] **Step 3.10: 跑测试（双 feature）**

Run: `cargo test -p octopus-desktop 2>&1 | tail -20`
Run: `cargo test -p octopus-desktop --features cloud 2>&1 | tail -20`
Expected: 全绿（pipeline + cloud_pipeline + 既有 coordinator/transcript 测试）。

- [ ] **Step 3.11: 提交**

```bash
git add crates/desktop/src/coordinator.rs
git commit -m "refactor(asr): cloud 流式合并进 Stage::Streaming（2c-2 T3，删 handle_cloud_streaming_tick）"
```

---

## Task 4: 验证（双 feature test + clippy）+ 文档同步

**目标：** 全量验证零行为差异（编译/测试/clippy 双 feature），同步 spec 横幅 + architecture.md。e2e（真实 DashScope key）由用户本地执行，不在本 task 自动化范围。

**Files:**
- Modify: `docs/superpowers/specs/2026-06-24-asr-pipeline-stage2c2-design.md`（横幅状态）
- Modify: `docs/superpowers/specs/2026-06-23-asr-pipeline-design.md`（§3.4 阶段进度行）
- Modify: `docs/architecture.md`（新增 `cloud_pipeline.rs` 模块 + Stage 状态机描述）

- [ ] **Step 4.1: workspace 全量 check + test（双 feature）**

Run: `cargo check --workspace --all-targets 2>&1 | tail -15`
Expected: 0 error。

Run: `cargo test --workspace 2>&1 | tail -25`
Expected: 全绿。

Run: `cargo check --workspace --all-targets --features desktop/cloud 2>&1 | tail -15`
Expected: 0 error。（确认 cloud feature 全 workspace 编译。）

- [ ] **Step 4.2: clippy（双 feature，零新 warning）**

Run: `cargo clippy -p octopus-desktop --features cloud -- -D warnings 2>&1 | tail -25`
Expected: 0 warning（`-D warnings` 视 0 为通过）。若有，按提示修复（常见：未用 import、`#[allow]` 缺失）。

- [ ] **Step 4.3: 零行为差异自检（逐条核对 spec §7）**

人工核对（不改代码，只读 + grep 验证）：

1. **tick 逻辑原样搬迁**：`grep -n "speech_confirm_count\|pre_roll_buffer\|push_pcm\|try_recv_text\|silence_duration" crates/desktop/src/cloud_pipeline.rs`——确认 onset 连续确认 / pre_roll 滚动 / push / drain / 双层 / 静音 finish / session take 全在。
2. **close 路径不动**：`grep -n "CloudClosing\|CloudStreamingDone\|finalize_cloud\|session_id" crates/desktop/src/coordinator.rs`——确认 `Stage::CloudClosing`/`Command::CloudStreamingDone`/`handle_cloud_streaming_done`/`finalize_cloud`/session_id 护栏原样保留。
3. **DB 时机不变**：cloud 仅 `Finished/Committed`（`changed`）时 DB（`handle_streaming_tick` cloud 分支 `if changed { update_transcription_raw }`）；local `changed` 时 DB。
4. **emit 频率不变**：cloud 每 tick emit（`handle_streaming_tick` cloud 分支末尾无 `if changed` 包裹）；local 仅 `changed` emit。
5. **预览不进 DB**：`drain_cloud_session` 的 `Text → current_partial`（无 event），仅 `Finished → Committed` 发事件 → 承载层 set_full → coordinator DB。
6. **逗号拼接一致**：`drain_cloud_session` 的 `if !committed_text.is_empty() && !committed_text.ends_with('，') { push '，' }` 与原 `coordinator.rs:1747-1752` 一致。

- [ ] **Step 4.4: 同步 spec 横幅**

`docs/superpowers/specs/2026-06-24-asr-pipeline-stage2c2-design.md` 第 4 行（`> **状态**：...`）改为：

```
> **状态**：设计已定 + 实施计划就绪（2026-06-24）。实现见 plan `docs/superpowers/plans/2026-06-24-asr-pipeline-stage2c2.md`。
```

`docs/superpowers/specs/2026-06-23-asr-pipeline-design.md` §3.4 阶段 2c-2 进度行（"设计已定 2026-06-24，待 plan"）改为 "计划就绪 2026-06-24，待实现 + e2e"。

- [ ] **Step 4.5: 同步 architecture.md**

在 `docs/architecture.md` 的 desktop 模块清单 + 状态机描述处：
- 新增模块 `crates/desktop/src/cloud_pipeline.rs`（cfg cloud）：`CloudPipelineEngine` impl `StreamingPipelineEngine`，承载云端流式 ASR 编排。
- `pipeline.rs` 描述更新：持 `Box<dyn StreamingPipelineEngine>`（`LocalPipelineEngine` / `CloudPipelineEngine`），`compute_speech_chunks` 共享 VAD helper。
- 状态机：`Stage::CloudStreaming` 已合并进 `Stage::Streaming`（cloud 走 100ms `CloudStreamingTick`，local 走 200ms `StreamingTick`，统一 `handle_streaming_tick`）；`Stage::CloudClosing` 保留（cloud async close 中间态）。

- [ ] **Step 4.6: 提交文档同步**

```bash
git add docs/superpowers/specs/2026-06-24-asr-pipeline-stage2c2-design.md docs/superpowers/specs/2026-06-23-asr-pipeline-design.md docs/architecture.md
git commit -m "docs(asr): 同步 2c-2 计划就绪状态 + cloud_pipeline 模块（spec/architecture）"
```

- [ ] **Step 4.7: e2e 清单（交用户本地执行，需 DashScope/云端 key）**

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
