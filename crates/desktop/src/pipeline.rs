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
//! （`check_and_trigger_polish`）留 coordinator（emit 与 DB 同步触发以保持 `set_full→DB→emit`
//! 顺序；DB/polish 被 local + VadSegmented + cloud 三路径共用）。transcript 也留
//! `Stage::Streaming`，`tick` 接收 `&mut Transcript`。全收敛留 2d。

use crate::transcript::Transcript;
use log::warn;
use octopus_asr::streaming_runner::{StreamingRunner, TranscriptEvent};
use octopus_asr::streaming_engine::StreamingSession;
use octopus_asr::vad::SileroVad;
use std::collections::HashMap;
use std::sync::Arc;
use octopus_infra::consts::{SEGMENT_DURATION_S, SEGMENT_OVERLAP_MS};

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

/// VadSegmented 段识别结果（pipeline 内部回传类型，2c-3）。
///
/// spawn 线程跑完 `engine.transcribe` 后，把结果发回 `VadSegmentedPipeline.rx`（**不发
/// coordinator.tx**），下个 tick `try_recv` drain。跨会话护栏由「stage 切换 = 新 pipeline
/// 实例」天然保证（旧 pipeline drop → rx disconnect → spawn 的 `tx.send` 失败忽略），
/// 无需 session_id（spec §4）。
pub(crate) struct SegmentResult {
    pub seq: u64,
    pub text: Result<String, String>,
}

/// 把一条段结果回填进缓存 + 递减 active_count（纯逻辑，2c-3）。
///
/// 空串/失败占位空串（保 `completed_seq` 连续推进，避免后续有效段积压丢失）。
/// 不判 session_id（跨会话护栏由 pipeline 随 stage drop 天然保证，spec §4）。
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
            // 避免拼接出 「。，」「？，」 等连续标点
            //
            // 不做 overlap 去重：force_cut 的 SEGMENT_OVERLAP_MS 仅 200ms（≈1 字），
            // 真重叠与边界巧合无法区分，dedup 净误删（曾把「识别的效果」误删成「的效果」）；
            // silence-cut 段音频本就不重叠，更无需 dedup。
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

/// desktop ASR pipeline 统一上层抽象（2c-3，spec §3.1）。
///
/// `StreamingPipeline`（流式，内持 `StreamingPipelineEngine`）与 `VadSegmentedPipeline`
///（VAD 分段伪流式）各 impl。coordinator 调 tick/finish/silence 统一接口。
/// emit/DB/polish/transcript 留 coordinator（2d 收敛）。
///
/// `silence_duration`/`reset`/`take_close_handle`/`is_cloud` 当前仅 StreamingPipeline 通过
/// inherent 方法调用（trait 暂未走全路径），用 `#[allow(unused)]` 抑制直至 2d 全收敛。
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

/// desktop 流式 pipeline 引擎（上层抽象，spec §3.4 阶段2c-2）。
///
/// local（包 `StreamingRunner`）与 cloud（持 `CloudStreamHandle`）各 impl。
/// 同步 `tick` + 同步 `finish`；cloud 的 async close 不在此 trait
/// （留 coordinator，spec §2——`close_async` 必须 async，否则 `block_on` 卡主线程 8s）。
pub trait StreamingPipelineEngine: Send {
    /// 喂一帧已降噪 16k 样本，返回本帧 `TranscriptEvent`（0..n）。
    fn tick(&mut self, samples: &[f32]) -> Vec<TranscriptEvent>;
    /// 收尾 flush（tail 已由 stop 路径 tick 喂入 accept）：
    /// local → `StreamingRunner::finish`（Final）；cloud → 返回最后 `current_partial` 作 Committed 兜底。
    fn finish(&mut self) -> TranscriptEvent;
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
    fn take_close_handle(&mut self) -> Option<octopus_asr_cloud::CloudStreamHandle> { None }
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
    fn finish(&mut self) -> TranscriptEvent {
        self.0.finish()
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
    /// - local 的 `Partial`/`Committed`/`Final` 都 set-full（幂等去重）。
    /// - cloud 的预览（`current_partial`）**不**经过此——engine 自持 + 暴露 `current_partial()`
    ///   （spec §4.1）；仅 `Committed`（Finished）经此 set-full。
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

    /// 收尾 flush（tail 已由 stop 路径 tick 喂入 accept）。委托 engine（local→Final；cloud→兜底 Committed）。
    ///
    /// coordinator 的 Streaming stage 持具体类型 `StreamingPipeline`，调此 inherent 方法（0 参）。
    /// 同名 trait 方法 `Pipeline::finish(&mut self, &mut Transcript)`（1 参）待 Task 5 coordinator
    /// 改 `Box<dyn Pipeline>` 后启用，届时本 inherent 可删。
    pub fn finish(&mut self) -> TranscriptEvent {
        self.engine.finish()
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
    pub fn take_close_handle(&mut self) -> Option<octopus_asr_cloud::CloudStreamHandle> {
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

impl Pipeline for StreamingPipeline {
    fn tick(&mut self, samples: &[f32], transcript: &mut Transcript) -> bool {
        // 复用既有 inherent StreamingPipeline::tick（engine tick → set_full，返回 changed）。
        self.tick(samples, transcript)
    }
    fn finish(&mut self, _transcript: &mut Transcript) -> TranscriptEvent {
        // tail 已由 stop 路径 tick 喂入 accept；此处仅 flush。
        self.engine.finish()
    }
    fn silence_duration(&self) -> f64 {
        self.engine.silence_duration()
    }
    fn reset(&mut self) {
        self.engine.reset();
    }
    #[cfg(feature = "cloud")]
    fn take_close_handle(&mut self) -> Option<octopus_asr_cloud::CloudStreamHandle> {
        self.engine.take_close_handle()
    }
    fn is_cloud(&self) -> bool {
        self.engine.is_cloud()
    }
    // took_segment_cut 用默认 false（流式停顿润色走 silence_duration 每 tick 判）。
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

/// VadSegmented 伪流式 pipeline：封装双 VAD + 切段 + spawn + 乱序回填（2c-3）。
/// 非 VAD 依赖字段集合；engine/language/asr_engine/segment_silence_ms 是 config 子集（不 clone 整 AppConfig）。
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
    fn spawn_offline(&self, speech_samples: Vec<f32>, seq: u64) {
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
                seq,
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
    /// `samples` 空则跳过切段/spawn（步骤 1-5），仍走 drain_rx（WaitingCompletion 收尾靠此）。
    pub(crate) fn run_tick(&mut self, samples: &[f32], transcript: &mut Transcript) -> bool {
        self.segment_cut_this_tick = false;
        let mut changed = false;

        if !samples.is_empty() {
            self.audio_buffer.extend_from_slice(samples);

            let speech_chunks = compute_speech_chunks(&mut self.detect_vad, samples);
            if speech_chunks >= 2 {
                self.silence_duration = 0.0;
                self.has_speech = true;
            } else {
                let chunk_duration = samples.len() as f64 / 16000.0;
                self.silence_duration += chunk_duration;
            }

            let buffer_duration_s = self.audio_buffer.len() as f64 / 16000.0;
            let silence_ms = self.silence_duration * 1000.0;
            let silence_cut = self.has_speech && silence_ms >= self.segment_silence_ms;
            let force_cut = self.has_speech && buffer_duration_s >= SEGMENT_DURATION_S;
            if silence_cut || force_cut {
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
                    self.spawn_offline(speech_samples, seq);
                }
            }
        }

        if self.drain_rx_and_consume(transcript) {
            changed = true;
        }
        changed
    }

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
}

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
        is_cloud: bool,
    }
    impl FakePipelineEngine {
        fn new(tick: Vec<TranscriptEvent>, partial: &str, finish: TranscriptEvent) -> Self {
            Self {
                tick_out: Mutex::new(tick),
                partial: partial.to_string(),
                finish_out: finish,
                silence: 0.0,
                is_cloud: false,
            }
        }
        fn new_cloud(tick: Vec<TranscriptEvent>, partial: &str, finish: TranscriptEvent) -> Self {
            Self {
                tick_out: Mutex::new(tick),
                partial: partial.to_string(),
                finish_out: finish,
                silence: 0.0,
                is_cloud: true,
            }
        }
    }
    impl StreamingPipelineEngine for FakePipelineEngine {
        fn tick(&mut self, _samples: &[f32]) -> Vec<TranscriptEvent> {
            std::mem::take(&mut *self.tick_out.lock().unwrap())
        }
        fn finish(&mut self) -> TranscriptEvent {
            self.finish_out.clone()
        }
        fn silence_duration(&self) -> f64 { self.silence }
        fn current_partial(&self) -> &str { &self.partial }
        fn reset(&mut self) {}
        fn is_cloud(&self) -> bool { self.is_cloud }
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
    fn finish_delegates_to_engine() {
        let mut p = pipeline(FakePipelineEngine::new(
            vec![],
            "",
            TranscriptEvent::Final("最终。".to_string()),
        ));
        // 走 inherent StreamingPipeline::finish（coordinator Task 4 也走此路径；
        // Pipeline trait 同名方法 Task 5 起 Box<dyn Pipeline> 时方启用）。
        let ev = p.finish();
        assert_eq!(ev, TranscriptEvent::Final("最终。".to_string()));
    }

    // ── tick_events（2d Task 1）──

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

    #[cfg(feature = "cloud")]
    #[test]
    fn take_close_handle_none_for_local_fake() {
        // FakePipelineEngine 不覆盖 take_close_handle → 默认 None（与 LocalPipelineEngine 一致）。
        // 方法本身 cfg cloud，故测试同步门控（无 cloud feature 时不编译）。
        let mut p = pipeline(FakePipelineEngine::new(vec![], "", TranscriptEvent::Final("".to_string())));
        assert!(p.take_close_handle().is_none());
    }

    // ── VadSegmentedPipeline 纯逻辑（2c-3）──

    use super::{apply_segment_result, consume_completed_results_vad, SegmentResult};
    use std::collections::HashMap;

    #[test]
    fn apply_segment_result_normal_inserts_text() {
        let mut results = HashMap::new();
        let mut active = 1u32;
        apply_segment_result(&mut results, &mut active, SegmentResult {
            seq: 0, text: Ok("你好".to_string()),
        });
        assert_eq!(results.get(&0).map(String::as_str), Some("你好"));
        assert_eq!(active, 0);
    }

    #[test]
    fn apply_segment_result_empty_occupies_slot() {
        let mut results = HashMap::new();
        let mut active = 1u32;
        apply_segment_result(&mut results, &mut active, SegmentResult {
            seq: 0, text: Ok(String::new()),
        });
        assert_eq!(results.get(&0).map(String::as_str), Some(""));
        assert_eq!(active, 0);
    }

    #[test]
    fn apply_segment_result_failed_occupies_slot() {
        let mut results = HashMap::new();
        let mut active = 2u32;
        apply_segment_result(&mut results, &mut active, SegmentResult {
            seq: 0, text: Err("boom".to_string()),
        });
        assert_eq!(results.get(&0).map(String::as_str), Some(""));
        assert_eq!(active, 1);
    }

    #[test]
    fn consume_appends_only_contiguous_seq() {
        let mut completed_seq = 0u64;
        let mut results = HashMap::new();
        results.insert(0u64, "甲".to_string());
        results.insert(2u64, "丙".to_string());
        let mut t = Transcript::new(0, PolishMode::Disabled);
        consume_completed_results_vad(&mut completed_seq, &mut results, &mut t);
        assert_eq!(t.full(), "甲");
        assert_eq!(completed_seq, 1);
        assert!(results.contains_key(&2));

        results.insert(1u64, "乙".to_string());
        consume_completed_results_vad(&mut completed_seq, &mut results, &mut t);
        assert_eq!(t.full(), "甲，乙，丙");
        assert_eq!(completed_seq, 3);
        assert!(results.is_empty());
    }
}
