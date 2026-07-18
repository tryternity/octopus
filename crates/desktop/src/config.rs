//! Desktop 应用配置接入。
//!
//! config.yaml 的 schema 与读取统一定义在 `octopus_infra::config`（AppConfig），
//! 本模块只保留依赖 `octopus_asr_local`/`octopus_llm` 的派生判断（is_streaming_engine / llm_config）——
//! 它们不能放进 infra（infra 不依赖任何项目 crate）。
//!
//! Task 2 模型激活语义重构后：两个函数都不再接收 `cfg` 参数——激活引擎统一从
//! `resolve_active_engine(domain)` 内存缓存取（启动时 `load_active_engine` 填充）。

// 复用 infra 的统一 AppConfig：desktop 内部用 crate::config::AppConfig 即可，
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
/// 从 LLM 域激活模型（`resolve_active_engine("llm")`）取配置构造 CompatibleLlmConfig。
///
/// follow-up #7：若 secret_key 以 `v1:` 开头（vault 已启用并迁移过），用全局 session
/// 透明解密。解密失败（vault 未初始化 / app_key 不可用）→ 回退 raw 值（让上层 HTTP
/// 调用暴露具体错误，而非在此吞掉）。
pub fn llm_config_ignore_mode() -> Option<octopus_llm::CompatibleLlmConfig> {
    match octopus_asr_local::config::resolve_active_engine("llm") {
        Ok(resolved) => {
            // 仅云端模型（is_local=0）的 secret_key 才可能是 v1: 加密格式；
            // 本地模型（Ollama 等）secret_key 为空 / 未迁移明文 → 透明解密对它们是 no-op。
            // 安全修复 #5：vault 启用但解密失败时**返回空字符串**（不返回密文），
            // 让 LLM 调用 401 暴露 vault 锁定问题，而不是把密文发到云端污染 access log。
            let secret_key = if resolved.entry.is_local {
                resolved.entry.secret_key.clone()
            } else {
                match crate::vault_secret_access::try_decrypt_secret_global(
                    &resolved.entry.secret_key,
                ) {
                    Ok(plain) => plain,
                    Err(e) => {
                        // 修复 F：与其他 3 处 fail-soft 不一致（那里返清晰 Err），
                        // 但 llm_config_ignore_mode 签名是 Option（非 Result），改签名
                        // 会扩散到所有调用方。最小修复：log 用更明显的"保险库未解锁"
                        // message（而非"返回空 key 让调用方 401"），让运维/用户排查更直接。
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
                    "LLM 激活模型 '{}' 其 API Key (secret_key) 为空，适用于本地不需要 key 的模型（如 Ollama 等）",
                    resolved.name
                );
            }
            Some(octopus_llm::CompatibleLlmConfig {
                provider: resolved.provider,
                model: resolved.name,
                base_url: resolved.entry.source,
                secret_key,
                is_thinking: resolved.is_thinking,
                is_local: resolved.entry.is_local,
                is_enabled: resolved.entry.is_enabled,
            })
        }
        Err(e) => {
            log::warn!("LLM 域无激活模型或解析失败，无法构造润色配置：{}", e);
            None
        }
    }
}
