//! 图像保存工具：将图片以指定格式保存到文件路径。
//! 支持 PNG（无损）、WebP（有损）和 JPEG（有损）。

use anyhow::{Context, Result};
use std::path::Path;

/// 将 PNG 字节数据保存为有损 WebP 格式。
///
/// 读取 `png_bytes` → 解码为 RGBA → webp crate 编码（quality=90）→ 写入 `output_path`。
pub fn save_as_webp(png_bytes: &[u8], output_path: &Path, quality: u8) -> Result<()> {
    let img = ::image::load_from_memory_with_format(png_bytes, ::image::ImageFormat::Png)
        .context("Failed to decode PNG bytes")?;
    let rgba = img.to_rgba8();

    let encoder = webp::Encoder::from_rgba(&rgba, rgba.width(), rgba.height());
    let webp_data = encoder.encode(quality as f32);

    std::fs::write(output_path, &*webp_data)
        .with_context(|| format!("Failed to write WebP to {}", output_path.display()))?;

    log::info!(
        "Saved WebP (quality={}): {}B → {}B ({})",
        quality,
        png_bytes.len(),
        webp_data.len(),
        output_path.display(),
    );

    Ok(())
}

/// 将 PNG 字节数据原样保存为 PNG 文件。
pub fn save_as_png(png_bytes: &[u8], output_path: &Path) -> Result<()> {
    std::fs::write(output_path, png_bytes)
        .with_context(|| format!("Failed to write PNG to {}", output_path.display()))?;
    Ok(())
}

/// 将 PNG 字节数据保存为 JPEG 格式（有损压缩）。
///
/// 读取 `png_bytes` → 解码为 RGB（丢弃 alpha）→ image crate JPEG 编码 → 写入 `output_path`。
/// quality 范围 1-100，数值越大画质越高、文件越大。
pub fn save_as_jpeg(png_bytes: &[u8], output_path: &Path, quality: u8) -> Result<()> {
    let img = ::image::load_from_memory_with_format(png_bytes, ::image::ImageFormat::Png)
        .context("Failed to decode PNG bytes")?;
    let rgb = img.to_rgb8();

    let mut buf = std::io::BufWriter::new(std::fs::File::create(output_path).with_context(
        || format!("Failed to create JPEG file at {}", output_path.display()),
    )?);
    let mut encoder = ::image::codecs::jpeg::JpegEncoder::new_with_quality(&mut buf, quality);
    encoder
        .encode(&rgb, rgb.width(), rgb.height(), ::image::ExtendedColorType::Rgb8)
        .context("Failed to encode JPEG")?;
    drop(buf);

    let out_size = std::fs::metadata(output_path).map(|m| m.len()).unwrap_or(0);
    log::info!(
        "Saved JPEG (quality={}): {}B → {}B ({})",
        quality,
        png_bytes.len(),
        out_size,
        output_path.display(),
    );

    Ok(())
}
