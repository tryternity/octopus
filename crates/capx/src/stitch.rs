use anyhow::{Context, Result};
use image::{GrayImage, GenericImage, RgbaImage};
use imageproc::gradients::sobel_gradients;
use std::ops::Range;

pub struct StitchConfig {
    /// 模板高度占选区高度的比例
    pub template_ratio: f32,
    /// NCC 最低接受阈值
    pub min_confidence: f32,
}

impl Default for StitchConfig {
    fn default() -> Self {
        Self {
            template_ratio: 0.3,
            min_confidence: 0.55,
        }
    }
}

/// 滚动截屏拼接器。
///
/// 核心思路：每次新帧到来时，从画布底部取一个 strip（模板），
/// 在新帧中从上往下滑动找到 NCC 得分最高的位置，该位置即为"旧底部在新帧中的对应位置"。
/// 从该位置之后到新帧底部的内容就是真正新增的像素行，追加到画布。
///
/// 相比旧版（固定模板位置 + 双模板 + PLL 跟踪），此实现更简单更稳健：
/// - 始终用画布底部 strip 做模板（它是上一帧的真实内容，不会偏移）
/// - 全局搜索最佳匹配位置（不依赖惯性窗口，不会卡在错误的周期上）
/// - 置信度不够就跳过该帧（用户滚动太快或画面静止时不拼接）
pub struct Stitcher {
    canvas: RgbaImage,
    last_edges: GrayImage,
    match_cols: Range<u32>,
    last_scroll: i32,
    low_conf_streak: u32,
    sticky_top: u32,
    sticky_bottom: u32,
    detected: bool,
    config: StitchConfig,
}

impl Stitcher {
    pub fn new(first_frame: RgbaImage, config: StitchConfig) -> Self {
        let (w, _h) = (first_frame.width(), first_frame.height());
        let edges = compute_edges(&first_frame);
        Self {
            canvas: first_frame,
            last_edges: edges,
            match_cols: 0..w,
            last_scroll: 0,
            low_conf_streak: 0,
            sticky_top: 0,
            sticky_bottom: 0,
            detected: false,
            config,
        }
    }

    pub fn process_frame(&mut self, frame: &RgbaImage) -> Result<bool> {
        let (w, h) = (frame.width(), frame.height());

        // 第二帧时检测 sticky + 匹配列（只执行一次）
        if !self.detected {
            self.detect_sticky_and_match_cols(frame);
            self.detected = true;
        }

        // 重复帧检测：画面没动时 NCC 仍会匹配成功（score 高），但不应追加内容。
        // 用当前帧 edges 和 last_edges 的稀疏采样均值差判断是否重复。
        let curr_edges = compute_edges(frame);
        if self.is_duplicate_fast(&curr_edges) {
            return Ok(false);
        }

        let eff_top = self.sticky_top;
        let eff_bottom = h.saturating_sub(self.sticky_bottom);
        if eff_bottom <= eff_top {
            return Ok(false);
        }
        let eff_h = eff_bottom - eff_top;

        // 计算边缘密度的辅助闭包
        let calc_mean_edge = |edges: &GrayImage, tpl_y: u32, tpl_h: u32, w: u32, cols: &Range<u32>| -> f64 {
            if tpl_h == 0 || cols.is_empty() { return 0.0; }
            let mut sum = 0u64;
            let mut count = 0u64;
            for y in (0..tpl_h).step_by(2) {
                for x in cols.clone().step_by(2) {
                    if x >= w { continue; }
                    sum += edges.get_pixel(x, tpl_y + y)[0] as u64;
                    count += 1;
                }
            }
            if count == 0 { return 0.0; }
            sum as f64 / count as f64
        };

        // 模板高度
        let tpl_h = ((eff_h as f32 * self.config.template_ratio) as u32).max(20).min(eff_h / 2);

        // 模板：从 last_edges（上一帧 edges）底部向上寻找首个包含足够纹理（边缘密度 > 4.0）的 strip，
        // 避免底部的空白背景区导致 NCC 匹配退化或产生周期性假匹配。
        let mut tpl_y_start = eff_bottom.saturating_sub(tpl_h);
        let min_y = eff_top.max(eff_bottom.saturating_sub(eff_h / 2));
        for y in (min_y..=eff_bottom.saturating_sub(tpl_h)).rev().step_by(4) {
            let mean = calc_mean_edge(&self.last_edges, y, tpl_h, w, &self.match_cols);
            if mean > 4.0 {
                tpl_y_start = y;
                break;
            }
        }

        // 在当前帧的 [eff_top, eff_bottom - tpl_h] 范围内搜索模板的最佳匹配位置
        let search_end = eff_bottom.saturating_sub(tpl_h);

        let mut best_offset: i32 = -1;
        let mut best_score: f32 = -1.0;

        // 期望的当前帧匹配位置（基于上一帧的滚动位移速度）。
        // dy 是滚动位移（从上一帧到当前帧内容向上移动的像素数），正数表示向下滚动（内容向上移动）。
        let expected_offset = tpl_y_start as i32 - self.last_scroll;

        // 设定搜索窗口限制：
        // 1. 若为第一帧滚动 (last_scroll == 0)，由于没有历史速度，搜索一个稍宽的合理向下滚动区间 [0, 120] 像素，防止首帧即触发全局搜索；
        // 2. 若为后续帧，在期望位置附近限制在极窄窗口内（dy 变化在 [-20, +30] 像素内），物理上阻断对周期性重复文本行的误匹配。
        let (lo, hi) = if self.last_scroll == 0 {
            ((tpl_y_start as i32 - 120).max(eff_top as i32), (tpl_y_start as i32 + 10).min(search_end as i32))
        } else {
            ((expected_offset - 30).max(eff_top as i32), (expected_offset + 20).min(search_end as i32))
        };

        eprintln!("[stitch] tpl_y_start={} tpl_h={} expected_offset={} search=[{},{}] cols={:?}",
            tpl_y_start, tpl_h, expected_offset, lo, hi, self.match_cols);

        // 1. 局部搜索
        for offset in lo..=hi {
            let score = ncc_score(
                &self.last_edges, &curr_edges,
                tpl_y_start, offset as u32, w, tpl_h, &self.match_cols,
            );
            if score > best_score {
                best_score = score;
                best_offset = offset;
            }
        }

        // 2. Fallback to global search if local search confidence is too low
        if best_score < self.config.min_confidence {
            // Enforce a strict threshold (0.85) to avoid false matches on periodic text
            let global_min_conf = 0.85f32;
            best_score = -1.0;
            best_offset = -1;
            for offset in (eff_top as i32)..=(search_end as i32) {
                let score = ncc_score(
                    &self.last_edges, &curr_edges,
                    tpl_y_start, offset as u32, w, tpl_h, &self.match_cols,
                );
                if score > best_score {
                    best_score = score;
                    best_offset = offset;
                }
            }
            if best_score < global_min_conf {
                best_offset = -1;
            } else {
                eprintln!("[stitch] global search re-acquired: best_offset={} best_score={:.4}", best_offset, best_score);
            }
        }

        if best_offset < 0 {
            // Match failed: update low_conf_streak and reset template to re-sync if stuck
            self.low_conf_streak += 1;
            if self.low_conf_streak >= 3 {
                self.last_edges = curr_edges;
                self.last_scroll = 0;
                self.low_conf_streak = 0;
                eprintln!("[stitch] lost track for 3 frames, resetting template to current frame bottom");
            }
            return Ok(false);
        }

        self.low_conf_streak = 0;

        // best_offset 是模板（画布底部 strip）在当前帧中的匹配位置。
        // 从 best_offset + tpl_h 到 eff_bottom 是真正新增的内容。
        // crop_start 是真正新增内容在当前帧中的起始 Y 坐标。
        // 无论模板 tpl_y_start 在何处，crop_start 在数学上均等价于：best_offset + (eff_bottom - tpl_y_start)
        let crop_start = best_offset as u32 + (eff_bottom - tpl_y_start);
        if crop_start >= eff_bottom {
            return Ok(false);
        }

        let new_h = eff_bottom - crop_start;
        if new_h < 2 {
            return Ok(false); // 滚动位移微乎其微
        }

        let new_rows = image::imageops::crop_imm(frame, 0, crop_start, w, new_h).to_image();
        let old_h = self.canvas.height();
        let mut combined = RgbaImage::new(w, old_h + new_rows.height());
        combined.copy_from(&self.canvas, 0, 0).context("canvas copy")?;
        combined.copy_from(&new_rows, 0, old_h).context("new_rows copy")?;
        self.canvas = combined;

        // 更新滚动位移状态 (dy = tpl_y_start - best_offset)
        self.last_scroll = tpl_y_start as i32 - best_offset;
        self.last_edges = curr_edges;

        eprintln!(
            "[stitch] match@{} score={:.2} crop {}+{}→{} new={} canvas={}",
            best_offset, best_score, crop_start, tpl_h, eff_bottom, new_h, self.canvas.height()
        );

        Ok(true)
    }

    pub fn canvas(&self) -> &RgbaImage { &self.canvas }
    pub fn height(&self) -> u32 { self.canvas.height() }

    /// 录制结束时补全最后一帧的剩余内容（sticky footer 等）
    pub fn finalize(&mut self, last_frame: &RgbaImage) -> Result<()> {
        let (w, h) = (last_frame.width(), last_frame.height());
        // 简单方案：最后一帧从 sticky_top 到帧底部全部追加（可能重叠少量行，但保证不缺底部）
        let eff_bottom = h.saturating_sub(self.sticky_bottom);
        let crop_start = (eff_bottom as i32 - self.last_scroll).max(0) as u32;
        if crop_start >= h {
            return Ok(());
        }
        let new_h = h - crop_start;
        if new_h == 0 { return Ok(()); }
        let new_rows = image::imageops::crop_imm(last_frame, 0, crop_start, w, new_h).to_image();
        let old_h = self.canvas.height();
        let mut combined = RgbaImage::new(w, old_h + new_rows.height());
        combined.copy_from(&self.canvas, 0, 0).context("finalize canvas copy")?;
        combined.copy_from(&new_rows, 0, old_h).context("finalize new_rows copy")?;
        self.canvas = combined;
        eprintln!("[stitch] finalize: appended {} rows", new_h);
        Ok(())
    }

    /// 快速重复帧检测：稀疏采样比较 last_edges 和 curr_edges 的均值差。
    /// 均值差 < 阈值说明画面没动，不应拼接。
    fn is_duplicate_fast(&self, curr_edges: &GrayImage) -> bool {
        let last = &self.last_edges;
        let step = 8u32;
        let mut diff_sum = 0u64;
        let mut count = 0u64;
        for y in (0..last.height().min(curr_edges.height())).step_by(step as usize) {
            for x in (0..last.width()).step_by(step as usize) {
                let a = last.get_pixel(x, y)[0] as i32;
                let b = curr_edges.get_pixel(x, y)[0] as i32;
                diff_sum += (a - b).unsigned_abs() as u64;
                count += 1;
            }
        }
        if count == 0 { return false; }
        let mean = diff_sum as f64 / count as f64;
        mean < 3.0
    }

    fn detect_sticky_and_match_cols(&mut self, frame: &RgbaImage) {
        let (w, ch) = (self.canvas.width(), self.canvas.height());
        let fh = frame.height();
        // canvas 和 frame 高度可能不同（canvas 已增长），sticky 检测用两者最小高度
        let cmp_h = ch.min(fh);

        // sticky top/bottom：首帧（canvas）和当前帧之间逐行比较不变的行
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

        // Scan for the most edge-dense 200px columns in the bottom half of the active region.
        // This ensures that the selected columns contain rich text features exactly where the template is extracted,
        // preventing the matcher from selecting an empty/margin column that only contains horizontal scrollbar borders.
        let edges = compute_edges(frame);
        let mut best_x = 0u32;
        let mut max_sum = 0u64;
        let limit = w.saturating_sub(200);
        let scan_y_start = self.sticky_top + (fh.saturating_sub(self.sticky_top).saturating_sub(self.sticky_bottom)) / 2;
        let scan_y_end = fh.saturating_sub(self.sticky_bottom);
        if w > 200 && scan_y_end > scan_y_start {
            for x in (0..=limit).step_by(8) {
                let mut sum = 0u64;
                for y in (scan_y_start..scan_y_end).step_by(4) {
                    for tx in (0..200).step_by(2) {
                        sum += edges.get_pixel(x + tx, y)[0] as u64;
                    }
                }
                if sum > max_sum {
                    max_sum = sum;
                    best_x = x;
                }
            }
            self.match_cols = best_x..(best_x + 200).min(w);
        } else {
            self.match_cols = 0..w;
        }
        eprintln!("[stitch] sticky_top={} sticky_bottom={} match_cols={:?}",
            self.sticky_top, self.sticky_bottom, self.match_cols);
    }
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

fn ncc_score(
    last: &GrayImage, curr: &GrayImage,
    tpl_y: u32, tgt_y: u32, w: u32, tpl_h: u32, cols: &Range<u32>,
) -> f32 {
    if tpl_h == 0 || cols.is_empty() { return 0.0; }

    let n = (tpl_h * (cols.end - cols.start)) as f64;
    if n < 1.0 { return 0.0; }

    let mut sum_t = 0.0f64;
    let mut sum_c = 0.0f64;
    for y in 0..tpl_h {
        for x in cols.clone() {
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
        for x in cols.clone() {
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
