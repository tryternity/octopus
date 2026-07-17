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
    let config = octopus_infra::config::load_config().map_err(|e| e.to_string())?;

    // translate_engine 存激活翻译模型的 DB id（Task 3）；空 / 非数字 / 不存在 / 非 translate
    // domain / 未启用 → 回退到润色 LLM 兜底翻译。与 action_bar_commands 的
    // resolve_translate_strategy 保持语义一致（仅以 DB 行字段为准，不再扫本地文件）。
    let (strategy, engine_name, available) = match config.translate_engine.parse::<i64>() {
        Ok(id) => match octopus_infra::db::get_model_by_id(id) {
            Ok(Some(row)) if row.domain == "translate" && row.is_enabled => {
                let s = if row.is_local { "local" } else { "cloud" };
                (s.to_string(), row.model_name, true)
            }
            _ => (
                "fallback_llm".into(),
                config.polish_llm.clone(),
                !config.polish_llm.is_empty(),
            ),
        },
        Err(_) => (
            "fallback_llm".into(),
            config.polish_llm.clone(),
            !config.polish_llm.is_empty(),
        ),
    };

    Ok(TranslateStatus {
        strategy,
        engine_name,
        available,
    })
}
