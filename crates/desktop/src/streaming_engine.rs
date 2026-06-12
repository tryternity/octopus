use anyhow::{Context, Result};
use std::sync::Mutex;

/// 统一的流式 ASR 引擎包装。
///
/// 对外统一返回**累积全文**语义：
/// - Zipformer 天然返回累积全文
/// - Paraformer 返回增量文本，内部追加到 accumulated 字段
/// 调用方无需关心底层引擎差异。
pub enum StreamingSession {
    Paraformer {
        engine: Mutex<octopus_asr::streaming_paraformer::StreamingParaformer>,
        accumulated: Mutex<String>,
    },
    Zipformer(Mutex<octopus_asr::streaming_zipformer::StreamingZipformer>),
}

impl StreamingSession {
    /// 根据引擎名创建流式 session。
    /// 仅支持 Paraformer 和 Zipformer 类别。
    pub fn new(engine_name: &str) -> Result<Self> {
        let category = octopus_asr::config::resolve_engine_category(engine_name)
            .context(format!("Unknown streaming engine: {}", engine_name))?;

        match category {
            octopus_asr::config::EngineCategory::Paraformer => {
                let engine = octopus_asr::streaming_paraformer::StreamingParaformer::new(engine_name)?;
                Ok(Self::Paraformer {
                    engine: Mutex::new(engine),
                    accumulated: Mutex::new(String::new()),
                })
            }
            octopus_asr::config::EngineCategory::Zipformer => {
                let engine = octopus_asr::streaming_zipformer::StreamingZipformer::new(engine_name)?;
                Ok(Self::Zipformer(Mutex::new(engine)))
            }
            other => {
                anyhow::bail!(
                    "Engine '{}' ({:?}) does not support streaming. Only Paraformer and Zipformer are supported.",
                    engine_name, other
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
            Self::Zipformer(m) => {
                let mut engine = m.lock().unwrap();
                match engine.accept_samples(samples)? {
                    Some(full_text) => Ok(Some(full_text)),
                    None => Ok(None),
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
                Ok(acc.clone())
            }
            Self::Zipformer(m) => {
                let mut eng = m.lock().unwrap();
                let text = eng.finish()?;
                Ok(text)
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
            Self::Zipformer(m) => {
                m.lock().unwrap().reset();
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
