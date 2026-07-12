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

/// (name, repo, size_mb, description)
const KNOWN_MODELS: &[(&str, &str, u64, &str)] = &[
    ("m2m100-418M", "lazycodepersona/m2m100_418m", 600, "多语言翻译（100+ 语言互译）"),
    ("opus-mt", "Xenova/opus-mt-zh-en", 500, "中英双向翻译（轻量快速）"),
];

pub fn discover_translation_models() -> Vec<TranslationModelInfo> {
    KNOWN_MODELS.iter().map(|(name, repo, size_mb, _)| {
        let (downloaded, path) = check_model(name, repo);
        TranslationModelInfo {
            name: name.to_string(),
            source: repo.to_string(),
            downloaded,
            size_mb: *size_mb,
            path,
        }
    }).collect()
}

pub fn list_downloadable_translation_models() -> Vec<DownloadableTranslationModel> {
    KNOWN_MODELS.iter().map(|(name, repo, size_mb, desc)| {
        DownloadableTranslationModel {
            name: name.to_string(),
            repo: repo.to_string(),
            description: desc.to_string(),
            size_mb: *size_mb,
        }
    }).collect()
}

/// 检查模型是否已下载。
/// m2m100：HF repo 解析。
/// opus-mt：检查 ~/.octopus/models/translate/opus-mt/ 下是否有 zh-en + en-zh 子目录。
fn check_model(name: &str, repo: &str) -> (bool, String) {
    if name == "opus-mt" {
        return check_opus_mt();
    }
    // m2m100 路径
    match onnx_infra::resolve_model_dir(repo) {
        Ok(dir) => {
            let downloaded = dir.join("onnx/encoder_model_quantized.onnx").exists()
                && dir.join("onnx/decoder_model_quantized.onnx").exists()
                && dir.join("tokenizer.json").exists();
            (downloaded, dir.to_string_lossy().to_string())
        }
        Err(_) => (false, String::new()),
    }
}

/// 检查 opus-mt 是否已下载（两个方向都有才算完整）
fn check_opus_mt() -> (bool, String) {
    let home = match std::env::var("HOME") {
        Ok(h) => h,
        Err(_) => return (false, String::new()),
    };
    let base = std::path::PathBuf::from(&home).join(".octopus/models/translate/opus-mt");
    let zh_en = base.join("zh-en");
    let en_zh = base.join("en-zh");

    let check_dir = |dir: &std::path::Path| {
        dir.join("onnx/encoder_model_int8.onnx").exists()
            && dir.join("onnx/decoder_model_int8.onnx").exists()
            && dir.join("tokenizer.json").exists()
    };

    if check_dir(&zh_en) && check_dir(&en_zh) {
        (true, base.to_string_lossy().to_string())
    } else {
        (false, String::new())
    }
}
