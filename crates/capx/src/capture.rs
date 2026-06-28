use anyhow::{Context, Result};
use xcap::Monitor;

pub struct ScreenCapture {
    pub rgba_bytes: Vec<u8>,
    pub width: u32,
    pub height: u32,
    pub monitor_x: i32,
    pub monitor_y: i32,
}

/// 截取主显示器全屏（返回 RGBA 像素 + 尺寸）。
/// 主显示器 = 包含鼠标当前位置的显示器。
pub fn capture_full_screen() -> Result<ScreenCapture> {
    // 用鼠标位置定位主显示器（比 Monitor::all().next() 更可靠）
    let mouse_pos = get_mouse_position().unwrap_or((0, 0));
    let monitor = Monitor::from_point(mouse_pos.0, mouse_pos.1)
        .or_else(|_| {
            log::warn!("Monitor::from_point failed, falling back to first monitor");
            Monitor::all()
                .context("Failed to list monitors")?
                .into_iter()
                .next()
                .context("No monitor found")
        })
        .context("Failed to get monitor")?;

    log::info!(
        "Capturing monitor: {} ({}x{}) at ({},{})",
        monitor.name().unwrap_or_default(),
        monitor.width().unwrap_or(0),
        monitor.height().unwrap_or(0),
        monitor.x().unwrap_or(0),
        monitor.y().unwrap_or(0),
    );

    let img = monitor
        .capture_image()
        .context("Failed to capture screen")?;

    let width = img.width();
    let height = img.height();
    let rgba_bytes = img.into_raw();

    // 检查是否全黑（权限未授权时返回空/黑屏）
    let non_zero: usize = rgba_bytes.chunks(4)
        .take(1000)
        .filter(|px| px[0] != 0 || px[1] != 0 || px[2] != 0)
        .count();
    log::info!(
        "Screen captured: {}x{} ({}KB RGBA), non-zero pixels in first 1000: {}/1000",
        width,
        height,
        rgba_bytes.len() / 1024,
        non_zero,
    );
    if non_zero == 0 {
        log::error!("Screenshot is entirely black — likely missing Screen Recording permission. Grant permission to your terminal app (Terminal/iTerm/Warp) and restart.");
    }

    Ok(ScreenCapture {
        rgba_bytes,
        width,
        height,
        monitor_x: monitor.x().unwrap_or(0),
        monitor_y: monitor.y().unwrap_or(0),
    })
}

/// 获取当前鼠标位置（跨平台，用 CGEvent / X11 / Win32）。
#[cfg(target_os = "macos")]
fn get_mouse_position() -> Option<(i32, i32)> {
    use core_graphics::event::CGEvent;
    use core_graphics::event_source::{CGEventSource, CGEventSourceStateID};
    let source = CGEventSource::new(CGEventSourceStateID::HIDSystemState).ok()?;
    let event = CGEvent::new(source).ok()?;
    let pt = event.location();
    Some((pt.x as i32, pt.y as i32))
}

#[cfg(not(target_os = "macos"))]
fn get_mouse_position() -> Option<(i32, i32)> {
    None
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
