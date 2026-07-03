//! 图片编码：RGBA → PNG → SHA-256 → WebP 无损 + 缩略图 → DB BLOB。
//! 替代旧文件系统方案，不再写 ~/.octopus/clipboard_images/。

use anyhow::{Context, Result};
use sha2::Sha256;

/// RGBA 像素 → PNG bytes + SHA-256 hash。
/// hash 用于去重（同一张图只存一份 BLOB）。
pub fn encode_and_hash(rgba: &[u8], width: u32, height: u32) -> Result<(Vec<u8>, String)> {
    let img = ::image::RgbaImage::from_raw(width, height, rgba.to_vec())
        .context("Failed to create RgbaImage from raw pixels")?;
    let mut png_bytes = Vec::new();
    img.write_to(&mut std::io::Cursor::new(&mut png_bytes), ::image::ImageFormat::Png)
        .context("Failed to encode PNG")?;
    let hash = sha256_hex(&png_bytes);
    Ok((png_bytes, hash))
}

/// 编码结果：WebP 无损原图 + WebP 缩略图。
pub struct EncodedImage {
    pub webp_blob: Vec<u8>,
    pub thumb_blob: Vec<u8>,
}

/// 单次编码尝试策略：lossless WebP / 有损 WebP(q) / JPEG(q)。
/// 由 `consts::IMAGE_SAVE_QUALITY` 解析得到（lossless 除外——它是正常尺寸的首选，
/// 不进降级常量，由 `encode_to_webp` 按尺寸插入链首）。
enum EncodeAttempt {
    WebpLossless,
    WebpLossy(u8),
    Jpeg(u8),
}

impl EncodeAttempt {
    fn label(&self) -> String {
        match self {
            EncodeAttempt::WebpLossless => "lossless WebP".to_string(),
            EncodeAttempt::WebpLossy(q) => format!("WebP q{}", q),
            EncodeAttempt::Jpeg(q) => format!("JPEG q{}", q),
        }
    }

    /// 执行一次编码，返回非空 BLOB；编码 panic（超大图常见）或失败或空 → None。
    fn try_encode(&self, img: &::image::DynamicImage, rgba: &::image::RgbaImage) -> Option<Vec<u8>> {
        let (w, h) = (rgba.width(), rgba.height());
        match self {
            EncodeAttempt::WebpLossless => {
                let encoder = webp::Encoder::from_rgba(rgba, w, h);
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| encoder.encode_lossless().to_vec()))
                    .ok()
                    .filter(|b| !b.is_empty())
            }
            EncodeAttempt::WebpLossy(q) => {
                let encoder = webp::Encoder::from_rgba(rgba, w, h);
                let qf = *q as f32;
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| encoder.encode(qf).to_vec()))
                    .ok()
                    .filter(|b| !b.is_empty())
            }
            EncodeAttempt::Jpeg(q) => {
                let qv = *q;
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    let mut buf = Vec::new();
                    let mut enc = ::image::codecs::jpeg::JpegEncoder::new_with_quality(&mut buf, qv);
                    enc.encode_image(img).map(|_| buf)
                }))
                .ok()
                .and_then(|r| r.ok())
                .filter(|b| !b.is_empty())
            }
        }
    }
}

/// 解析 `IMAGE_SAVE_QUALITY` 常量（如 `"webp:80;jpeg:80"`）→ 降级尝试列表。
/// 按 `;` 分割条目，每条 `<格式>:<质量>`；未知格式跳过。
fn parse_image_fallbacks(s: &str) -> Vec<EncodeAttempt> {
    s.split(';')
        .filter_map(|entry| {
            let (fmt, q) = entry.trim().split_once(':')?;
            let q: u8 = q.trim().parse().ok()?;
            match fmt.trim().to_ascii_lowercase().as_str() {
                "webp" => Some(EncodeAttempt::WebpLossy(q)),
                "jpeg" | "jpg" => Some(EncodeAttempt::Jpeg(q)),
                _ => None,
            }
        })
        .collect()
}

/// DynamicImage → WebP 编码 + 缩略图 WebP 20%（240×240 Triangle）。
///
/// 接收已解码的 `DynamicImage`，**不再**做 PNG 解码（旧实现接收 PNG bytes 内部
/// `load_from_memory` 解码，致「RGBA→PNG(编码)→RGBA(解码)→WebP(编码)」冗余；
/// watcher / screenshot / migration 手里本就有解码好的图像，直接传入省一次 PNG 解码）。
///
/// **编码降级链**（`consts::IMAGE_SAVE_QUALITY`，如 `"webp:80;jpeg:80"`，按 `;` 分割、
/// `:` 解析格式与质量，依次尝试直至首个成功）：正常尺寸先试 lossless WebP（最佳质量），
/// 失败后走降级链；超长图（>16383px，VP8 尺寸上限）lossless 必失败，跳过直接进降级链。
/// 每次 WebP/JPEG 编码经 `catch_unwind` 兜底，防超大图编码 panic。返回的 BLOB 可能是
/// WebP 或 JPEG（兜底产物），统一存入 `image_data.blob`。
pub fn encode_to_webp(img: &::image::DynamicImage) -> Result<EncodedImage> {
    let rgba = img.to_rgba8();
    let w = rgba.width();
    let h = rgba.height();

    // 组装尝试链：正常尺寸链首插 lossless；超长图（VP8 上限）跳过。
    let mut chain = parse_image_fallbacks(octopus_infra::consts::IMAGE_SAVE_QUALITY);
    if w > 16383 || h > 16383 {
        log::warn!("[clipboard] Image exceeds WebP max dimension ({}×{}), skipping lossless", w, h);
    } else {
        chain.insert(0, EncodeAttempt::WebpLossless);
    }

    let webp_blob = chain
        .iter()
        .find_map(|attempt| {
            let blob = attempt.try_encode(img, &rgba);
            match &blob {
                Some(b) => log::info!("[clipboard] {} succeeded: {} bytes ({}×{})", attempt.label(), b.len(), w, h),
                None => log::warn!("[clipboard] {} failed ({}×{})", attempt.label(), w, h),
            }
            blob
        })
        .ok_or_else(|| anyhow::anyhow!("All image encoding failed for {}×{}", w, h))?;

    // 缩略图：resize 240×240 → WebP 20%（固定有损，不进降级链）。
    // 针对超大长图，Lanczos3 插值开销过大；改用轻量级 Triangle (双线性) 过滤大幅降低 CPU 计算耗时。
    let thumb_img = img.resize(240, 240, ::image::imageops::FilterType::Triangle);
    let thumb_rgba = thumb_img.to_rgba8();
    let thumb_encoder = webp::Encoder::from_rgba(&thumb_rgba, thumb_rgba.width(), thumb_rgba.height());
    let thumb_blob = thumb_encoder.encode(20.0);
    let thumb_blob = thumb_blob.to_vec();

    Ok(EncodedImage { webp_blob, thumb_blob })
}

/// SHA-256 十六进制哈希。
pub fn sha256_hex(data: &[u8]) -> String {
    use sha2::Digest;
    let mut hasher = Sha256::new();
    hasher.update(data);
    let result = hasher.finalize();
    let mut hex = String::with_capacity(64);
    for byte in result {
        use std::fmt::Write;
        write!(&mut hex, "{:02x}", byte).unwrap();
    }
    hex
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_encode_and_hash() {
        let rgba = vec![255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, 255, 255, 0, 255];
        let (png, hash) = encode_and_hash(&rgba, 2, 2).unwrap();
        assert!(!png.is_empty());
        assert_eq!(hash.len(), 64);
    }

    #[test]
    fn test_dedup_same_image() {
        let rgba = vec![255, 0, 0, 255, 0, 255, 0, 255];
        let (_, hash1) = encode_and_hash(&rgba, 2, 1).unwrap();
        let (_, hash2) = encode_and_hash(&rgba, 2, 1).unwrap();
        assert_eq!(hash1, hash2);
    }

    #[test]
    fn test_encode_to_webp() {
        let rgba = vec![255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, 255, 255, 0, 255];
        let img = ::image::DynamicImage::ImageRgba8(
            ::image::RgbaImage::from_raw(2, 2, rgba).unwrap()
        );
        let encoded = encode_to_webp(&img).unwrap();
        assert!(!encoded.webp_blob.is_empty());
        assert!(!encoded.thumb_blob.is_empty());
        assert_eq!(&encoded.webp_blob[..4], b"RIFF");
        assert_eq!(&encoded.thumb_blob[..4], b"RIFF");
    }

    #[test]
    fn test_parse_image_fallbacks() {
        // 标准常量解析为降级链
        let chain = parse_image_fallbacks(octopus_infra::consts::IMAGE_SAVE_QUALITY);
        assert_eq!(chain.len(), 2);
        assert!(matches!(chain[0], EncodeAttempt::WebpLossy(80)));
        assert!(matches!(chain[1], EncodeAttempt::Jpeg(80)));

        // 容错：空白容忍、未知格式跳过、质量非数字跳过
        assert_eq!(parse_image_fallbacks(" webp : 70 ; png:90 ; jpeg:60 ; bad").len(), 2);
        assert!(parse_image_fallbacks("").is_empty());
    }
}
