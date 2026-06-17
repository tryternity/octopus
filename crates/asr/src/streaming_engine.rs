use anyhow::{Context, Result};
use std::sync::Mutex;

/// 统一的流式 ASR 引擎包装。
///
/// 对外统一返回**累积全文**语义：
/// - Zipformer 天然返回累积全文
pub enum StreamingSession {
    Paraformer {
        engine: Mutex<crate::streaming_paraformer::StreamingParaformer>,
        accumulated: Mutex<String>,
    },
    Zipformer {
        engine: Mutex<crate::streaming_zipformer::StreamingZipformer>,
        accumulated: Mutex<String>,
    },
}

impl StreamingSession {
    /// 根据引擎 spec 创建流式 session。
    ///
    /// `engine_spec` 支持 `local:name` / `category:name` / `name` 格式（见 [`parse_model_spec`]）。
    /// 仅支持 Paraformer 和 Zipformer 类别。
    pub fn new(engine_spec: &str) -> Result<Self> {
        let category = crate::config::resolve_engine_category(engine_spec)
            .context(format!("Unknown streaming engine: {}", engine_spec))?;
        let parsed = crate::config::parse_model_spec(engine_spec);
        let bare_name = parsed.name();

        match category {
            crate::config::EngineCategory::Paraformer => {
                let engine = crate::streaming_paraformer::StreamingParaformer::new(bare_name)?;
                Ok(Self::Paraformer {
                    engine: Mutex::new(engine),
                    accumulated: Mutex::new(String::new()),
                })
            }
            crate::config::EngineCategory::Zipformer => {
                let engine = crate::streaming_zipformer::StreamingZipformer::new(bare_name)?;
                Ok(Self::Zipformer {
                    engine: Mutex::new(engine),
                    accumulated: Mutex::new(String::new()),
                })
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

                        // 上一轮静默、本轮有新文本 → 说话恢复，插入逗号
                        if was_silent && !acc.is_empty() {
                            acc.push('，');
                        }

                        acc.push_str(&delta);
                        Ok(Some(acc.clone()))
                    }
                    None => Ok(None),
                }
            }
            Self::Zipformer { engine, accumulated } => {
                let mut eng = engine.lock().unwrap();

                // 如果上一段被判定为静默，且我们已经有积累的内容，说明我们需要“斩断并重置状态”
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

                // 接受当前段的输入并生成当前短句的最新识别结果
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
        }
    }

    /// 主动冲刷剩余音频（不重置状态，用于静音期间强制吐字）。
    /// 返回更新后的累积文本（如果有新结果）。
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
            Self::Zipformer { engine, accumulated } => {
                let mut eng = engine.lock().unwrap();
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
            Self::Zipformer { engine, accumulated } => {
                let mut eng = engine.lock().unwrap();
                let final_segment = eng.finish()?;
                let trimmed_final = final_segment.trim();

                let mut acc = accumulated.lock().unwrap();
                if !trimmed_final.is_empty() {
                    if !acc.is_empty() {
                        acc.push('，');
                    }
                    acc.push_str(trimmed_final);
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
            Self::Zipformer { engine, accumulated } => {
                engine.lock().unwrap().reset();
                accumulated.lock().unwrap().clear();
            }
        }
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
