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
/// - `use_word_segmentation` 是否需要英文单词分词后处理（PP-OCR v5 true / v6 false；
///   VLM 默认 false——输出自带空格）
pub trait OcrBackend: Send {
    fn recognize(&mut self, image: &DynamicImage) -> Result<OcrOutput>;
    fn provides_layout(&self) -> bool { false }
    /// 是否需要 OcrEngine 后处理链调 segment_english_words。默认 false——
    /// VLM/未来后端输出自带空格时无需分词；PP-OCR v5 及更早 override 为 true。
    fn use_word_segmentation(&self) -> bool { false }
    fn unload(&mut self);
    fn name(&self) -> &str;
}
