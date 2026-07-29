//! Desktop 应用配置接入。
//!
//! config.yaml 的 schema 与读取统一定义在 `octopus_infra::config`（AppConfig），
//! 本模块只保留依赖 `octopus_asr_local`/`octopus_llm` 的派生判断（is_streaming_engine / llm_config）——
//! 它们不能放进 infra（infra 不依赖任何项目 crate）。
//!
//! Task 2 模型激活语义重构后：两个函数都不再接收 `cfg` 参数——激活引擎统一从
//! `resolve_active_engine(domain)` 内存缓存取（启动时 `load_active_engine` 填充）。

// 复用 infra 的统一 AppConfig：desktop 内部用 crate::core::config::AppConfig 即可，
// 调用点无需写全 octopus_infra::config::AppConfig。
pub use octopus_infra::config::{AppConfig, PolishMode};

/// 检查激活的 ASR 引擎是否支持**本地**流式识别（StreamingSession）。
///
/// 云端引擎（Aliyun）的 `is_streaming=true` 表示支持云端 WS 流式（dashscope），
/// **不**走本地 StreamingSession——必须排除，否则 aliyun feature 未启用时
/// 会错误地走 StreamingSession 路径并在 `new()` 中 bail。
pub fn is_streaming_engine() -> bool {
    match octopus_asr_local::config::resolve_active_engine("asr") {
        Ok(resolved) => {
            resolved.entry.is_streaming
                && resolved.as_engine_category()
                    != Some(octopus_asr_local::config::EngineCategory::Aliyun)
        }
        Err(_) => false,
    }
}

/// 构建 LLM 配置，用于传给 octopus_llm::polish()。
/// polish_mode 为 Disabled 时返回 None；LLM 域无激活模型时也返回 None。
pub fn llm_config(polish_mode: PolishMode) -> Option<octopus_llm::CompatibleLlmConfig> {
    if polish_mode == PolishMode::Disabled {
        return None;
    }
    llm_config_ignore_mode()
}

/// 不检查 polish_mode 的 LLM 配置（供「立即润色」用——忽略 mode 直接润色）。
///
/// 从 ResolvedEngine 构造 CompatibleLlmConfig（含 vault 解密逻辑）。
///
/// 抽自 `llm_config_ignore_mode`，供按 key 查（`llm_config_by_key`）复用。
/// 云端模型（source_type=2）secret_key 可能是 v1: 加密格式 → 透明解密；
/// 本地模型 secret_key 为 manifest JSON 或空 → 解密 no-op。
/// vault 解密失败 → 返回空 key（让 LLM 调用 401 暴露问题，不发密文到云端）。
fn resolved_to_llm_config(resolved: &octopus_asr_local::config::ResolvedEngine) -> octopus_llm::CompatibleLlmConfig {
    let secret_key = if resolved.entry.is_local_or_builtin() {
        resolved.entry.secret_key.clone()
    } else {
        match crate::vault::vault_secret_access::try_decrypt_secret_global(
            &resolved.entry.secret_key,
        ) {
            Ok(plain) => plain,
            Err(e) => {
                log::warn!(
                    "LLM secret_key 解密失败——保险库未解锁或密文损坏，\
                     LLM 调用将以空 key 触发 401（避免密文入云端 log）：{}",
                    e
                );
                String::new()
            }
        }
    };
    if secret_key.is_empty() {
        log::info!(
            "LLM 模型 '{}' 其 API Key (secret_key) 为空，适用于本地不需要 key 的模型（如 Ollama 等）",
            resolved.name
        );
    }
    octopus_llm::CompatibleLlmConfig {
        provider: resolved.provider.clone(),
        model: resolved.name.clone(),
        base_url: resolved.entry.source.clone(),
        secret_key,
        is_thinking: resolved.is_thinking,
        source_type: resolved.entry.source_type,
        is_enabled: resolved.entry.is_enabled,
    }
}

/// 从 LLM 域激活模型（`resolve_active_engine("llm")`）取配置构造 CompatibleLlmConfig。
///
/// follow-up #7：若 secret_key 以 `v1:` 开头（vault 已启用并迁移过），用全局 session
/// 透明解密。解密失败（vault 未初始化 / app_key 不可用）→ 回退 raw 值（让上层 HTTP
/// 调用暴露具体错误，而非在此吞掉）。
pub fn llm_config_ignore_mode() -> Option<octopus_llm::CompatibleLlmConfig> {
    match octopus_asr_local::config::resolve_active_engine("llm") {
        Ok(resolved) => Some(resolved_to_llm_config(&resolved)),
        Err(e) => {
            log::warn!("LLM 域无激活模型或解析失败，无法构造润色配置：{}", e);
            None
        }
    }
}

/// 按 `provider:model_name` key 从 DB 查 LLM 配置（字幕润色用，非激活模型也可选）。
///
/// key 格式 `"openai:gpt-4o"`（provider:model_name）。split 后查 DB models 表
/// domain='llm'。找不到 → None（调用方走 NoLlmConfig 降级）。
pub fn llm_config_by_key(key: &str) -> Option<octopus_llm::CompatibleLlmConfig> {
    let (provider, model_name) = key.split_once(':')?;
    // get_model_id / get_model_by_id 内部各自 with_db（ReentrantMutex 可重入），不嵌套外层 with_db。
    let id = octopus_infra::db::get_model_id("llm", model_name, provider).ok().flatten()?;
    let row = octopus_infra::db::get_model_by_id(id).ok().flatten()?;
    let resolved = octopus_asr_local::config::resolved_engine_from_row(&row);
    Some(resolved_to_llm_config(&resolved))
}
