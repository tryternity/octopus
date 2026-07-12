use octopus_translation::{
    discover_translation_models as do_discover,
    list_downloadable_translation_models as do_list_downloadable,
    DownloadableTranslationModel, TranslationModelInfo,
};
use serde::Serialize;

#[tauri::command]
pub fn list_downloadable_translation_models() -> Result<Vec<DownloadableTranslationModel>, String> {
    Ok(do_list_downloadable())
}

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
    let spec = &config.translate_engine;

    if spec == "llm" {
        return Ok(TranslateStatus {
            strategy: "llm".into(),
            engine_name: "LLM".into(),
            available: true,
        });
    }

    let models = do_discover();

    if spec.is_empty() {
        if let Some(m) = models.iter().find(|m| m.downloaded) {
            return Ok(TranslateStatus {
                strategy: "auto".into(),
                engine_name: m.name.clone(),
                available: true,
            });
        }
        return Ok(TranslateStatus {
            strategy: "auto".into(),
            engine_name: "LLM".into(),
            available: true,
        });
    }

    if let Some(m) = models.into_iter().find(|m| m.downloaded) {
        return Ok(TranslateStatus {
            strategy: "local".into(),
            engine_name: m.name.clone(),
            available: true,
        });
    }

    Ok(TranslateStatus {
        strategy: "auto".into(),
        engine_name: "LLM".into(),
        available: true,
    })
}
