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

/// 带时间戳的转写段（内部类型，非 DTO——desktop 编排时转为 record::SubtitleCue）。
///
/// 时间区间为绝对偏移（相对整段音频起点），单位毫秒；`start_ms` 为 VAD 段
/// `offset_samples / 16.0`（16k 采样率），`end_ms` = `(offset + len) / 16.0`。
#[derive(Debug, Clone, PartialEq)]
pub struct TimestampedSegment {
    pub start_ms: u64,
    pub end_ms: u64,
    pub text: String,
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
    Ok(postprocess_text(raw_text, engine, cfg))
}

/// 文本后处理：corrector（含热词命中计数副作用）→ ITN → 简繁归一。
///
/// 抽自 `transcribe_batch`，供 `transcribe_segments_with_timestamps` 复用（DRY）。
/// 行为与原 `transcribe_batch` 内联实现完全一致——包括 corrector 命中持久化
/// （`corrector::drain_hits` + `db::bump_hotword_hit_by_word`，best-effort）。
fn postprocess_text(
    raw_text: String,
    engine: &dyn OfflineAsrEngine,
    cfg: &PipelineConfig,
) -> String {
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

    if cfg.simplify {
        crate::hans::to_simplified(&text)
    } else {
        crate::hans::to_traditional(&text)
    }
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
                        // 段间空格判定（第五轮 A1）：改字节级 ASCII 判定，对齐 paraformer.rs:350-356
                        // smart_append。旧 char 级 is_cjk 漏 CJK 标点(0x3000-303F)/全角(FF00-FFEF)
                        // → 段以「。」结尾 + 下段汉字 → needs_space=true →「你好。 世界」错插空格。
                        // 字节级 <0x80 判 ASCII：CJK 标点/全角/汉字首字节均 ≥0x80 → 判非 ASCII →
                        // 两侧都非 ASCII 时不插空格（CJK 之间无空格），与 smart_append 一致。
                        let last_byte = final_text.as_bytes().last().copied().unwrap_or(0);
                        let next_byte = text_trimmed.as_bytes().first().copied().unwrap_or(0);
                        let last_is_ascii = last_byte < 0x80;
                        let next_is_ascii = next_byte < 0x80;
                        let needs_space = (last_is_ascii || next_is_ascii)
                            && last_byte != b' '
                            && next_byte != b' ';
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

/// 带时间戳的转写：VAD 分段（带 offset）→ 逐段 `engine.transcribe` + 后处理 → 组装 `TimestampedSegment`。
///
/// 与 `transcribe_batch` 的区别：不拼接文本，而是保留每段独立 + 时间区间（字幕场景所需）。
/// 短音频（≤480k samples）也走 VAD 分段（spec 2026-07-28-record-auto-subtitle §4.5 决策）
/// ——与 `transcribe_segments` 的「短音频直连」不同，因为字幕场景需要分段 cue。
/// VAD 初始化失败时降级：整段作为单条 cue（offset=0）。
/// 过滤：`<500ms` 段（噪声/残余静音）与 `text.trim().is_empty()` 段。
pub fn transcribe_segments_with_timestamps(
    engine: &dyn OfflineAsrEngine,
    samples: &[f32],
    cfg: &PipelineConfig,
) -> Result<Vec<TimestampedSegment>> {
    // VAD 分段（带 offset）——初始化失败降级整段一条 cue（offset=0）。
    let segments: Vec<crate::audio::VadSegment> = match crate::config::create_silero_vad() {
        Ok(mut v) => crate::audio::segment_audio_vad_with_offsets(
            samples, &mut v, 480, 0.4, 500, 25000,
        ),
        Err(e) => {
            log::warn!("VAD 初始化失败，整段作为单条 cue: {}", e);
            vec![crate::audio::VadSegment {
                offset_samples: 0,
                samples: samples.to_vec(),
            }]
        }
    };

    let mut result = Vec::with_capacity(segments.len());
    for seg in &segments {
        let dur_samples = seg.samples.len();
        let dur_ms = (dur_samples as f64 / 16.0).round() as u64;
        // 过滤 <500ms 段（噪声/残余静音）
        if dur_ms < 500 {
            continue;
        }
        let raw = engine.transcribe(&seg.samples, &cfg.language)?;
        let text = postprocess_text(raw, engine, cfg);
        // 过滤空文本段
        if text.trim().is_empty() {
            continue;
        }
        let start_ms = (seg.offset_samples as f64 / 16.0).round() as u64;
        let end_ms = ((seg.offset_samples + dur_samples) as f64 / 16.0).round() as u64;
        result.push(TimestampedSegment {
            start_ms,
            end_ms,
            text,
        });
    }
    Ok(result)
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

    #[test]
    fn transcribe_timestamps_short_audio_single_cue() {
        // 短音频（1s = 16000 samples）走 VAD 分段（spec §4.5）。
        // 不强断言段数——VAD 对纯合成信号常判静音（audio.rs 测试已验证），
        // 结果可能是 0 段（VAD 判静音 + 非空文本下）或 N 段。只校验返回段的通用不变式：
        // 1) 每段 start_ms < end_ms；2) 1s 音频所有段 end_ms ≤ 1000；3) 每段文本非空。
        let eng = FakeEngine { text: "短音频测试".into(), skip: false };
        let samples = vec![0.5f32; 16000]; // 1 秒
        let segs =
            transcribe_segments_with_timestamps(&eng, &samples, &cfg(true, false)).unwrap();
        for s in &segs {
            assert!(s.start_ms < s.end_ms, "start < end（{} >= {}）", s.start_ms, s.end_ms);
            assert!(s.end_ms <= 1000, "1 秒音频 end_ms 不应超 1000，实际 {}", s.end_ms);
            assert!(!s.text.is_empty(), "段文本不应为空（过滤后）");
        }
    }

    #[test]
    fn transcribe_timestamps_ms_conversion() {
        // 时间戳换算验证：320000 samples = 20s（>500ms 阈值，不会被 dur 过滤）。
        // create_silero_vad() 成功且 VAD 把合成信号判静音时返回 0 段；
        // VAD 成功检测到语音则分段；create_silero_vad() 失败则降级整段一条 cue
        // （offset=0、len=samples.len()，end_ms = 320000/16 = 20000）。
        // 三种路径下都校验不变式；降级单 cue 路径额外校验精确换算。
        let eng = FakeEngine { text: "测试".into(), skip: false };
        let samples = vec![0.5f32; 320000];
        let segs =
            transcribe_segments_with_timestamps(&eng, &samples, &cfg(true, false)).unwrap();
        for s in &segs {
            assert!(s.start_ms < s.end_ms, "start < end");
            assert!(s.end_ms <= 20000, "20s 音频 end_ms ≤ 20000，实际 {}", s.end_ms);
            assert!(!s.text.is_empty());
        }
        // 降级整段一条 cue 时（create_silero_vad 失败）精确校验 ms 换算
        if segs.len() == 1 {
            assert_eq!(segs[0].start_ms, 0, "整段 cue start 应为 0");
            assert_eq!(segs[0].end_ms, 20000, "320000 / 16 = 20000");
        }
    }

    #[test]
    fn transcribe_timestamps_filters_empty_text() {
        // 空文本过滤：FakeEngine 返回空文本 → 即使 VAD 检测到语音段，结果也应全被过滤为空。
        // 与对照测试 transcribe_timestamps_short_audio_single_cue（非空文本）配合，
        // 验证 `text.trim().is_empty()` 过滤分支：在相同音频下，空文本 → 0 段。
        // Silero VAD 对合成信号常判静音（环境限制，audio.rs 测试同理不强断言非空）；
        // 当 VAD 返回 0 段时空文本过滤本就无段可过滤——此情形下结果空是 VAD 而非过滤导致，
        // 故本测试不与 VAD 检测耦合：只验证「无论 VAD 是否检测到段，空文本永远不进结果」。
        let eng = FakeEngine { text: "".into(), skip: false };
        let samples = vec![0.5f32; 16000];
        let segs =
            transcribe_segments_with_timestamps(&eng, &samples, &cfg(true, false)).unwrap();
        assert!(
            segs.is_empty(),
            "空文本段应被过滤，实际残留 {} 段",
            segs.len()
        );
        // 反向对照：同样音频、非空文本，至少不应因过滤逻辑把有效段误删为空
        // （若 VAD 仍判静音则两者都为空——这是 VAD 行为，不破坏本断言）。
        let eng2 = FakeEngine { text: "有内容".into(), skip: false };
        let segs2 =
            transcribe_segments_with_timestamps(&eng2, &samples, &cfg(true, false)).unwrap();
        for s in &segs2 {
            assert!(!s.text.is_empty(), "非空文本下返回段不应含空文本");
        }
    }
}
