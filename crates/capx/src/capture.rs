use anyhow::{Context, Result};
use xcap::Monitor;

pub struct ScreenCapture {
    pub rgba_bytes: Vec<u8>,
    pub width: u32,
    pub height: u32,
}

/// 截取主显示器全屏（返回 RGBA 像素 + 尺寸）。
pub fn capture_full_screen() -> Result<ScreenCapture> {
    let monitors = Monitor::all().context("Failed to list monitors")?;
    let monitor = monitors
        .into_iter()
        .next()
        .context("No monitor found")?;

    let img = monitor
        .capture_image()
        .context("Failed to capture screen")?;

    let width = img.width();
    let height = img.height();
    let rgba_bytes = img.into_raw();

    log::info!(
        "Screen captured: {}x{} ({}KB RGBA)",
        width,
        height,
        rgba_bytes.len() / 1024
    );

    Ok(ScreenCapture {
        rgba_bytes,
        width,
        height,
    })
}

/// 从全屏 RGBA 中裁剪矩形区域，返回 PNG bytes。
/// 坐标为物理像素。
pub fn crop_region(
    full: &ScreenCapture,
    x: u32,
    y: u32,
    w: u32,
    h: u32,
) -> Result<Vec<u8>> {
    let img =
        ::image::RgbaImage::from_raw(full.width, full.height, full.rgba_bytes.clone())
            .context("Failed to create RgbaImage from full screen")?;

    let x = x.min(full.width.saturating_sub(1));
    let y = y.min(full.height.saturating_sub(1));
    let w = w.min(full.width - x);
    let h = h.min(full.height - y);

    let cropped = ::image::imageops::crop_imm(&img, x, y, w, h).to_image();

    let mut png_bytes = Vec::new();
    cropped
        .write_to(
            &mut std::io::Cursor::new(&mut png_bytes),
            ::image::ImageFormat::Png,
        )
        .context("Failed to encode cropped PNG")?;

    Ok(png_bytes)
}
