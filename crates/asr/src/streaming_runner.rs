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

/// VAD 块大小（样本数，16k 下 32ms）。
const VAD_CHUNK_SIZE: usize = 512;
/// 语音概率阈值。
const VAD_SPEECH_THRESHOLD: f32 = 0.5;
/// 标点（逗号）触发的静音时长阈值（秒）。
const PUNCTUATION_SILENCE_THRESHOLD: f64 = 0.5;
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
        let mut vad = crate::config::find_silero_vad()
            .ok()
            .and_then(|p| SileroVad::new(&p).ok());
        if let Some(v) = vad.as_mut() {
            preroll_vad(v);
        }
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

    /// 收尾并先吃入尾部样本（stop 路径用）。
    ///
    /// 精确等价 `coordinator.rs` stop 顺序：`engine.accept_samples(tail, false)`
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// 可编程 fake：accept/flush 按预设序列出队返回，finish 返回固定串。
    /// accept 队列耗尽时返回 `Err`（覆盖 error 路径）。
    struct FakeStreamingEngine {
        accept_out: Mutex<Vec<Option<String>>>,
        flush_out: Mutex<Vec<Option<String>>>,
        finish_out: Mutex<String>,
    }

    impl FakeStreamingEngine {
        fn new(accept: Vec<&str>, flush: Vec<&str>, finish: &str) -> Self {
            Self {
                accept_out: Mutex::new(
                    accept.into_iter().map(|s| Some(s.to_string())).collect(),
                ),
                flush_out: Mutex::new(
                    flush.into_iter().map(|s| Some(s.to_string())).collect(),
                ),
                finish_out: Mutex::new(finish.to_string()),
            }
        }
    }

    impl StreamingEngine for FakeStreamingEngine {
        fn accept_samples(&self, _samples: &[f32], _was_silent: bool) -> Result<Option<String>> {
            let mut q = self.accept_out.lock().unwrap();
            if q.is_empty() {
                anyhow::bail!("fake accept error");
            }
            Ok(q.remove(0))
        }
        fn flush(&self, _insert_comma: bool) -> Result<Option<String>> {
            let mut q = self.flush_out.lock().unwrap();
            if q.is_empty() {
                return Ok(None);
            }
            Ok(q.remove(0))
        }
        fn finish(&self) -> Result<String> {
            Ok(self.finish_out.lock().unwrap().clone())
        }
        fn reset(&self) {}
    }

    fn runner(fake: FakeStreamingEngine) -> StreamingRunner {
        StreamingRunner::new(Box::new(fake), false).unwrap()
    }

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

    #[test]
    fn push_samples_relays_accept_as_partial() {
        // VAD=None（测试环境无 silero 模型）→ 无标点/冲刷；accept 首次返回 Some("你好")
        let mut r = runner(FakeStreamingEngine::new(vec!["你好"], vec![], "你好。"));
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
        // accept 队列空 → bail! → Error 事件；非致命，finish 仍可调用
        let mut r = runner(FakeStreamingEngine::new(vec![], vec![], ""));
        let evs = r.push_samples(&[0.0; 512]);
        assert_eq!(evs.len(), 1);
        assert!(matches!(evs[0], TranscriptEvent::Error(_)));
        let _ = r.finish(); // 不 panic
    }

    #[test]
    fn finish_with_tail_emits_final() {
        // accept 队列给 1 个（tail 吃入），finish 返回固定串
        let mut r = runner(FakeStreamingEngine::new(vec!["尾"], vec![], "最终。"));
        let ev = r.finish_with_tail(&[0.0; 512]);
        assert_eq!(ev, TranscriptEvent::Final("最终。".to_string()));
    }

    #[test]
    fn finish_with_tail_empty_tail_still_finishes() {
        // 空 tail → 不调 accept（队列不消耗）→ finish 直接返回
        let mut r = runner(FakeStreamingEngine::new(vec![], vec![], "空尾。"));
        let ev = r.finish_with_tail(&[]);
        assert_eq!(ev, TranscriptEvent::Final("空尾。".to_string()));
    }
}
