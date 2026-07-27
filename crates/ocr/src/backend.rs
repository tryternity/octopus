//! OCR 后端抽象。本地（PP-OCRv6）和云端（VLM，后续）统一接口。
//!
//! 详见 spec 2026-07-27-ocr-backend-trait-design.md。

use anyhow::Result;
use image::DynamicImage;
use octopus_paddle_ocr::OcrOutput;

/// OCR 后端 trait。
///
/// - `recognize` 返回 paddle-ocr 格式的 OcrOutput（text + Quads + scores）
/// - `provides_layout` VLM=true 跳过 to_markdown 后处理；PP-OCR=false 走全链
/// - `unload` 释放模型内存（PP-OCR drop RapidOcr；VLM 空实现）
pub trait OcrBackend: Send {
    fn recognize(&mut self, image: &DynamicImage) -> Result<OcrOutput>;
    fn provides_layout(&self) -> bool { false }
    fn unload(&mut self);
    fn name(&self) -> &str;
}
