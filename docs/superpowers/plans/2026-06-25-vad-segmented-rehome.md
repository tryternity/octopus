# 2c-3 VadSegmented 归位 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 把散在 `coordinator.rs` 的 VadSegmented（非流式引擎 VAD 分段伪流式）编排 + 乱序回填收进统一 `Pipeline` 角色（`VadSegmentedPipeline`），删除 `Command::TranscriptionDone`，coordinator 持 `Box<dyn Pipeline>` 不再按 stage 分流 tick 逻辑。

**Architecture:** 新增上层 `Pipeline` trait（`tick/finish/silence_duration/reset/take_close_handle/is_cloud/took_segment_cut`）。`VadSegmentedPipeline` 内部持 mpsc channel：切段后 `tauri::async_runtime::spawn` 跑 `engine.transcribe`，结果发回 pipeline 自持 `rx`（**不发 coordinator.tx**），下一个 tick `try_recv` drain + 乱序回填 + 消费连续 seq + set_full——异步命令回传转成同步 tick 输出。`StreamingPipeline` 外层加 `impl Pipeline`（内层 `StreamingPipelineEngine` 两层不动）。coordinator `Stage::VadSegmented`/`WaitingCompletion` 字段改持 pipeline，删 `TranscriptionDone` 命令与两处回填 handler。emit/DB/polish/transcript 仍留 coordinator（2d 收敛）。每 Task 零行为差异 + 双 feature 编译 + clippy 零新 warning。

**Tech Stack:** Rust，tauri 2（`tauri::async_runtime::spawn`），`std::sync::mpsc`，`octopus_asr`（`SileroVad`/`TranscriptEvent`/`streaming_runner`），`octopus_infra::consts`（`SEGMENT_DURATION_S`/`SEGMENT_OVERLAP_MS`）。

**Spec:** `docs/superpowers/specs/2026-06-25-vad-segmented-rehome-design.md`

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

依赖边界不变：`octopus-desktop ──→ octopus-asr + octopus-infra`，无 cloud 依赖（VadSegmented 仅非流式本地引擎，`is_cloud()` 恒 false）。

---

## Task 1: `Pipeline` trait + `SegmentResult` 类型

**Files:**
- Modify: `crates/desktop/src/pipeline.rs`（顶部 `use` 之后、`StreamingPipelineEngine` trait 之前插入）

**目的：** 定义统一上层抽象与 VadSegmented 内部回传类型。此 Task 仅定义、不 impl，trait 暂未使用会有 `dead_code`/`unused` 警告——Task 2/3/4 impl 后消失；本 Task 用 `#[allow(unused)]` 临时压住。

- [ ] **Step 1: 加 `SegmentResult` 与 `Pipeline` trait**

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

- [ ] **Step 2: 验证编译**

```bash
cargo check -p octopus-desktop 2>&1 | tail -20
```
Expected: 0 error（可能有 `unused`/`dead_code` 警告——已用 `#[allow(unused)]` 压 trait；`SegmentResult` 未构造会有 dead_code 警告，Task 2 消除）。

- [ ] **Step 3: Commit**

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

- [ ] **Step 1: 写失败的单测（纯函数：回填 + 乱序消费）**

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

- [ ] **Step 2: 运行测试，确认失败**

```bash
cargo test -p octopus-desktop pipeline::tests::apply_segment_result_normal_inserts_text 2>&1 | tail -15
```
Expected: 编译失败——`apply_segment_result`/`consume_completed_results_vad`/`SegmentResult` 字段未定义。

- [ ] **Step 3: 实现 `SegmentResult` 字段可见性 + 2 个纯函数**

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

- [ ] **Step 4: 运行测试，确认通过**

```bash
cargo test -p octopus-desktop pipeline::tests 2>&1 | grep "test result"
```
Expected: `test result: ok. N passed`（含 4 个新测试 + 既有 StreamingPipeline 测试）。

- [ ] **Step 5: 加 `VadSegmentedPipeline` 结构 + 构造 + 搬入 helper**

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
    let speech = octopus_asr::audio::filter_speech(samples, filter_vad, 480, 0.5);
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
        let path = octopus_asr::config::find_silero_vad()?;
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

- [ ] **Step 6: 验证编译 + 测试**

```bash
cargo check -p octopus-desktop 2>&1 | tail -20
cargo test -p octopus-desktop pipeline::tests 2>&1 | grep "test result"
```
Expected: 0 error；测试全绿（`SegmentResult`/`consume_completed_results_vad`/`apply_segment_result` 的 dead_code 警告消失——已被结构与测试引用）。`run_tick`/`new`/`spawn_offline`/`drain_rx_and_consume`/`active_count` 未被外部调用会有 dead_code 警告，Task 3/5 消除；如需临时压住可加 `#[allow(dead_code)]` 到结构，但建议留作 Task 3 自然消除。

- [ ] **Step 7: Commit**

```bash
git add crates/desktop/src/pipeline.rs
git commit -m "feat(desktop): VadSegmentedPipeline 结构 + tick 编排 + 回填纯逻辑（2c-3 Task 2）"
```

---

## Task 3: `impl Pipeline for VadSegmentedPipeline`

**Files:**
- Modify: `crates/desktop/src/pipeline.rs`

**目的：** 给 `VadSegmentedPipeline` 套上 `Pipeline` trait（tick 转 `run_tick`；finish = drain rx 至空 + consume；silence/reset/take_close_handle/is_cloud/took_segment_cut）。

- [ ] **Step 1: 写 impl**

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

- [ ] **Step 2: 验证编译 + clippy**

```bash
cargo check -p octopus-desktop 2>&1 | tail -20
cargo clippy -p octopus-desktop --all-targets 2>&1 | grep -E "^(error|warning)" | grep -v "cloud_pipeline\|coordinator.rs" | tail -20
```
Expected: 0 error；本 Task 新代码 0 新 warning（coordinator/cloud_pipeline 的预存 warning 非本 Task 引入）。

- [ ] **Step 3: Commit**

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

- [ ] **Step 1: 改 `StreamingPipelineEngine` trait 签名（pipeline.rs L34）**

`finish_with_tail(&mut self, tail: &[f32]) -> TranscriptEvent` → `finish(&mut self) -> TranscriptEvent`。更新该方法的文档注释为「收尾 flush（tail 已由 stop 路径 tick 喂入 accept）：local → `StreamingRunner::finish`（Final）；cloud → 返回最后 `current_partial` 作 Committed 兜底」。

- [ ] **Step 2: 改 `LocalPipelineEngine` impl（pipeline.rs L67-69）**

```rust
    fn finish(&mut self) -> TranscriptEvent {
        self.0.finish()
    }
```

- [ ] **Step 3: 改 `CloudPipelineEngine` impl（cloud_pipeline.rs L270 区域）**

现状 `finish_with_tail` 内部 `push_pcm(tail)` + 返回 `current_partial` 兜底。去 tail 后只返回兜底（tail 由 stop 路径 `tick` 内的 `push_pcm` 喂入）：

```rust
    fn finish(&mut self) -> TranscriptEvent {
        // tail 已由 stop 路径 tick 喂入 push_pcm；此处仅返回最后 current_partial 作 Committed 兜底。
        // cloud stop 路径不用其返回值（走 finalize_cloud / CloudClosing）。
        TranscriptEvent::Committed(self.current_partial().to_string())
    }
```

（删除原 `finish_with_tail` 内的 `push_pcm` 逻辑——已移到 tick 路径。）

- [ ] **Step 4: 改 `StreamingPipeline` 包装方法（pipeline.rs L126-128）+ 加 `impl Pipeline`**

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

- [ ] **Step 5: 改 coordinator stop 路径（coordinator.rs L840-895）**

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

- [ ] **Step 6: 改既有流式测试（pipeline.rs L296 `finish_with_tail_delegates_to_engine`）**

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

- [ ] **Step 7: 双 feature 编译 + 既有测试 + clippy**

```bash
cargo check -p octopus-desktop 2>&1 | tail -5
cargo check -p octopus-desktop --features cloud 2>&1 | tail -5
cargo test -p octopus-desktop pipeline::tests 2>&1 | grep "test result"
cargo test -p octopus-desktop --features cloud cloud_pipeline 2>&1 | grep "test result"
cargo clippy -p octopus-desktop --features cloud --all-targets 2>&1 | grep -E "^warning" | grep -E "pipeline.rs|cloud_pipeline.rs" | tail
```
Expected: 全 0 error；pipeline/cloud_pipeline 测试绿；新代码 0 新 warning。

- [ ] **Step 8: Commit**

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

- [ ] **Step 1: 改 `Stage` 枚举字段**

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

- [ ] **Step 2: 改 start 路径构造（coordinator.rs L716-755）**

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

- [ ] **Step 3: 改 `VadSegmentedTick` dispatch（coordinator.rs L301-321）**

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

- [ ] **Step 4: 重写 `handle_vad_segmented_tick`（coordinator.rs L1292-1387）**

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

- [ ] **Step 5: 改 stop 路径 `Stage::VadSegmented` 分支（coordinator.rs L770-828）**

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

- [ ] **Step 6: 删 `Command::TranscriptionDone` + dispatch arm + handler + spawn helper**

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

- [ ] **Step 7: 修其余 `Stage::WaitingCompletion` / `Stage::VadSegmented` 的 match arm**

grep 全部 `WaitingCompletion` / `VadSegmented` match（`stage_name`、`current_transcript`、`handle_cancel`/`handle_discard` 等 ~10 处，见 Task 调研 grep 结果 L1673/1691/1756/1827/2036...）：把旧字段解构（`transcript, active_count, completed_seq, completed_results`）改为新字段（`pipeline, transcript, tick_active`）。**Cancel/Discard 路径需停 tick 线程**（`tick_active.store(false)`）防泄漏。

```bash
grep -n "WaitingCompletion\|Stage::VadSegmented" crates/desktop/src/coordinator.rs
```
逐一改每个 match arm 的字段绑定。

- [ ] **Step 8: workspace 编译 + clippy**

```bash
cargo check --workspace --all-targets 2>&1 | tail -10
cargo clippy -p octopus-desktop --features cloud --all-targets 2>&1 | grep -E "^(error|warning)" | tail -20
cargo test -p octopus-desktop 2>&1 | grep "test result"
```
Expected: 0 error；新代码 0 新 warning；desktop 测试全绿。

- [ ] **Step 9: Commit**

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

- [ ] **Step 1: 全量编译 + 测试矩阵**

```bash
cargo check --workspace --all-targets 2>&1 | tail -5
cargo check --workspace --all-targets --features cloud 2>&1 | tail -5
cargo clippy --workspace --features cloud --all-targets 2>&1 | grep -E "^warning" | wc -l
cargo test --workspace 2>&1 | grep "test result"
```
Expected: 双 feature 0 error；clippy 无新 warning（与基线比）；workspace 测试全绿。

- [ ] **Step 2: 手动 e2e（非流式本地引擎 VadSegmented 全路径）**

启动 desktop（`cargo tauri dev` 或既有启动方式），配一个**非流式本地引擎**（如 moonshine / zipformer-non-streaming，`is_streaming_engine()==false`），验证：
1. **onset**：开始录音 → result window 显示「正在聆听…」→ 说话 → 段识别结果乱序回填、按 seq 顺序拼接（逗号分隔）。
2. **强制切段**：连续说话 ≥20s → force_cut 触发，overlap 衔接连贯（无丢字/重复）。
3. **停顿润色**：polish_mode=2，说话→停顿（≥segment_silence）→ 切段后触发即时润色显示（与改造前时机一致）。
4. **stop WaitingCompletion drain**：说话中按 Toggle 停止（有在途段）→ 进 WaitingCompletion → tick 继续 drain → active_count==0 → finalize 粘贴（文本完整，无截断/丢失）。
5. **stop 直接 finalize**：静音后停止（无在途段）→ 直接 finalize。
6. **跨会话护栏**：停止后立刻重开新会话 → 旧会话迟到的段结果不污染新会话（pipeline 随 stage drop，rx disconnect）。
7. **Cancel/Discard**：录音中 Cancel/Discard → tick 线程停止、无泄漏、无迟到的粘贴。

- [ ] **Step 3: 同步 spec 横幅 + plan 复选框**

spec `docs/superpowers/specs/2026-06-25-vad-segmented-rehome-design.md` 顶部状态行改：
```
> **状态**：✅ 已实施（待 ff-merge main）。Task 1-6 编译/测试/clippy 全通过；e2e 验证通过（2026-06-25）。
```
本 plan 所有 `- [ ]` → `- [x]`。

- [ ] **Step 4: Commit 文档**

```bash
git add docs/superpowers/specs/2026-06-25-vad-segmented-rehome-design.md docs/superpowers/plans/2026-06-25-vad-segmented-rehome.md
git commit -m "docs(spec/plan): 2c-3 VadSegmented 归位 e2e 通过、状态同步"
```

- [ ] **Step 5: 收尾（finishing-a-development-branch）**

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
