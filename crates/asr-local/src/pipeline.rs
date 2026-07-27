//! ASR pipeline 编排：批处理 helper（流式 helper / StreamingRunner 见后续阶段）。
//!
//! `transcribe_batch` 收编原 `engine::transcribe_with_vad` 的 VAD 分段编排，把纠错
//! （`correct`）与简繁归一化（`simplify`）从「读全局 app_config」参数化为 `PipelineConfig`
//! 字段，使编排可被多端（cli/desktop/server）以明确参数复用，而非隐式依赖全局配置。
//!
//! 设计详见 `docs/superpowers/specs/2026-06-23-asr-pipeline-design.md`。

use crate::config::load_app_config_cached;
use crate::engine::OfflineAsrEngine;
use anyhow::Result;

/// 批处理 pipeline 配置。
///
/// 阶段1 精简版：`correct` / `simplify` 在 `transcribe_batch` 内替代原 `transcribe_with_vad`
/// 对全局 `app_config` 的读取；`ngram` 为预留字段（解码纠错，尚未实现）。流式相关字段
/// （`backend` / `denoise` / 音频源）随阶段2 流式 helper 加入。
pub struct PipelineConfig {
    pub language: String,
    /// 是否对 ASR 输出做拼音/bigram 纠错（原 `app_config.asr_correct`）。
    pub correct: bool,
    /// true→输出简体，false→输出繁体（原 `app_config.output_simplified`）。
    pub simplify: bool,
    /// ngram 解码纠错开关（预留，尚未实现；`transcribe_batch` 见到 true 仅 warn）。
    pub ngram: bool,
}

impl PipelineConfig {
    /// 从全局 `app_config` 构造（向后兼容 `transcribe_with_vad` / desktop 既有行为）。
    pub fn from_app_config(language: &str) -> Self {
        let app = load_app_config_cached();
        Self {
            language: language.to_string(),
            correct: app.asr_correct,
            simplify: app.output_simplified,
            ngram: false,
        }
    }
}

/// 批处理转写：VAD 分段 → 逐段 `engine.transcribe` → 连接 → 纠错 → 简繁归一化。
///
/// 收编自原 `engine::transcribe_with_vad`；纠错/简繁改由 `cfg` 控制（不读全局 config），
/// 使 cli/server 能以明确参数复用同一编排。短音频（≤480k samples = 30s）跳过 VAD 直连引擎。
/// ngram 解码尚未实现（`cfg.ngram=true` 时仅 warn，不改变行为）——预留接入点。
pub fn transcribe_batch(
    engine: &dyn OfflineAsrEngine,
    samples: &[f32],
    cfg: &PipelineConfig,
) -> Result<String> {
    if cfg.ngram {
        log::warn!("ngram 解码尚未实现，忽略 cfg.ngram 开关");
    }

    let raw_text = transcribe_segments(engine, samples, &cfg.language)?;

    let is_english = cfg.language.eq_ignore_ascii_case("en");
    let text = if cfg.correct && !engine.skip_corrector() && !is_english {
        let corrected = crate::corrector::get_corrector().correct(&raw_text);
        // 热词命中计数（best-effort：失败仅 warn，不阻断纠错）。
        // corrector 收集命中、pipeline 持久化——分层避免 corrector 单测污染真实 DB。
        for word in crate::corrector::drain_hits() {
            if let Err(e) = crate::db::bump_hotword_hit_by_word(&word) {
                log::warn!("[hotword] bump 命中计数失败 '{}': {}", word, e);
            }
        }
        corrected
    } else {
        raw_text
    };

    // ITN：中文数字→阿拉伯数字（corrector 后、hans 前，spec 2026-07-27-asr-itn-design §2）
    let text = crate::itn::normalize(&text);

    Ok(if cfg.simplify {
        crate::hans::to_simplified(&text)
    } else {
        crate::hans::to_traditional(&text)
    })
}

/// VAD 分段转写：短音频直连；长音频用 Silero VAD 切片后逐段转写，并按 CJK/非 CJK 规则连接。
/// VAD 不可用时降级为整段转写。搬自原 `engine::transcribe_with_vad` 的分段主体（逻辑不变）。
fn transcribe_segments(
    engine: &dyn OfflineAsrEngine,
    samples: &[f32],
    language: &str,
) -> Result<String> {
    if samples.len() <= 480_000 {
        return engine.transcribe(samples, language);
    }

    let vad = match crate::config::create_silero_vad() {
        Ok(v) => Some(v),
        Err(e) => {
            log::warn!(
                "Failed to initialize Silero VAD, falling back to full audio transcription: {}", e);
            None
        }
    };

    if let Some(mut v) = vad {
        let total_secs = samples.len() as f64 / 16000.0;
        log::info!("[ASR] Long audio detected ({:.2}s). Segmenting audio using VAD...", total_secs);
        let segments = crate::audio::segment_audio_vad(samples, &mut v, 480, 0.4, 500, 25000);
        log::info!("[ASR] Audio segmented into {} speech chunks.", segments.len());

        let mut final_text = String::new();
        for (idx, seg) in segments.iter().enumerate() {
            if !seg.is_empty() {
                let seg_secs = seg.len() as f64 / 16000.0;
                log::debug!(
                    "[ASR] Transcribing segment {}/{} ({:.2}s)...", idx + 1, segments.len(), seg_secs);
                let text = engine.transcribe(seg, language)?;
                let text_cleaned = text.replace("<|nospeech|>", "");
                let text_trimmed = text_cleaned.trim();
                if !text_trimmed.is_empty() {
                    if !final_text.is_empty() {
                        let last_char = final_text.chars().last();
                        let next_char = text_trimmed.chars().next();
                        let needs_space = match (last_char, next_char) {
                            (Some(lc), Some(nc)) => {
                                let is_cjk = |c: char| {
                                    let u = c as u32;
                                    (0x4E00..=0x9FFF).contains(&u) // CJK Unified Ideographs
                                        || (0x3040..=0x309F).contains(&u) // Hiragana
                                        || (0x30A0..=0x30FF).contains(&u) // Katakana
                                        || (0xAC00..=0xD7AF).contains(&u)  // Hangul
                                };
                                !is_cjk(lc) || !is_cjk(nc)
                            }
                            _ => true,
                        };
                        if needs_space {
                            final_text.push(' ');
                        }
                    }
                    final_text.push_str(text_trimmed);
                }
            }
        }
        Ok(final_text)
    } else {
        engine.transcribe(samples, language)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::Result;

    struct FakeEngine {
        text: String,
        skip: bool,
    }
    impl OfflineAsrEngine for FakeEngine {
        fn transcribe(&self, _samples: &[f32], _language: &str) -> Result<String> {
            Ok(self.text.clone())
        }
        fn skip_corrector(&self) -> bool {
            self.skip
        }
    }

    fn cfg(simplify: bool, correct: bool) -> PipelineConfig {
        PipelineConfig {
            language: "zh".into(),
            correct,
            simplify,
            ngram: false,
        }
    }

    #[test]
    fn batch_simplify_on_converts_traditional() {
        let eng = FakeEngine { text: "語言識別".into(), skip: false };
        let out = transcribe_batch(&eng, &[], &cfg(true, false)).unwrap();
        assert_eq!(out, "语言识别");
    }

    #[test]
    fn batch_simplify_off_keeps_traditional() {
        let eng = FakeEngine { text: "语言".into(), skip: false };
        let out = transcribe_batch(&eng, &[], &cfg(false, false)).unwrap();
        assert_eq!(out, "語言");
    }

    #[test]
    fn batch_ngram_flag_does_not_panic() {
        let eng = FakeEngine { text: "你好".into(), skip: false };
        let mut c = cfg(true, false);
        c.ngram = true;
        let out = transcribe_batch(&eng, &[], &c).unwrap();
        assert_eq!(out, "你好");
    }

    #[test]
    fn batch_short_audio_calls_engine_directly() {
        // ≤480k samples 走直连，不经 VAD（FakeEngine 不依赖真实模型即可验证路径）
        let eng = FakeEngine { text: "短音频".into(), skip: false };
        let samples = vec![0.0f32; 1000];
        let out = transcribe_batch(&eng, &samples, &cfg(true, false)).unwrap();
        assert_eq!(out, "短音频");
    }
}
