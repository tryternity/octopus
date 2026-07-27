//! 二维码识别（rqrr，纯 Rust）。
//!
//! 详见 spec 2026-07-27-qrcode-scan-design.md。

use anyhow::Result;
use image::DynamicImage;

/// 识别图片中的所有二维码，返回内容列表（可能空）。
///
/// 多码全识别——rqrr `detect_grids` 返回所有检测到的 QR grid。
pub fn scan(image: &DynamicImage) -> Result<Vec<String>> {
    let luma = image.to_luma8();
    let mut prepared = rqrr::PreparedImage::prepare(luma);
    let grids = prepared.detect_grids();
    let mut results = Vec::new();
    for grid in grids {
        if let Ok((_meta, content)) = grid.decode() {
            if !content.is_empty() {
                results.push(content);
            }
        }
    }
    Ok(results)
}
