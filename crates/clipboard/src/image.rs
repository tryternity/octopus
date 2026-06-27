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

/// PNG bytes → WebP 100% 无损 + 缩略图 WebP 20%（240×240 Lanczos）。
pub fn encode_to_webp(png_bytes: &[u8], width: u32, height: u32) -> Result<EncodedImage> {
    let img = ::image::load_from_memory_with_format(png_bytes, ::image::ImageFormat::Png)
        .context("Failed to decode PNG for WebP encoding")?;
    let rgba = img.to_rgba8();

    // 无损 WebP 原图
    let encoder = webp::Encoder::from_rgba(&rgba, rgba.width(), rgba.height());
    let webp_blob = encoder.encode_lossless();
    let webp_blob = webp_blob.to_vec();

    // 缩略图：resize 240×240 → WebP 20%
    let thumb_img = img.resize(240, 240, ::image::imageops::FilterType::Lanczos3);
    let thumb_rgba = thumb_img.to_rgba8();
    let thumb_encoder = webp::Encoder::from_rgba(&thumb_rgba, thumb_rgba.width(), thumb_rgba.height());
    let thumb_blob = thumb_encoder.encode(20.0);
    let thumb_blob = thumb_blob.to_vec();

    Ok(EncodedImage { webp_blob, thumb_blob })
}

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
        let (png, _) = encode_and_hash(&rgba, 2, 2).unwrap();
        let encoded = encode_to_webp(&png, 2, 2).unwrap();
        assert!(!encoded.webp_blob.is_empty());
        assert!(!encoded.thumb_blob.is_empty());
        assert_eq!(&encoded.webp_blob[..4], b"RIFF");
        assert_eq!(&encoded.thumb_blob[..4], b"RIFF");
    }
}
