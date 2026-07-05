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

/// BGRA→RGBA 字节重排（平台无关纯函数，便于测试）。
/// 输入：已去 bpr padding 的紧凑 BGRA 行数据。
#[cfg(target_os = "macos")]
fn bgra_to_rgba(raw: &[u8], rgba: &mut Vec<u8>) {
    for px in raw.chunks_exact(4) {
        rgba.push(px[2]); // R
        rgba.push(px[1]); // G
        rgba.push(px[0]); // B
        rgba.push(px[3]); // A
    }
}

/// macOS CGImage 解析 + BGRA→RGBA 转换的公共 helper。
/// 返回 (rgba_bytes, width, height)。三处捕获函数共用，消除重复样板。
#[cfg(target_os = "macos")]
fn cgimage_to_rgba(
    cg_image: &core_graphics::image::CGImage,
) -> Result<(Vec<u8>, u32, u32)> {
    let width = cg_image.width() as u32;
    let height = cg_image.height() as u32;
    let bpr = cg_image.bytes_per_row();
    let bpp = cg_image.bits_per_pixel();

    if bpp != 32 {
        anyhow::bail!("Unsupported screenshot format: {} bpp (expected 32)", bpp);
    }

    let cf_data = cg_image.data();
    let raw = cf_data.bytes();

    // macOS 截图 CGImage 通常为 BGRA（little-endian 32bit）。转为 RGBA。
    let mut rgba = Vec::with_capacity(width as usize * height as usize * 4);
    for y in 0..height as usize {
        let row_start = y * bpr;
        let row = &raw[row_start..row_start + width as usize * 4];
        bgra_to_rgba(row, &mut rgba);
    }

    Ok((rgba, width, height))
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

    let (rgba, width, height) = cgimage_to_rgba(&cg_image)?;

    Ok(RgbaBytes { rgba_bytes: rgba, width, height })
}

/// RGBA 像素数据（不含 monitor 坐标）。
pub struct RgbaBytes {
    pub rgba_bytes: Vec<u8>,
    pub width: u32,
    pub height: u32,
}

#[cfg(target_os = "macos")]
#[link(name = "CoreGraphics", kind = "framework")]
extern "C" {
    fn CGWindowListCopyWindowInfo(
        option: u32,
        relativeToWindow: u32,
    ) -> core_foundation::array::CFArrayRef;
}

/// macOS: Find the main window ID associated with a process ID (PID).
#[cfg(target_os = "macos")]
pub fn find_window_id_by_pid(pid: i32) -> Option<u32> {
    use core_foundation::array::CFArray;
    use core_foundation::dictionary::CFDictionary;
    use core_foundation::number::CFNumber;
    use core_foundation::string::CFString;
    use core_foundation::base::TCFType;

    // kCGWindowListOptionOnScreenOnly = 1 << 0
    let option = 1 << 0;

    unsafe {
        let array_ref = CGWindowListCopyWindowInfo(option, 0); // kCGNullWindowID = 0
        if array_ref.is_null() {
            return None;
        }
        let array = CFArray::<CFDictionary>::wrap_under_create_rule(array_ref);

        let pid_key = CFString::from_static_string("kCGWindowOwnerPID");
        let layer_key = CFString::from_static_string("kCGWindowLayer");
        let number_key = CFString::from_static_string("kCGWindowNumber");
        let bounds_key = CFString::from_static_string("kCGWindowBounds");
        let w_key = CFString::from_static_string("Width");
        let h_key = CFString::from_static_string("Height");

        for i in 0..array.len() {
            let dict = array.get(i).unwrap();

            // 1. Verify Owner PID
            let pid_value = dict.find(pid_key.as_CFTypeRef());
            if pid_value.is_none() { continue; }
            let pid_num = CFNumber::wrap_under_get_rule(*pid_value.unwrap() as *const _);
            let window_pid = pid_num.to_i32();

            // 2. Verify Window Layer (0 means the main application window)
            let layer_value = dict.find(layer_key.as_CFTypeRef());
            if layer_value.is_none() { continue; }
            let layer_num = CFNumber::wrap_under_get_rule(*layer_value.unwrap() as *const _);
            let window_layer = layer_num.to_i32();

            // 3. Extract Window Number ID
            let number_value = dict.find(number_key.as_CFTypeRef());
            if number_value.is_none() { continue; }
            let number_num = CFNumber::wrap_under_get_rule(*number_value.unwrap() as *const _);
            let window_id = number_num.to_i64();

            if window_pid != Some(pid) { continue; }
            if window_layer != Some(0) { continue; }

            // 4. Optional: check bounds to skip tiny helper windows (e.g., width/height < 100)
            if let Some(bounds_val) = dict.find(bounds_key.as_CFTypeRef()) {
                let bounds_dict = CFDictionary::<*const std::ffi::c_void, *const std::ffi::c_void>::wrap_under_get_rule(*bounds_val as *const _);
                let mut is_small = false;
                if let Some(w_val) = bounds_dict.find(w_key.as_CFTypeRef()) {
                    let w_num = CFNumber::wrap_under_get_rule(*w_val as *const _);
                    if let Some(w) = w_num.to_i64() {
                        if w < 100 { is_small = true; }
                    }
                }
                if let Some(h_val) = bounds_dict.find(h_key.as_CFTypeRef()) {
                    let h_num = CFNumber::wrap_under_get_rule(*h_val as *const _);
                    if let Some(h) = h_num.to_i64() {
                        if h < 100 { is_small = true; }
                    }
                }
                if is_small {

                    continue;
                }
            }

            if let Some(wid) = window_id {
                return Some(wid as u32);
            }
        }
    }
    None
}

/// macOS: Capture ONLY the backing store layer of a specific window.
#[cfg(target_os = "macos")]
pub fn capture_window_region(
    window_id: u32,
    rect_x: f64,
    rect_y: f64,
    rect_w: f64,
    rect_h: f64,
) -> Result<RgbaBytes> {
    use core_graphics::display::{
        kCGWindowImageDefault, kCGWindowListOptionIncludingWindow,
    };
    use core_graphics::geometry::{CGPoint, CGRect, CGSize};

    let capture_rect = CGRect {
        origin: CGPoint { x: rect_x, y: rect_y },
        size: CGSize { width: rect_w, height: rect_h },
    };

    // kCGWindowListOptionIncludingWindow ensures only the specified window is rendered
    let cg_image = core_graphics::display::CGDisplay::screenshot(
        capture_rect,
        kCGWindowListOptionIncludingWindow,
        window_id,
        kCGWindowImageDefault,
    )
    .context("CGWindowListCreateImage for single window failed")?;

    let (rgba, width, height) = cgimage_to_rgba(&cg_image)?;

    Ok(RgbaBytes { rgba_bytes: rgba, width, height })
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;

    #[test]
    fn test_bgra_to_rgba_basic() {
        // BGRA: B=10, G=20, R=30, A=255 → RGBA: 30,20,10,255
        let bgra = [10u8, 20, 30, 255];
        let mut rgba = Vec::new();
        bgra_to_rgba(&bgra, &mut rgba);
        assert_eq!(rgba, vec![30, 20, 10, 255]);
    }

    #[test]
    fn test_bgra_to_rgba_multiple_pixels() {
        let bgra = [1u8, 2, 3, 4, 5, 6, 7, 8];
        let mut rgba = Vec::new();
        bgra_to_rgba(&bgra, &mut rgba);
        assert_eq!(rgba, vec![3, 2, 1, 4, 7, 6, 5, 8]);
    }

    #[test]
    fn test_bgra_to_rgba_empty() {
        let mut rgba = Vec::new();
        bgra_to_rgba(&[], &mut rgba);
        assert!(rgba.is_empty());
    }
}
