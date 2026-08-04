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

    /// 取底部 `strip_h` 行作为独立 GrayBuf（y_offset 归零）。
    /// 用于 canvas-anchored 模板提取与相邻帧参考 fallback：把已对齐到底部的内容
    /// 切出来作 NCC 模板。`strip_h` 超过本 buffer 行数时返回整个 buffer。
    pub(crate) fn bottom_strip(&self, strip_h: usize) -> GrayBuf {
        let total_h = self.data.len() / self.width;
        let start_row = total_h.saturating_sub(strip_h);
        GrayBuf {
            data: self.data[start_row * self.width..].to_vec(),
            width: self.width,
            y_offset: 0,
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

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::tests::{make_frame, make_frame_textured};
    /// 默认测试尺寸：宽度需足够大让 X_START_RATIO..X_END_RATIO 区间有意义，
    /// 高度需远大于 STRIP_H + MAX_SCROLL。
    const TW: u32 = 400;
    const TH: u32 = 600;

    /// P1-5 测试：自写 Sobel + Welford 归一化的 to_feature_map 必须与原 imageproc
    /// 实现像素级一致（边界 clamp / kernel 系数 / sqrt 幅值 / mean+3σ 归一化全部对齐）。
    /// 钉死行为，防止未来手写 Sobel 漂移。
    fn reference_feature_map(gray: &GrayBuf) -> (image::GrayImage, bool) {
        use imageproc::gradients::sobel_gradients;
        let luma_img = gray.to_gray_image();
        let gradients = sobel_gradients(&luma_img);
        let max_gradient = gradients.iter().copied().max().unwrap_or(0);
        if max_gradient == 0 {
            return (image::GrayImage::new(luma_img.width(), luma_img.height()), false);
        }
        let n = (gradients.width() * gradients.height()) as f64;
        let sum: f64 = gradients.iter().map(|&p| p as f64).sum();
        let mean = sum / n;
        let var: f64 = gradients.iter().map(|&p| {
            let d = p as f64 - mean;
            d * d
        }).sum::<f64>() / n;
        let stddev = var.sqrt();
        let normalizer = ((mean + 3.0 * stddev) as f32).max(1.0);
        let normalized = image::GrayImage::from_fn(gradients.width(), gradients.height(), |x, y| {
            let g = gradients.get_pixel(x, y)[0] as f32;
            let scaled = (g / normalizer) * 255.0;
            image::Luma([scaled.round().clamp(0.0, 255.0) as u8])
        });
        (normalized, true)
    }

    #[test]
    fn to_feature_map_matches_reference_constant() {
        // 常数图：max_gradient=0 → 返回 (空白, false)
        let gray = GrayBuf { data: vec![100; 12 * 7], width: 12, y_offset: 0 };
        let (feat, has) = to_feature_map(&gray);
        assert!(!has, "常数图 Sobel 应退化");
        assert_eq!(feat.dimensions(), (12, 7));
        let (ref_feat, ref_has) = reference_feature_map(&gray);
        assert_eq!(has, ref_has);
        assert_eq!(feat.dimensions(), ref_feat.dimensions());
    }

    #[test]
    fn to_feature_map_matches_reference_gradient() {
        // y 方向线性渐变 + 水平条纹，覆盖 Sobel 非零场景
        let w = 32usize;
        let h = 24usize;
        let data: Vec<u8> = (0..h).flat_map(|y| {
            (0..w).map(move |x| {
                let mut v = (y * 8) as u32 % 256;
                if (x + y) % 7 == 0 { v = (v + 80) % 256; }
                v as u8
            })
        }).collect();
        let gray = GrayBuf { data, width: w, y_offset: 0 };
        let (feat, has) = to_feature_map(&gray);
        let (ref_feat, ref_has) = reference_feature_map(&gray);
        assert_eq!(has, ref_has, "has_feature 不一致");
        assert!(has, "此输入应有 Sobel 特征");
        assert_eq!(feat.dimensions(), ref_feat.dimensions());
        // 像素级比对（允许 ±1 浮点误差：Welford vs 两遍 sum 的浮点差异）
        let a = feat.as_raw();
        let b = ref_feat.as_raw();
        assert_eq!(a.len(), b.len());
        let max_diff = a.iter().zip(b.iter()).map(|(x, y)| (*x as i32 - *y as i32).abs()).max().unwrap_or(0);
        assert!(max_diff <= 1, "自写 Sobel 与 imageproc 对照最大像素差 {} > 1", max_diff);
    }

    #[test]
    fn to_feature_map_matches_reference_realistic() {
        // 模拟真实滚动截图：宽 200 高 100，含文字行 + 噪点
        let w = 200usize;
        let h = 100usize;
        let data: Vec<u8> = (0..h).flat_map(|y| {
            (0..w).map(move |x| {
                let base = ((y / 20) * 50 + x / 30 * 30) as u32 % 256;
                let noise = ((x * 13 + y * 7) % 17) as u32;
                ((base + noise) % 256) as u8
            })
        }).collect();
        let gray = GrayBuf { data, width: w, y_offset: 0 };
        let (feat, has) = to_feature_map(&gray);
        let (ref_feat, ref_has) = reference_feature_map(&gray);
        assert_eq!(has, ref_has);
        assert!(has);
        let a = feat.as_raw();
        let b = ref_feat.as_raw();
        let max_diff = a.iter().zip(b.iter()).map(|(x, y)| (*x as i32 - *y as i32).abs()).max().unwrap_or(0);
        assert!(max_diff <= 1, "realistic 场景最大像素差 {} > 1", max_diff);
    }

    #[test]
    fn to_feature_map_handles_small_images() {
        // 1×1 和 2×2：边界 clamp 不应 panic
        let tiny = GrayBuf { data: vec![50, 200], width: 2, y_offset: 0 };
        let (feat, has) = to_feature_map(&tiny);
        assert_eq!(feat.dimensions(), (2, 1));
        let _ = has;

        let single = GrayBuf { data: vec![128], width: 1, y_offset: 0 };
        let (feat2, has2) = to_feature_map(&single);
        assert_eq!(feat2.dimensions(), (1, 1));
        // 单像素 Sobel 必退化（无邻居差分）
        assert!(!has2);
    }

    #[test]
    fn test_graybuf_color_pixel_luma() {
        // 验证彩色像素的灰度公式（非灰度输入）
        // R=100, G=150, B=200 → luma = (2126*100 + 7152*150 + 722*200) / 10000
        //                         = (212600 + 1072800 + 144400) / 10000 = 1429800 / 10000 = 142
        let mut img: image::ImageBuffer<image::Rgba<u8>, Vec<u8>> = image::ImageBuffer::new(1, 1);
        img.put_pixel(0, 0, image::Rgba([100, 150, 200, 255]));
        let buf = GrayBuf::from_rgba_roi(&img, 0, img.height() as usize);
        assert_eq!(buf.row(0)[0], 142, "彩色像素灰度公式验证");
    }

    #[test]
    fn test_graybuf_matches_image_grayscale() {
        // 验证 GrayBuf::from_rgba 与 image::imageops::grayscale 逐像素相等
        let img = make_frame(TW, TH, 0);
        let reference = image::imageops::grayscale(&img);
        let buf = GrayBuf::from_rgba_roi(&img, 0, img.height() as usize);
        assert_eq!(buf.width, TW as usize);
        assert_eq!(buf.data.len(), TW as usize * TH as usize);
        for y in 0..TH as usize {
            for x in 0..TW as usize {
                let a = reference.get_pixel(x as u32, y as u32)[0];
                let b = buf.row(y)[x];
                assert_eq!(a, b, "灰度不一致 @ ({},{})", x, y);
            }
        }
    }

    #[test]
    fn test_graybuf_bottom_strip_normal() {
        // 构造 4×8 GrayBuf（每行填行号作为内容），取底部 3 行
        let width = 4;
        let total_h = 8;
        let strip_h = 3;
        let data: Vec<u8> = (0..total_h).flat_map(|y| vec![y as u8; width]).collect();
        let buf = GrayBuf { data, width, y_offset: 0 };
        let strip = buf.bottom_strip(strip_h);
        assert_eq!(strip.width, width);
        assert_eq!(strip.y_offset, 0);
        assert_eq!(strip.data.len(), width * strip_h);
        // 底部 3 行 = 第 5、6、7 行（值 5、6、7）
        for (i, &v) in strip.data.iter().enumerate() {
            let expected = (total_h - strip_h + i / width) as u8;
            assert_eq!(v, expected, "bottom_strip[{}] = {}, 期望 {}", i, v, expected);
        }
    }

    #[test]
    fn test_graybuf_bottom_strip_exceeds_height() {
        // strip_h 超过 buffer 行数 → saturating_sub 兜底，返回整个 buffer
        let width = 2;
        let total_h = 3;
        let data: Vec<u8> = (0..total_h).flat_map(|y| vec![y as u8; width]).collect();
        let buf = GrayBuf { data: data.clone(), width, y_offset: 0 };
        let strip = buf.bottom_strip(10);
        assert_eq!(strip.data, data, "strip_h > total_h 时应返回整个 buffer");
        assert_eq!(strip.data.len(), width * total_h);
    }

    #[test]
    fn test_graybuf_bottom_strip_zero() {
        // strip_h=0 → 返回空 buffer（data.len()=0）
        let width = 4;
        let data = vec![10u8; width * 5];
        let buf = GrayBuf { data, width, y_offset: 0 };
        let strip = buf.bottom_strip(0);
        assert_eq!(strip.data.len(), 0);
        assert_eq!(strip.width, width);
        assert_eq!(strip.y_offset, 0);
    }

    #[test]
    fn test_sobel_pure_color_degrades() {
        // 真正的纯色帧（固定像素值，无渐变）
        let img: image::ImageBuffer<image::Rgba<u8>, Vec<u8>> = image::ImageBuffer::from_pixel(TW, TH, image::Rgba([128, 128, 128, 255]));
        let gray = GrayBuf::from_rgba_roi(&img, 0, TH as usize);
        let (_feat, has_feat) = to_feature_map(&gray);
        assert!(!has_feat, "纯色帧应无 Sobel 特征");
    }

    #[test]
    fn test_sobel_textured_has_features() {
        let f = make_frame_textured(TW, TH, 0, 2);
        let gray = GrayBuf::from_rgba_roi(&f, 0, TH as usize);
        let (_feat, has_feat) = to_feature_map(&gray);
        assert!(has_feat, "密集条纹帧应有 Sobel 特征");
    }
}
