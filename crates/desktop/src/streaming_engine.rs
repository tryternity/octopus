use anyhow::{Context, Result};
use std::sync::Mutex;

/// 统一的流式 ASR 引擎包装。
///
/// Paraformer 返回增量文本（每次 delta），Zipformer 返回累积全文。
/// 内部统一为"累积文本"语义，调用方无需关心差异。
pub enum StreamingSession {
    Paraformer(Mutex<octopus_asr::streaming_paraformer::StreamingParaformer>),
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
                Ok(Self::Paraformer(Mutex::new(engine)))
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
    pub fn accept_samples(&self, samples: &[f32]) -> Result<Option<String>> {
        if samples.is_empty() {
            return Ok(None);
        }

        match self {
            Self::Paraformer(m) => {
                let mut engine = m.lock().unwrap();
                // Paraformer 返回增量文本
                match engine.accept_samples(samples)? {
                    Some(delta) => Ok(Some(delta)),
                    None => Ok(None),
                }
            }
            Self::Zipformer(m) => {
                let mut engine = m.lock().unwrap();
                // Zipformer 返回累积全文
                match engine.accept_samples(samples)? {
                    Some(full_text) => Ok(Some(full_text)),
                    None => Ok(None),
                }
            }
        }
    }

    /// 冲刷剩余音频，返回最终文本。
    /// 对于 Paraformer 返回最终增量，对于 Zipformer 返回完整累积文本。
    pub fn finish(&self) -> Result<String> {
        match self {
            Self::Paraformer(m) => {
                let mut engine = m.lock().unwrap();
                engine.finish()
            }
            Self::Zipformer(m) => {
                let mut engine = m.lock().unwrap();
                engine.finish()
            }
        }
    }

    /// 重置引擎状态，准备新的识别轮次（不重新加载模型）。
    pub fn reset(&self) {
        match self {
            Self::Paraformer(m) => {
                m.lock().unwrap().reset();
            }
            Self::Zipformer(m) => {
                m.lock().unwrap().reset();
            }
        }
    }
}
