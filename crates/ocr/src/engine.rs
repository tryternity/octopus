use anyhow::{Context, Result};
use std::sync::{Arc, OnceLock};

use crate::model;

pub struct OcrEngine {
    inner: ocr_rs::engine::OcrEngine,
}

static INSTANCE: OnceLock<Arc<OcrEngine>> = OnceLock::new();

impl OcrEngine {
    /// 全局单例，首次调用时懒加载。
    /// model_name 从 app_config.ocr_model 读取，默认 PP-OCRv6-small。
    pub fn instance() -> Result<Arc<OcrEngine>> {
        if let Some(e) = INSTANCE.get() {
            return Ok(e.clone());
        }

        let model_name = octopus_infra::db::load_config_key("ocr_model")
            .ok()
            .flatten()
            .unwrap_or_else(|| model::DEFAULT_OCR_MODEL.to_string());

        if !model::is_model_ready(&model_name) {
            anyhow::bail!(
                "OCR 模型未就绪: {}（请检查 ~/.octopus/models/ocr/{}/）",
                model_name,
                model_name
            );
        }

        let dir = model::model_dir(&model_name);
        let det_path = dir.join("det.mnn");
        let rec_path = dir.join("rec.mnn");
        let keys_path = dir.join("keys.txt");

        log::info!("Loading OCR model: {} from {}", model_name, dir.display());

        let inner = ocr_rs::engine::OcrEngine::new(&det_path, &rec_path, &keys_path, None)
            .map_err(|e| anyhow::anyhow!("Failed to init ocr_rs::OcrEngine: {:?}", e))?;

        let engine = Arc::new(OcrEngine { inner });

        let _ = INSTANCE.set(engine.clone());

        Ok(engine)
    }

    /// 识别图片字节，返回识别文本（多行用 \n 连接）。
    /// 支持 WebP / PNG 等常见格式（image crate 自动检测）。
    pub fn recognize(&self, image_bytes: &[u8]) -> Result<String> {
        let img = ::image::load_from_memory(image_bytes)
            .context("Failed to decode image")?;

        let results = self
            .inner
            .recognize(&img)
            .map_err(|e| anyhow::anyhow!("OCR recognize failed: {:?}", e))?;

        let text: Vec<String> = results.into_iter().map(|r| r.text).collect();

        Ok(text.join("\n"))
    }
}
