//! Desktop 应用配置接入。
//!
//! config.yaml 的 schema 与读取统一定义在 `octopus_infra::config`（AppConfig），
//! 本模块只保留依赖 `octopus_asr`/`octopus_llm` 的派生判断（is_streaming_engine / llm_config）——
//! 它们不能放进 infra（infra 不依赖任何项目 crate）。

// 复用 infra 的统一 AppConfig：desktop 内部用 crate::config::AppConfig 即可，
// 调用点无需写全 octopus_infra::config::AppConfig。
pub use octopus_infra::config::{AppConfig, PolishMode};

/// 检查配置的 ASR 引擎是否支持**本地**流式识别（StreamingSession）。
///
/// 云端引擎（Aliyun）的 `is_streaming=true` 表示支持云端 WS 流式（dashscope），
/// **不**走本地 StreamingSession——必须排除，否则 aliyun feature 未启用时
/// 会错误地走 StreamingSession 路径并在 `new()` 中 bail。
pub fn is_streaming_engine(cfg: &AppConfig) -> bool {
    if let Ok(resolved) = octopus_asr::config::resolve_active_engine(&cfg.asr_engine) {
        resolved.entry.is_streaming
            && resolved.category != octopus_asr::config::EngineCategory::Aliyun
    } else {
        false
    }
}

/// 构建 LLM 配置，用于传给 octopus_llm::polish()。
/// polish_mode 为 Disabled 时返回 None，或者如果 DB 中没有找到对应的 LLM 配置，也返回 None。
pub fn llm_config(cfg: &AppConfig) -> Option<octopus_llm::CompatibleLlmConfig> {
    if cfg.polish_mode == PolishMode::Disabled {
        return None;
    }
    llm_config_ignore_mode(cfg)
}

/// 不检查 polish_mode 的 LLM 配置（供「立即润色」用——忽略 mode 直接润色）。
pub fn llm_config_ignore_mode(cfg: &AppConfig) -> Option<octopus_llm::CompatibleLlmConfig> {
    match octopus_asr::db::load_llm_model(&cfg.polish_llm) {
        Ok(Some(llm_cfg)) => {
            if llm_cfg.secret_key.is_empty() {
                log::info!("polish_llm 为 '{}'，其 API Key (secret_key) 为空，适用于本地不需要 key 的模型（如 Ollama 等）", cfg.polish_llm);
            }
            Some(llm_cfg)
        }
        Ok(None) => {
            log::warn!("未在数据库中找到 '{}' 对应的 LLM 润色模型配置", cfg.polish_llm);
            None
        }
        Err(e) => {
            log::error!("从数据库读取 LLM 润色模型 '{}' 失败: {:?}", cfg.polish_llm, e);
            None
        }
    }
}
