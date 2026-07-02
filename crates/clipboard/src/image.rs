//! 图片编码：RGBA → PNG → SHA-256 → WebP 无损 + 缩略图 → DB BLOB。
//! 替代旧文件系统方案，不再写 ~/.octopus/clipboard_images/。

use anyhow::{Context, Result};
use sha2::{Digest, Sha256};

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

/// DynamicImage → WebP 100% 无损 + 缩略图 WebP 20%（240×240 Lanczos）。
///
/// 接收已解码的 `DynamicImage`，**不再**做 PNG 解码。
/// 旧实现接收 PNG bytes 并在内部 `load_from_memory` 解码，导致调用方「RGBA→PNG(编码)→
/// RGBA(解码)→WebP(编码)」的冗余编解码；watcher / screenshot 路径手里本就有解码好的
/// 图像（watcher 从剪贴板 RGBA 构造，screenshot/migration 从 `load_from_memory` 得到），
/// 直接传入即可省掉一次完整的 PNG 解码。
pub fn encode_to_webp(img: &::image::DynamicImage) -> Result<EncodedImage> {
    let rgba = img.to_rgba8();
    let w = rgba.width();
    let h = rgba.height();

    // WebP 最大尺寸 16383px。超过则逐级降质量，50% 仍失败则放弃。
    let webp_blob = if w > 16383 || h > 16383 {
        log::warn!("[clipboard] Image too large for lossless WebP ({}×{}), trying lossy", w, h);
        match encode_webp_lossy_with_budget(&rgba) {
            Some(blob) => blob,
            None => {
                log::error!("[clipboard] WebP encoding failed at all quality levels, skipping");
                return Err(anyhow::anyhow!("WebP encoding failed for {}×{}", w, h));
            }
        }
    } else {
        let encoder = webp::Encoder::from_rgba(&rgba, w, h);
        // 无损
        match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| encoder.encode_lossless().to_vec())) {
            Ok(blob) => blob,
            Err(_) => {
                log::warn!("[clipboard] lossless WebP failed ({}×{}), trying lossy", w, h);
                match encode_webp_lossy_with_budget(&rgba) {
                    Some(blob) => blob,
                    None => {
                        log::error!("[clipboard] All WebP encoding failed, skipping");
                        return Err(anyhow::anyhow!("WebP encoding failed for {}×{}", w, h));
                    }
                }
            }
        }
    };

/// WebP 有损编码，逐级降质量（90%→80%→70%→60%→50%）。
/// 每级用 catch_unwind 防 panic。50% 仍失败则返回 None。
fn encode_webp_lossy_with_budget(rgba: &::image::RgbaImage) -> Option<Vec<u8>> {
    let encoder = webp::Encoder::from_rgba(rgba, rgba.width(), rgba.height());
    for quality in [90.0f32, 80.0, 70.0, 60.0, 50.0] {
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            encoder.encode(quality).to_vec()
        }));
        match result {
            Ok(blob) if !blob.is_empty() => {
                log::info!("[clipboard] WebP lossy q{}: {} bytes ({}×{})",
                    quality, blob.len(), rgba.width(), rgba.height());
                return Some(blob);
            }
            Ok(_) => {
                log::warn!("[clipboard] WebP lossy q{} produced empty blob", quality);
            }
            Err(_) => {
                log::warn!("[clipboard] WebP lossy q{} panicked", quality);
            }
        }
    }
    None
}

// 缩略图：resize 240×240 → WebP 20%
    // 针对超大长图，Lanczos3 插值开销过大；改用轻量级 Triangle (双线性) 过滤大幅降低 CPU 计算耗时
    let thumb_img = img.resize(240, 240, ::image::imageops::FilterType::Triangle);
    let thumb_rgba = thumb_img.to_rgba8();
    let thumb_encoder = webp::Encoder::from_rgba(&thumb_rgba, thumb_rgba.width(), thumb_rgba.height());
    let thumb_blob = thumb_encoder.encode(20.0);
    let thumb_blob = thumb_blob.to_vec();

    Ok(EncodedImage { webp_blob, thumb_blob })
}

/// 已解码的 DynamicImage → WebP 100% 无损 + 缩略图 WebP 20%（避免重复解码）。
///
/// rebase 合并时与 main 的 `encode_to_webp_from_image` 重复——统一收敛到本函数
/// （`encode_to_webp(img)`），watcher / image_migration / screenshot 全走它，
/// 删除冗余的 `_from_image` 变体。
pub fn sha256_hex(data: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(data);
    let result = hasher.finalize();
    let mut hex = String::with_capacity(64);
    for byte in result {
        hex.push_str(&format!("{:02x}", byte));
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
}
