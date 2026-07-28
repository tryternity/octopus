//! 二维码识别（zxing-cpp，C++ FFI，bundled 编译）。
//!
//! zxing-cpp 是工业级 QR/条码库，RS 纠错能力远强于纯 Rust 库
//!（quircs / rqrr 对 JPEG 有损压缩后的 QR 会出现 DataEcc 失败）。
//! Apache-2.0 许可证，vendored C++ 静态编译（bundled feature）。
//!
//! 详见 spec 2026-07-27-qrcode-scan-design.md。

use anyhow::Result;
use image::DynamicImage;
use zxingcpp::{BarcodeFormat, ImageView, ImageFormat};

/// 识别图片中的所有二维码，返回内容列表（可能空）。
///
/// 多码全识别——zxing-cpp reader 返回所有检测到的码。
///
/// **不依赖 zxing-cpp 的 `image` feature**（该 feature 会连带拉入 image crate 的
/// default features → avif → rav1e 整条重依赖链）。改为直接用 `ImageView::from_slice`
/// 从灰度像素构造，QR 识别只需亮度通道，无需彩色格式。
pub fn scan(image: &DynamicImage) -> Result<Vec<String>> {
    let reader = zxingcpp::read()
        .formats(zxingcpp::BarcodeFormats::list(BarcodeFormat::QRCode));
    // 转灰度（QR 纠错只关心亮度），用 Lum 格式构造 ImageView
    let luma = image.to_luma8();
    let iv = ImageView::from_slice(luma.as_raw(), luma.width(), luma.height(), ImageFormat::Lum)?;
    let results = reader.from(&iv)?;
    Ok(results.into_iter()
        .map(|r| r.text().to_string())
        .filter(|s| !s.is_empty())
        .collect())
}
