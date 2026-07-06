use std::path::PathBuf;

pub const DEFAULT_OCR_MODEL: &str = "PP-OCRv5";

/// 模型组目录：~/.octopus/models/ocr/<model_name>/
pub fn model_dir(model_name: &str) -> PathBuf {
    octopus_infra::paths::octopus_config_home()
        .join("models")
        .join("ocr")
        .join(model_name)
}

/// 检查模型组是否就绪：det.onnx + rec.onnx + keys.txt（cls.onnx 可选）
pub fn is_model_ready(model_name: &str) -> bool {
    let dir = model_dir(model_name);
    dir.join("det.onnx").exists()
        && dir.join("rec.onnx").exists()
        && dir.join("keys.txt").exists()
}
