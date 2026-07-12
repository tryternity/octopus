use serde::Serialize;

#[derive(Serialize, Clone)]
pub struct TranslationModelInfo {
    pub name: String,
    pub source: String,
    pub downloaded: bool,
    pub size_mb: u64,
    pub path: String,
}

#[derive(Serialize, Clone)]
pub struct DownloadableTranslationModel {
    pub name: String,
    pub repo: String,
    pub description: String,
    pub size_mb: u64,
}

const KNOWN_MODELS: &[(&str, &str, u64)] = &[
    ("m2m100-418M (int8)", "venddair/m2m100-418M-onnx-int8", 724),
];

pub fn discover_translation_models() -> Vec<TranslationModelInfo> {
    KNOWN_MODELS.iter().map(|(name, repo, size_mb)| {
        let path = find_model_path(repo);
        let downloaded = path.as_ref().map(|d| {
            d.join("encoder_model.onnx").exists()
                && d.join("decoder_model.onnx").exists()
                && d.join("sentencepiece.bpe.model").exists()
        }).unwrap_or(false);
        TranslationModelInfo {
            name: name.to_string(),
            source: repo.to_string(),
            downloaded,
            size_mb: *size_mb,
            path: path.map(|p| p.to_string_lossy().to_string()).unwrap_or_default(),
        }
    }).collect()
}

pub fn list_downloadable_translation_models() -> Vec<DownloadableTranslationModel> {
    KNOWN_MODELS.iter().map(|(name, repo, size_mb)| {
        DownloadableTranslationModel {
            name: name.to_string(),
            repo: repo.to_string(),
            description: "多语言翻译（100+ 语言互译）".to_string(),
            size_mb: *size_mb,
        }
    }).collect()
}

fn find_model_path(repo: &str) -> Option<std::path::PathBuf> {
    if let Ok(dir) = onnx_infra::resolve_model_dir(repo) {
        return Some(dir);
    }
    None
}
