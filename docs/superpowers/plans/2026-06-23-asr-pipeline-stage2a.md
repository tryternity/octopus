# ASR Pipeline 阶段2a：asr 流式基础设施（StreamingRunner）实施计划

> ✅ **已实施（2026-06-23，commit `10f612c`）**：Task 1-4 全完成。`crates/asr/src/streaming_runner.rs` 新增（371 行），`cargo test -p octopus-asr-local` 81 tests（75 pass + 6 ignored 模型相关，0 fail，含新增 7 个），`cargo check --workspace --all-targets` 干净，clippy 无新 warning。纯新增不碰 desktop，运行时零行为变化。

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development 或 superpowers:executing-plans 按任务实施。Steps 用 checkbox（`- [ ]`）跟踪。

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
docs/superpowers/plans/2026-06-23-asr-pipeline-stage2a.md。纯新增，不碰 desktop。"
```

> 分类器提示：`git add` 与 `git commit` 分两行（换行分隔，**不要** `&&`）。

---

## Self-Review（writing-plans 自检）

**1. Spec 覆盖：** spec §3.2 `StreamingEngine`/`AudioSource`/`TranscriptEvent` → Task 1（`StreamingEngine`+`TranscriptEvent`）、`AudioSource` 显式延后（设计调整 §2，2b）；§3.3 `StreamingRunner` → Task 2；§3.3 流式纠错 hook → Task 2 `correct`/`maybe_correct`（默认关）；§9.1 错误事件 → Task 2 `push_samples`/`finish` 的 Error 分支 + Task 3 测试。spec §3.6 denoise 迁移 → 设计调整 §1 明确不迁（用户决策）。✅

**2. 占位符扫描：** Task 3 Step 3 的 `accept_error_becomes_error_event_nonfatal` 初版用「队列耗尽 panic」是不正确的占位——Step 4 已给出 `always_err` fake + Err 用例完整代码替换。最终提交时 `FakeStreamingEngine` 的 `impl` 只写一次（含空队列→Err 分支），无 TBD/TODO。✅

**3. 类型一致性：** `StreamingEngine::{accept_samples,flush,finish,reset}` 与 `StreamingSession` 同名方法签名一致（已核实 streaming_engine.rs:78/154/209/256）；`StreamingRunner::push_samples` 调用 `detect_silence_gap(&mut Option<SileroVad>, &[f32], &mut f64, &mut bool) -> (bool,bool)` 与 Step 2 定义一致；`TranscriptEvent` 变体在 push_samples（Partial/Committed/Error）、finish（Final/Error）、maybe_correct 中使用一致。✅

**4. 行为不变性：** 2a 无调用方（desktop 2b 才接入），运行时行为零变化；`step_silence`/`detect_silence_gap` 逐字搬迁 coordinator 语义（speech≥2 重置、`total_chunks*chunk_duration` 累加、`prev≥阈值` 标点、`silence==0→flushed=false`、`≥阈值&&!flushed→flush+上锁`），单测锁死边界。✅
