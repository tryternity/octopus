use std::path::PathBuf;

pub const DEFAULT_OCR_MODEL: &str = "PP-OCRv6-small";

/// 模型组目录：~/.octopus/models/ocr/<model_name>/
pub fn model_dir(model_name: &str) -> PathBuf {
    octopus_infra::paths::octopus_config_home()
        .join("models")
        .join("ocr")
        .join(model_name)
}

/// 检查模型组三件套是否就绪：det.mnn + rec.mnn + keys.txt
pub fn is_model_ready(model_name: &str) -> bool {
    let dir = model_dir(model_name);
    dir.join("det.mnn").exists()
        && dir.join("rec.mnn").exists()
        && dir.join("keys.txt").exists()
}
