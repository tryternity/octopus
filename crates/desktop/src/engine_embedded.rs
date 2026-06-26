use crate::engine::TranscriptionEngine;
use anyhow::Result;
use async_trait::async_trait;
use std::sync::Arc;
use octopus_asr_local::engine::AsrEngineManager;

/// 嵌入式引擎：进程内直接调用 octopus-asr-local
pub struct EmbeddedEngine {
    engine_manager: Arc<AsrEngineManager>,
}

impl EmbeddedEngine {
    pub fn new(engine_manager: Arc<AsrEngineManager>) -> Self {
        Self { engine_manager }
    }
}

#[async_trait]
impl TranscriptionEngine for EmbeddedEngine {
    async fn transcribe(&self, samples: &[f32], language: &str, engine: &str) -> Result<String> {
        // ASR 推理是 CPU 密集型，在 spawn_blocking 中执行
        let samples = samples.to_vec();
        let language = language.to_string();
        let engine = engine.to_string();
        let engine_manager = self.engine_manager.clone();

        tokio::task::spawn_blocking(move || {
            engine_manager.switch_model(&engine)?;
            engine_manager.transcribe(&samples, &language)
        })
        .await?
    }

    async fn health_check(&self) -> bool {
        true
    }
}
