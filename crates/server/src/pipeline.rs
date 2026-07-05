//! server 流式 pipeline：WS↔asr `StreamingRunner` 桥接 + `TranscriptEvent`→JSON 序列化。
//!
//! 薄包 [`StreamingRunner`]（VAD 静音 + 标点 + accept/flush/finish + 纠错已收编）。
//! 不含 polish / denoise（总 spec §3.8/§3.6：留端，server 不依赖 llm/cpal）。

use anyhow::Result;
use std::sync::Arc;
use octopus_asr_local::streaming_runner::{StreamingEngine, StreamingRunner, TranscriptEvent};

/// WS 流式会话：薄包 asr `StreamingRunner`。
pub struct WsStreamSession {
    runner: StreamingRunner,
}

impl WsStreamSession {
    /// 由已构造的流式引擎装箱传入（解耦 `StreamingSession`，便于测试注入 fake）。
    /// `correct` 来自 `app_config.asr_correct`（与批处理 `PipelineConfig.correct` 同源）。
    pub fn new(engine: Arc<dyn StreamingEngine>, correct: bool) -> Result<Self> {
        Ok(Self {
            runner: StreamingRunner::new(engine, correct)?,
        })
    }

    /// 测试辅助：不加载 VAD（跳过门控），验证纯 relay 管线。
    #[cfg(test)]
    pub fn new_no_vad(engine: Arc<dyn StreamingEngine>, correct: bool) -> Result<Self> {
        Ok(Self {
            runner: StreamingRunner::new_no_vad(engine, correct)?,
        })
    }

    /// 喂一帧已降噪 16k 样本，返回本帧事件流（0..n 个 TranscriptEvent）。
    pub fn feed(&mut self, samples_16k: &[f32]) -> Vec<TranscriptEvent> {
        self.runner.push_samples(samples_16k)
    }

    /// 收尾：runner.finish() → Final（追加句号 + 简繁归一）。
    pub fn finish(&mut self) -> TranscriptEvent {
        self.runner.finish()
    }

    /// 重置（会话间复用前调用）。
    pub fn reset(&mut self) {
        self.runner.reset()
    }
}

/// `TranscriptEvent` → server 私有 WS JSON（统一 `{type,text}`）。
///
/// `TranscriptEvent` 无 Serialize（仅 Debug/Clone），为不污染 asr crate
/// （总 spec §3.1：asr = 零件库 + 端做桥接），server 端 match 序列化。
pub fn event_to_json(ev: &TranscriptEvent) -> String {
    let (ty, text) = match ev {
        TranscriptEvent::Partial(t) => ("partial", t),
        TranscriptEvent::Committed(t) => ("committed", t),
        TranscriptEvent::Final(t) => ("final", t),
        TranscriptEvent::Error(t) => ("error", t),
    };
    // 用 serde_json 安全转义所有控制字符（含 \t \r 等）
    let escaped = serde_json::Value::String(text.to_string()).to_string();
    // escaped 形如 "text"（含引号），直接插入 JSON
    format!(r#"{{"type":"{}","text":{}}}"#, ty, escaped)
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

    #[test]
    fn event_to_json_escapes_control_chars() {
        // ASR 输出含 \t \r 等控制字符，必须产生合法 JSON
        let ev = TranscriptEvent::Partial("hello\tworld\r\n".into());
        let json = event_to_json(&ev);
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed["type"], "partial");
        assert_eq!(parsed["text"], "hello\tworld\r\n");
    }

    /// 可编程 fake：第一次 accept 返 Some，之后 None；finish 返固定串。
    struct FakeEngine {
        next_accept: Mutex<Option<String>>,
        finish_text: String,
    }
    impl StreamingEngine for FakeEngine {
        fn accept_samples(&self, _samples: &[f32], _was_silent: bool, _has_speech: bool) -> Result<Option<String>> {
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
        let mut s = WsStreamSession::new_no_vad(Arc::new(engine), false).unwrap();
        // 无 VAD（vad=None）→ detect_silence_gap 返回 (false,false,false)，不门控不冲刷，
        // 只走 accept_samples → Partial。VAD 门控行为由 asr-local 自身测试覆盖。
        assert_eq!(
            s.feed(&[0.0_f32; 512]),
            vec![TranscriptEvent::Partial("hi".into())]
        );
        // accept 已 take → 第二次 None → 空事件。
        assert!(s.feed(&[0.0_f32; 512]).is_empty());
        assert_eq!(s.finish(), TranscriptEvent::Final("final".into()));
    }
}
