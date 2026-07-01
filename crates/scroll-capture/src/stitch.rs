use anyhow::{Context, Result};
use image::{GrayImage, GenericImage, RgbaImage};

pub struct StitchConfig {
    /// 最小有效滚动位移（像素）。低于此值视为静止。
    pub min_scroll_px: f64,
    /// 置信度阈值 (空间匹配)
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

/// 滚动截屏拼接器——无状态全局 2D 空间模板匹配 (SAD)。
///
/// 每次新帧到来时，在全量区间 [-220, 0] 内进行 2D 块匹配。
/// 相比 1D 投影或局部速度跟踪，全局 2D 空间匹配充分利用了字符的 2D 独特排布，
/// 使得真正的对齐点总是具有绝对最小的 SAD，从根本上消除了对速度预测和精细调参的依赖。
pub struct Stitcher {
    canvas: RgbaImage,
    /// 2D 灰度参考帧，用于空间模板匹配
    reference_gray: GrayImage,
    sticky_top: u32,
    sticky_bottom: u32,
    detected: bool,
    config: StitchConfig,
}

impl Stitcher {
    pub fn new(first_frame: RgbaImage, config: StitchConfig) -> Self {
        Self {
            canvas: first_frame,
            reference_gray: GrayImage::new(0, 0),
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
            // 裁掉画布（首帧）的 sticky 区域，只保留有效内容
            let eff_top0 = self.sticky_top;
            let eff_bottom0 = self.canvas.height().saturating_sub(self.sticky_bottom);
            let w = self.canvas.width();
            if eff_bottom0 > eff_top0 {
                // 仅裁掉底部的 sticky_bottom 区域，保留顶部的 sticky_top 区域
                let cropped = image::imageops::crop_imm(&self.canvas.clone(), 0, 0, w, eff_bottom0).to_image();
                self.canvas = cropped;
            }
            // 用第二帧初始化参考帧灰度图
            self.reference_gray = image::imageops::grayscale(frame);
    
            return Ok(false); // 第二帧用于初始化，不拼接
        }

        let eff_top = self.sticky_top;
        let eff_bottom = h.saturating_sub(self.sticky_bottom);
        if eff_bottom <= eff_top {
            return Ok(false);
        }

        let curr_gray = image::imageops::grayscale(frame);
        
        // 排除最左侧的 10% (通常有图标/树状图) 和最右侧的 20% (通常有滚动条/时间戳)
        let x_start = (w as f64 * 0.10) as u32;
        let x_end = (w as f64 * 0.80) as u32;

        let (dy, confidence) = match find_overlap_spatial(
            &self.reference_gray,
            &curr_gray,
            x_start,
            x_end,
            eff_top,
            eff_bottom,
        ) {
            Some(v) => v,
            None => {
                log::info!("[stitch] find_overlap_spatial returned None");
                return Ok(false);
            }
        };

        // dy < 0 = 用户向下滚动（内容上移），dy > 0 = 向上滚动（忽略）
        if dy >= 0.0 {
            log::info!("[stitch] skipped frame: dy={:.1} >= 0.0 (conf={:.4})", dy, confidence);
            return Ok(false);
        }

        let new_rows = (-dy).round() as u32;
        let max_scroll = (eff_bottom - eff_top) * 2 / 3;

        // 静止或滚动超过限额
        if new_rows < self.config.min_scroll_px as u32 || new_rows >= max_scroll {
            log::info!("[stitch] skipped frame: new_rows={} invalid (min={}, max={}) (conf={:.4})", new_rows, self.config.min_scroll_px, max_scroll, confidence);
            return Ok(false);
        }

        log::info!("[stitch] dy={:.1} conf={:.4} new_rows={} eff=[{},{}] canvas_h={}",
            dy, confidence, new_rows, eff_top, eff_bottom, self.canvas.height());

        let crop_y = eff_bottom - new_rows;
        let new_content = image::imageops::crop_imm(frame, 0, crop_y, w, new_rows).to_image();

        let old_h = self.canvas.height();
        let mut combined = RgbaImage::new(w, old_h + new_rows);
        combined.copy_from(&self.canvas, 0, 0).context("canvas copy")?;
        combined.copy_from(&new_content, 0, old_h).context("new_rows copy")?;
        self.canvas = combined;

        // 更新参考灰度图为当前帧
        self.reference_gray = curr_gray;

        Ok(true)
    }

    pub fn canvas(&self) -> &RgbaImage { &self.canvas }
    pub fn height(&self) -> u32 { self.canvas.height() }

    pub fn finalize(&mut self, last_frame: &RgbaImage) -> Result<()> {
        // 补全最后一帧的 sticky_bottom 区域
        let h = last_frame.height();
        let w = last_frame.width();
        let eff_bottom = h.saturating_sub(self.sticky_bottom);
        if eff_bottom >= h { return Ok(()); }
        let footer_h = h - eff_bottom;
        let footer = image::imageops::crop_imm(last_frame, 0, eff_bottom, w, footer_h).to_image();
        let old_h = self.canvas.height();
        let mut combined = RgbaImage::new(w, old_h + footer_h);
        combined.copy_from(&self.canvas, 0, 0).context("finalize canvas")?;
        combined.copy_from(&footer, 0, old_h).context("finalize footer")?;
        self.canvas = combined;

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
    }
}

/// 空间域 2D 模板匹配算法，查找最匹配的垂直位移 dy。
/// 采用 SAD (Sum of Absolute Differences) 准则与列抽样加速，保留 2D 空间排布，彻底避免 1D 投影带来的周期列表混淆。
fn find_overlap_spatial(
    ref_img: &GrayImage,
    curr_img: &GrayImage,
    x_start: u32,
    x_end: u32,
    eff_top: u32,
    eff_bottom: u32,
) -> Option<(f64, f64)> {
    let strip_h = 80u32; // 模板条的高度，包含更多文本特征
    if eff_bottom - eff_top <= strip_h + 10 {
        return None;
    }
    let template_y = eff_bottom - strip_h;

    // 每隔 2 列采样一次，提供双倍的空间特征解析度，消除 Retina 屏幕亚像素渲染带来的对齐模糊
    let step_x = 2u32;

    // 先计算 dy = 0.0 (即 y_offset = template_y) 的平均像素差值作为静止锚点
    let mut sad_0 = 0.0;
    let mut count_0 = 0.0;
    for dy in 0..strip_h {
        let ref_y = template_y + dy;
        let curr_y = template_y + dy;
        for x in (x_start..x_end).step_by(step_x as usize) {
            let p_ref = ref_img.get_pixel(x, ref_y)[0] as f64;
            let p_curr = curr_img.get_pixel(x, curr_y)[0] as f64;
            sad_0 += (p_ref - p_curr).abs();
            count_0 += 1.0;
        }
    }
    let avg_sad_0 = sad_0 / count_0;
    
    // 如果 dy = 0 时的平均像素差值小于 2.0，说明内容基本没有发生滚动位移（静止状态）
    if avg_sad_0 < 2.0 {
        return Some((0.0, 1.0));
    }

    // 限制滚动搜索的最大位移，在全量 [0, 220] 像素内进行全局匹配，无需保存历史状态或担心预测失准
    let max_scroll = 220u32;
    let min_y_offset = (template_y as i32 - max_scroll as i32).max(eff_top as i32) as u32;
    let max_y_offset = template_y;

    let mut best_y_offset = 0u32;
    let mut min_sad = f64::MAX;

    // 在全量范围内查找 SAD 最小的偏移点
    for y_offset in min_y_offset..=max_y_offset {
        let mut sad = 0.0;
        let mut count = 0.0;
        for dy in 0..strip_h {
            let ref_y = template_y + dy;
            let curr_y = y_offset + dy;
            for x in (x_start..x_end).step_by(step_x as usize) {
                let p_ref = ref_img.get_pixel(x, ref_y)[0] as f64;
                let p_curr = curr_img.get_pixel(x, curr_y)[0] as f64;
                sad += (p_ref - p_curr).abs();
                count += 1.0;
            }
        }
        let avg_sad = sad / count;
        if avg_sad < min_sad {
            min_sad = avg_sad;
            best_y_offset = y_offset;
        }
    }

    // 对比静止锚点：如果当前帧在 dy = 0 处的对齐误差比搜索到的最佳值还要小（或者几乎一样小），
    // 说明真实的位移其实是 0.0（静止状态），搜索窗口内的最小值只是周期性假匹配。
    if avg_sad_0 < min_sad + 1.0 {
        return Some((0.0, 1.0));
    }

    // 估计置信度：评估最佳 SAD 与其他偏移处的平均 SAD 的差距比例
    let mut sum_sad = 0.0;
    let mut sample_count = 0.0;
    // 稀疏采样以快速计算均值
    for y_offset in (min_y_offset..=max_y_offset).step_by(10) {
        let mut sad = 0.0;
        let mut count = 0.0;
        for dy in (0..strip_h).step_by(2) {
            let ref_y = template_y + dy;
            let curr_y = y_offset + dy;
            for x in (x_start..x_end).step_by(step_x as usize * 2) {
                let p_ref = ref_img.get_pixel(x, ref_y)[0] as f64;
                let p_curr = curr_img.get_pixel(x, curr_y)[0] as f64;
                sad += (p_ref - p_curr).abs();
                count += 1.0;
            }
        }
        sum_sad += sad / count;
        sample_count += 1.0;
    }
    let mean_sad = sum_sad / sample_count;
    let mut confidence = 0.0;
    if mean_sad > 1e-5 {
        confidence = 1.0 - (min_sad / mean_sad);
    }

    // 限制匹配质量：只接收未发生严重运动模糊与屏幕拖影的清晰对齐帧 (SAD < 4.5 且 confidence > 0.20)
    // 2D 匹配由于极强的文字排布空间唯一性，SAD < 4.5 确保对齐点绝对精准，防止拖影帧误配导致丢行
    if min_sad < 4.5 && confidence > 0.20 {
        let dy = best_y_offset as f64 - template_y as f64;
        Some((dy, confidence))
    } else {
        None
    }
}

fn rows_equal(a: &RgbaImage, b: &RgbaImage, ya: u32, yb: u32, w: u32) -> bool {
    for x in 0..w {
        if a.get_pixel(x, ya) != b.get_pixel(x, yb) { return false; }
    }
    true
}
