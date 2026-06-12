use crate::engine::TranscriptionEngine;
use anyhow::Result;
use async_trait::async_trait;

/// 嵌入式引擎：进程内直接调用 octopus-asr
pub struct EmbeddedEngine;

#[async_trait]
impl TranscriptionEngine for EmbeddedEngine {
    async fn transcribe(&self, samples: &[f32], language: &str, engine: &str) -> Result<String> {
        // ASR 推理是 CPU 密集型，在 spawn_blocking 中执行
        let samples = samples.to_vec();
        let language = language.to_string();
        let engine = engine.to_string();

        tokio::task::spawn_blocking(move || {
            // Resolve engine name → category via model.json, then route
            let category = octopus_asr::config::resolve_engine_category(&engine);

            match category {
                Some(octopus_asr::config::EngineCategory::Whisper) => {
                    octopus_asr::whisper::transcribe(&samples, &language)
                }
                Some(octopus_asr::config::EngineCategory::Paraformer) => {
                    octopus_asr::paraformer::transcribe(&samples, &language)
                }
                Some(octopus_asr::config::EngineCategory::Qwen3Asr) => {
                    octopus_asr::qwen3_asr::transcribe(&samples, &language)
                }
                Some(octopus_asr::config::EngineCategory::Zipformer) => {
                    octopus_asr::zipformer::transcribe(&samples, &language)
                }
                // SenseVoice as default (also for unrecognized engines)
                Some(octopus_asr::config::EngineCategory::SenseVoice) | None => {
                    octopus_asr::sensevoice::transcribe(&samples, &language)
                }
            }
        })
        .await?
    }

    async fn health_check(&self) -> bool {
        true
    }
}
