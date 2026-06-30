use anyhow::{Context, Result};
use image::{GrayImage, GenericImage, RgbaImage};
use imageproc::gradients::sobel_gradients;
use rustfft::{FftPlanner, num_complex::Complex};

pub struct StitchConfig {
    /// 最小有效滚动位移（像素）。低于此值视为静止。
    pub min_scroll_px: f64,
    /// 相位相关峰值置信度（0~1）
    pub min_confidence: f64,
}

impl Default for StitchConfig {
    fn default() -> Self {
        Self {
            min_scroll_px: 2.0,
            min_confidence: 0.15,
        }
    }
}

/// 滚动截屏拼接器——FFT 相位相关 + Canvas-Anchored。
///
/// 每次新帧到来时，将参考帧（上次成功拼接的帧）与当前帧做 1D 相位相关，
/// 得到亚像素级垂直位移 dy。dy > min_scroll_px 时追加新内容到画布。
///
/// 相比 NCC 模板匹配的优势：
/// - 亚像素精度（抛物线拟合 → 0.1px），消除整数累积误差导致的模糊
/// - 频率域全局主峰，对周期性内容（列表行）鲁棒，不会跳到隔壁行
/// - O(N log N) FFT，比逐行 NCC 滑窗更快
pub struct Stitcher {
    canvas: RgbaImage,
    /// 参考投影：上次成功拼接帧的垂直投影 1D 信号
    reference_proj: Vec<f64>,
    sticky_top: u32,
    sticky_bottom: u32,
    detected: bool,
    config: StitchConfig,
}

impl Stitcher {
    pub fn new(first_frame: RgbaImage, config: StitchConfig) -> Self {
        let proj = project_vertical(&compute_edges(&first_frame));
        Self {
            canvas: first_frame,
            reference_proj: proj,
            sticky_top: 0,
            sticky_bottom: 0,
            detected: false,
            config,
        }
    }

    pub fn process_frame(&mut self, frame: &RgbaImage) -> Result<bool> {
        let (w, h) = (frame.width(), frame.height());

        if !self.detected {
            self.detect_sticky(frame);
            self.detected = true;
        }

        let eff_top = self.sticky_top;
        let eff_bottom = h.saturating_sub(self.sticky_bottom);
        if eff_bottom <= eff_top {
            return Ok(false);
        }

        let curr_edges = compute_edges(frame);
        let curr_proj = project_vertical_range(&curr_edges, eff_top, eff_bottom);

        // 相位相关求位移
        let (dy, confidence) = match phase_correlation_dy(&self.reference_proj, &curr_proj) {
            Some(v) => v,
            None => return Ok(false),
        };

        eprintln!("[stitch] dy={:.2} conf={:.4} (thresh {:.2}/{:.2})",
            dy, confidence, self.config.min_scroll_px, self.config.min_confidence);

        // 置信度太低
        if confidence < self.config.min_confidence {
            return Ok(false);
        }

        let scroll = dy.abs();

        // 静止
        if scroll < self.config.min_scroll_px {
            return Ok(false);
        }

        // 向下滚动（dy < 0 表示内容上移 = 滚轮下滚）
        // 只处理向下滚动
        if dy > 0.0 {
            return Ok(false);
        }

        let new_rows = scroll.round() as u32;
        if new_rows >= (eff_bottom - eff_top) {
            return Ok(false);
        }

        // 新内容在当前帧底部 new_rows 行
        let crop_y = eff_bottom - new_rows;
        let new_content = image::imageops::crop_imm(frame, 0, crop_y, w, new_rows).to_image();

        let old_h = self.canvas.height();
        let mut combined = RgbaImage::new(w, old_h + new_rows);
        combined.copy_from(&self.canvas, 0, 0).context("canvas copy")?;
        combined.copy_from(&new_content, 0, old_h).context("new_rows copy")?;
        self.canvas = combined;

        // 更新参考投影为当前帧
        self.reference_proj = curr_proj;

        eprintln!("[stitch] appended {}px, canvas now {}px", new_rows, self.canvas.height());
        Ok(true)
    }

    pub fn canvas(&self) -> &RgbaImage { &self.canvas }
    pub fn height(&self) -> u32 { self.canvas.height() }

    pub fn finalize(&mut self, _last_frame: &RgbaImage) -> Result<()> {
        Ok(())
    }

    fn detect_sticky(&mut self, frame: &RgbaImage) {
        let (w, ch) = (self.canvas.width(), self.canvas.height());
        let fh = frame.height();
        let cmp_h = ch.min(fh);
        let mut sticky_t = 0u32;
        for y in 0..cmp_h.min(80) {
            if rows_equal(&self.canvas, frame, y, y, w) { sticky_t = y + 1; }
            else { break; }
        }
        let mut sticky_b = 0u32;
        for y in 0..cmp_h.min(80) {
            let ya = cmp_h - 1 - y;
            if rows_equal(&self.canvas, frame, ya, ya, w) { sticky_b = y + 1; }
            else { break; }
        }
        self.sticky_top = sticky_t;
        self.sticky_bottom = sticky_b;
        eprintln!("[stitch] sticky_top={} sticky_bottom={}", self.sticky_top, self.sticky_bottom);
    }
}

/// 将边缘图投影为垂直方向 1D 信号（每行所有列的平均边缘强度）。
fn project_vertical(edges: &GrayImage) -> Vec<f64> {
    let (w, h) = (edges.width(), edges.height());
    (0..h).map(|y| {
        let mut sum = 0f64;
        for x in 0..w {
            sum += edges.get_pixel(x, y)[0] as f64;
        }
        sum / w as f64
    }).collect()
}

/// 投影 [eff_top, eff_bottom) 范围内的行。
fn project_vertical_range(edges: &GrayImage, eff_top: u32, eff_bottom: u32) -> Vec<f64> {
    let w = edges.width();
    (eff_top..eff_bottom).map(|y| {
        let mut sum = 0f64;
        for x in 0..w {
            sum += edges.get_pixel(x, y)[0] as f64;
        }
        sum / w as f64
    }).collect()
}

/// 1D FFT 相位相关，返回 (dy, confidence)。
/// dy < 0 表示内容向上移动（用户向下滚动）。
/// dy > 0 表示内容向下移动（用户向上滚动）。
fn phase_correlation_dy(a: &[f64], b: &[f64]) -> Option<(f64, f64)> {
    let n = a.len();
    if b.len() != n || n < 8 { return None; }

    let mut planner = FftPlanner::<f64>::new();
    let fft = planner.plan_fft_forward(n);
    let ifft = planner.plan_fft_inverse(n);

    let mut fa: Vec<Complex<f64>> = a.iter().map(|v| Complex::new(*v, 0.0)).collect();
    let mut fb: Vec<Complex<f64>> = b.iter().map(|v| Complex::new(*v, 0.0)).collect();

    fft.process(&mut fa);
    fft.process(&mut fb);

    // Normalized cross power spectrum: R = conj(Fa) * Fb / |conj(Fa) * Fb|
    let mut r: Vec<Complex<f64>> = fa.iter().zip(fb.iter()).map(|(fa, fb)| {
        let cross = fa.conj() * fb;
        let norm = cross.norm();
        if norm > 1e-10 { cross / norm } else { Complex::new(0.0, 0.0) }
    }).collect();

    ifft.process(&mut r);

    let scale = 1.0 / n as f64;

    // Find peak (skip index 0 = DC component)
    let mut peak_idx = 1usize;
    let mut peak_val = 0f64;
    for i in 1..n {
        let mag = r[i].norm() * scale;
        if mag > peak_val { peak_val = mag; peak_idx = i; }
    }

    // Displacement from circular peak position
    let dy_int = if peak_idx <= n / 2 {
        peak_idx as f64
    } else {
        peak_idx as f64 - n as f64
    };

    // Subpixel: parabolic interpolation
    let prev_val = r[if peak_idx == 0 { n - 1 } else { peak_idx - 1 }].norm() * scale;
    let next_val = r[(peak_idx + 1) % n].norm() * scale;
    let denom = prev_val - 2.0 * peak_val + next_val;
    let delta = if denom.abs() > 1e-10 {
        0.5 * (prev_val - next_val) / denom
    } else {
        0.0
    };

    Some((dy_int + delta, peak_val))
}

fn compute_edges(img: &RgbaImage) -> GrayImage {
    let gray = image::imageops::grayscale(img);
    let grad_u16 = sobel_gradients(&gray);
    let mut edges = GrayImage::new(grad_u16.width(), grad_u16.height());
    for y in 0..grad_u16.height() {
        for x in 0..grad_u16.width() {
            let v = grad_u16.get_pixel(x, y)[0].min(255) as u8;
            edges.put_pixel(x, y, image::Luma([v]));
        }
    }
    edges
}

fn rows_equal(a: &RgbaImage, b: &RgbaImage, ya: u32, yb: u32, w: u32) -> bool {
    for x in 0..w {
        if a.get_pixel(x, ya) != b.get_pixel(x, yb) { return false; }
    }
    true
}
