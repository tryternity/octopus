//! 二维码识别（zxing-cpp，C++ FFI，bundled 编译）。
//!
//! zxing-cpp 是工业级 QR/条码库，RS 纠错能力远强于纯 Rust 库
//!（quircs / rqrr 对 JPEG 有损压缩后的 QR 会出现 DataEcc 失败）。
//! Apache-2.0 许可证，vendored C++ 静态编译（bundled feature）。
//!
//! 详见 spec 2026-07-27-qrcode-scan-design.md。

use anyhow::Result;
use image::DynamicImage;
use zxingcpp::BarcodeFormat;

/// 识别图片中的所有二维码，返回内容列表（可能空）。
///
/// 多码全识别——zxing-cpp reader 返回所有检测到的码。
pub fn scan(image: &DynamicImage) -> Result<Vec<String>> {
    let reader = zxingcpp::read()
        .formats(zxingcpp::BarcodeFormats::list(BarcodeFormat::QRCode));
    let results = reader.from(image)?;
    Ok(results.into_iter()
        .map(|r| r.text().to_string())
        .filter(|s| !s.is_empty())
        .collect())
}
