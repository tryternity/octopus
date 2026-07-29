//! 云端流式 pipeline 引擎（spec §3.4 阶段2c-2，cfg cloud）。
//!
//! [`CloudPipelineEngine`] impl [`crate::engine::pipeline::StreamingPipelineEngine`]，把原
//! `coordinator::handle_cloud_streaming_tick` 的 ASR 编排（VAD onset / push_pcm / drain
//! events / partial-transcript 双层 / 静音非阻塞 finish）迁入 `tick`，产
//! `Vec<TranscriptEvent>`。emit/DB/polish 留 coordinator（§4.2 不对称）。

use crate::engine::pipeline::{compute_speech_chunks, StreamingPipelineEngine};
use log::{debug, error, info, warn};
use crate::core::error_util::e2s;
use octopus_asr_local::streaming_runner::TranscriptEvent;
use octopus_asr_local::vad::SileroVad;
use octopus_asr_cloud::{CloudStreamHandle, StreamEvent};

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
    pub language: &'a str,
}

/// drain `try_recv_text` 事件并映射为 `TranscriptEvent`（迁自 `handle_cloud_streaming_tick:1721-1770`）。
///
/// - `Text(t)` 非空 → `current_partial=t`（**预览层，不发事件**，不进 transcript/DB）。
/// - `Finished` → `committed_text` 追加（按 language 选分隔符：英文空格 / 其他中文逗号）+
///   发 `Committed(committed_text)`（**DB 触发点**，由承载层 set_full）；清 `current_partial`；
///   `is_closing=false`、`is_speaking=false`。
/// - `Failed(msg)` → 发 `Error("⚠️ 云端识别失败：{msg}")`（coordinator 取 `take_error` 上报）；
///   清 `current_partial`/状态（下次 onset 重开，瞬时抖动自动重试）。
/// - drain 后 `!is_closing && !is_speaking` → `session.take()`（drop → channels 关 → WS task 结束）。
pub(super) fn drain_cloud_session(s: CloudDrainState) -> Vec<TranscriptEvent> {
    let sep = octopus_asr_local::sentence_separator(s.language);
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
                        if !s.committed_text.is_empty() && !s.committed_text.ends_with(sep) {
                            s.committed_text.push_str(sep);
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
    pause_polish_threshold_ms: f64,
) -> bool {
    is_speaking && !is_closing && silence_ms >= pause_polish_threshold_ms
}

/// 从 pre-roll 滚动缓冲区取最后 `CLOUD_PREROLL_SAMPLES` 样本作为前导音频（迁自 coordinator）。
pub(super) fn take_preroll(pre_roll_buffer: &[f32]) -> Vec<f32> {
    if pre_roll_buffer.len() >= CLOUD_PREROLL_SAMPLES {
        pre_roll_buffer[pre_roll_buffer.len() - CLOUD_PREROLL_SAMPLES..].to_vec()
    } else {
        pre_roll_buffer.to_vec()
    }
}

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
    .map_err(e2s)
}

/// cloud 流式 pipeline 引擎（持 `CloudStreamHandle` + onset/状态，spec §3.3）。
pub struct CloudPipelineEngine {
    vad: SileroVad,
    pre_roll_buffer: Vec<f32>,
    session: Option<CloudStreamHandle>,
    /// 已提交累积（镜像 `transcript.full` 的提交层；engine 无 transcript 访问，故自持）。
    /// **T3 接线硬约束**：`transcript.full` 除本 engine 的 `Committed` 经 `set_full` 覆盖外，
    /// 不被其他路径（如 `append_segment`）修改，否则镜像失同步会导致下次 commit 的逗号拼接
    /// 或全量覆盖出错。
    committed_text: String,
    current_partial: String,
    silence_duration: f64,
    is_speaking: bool,
    speech_confirm_count: u32,
    is_closing: bool,
    asr_engine: String,
    language: String,
    pause_polish_threshold_ms: f64,
}

impl CloudPipelineEngine {
    /// 构造。`vad` 由 coordinator 经 `find_silero_vad` + `vad_preroll` 预热后传入。
    /// `asr_engine`/`language`/`pause_polish_threshold_ms` 从 config 快照克隆（onset 时开 session / finish 刡定用）。
    pub fn new(
        vad: SileroVad,
        asr_engine: String,
        language: String,
        pause_polish_threshold_ms: f64,
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
        // 迁自 handle_cloud_streaming_tick:1654-1800 的 ASR 部分；产事件，不直接写 transcript/emit。

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
                // 连续 tick 确认：消除单次噪声脉冲误触发 onset
                if !self.is_speaking && !self.is_closing {
                    self.speech_confirm_count += 1;
                }
            } else {
                let chunk_duration = samples.len() as f64 / 16000.0;
                self.silence_duration += chunk_duration;
                // 静音重置确认计数（除非已在 speaking 状态）
                if !self.is_speaking && !self.is_closing {
                    self.speech_confirm_count = 0;
                }
            }
        }

        // 4. 无活跃 WSS + 连续 2 tick 确认 onset → 开 WSS + pre-roll + push
        //    连续 2 tick（~200ms）检测到语音才开 WSS，避免噪声脉冲浪费 API 调用
        if onset_confirmed(
            has_speech_now,
            self.is_speaking,
            self.is_closing,
            self.speech_confirm_count,
        ) {
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
        let events = drain_cloud_session(CloudDrainState {
            session: &mut self.session,
            committed_text: &mut self.committed_text,
            current_partial: &mut self.current_partial,
            is_closing: &mut self.is_closing,
            is_speaking: &mut self.is_speaking,
            language: &self.language,
        });
        //（drain_cloud_session 内部在 !is_closing && !is_speaking 时已 session.take()）

        // 6. 有活跃 WSS + 静音 ≥ threshold → 非阻塞 finish（Finish 由 close_async 最终发，此处只触发服务端收尾）
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

    fn finish(&mut self) -> TranscriptEvent {
        // tail 已由 stop 路径 tick 喂入 push_pcm；此处仅返回最后 current_partial 作 Committed 兜底。
        // cloud stop 路径不用其返回值（走 finalize_cloud / CloudClosing）。
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

#[cfg(test)]
mod tests {
    use super::*;
    use octopus_asr_cloud::{CloudStreamHandle, StreamEvent};

    /// 构造一个预载事件序列的 CloudStreamHandle（onset 后 drain 用）。
    fn handle_with_events(events: Vec<StreamEvent>) -> CloudStreamHandle {
        let (handle, result_tx) = CloudStreamHandle::new_for_test();
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
            language: "zh",
        });
        assert!(evs.is_empty()); // 预览不发事件
        assert_eq!(partial, "你好");
    }

    #[test]
    fn drain_finished_emits_committed_with_comma() {
        // 已提交 "第一句" + current_partial "第二句" → Finished → Committed("第一句，第二句")
        // 分两次 drain（drain 的 while 循环会一次清空所有已排队事件，故用 result_tx 跨调用分段投递）：
        //   先 Text 进 partial（is_speaking=true，不 take session），再 Finished 提交。
        let (handle, result_tx) = CloudStreamHandle::new_for_test();
        let mut session = Some(handle);
        let (mut committed, mut partial, mut is_closing, mut is_speaking) =
            ("第一句".to_string(), String::new(), false, true);
        let _ = result_tx.send(StreamEvent::Text("第二句".to_string()));
        let _ = drain_cloud_session(CloudDrainState {
            session: &mut session,
            committed_text: &mut committed,
            current_partial: &mut partial,
            is_closing: &mut is_closing,
            is_speaking: &mut is_speaking,
            language: "zh",
        });
        assert_eq!(partial, "第二句");
        let _ = result_tx.send(StreamEvent::Finished);
        let evs = drain_cloud_session(CloudDrainState {
            session: &mut session,
            committed_text: &mut committed,
            current_partial: &mut partial,
            is_closing: &mut is_closing,
            is_speaking: &mut is_speaking,
            language: "zh",
        });
        assert_eq!(evs, vec![TranscriptEvent::Committed("第一句，第二句".to_string())]);
        assert_eq!(committed, "第一句，第二句");
        assert_eq!(partial, ""); // 提交后清零
        assert!(!is_closing);
        assert!(!is_speaking);
        assert!(session.is_none()); // Finished → !is_closing && !is_speaking → take
    }

    #[test]
    fn drain_finished_no_double_comma_when_committed_ends_with_comma() {
        // committed 已以 '，' 结尾 + partial "第二句" → Finished → 不再加逗号 → "第一句，第二句"
        // 防回归：若误改成无条件 push_str(sep) 会得 "第一句，，第二句"
        let (handle, result_tx) = CloudStreamHandle::new_for_test();
        let mut session = Some(handle);
        let (mut committed, mut partial, mut is_closing, mut is_speaking) =
            ("第一句，".to_string(), String::new(), false, true);
        let _ = result_tx.send(StreamEvent::Text("第二句".to_string()));
        let _ = drain_cloud_session(CloudDrainState {
            session: &mut session,
            committed_text: &mut committed,
            current_partial: &mut partial,
            is_closing: &mut is_closing,
            is_speaking: &mut is_speaking,
            language: "zh",
        });
        assert_eq!(partial, "第二句");
        let _ = result_tx.send(StreamEvent::Finished);
        let evs = drain_cloud_session(CloudDrainState {
            session: &mut session,
            committed_text: &mut committed,
            current_partial: &mut partial,
            is_closing: &mut is_closing,
            is_speaking: &mut is_speaking,
            language: "zh",
        });
        assert_eq!(evs, vec![TranscriptEvent::Committed("第一句，第二句".to_string())]);
        assert_eq!(committed, "第一句，第二句"); // 不双逗号
    }

    #[test]
    fn drain_finished_no_partial_no_event_no_comma() {
        // current_partial 空 + Finished → 不 append、不发事件（与原 `if !current_partial.is_empty()` 一致）
        let mut session = Some(handle_with_events(vec![StreamEvent::Finished]));
        let (mut committed, mut partial, mut is_closing, mut is_speaking) =
            ("已有".to_string(), String::new(), false, true);
        let evs = drain_cloud_session(CloudDrainState {
            session: &mut session,
            committed_text: &mut committed,
            current_partial: &mut partial,
            is_closing: &mut is_closing,
            is_speaking: &mut is_speaking,
            language: "zh",
        });
        assert!(evs.is_empty());
        assert_eq!(committed, "已有"); // 不变
        assert!(session.is_none()); // Finished → !speaking → take
    }

    #[test]
    fn drain_failed_emits_error_clears_partial() {
        // 分两次 drain：先 Text 进 partial，再 Failed → Error + 清 partial
        let (handle, result_tx) = CloudStreamHandle::new_for_test();
        let mut session = Some(handle);
        let (mut committed, mut partial, mut is_closing, mut is_speaking) =
            (String::new(), String::new(), false, true);
        let _ = result_tx.send(StreamEvent::Text("抖动".to_string()));
        let _ = drain_cloud_session(CloudDrainState {
            session: &mut session,
            committed_text: &mut committed,
            current_partial: &mut partial,
            is_closing: &mut is_closing,
            is_speaking: &mut is_speaking,
            language: "zh",
        });
        assert_eq!(partial, "抖动");
        let _ = result_tx.send(StreamEvent::Failed("boom".to_string()));
        let evs = drain_cloud_session(CloudDrainState {
            session: &mut session,
            committed_text: &mut committed,
            current_partial: &mut partial,
            is_closing: &mut is_closing,
            is_speaking: &mut is_speaking,
            language: "zh",
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
        assert!(should_send_finish(true, false, 800.0, 700.0));   // speaking + 静音 800≥700
        assert!(!should_send_finish(false, false, 800.0, 700.0)); // 未 speaking
        assert!(!should_send_finish(true, true, 800.0, 700.0));   // 已 closing
        assert!(!should_send_finish(true, false, 600.0, 700.0));  // 静音不足
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
