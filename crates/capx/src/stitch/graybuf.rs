//! GrayBuf: 连续 row-major 灰度 buffer，替代 image::GrayImage。
//! 消除 get_pixel() 的坐标计算 + 边界检查开销，用整行切片直访。
//!
//! 拆分自原 stitch.rs（2026-08-04），机械迁移无行为变更。

use super::*;

/// 连续 row-major 灰度 buffer，替代 image::GrayImage。
/// 消除 get_pixel() 的坐标计算 + 边界检查开销，用整行切片直访。
#[derive(Clone)]
pub(crate) struct GrayBuf {
    pub(crate) data: Vec<u8>,
    pub(crate) width: usize,
    /// 该 buffer 的首行在原始图像中的 y 坐标。ROI 灰度转换时 > 0。
    pub(crate) y_offset: usize,
}

impl GrayBuf {
    /// 从 RGBA 图像的指定行范围 [y_start, y_end) 转换灰度（ROI 优化）。
    /// 仅转换需要参与匹配的行，减少 60%+ 的灰度计算量。
    pub(crate) fn from_rgba_roi(rgba: &RgbaImage, y_start: usize, y_end: usize) -> Self {
        let width = rgba.width() as usize;
        let row_bytes = width * 4;
        let raw = rgba.as_raw();
        let mut data = Vec::with_capacity(width * (y_end - y_start));
        for y in y_start..y_end {
            let row_start = y * row_bytes;
            for x in 0..width {
                let off = row_start + x * 4;
                let r = raw[off] as u32;
                let g = raw[off + 1] as u32;
                let b = raw[off + 2] as u32;
                let luma = (2126 * r + 7152 * g + 722 * b) / 10000;
                data.push(luma as u8);
            }
        }
        Self { data, width, y_offset: y_start }
    }

    /// 整行切片直访，无边界检查。y 为原始图像坐标（自动减去 y_offset）。
    #[inline]
    pub(crate) fn row(&self, y: usize) -> &[u8] {
        let local_y = y - self.y_offset;
        &self.data[local_y * self.width..(local_y + 1) * self.width]
    }

    /// 转为 image::GrayImage（供 imageproc 使用）。
    pub(crate) fn to_gray_image(&self) -> image::GrayImage {
        let h = (self.data.len() / self.width) as u32;
        match image::GrayImage::from_raw(self.width as u32, h, self.data.clone()) {
            Some(img) => img,
            None => {
                // 数据不一致（width * height != data.len()）→ 返回 1×1 空图降级，
                // 避免 panic 扼杀整个截图拼接流程。
                log::error!("GrayBuf → GrayImage 失败: width={}, data_len={}", self.width, self.data.len());
                image::GrayImage::new(1, 1)
            }
        }
    }
}

/// 将 GrayBuf 转为 Sobel 梯度特征图 + 归一化。
/// 纯色区域（max_gradient=0）返回 (空白, false)，调用方退回灰度。
///
/// **P1-5 优化（2026-07-17）**：原先调 imageproc::sobel_gradients 走 filter3x3
/// 三次分配（i16 horizontal + i16 vertical + u16 output 三个 Image）+ to_gray_image
/// 一次 data.clone()。改为自写 Sobel 直接消费 GrayBuf.data：
/// - 跳过 to_gray_image 的 clone（直接读 GrayBuf row）
/// - 跳过 imageproc 三个中间 Image 分配（只 1 个 Vec<u16>）
/// - Sobel 卷积 + max + sum + sum_sq 单 pass 算完（原 4 次全图扫描：max + mean sum +
///   var sum + from_fn 归一化 → 2 pass：Sobel+累加 + 归一化输出）
///
/// Sobel kernel + border handling（clamp / edge replication）与 imageproc 0.25.1
/// 完全一致（kernel 系数 + filter3x3 的 clamp 边界）——由单测 to_feature_map_*_matches_
/// imageproc 钉死（构造已知输入，比对输出像素级一致）。
pub(crate) fn to_feature_map(gray: &GrayBuf) -> (image::GrayImage, bool) {
    let w = gray.width;
    let h = gray.data.len() / gray.width;
    if w == 0 || h == 0 {
        return (image::GrayImage::new(w as u32, h as u32), false);
    }

    // Sobel kernel（与 imageproc::gradients::HORIZONTAL_SOBEL / VERTICAL_SOBEL 一致）：
    //   horizontal = [-1, 0, 1, -2, 0, 2, -1, 0, 1]  (检测水平方向梯度)
    //   vertical   = [-1, -2, -1, 0, 0, 0, 1, 2, 1]  (检测垂直方向梯度)
    // 梯度幅值 = sqrt(dx² + dy²) as u16（与 imageproc::gradient_magnitude 一致）。
    // Border handling：clamp / edge replication（与 filter3x3 一致）。
    let mut gradients: Vec<u16> = Vec::with_capacity(w * h);
    // 单 pass 累积统计量——Welford 在线算法（数值稳定，避免 E[X²]-E[X]² 的
    // catastrophic cancellation，原 mean_stddev_u16 两遍 sum 等价但慢）。
    let mut max_gradient: u16 = 0;
    let mut count: u64 = 0;
    let mut mean: f64 = 0.0;
    let mut m2: f64 = 0.0; // Σ(x-mean)²
    for y in 0..h {
        // clamp y 邻居（edge replication）
        let ym1 = y.saturating_sub(1);
        let yp1 = (y + 1).min(h - 1);
        for x in 0..w {
            let xm1 = x.saturating_sub(1);
            let xp1 = (x + 1).min(w - 1);
            // 取 3×3 邻域（直接索引 GrayBuf.data，无 to_gray_image 的 clone）
            let p00 = gray.data[ym1 * w + xm1] as i32;
            let p01 = gray.data[ym1 * w + x] as i32;
            let p02 = gray.data[ym1 * w + xp1] as i32;
            let p10 = gray.data[y * w + xm1] as i32;
            let p12 = gray.data[y * w + xp1] as i32;
            let p20 = gray.data[yp1 * w + xm1] as i32;
            let p21 = gray.data[yp1 * w + x] as i32;
            let p22 = gray.data[yp1 * w + xp1] as i32;
            // Sobel 卷积（center 系数全 0 跳过）
            let dx = (p02 + 2 * p12 + p22) - (p00 + 2 * p10 + p20);
            let dy = (p20 + 2 * p21 + p22) - (p00 + 2 * p01 + p02);
            let mag = ((dx as f32 * dx as f32 + dy as f32 * dy as f32).sqrt()) as u16;
            gradients.push(mag);
            if mag > max_gradient { max_gradient = mag; }
            // Welford 在线更新（单 pass，数值稳定）
            count += 1;
            let x = mag as f64;
            let delta = x - mean;
            mean += delta / count as f64;
            m2 += delta * (x - mean);
        }
    }

    if max_gradient == 0 {
        return (image::GrayImage::new(w as u32, h as u32), false);
    }

    // 归一化：mean + 3σ（与原实现公式一致）。var = m2 / n（Welford 最终方差）
    let var = if count > 0 { m2 / count as f64 } else { 0.0 };
    let stddev = var.max(0.0).sqrt();
    let normalizer = ((mean + 3.0 * stddev) as f32).max(1.0);

    // 单 pass 归一化输出（原 from_fn 内部走 get_pixel 慢，直接 from_raw 一次性建）
    let normalized_data: Vec<u8> = gradients.iter().map(|&g| {
        let scaled = (g as f32 / normalizer) * 255.0;
        scaled.round().clamp(0.0, 255.0) as u8
    }).collect();
    let normalized = image::GrayImage::from_raw(w as u32, h as u32, normalized_data)
        .unwrap_or_else(|| image::GrayImage::new(w as u32, h as u32));
    (normalized, true)
}

/// 计算灰度 buffer 指定行范围 [y_start, y_end) 的每行抽样列均值，降为一维信号。
pub(crate) fn row_projection_means(buf: &GrayBuf, cols: &[usize], y_start: u32, y_end: u32) -> Vec<f64> {
    let n = (y_end - y_start) as usize;
    let mut proj = Vec::with_capacity(n);
    for y in y_start..y_end {
        let row = buf.row(y as usize);
        let sum: u64 = cols.iter().map(|&x| row[x] as u64).sum();
        proj.push(sum as f64 / cols.len() as f64);
    }
    proj
}
