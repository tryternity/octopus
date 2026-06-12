use anyhow::Result;
use async_trait::async_trait;

/// ASR 推理引擎抽象接口
#[async_trait]
pub trait TranscriptionEngine: Send + Sync {
    /// 语音识别：输入 16kHz mono f32 样本，返回识别文本
    async fn transcribe(&self, samples: &[f32], language: &str, engine: &str) -> Result<String>;

    /// 健康检查
    async fn health_check(&self) -> bool;
}
