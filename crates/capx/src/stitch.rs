use anyhow::{Context, Result};
use image::{GrayImage, GenericImage, RgbaImage};

// ===== 拼接算法常量（原散落在 find_overlap_spatial_ext 与 process_frame 中的魔法数字）=====

/// 模板条高度（像素）。从参考帧底部取此高度的条带做空间模板匹配。
const STRIP_H: u32 = 80;
/// 全量搜索范围（像素）。`process_frame` 中限制滚动位移搜索上界。
const MAX_SCROLL: u32 = 220;
/// 静止判定阈值。dy=0 处的平均像素差值小于此值视为内容未滚动。
const STATIONARY_SAD: f64 = 2.0;
/// 匹配接受阈值。最佳 SAD 必须小于此值才接受拼接。
const SAD_ACCEPT: f64 = 7.5;
/// 置信度下限。估计置信度必须大于此值才接受拼接。
const MIN_CONFIDENCE: f64 = 0.15;
/// 软速度罚分系数。拉近与上一帧速度的距离，防止周期跳变。
const SPEED_PENALTY: f64 = 0.04;
/// 排除最左侧的比例（通常有图标/树状图）。
const X_START_RATIO: f64 = 0.10;
/// 排除最右侧的比例截止点（通常有滚动条/时间戳），即保留 10%~80% 横向区间。
const X_END_RATIO: f64 = 0.80;
/// 列抽样步长（像素）。每隔此值采样一列，提供双倍空间特征解析度。
const SAMPLE_STEP_X: usize = 2;
/// sticky 区域检测的最大高度（像素），顶部/底部各扫此高度。
const STICKY_DETECT_MAX: u32 = 80;

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

/// 滚动截屏拼接器——全局 2D 空间模板匹配 (SAD) + 软速度罚分 + Finalize 补缝。
pub struct Stitcher {
    canvas: RgbaImage,
    /// 2D 灰度参考帧，用于空间模板匹配
    reference_gray: GrayImage,
    sticky_top: u32,
    sticky_bottom: u32,
    detected: bool,
    config: StitchConfig,
    /// 上一次成功拼接的滚动位移，用于软速度罚分防止周期跳变
    last_dy: Option<f64>,
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
            last_dy: None,
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
        
        let x_start = (w as f64 * X_START_RATIO) as u32;
        let x_end = (w as f64 * X_END_RATIO) as u32;

        let max_scroll = MAX_SCROLL;
        let (dy, confidence) = match find_overlap_spatial_ext(
            &self.reference_gray,
            &curr_gray,
            x_start,
            x_end,
            eff_top,
            eff_bottom,
            max_scroll,
            self.last_dy,
        ) {
            Some(v) => v,
            None => {
                log::info!("[stitch] find_overlap_spatial returned None");
                self.last_dy = None;
                return Ok(false);
            }
        };

        // dy < 0 = 用户向下滚动（内容上移），dy > 0 = 向上滚动（忽略）
        if dy >= 0.0 {
            log::info!("[stitch] skipped frame: dy={:.1} >= 0.0 (conf={:.4})", dy, confidence);
            self.last_dy = None;
            return Ok(false);
        }

        let new_rows = (-dy).round() as u32;
        let max_scroll_limit = (eff_bottom - eff_top) * 4 / 5; // 允许最大滚动比例扩大到 80%

        // 静止或滚动超过限额
        if new_rows < self.config.min_scroll_px as u32 || new_rows >= max_scroll_limit {
            log::info!("[stitch] skipped frame: new_rows={} invalid (min={}, max={}) (conf={:.4})", new_rows, self.config.min_scroll_px, max_scroll_limit, confidence);
            self.last_dy = None;
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

        // 更新参考灰度图与速度缓存
        self.reference_gray = curr_gray;
        self.last_dy = Some(dy);

        Ok(true)
    }

    pub fn canvas(&self) -> &RgbaImage { &self.canvas }
    pub fn height(&self) -> u32 { self.canvas.height() }

    pub fn finalize(&mut self, last_frame: &RgbaImage) -> Result<()> {
        let h = last_frame.height();
        let w = last_frame.width();
        let eff_top = self.sticky_top;
        let eff_bottom = h.saturating_sub(self.sticky_bottom);
        if eff_bottom <= eff_top {
            return Ok(());
        }

        // 1. 尝试将最后一帧与参考帧对齐，补全因为丢帧/快速滑动积累的剩余未拼接区域
        let last_gray = image::imageops::grayscale(last_frame);
        let x_start = (w as f64 * X_START_RATIO) as u32;
        let x_end = (w as f64 * X_END_RATIO) as u32;

        // 允许最大对齐位移为有效高度的 90%
        let max_finalize_scroll = ((eff_bottom - eff_top) as f64 * 0.90) as u32;
        if let Some((dy, confidence)) = find_overlap_spatial_ext(
            &self.reference_gray,
            &last_gray,
            x_start,
            x_end,
            eff_top,
            eff_bottom,
            max_finalize_scroll,
            None, // 最后一帧匹配不施加速度限制
        ) {
            if dy < 0.0 {
                let new_rows = (-dy).round() as u32;
                if new_rows < eff_bottom - eff_top {
                    log::info!("[stitch] finalize: stitching remaining {} rows (conf={:.4})", new_rows, confidence);
                    let crop_y = eff_bottom - new_rows;
                    let new_content = image::imageops::crop_imm(last_frame, 0, crop_y, w, new_rows).to_image();
                    let old_h = self.canvas.height();
                    let mut combined = RgbaImage::new(w, old_h + new_rows);
                    combined.copy_from(&self.canvas, 0, 0).context("finalize copy canvas")?;
                    combined.copy_from(&new_content, 0, old_h).context("finalize copy new_rows")?;
                    self.canvas = combined;
                }
            }
        }

        // 2. 补全最后一帧的 sticky_bottom 区域
        let footer_h = h - eff_bottom;
        if footer_h > 0 {
            let footer = image::imageops::crop_imm(last_frame, 0, eff_bottom, w, footer_h).to_image();
            let old_h = self.canvas.height();
            let mut combined = RgbaImage::new(w, old_h + footer_h);
            combined.copy_from(&self.canvas, 0, 0).context("finalize canvas footer")?;
            combined.copy_from(&footer, 0, old_h).context("finalize footer")?;
            self.canvas = combined;
        }

        Ok(())
    }

    fn detect_sticky(&mut self, frame: &RgbaImage) {
        let (w, ch) = (self.canvas.width(), self.canvas.height());
        let fh = frame.height();
        let cmp_h = ch.min(fh);
        let mut sticky_t = 0u32;
        for y in 0..cmp_h.min(STICKY_DETECT_MAX) {
            if rows_equal(&self.canvas, frame, y, y, w) { sticky_t = y + 1; }
            else { break; }
        }
        let mut sticky_b = 0u32;
        for y in 0..cmp_h.min(STICKY_DETECT_MAX) {
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
fn find_overlap_spatial_ext(
    ref_img: &GrayImage,
    curr_img: &GrayImage,
    x_start: u32,
    x_end: u32,
    eff_top: u32,
    eff_bottom: u32,
    max_scroll: u32,
    last_dy: Option<f64>,
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

    // 限制滚动搜索的位移范围
    let min_y_offset = (template_y as i32 - max_scroll as i32).max(eff_top as i32) as u32;
    let max_y_offset = template_y;

    let mut best_y_offset = 0u32;
    let mut min_penalized_sad = f64::MAX;
    let mut best_original_sad = f64::MAX;

    // 在指定范围内查找 SAD 最小 of 偏移点
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
        let original_sad = sad / count;
        let mut penalized_sad = original_sad;
        
        // 软速度罚分 (Regularization)：拉近与上一帧速度的距离，防止在快速滚动与模糊行中误跳变到邻近周期行
        if let Some(ldy) = last_dy {
            let dy = y_offset as f64 - template_y as f64;
            let penalty = 0.04 * (dy - ldy).abs();
            penalized_sad += penalty;
        }

        if penalized_sad < min_penalized_sad {
            min_penalized_sad = penalized_sad;
            best_original_sad = original_sad;
            best_y_offset = y_offset;
        }
    }

    // 对比静止锚点：如果当前帧在 dy = 0 处的对齐误差比搜索到的最佳值还要小（或者几乎一样小），
    // 说明真实的位移其实是 0.0（静止状态），搜索窗口内的最小值只是周期性假匹配。
    if avg_sad_0 < best_original_sad + 1.0 {
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
        confidence = 1.0 - (best_original_sad / mean_sad);
    }

    // 限制匹配质量：只接收对齐良好的对齐帧 (SAD < 7.5 且 confidence > 0.15)
    // 2D 匹配由于极强的文字排布空间唯一性与软速度罚分保护，SAD < 7.5 即可确保在各种滚动状态下都十分精准
    if best_original_sad < 7.5 && confidence > 0.15 {
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

#[cfg(test)]
mod tests {
    use super::*;
    use image::{ImageBuffer, Rgba};

    /// 合成 RGBA 测试帧：宽 W × 高 H，包含可识别空间特征以便 SAD 匹配。
    /// - 背景按 y 线性渐变（值 = y % 256），提供垂直方向唯一性
    /// - 每 45 行一条强对比水平线（模拟文件列表行高），值翻转
    /// - 每 7 列一个亮列（模拟文字竖排），提供水平方向特征
    /// - 叠加少量确定性格点噪点（非随机，保证测试可复现）
    ///
    /// `scroll_offset` 模拟"用户向下滚动 scroll_offset 像素"：
    /// 即内容整体上移 scroll_offset，顶部 scroll_offset 行用新内容填充。
    fn make_frame(width: u32, height: u32, scroll_offset: u32) -> RgbaImage {
        let mut img: ImageBuffer<Rgba<u8>, Vec<u8>> = ImageBuffer::new(width, height);
        for y in 0..height {
            for x in 0..width {
                // 基础渐变（y 方向唯一）
                let mut v = ((y + scroll_offset) % 256) as u8;
                // 每 45 行水平分隔线：强对比
                if (y + scroll_offset) % 45 == 0 {
                    v = 255 - v;
                }
                // 每 7 列亮列
                if x % 7 == 0 {
                    v = v.saturating_add(80);
                }
                // 确定性格点噪点（(x*3+y*5) % 11 == 0 处加亮）
                if (x as u32 * 3 + (y + scroll_offset) * 5) % 11 == 0 {
                    v = v.saturating_add(40);
                }
                let px = Rgba([v, v, v, 255]);
                img.put_pixel(x, y, px);
            }
        }
        img
    }

    /// 构造一个带 sticky 顶/底区域的帧：顶部 `top_h` 行和底部 `bot_h` 行固定不变，
    /// 中间内容随 `scroll_offset` 变化。
    fn make_frame_with_sticky(
        width: u32,
        height: u32,
        top_h: u32,
        bot_h: u32,
        scroll_offset: u32,
    ) -> RgbaImage {
        let mut img = make_frame(width, height, scroll_offset);
        // 顶部 sticky：固定内容（与 scroll_offset 无关）
        let sticky_top = make_frame(width, top_h, 999);
        // 底部 sticky
        let sticky_bot = make_frame(width, bot_h, 888);
        for y in 0..top_h {
            for x in 0..width {
                img.put_pixel(x, y, sticky_top.get_pixel(x, y).clone());
            }
        }
        for y in 0..bot_h {
            for x in 0..width {
                img.put_pixel(x, height - bot_h + y, sticky_bot.get_pixel(x, y).clone());
            }
        }
        img
    }

    // 行为测试
    /// 默认测试尺寸：宽度需足够大让 X_START_RATIO..X_END_RATIO 区间有意义，
    /// 高度需远大于 STRIP_H + MAX_SCROLL。
    const TW: u32 = 400;
    const TH: u32 = 600;

    #[test]
    fn test_stationary_frame_returns_false() {
        // 两帧完全相同 → 无滚动，process_frame 返回 Ok(false)
        let f0 = make_frame(TW, TH, 0);
        let mut s = Stitcher::new(f0.clone(), StitchConfig::default());
        // 第一帧用于初始化（detect_sticky + reference），返回 false
        let f1 = make_frame(TW, TH, 0);
        let added = s.process_frame(&f1).unwrap();
        assert!(!added, "静止帧不应追加内容");
    }

    #[test]
    fn test_known_scroll_appends_rows() {
        // 首帧 scroll=0，init 帧 scroll=0（建立 reference），第三帧 scroll=40
        // 期望：第二次 process_frame 返回 true，canvas 高度增加约 40px
        let f0 = make_frame(TW, TH, 0);
        let mut s = Stitcher::new(f0.clone(), StitchConfig::default());
        // 第一次调用：初始化（detect_sticky + reference），返回 false
        let f1 = make_frame(TW, TH, 0);
        s.process_frame(&f1).unwrap();
        // 第二次调用：实际滚动检测
        let f2 = make_frame(TW, TH, 40);
        let added = s.process_frame(&f2).unwrap();
        assert!(added, "滚动 40px 应追加内容");
        let h_after = s.height();
        assert!(
            h_after > TH - STRIP_H,
            "追加后画布高度 {} 应大于裁剪后首帧高度，表示有新行追加",
            h_after
        );
    }

    #[test]
    fn test_scroll_direction_dy_negative() {
        // 验证 dy 符号约定：用户向下滚 → dy < 0。
        let f0 = make_frame(TW, TH, 0);
        let mut s = Stitcher::new(f0, StitchConfig::default());
        let f1 = make_frame(TW, TH, 0);
        s.process_frame(&f1).unwrap(); // init
        let f2 = make_frame(TW, TH, 30);
        let added = s.process_frame(&f2).unwrap();
        assert!(added, "向下滚 30px 应被接受（dy<0）");
    }

    #[test]
    fn test_repeated_scroll_grows_canvas() {
        // 连续多次小步滚动，画布应单调增长
        let f0 = make_frame(TW, TH, 0);
        let mut s = Stitcher::new(f0, StitchConfig::default());
        let f1 = make_frame(TW, TH, 0);
        s.process_frame(&f1).unwrap(); // init
        let mut last_h = s.height();
        for offset in (30..=150).step_by(30) {
            let f = make_frame(TW, TH, offset);
            if s.process_frame(&f).unwrap() {
                let h = s.height();
                assert!(h >= last_h, "画布高度不应回退：{} -> {}", last_h, h);
                last_h = h;
            }
        }
        assert!(last_h > TH, "多次滚动后画布应显著增长：{}", last_h);
    }

    #[test]
    fn test_canvas_returns_valid_rgba() {
        // canvas() 返回的 RgbaImage 可 clone，尺寸与 height() 一致
        let f0 = make_frame(TW, TH, 0);
        let mut s = Stitcher::new(f0, StitchConfig::default());
        let f1 = make_frame(TW, TH, 0);
        s.process_frame(&f1).unwrap(); // init
        let f2 = make_frame(TW, TH, 50);
        s.process_frame(&f2).unwrap();
        let canvas = s.canvas().clone();
        assert_eq!(canvas.height(), s.height());
        assert_eq!(canvas.width(), TW);
    }

    #[test]
    fn test_finalize_appends_footer() {
        // finalize 应补全最后一帧的 sticky_bottom 区域，画布高度应 >= finalize 前
        let f0 = make_frame(TW, TH, 0);
        let mut s = Stitcher::new(f0, StitchConfig::default());
        let f1 = make_frame(TW, TH, 0);
        s.process_frame(&f1).unwrap(); // init
        let f2 = make_frame(TW, TH, 60);
        s.process_frame(&f2).unwrap();
        let h_before = s.height();
        let last = make_frame(TW, TH, 90);
        s.finalize(&last).unwrap();
        let h_after = s.height();
        assert!(h_after >= h_before, "finalize 不应缩减画布：{} -> {}", h_before, h_after);
    }
}
