use anyhow::{Context, Result};
use parking_lot::Mutex;

use crate::sentence_separator;

/// 统一的流式 ASR 引擎包装。
///
/// 对外统一返回**累积全文**语义：
/// - Zipformer CTC / Transducer 都返回当前段文本，由上层 accumulated 拼接
pub enum StreamingSession {
    Paraformer {
        engine: Mutex<crate::streaming_paraformer::StreamingParaformer>,
        /// 已提交的 ASR 文本 + 分隔符（静音点冻结的历史段）
        punct_prefix: Mutex<String>,
        /// prefix 中已提交的 ASR 文本字符数（不含标点）
        committed_chars: Mutex<usize>,
        /// 段间分隔符（按 language 选择：英文空格 / 其他中文逗号）
        separator: &'static str,
    },
    ZipformerCtc {
        engine: Mutex<crate::streaming_zipformer::StreamingZipformer>,
        accumulated: Mutex<String>,
        separator: &'static str,
    },
    ZipformerTransducer {
        engine: Mutex<crate::streaming_zipformer::StreamingZipformerTransducer>,
        accumulated: Mutex<String>,
        separator: &'static str,
    },
}

impl StreamingSession {
    /// 根据引擎 spec 创建流式 session。
    ///
    /// `language` 决定段间分隔符（英文空格 / 其他中文逗号，见 [`sentence_separator`]）。
    ///
    /// 使用 `resolve_active_engine`（带兜底）而非 `resolve_engine_category`（无兜底），
    /// 与 `is_streaming_engine` 的判定对称——否则 DB 未命中时 `is_streaming_engine` 兜底成功
    /// （返回 true → 进 streaming 路径），但此处无兜底失败 → streaming session 创建失败。
    pub fn new(engine_spec: &str, language: &str) -> Result<Self> {
        let resolved = crate::config::resolve_active_engine(engine_spec)
            .context(format!("Failed to resolve streaming engine: {}", engine_spec))?;
        let category = resolved.category;
        let bare_name = resolved.name.as_str();
        let separator = sentence_separator(language);

        match category {
            crate::config::EngineCategory::Paraformer => {
                let engine = crate::streaming_paraformer::StreamingParaformer::new(bare_name)?;
                Ok(Self::Paraformer {
                    engine: Mutex::new(engine),
                    punct_prefix: Mutex::new(String::new()),
                    committed_chars: Mutex::new(0),
                    separator,
                })
            }
            crate::config::EngineCategory::Zipformer => {
                // 检测有无 decoder.onnx：有则为 Transducer（RNN-T），无则为 CTC
                // 直接传已解析的 entry，避免流式引擎内部再次查 DB 取到错误条目
                let hf_path = crate::config::resolve_model_dir(&resolved.entry.source)
                    .context("Failed to resolve model dir for streaming Zipformer")?;
                if hf_path.join("decoder.onnx").exists() {
                    let engine = crate::streaming_zipformer::StreamingZipformerTransducer::new_from_entry(&resolved.entry)?;
                    Ok(Self::ZipformerTransducer {
                        engine: Mutex::new(engine),
                        accumulated: Mutex::new(String::new()),
                        separator,
                    })
                } else {
                    let engine = crate::streaming_zipformer::StreamingZipformer::new_from_entry(&resolved.entry)?;
                    Ok(Self::ZipformerCtc {
                        engine: Mutex::new(engine),
                        accumulated: Mutex::new(String::new()),
                        separator,
                    })
                }
            }
            other => {
                anyhow::bail!(
                    "Engine '{}' ({:?}) does not support streaming. Only Paraformer and Zipformer are supported.",
                    engine_spec, other
                )
            }
        }
    }

    /// 送入音频样本（16kHz mono f32），返回累积识别文本（如果有新结果）。
    /// - `was_silent`：上一轮静音≥阈值（由调用方 VAD 判定）。
    /// - `has_speech`：本轮 VAD 判定有语音。仅 zipformer 用：`was_silent && !has_speech` 时 finish+reset
    ///   （持续静音=真段边界）；`was_silent && has_speech`（静音→语音过渡）不 reset，避免开口瞬间
    ///   反复冲刷冲掉首字音头（首字缺失根因）。paraformer 忽略（流式不 reset，标点仅靠 was_silent）。
    pub fn accept_samples(&self, samples: &[f32], was_silent: bool, has_speech: bool) -> Result<Option<String>> {
        if samples.is_empty() {
            return Ok(None);
        }

        match self {
            Self::Paraformer { engine, punct_prefix, committed_chars, separator } => {
                let mut eng = engine.lock();
                match eng.accept_samples(samples)? {
                    Some(full_asr) => {
                        // full_asr 是本次话语的完整 ASR 文本（跨 chunk 累积解码）
                        let mut prefix = punct_prefix.lock();
                        let mut clen = committed_chars.lock();

                        if was_silent && !prefix.is_empty() && !ends_with_punct(&prefix) {
                            // 静音恢复 → 提交当前 delta + 分隔符
                            let delta: String = full_asr.chars().skip(*clen).collect();
                            let delta = delta.trim_start();
                            if !delta.is_empty() {
                                crate::paraformer::smart_append(&mut prefix, delta);
                                *clen = full_asr.chars().count();
                                // 只有确实识别出新文本才插分隔符，避免静音波动产生多余标点
                                if !ends_with_punct(&prefix) {
                                    prefix.push_str(separator);
                                }
                            }
                        }

                        // 展示文本 = prefix + 未提交的 delta
                        let delta: String = full_asr.chars().skip(*clen).collect();
                        let mut display = prefix.clone();
                        if !delta.trim_start().is_empty() {
                            crate::paraformer::smart_append(&mut display, delta.trim_start());
                        }
                        Ok(Some(display))
                    }
                    None => Ok(None),
                }
            }
            Self::ZipformerCtc { engine, accumulated, separator } => {
                let mut eng = engine.lock();
                if was_silent && !has_speech {
                    let segment_text = eng.finish()?;
                    let trimmed = segment_text.trim();
                    if !trimmed.is_empty() {
                        let mut acc = accumulated.lock();
                        if !acc.is_empty() {
                            acc.push_str(separator);
                        }
                        acc.push_str(trimmed);
                    }
                    eng.reset();
                }
                zipformer_accept(&mut *eng, accumulated, separator, samples)
            }
            Self::ZipformerTransducer { engine, accumulated, separator } => {
                let mut eng = engine.lock();
                if was_silent && !has_speech {
                    let segment_text = eng.finish()?;
                    let trimmed = segment_text.trim();
                    if !trimmed.is_empty() {
                        let mut acc = accumulated.lock();
                        if !acc.is_empty() {
                            acc.push_str(separator);
                        }
                        acc.push_str(trimmed);
                    }
                    eng.reset();
                }
                zipformer_accept(&mut *eng, accumulated, separator, samples)
            }
        }
    }

    /// 主动冲刷剩余音频（不重置状态，用于静音期间强制吐字）。
    /// `insert_comma` 为 true 时，在冲刷出的文本末尾追加分隔符（静音停顿产生分句）。
    pub fn flush(&self, insert_comma: bool) -> Result<Option<String>> {
        match self {
            Self::Paraformer { engine, punct_prefix, committed_chars, separator } => {
                let mut eng = engine.lock();
                match eng.flush()? {
                    Some(full_asr) => {
                        let mut prefix = punct_prefix.lock();
                        let mut clen = committed_chars.lock();

                        let delta: String = full_asr.chars().skip(*clen).collect();
                        let delta_trimmed = delta.trim_start();

                        if insert_comma {
                            // 提交当前 delta + 分隔符
                            if !delta_trimmed.is_empty() {
                                crate::paraformer::smart_append(&mut prefix, delta_trimmed);
                            }
                            *clen = full_asr.chars().count();
                            if !prefix.is_empty() && !ends_with_punct(&prefix) {
                                prefix.push_str(separator);
                            }
                            Ok(Some(prefix.clone()))
                        } else {
                            let mut display = prefix.clone();
                            if !delta_trimmed.is_empty() {
                                crate::paraformer::smart_append(&mut display, delta_trimmed);
                            }
                            Ok(Some(display))
                        }
                    }
                    None => {
                        if insert_comma {
                            let mut prefix = punct_prefix.lock();
                            if !prefix.is_empty() && !ends_with_punct(&prefix) {
                                prefix.push_str(separator);
                                return Ok(Some(prefix.clone()));
                            }
                        }
                        Ok(None)
                    }
                }
            }
            Self::ZipformerCtc { engine, accumulated, separator } => {
                let mut eng = engine.lock();
                zipformer_flush(&mut *eng, accumulated, separator)
            }
            Self::ZipformerTransducer { engine, accumulated, separator } => {
                let mut eng = engine.lock();
                zipformer_flush(&mut *eng, accumulated, separator)
            }
        }
    }

    /// 冲刷剩余音频，返回最终累积文本。
    /// 在末尾追加句号（如果文本不为空且不以标点结尾）。
    pub fn finish(&self) -> Result<String> {
        match self {
            Self::Paraformer { engine, punct_prefix, committed_chars, .. } => {
                let mut eng = engine.lock();
                let full_asr = eng.finish()?;
                let prefix = punct_prefix.lock();
                let clen = committed_chars.lock();

                let delta: String = full_asr.chars().skip(*clen).collect();
                let delta_trimmed = delta.trim_start();
                let mut display = prefix.clone();
                if !delta_trimmed.is_empty() {
                    crate::paraformer::smart_append(&mut display, delta_trimmed);
                }
                append_final_punctuation(&mut display);
                Ok(crate::hans::normalize_variant(&display))
            }
            Self::ZipformerCtc { engine, accumulated, separator } => {
                let final_segment = engine.lock().finish()?;
                let trimmed = final_segment.trim();
                let mut acc = accumulated.lock();
                if !trimmed.is_empty() {
                    if !acc.is_empty() {
                        acc.push_str(separator);
                    }
                    acc.push_str(trimmed);
                }
                append_final_punctuation(&mut acc);
                Ok(crate::hans::normalize_variant(&acc))
            }
            Self::ZipformerTransducer { engine, accumulated, separator } => {
                let final_segment = engine.lock().finish()?;
                let trimmed = final_segment.trim();
                let mut acc = accumulated.lock();
                if !trimmed.is_empty() {
                    if !acc.is_empty() {
                        acc.push_str(separator);
                    }
                    acc.push_str(trimmed);
                }
                append_final_punctuation(&mut acc);
                Ok(crate::hans::normalize_variant(&acc))
            }
        }
    }

    /// 重置引擎状态，准备新的识别轮次（不重新加载模型）。
    pub fn reset(&self) {
        match self {
            Self::Paraformer { engine, punct_prefix, committed_chars, .. } => {
                engine.lock().reset();
                punct_prefix.lock().clear();
                *committed_chars.lock() = 0;
            }
            Self::ZipformerCtc { engine, accumulated, .. } => {
                engine.lock().reset();
                accumulated.lock().clear();
            }
            Self::ZipformerTransducer { engine, accumulated, .. } => {
                engine.lock().reset();
                accumulated.lock().clear();
            }
        }
    }
}

// ── Zipformer 共用流式逻辑（CTC 和 Transducer 方法签名相同）──

/// Zipformer accept_samples 后的标准处理：拿当前段文本，与 accumulated 拼接。
fn zipformer_accept<E: ZipformerStreamOps>(
    eng: &mut E,
    accumulated: &Mutex<String>,
    separator: &str,
    samples: &[f32],
) -> Result<Option<String>> {
    match eng.accept_samples(samples)? {
        Some(current_segment) => {
            let trimmed_segment = current_segment.trim();
            let acc = accumulated.lock();
            if acc.is_empty() {
                Ok(Some(trimmed_segment.to_string()))
            } else if trimmed_segment.is_empty() {
                Ok(Some(acc.clone()))
            } else {
                Ok(Some(format!("{}{}{}", *acc, separator, trimmed_segment)))
            }
        }
        None => {
            let acc = accumulated.lock();
            if acc.is_empty() {
                Ok(None)
            } else {
                Ok(Some(acc.clone()))
            }
        }
    }
}

/// Zipformer flush 后的标准处理。
fn zipformer_flush<E: ZipformerStreamOps>(
    eng: &mut E,
    accumulated: &Mutex<String>,
    separator: &str,
) -> Result<Option<String>> {
    match eng.flush()? {
        Some(current_segment) => {
            let trimmed_segment = current_segment.trim();
            let acc = accumulated.lock();
            if acc.is_empty() {
                Ok(Some(trimmed_segment.to_string()))
            } else if trimmed_segment.is_empty() {
                Ok(Some(acc.clone()))
            } else {
                Ok(Some(format!("{}{}{}", *acc, separator, trimmed_segment)))
            }
        }
        None => {
            let acc = accumulated.lock();
            if acc.is_empty() {
                Ok(None)
            } else {
                Ok(Some(acc.clone()))
            }
        }
    }
}

/// Zipformer 流式引擎共用接口（CTC 和 Transducer 方法签名完全相同）。
trait ZipformerStreamOps {
    fn accept_samples(&mut self, samples: &[f32]) -> Result<Option<String>>;
    fn flush(&mut self) -> Result<Option<String>>;
    #[allow(dead_code)]
    fn finish(&mut self) -> Result<String>;
    #[allow(dead_code)]
    fn reset(&mut self);
}

impl ZipformerStreamOps for crate::streaming_zipformer::StreamingZipformer {
    fn accept_samples(&mut self, samples: &[f32]) -> Result<Option<String>> {
        self.accept_samples(samples)
    }
    fn flush(&mut self) -> Result<Option<String>> {
        self.flush()
    }
    fn finish(&mut self) -> Result<String> {
        self.finish()
    }
    fn reset(&mut self) {
        self.reset()
    }
}

impl ZipformerStreamOps for crate::streaming_zipformer::StreamingZipformerTransducer {
    fn accept_samples(&mut self, samples: &[f32]) -> Result<Option<String>> {
        self.accept_samples(samples)
    }
    fn flush(&mut self) -> Result<Option<String>> {
        self.flush()
    }
    fn finish(&mut self) -> Result<String> {
        self.finish()
    }
    fn reset(&mut self) {
        self.reset()
    }
}

/// 在文本末尾追加句号（如果文本不为空且不以标点结尾）
fn append_final_punctuation(text: &mut String) {
    if text.is_empty() {
        return;
    }
    if !ends_with_punct(text) {
        text.push('。');
    }
}

/// 检查文本是否以标点结尾
fn ends_with_punct(text: &str) -> bool {
    match text.chars().last() {
        Some(last) => {
            let punctuation = ['，', '。', '！', '？', '；', '：', '、', '.', '!', '?', ';', ','];
            punctuation.contains(&last)
        }
        None => false,
    }
}
