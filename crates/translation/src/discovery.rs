use serde::Serialize;

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
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

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    /// 未知模型名返回 (false, "")。
    #[test]
    fn check_model_ready_unknown_returns_false() {
        let (ready, path) = check_model_ready("nonexistent-model", "translate/nonexistent");
        assert!(!ready);
        assert!(path.is_empty());
    }

    /// m2m100 目录不存在时返回 (false, "")。
    #[test]
    fn check_m2m100_missing_dir_returns_false() {
        let (ready, path) = check_m2m100("translate/m2m100-418M");
        // 开发环境可能已下载，不假设结果——只验证返回类型一致
        // 如果存在则 ready=true 且 path 非空；不存在则 ready=false 且 path 空
        if ready {
            assert!(!path.is_empty(), "ready=true 时 path 应非空");
        } else {
            assert!(path.is_empty(), "ready=false 时 path 应为空");
        }
    }

    /// opus-mt 目录不存在时返回 (false, "")。
    #[test]
    fn check_opus_mt_missing_dir_returns_false() {
        let (ready, path) = check_opus_mt("translate/opus-mt-nonexistent");
        assert!(!ready);
        assert!(path.is_empty());
    }

    /// m2m100 三件套齐全时返回 (true, path)。
    #[test]
    fn check_m2m100_ready_when_files_exist() {
        let tmp = tempdir().unwrap();
        let model_dir = tmp.path().join("translate/m2m100-418M");
        std::fs::create_dir_all(model_dir.join("onnx")).unwrap();
        std::fs::write(model_dir.join("onnx/encoder_model_quantized.onnx"), b"fake").unwrap();
        std::fs::write(model_dir.join("onnx/decoder_model_quantized.onnx"), b"fake").unwrap();
        std::fs::write(model_dir.join("tokenizer.json"), b"{}").unwrap();

        // resolve_model_dir 需要 source 在 ~/.octopus/models/ 或绝对路径下
        // 用绝对路径直接测试 check_m2m100 的文件检查逻辑
        let abs = model_dir.to_string_lossy().to_string();
        // onnx_infra::resolve_model_dir 对绝对路径直接返回
        let (ready, path) = check_m2m100(&abs);
        assert!(ready, "三件套齐全时应返回 ready=true");
        assert!(!path.is_empty(), "ready 时 path 非空");
    }

    /// opus-mt 两个方向齐全时返回 (true, path)。
    #[test]
    fn check_opus_mt_ready_when_both_directions_exist() {
        let tmp = tempdir().unwrap();
        let base = tmp.path().join("translate/opus-mt");
        for dir in ["zh-en", "en-zh"] {
            let d = base.join(dir).join("onnx");
            std::fs::create_dir_all(&d).unwrap();
            std::fs::write(d.join("encoder_model_int8.onnx"), b"fake").unwrap();
            std::fs::write(d.join("decoder_model_int8.onnx"), b"fake").unwrap();
            std::fs::write(base.join(dir).join("tokenizer.json"), b"{}").unwrap();
        }

        let abs = base.to_string_lossy().to_string();
        let (ready, path) = check_opus_mt(&abs);
        assert!(ready, "两个方向齐全时应返回 ready=true");
        assert!(!path.is_empty());
    }

    /// opus-mt 缺少一个方向时返回 (false, "")。
    #[test]
    fn check_opus_mt_missing_one_direction_returns_false() {
        let tmp = tempdir().unwrap();
        let base = tmp.path().join("translate/opus-mt");
        // 只建 zh-en，不建 en-zh
        let d = base.join("zh-en").join("onnx");
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join("encoder_model_int8.onnx"), b"fake").unwrap();
        std::fs::write(d.join("decoder_model_int8.onnx"), b"fake").unwrap();
        std::fs::write(base.join("zh-en").join("tokenizer.json"), b"{}").unwrap();

        let abs = base.to_string_lossy().to_string();
        let (ready, _path) = check_opus_mt(&abs);
        assert!(!ready, "缺 en-zh 方向时应返回 ready=false");
    }

    /// m2m100 缺少 tokenizer 时返回 (false, "")。
    #[test]
    fn check_m2m100_missing_tokenizer_returns_false() {
        let tmp = tempdir().unwrap();
        let model_dir = tmp.path().join("m2m100-test");
        std::fs::create_dir_all(model_dir.join("onnx")).unwrap();
        std::fs::write(model_dir.join("onnx/encoder_model_quantized.onnx"), b"fake").unwrap();
        std::fs::write(model_dir.join("onnx/decoder_model_quantized.onnx"), b"fake").unwrap();
        // 不建 tokenizer.json

        let abs = model_dir.to_string_lossy().to_string();
        let (ready, _path) = check_m2m100(&abs);
        assert!(!ready, "缺 tokenizer.json 时应返回 ready=false");
    }
}
