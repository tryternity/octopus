/// 录制循环：截图 + FFT 拼接 + finalize。
/// 平台无关，调用 overlay trait + capx stitch。

use anyhow::Result;
use image::RgbaImage;

/// 录制参数。
pub struct RecordingConfig {
    /// 排除的窗口 ID（覆盖窗口）。
    pub exclude_window_id: u32,
    /// 选区全局 Quartz 逻辑坐标。
    pub sel_x: f64,
    pub sel_y: f64,
    /// 选区逻辑尺寸。
    pub sel_w: f64,
    pub sel_h: f64,
    /// 屏幕 scale factor。
    pub scale: f64,
}

/// 截取选区区域（平台特定实现）。
pub trait CaptureSource {
    fn capture(&self) -> Result<RgbaImage>;
}

/// 运行录制循环。阻塞调用线程直到 stop() 被调用。
pub fn run_loop<C: CaptureSource>(
    capture: &C,
    on_complete: Box<dyn FnOnce(Vec<u8>) + Send>,
) {
    let mut stitcher = match capture_first(capture) {
        Some(img) => octopus_capx::stitch::Stitcher::new(img, Default::default()),
        None => {
            log::error!("scroll-capture: failed to capture first frame");
            return;
        }
    };

    let frame_duration = std::time::Duration::from_millis(100);
    loop {
        std::thread::sleep(frame_duration);
        if !crate::is_recording() {
            break;
        }
        match capture.capture() {
            Ok(frame) => {
                let _ = stitcher.process_frame(&frame);
            }
            Err(e) => {
                log::warn!("scroll-capture: capture failed: {}", e);
            }
        }
    }

    // Finalize: encode canvas to PNG
    let canvas = stitcher.canvas().clone();
    let mut png_bytes = Vec::new();
    use image::ImageEncoder;
    let encoder = image::codecs::png::PngEncoder::new(&mut png_bytes);
    let rgb = image::DynamicImage::ImageRgba8(canvas).into_rgba8();
    let _ = encoder.write_image(
        rgb.as_raw(),
        rgb.width(),
        rgb.height(),
        image::ExtendedColorType::Rgba8,
    );
    on_complete(png_bytes);
}

fn capture_first<C: CaptureSource>(capture: &C) -> Option<RgbaImage> {
    for _ in 0..5 {
        if let Ok(img) = capture.capture() {
            return Some(img);
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    None
}
