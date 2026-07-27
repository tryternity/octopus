//! 二维码识别（quircs，纯 Rust，quirc port）。
//!
//! quircs 比 rqrr 纠错能力更强（Reed-Solomon），对微信/支付宝等
//! 高纠错级别 QR 的 decode 成功率更高。
//!
//! 详见 spec 2026-07-27-qrcode-scan-design.md。

use anyhow::Result;
use image::DynamicImage;

/// 识别图片中的所有二维码，返回内容列表（可能空）。
///
/// 多码全识别——quircs `identify` 返回所有检测到的 QR。
pub fn scan(image: &DynamicImage) -> Result<Vec<String>> {
    let luma = image.to_luma8();
    let w = luma.width() as usize;
    let h = luma.height() as usize;
    let mut scanner = quircs::Quirc::new();
    let entries: Vec<_> = scanner.identify(w, h, &luma).collect();
    let mut results = Vec::new();
    for entry in entries {
        if let Ok(code) = entry {
            if let Ok(content) = code.decode() {
                let text = String::from_utf8_lossy(&content.payload).to_string();
                if !text.is_empty() {
                    results.push(text);
                }
            }
        }
    }
    Ok(results)
}
