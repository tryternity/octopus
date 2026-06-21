#[cfg(feature = "cloud")]
use crate::engine::TranscriptionEngine;
#[cfg(feature = "cloud")]
use crate::engine_embedded::EmbeddedEngine;
#[cfg(feature = "cloud")]
use anyhow::Result;
#[cfg(feature = "cloud")]
use async_trait::async_trait;
#[cfg(feature = "cloud")]
use std::sync::Arc;

/// 动态路由引擎：持有本地 + 云端两个引擎实例，每次 transcribe 按 spec
/// 解析出的 EngineCategory 动态分发——本地族 → EmbeddedEngine，
/// Aliyun → AliyunEngine。
///
/// 解决运行时切换 asr_engine（工具栏/设置窗口）时引擎实例不匹配的问题：
/// 启动时 AliyunEngine 与 EmbeddedEngine 都创建好，transcribe 的 engine
/// 参数（spec 字符串）决定实际路由，不再依赖启动时的 is_cloud_aliyun 判定。
#[cfg(feature = "cloud")]
pub struct DispatchEngine {
    embedded: EmbeddedEngine,
    dashscope: crate::engine_aliyun::AliyunEngine,
}

#[cfg(feature = "cloud")]
impl DispatchEngine {
    pub fn new(engine_manager: Arc<octopus_asr::engine::AsrEngineManager>) -> Self {
        Self {
            embedded: EmbeddedEngine::new(engine_manager),
            dashscope: crate::engine_aliyun::AliyunEngine::new(),
        }
    }
}

#[cfg(feature = "cloud")]
#[async_trait]
impl TranscriptionEngine for DispatchEngine {
    async fn transcribe(&self, samples: &[f32], language: &str, engine: &str) -> Result<String> {
        // 按 spec 解析 category 动态路由
        let is_cloud = octopus_asr::config::resolve_engine_category(engine)
            .map(|c| c == octopus_asr::config::EngineCategory::Aliyun)
            .unwrap_or(false);

        if is_cloud {
            self.dashscope.transcribe(samples, language, engine).await
        } else {
            self.embedded.transcribe(samples, language, engine).await
        }
    }

    async fn health_check(&self) -> bool {
        true
    }
}
