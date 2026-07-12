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

/// opus-mt 由两个 HF repo 组成（zh-en + en-zh），downloadable 列表需列出两个。
/// (name, repo, size_mb, description) — name 相同的视为一组。
const KNOWN_MODELS: &[(&str, &str, u64, &str)] = &[
    ("m2m100-418M", "lazycodepersona/m2m100_418m", 600, "多语言翻译（100+ 语言互译）"),
    ("opus-mt", "Xenova/opus-mt-zh-en", 250, "中英翻译：中→英（轻量快速）"),
    ("opus-mt", "Xenova/opus-mt-en-zh", 250, "中英翻译：英→中（轻量快速）"),
];

pub fn discover_translation_models() -> Vec<TranslationModelInfo> {
    // opus-mt 两个方向合并为一行（dedup by name）
    let mut seen_names: Vec<String> = Vec::new();
    let results: Vec<TranslationModelInfo> = KNOWN_MODELS.iter()
        .filter_map(|(name, repo, size_mb, _)| {
            // opus-mt：dedup（只输出一行），downloaded = 两个方向都存在
            if *name == "opus-mt" {
                if seen_names.contains(&name.to_string()) {
                    return None;
                }
                seen_names.push(name.to_string());
                let (downloaded, path) = check_opus_mt();
                return Some(TranslationModelInfo {
                    name: name.to_string(),
                    source: "Xenova/opus-mt-zh-en".to_string(),
                    downloaded,
                    size_mb: 500, // 两方向合计
                    path,
                });
            }
            let (downloaded, path) = check_model(repo);
            Some(TranslationModelInfo {
                name: name.to_string(),
                source: repo.to_string(),
                downloaded,
                size_mb: *size_mb,
                path,
            })
        })
        .collect();
    results
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

/// 检查模型是否已下载（m2m100 路径）。
fn check_model(repo: &str) -> (bool, String) {
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

/// opus-mt 的两个 HF repo。
const OPUS_REPOS: &[&str] = &["Xenova/opus-mt-zh-en", "Xenova/opus-mt-en-zh"];

/// 检查 opus-mt 是否已下载（两个方向都有才算完整）。
/// 查找路径：(1) HF cache (2) ~/.octopus/models/<repo>（download 落盘路径）。
fn check_opus_mt() -> (bool, String) {
    for repo in OPUS_REPOS {
        match onnx_infra::resolve_model_dir(repo) {
            Ok(dir) => {
                let ok = dir.join("onnx/encoder_model_int8.onnx").exists()
                    && dir.join("onnx/decoder_model_int8.onnx").exists()
                    && dir.join("tokenizer.json").exists();
                if !ok {
                    return (false, String::new());
                }
            }
            Err(_) => return (false, String::new()),
        }
    }
    // 两个方向都找到
    (true, OPUS_REPOS[0].to_string())
}
