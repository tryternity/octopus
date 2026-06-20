use anyhow::{Context, Result};
use std::sync::Mutex;

/// 统一的流式 ASR 引擎包装。
///
/// 对外统一返回**累积全文**语义：
/// - Zipformer CTC / Transducer 都返回当前段文本，由上层 accumulated 拼接
pub enum StreamingSession {
    Paraformer {
        engine: Mutex<crate::streaming_paraformer::StreamingParaformer>,
        accumulated: Mutex<String>,
    },
    ZipformerCtc {
        engine: Mutex<crate::streaming_zipformer::StreamingZipformer>,
        accumulated: Mutex<String>,
    },
    ZipformerTransducer {
        engine: Mutex<crate::streaming_zipformer::StreamingZipformerTransducer>,
        accumulated: Mutex<String>,
    },
}

impl StreamingSession {
    /// 根据引擎 spec 创建流式 session。
    ///
    /// 使用 `resolve_active_engine`（带兜底）而非 `resolve_engine_category`（无兜底），
    /// 与 `is_streaming_engine` 的判定对称——否则 DB 未命中时 `is_streaming_engine` 兜底成功
    /// （返回 true → 进 streaming 路径），但此处无兜底失败 → streaming session 创建失败。
    pub fn new(engine_spec: &str) -> Result<Self> {
        let resolved = crate::config::resolve_active_engine(engine_spec)
            .context(format!("Failed to resolve streaming engine: {}", engine_spec))?;
        let category = resolved.category;
        let bare_name = resolved.name.as_str();

        match category {
            crate::config::EngineCategory::Paraformer => {
                let engine = crate::streaming_paraformer::StreamingParaformer::new(bare_name)?;
                Ok(Self::Paraformer {
                    engine: Mutex::new(engine),
                    accumulated: Mutex::new(String::new()),
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
                    })
                } else {
                    let engine = crate::streaming_zipformer::StreamingZipformer::new_from_entry(&resolved.entry)?;
                    Ok(Self::ZipformerCtc {
                        engine: Mutex::new(engine),
                        accumulated: Mutex::new(String::new()),
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
    /// `was_silent` 表示上一轮音频是否为静默（由调用方根据采样量判断），
    /// 如果上一轮静默、本轮有新文本，则在文本前插入逗号。
    pub fn accept_samples(&self, samples: &[f32], was_silent: bool) -> Result<Option<String>> {
        if samples.is_empty() {
            return Ok(None);
        }

        match self {
            Self::Paraformer { engine, accumulated } => {
                let mut eng = engine.lock().unwrap();
                match eng.accept_samples(samples)? {
                    Some(delta) => {
                        let mut acc = accumulated.lock().unwrap();
                        // 不在此处插逗号——Paraformer flush 挤出的尾音与之前文本
                        // 属于同一句话（如"现"+"在"），插逗号会断词（"现，在"）。
                        // 段间标点由 coordinator 的 finish 拼接处理。
                        acc.push_str(&delta);
                        Ok(Some(acc.clone()))
                    }
                    None => Ok(None),
                }
            }
            Self::ZipformerCtc { engine, accumulated } => {
                let mut eng = engine.lock().unwrap();
                if was_silent {
                    let segment_text = eng.finish()?;
                    let trimmed = segment_text.trim();
                    if !trimmed.is_empty() {
                        let mut acc = accumulated.lock().unwrap();
                        if !acc.is_empty() {
                            acc.push('，');
                        }
                        acc.push_str(trimmed);
                    }
                    eng.reset();
                }
                zipformer_accept(&mut *eng, accumulated, samples)
            }
            Self::ZipformerTransducer { engine, accumulated } => {
                let mut eng = engine.lock().unwrap();
                if was_silent {
                    let segment_text = eng.finish()?;
                    let trimmed = segment_text.trim();
                    if !trimmed.is_empty() {
                        let mut acc = accumulated.lock().unwrap();
                        if !acc.is_empty() {
                            acc.push('，');
                        }
                        acc.push_str(trimmed);
                    }
                    eng.reset();
                }
                zipformer_accept(&mut *eng, accumulated, samples)
            }
        }
    }

    /// 主动冲刷剩余音频（不重置状态，用于静音期间强制吐字）。
    pub fn flush(&self) -> Result<Option<String>> {
        match self {
            Self::Paraformer { engine, accumulated } => {
                let mut eng = engine.lock().unwrap();
                match eng.flush()? {
                    Some(delta) => {
                        let mut acc = accumulated.lock().unwrap();
                        acc.push_str(&delta);
                        Ok(Some(acc.clone()))
                    }
                    None => Ok(None),
                }
            }
            Self::ZipformerCtc { engine, accumulated } => {
                let mut eng = engine.lock().unwrap();
                zipformer_flush(&mut *eng, accumulated)
            }
            Self::ZipformerTransducer { engine, accumulated } => {
                let mut eng = engine.lock().unwrap();
                zipformer_flush(&mut *eng, accumulated)
            }
        }
    }

    /// 冲刷剩余音频，返回最终累积文本。
    /// 在末尾追加句号（如果文本不为空且不以标点结尾）。
    pub fn finish(&self) -> Result<String> {
        match self {
            Self::Paraformer { engine, accumulated } => {
                let mut eng = engine.lock().unwrap();
                let delta = eng.finish()?;
                let mut acc = accumulated.lock().unwrap();
                if !delta.is_empty() {
                    acc.push_str(&delta);
                }
                append_final_punctuation(&mut acc);
                Ok(crate::hans::normalize_variant(&*acc))
            }
            Self::ZipformerCtc { engine, accumulated } => {
                let final_segment = engine.lock().unwrap().finish()?;
                let trimmed = final_segment.trim();
                let mut acc = accumulated.lock().unwrap();
                if !trimmed.is_empty() {
                    if !acc.is_empty() {
                        acc.push('，');
                    }
                    acc.push_str(trimmed);
                }
                append_final_punctuation(&mut acc);
                Ok(crate::hans::normalize_variant(&*acc))
            }
            Self::ZipformerTransducer { engine, accumulated } => {
                let final_segment = engine.lock().unwrap().finish()?;
                let trimmed = final_segment.trim();
                let mut acc = accumulated.lock().unwrap();
                if !trimmed.is_empty() {
                    if !acc.is_empty() {
                        acc.push('，');
                    }
                    acc.push_str(trimmed);
                }
                append_final_punctuation(&mut acc);
                Ok(crate::hans::normalize_variant(&*acc))
            }
        }
    }

    /// 重置引擎状态，准备新的识别轮次（不重新加载模型）。
    pub fn reset(&self) {
        match self {
            Self::Paraformer { engine, accumulated } => {
                engine.lock().unwrap().reset();
                accumulated.lock().unwrap().clear();
            }
            Self::ZipformerCtc { engine, accumulated } => {
                engine.lock().unwrap().reset();
                accumulated.lock().unwrap().clear();
            }
            Self::ZipformerTransducer { engine, accumulated } => {
                engine.lock().unwrap().reset();
                accumulated.lock().unwrap().clear();
            }
        }
    }
}

// ── Zipformer 共用流式逻辑（CTC 和 Transducer 方法签名相同）──

/// Zipformer accept_samples 后的标准处理：拿当前段文本，与 accumulated 拼接。
fn zipformer_accept<E: ZipformerStreamOps>(
    eng: &mut E,
    accumulated: &Mutex<String>,
    samples: &[f32],
) -> Result<Option<String>> {
    match eng.accept_samples(samples)? {
        Some(current_segment) => {
            let trimmed_segment = current_segment.trim();
            let acc = accumulated.lock().unwrap();
            if acc.is_empty() {
                Ok(Some(trimmed_segment.to_string()))
            } else if trimmed_segment.is_empty() {
                Ok(Some(acc.clone()))
            } else {
                Ok(Some(format!("{}，{}", *acc, trimmed_segment)))
            }
        }
        None => {
            let acc = accumulated.lock().unwrap();
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
) -> Result<Option<String>> {
    match eng.flush()? {
        Some(current_segment) => {
            let trimmed_segment = current_segment.trim();
            let acc = accumulated.lock().unwrap();
            if acc.is_empty() {
                Ok(Some(trimmed_segment.to_string()))
            } else if trimmed_segment.is_empty() {
                Ok(Some(acc.clone()))
            } else {
                Ok(Some(format!("{}，{}", *acc, trimmed_segment)))
            }
        }
        None => {
            let acc = accumulated.lock().unwrap();
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
    let last = text.chars().last().unwrap();
    let punctuation = ['，', '。', '！', '？', '；', '：', '、', '.', '!', '?', ';', ','];
    if !punctuation.contains(&last) {
        text.push('。');
    }
}
