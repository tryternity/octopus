//! PP-OCRv6 本地 OCR 后端（现有 RapidOcr 逻辑，从 engine.rs 搬入）。
//!
//! 详见 spec 2026-07-27-ocr-backend-trait-design.md。
//! 暂与 engine.rs 内的 RapidOcr 逻辑并存（Task 3 后 engine.rs 切换到本 backend）。

use anyhow::Result;
use image::DynamicImage;
use octopus_paddle_ocr::{EngineConfig, OcrCallOptions, OcrOutput, RapidOcr};

use crate::backend::OcrBackend;

/// PP-OCR 本地 OCR 后端——封装 octopus-paddle-ocr（基于 ONNX Runtime 的 PaddleOCR）。
///
/// 持有 RapidOcr 实例（可被 `unload` 释放）+ 当前模型名 + 是否需要英文分词后处理。
/// `recognize` 返回 paddle-ocr 原生 OcrOutput（text + Quads + scores）；
/// 后处理链（merge/segment/to_markdown）在 OcrEngine 层，按 `use_word_segmentation`
/// 决定是否调 segment_english_words。
pub struct PaddleOcrBackend {
    /// None=已 unload 释放，OcrEngine 重载时新建 backend。
    inner: Option<RapidOcr>,
    model_name: String,
    /// v6 的 CTC space token 被正确激活，输出自带英文空格，不需要后处理分词。
    /// v5 及更早版本需要 words_alpha 词库做贪心分词。
    use_word_segmentation: bool,
}

impl PaddleOcrBackend {
    /// 从 model_name 构造——构造前应已 `model::is_model_ready` 校验。
    pub fn new(model_name: &str) -> Result<Self> {
        let dir = crate::model::model_dir(model_name);
        log::info!("Loading OCR model: {} from {}", model_name, dir.display());
        let config = build_engine_config(&dir)?;
        let ocr = RapidOcr::new(config)
            .map_err(|e| anyhow::anyhow!("Failed to init RapidOcr: {e}"))?;

        // v6 的 CTC space token 被正确激活，输出自带英文空格，不需要后处理分词。
        // v5 及更早版本需要 words_alpha 词库做贪心分词。
        let use_word_segmentation = !model_name.starts_with("PP-OCRv6");

        log::info!(
            "[paddle-backend] RapidOcr loaded — model={}, word_segmentation={}",
            model_name,
            use_word_segmentation
        );

        Ok(Self {
            inner: Some(ocr),
            model_name: model_name.to_string(),
            use_word_segmentation,
        })
    }

    /// 是否需要英文单词分词后处理（v5 及更早 true；v6 false）。
    /// OcrEngine 后处理链据此决定是否调 segment_english_words。
    pub fn use_word_segmentation(&self) -> bool {
        self.use_word_segmentation
    }
}

impl OcrBackend for PaddleOcrBackend {
    fn recognize(&mut self, image: &DynamicImage) -> Result<OcrOutput> {
        let engine_ref = self
            .inner
            .as_mut()
            .ok_or_else(|| anyhow::anyhow!("PaddleOcrBackend inner unloaded: {}", self.model_name))?;

        let rec_img = dynamic_to_rec_image(image)?;
        let opts = OcrCallOptions::default();
        let result = engine_ref
            .run(rec_img, opts)
            .map_err(|e| anyhow::anyhow!("OCR run failed: {e}"))?;
        Ok(result)
    }

    fn provides_layout(&self) -> bool {
        false
    }

    fn unload(&mut self) {
        // drop RapidOcr → 释放 ort session + mmap 权重
        self.inner = None;
        log::info!("[paddle-backend] RapidOcr unloaded — model={}", self.model_name);
    }

    fn name(&self) -> &str {
        &self.model_name
    }
}

/// 构建 RapidOcr 的 EngineConfig（拼 det/rec/cls/keys 路径，禁止下载）。
/// 从 engine.rs 搬入——逻辑完全一致，不重构不优化。
fn build_engine_config(dir: &std::path::Path) -> Result<EngineConfig> {
    use octopus_paddle_ocr::*;

    let det_path = dir.join("det.onnx");
    let rec_path = dir.join("rec.onnx");
    let keys_path = dir.join("keys.txt");
    let cls_path = dir.join("cls.onnx");

    let mut config = EngineConfig::default();

    config.det.model_path = Some(det_path);
    config.det.allow_download = false;

    config.rec.model.model_path = Some(rec_path);
    config.rec.model.rec_keys_path = Some(keys_path);
    config.rec.model.allow_download = false;

    if cls_path.exists() {
        config.cls.model_path = Some(cls_path);
        config.cls.allow_download = false;
    } else {
        config.global.use_cls = false;
    }

    Ok(config)
}

/// DynamicImage → RecImage。从 engine.rs 搬入——逻辑完全一致。
fn dynamic_to_rec_image(img: &DynamicImage) -> Result<octopus_paddle_ocr::RecImage> {
    let rgb = img.to_rgb8();
    let (w, h) = rgb.dimensions();
    // 直接用 from_rgb_u8——RecImage 内部记录 color_order，as_bgr_cow() 按需做 RGB→BGR
    // 转换（与原手动 swap 等价）。省一次 ~25MB BGR vec 分配（4K 图）+ 25M 像素循环。
    octopus_paddle_ocr::RecImage::from_rgb_u8(w as usize, h as usize, rgb.into_raw())
        .map_err(|e| anyhow::anyhow!("Failed to create RecImage: {e}"))
}
