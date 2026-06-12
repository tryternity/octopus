use crate::engine::TranscriptionEngine;
use anyhow::{Context, Result};
use async_trait::async_trait;
use log::debug;

/// gRPC 远程引擎
pub struct GrpcRemoteEngine {
    endpoint: String,
}

impl GrpcRemoteEngine {
    pub fn new(endpoint: &str) -> Self {
        Self {
            endpoint: endpoint.to_string(),
        }
    }
}

#[async_trait]
impl TranscriptionEngine for GrpcRemoteEngine {
    async fn transcribe(&self, samples: &[f32], language: &str, engine: &str) -> Result<String> {
        let mut client = asr::asr_service_client::AsrServiceClient::connect(self.endpoint.clone())
            .await
            .with_context(|| format!("gRPC connect to {} failed", self.endpoint))?;

        // f32 samples → little-endian bytes
        let audio_bytes: Vec<u8> = samples.iter().flat_map(|s| s.to_le_bytes()).collect();

        let request = tonic::Request::new(asr::TranscribeRequest {
            audio: audio_bytes,
            language: language.to_string(),
            engine: engine.to_string(),
        });

        let response = client
            .transcribe(request)
            .await
            .with_context(|| "gRPC transcribe failed")?;

        let result = response.into_inner();
        debug!("gRPC result: '{}' (rtf: {:.2})", result.text, result.rtf);
        Ok(result.text)
    }

    async fn health_check(&self) -> bool {
        match tonic::transport::Channel::from_shared(self.endpoint.clone()) {
            Ok(endpoint) => endpoint.connect().await.is_ok(),
            Err(_) => false,
        }
    }
}

// Generated protobuf types
pub mod asr {
    tonic::include_proto!("asr");
}
