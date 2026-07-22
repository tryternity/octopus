//! desktop 流式 pipeline（spec §3.4 阶段 2c-1/2c-2）。
//!
//! [`StreamingPipeline`] 持 `Box<dyn StreamingPipelineEngine>`（上层抽象），承载
//! 「engine 事件（`TranscriptEvent`）→ 文本状态更新（`Transcript::apply_engine_full`）」。
//! - [`LocalPipelineEngine`]：薄包 asr `StreamingRunner`（VAD + accept/flush，2a/2b/2c-1）。
//! - `CloudPipelineEngine`（cfg cloud，见 `cloud_pipeline.rs`）：持 `CloudStreamHandle`
//!   （onset/push/drain/双层文本/静音非阻塞 finish，2c-2）。cloud 的 async close 不在
//!   trait（留 coordinator，spec §2）。
//!
//! **边界**：emit（`result_window::update_result`）/DB（`update_transcription_raw`）/polish
//! （`check_and_trigger_polish`）留 coordinator（emit 与 DB 同步触发以保持 `apply_engine_full→DB→emit`
//! 顺序；DB/polish 被 local + VadSegmented + cloud 三路径共用）。transcript 也留
//! `Stage::Streaming`，`tick` 接收 `&mut Transcript`。全收敛留 2d。

use crate::transcript::Transcript;
use log::warn;
use octopus_asr_local::streaming_runner::{StreamingEngine, StreamingRunner, TranscriptEvent};
use octopus_asr_local::vad::SileroVad;
use std::collections::HashMap;
use std::sync::Arc;
use octopus_infra::consts::{SEGMENT_DURATION_S, SEGMENT_OVERLAP_MS};

/// pipeline tick 产出的「该做什么」事件。coordinator `apply_pipeline_events` 据此执行端动作
/// （DB/emit/polish/错误上报）。不携带 transcript 状态（transcript 留 Stage，coordinator 持 &mut）
/// ——只携带「决定 + 必要字符串」。（2d，spec §3.2）
#[derive(Debug, PartialEq)]
pub enum PipelineEvent {
    /// 落库 text/segments（pipeline 已判文本变化，写 raw 段）。engine_mode = DB engine_mode 列（"streaming"/"vad_segmented"）。
    /// coordinator 调 update_transcription_raw(&mut transcript, &config.asr_engine, engine_mode)。
    PersistRaw { engine_mode: &'static str },
    /// 刷新结果窗口。display 已由 pipeline 算好（local=transcript.display_text()；cloud=display+current_partial）。
    /// `insertion=true` 表示中间插入态（caret_gap < segments.len()），前端立即渲染（跳过 300ms diverted 延迟）。
    /// `caret` = transcript.caret_char_offset()（扁平文本里光标的 char 偏移，随插入自然增长；前端据此定位闪烁
    /// 光标，使其跟在最后插入的文字后右移，而非停在点击点）。insertion=false 时前端忽略 caret。
    Emit { display: String, insertion: bool, caret: usize },
    /// 触发停顿润色。silence = 停顿时长（streaming 传 silence_duration；vad-seg 段边界传 f64::INFINITY 必过，
    /// 等价原 after_vad_tick 传 pause_polish_threshold_ms 让 check_and_trigger_polish 静音检查自动达标）。
    /// coordinator 调 check_and_trigger_polish(&mut transcript, silence, config, tx)（防抖五重检查原样在彼处）。
    Polish { silence: f64 },
    /// 用户可见错误（cloud WSS 开启失败 / StreamEvent::Failed；local 错误只在承载层 warn，不产此事件）。
    Error(String),
    /// VAD 说话状态变化（有语音→无语音 或 无语音→有语音）。coordinator emit("update-speaking")。
    Speaking(bool),
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

/// `spawn_offline` 的 panic/abort 兜底（2026-07-09 审查防御）：
/// spawned 识别 task 若 panic（unwind），闭包内 `tx.send` 不会执行 → active_count 永不归零
/// → coordinator `WaitingCompletion` 永挂。此 guard 持 tx clone，Drop 时若未 `done` 则发 Err sentinel，
/// Rust 保证 panic unwind 时局部变量 drop，故 sentinel 必发、active_count 必归零。
/// profile 为 panic=unwind（托盘 app 需存活）；若改 panic=abort 则进程直接崩，无需此守卫。
struct SendOnDrop {
    tx: std::sync::mpsc::Sender<SegmentResult>,
    seq: u64,
    done: bool,
}

impl Drop for SendOnDrop {
    fn drop(&mut self) {
        if !self.done {
            let _ = self.tx.send(SegmentResult {
                seq: self.seq,
                text: Err("spawned transcription task aborted/panicked".into()),
            });
        }
    }
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
            // log::info!("VadSegmented seq={}: '{}'", seg.seq, t);
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
    language: &str,
) {
    let sep = octopus_asr_local::sentence_separator(language);
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
                transcript.append_segment(sep);
            }
            transcript.append_segment(&text);
        }
        *completed_seq += 1;
    }
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
    /// 构造 local 引擎，包已取用的流式引擎 Arc（来自 StreamingSessionManager，
    /// 录音结束 pipeline drop 仅释放此 Arc clone，manager 原 Arc 仍持有 → 引擎不销毁、下次复用）。
    /// 内部构造 `StreamingRunner`（含 VAD 预热，2a/2b）。
    pub fn from_session(engine: Arc<dyn StreamingEngine>, correct: bool) -> anyhow::Result<Self> {
        Ok(Self(StreamingRunner::new(engine, correct)?))
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

/// local 流式 pipeline 壳：持 `Box<dyn StreamingPipelineEngine>`，承载事件 → apply_engine_full。
///
/// 不持 transcript（留 `Stage::Streaming`），`tick` 接收 `&mut Transcript`。
/// 不持 denoise/resample（留 `audio.rs`，输入为已降噪 16k 样本）。
pub struct StreamingPipeline {
    engine: Box<dyn StreamingPipelineEngine>,
    /// 上一 tick 承载层捕获的用户可见错误（cloud WSS 开启失败 / `StreamEvent::Failed`）。
    /// 由 `tick` 在下个调用取出注入 `PipelineEvent::Error`（2d 收敛）；local 错误只在承载层 warn。
    last_error: Option<String>,
    /// 上一 tick 的 has_speech 状态（变化时产 Speaking 事件）
    prev_speaking: bool,
    /// 诊断打点节流（spec 2026-07-19 第二轮）：[TICK-DETAIL] 1Hz 节流。
    last_detail_log: std::time::Instant,
}

impl StreamingPipeline {
    /// 构造 pipeline。`engine` 由调用方创建（`LocalPipelineEngine` 或 `CloudPipelineEngine`）。
    pub fn new(engine: Box<dyn StreamingPipelineEngine>) -> anyhow::Result<Self> {
        Ok(Self {
            engine, last_error: None, prev_speaking: false,
            last_detail_log: std::time::Instant::now(),
        })
    }

    /// 喂一帧已降噪 16k 样本：engine 产事件 → apply_engine_full，返回 tick 事件流（2d 合并）。
    /// - local：changed→[PersistRaw,Emit]；每 tick→[Polish]；空样本→[]（早退）
    /// - cloud：changed→[PersistRaw,Polish]；每 tick→[Emit{display+partial}]；error→[Error]
    pub fn tick(&mut self, samples: &[f32], transcript: &mut Transcript) -> Vec<PipelineEvent> {
        let t_start = std::time::Instant::now();
        let is_cloud = self.engine.is_cloud();
        // local 空样本早退（等价原 handle_streaming_tick L1370）；cloud 不早退（仍 emit 预览/drain）
        if !is_cloud && samples.is_empty() {
            return Vec::new();
        }
        let mut changed = false;
        let t_infer = std::time::Instant::now();
        for event in self.engine.tick(samples) {
            match event {
                TranscriptEvent::Partial(text) | TranscriptEvent::Committed(text) => {
                    if transcript.apply_engine_full(&text) { changed = true; }
                }
                TranscriptEvent::Final(text) => {
                    transcript.apply_engine_full(&text);
                    changed = true;
                }
                TranscriptEvent::Error(e) => {
                    warn!("StreamingPipeline event error: {}", e);
                    self.last_error = Some(e);
                }
            }
        }
        let infer_ms = t_infer.elapsed().as_millis();
        let mut events = Vec::new();
        // VAD 说话状态变化 → Speaking 事件
        let speaking = self.engine.silence_duration() < 0.3;
        if speaking != self.prev_speaking {
            crate::perf_log::log(&format!(
                "[SPEAKING] local {} silence={:.2}",
                speaking, self.engine.silence_duration(),
            ));
            self.prev_speaking = speaking;
            events.push(PipelineEvent::Speaking(speaking));
        }
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
            events.push(PipelineEvent::Emit { display, insertion: transcript.is_inserting(), caret: transcript.caret_char_offset() });
        } else {
            if changed {
                events.push(PipelineEvent::PersistRaw { engine_mode: "streaming" });
                events.push(PipelineEvent::Emit { display: transcript.display_text(), insertion: transcript.is_inserting(), caret: transcript.caret_char_offset() });
            }
            // local 每 tick 查停顿润色（等价原 handle_streaming_tick L1408）
            events.push(PipelineEvent::Polish { silence: self.engine.silence_duration() });
        }
        let total_ms = t_start.elapsed().as_millis();
        if total_ms > 30 {
            crate::perf_log::log(&format!(
                "[BE tick] total={}ms infer={}ms samples={} changed={} is_cloud={}",
                total_ms, infer_ms, samples.len(), changed, is_cloud
            ));
        }
        // 诊断（spec 2026-07-19 第二轮）：tick 详情 1Hz 节流，验证假设 B（绿条延迟亮 + 不出字）
        // 包含 silence / speaking / changed / events 数。配合 streaming_runner.rs 内部 [runner] debug 互补。
        if self.last_detail_log.elapsed() >= std::time::Duration::from_secs(1) {
            crate::perf_log::log(&format!(
                "[TICK-DETAIL] pipeline-local silence={:.2} speaking={} prev_speaking={} changed={} events={} infer_events={} samples={} infer_ms={} is_cloud={}",
                self.engine.silence_duration(), speaking, self.prev_speaking, changed, events.len(),
                infer_ms, samples.len(), infer_ms, is_cloud,
            ));
            self.last_detail_log = std::time::Instant::now();
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

    /// cloud 预览（`current_partial`），local 恒空。coordinator display 拼接用。
    /// 仅 cloud feature 下由 coordinator 调用（默认构建无 cloud 时为 dead code）。
    #[allow(dead_code)]
    pub fn current_partial(&self) -> &str {
        self.engine.current_partial()
    }

    /// stop 路径分派：cloud → `Some(CloudStreamHandle)`（coordinator spawn close_async）；local → `None`。
    /// cfg cloud（与 trait 方法同步门控）。
    #[cfg(feature = "cloud")]
    pub fn take_close_handle(&mut self) -> Option<octopus_asr_cloud::CloudStreamHandle> {
        self.engine.take_close_handle()
    }

    /// 是否 cloud 引擎（§4.2/§4.3 不对称判别）。
    /// 仅 cloud feature 下由 coordinator 调用（默认构建无 cloud 时为 dead code）。
    #[allow(dead_code)]
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

/// 对缓冲区音频做 VAD 过滤（搬迁自 coordinator.rs:1495）。
/// 用独立 `filter_vad`（与检测流分离），过滤前 reset()+preroll 归零并预热 LSTM
///（与 detect_vad 对称——2026-07-09 审查补 preroll：冷启动段首几帧 prob 偏低，filter_speech
/// 的 first_active 可能偏后丢音头；preroll 喂静音让 LSTM 进入静音稳态，改善首帧响应）。
fn filter_speech_from_buffer(filter_vad: &mut SileroVad, samples: &[f32]) -> Vec<f32> {
    filter_vad.reset();
    vad_preroll(filter_vad);
    let speech = octopus_asr_local::audio::filter_speech(samples, filter_vad, 480, 0.5);
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
    /// 检测 VAD（流式有状态，跨 tick 续接，切段后 reset+preroll 归零——见 spec 2026-07-08-vad-segmented-pipeline §5①）。
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
    /// 上一 tick 的 has_speech 状态（变化时产 Speaking 事件）
    prev_speaking: bool,
    /// 诊断打点节流（spec 2026-07-19 第二轮）：[TICK-DETAIL] 1Hz 节流。
    last_detail_log: std::time::Instant,
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
        let mut detect_vad = octopus_asr_local::config::create_silero_vad()?;
        vad_preroll(&mut detect_vad);
        let filter_vad = octopus_asr_local::config::create_silero_vad()?;
        let (tx, rx) = std::sync::mpsc::channel();
        Ok(Self {
            engine, language, asr_engine, segment_silence_ms,
            detect_vad, filter_vad,
            audio_buffer: Vec::new(), overlap_tail: Vec::new(),
            silence_duration: 0.0, has_speech: false,
            active_count: 0, next_seq: 0, completed_seq: 0,
            completed_results: HashMap::new(),
            tx, rx, segment_cut_this_tick: false,
            prev_speaking: false,
            last_detail_log: std::time::Instant::now(),
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
        let _duration = samples_len as f64 / 16000.0;
        tauri::async_runtime::spawn(async move {
            let start = std::time::Instant::now();
            // guard：panic unwind 时 Drop 发 Err sentinel，保 active_count 归零（防 WaitingCompletion 永挂）。
            let mut guard = SendOnDrop { tx, seq, done: false };
            let result = engine.transcribe(&speech_samples, &language, &asr_engine).await;
            let _elapsed = start.elapsed();
            // log::info!(
            //     "Transcription seq={} took {:.2}s (audio: {:.2}s, RTF: {:.2})",
            //     seq, elapsed.as_secs_f64(), duration,
            //     elapsed.as_secs_f64() / duration.max(0.001)
            // );
            let _ = guard.tx.send(SegmentResult {
                seq: guard.seq,
                text: result.map_err(|e| e.to_string()),
            });
            guard.done = true;
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
            &mut self.completed_seq, &mut self.completed_results, transcript, &self.language,
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
            // force_cut 解绑 has_speech：detect_vad 漂移失灵时 has_speech 卡 false 致 buffer 无限堆积。
            // 达上限必切，由 filter_vad（每段 reset，不受漂移污染）独立兜底判定有无语音。
            let force_cut = buffer_duration_s >= SEGMENT_DURATION_S;
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
                // 切段 = 一段语音结束、新段开始，是 LSTM 状态的安全重置点。
                // reset+preroll 让 detect_vad 从干净状态检测（与构造 L371 对称），消除跨段累积漂移。
                // 根因：detect_vad 会话内从不 reset，几段后 LSTM 漂移致真实语音持续 prob<0.5 →
                // has_speech 卡 false → silence_cut/force_cut 均不触发 → buffer 无限堆积不吐字。
                self.detect_vad.reset();
                vad_preroll(&mut self.detect_vad);

                let speech_samples = filter_speech_from_buffer(&mut self.filter_vad, &send_buffer);
                if !speech_samples.is_empty() {
                    self.segment_cut_this_tick = true;
                    let seq = self.next_seq;
                    self.next_seq += 1;
                    self.active_count += 1;
                    // log::debug!(
                    //     "VadSegmented: {} cut, seq={}, samples={}, active_count={}",
                    //     if force_cut { "force" } else { "silence" },
                    //     seq, speech_samples.len(), self.active_count,
                    // );
                    self.spawn_offline(speech_samples, seq);
                }
            }
        }

        if self.drain_rx_and_consume(transcript) {
            changed = true;
        }
        changed
    }

    /// tick 事件流（2d 合并，spec §3.4）。复用 `run_tick`（双 VAD+切段+spawn+drain+apply_engine_full，不重复），
    /// 按 `changed`/`segment_cut` 产事件：
    /// `changed`→`[PersistRaw{vad_segmented}, Emit]`；`segment_cut`→追加 `[Polish{INFINITY}]`
    ///（段边界 silence 必过，等价原 after_vad_tick L1221 传 pause_polish_threshold_ms）。
    /// WaitingCompletion 收尾也走此（空样本 run_tick 跳过切段仅 drain，segment_cut 恒 false → 无 Polish）。
    pub(crate) fn tick(
        &mut self,
        samples: &[f32],
        transcript: &mut Transcript,
    ) -> Vec<PipelineEvent> {
        let changed = self.run_tick(samples, transcript);
        let segment_cut = self.segment_cut_this_tick;
        let mut events = Vec::new();
        // VAD 说话状态变化 → Speaking 事件
        // has_speech=true 才算说话（开口后才亮）；has_speech=false 时一定不算
        let speaking = self.has_speech && self.silence_duration < 0.3;
        if speaking != self.prev_speaking {
            log::info!("[vad-seg] speaking {} → {} (has_speech={}, silence={:.2})", self.prev_speaking, speaking, self.has_speech, self.silence_duration);
            crate::perf_log::log(&format!(
                "[SPEAKING] vad-seg {} has_speech={} silence={:.2}",
                speaking, self.has_speech, self.silence_duration,
            ));
            self.prev_speaking = speaking;
            events.push(PipelineEvent::Speaking(speaking));
        }
        if changed {
            events.push(PipelineEvent::PersistRaw { engine_mode: "vad_segmented" });
            events.push(PipelineEvent::Emit { display: transcript.display_text(), insertion: transcript.is_inserting(), caret: transcript.caret_char_offset() });
        }
        if segment_cut {
            events.push(PipelineEvent::Polish { silence: f64::INFINITY });
        }
        // 诊断（spec 2026-07-19 第二轮）：tick 详情 1Hz 节流，验证假设 B
        if self.last_detail_log.elapsed() >= std::time::Duration::from_secs(1) {
            crate::perf_log::log(&format!(
                "[TICK-DETAIL] pipeline-vad-seg silence={:.2} has_speech={} speaking={} prev_speaking={} changed={} events={} samples={} active_count={} buffer_s={:.1}",
                self.silence_duration, self.has_speech, speaking, self.prev_speaking, changed, events.len(),
                samples.len(), self.active_count,
                self.audio_buffer.len() as f64 / 16000.0,
            ));
            self.last_detail_log = std::time::Instant::now();
        }
        events
    }

    /// 收尾：强制转码末段 audio_buffer + drain rx 至空 + 消费在途段。
    ///
    /// 末段处理：stop 路径 tick(tail) 可能未触发 silence_cut/force_cut（末尾静音不足 /
    /// buffer 未满），剩余 audio_buffer（+ overlap_tail）若不主动转码会丢失——末段甚至
    /// 整句（active_count==0 时 coordinator 直接 finalize）。此处复用 tick 的切段口径
    /// （overlap_tail + audio_buffer 合并 → filter_vad 判语音 → spawn_offline + active_count+1），
    /// spawn 后 active_count>0 让 coordinator 进 WaitingCompletion 轮询收尾（2026-07-09 审查修复）。
    pub fn finish(&mut self, transcript: &mut Transcript) -> TranscriptEvent {
        if !self.audio_buffer.is_empty() {
            let mut send_buffer = self.overlap_tail.clone();
            send_buffer.extend_from_slice(&self.audio_buffer);
            self.audio_buffer.clear();
            self.overlap_tail.clear();
            let speech_samples = filter_speech_from_buffer(&mut self.filter_vad, &send_buffer);
            if !speech_samples.is_empty() {
                let seq = self.next_seq;
                self.next_seq += 1;
                self.active_count += 1;
                self.spawn_offline(speech_samples, seq);
            }
        }
        self.drain_rx_and_consume(transcript);
        // VadSegmented 不产 Final 事件（文本经 append_segment 累积），返回空 Committed 作占位
        //（coordinator stop 路径不读 vad-seg 的 finish 返回值）。
        TranscriptEvent::Committed(String::new())
    }

    /// 重置（会话间复用）：清缓冲 + VAD 状态。rx 内残余旧段丢弃（新会话 seq 从 0 重来）。
    /// 当前 coordinator 在 stage 切换时直接 drop pipeline（未调 reset），保留供未来会话复用场景。
    #[allow(dead_code)]
    pub fn reset(&mut self) {
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::PolishMode;
    use parking_lot::Mutex;

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
            std::mem::take(&mut *self.tick_out.lock())
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
        let mut t = Transcript::new(0, PolishMode::Disabled, crate::coordinator::RecordType::Input);
        let events = p.tick(&[0.0; 1600], &mut t);
        assert_eq!(t.full(), "你好");
        assert!(events.contains(&PipelineEvent::PersistRaw { engine_mode: "streaming" }));
    }

    #[test]
    fn tick_final_overrides_transcript() {
        // Final 显式承载（2c-2 新增分支，local stop 产 Final）。
        // 段模型下 apply_engine_full 走前缀追加：预设「最终」→ 喂 Final「最终。」前缀追加「。」。
        let mut p = pipeline(FakePipelineEngine::new(
            vec![TranscriptEvent::Final("最终。".to_string())],
            "",
            TranscriptEvent::Final("最终。".to_string()),
        ));
        let mut t = Transcript::new(0, PolishMode::Disabled, crate::coordinator::RecordType::Input);
        t.apply_engine_full("最终");
        let events = p.tick(&[0.0; 1600], &mut t);
        assert_eq!(t.full(), "最终。"); // Final 前缀推进
        assert!(events.contains(&PipelineEvent::PersistRaw { engine_mode: "streaming" }));
    }

    #[test]
    fn tick_committed_idempotent_no_change_skip() {
        // Committed 与当前 full 相同 → apply_engine_full delta 空 → changed=false → 只产 Polish（local 每 tick），无 PersistRaw/Emit
        let mut p = pipeline(FakePipelineEngine::new(
            vec![TranscriptEvent::Committed("一样".to_string())],
            "",
            TranscriptEvent::Final("".to_string()),
        ));
        let mut t = Transcript::new(0, PolishMode::Disabled, crate::coordinator::RecordType::Input);
        t.apply_engine_full("一样");
        let events = p.tick(&[0.0; 1600], &mut t);
        assert!(!events.contains(&PipelineEvent::PersistRaw { engine_mode: "streaming" }));
        assert_eq!(events, vec![PipelineEvent::Speaking(true), PipelineEvent::Polish { silence: 0.0 }]);
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

    // ── tick 事件流（2d Task 1）──

    #[test]
    fn tick_events_local_changed_produces_persist_emit_polish() {
        let mut p = pipeline(FakePipelineEngine::new(
            vec![TranscriptEvent::Partial("你好".to_string())],
            "", TranscriptEvent::Final("".into()),
        ));
        let mut t = Transcript::new(0, PolishMode::Disabled, crate::coordinator::RecordType::Input);
        let events = p.tick(&[0.0; 1600], &mut t);
        assert_eq!(events, vec![
            PipelineEvent::Speaking(true),
            PipelineEvent::PersistRaw { engine_mode: "streaming" },
            PipelineEvent::Emit { display: "你好".to_string(), insertion: false, caret: 2 },
            PipelineEvent::Polish { silence: 0.0 },
        ]);
    }

    #[test]
    fn tick_events_local_empty_samples_returns_empty() {
        let mut p = pipeline(FakePipelineEngine::new(vec![], "", TranscriptEvent::Final("".into())));
        let mut t = Transcript::new(0, PolishMode::Disabled, crate::coordinator::RecordType::Input);
        assert!(p.tick(&[], &mut t).is_empty());
    }

    #[test]
    fn tick_events_local_no_change_only_polish() {
        // Committed 与 full 同 → apply_engine_full delta 空 → changed=false → 只产 Polish（local 每 tick）
        let mut p = pipeline(FakePipelineEngine::new(
            vec![TranscriptEvent::Committed("一样".into())], "", TranscriptEvent::Final("".into()),
        ));
        let mut t = Transcript::new(0, PolishMode::Disabled, crate::coordinator::RecordType::Input);
        t.apply_engine_full("一样");
        let events = p.tick(&[0.0; 1600], &mut t);
        assert_eq!(events, vec![PipelineEvent::Speaking(true), PipelineEvent::Polish { silence: 0.0 }]);
    }

    #[test]
    fn tick_events_cloud_changed_emits_display_with_partial() {
        let mut p = pipeline(FakePipelineEngine::new_cloud(
            vec![TranscriptEvent::Committed("已提交".into())],
            "预览中", TranscriptEvent::Final("".into()),
        ));
        let mut t = Transcript::new(0, PolishMode::Disabled, crate::coordinator::RecordType::Input);
        let events = p.tick(&[0.0; 1600], &mut t);
        // changed → PersistRaw + Polish；每 tick Emit(display+partial) = "已提交预览中"
        assert!(events.contains(&PipelineEvent::PersistRaw { engine_mode: "streaming" }));
        assert!(events.iter().any(|e| matches!(e, PipelineEvent::Emit { display, insertion: false, .. } if display == "已提交预览中")));
    }

    #[test]
    fn tick_events_cloud_error_produces_error_event() {
        let mut p = pipeline(FakePipelineEngine::new_cloud(
            vec![TranscriptEvent::Error("boom".into())],
            "", TranscriptEvent::Final("".into()),
        ));
        let mut t = Transcript::new(0, PolishMode::Disabled, crate::coordinator::RecordType::Input);
        let events = p.tick(&[0.0; 1600], &mut t);
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

    use super::{apply_segment_result, consume_completed_results_vad, SegmentResult, SendOnDrop};
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
    fn send_on_drop_sends_sentinel_when_not_done() {
        // 模拟 panic：guard 在 done=false 时 drop → 发 Err sentinel（保 active_count 归零）
        let (tx, rx) = std::sync::mpsc::channel();
        {
            let _g = SendOnDrop { tx, seq: 42, done: false };
        }
        let seg = rx.try_recv().expect("Drop 应发 sentinel");
        assert_eq!(seg.seq, 42);
        assert!(seg.text.is_err(), "sentinel 应为 Err");
    }

    #[test]
    fn send_on_drop_silent_when_done() {
        let (tx, rx) = std::sync::mpsc::channel();
        {
            let mut g = SendOnDrop { tx, seq: 7, done: false };
            g.done = true;
        }
        assert!(rx.try_recv().is_err(), "done=true 时不应发 sentinel");
    }

    #[test]
    fn consume_appends_only_contiguous_seq() {
        let mut completed_seq = 0u64;
        let mut results = HashMap::new();
        results.insert(0u64, "甲".to_string());
        results.insert(2u64, "丙".to_string());
        let mut t = Transcript::new(0, PolishMode::Disabled, crate::coordinator::RecordType::Input);
        consume_completed_results_vad(&mut completed_seq, &mut results, &mut t, "zh");
        assert_eq!(t.full(), "甲");
        assert_eq!(completed_seq, 1);
        assert!(results.contains_key(&2));

        results.insert(1u64, "乙".to_string());
        consume_completed_results_vad(&mut completed_seq, &mut results, &mut t, "zh");
        assert_eq!(t.full(), "甲，乙，丙");
        assert_eq!(completed_seq, 3);
        assert!(results.is_empty());
    }

    // ── Speaking 事件测试 ──

    /// StreamingPipeline：silence_duration < 0.3 时 Speaking(true)，≥0.3 时 Speaking(false)
    #[test]
    fn streaming_speaking_event_on_silence_change() {
        let mut t = Transcript::new(0, PolishMode::Disabled, crate::coordinator::RecordType::Input);

        // tick 1: silence=0 → speaking=true
        let mut fake1 = FakePipelineEngine::new(vec![], "", TranscriptEvent::Final("".to_string()));
        fake1.silence = 0.0;
        let mut p = StreamingPipeline::new(Box::new(fake1)).unwrap();
        let events = p.tick(&[0.0; 1600], &mut t);
        assert!(events.iter().any(|e| matches!(e, PipelineEvent::Speaking(true))),
            "silence=0 < 0.3 → should emit Speaking(true)");

        // tick 2: silence=0.5 → speaking=false
        let mut fake2 = FakePipelineEngine::new(vec![], "", TranscriptEvent::Final("".to_string()));
        fake2.silence = 0.5;
        p.engine = Box::new(fake2);
        let events = p.tick(&[0.0; 1600], &mut t);
        assert!(events.iter().any(|e| matches!(e, PipelineEvent::Speaking(false))),
            "silence=0.5 ≥ 0.3 → should emit Speaking(false)");

        // tick 3: silence=0 → speaking=true again
        let mut fake3 = FakePipelineEngine::new(vec![], "", TranscriptEvent::Final("".to_string()));
        fake3.silence = 0.0;
        p.engine = Box::new(fake3);
        let events = p.tick(&[0.0; 1600], &mut t);
        assert!(events.iter().any(|e| matches!(e, PipelineEvent::Speaking(true))),
            "silence back to 0 → should emit Speaking(true)");
    }

    /// StreamingPipeline：状态不变时不重复 emit
    #[test]
    fn streaming_no_speaking_event_when_unchanged() {
        let mut fake = FakePipelineEngine::new(vec![], "", TranscriptEvent::Final("".to_string()));
        fake.silence = 0.0;
        let mut p = StreamingPipeline::new(Box::new(fake)).unwrap();
        let mut t = Transcript::new(0, PolishMode::Disabled, crate::coordinator::RecordType::Input);
        // 第 1 tick：emit Speaking(true)
        let events = p.tick(&[0.0; 1600], &mut t);
        assert!(events.iter().any(|e| matches!(e, PipelineEvent::Speaking(true))));
        // 第 2 tick：silence 仍 0 → 不应重复 emit
        let events = p.tick(&[0.0; 1600], &mut t);
        assert!(!events.iter().any(|e| matches!(e, PipelineEvent::Speaking(_))),
            "silence unchanged → no Speaking event");
    }

    // ── VadSegmented force_cut 兜底（SenseVoice "几段后不吐字" 根因 B 修复回归）──

    /// Dummy TranscriptionEngine：全静音不触发 spawn，transcribe 不被调（安全返回即可）。
    struct DummyTranscriptionEngine;
    #[async_trait::async_trait]
    impl crate::engine::TranscriptionEngine for DummyTranscriptionEngine {
        async fn transcribe(&self, _: &[f32], _: &str, _: &str) -> anyhow::Result<String> {
            Ok("dummy".to_string())
        }
        async fn health_check(&self) -> bool { true }
    }

    /// 探 silero_vad 可加载则构造 VadSegmentedPipeline，失败返回 None（测试 skip 不 FAIL）。
    fn try_new_vad_pipeline() -> Option<VadSegmentedPipeline> {
        octopus_asr_local::config::create_silero_vad().ok()?;
        let engine: std::sync::Arc<dyn crate::engine::TranscriptionEngine> =
            std::sync::Arc::new(DummyTranscriptionEngine);
        VadSegmentedPipeline::new(engine, "zh".into(), "sensevoice".into(), 800.0).ok()
    }

    #[test]
    fn force_cut_clears_buffer_when_no_speech_detected() {
        // 回归（根因 B 兜底）：detect_vad LSTM 漂移致 has_speech 卡 false 时，force_cut（解绑 has_speech）
        // 应在 buffer 达 SEGMENT_DURATION_S 时触发切段清空 buffer，防无限堆积。
        // 此前 force_cut 被 && has_speech 门控，has_speech=false 时永不触发 → 不吐字。
        let mut p = match try_new_vad_pipeline() {
            Some(p) => p,
            None => { println!("skip: 测试环境无 silero_vad 模型"); return; }
        };
        let mut t = Transcript::new(0, PolishMode::Disabled, crate::coordinator::RecordType::Input);
        // 灌 SEGMENT_DURATION_S(20s) 纯静音：detect_vad 已 preroll 静音稳态 → 判无语音 → has_speech 保持 false。
        // force_cut = buffer_duration_s >= SEGMENT_DURATION_S 触发切段（与 has_speech 无关）→ audio_buffer 清空。
        let silence = vec![0.0_f32; (SEGMENT_DURATION_S * 16000.0) as usize];
        p.run_tick(&silence, &mut t);
        assert!(p.audio_buffer.is_empty(),
            "force_cut 应清空 buffer（has_speech=false 兜底），实际 len={}", p.audio_buffer.len());
    }
}
