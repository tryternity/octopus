//! Desktop 应用配置接入。
//!
//! config.yaml 的 schema 与读取统一定义在 `octopus_infra::config`（AppConfig），
//! 本模块只保留依赖 `octopus_asr`/`octopus_llm` 的派生判断（is_streaming_engine / llm_config）——
//! 它们不能放进 infra（infra 不依赖任何项目 crate）。

// 复用 infra 的统一 AppConfig：desktop 内部用 crate::config::AppConfig 即可，
// 调用点无需写全 octopus_infra::config::AppConfig。
pub use octopus_infra::config::{AppConfig, PolishMode};

/// 检查配置的 ASR 引擎是否支持流式识别。仅 Paraformer 和 Zipformer 支持流式。
pub fn is_streaming_engine(cfg: &AppConfig) -> bool {
    match octopus_asr::config::resolve_engine_category(&cfg.asr_engine) {
        Some(
            octopus_asr::config::EngineCategory::Paraformer
                | octopus_asr::config::EngineCategory::Zipformer,
        ) => true,
        _ => false,
    }
}

/// 构建 LLM 配置，用于传给 octopus_llm::polish()。
/// polish_mode 为 Disabled 或 secret_key 为空时返回 None（模式 1/2 都启用最终润色）。
pub fn llm_config(cfg: &AppConfig) -> Option<octopus_llm::CompatibleLlmConfig> {
    if cfg.polish_mode == PolishMode::Disabled || cfg.llm_secret_key.is_empty() {
        return None;
    }
    Some(octopus_llm::CompatibleLlmConfig {
        provider: cfg.llm_provider.clone(),
        model: cfg.llm_model.clone(),
        base_url: cfg.llm_base_url.clone(),
        secret_key: cfg.llm_secret_key.clone(),
    })
}
