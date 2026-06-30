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

/// macOS：截取指定显示器，排除指定的 overlay 窗口。
/// display_id = CGDirectDisplayID, exclude_window_id = NSWindow.windowNumber
/// 返回 RGBA bytes + 物理像素尺寸。
#[cfg(target_os = "macos")]
#[allow(dead_code)]
pub fn capture_display_excluding_window(
    display_id: u32,
    exclude_window_id: u32,
) -> Result<ScreenCapture> {
    use core_graphics::display::{
        CGDisplay, kCGWindowImageDefault, kCGWindowListOptionOnScreenBelowWindow,
    };

    let display = CGDisplay::new(display_id);
    let bounds = display.bounds();

    let cg_image = CGDisplay::screenshot(
        bounds,
        kCGWindowListOptionOnScreenBelowWindow,
        exclude_window_id,
        kCGWindowImageDefault,
    )
    .context("CGWindowListCreateImage failed (display may be asleep)")?;

    let width = cg_image.width() as u32;
    let height = cg_image.height() as u32;
    let bpr = cg_image.bytes_per_row();
    let bpp = cg_image.bits_per_pixel();

    let cf_data = cg_image.data();
    let raw = cf_data.bytes();

    if bpp != 32 {
        anyhow::bail!("Unsupported screenshot format: {} bpp (expected 32)", bpp);
    }

    // macOS 截图 CGImage 通常为 BGRA（little-endian 32bit）。转为 RGBA。
    let mut rgba = Vec::with_capacity((width as usize) * (height as usize) * 4);
    for y in 0..height as usize {
        let row_start = y * bpr;
        for x in 0..width as usize {
            let off = row_start + x * 4;
            rgba.push(raw[off + 2]); // R
            rgba.push(raw[off + 1]); // G
            rgba.push(raw[off]); // B
            rgba.push(raw[off + 3]); // A
        }
    }

    // CGDisplayBounds 返回全局逻辑坐标（points），与 xcap Monitor::x()/y() 一致。
    Ok(ScreenCapture {
        rgba_bytes: rgba,
        width,
        height,
        monitor_x: bounds.origin.x as i32,
        monitor_y: bounds.origin.y as i32,
    })
}

/// macOS：只截取选区区域（排除 overlay 窗口）。
/// 相比 capture_display_excluding_window + crop_region，避免截全屏 4K + PNG 编解码往返，
/// 性能提升约 10×（截 ~2000×500 而非 3840×2160）。
/// 坐标参数为全局逻辑坐标（points），返回物理像素 RGBA。
#[cfg(target_os = "macos")]
pub fn capture_region_excluding_window(
    exclude_window_id: u32,
    rect_x: f64,
    rect_y: f64,
    rect_w: f64,
    rect_h: f64,
) -> Result<RgbaBytes> {
    use core_graphics::display::{
        CGDisplay, kCGWindowImageDefault, kCGWindowListOptionOnScreenBelowWindow,
    };
    use core_graphics::geometry::{CGPoint, CGRect, CGSize};

    let capture_rect = CGRect {
        origin: CGPoint { x: rect_x, y: rect_y },
        size: CGSize { width: rect_w, height: rect_h },
    };

    let cg_image = CGDisplay::screenshot(
        capture_rect,
        kCGWindowListOptionOnScreenBelowWindow,
        exclude_window_id,
        kCGWindowImageDefault,
    )
    .context("CGWindowListCreateImage failed (display may be asleep)")?;

    let width = cg_image.width() as u32;
    let height = cg_image.height() as u32;
    let bpr = cg_image.bytes_per_row();
    let bpp = cg_image.bits_per_pixel();

    let cf_data = cg_image.data();
    let raw = cf_data.bytes();

    if bpp != 32 {
        anyhow::bail!("Unsupported screenshot format: {} bpp (expected 32)", bpp);
    }

    let mut rgba = Vec::with_capacity((width as usize) * (height as usize) * 4);
    for y in 0..height as usize {
        let row_start = y * bpr;
        let row = &raw[row_start..row_start + width as usize * 4];
        for px in row.chunks_exact(4) {
            rgba.push(px[2]);
            rgba.push(px[1]);
            rgba.push(px[0]);
            rgba.push(px[3]);
        }
    }

    Ok(RgbaBytes { rgba_bytes: rgba, width, height })
}

/// RGBA 像素数据（不含 monitor 坐标）。
pub struct RgbaBytes {
    pub rgba_bytes: Vec<u8>,
    pub width: u32,
    pub height: u32,
}
