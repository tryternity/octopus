use octopus_translation::{discover_translation_models as do_discover, TranslationModelInfo};
use serde::Serialize;

#[tauri::command]
pub fn discover_translation_models() -> Result<Vec<TranslationModelInfo>, String> {
    Ok(do_discover())
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TranslateStatus {
    pub strategy: String,
    pub engine_name: String,
    pub available: bool,
}

#[tauri::command]
pub fn translate_status() -> Result<TranslateStatus, String> {
    // Task 2 后：翻译激活模型从 ACTIVE_ENGINES 缓存取（resolve_active_engine("translate")）。
    // 未激活 / 云端未填 secret_key → fallback 到激活的润色 LLM（resolve_active_engine("llm")）。
    // 与 action_bar_commands 的 resolve_translate_strategy 保持语义完全对称。
    let (strategy, engine_name, available) = match octopus_asr_local::config::resolve_active_engine("translate") {
        Ok(row) => {
            if row.entry.is_local_or_builtin() {
                ("local".to_string(), row.name, true)
            } else if row.entry.secret_key.is_empty() {
                // 云端模型未填 secret_key → fallback（与 resolve_translate_strategy 对称）
                let llm_available = octopus_asr_local::config::resolve_active_engine("llm").is_ok();
                let llm_name = octopus_asr_local::config::resolve_active_engine("llm")
                    .map(|r| r.name)
                    .unwrap_or_default();
                ("fallback_llm".into(), llm_name, llm_available)
            } else {
                ("cloud".to_string(), row.name, true)
            }
        }
        Err(_) => {
            // translate 域未激活 → fallback 到激活 LLM
            let llm_available = octopus_asr_local::config::resolve_active_engine("llm").is_ok();
            let llm_name = octopus_asr_local::config::resolve_active_engine("llm")
                .map(|r| r.name)
                .unwrap_or_default();
            ("fallback_llm".into(), llm_name, llm_available)
        }
    };

    Ok(TranslateStatus {
        strategy,
        engine_name,
        available,
    })
}
