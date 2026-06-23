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
use octopus_asr::streaming_runner::{StreamingEngine, StreamingRunner, TranscriptEvent};

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

    /// 可编程 fake（搬自 `streaming_runner::tests`，简化：只 accept + finish）。
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
