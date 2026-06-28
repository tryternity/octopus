use anyhow::{Context, Result};
use xcap::Monitor;

pub struct ScreenCapture {
    pub rgba_bytes: Vec<u8>,
    pub width: u32,
    pub height: u32,
    pub monitor_x: i32,
    pub monitor_y: i32,
}

/// 截取所有显示器（返回每个显示器的截图 + 坐标）。
pub fn capture_all_monitors() -> Result<Vec<ScreenCapture>> {
    let monitors = Monitor::all().context("Failed to list monitors")?;
    let mut captures = Vec::with_capacity(monitors.len());

    for monitor in monitors {
        let name = monitor.name().unwrap_or_default();
        let mw = monitor.width().unwrap_or(0);
        let mh = monitor.height().unwrap_or(0);
        let mx = monitor.x().unwrap_or(0);
        let my = monitor.y().unwrap_or(0);
        log::info!("Capturing monitor: {} ({}x{}) at ({},{})", name, mw, mh, mx, my);

        let img = match monitor.capture_image() {
            Ok(img) => img,
            Err(e) => {
                log::warn!("Failed to capture monitor {}: {}", name, e);
                continue;
            }
        };

        let width = img.width();
        let height = img.height();
        let rgba_bytes = img.into_raw();

        let non_zero: usize = rgba_bytes.chunks(4)
            .take(1000)
            .filter(|px| px[0] != 0 || px[1] != 0 || px[2] != 0)
            .count();
        log::info!(
            "Monitor {} captured: {}x{} ({}KB), non-zero: {}/1000",
            name, width, height, rgba_bytes.len() / 1024, non_zero,
        );
        if non_zero == 0 {
            log::error!("Monitor {} is entirely black — likely missing Screen Recording permission.", name);
        }

        captures.push(ScreenCapture {
            rgba_bytes,
            width,
            height,
            monitor_x: mx,
            monitor_y: my,
        });
    }

    if captures.is_empty() {
        anyhow::bail!("No monitors captured");
    }
    Ok(captures)
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
