use anyhow::Result;
use image::RgbaImage;
use core_graphics::display::CGDisplay;
use core_graphics::geometry::{CGRect, CGPoint, CGSize};

/// 截取指定区域，排除 overlay 窗口。
/// 坐标为全局 Quartz 逻辑坐标（原点左上）。
pub fn capture_region_excluding(
    exclude_window_id: u32,
    rect_x: f64, rect_y: f64,
    rect_w: f64, rect_h: f64,
) -> Result<RgbaImage> {
    use core_graphics::display::{
        kCGWindowImageDefault, kCGWindowListOptionOnScreenBelowWindow,
    };

    log::info!("[scroll-capture] CGWindowList rect=({},{},{},{}) exclude_wid={}",
        rect_x, rect_y, rect_w, rect_h, exclude_window_id);

    let rect = CGRect {
        origin: CGPoint { x: rect_x, y: rect_y },
        size: CGSize { width: rect_w, height: rect_h },
    };

    let cg_image = CGDisplay::screenshot(
        rect,
        kCGWindowListOptionOnScreenBelowWindow,
        exclude_window_id,
        kCGWindowImageDefault,
    ).ok_or_else(|| anyhow::anyhow!("CGWindowListCreateImage failed"))?;

    let width = cg_image.width() as u32;
    let height = cg_image.height() as u32;
    let bpr = cg_image.bytes_per_row();
    let raw = {
        let data = cg_image.data();
        data.bytes().to_vec()
    };

    // BGRA → RGBA
    let mut rgba = Vec::with_capacity((width as usize) * (height as usize) * 4);
    for y in 0..height as usize {
        let row_start = y * bpr;
        let row = &raw[row_start..row_start + width as usize * 4];
        for px in row.chunks_exact(4) {
            rgba.push(px[2]); // R
            rgba.push(px[1]); // G
            rgba.push(px[0]); // B
            rgba.push(px[3]); // A
        }
    }

    RgbaImage::from_raw(width, height, rgba)
        .ok_or_else(|| anyhow::anyhow!("failed to create RgbaImage"))
}

/// 仅截取特定窗口的选区。
/// 坐标为全局 Quartz 逻辑坐标（原点左上）。
pub fn capture_region_window(
    window_id: u32,
    rect_x: f64, rect_y: f64,
    rect_w: f64, rect_h: f64,
) -> Result<RgbaImage> {
    use core_graphics::display::{
        kCGWindowImageDefault, kCGWindowListOptionIncludingWindow,
    };

    let rect = CGRect {
        origin: CGPoint { x: rect_x, y: rect_y },
        size: CGSize { width: rect_w, height: rect_h },
    };

    let cg_image = CGDisplay::screenshot(
        rect,
        kCGWindowListOptionIncludingWindow,
        window_id,
        kCGWindowImageDefault,
    ).ok_or_else(|| anyhow::anyhow!("CGWindowListCreateImage failed"))?;

    let width = cg_image.width() as u32;
    let height = cg_image.height() as u32;
    let bpr = cg_image.bytes_per_row();
    let raw = {
        let data = cg_image.data();
        data.bytes().to_vec()
    };

    // BGRA → RGBA
    let mut rgba = Vec::with_capacity((width as usize) * (height as usize) * 4);
    for y in 0..height as usize {
        let row_start = y * bpr;
        let row = &raw[row_start..row_start + width as usize * 4];
        for px in row.chunks_exact(4) {
            rgba.push(px[2]); // R
            rgba.push(px[1]); // G
            rgba.push(px[0]); // B
            rgba.push(px[3]); // A
        }
    }

    RgbaImage::from_raw(width, height, rgba)
        .ok_or_else(|| anyhow::anyhow!("failed to create RgbaImage"))
}
