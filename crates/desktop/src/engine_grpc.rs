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
        let endpoint = self.endpoint.clone();
        let samples = samples.to_vec();
        let language = language.to_string();
        let engine = engine.to_string();

        let fut = async move {
            let mut client = asr::asr_service_client::AsrServiceClient::connect(endpoint.clone())
                .await
                .with_context(|| format!("gRPC connect to {} failed", endpoint))?;

            // f32 samples → little-endian bytes
            let audio_bytes: Vec<u8> = samples.iter().flat_map(|s| s.to_le_bytes()).collect();

            let request = tonic::Request::new(asr::TranscribeRequest {
                audio: audio_bytes,
                language,
                engine,
            });

            let response = client
                .transcribe(request)
                .await
                .with_context(|| "gRPC transcribe failed")?;

            let result = response.into_inner();
            debug!("gRPC result: '{}' (rtf: {:.2})", result.text, result.rtf);
            Ok(result.text)
        };

        tokio::time::timeout(std::time::Duration::from_secs(8), fut)
            .await
            .map_err(|_| anyhow::anyhow!("gRPC transcription timeout"))?
    }

    async fn health_check(&self) -> bool {
        // 与 transcribe 一致加超时：避免后端无响应时健康探测无限期挂起。
        match tonic::transport::Channel::from_shared(self.endpoint.clone()) {
            Ok(endpoint) => tokio::time::timeout(std::time::Duration::from_secs(3), endpoint.connect())
                .await
                .map(|r| r.is_ok())
                .unwrap_or(false),
            Err(_) => false,
        }
    }
}

// Generated protobuf types
pub mod asr {
    tonic::include_proto!("asr");
}
