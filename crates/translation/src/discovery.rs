use serde::Serialize;

#[derive(Serialize, Clone)]
pub struct TranslationModelInfo {
    pub name: String,
    pub source: String,
    pub downloaded: bool,
    pub size_mb: u64,
    pub path: String,
}

/// 从 DB 读翻译模型列表 + 文件系统检查就绪状态。
pub fn discover_translation_models() -> Vec<TranslationModelInfo> {
    let rows = match octopus_infra::db::list_local_models_by_domain("translate") {
        Ok(r) => r,
        Err(_) => return Vec::new(),
    };
    rows.iter()
        .filter_map(|r| {
            let (downloaded, path) = check_model_ready(&r.model_name, &r.source);
            Some(TranslationModelInfo {
                name: r.model_name.clone(),
                source: r.source.clone(),
                downloaded,
                size_mb: 0,
                path,
            })
        })
        .collect()
}

/// 检查翻译模型是否已下载就绪。
fn check_model_ready(model_name: &str, source: &str) -> (bool, String) {
    match model_name {
        "opus-mt" => check_opus_mt(source),
        "m2m100-418M" => check_m2m100(source),
        _ => (false, String::new()),
    }
}

/// m2m100：检查 encoder + decoder + tokenizer 三件套。
fn check_m2m100(source: &str) -> (bool, String) {
    match onnx_infra::resolve_model_dir(source) {
        Ok(dir) => {
            let downloaded = dir.join("onnx/encoder_model_quantized.onnx").exists()
                && dir.join("onnx/decoder_model_quantized.onnx").exists()
                && dir.join("tokenizer.json").exists();
            (downloaded, dir.to_string_lossy().to_string())
        }
        Err(_) => (false, String::new()),
    }
}

/// opus-mt：检查 zh-en 和 en-zh 两个方向都存在。
fn check_opus_mt(source: &str) -> (bool, String) {
    match onnx_infra::resolve_model_dir(source) {
        Ok(base) => {
            for dir in ["zh-en", "en-zh"] {
                let d = base.join(dir);
                let ok = d.join("onnx/encoder_model_int8.onnx").exists()
                    && d.join("onnx/decoder_model_int8.onnx").exists()
                    && d.join("tokenizer.json").exists();
                if !ok {
                    return (false, String::new());
                }
            }
            (true, base.to_string_lossy().to_string())
        }
        Err(_) => (false, String::new()),
    }
}
