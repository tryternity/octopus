//! 图像保存工具：将图片以指定格式保存到文件路径。
//! 支持 PNG（无损）和 WebP（90% 有损压缩）。

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
