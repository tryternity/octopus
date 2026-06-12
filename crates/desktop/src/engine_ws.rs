#[cfg(feature = "remote-ws")]
use crate::engine::TranscriptionEngine;
#[cfg(feature = "remote-ws")]
use anyhow::{Context, Result};
#[cfg(feature = "remote-ws")]
use async_trait::async_trait;
#[cfg(feature = "remote-ws")]
use futures_util::{SinkExt, StreamExt};
#[cfg(feature = "remote-ws")]
use log::debug;
#[cfg(feature = "remote-ws")]
use tokio_tungstenite::tungstenite::Message;

/// WebSocket 远程引擎
#[cfg(feature = "remote-ws")]
pub struct WsRemoteEngine {
    url: String,
}

#[cfg(feature = "remote-ws")]
impl WsRemoteEngine {
    pub fn new(url: &str) -> Self {
        Self {
            url: url.to_string(),
        }
    }
}

#[cfg(feature = "remote-ws")]
#[async_trait]
impl TranscriptionEngine for WsRemoteEngine {
    async fn transcribe(&self, samples: &[f32], _language: &str, _engine: &str) -> Result<String> {
        let (mut ws, _) = tokio_tungstenite::connect_async(&self.url)
            .await
            .with_context(|| format!("WebSocket connect to {} failed", self.url))?;

        // 发送 f32 PCM 音频帧（little-endian bytes）
        let audio_bytes: Vec<u8> = samples.iter().flat_map(|s| s.to_le_bytes()).collect();
        ws.send(Message::Binary(audio_bytes.into())).await?;

        // 发送 flush 命令
        ws.send(Message::Text("flush".into())).await?;

        // 接收结果
        while let Some(msg) = ws.next().await {
            match msg {
                Ok(Message::Text(text)) => {
                    // 解析 JSON: {"text": "...", "final": true}
                    if let Ok(val) = serde_json::from_str::<serde_json::Value>(&text) {
                        if let Some(t) = val.get("text").and_then(|t| t.as_str()) {
                            debug!("WS result: {}", t);
                            return Ok(t.to_string());
                        }
                    }
                    if text.starts_with('{') {
                        continue; // 等待下一个消息
                    }
                    return Ok(text);
                }
                Ok(Message::Close(_)) => break,
                Err(e) => return Err(anyhow::anyhow!("WebSocket error: {}", e)),
                _ => {}
            }
        }

        Ok(String::new())
    }

    async fn health_check(&self) -> bool {
        tokio_tungstenite::connect_async(&self.url).await.is_ok()
    }
}
