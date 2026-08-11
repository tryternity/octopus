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
///
/// 多显示器**并行**截图：`Monitor::all()` 必须在调用方线程（xcap 内部用
/// `MainThreadMarker`），但 `capture_image()` 调用的
/// `CGWindowListCreateImage` 是线程安全的（Apple 官方文档明确，无 SCStream
/// 的 runloop 亲和性约束），`Monitor` 内部仅持有 `CGDirectDisplayID = u32`
/// （`Send + Sync`），可跨线程 move。
///
/// 双屏 4K 从串行 ~800ms 降到并行 ~400ms（取最慢一屏）。
pub fn capture_all_monitors() -> Result<Vec<ScreenCapture>> {
    let monitors = Monitor::all().context("Failed to list monitors")?;

    // 预提取 monitor 元数据，避免 scope 闭包借用 monitors 集合。
    // Monitor: Send，可 move 进 spawned thread。
    let monitor_infos: Vec<(Monitor, String, i32, i32, u32, u32)> = monitors
        .into_iter()
        .map(|m| {
            let name = m.name().unwrap_or_default();
            let mw = m.width().unwrap_or(0);
            let mh = m.height().unwrap_or(0);
            let mx = m.x().unwrap_or(0);
            let my = m.y().unwrap_or(0);
            log::debug!("Capturing monitor: {} ({}x{}) at ({},{})", name, mw, mh, mx, my);
            (m, name, mx, my, mw, mh)
        })
        .collect();

    // std::thread::scope：所有 spawned thread 在 scope 块结束前 join，
    // 无需 Arc/<static lifetime> 约束（借用以引用方式传递）。
    let captures: Vec<ScreenCapture> = std::thread::scope(|s| {
        let handles: Vec<_> = monitor_infos
            .into_iter()
            .map(|(m, name, mx, my, _mw, _mh)| {
                s.spawn(move || -> Result<ScreenCapture> {
                    let t0 = std::time::Instant::now();
                    let img = m
                        .capture_image()
                        .with_context(|| format!("Failed to capture monitor {}", name))?;
                    let elapsed = t0.elapsed();

                    let width = img.width();
                    let height = img.height();
                    let rgba_bytes = img.into_raw();

                    let non_zero: usize = rgba_bytes
                        .chunks(4)
                        .take(1000)
                        .filter(|px| px[0] != 0 || px[1] != 0 || px[2] != 0)
                        .count();
                    log::debug!(
                        "Monitor {} captured: {}x{} ({}KB), non-zero: {}/1000, elapsed: {:?}",
                        name,
                        width,
                        height,
                        rgba_bytes.len() / 1024,
                        non_zero,
                        elapsed,
                    );
                    if non_zero == 0 {
                        log::error!(
                            "Monitor {} is entirely black — likely missing Screen Recording permission.",
                            name
                        );
                    }

                    Ok(ScreenCapture {
                        rgba_bytes,
                        width,
                        height,
                        monitor_x: mx,
                        monitor_y: my,
                    })
                })
            })
            .collect();

        // join 所有线程：失败/panic 的 monitor 跳过（与原 continue 逻辑一致）。
        handles
            .into_iter()
            .filter_map(|h| match h.join() {
                Ok(Ok(capture)) => Some(capture),
                Ok(Err(e)) => {
                    log::warn!("Monitor capture failed: {:?}", e);
                    None
                }
                Err(panic) => {
                    log::error!("Monitor capture thread panicked: {:?}", panic);
                    None
                }
            })
            .collect()
    });

    if captures.is_empty() {
        anyhow::bail!("No monitors captured");
    }
    Ok(captures)
}

/// 仅截取指定坐标位置的单个显示器，避免多屏冗余捕获与内存分配。
pub fn capture_single_monitor(mon_x: i32, mon_y: i32) -> Result<ScreenCapture> {
    let mut monitors = Monitor::all().context("Failed to list monitors")?;
    
    let index = monitors.iter().position(|m| m.x().unwrap_or(0) == mon_x && m.y().unwrap_or(0) == mon_y);
    
    let monitor = match index {
        Some(i) => monitors.remove(i),
        None => {
            log::warn!("Requested monitor at ({},{}) not found, falling back to primary monitor", mon_x, mon_y);
            if !monitors.is_empty() {
                monitors.remove(0)
            } else {
                anyhow::bail!("No monitors available");
            }
        }
    };

    let name = monitor.name().unwrap_or_default();
    let img = monitor.capture_image().context("Failed to capture single monitor")?;
    
    let width = img.width();
    let height = img.height();
    let rgba_bytes = img.into_raw();

    log::debug!(
        "Monitor {} captured: {}x{} ({}KB)",
        name, width, height, rgba_bytes.len() / 1024,
    );

    Ok(ScreenCapture {
        rgba_bytes,
        width,
        height,
        monitor_x: monitor.x().unwrap_or(0),
        monitor_y: monitor.y().unwrap_or(0),
    })
}

/// clamp 矩形坐标到 [0, full_w]×[0, full_h] 范围内（3 个裁剪函数共用）。
/// 返回 (x, y, w, h)——均保证 x+w ≤ full_w、y+h ≤ full_h、x/y < full_w/full_h。
/// 2026-08-05 抽取：消除 crop_region / crop_region_rgba / crop_region_rgba_direct 的 clamp 重复。
fn clamp_rect(full_w: u32, full_h: u32, x: u32, y: u32, w: u32, h: u32) -> (u32, u32, u32, u32) {
    let x = x.min(full_w.saturating_sub(1));
    let y = y.min(full_h.saturating_sub(1));
    let w = w.min(full_w - x);
    let h = h.min(full_h - y);
    (x, y, w, h)
}

/// 从全屏 RGBA 中裁剪矩形区域，返回 PNG bytes。
/// 坐标为物理像素。
///
/// 注意：此函数先裁剪再 PNG 编码，**不适合高频热路径**（30ms 截帧循环）——
/// PNG 编码 + 调用方 `load_from_memory` 解码构成无意义的往返开销。
/// 高频路径请用 [`crop_region_rgba`]，直接返回 `RgbaImage`，零编码。
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

    let (x, y, w, h) = clamp_rect(full.width, full.height, x, y, w, h);

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

/// 从全屏 RGBA 中裁剪矩形区域，直接返回 `RgbaImage`（零 PNG 编解码）。
/// 坐标为物理像素。用于滚动截帧等高频热路径——相比 [`crop_region`]，
/// 省去「裁剪→PNG 编码→PNG 解码→to_rgba8」往返，4K 单屏下单均可省 ~10-30ms。
pub fn crop_region_rgba(
    full: &ScreenCapture,
    x: u32,
    y: u32,
    w: u32,
    h: u32,
) -> Result<::image::RgbaImage> {
    let img =
        ::image::RgbaImage::from_raw(full.width, full.height, full.rgba_bytes.clone())
            .context("Failed to create RgbaImage from full screen")?;

    let (x, y, w, h) = clamp_rect(full.width, full.height, x, y, w, h);

    Ok(::image::imageops::crop_imm(&img, x, y, w, h).to_image())
}

/// 从只读的 RGBA 像素 Slice 中直接裁剪矩形区域，返回 `RgbaImage`（零全屏克隆与编解码）。
/// 坐标为物理像素。用于高频热路径，相比 [`crop_region_rgba`] 避免了全屏 RGBA 字节克隆，
/// 内存分配量可减少约 98% 以上。
pub fn crop_region_rgba_direct(
    full_width: u32,
    full_height: u32,
    rgba_bytes: &[u8],
    x: u32,
    y: u32,
    w: u32,
    h: u32,
) -> Result<::image::RgbaImage> {
    if rgba_bytes.len() != (full_width * full_height * 4) as usize {
        anyhow::bail!(
            "Invalid buffer size: expected {}, got {}",
            full_width * full_height * 4,
            rgba_bytes.len()
        );
    }

    let (x, y, w, h) = clamp_rect(full_width, full_height, x, y, w, h);
    let (x, y, w, h) = (x as usize, y as usize, w as usize, h as usize);

    let mut cropped_bytes = vec![0u8; w * h * 4];
    let full_width_usize = full_width as usize;

    for row in 0..h {
        let src_start = ((y + row) * full_width_usize + x) * 4;
        let src_end = src_start + w * 4;
        let dst_start = row * w * 4;
        let dst_end = dst_start + w * 4;
        
        if src_end <= rgba_bytes.len() {
            cropped_bytes[dst_start..dst_end].copy_from_slice(&rgba_bytes[src_start..src_end]);
        } else {
            anyhow::bail!("crop_region_rgba_direct: row index out of bounds");
        }
    }

    ::image::RgbaImage::from_raw(w as u32, h as u32, cropped_bytes)
        .context("Failed to construct cropped RgbaImage")
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
    // 第三十一轮 P2-3：显式校验 bpr>=width*4——macOS 不变量保证，但 helper 缺显式 ensure!
    // 若 bpr<width*4（理论不变量违反），:289 slice 越界 panic。
    anyhow::ensure!(
        bpr >= width as usize * 4,
        "CGImage bpr ({}) < width*4 ({})——数据不变量违反",
        bpr, width as usize * 4
    );
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
