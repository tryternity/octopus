use crate::engine::engine::TranscriptionEngine;
use anyhow::{Context, Result};
use async_trait::async_trait;
use log::debug;
use tokio::sync::OnceCell;
use tonic::transport::Channel;

/// gRPC 远程引擎
pub struct GrpcRemoteEngine {
    endpoint: String,
    channel: OnceCell<Channel>,
}

impl GrpcRemoteEngine {
    pub fn new(endpoint: &str) -> Self {
        Self {
            endpoint: endpoint.to_string(),
            channel: OnceCell::new(),
        }
    }
}

#[async_trait]
impl TranscriptionEngine for GrpcRemoteEngine {
    async fn transcribe(&self, samples: &[f32], language: &str, engine: &str) -> Result<String> {
        let samples = samples.to_vec();
        let language = language.to_string();
        let engine = engine.to_string();

        let endpoint = self.endpoint.clone();
        let channel_cell = &self.channel;

        let fut = async move {
            // connect 也受 timeout 保护（get_or_try_init 移入 fut）
            let channel = channel_cell.get_or_try_init(|| async {
                tonic::transport::Channel::from_shared(endpoint.clone())?
                    .connect()
                    .await
                    .with_context(|| format!("gRPC connect to {} failed", endpoint))
            }).await?.clone();

            let mut client = asr::asr_service_client::AsrServiceClient::new(channel);

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

        tokio::time::timeout(
            std::time::Duration::from_secs(octopus_infra::net::GRPC_REQUEST_TIMEOUT_SECS),
            fut,
        )
            .await
            .map_err(|_| anyhow::anyhow!("gRPC transcription timeout"))?
    }

    async fn health_check(&self) -> bool {
        let endpoint = self.endpoint.clone();
        let fut = self.channel.get_or_try_init(|| async {
            tonic::transport::Channel::from_shared(endpoint.clone())?
                .connect()
                .await
                .with_context(|| format!("gRPC connect to {} failed", endpoint))
        });
        tokio::time::timeout(std::time::Duration::from_secs(3), fut)
            .await
            .map(|r| r.is_ok())
            .unwrap_or(false)
    }
}

// Generated protobuf types
pub mod asr {
    tonic::include_proto!("asr");
}

