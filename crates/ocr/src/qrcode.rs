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
///
/// JPEG 有损压缩（入库后图片 q85）会模糊 QR 码边缘，quircs 可能只检测到
/// 部分 QR。因此原图 + 放大 2x 各识别一次，合并去重取并集。
pub fn scan(image: &DynamicImage) -> Result<Vec<String>> {
    let mut results = try_scan(image);

    // 放大 2x 重试（Nearest 保持像素清晰度），合并去重
    let resized = image.resize_exact(
        image.width() * 2,
        image.height() * 2,
        image::imageops::FilterType::Nearest,
    );
    for code in try_scan(&resized) {
        if !results.contains(&code) {
            results.push(code);
        }
    }

    Ok(results)
}

fn try_scan(image: &DynamicImage) -> Vec<String> {
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
    results
}
