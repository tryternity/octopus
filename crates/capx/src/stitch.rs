use anyhow::{Context, Result};
use image::{GrayImage, GenericImage, RgbaImage};
use imageproc::gradients::sobel_gradients;
use std::ops::Range;

pub struct StitchConfig {
    pub template_ratio: f32,
    pub min_confidence: f32,
    pub inertia_px: i32,
    pub max_lowconf_streak: u32,
}

impl Default for StitchConfig {
    fn default() -> Self {
        Self {
            template_ratio: 0.2,
            min_confidence: 0.5,
            inertia_px: 100,
            max_lowconf_streak: 8,
        }
    }
}

pub struct Stitcher {
    canvas: RgbaImage,
    last_edges: Option<GrayImage>,
    sticky_top: u32,
    sticky_bottom: u32,
    active_cols: Range<u32>,
    match_cols: Range<u32>,
    last_delta: i32,
    last_dy: i32,
    low_conf_streak: u32,
    config: StitchConfig,
    frame_count: u32,
}

impl Stitcher {
    pub fn new(first_frame: RgbaImage, config: StitchConfig) -> Self {
        let (w, _h) = (first_frame.width(), first_frame.height());
        let edges = compute_edges(&first_frame);
        Self {
            canvas: first_frame,
            last_edges: Some(edges),
            sticky_top: 0,
            sticky_bottom: 0,
            active_cols: 0..w,
            match_cols: 0..w,
            last_delta: 0,
            last_dy: 0,
            low_conf_streak: 0,
            config,
            frame_count: 1,
        }
    }

    pub fn process_frame(&mut self, frame: &RgbaImage) -> Result<bool> {
        if self.is_duplicate(frame) {
            return Ok(false);
        }

        self.frame_count += 1;
        let (w, h) = (frame.width(), frame.height());

        if self.frame_count == 2 {
            self.detect_sticky_and_active(&self.canvas.clone(), frame);
        }

        let eff_top = self.sticky_top;
        let eff_bottom = h.saturating_sub(self.sticky_bottom);
        if eff_bottom <= eff_top { return Ok(false); }
        let eff_h = eff_bottom - eff_top;

        let edges = compute_edges(frame);
        let last = match &self.last_edges {
            Some(e) => e.clone(),
            None => { self.last_edges = Some(edges); return Ok(false); }
        };

        let tpl_h = ((eff_h as f32 * self.config.template_ratio) as u32).max(10);
        let tpl_h = tpl_h.min(eff_h / 4);

        let bottom_limit = eff_bottom.saturating_sub(tpl_h);
        let middle_divider = eff_top + eff_h * 2 / 3;

        // 模板 1 (在有效区域底部 1/3 寻找纹理最丰富的 Y 坐标)
        let search_range1 = middle_divider..bottom_limit;
        let tpl_top1 = if search_range1.start < search_range1.end {
            find_best_template_y(&last, search_range1, tpl_h, w, &self.match_cols)
        } else {
            bottom_limit
        };

        // 模板 2 (在有效区域中上部 2/3 寻找纹理最丰富的 Y 坐标)
        let search_range2 = (eff_top + eff_h / 10)..(middle_divider.saturating_sub(tpl_h));
        let tpl_top2 = if search_range2.start < search_range2.end {
            find_best_template_y(&last, search_range2, tpl_h, w, &self.match_cols)
        } else {
            eff_top + eff_h / 3
        };

        let (delta1, confidence1) = self.match_template(
            &last, &edges, tpl_top1, eff_top, eff_bottom, tpl_h, w, self.last_dy,
        );

        let (delta2, confidence2) = self.match_template(
            &last, &edges, tpl_top2, eff_top, eff_bottom, tpl_h, w, self.last_dy,
        );

        // 计算所选模板在 last 帧上的平均边缘强度以判断其是否包含纹理
        fn calc_mean_edge(edges: &GrayImage, tpl_y: u32, tpl_h: u32, w: u32, cols: &Range<u32>) -> f64 {
            if tpl_h == 0 || cols.is_empty() { return 0.0; }
            let mut sum = 0u64;
            let mut count = 0u64;
            for y in 0..tpl_h {
                for x in cols.clone() {
                    if x >= w { continue; }
                    sum += edges.get_pixel(x, tpl_y + y)[0] as u64;
                    count += 1;
                }
            }
            if count == 0 { return 0.0; }
            sum as f64 / count as f64
        }

        let mean1 = calc_mean_edge(&last, tpl_top1, tpl_h, w, &self.match_cols);
        let mean2 = calc_mean_edge(&last, tpl_top2, tpl_h, w, &self.match_cols);

        let has_tex1 = mean1 > 3.0;
        let has_tex2 = mean2 > 3.0;

        let match1_ok = confidence1 >= self.config.min_confidence;
        let match2_ok = confidence2 >= self.config.min_confidence;

        let ok = if has_tex1 && has_tex2 {
            // 如果两个区域在上一帧都是有纹理的，那么为了防撕裂，在当前帧必须两个都匹配成功且位移一致！
            match1_ok && match2_ok && {
                let expected_diff = tpl_top2 as i32 - tpl_top1 as i32;
                let actual_diff = delta2 - delta1;
                (actual_diff - expected_diff).abs() <= 1
            }
        } else if has_tex1 {
            match1_ok
        } else if has_tex2 {
            match2_ok
        } else {
            false
        };

        if ok {
            self.low_conf_streak = 0;
            if match1_ok {
                self.last_delta = delta1;
                self.last_dy = tpl_top1 as i32 - (eff_top as i32 + delta1);
            } else {
                self.last_delta = delta2 - (tpl_top2 as i32 - tpl_top1 as i32);
                self.last_dy = tpl_top2 as i32 - (eff_top as i32 + delta2);
            }
        } else {
            self.low_conf_streak += 1;
            if self.low_conf_streak >= self.config.max_lowconf_streak {
                self.last_edges = Some(edges);
            }
            return Ok(false);
        }

        let new_start = eff_top + (self.last_delta as u32).min(eff_h);
        let new_start_actual = new_start + tpl_h;
        if new_start_actual >= eff_bottom {
            self.last_edges = Some(edges);
            return Ok(false);
        }

        let new_h = eff_bottom - new_start_actual;
        let new_rows = image::imageops::crop_imm(frame, 0, new_start_actual, w, new_h).to_image();

        let old_h = self.canvas.height();
        let mut combined = RgbaImage::new(w, old_h + new_rows.height());
        combined.copy_from(&self.canvas, 0, 0).context("canvas copy")?;
        combined.copy_from(&new_rows, 0, old_h).context("new_rows copy")?;
        self.canvas = combined;

        self.last_edges = Some(edges);
        Ok(true)
    }

    pub fn canvas(&self) -> &RgbaImage { &self.canvas }
    pub fn height(&self) -> u32 { self.canvas.height() }

    fn detect_sticky_and_active(&mut self, frame_a: &RgbaImage, frame_b: &RgbaImage) {
        let (w, h) = (frame_a.width(), frame_a.height());

        let mut sticky_t = 0u32;
        for y in 0..h.min(100) {
            if rows_equal(frame_a, frame_b, y, y, w) { sticky_t = y + 1; }
            else { break; }
        }

        let mut sticky_b = 0u32;
        for y in 0..h.min(100) {
            let ya = h - 1 - y;
            if rows_equal(frame_a, frame_b, ya, ya, w) { sticky_b = y + 1; }
            else { break; }
        }
        self.sticky_top = sticky_t;
        self.sticky_bottom = sticky_b;

        let mut min_col = w;
        let mut max_col = 0u32;
        for x in 0..w {
            if !cols_equal(frame_a, frame_b, x, h) {
                min_col = min_col.min(x);
                max_col = max_col.max(x);
            }
        }
        if min_col <= max_col {
            self.active_cols = min_col..max_col + 1;
        }

        // 寻找 active_cols 中边缘投影最密集的 200px 列作为匹配特征列
        let edges_b = compute_edges(frame_b);
        let col_w = self.active_cols.end - self.active_cols.start;
        if col_w > 200 {
            let mut best_x = self.active_cols.start;
            let mut max_sum = 0u64;
            let limit = self.active_cols.end.saturating_sub(200);
            for x in (self.active_cols.start..=limit).step_by(4) {
                let mut sum = 0u64;
                for y in (self.sticky_top..h.saturating_sub(self.sticky_bottom)).step_by(4) {
                    for tx in 0..200 {
                        sum += edges_b.get_pixel(x + tx, y)[0] as u64;
                    }
                }
                if sum > max_sum {
                    max_sum = sum;
                    best_x = x;
                }
            }
            self.match_cols = best_x..(best_x + 200);
        } else {
            self.match_cols = self.active_cols.clone();
        }
    }

    fn is_duplicate(&self, frame: &RgbaImage) -> bool {
        let last = match &self.last_edges {
            Some(e) => e,
            None => return false,
        };
        let curr = compute_edges(frame);
        let step = 8u32;
        let mut diff_sum = 0u64;
        let mut count = 0u64;
        for y in (0..last.height()).step_by(step as usize) {
            for x in (0..last.width()).step_by(step as usize) {
                let a = last.get_pixel(x, y)[0] as i32;
                let b = curr.get_pixel(x, y)[0] as i32;
                diff_sum += (a - b).unsigned_abs() as u64;
                count += 1;
            }
        }
        if count == 0 { return false; }
        let mean = diff_sum as f64 / count as f64;
        mean < 2.0
    }

    fn match_template(
        &self,
        last: &GrayImage, curr: &GrayImage,
        tpl_top: u32, eff_top: u32, eff_bottom: u32, tpl_h: u32, w: u32,
        last_dy: i32,
    ) -> (i32, f32) {
        let full_end = eff_bottom - eff_top - tpl_h;

        // 1. 局部跟踪模式 (Tracking mode)
        // dy 候选范围限制在 [last_dy - 20 ..= last_dy + 80]，可杜绝高度重复结构（如列表行）在全局范围找到其他周期性假匹配的问题
        let dy_start = last_dy - 20;
        let dy_end = last_dy + 80;

        let mut best_delta = 0;
        let mut best_score = -1.0f32;

        for dy in dy_start..=dy_end {
            let d = tpl_top as i32 - dy - eff_top as i32;
            if d < 0 || d > full_end as i32 { continue; }
            let score = ncc_score(last, curr, tpl_top, eff_top + d as u32, w, tpl_h, &self.match_cols);
            if score > best_score {
                best_score = score;
                best_delta = d;
            }
        }

        // 2. 重新捕获模式 (Re-acquisition mode)：若局部置信度太低，可能是大步长跳转或失锁，退回到全局搜索
        if best_score < self.config.min_confidence {
            for d in 0..=full_end {
                let score = ncc_score(last, curr, tpl_top, eff_top + d, w, tpl_h, &self.match_cols);
                if score > best_score {
                    best_score = score;
                    best_delta = d as i32;
                }
            }
        }

        (best_delta, best_score)
    }
}

fn compute_edges(img: &RgbaImage) -> GrayImage {
    let gray = image::imageops::grayscale(img);
    let grad_u16 = sobel_gradients(&gray);
    // sobel_gradients returns Luma<u16>, convert to Luma<u8>
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

fn cols_equal(a: &RgbaImage, b: &RgbaImage, x: u32, h: u32) -> bool {
    for y in 0..h {
        if a.get_pixel(x, y) != b.get_pixel(x, y) { return false; }
    }
    true
}

fn ncc_score(
    last: &GrayImage, curr: &GrayImage,
    tpl_y: u32, tgt_y: u32, w: u32, tpl_h: u32, cols: &Range<u32>,
) -> f32 {
    if tpl_h == 0 || cols.is_empty() { return 0.0; }
    
    // 限制最大匹配宽度为 200 像素，避免 Retina 屏幕下过宽导致 CPU 算力饱和
    let col_w_raw = cols.end - cols.start;
    let cols_selected = if col_w_raw > 200 {
        let mid = (cols.start + cols.end) / 2;
        (mid - 100)..(mid + 100)
    } else {
        cols.clone()
    };

    let col_w = cols_selected.end - cols_selected.start;
    let n = (tpl_h * col_w) as f64;
    if n < 1.0 { return 0.0; }

    let mut sum_t = 0.0f64;
    let mut sum_c = 0.0f64;
    for y in 0..tpl_h {
        for x in cols_selected.clone() {
            if x >= w { continue; }
            sum_t += last.get_pixel(x, tpl_y + y)[0] as f64;
            sum_c += curr.get_pixel(x, tgt_y + y)[0] as f64;
        }
    }
    let mean_t = sum_t / n;
    let mean_c = sum_c / n;

    let mut num = 0.0f64;
    let mut den_t = 0.0f64;
    let mut den_c = 0.0f64;
    for y in 0..tpl_h {
        for x in cols_selected.clone() {
            if x >= w { continue; }
            let t = last.get_pixel(x, tpl_y + y)[0] as f64 - mean_t;
            let c = curr.get_pixel(x, tgt_y + y)[0] as f64 - mean_c;
            num += t * c;
            den_t += t * t;
            den_c += c * c;
        }
    }
    let denom = (den_t * den_c).sqrt();
    if denom < 1e-6 { return 0.0; }
    (num / denom) as f32
}

fn find_best_template_y(
    edges: &GrayImage,
    search_range: Range<u32>,
    tpl_h: u32,
    w: u32,
    cols: &Range<u32>,
) -> u32 {
    let mut best_y = search_range.start;
    let mut max_sum = 0u64;

    let col_w_raw = cols.end - cols.start;
    let cols_selected = if col_w_raw > 200 {
        let mid = (cols.start + cols.end) / 2;
        (mid - 100)..(mid + 100)
    } else {
        cols.clone()
    };

    for y in search_range.step_by(2) {
        if y + tpl_h > edges.height() { break; }
        let mut sum = 0u64;
        for ty in 0..tpl_h {
            for x in cols_selected.clone().step_by(4) {
                if x >= w { continue; }
                sum += edges.get_pixel(x, y + ty)[0] as u64;
            }
        }
        if sum > max_sum {
            max_sum = sum;
            best_y = y;
        }
    }
    best_y
}


#[cfg(test)]
mod tests {
    use super::*;
    use image::Rgba;

    #[test]
    fn test_stitch_identical_frame() {
        let frame = RgbaImage::from_pixel(100, 200, Rgba([255, 0, 0, 255]));
        let mut stitcher = Stitcher::new(frame.clone(), StitchConfig::default());
        let result = stitcher.process_frame(&frame).unwrap();
        assert!(!result, "Identical frame should be skipped");
    }

    #[test]
    fn test_stitch_offset() {
        // 创建有丰富纹理的图像（棋盘格 + 渐变），滚动 50px
        let mut frame_a = RgbaImage::new(100, 200);
        for y in 0..200 {
            for x in 0..100 {
                let checker = if (x / 10 + y / 10) % 2 == 0 { 200u8 } else { 50u8 };
                let v = ((x as u16 + y as u16) % 256) as u8;
                frame_a.put_pixel(x, y, Rgba([checker, v, 128, 255]));
            }
        }
        // frame_b = frame_a 向上滚动 50px
        let mut frame_b = RgbaImage::new(100, 200);
        for y in 0..200 {
            for x in 0..100 {
                let src_y = (y + 50).min(199);
                frame_b.put_pixel(x, y, frame_a.get_pixel(x, src_y).clone());
            }
        }
        let mut stitcher = Stitcher::new(frame_a, StitchConfig::default());
        let result = stitcher.process_frame(&frame_b).unwrap();
        assert!(result, "Offset frame should produce new content");
        assert!(stitcher.height() > 200, "Canvas should grow, got {}", stitcher.height());
    }
}
