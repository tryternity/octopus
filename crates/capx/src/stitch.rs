use anyhow::Result;
use image::RgbaImage;
use std::collections::VecDeque;

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
/// 低置信度匹配（如 0.3-0.4）大概率是周期性假匹配，会腐蚀画布底部导致后续连锁失败。
const MIN_CONFIDENCE: f64 = 0.5;
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

// ===== 健壮性优化常量 =====

/// 时序平滑：静止判断的 dy 均值阈值（近 N 帧 |dy| 均值 < 此值 → 静止）
const STATIONARY_DY_THRESHOLD: f64 = 2.0;
/// dy 历史长度
const DY_HISTORY_LEN: usize = 8;
/// 纹理密度评估：水平梯度阈值
const TEXTURE_EDGE_THRESHOLD: i32 = 20;
/// 动态阈值：纹理密度奖励系数（texture ∈ [0,1] × 30 → 最多加 30）
const TEXTURE_BONUS_FACTOR: f64 = 30.0;
/// 动态阈值：历史基线倍数（sad_baseline × 1.5 + 5）
const SAD_BASELINE_MULTIPLIER: f64 = 1.5;
/// 动态阈值：历史基线 padding
const SAD_BASELINE_PADDING: f64 = 5.0;
/// 动态阈值：EMA 平滑系数
const SAD_BASELINE_ALPHA: f64 = 0.3;
/// 降级 2：缩小模板高度
const FALLBACK_STRIP_H: u32 = 40;
/// 降级 2：阈值放宽倍数
const FALLBACK_SAD_MULTIPLIER: f64 = 1.5;

/// 连续 row-major 灰度 buffer，替代 image::GrayImage。
/// 消除 get_pixel() 的坐标计算 + 边界检查开销，用整行切片直访。
#[derive(Clone)]
struct GrayBuf {
    data: Vec<u8>,
    width: usize,
    /// 该 buffer 的首行在原始图像中的 y 坐标。ROI 灰度转换时 > 0。
    y_offset: usize,
}

impl GrayBuf {
    /// 从 RGBA 图像的指定行范围 [y_start, y_end) 转换灰度（ROI 优化）。
    /// 仅转换需要参与匹配的行，减少 60%+ 的灰度计算量。
    fn from_rgba_roi(rgba: &RgbaImage, y_start: usize, y_end: usize) -> Self {
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
    fn row(&self, y: usize) -> &[u8] {
        let local_y = y - self.y_offset;
        &self.data[local_y * self.width..(local_y + 1) * self.width]
    }
}

/// 评估模板条区域的纹理密度（边缘像素占比）。
/// 复用 sample_cols 的相邻列对做水平差分，O(STRIP_H × n_cols)，开销极低。
fn estimate_texture_density(buf: &GrayBuf, sample_cols: &[usize], template_y: u32) -> f64 {
    let mut edge_count = 0u32;
    let mut total = 0u32;
    for dy in 0..STRIP_H {
        let row = buf.row((template_y + dy) as usize);
        for w in sample_cols.windows(2) {
            total += 1;
            if (row[w[0]] as i32 - row[w[1]] as i32).abs() > TEXTURE_EDGE_THRESHOLD {
                edge_count += 1;
            }
        }
    }
    if total == 0 { return 0.0; }
    edge_count as f64 / total as f64
}

/// 计算灰度 buffer 指定行范围 [y_start, y_end) 的每行抽样列均值，降为一维信号。
fn row_projection_means(buf: &GrayBuf, cols: &[usize], y_start: u32, y_end: u32) -> Vec<f64> {
    let n = (y_end - y_start) as usize;
    let mut proj = Vec::with_capacity(n);
    for y in y_start..y_end {
        let row = buf.row(y as usize);
        let sum: u64 = cols.iter().map(|&x| row[x] as u64).sum();
        proj.push(sum as f64 / cols.len() as f64);
    }
    proj
}

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
    canvas_w: u32,
    canvas_h: u32,
    /// 连续 RGBA 画布数据（真实数据源，增量 extend 追加）。
    canvas_buf: Vec<u8>,
    /// 惰性重建缓存。append 后置 None，canvas() 调用时按需重建。
    canvas_cache: Option<RgbaImage>,
    sticky_top: u32,
    sticky_bottom: u32,
    detected: bool,
    config: StitchConfig,
    /// 上一次成功拼接的滚动位移，用于软速度罚分防止周期跳变
    last_dy: Option<f64>,
    /// 最近若干帧的 dy 历史，用于时序平滑判断静止。
    dy_history: VecDeque<f64>,
    /// 历史成功匹配的 SAD 均值（EMA）。
    sad_baseline: f64,
    /// 连续 best-guess 次数（主匹配成功时归零，超过 3 次熔断）。
    best_guess_streak: u32,
}

impl Stitcher {
    pub fn new(first_frame: RgbaImage, config: StitchConfig) -> Self {
        let w = first_frame.width();
        let h = first_frame.height();
        Self {
            canvas_w: w,
            canvas_h: h,
            canvas_buf: first_frame.into_raw(),
            canvas_cache: None,
            sticky_top: 0,
            sticky_bottom: 0,
            detected: false,
            config,
            last_dy: None,
            dy_history: VecDeque::with_capacity(DY_HISTORY_LEN),
            sad_baseline: 0.0,
            best_guess_streak: 0,
        }
    }

    pub fn process_frame(&mut self, frame: &RgbaImage) -> Result<bool> {
        let (w, h) = (frame.width(), frame.height());

        // 防御性校验：帧宽度必须与画布一致，否则切片越界或数据污染
        if w != self.canvas_w {
            log::warn!("[stitch] frame width {} != canvas_w {}, skipping", w, self.canvas_w);
            return Ok(false);
        }

        if !self.detected {
            self.detect_sticky(frame);
            self.detected = true;
            // 裁掉画布（首帧）的 sticky_bottom 区域，保留 sticky_top。
            let eff_bottom0 = self.canvas_h.saturating_sub(self.sticky_bottom);
            if eff_bottom0 > self.sticky_top {
                self.canvas_buf.truncate(eff_bottom0 as usize * self.canvas_w as usize * 4);
                self.canvas_h = eff_bottom0;
                self.invalidate_cache();
            }
            // Canvas-Anchored：下一帧直接从 canvas 底部提取模板，无需存 reference

            return Ok(false); // 第二帧用于初始化，不拼接
        }

        let eff_top = self.sticky_top;
        let eff_bottom = h.saturating_sub(self.sticky_bottom);
        if eff_bottom <= eff_top {
            return Ok(false);
        }

        // ROI 灰度转换：覆盖最大可能搜索范围（含降级 1 的 ×2 扩大）
        let roi_top = eff_top.max(eff_bottom.saturating_sub(STRIP_H + MAX_SCROLL * 2)) as usize;
        let roi_bottom = eff_bottom as usize;
        let curr_buf = GrayBuf::from_rgba_roi(frame, roi_top, roi_bottom);
        let canvas_ref = self.extract_canvas_bottom_gray(STRIP_H);

        let x_start = (w as f64 * X_START_RATIO) as u32;
        let x_end = (w as f64 * X_END_RATIO) as u32;

        let max_scroll = MAX_SCROLL;

        // 动态阈值：根据画布底部纹理密度 + 历史基线计算
        let sample_cols: Vec<usize> = (x_start as usize..x_end as usize)
            .step_by(SAMPLE_STEP_X)
            .collect();
        let texture = estimate_texture_density(&canvas_ref, &sample_cols, 0);
        let sad_accept = self.dynamic_sad_accept(texture);

        let (dy, confidence, best_sad) = match find_overlap_spatial_ext(
            &canvas_ref,
            &curr_buf,
            x_start,
            x_end,
            eff_top,
            eff_bottom,
            max_scroll,
            self.last_dy,
            sad_accept,
            STRIP_H,
        ) {
            Some(v) => v,
            None => {
                // 三级降级链
                log::info!("[stitch] main match failed, entering fallback chain");

                // 降级 1：扩大搜索范围 ×2
                if let Some((dy, conf, sad)) = self.try_match(
                    &canvas_ref, &curr_buf, x_start, x_end, eff_top, eff_bottom, max_scroll * 2, sad_accept, STRIP_H,
                ) {
                    log::info!("[stitch] fallback 1: expanded search range, dy={:.1} conf={:.4}", dy, conf);
                    self.best_guess_streak = 0;
                    return self.apply_fallback_match(dy, conf, sad, frame, &curr_buf, w, eff_top, eff_bottom);
                }

                // 降级 2：缩小模板 + 放宽阈值
                if let Some((dy, conf, sad)) = self.try_match(
                    &canvas_ref, &curr_buf, x_start, x_end, eff_top, eff_bottom, max_scroll,
                    sad_accept * FALLBACK_SAD_MULTIPLIER, FALLBACK_STRIP_H,
                ) {
                    log::info!("[stitch] fallback 2: reduced strip height, dy={:.1} conf={:.4}", dy, conf);
                    self.best_guess_streak = 0;
                    return self.apply_fallback_match(dy, conf, sad, frame, &curr_buf, w, eff_top, eff_bottom);
                }

                // 降级 3：1D 灰度投影匹配
                if let Some((dy, conf, sad)) = self.try_match_1d_projection(
                    &canvas_ref, &curr_buf, x_start, x_end, eff_top, eff_bottom, max_scroll, sad_accept,
                ) {
                    log::info!("[stitch] fallback 3: 1D projection match, dy={:.1} conf={:.4}", dy, conf);
                    self.best_guess_streak = 0;
                    return self.apply_fallback_match(dy, conf, sad, frame, &curr_buf, w, eff_top, eff_bottom);
                }

                log::info!("[stitch] all fallbacks exhausted, trying best-guess");

                // 静止检测：匹配全失败时，先检查画面是否实际没动。
                // 计算当前帧底部 strip 与画布底部 strip 的全局 SAD，极低说明静止。
                let stationary_sad = self.quick_stationary_check(&curr_buf, &canvas_ref, &sample_cols);
                if stationary_sad < STATIONARY_SAD {
                    log::info!("[stitch] stationary detected before best-guess (sad={:.2}), clearing history", stationary_sad);
                    self.dy_history.clear();
                    self.best_guess_streak = 0;
                    self.last_dy = None;
                    return Ok(false);
                }

                // 降级 4：Best-Guess——用历史 dy 估算位移，宁可轻微错位也不丢内容。
                // 打破"匹配失败 → canvas 不长 → 位移差扩大 → 永久失败"的死亡螺旋。
                // 熔断：连续 best-guess 超过 3 次则停止猜测，避免拼出严重错位的长图。
                if self.best_guess_streak < 3 {
                    if let Some(dy) = self.estimate_dy_hint() {
                        log::info!("[stitch] fallback 4: best-guess dy={:.1} (streak={})", dy, self.best_guess_streak + 1);
                        self.best_guess_streak += 1;
                        return self.apply_fallback_match(dy, 0.0, 0.0, frame, &curr_buf, w, eff_top, eff_bottom);
                    }
                } else {
                    log::info!("[stitch] best-guess circuit breaker tripped (streak >= 3), skipping");
                }

                log::info!("[stitch] best-guess also unavailable, skipping frame");
                self.last_dy = None;
                return Ok(false);
            }
        };

        // 静止双重校验：dy ≈ 0 且时序也确认静止才跳过
        if dy.abs() < 0.5 && self.is_stationary() {
            log::info!("[stitch] stationary confirmed by temporal smoothing");
            // 写入 dy_history 持续稀释旧滚动速度，防止后续 best-guess 误触发
            self.dy_history.push_back(dy);
            if self.dy_history.len() > DY_HISTORY_LEN {
                self.dy_history.pop_front();
            }
            return Ok(false);
        }

        // dy < 0 = 用户向下滚动（内容上移），dy > 0 = 向上滚动（忽略）
        // dy=0（静止或慢速滚动）时保留 last_dy，维持速度上下文供下一帧搜索
        if dy >= 0.0 {
            log::info!("[stitch] skipped frame: dy={:.1} >= 0.0 (conf={:.4})", dy, confidence);
            // 更新 dy_history：静止帧(0)和向上滚动帧(>0)也写入历史，
            // 使中位数收敛到真实速度，防止停止后 best-guess 幽灵滚动
            self.dy_history.push_back(dy);
            if self.dy_history.len() > DY_HISTORY_LEN {
                self.dy_history.pop_front();
            }
            return Ok(false);
        }

        let new_rows = (-dy).round() as u32;
        let max_scroll_limit = (eff_bottom - eff_top) * 4 / 5; // 允许最大滚动比例扩大到 80%

        // 静止或滚动超过限额
        if new_rows < self.config.min_scroll_px as u32 || new_rows >= max_scroll_limit {
            log::info!("[stitch] skipped frame: new_rows={} invalid (min={}, max={}) (conf={:.4})", new_rows, self.config.min_scroll_px, max_scroll_limit, confidence);
            // 不清除 last_dy：微小位移帧不意味着速度上下文失效
            return Ok(false);
        }

        log::info!("[stitch] dy={:.1} conf={:.4} new_rows={} eff=[{},{}] canvas_h={}",
            dy, confidence, new_rows, eff_top, eff_bottom, self.canvas_h);

        // 主匹配成功：重置 best-guess 连续计数
        self.best_guess_streak = 0;

        // 增量追加：从 frame 直接切出 new_rows 行 RGBA，extend 到 canvas_buf
        let crop_y = eff_bottom - new_rows;
        let row_bytes = w as usize * 4;
        let start = crop_y as usize * row_bytes;
        let end = start + new_rows as usize * row_bytes;
        let frame_raw = frame.as_raw();
        self.canvas_buf.extend_from_slice(&frame_raw[start..end]);
        self.canvas_h += new_rows;
        self.invalidate_cache();

        // Canvas-Anchored：无需更新 reference（每帧从 canvas 提取）
        self.last_dy = Some(dy);

        // 更新 dy_history（时序平滑）和 sad_baseline（动态阈值 EMA）
        self.dy_history.push_back(dy);
        if self.dy_history.len() > DY_HISTORY_LEN {
            self.dy_history.pop_front();
        }
        if self.sad_baseline == 0.0 {
            self.sad_baseline = best_sad;
        } else {
            self.sad_baseline = SAD_BASELINE_ALPHA * best_sad + (1.0 - SAD_BASELINE_ALPHA) * self.sad_baseline;
        }

        Ok(true)
    }

    /// 使画布缓存失效。每次 append/truncate 后调用。
    #[inline]
    fn invalidate_cache(&mut self) {
        self.canvas_cache = None;
    }

    /// 从画布底部提取 strip_h 行 RGBA 转灰度，作为 Canvas-Anchored 匹配模板。
    /// 无论多少帧匹配失败，画布底部始终是最新已确认内容 → 消除累积漂移。
    fn extract_canvas_bottom_gray(&self, strip_h: u32) -> GrayBuf {
        let row_bytes = self.canvas_w as usize * 4;
        let start_row = self.canvas_h.saturating_sub(strip_h);
        let mut data = Vec::with_capacity(strip_h as usize * self.canvas_w as usize);
        for y in start_row..self.canvas_h {
            let row_start = y as usize * row_bytes;
            for x in 0..self.canvas_w as usize {
                let off = row_start + x * 4;
                let r = self.canvas_buf[off] as u32;
                let g = self.canvas_buf[off + 1] as u32;
                let b = self.canvas_buf[off + 2] as u32;
                let luma = (2126 * r + 7152 * g + 722 * b) / 10000;
                data.push(luma as u8);
            }
        }
        GrayBuf { data, width: self.canvas_w as usize, y_offset: 0 }
    }

    /// 根据当前帧纹理密度 + 历史 SAD 基线动态计算 SAD 接受阈值。
    fn dynamic_sad_accept(&self, texture: f64) -> f64 {
        // 纹理越丰富 → 绝对 SAD 天然更高 → 允许更高阈值
        let texture_bonus = texture * TEXTURE_BONUS_FACTOR;
        // 历史基线浮动：EMA 均值的倍数 + padding 作为上界
        let baseline_cap = self.sad_baseline * SAD_BASELINE_MULTIPLIER + SAD_BASELINE_PADDING;
        (SAD_ACCEPT + texture_bonus).min(baseline_cap).max(SAD_ACCEPT)
    }

    /// 轻量静止检测：比较当前帧底部 strip 与画布底部 strip 的全局 SAD。
    /// 用于 best-guess 前判断画面是否实际没动（如滚到底部）。
    fn quick_stationary_check(&self, curr: &GrayBuf, canvas_ref: &GrayBuf, sample_cols: &[usize]) -> f64 {
        let mut sad: u64 = 0;
        let mut count: u64 = 0;
        for dy in 0..STRIP_H {
            let ref_row = canvas_ref.row(dy as usize);
            // curr 底部 strip：y_offset + (curr 行数 - STRIP_H + dy)
            let curr_bottom_start = (curr.data.len() / curr.width).saturating_sub(STRIP_H as usize);
            let curr_row = curr.row((curr_bottom_start + dy as usize) + curr.y_offset);
            for &x in sample_cols {
                sad += (ref_row[x] as i32 - curr_row[x] as i32).unsigned_abs() as u64;
                count += 1;
            }
        }
        if count > 0 { sad as f64 / count as f64 } else { f64::MAX }
    }

    /// 判断当前是否为静止状态（基于历史 dy 均值）。
    /// 回弹帧 dy 可能抖动到 -3，但历史 [-15,-12,-10,-3] 均值 -10，不判静止。
    fn is_stationary(&self) -> bool {
        if self.dy_history.len() < 3 {
            return false; // 不足 3 帧，不判静止（让 SAD 主匹配决定）
        }
        let n = self.dy_history.len().min(5);
        let recent: f64 = self.dy_history.iter().rev().take(n).sum::<f64>() / n as f64;
        recent.abs() < STATIONARY_DY_THRESHOLD
    }

    /// 用历史 dy 中位数估算当前预期位移（Best-Guess 提示）。
    /// 当所有匹配策略失败时，用此值追加内容，打破死亡螺旋。
    fn estimate_dy_hint(&self) -> Option<f64> {
        if self.dy_history.len() < 2 {
            return None;
        }
        let mut recent: Vec<f64> = self.dy_history.iter().rev().take(5).copied().collect();
        recent.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let median = recent[recent.len() / 2];
        // 只在用户持续向下滚（median < 0）时提供 hint
        if median < -1.0 {
            Some(median)
        } else {
            None
        }
    }

    /// 主匹配封装：调用 find_overlap_spatial_ext。
    fn try_match(
        &self,
        ref_buf: &GrayBuf,
        curr: &GrayBuf,
        x_start: u32,
        x_end: u32,
        eff_top: u32,
        eff_bottom: u32,
        max_scroll: u32,
        sad_accept: f64,
        strip_h: u32,
    ) -> Option<(f64, f64, f64)> {
        find_overlap_spatial_ext(
            ref_buf,
            curr,
            x_start,
            x_end,
            eff_top,
            eff_bottom,
            max_scroll,
            self.last_dy,
            sad_accept,
            strip_h,
        )
    }

    /// 降级 3：1D 灰度投影匹配。
    /// 将每行像素按抽样列取均值降为一维信号，对一维信号做 SAD 搜索。
    /// 对纯色/低纹理场景（2D SAD 缺乏特征）更鲁棒。
    fn try_match_1d_projection(
        &self,
        ref_buf: &GrayBuf,
        curr: &GrayBuf,
        x_start: u32,
        x_end: u32,
        eff_top: u32,
        eff_bottom: u32,
        max_scroll: u32,
        sad_accept: f64,
    ) -> Option<(f64, f64, f64)> {
        let strip_h = STRIP_H;
        if eff_bottom <= eff_top + strip_h + 10 {
            return None;
        }
        // 防御性校验：ref_buf 必须至少有 strip_h 行
        if (ref_buf.data.len() / ref_buf.width) < strip_h as usize {
            return None;
        }
        let template_y = eff_bottom - strip_h;

        let cols: Vec<usize> = (x_start as usize..x_end as usize)
            .step_by(SAMPLE_STEP_X)
            .collect();
        if cols.is_empty() {
            return None;
        }

        let ref_proj = row_projection_means(ref_buf, &cols, 0, strip_h);
        let search_start = (template_y as i32 - max_scroll as i32).max(eff_top as i32) as u32;

        let mut best_offset = template_y;
        let mut min_sad = f64::MAX;
        let total = strip_h as f64;

        for y_offset in search_start..=template_y {
            let curr_proj = row_projection_means(curr, &cols, y_offset, y_offset + strip_h);
            let mut sad = 0.0f64;
            for i in 0..strip_h as usize {
                sad += (ref_proj[i] - curr_proj[i]).abs();
            }
            let sad_avg = sad / total;
            if sad_avg < min_sad {
                min_sad = sad_avg;
                best_offset = y_offset;
            }
        }

        // 静止检查
        let curr_proj_stationary = row_projection_means(curr, &cols, template_y, template_y + strip_h);
        let mut stationary_sad = 0.0f64;
        for i in 0..strip_h as usize {
            stationary_sad += (ref_proj[i] - curr_proj_stationary[i]).abs();
        }
        let stationary_sad_avg = stationary_sad / total;
        if stationary_sad_avg < STATIONARY_SAD {
            return Some((0.0, 1.0, 0.0));
        }

        // 置信度（1D 最佳与均值比）
        let mut sum_sad = 0.0f64;
        let mut count = 0.0f64;
        for y_offset in (search_start..=template_y).step_by(10) {
            let curr_proj = row_projection_means(curr, &cols, y_offset, y_offset + strip_h);
            let mut sad = 0.0f64;
            for i in 0..strip_h as usize {
                sad += (ref_proj[i] - curr_proj[i]).abs();
            }
            sum_sad += sad / total;
            count += 1.0;
        }
        let mean_sad = sum_sad / count;
        let confidence = if mean_sad > 1e-5 {
            1.0 - (min_sad / mean_sad)
        } else {
            0.0
        };

        // 1D 投影置信度要求更严（0.25 vs 0.15）
        if min_sad < sad_accept && confidence > 0.25 {
            let dy = best_offset as f64 - template_y as f64;
            Some((dy, confidence, min_sad))
        } else {
            None
        }
    }

    /// 降级匹配结果的处理（复用主匹配的 dy 检查 + 画布追加 + 状态更新）。
    fn apply_fallback_match(
        &mut self,
        dy: f64,
        _confidence: f64,
        best_sad: f64,
        frame: &RgbaImage,
        _curr_buf: &GrayBuf,
        w: u32,
        eff_top: u32,
        eff_bottom: u32,
    ) -> Result<bool> {
        if dy >= 0.0 {
            self.last_dy = None;
            return Ok(false);
        }
        let new_rows = (-dy).round() as u32;
        let max_scroll_limit = (eff_bottom - eff_top) * 4 / 5;
        if new_rows < self.config.min_scroll_px as u32 || new_rows >= max_scroll_limit {
            self.last_dy = None;
            return Ok(false);
        }

        let crop_y = eff_bottom - new_rows;
        let row_bytes = w as usize * 4;
        let start = crop_y as usize * row_bytes;
        let end = start + new_rows as usize * row_bytes;
        let frame_raw = frame.as_raw();
        self.canvas_buf.extend_from_slice(&frame_raw[start..end]);
        self.canvas_h += new_rows;
        self.invalidate_cache();

        // Canvas-Anchored：无需更新 reference
        self.last_dy = Some(dy);
        self.dy_history.push_back(dy);
        if self.dy_history.len() > DY_HISTORY_LEN {
            self.dy_history.pop_front();
        }
        // 更新 sad_baseline（仅当有真实 SAD 值时；best-guess 传 0.0 跳过）
        if best_sad > 0.0 {
            if self.sad_baseline == 0.0 {
                self.sad_baseline = best_sad;
            } else {
                self.sad_baseline = SAD_BASELINE_ALPHA * best_sad + (1.0 - SAD_BASELINE_ALPHA) * self.sad_baseline;
            }
        }

        Ok(true)
    }

    pub fn canvas(&mut self) -> &RgbaImage {
        if self.canvas_cache.is_none() {
            let rebuilt = RgbaImage::from_raw(self.canvas_w, self.canvas_h, self.canvas_buf.clone())
                .expect("canvas_buf 长度与 canvas_w/h 不匹配");
            self.canvas_cache = Some(rebuilt);
        }
        self.canvas_cache.as_ref().unwrap()
    }

    pub fn height(&self) -> u32 { self.canvas_h }

    pub fn finalize(&mut self, last_frame: &RgbaImage) -> Result<()> {
        let h = last_frame.height();
        let w = last_frame.width();
        // 防御性校验：帧宽度必须与画布一致
        if w != self.canvas_w {
            log::warn!("[stitch] finalize: frame width {} != canvas_w {}, skipping", w, self.canvas_w);
            return Ok(());
        }
        let eff_top = self.sticky_top;
        let eff_bottom = h.saturating_sub(self.sticky_bottom);
        if eff_bottom <= eff_top {
            return Ok(());
        }

        // 1. 尝试将最后一帧与画布底部对齐，补全因为丢帧/快速滑动积累的剩余未拼接区域
        // ROI 灰度转换：finalize 搜索范围达 90% 有效高度，需要覆盖全部有效行
        let last_buf = GrayBuf::from_rgba_roi(last_frame, eff_top as usize, eff_bottom as usize);
        let canvas_ref = self.extract_canvas_bottom_gray(STRIP_H);
        let x_start = (w as f64 * X_START_RATIO) as u32;
        let x_end = (w as f64 * X_END_RATIO) as u32;

        // 允许最大对齐位移为有效高度的 90%
        let max_finalize_scroll = ((eff_bottom - eff_top) as f64 * 0.90) as u32;
        if let Some((dy, confidence, _)) = find_overlap_spatial_ext(
            &canvas_ref,
            &last_buf,
            x_start,
            x_end,
            eff_top,
            eff_bottom,
            max_finalize_scroll,
            None, // 最后一帧匹配不施加速度限制
            SAD_ACCEPT,
            STRIP_H,
        ) {
            if dy < 0.0 {
                let new_rows = (-dy).round() as u32;
                if new_rows < eff_bottom - eff_top {
                    log::info!("[stitch] finalize: stitching remaining {} rows (conf={:.4})", new_rows, confidence);
                    let crop_y = eff_bottom - new_rows;
                    let row_bytes = w as usize * 4;
                    let start = crop_y as usize * row_bytes;
                    let end = start + new_rows as usize * row_bytes;
                    let frame_raw = last_frame.as_raw();
                    self.canvas_buf.extend_from_slice(&frame_raw[start..end]);
                    self.canvas_h += new_rows;
                    self.invalidate_cache();
                }
            }
        }

        // 2. 补全最后一帧的 sticky_bottom 区域
        let footer_h = h - eff_bottom;
        if footer_h > 0 {
            let row_bytes = w as usize * 4;
            let start = eff_bottom as usize * row_bytes;
            let end = start + footer_h as usize * row_bytes;
            let frame_raw = last_frame.as_raw();
            self.canvas_buf.extend_from_slice(&frame_raw[start..end]);
            self.canvas_h += footer_h;
            self.invalidate_cache();
        }

        Ok(())
    }

    fn detect_sticky(&mut self, frame: &RgbaImage) {
        let w = self.canvas_w;
        let ch = self.canvas_h;
        let fh = frame.height();
        let cmp_h = ch.min(fh);
        let mut sticky_t = 0u32;
        for y in 0..cmp_h.min(STICKY_DETECT_MAX) {
            if rows_equal_buf(&self.canvas_buf, w, frame, y, y) { sticky_t = y + 1; }
            else { break; }
        }
        let mut sticky_b = 0u32;
        for y in 0..cmp_h.min(STICKY_DETECT_MAX) {
            let ya = cmp_h - 1 - y;
            if rows_equal_buf(&self.canvas_buf, w, frame, ya, ya) { sticky_b = y + 1; }
            else { break; }
        }
        self.sticky_top = sticky_t;
        self.sticky_bottom = sticky_b;
    }
}

/// 空间域 2D 模板匹配算法，查找最匹配的垂直位移 dy。
/// 采用 SAD (Sum of Absolute Differences) 准则与列抽样加速，保留 2D 空间排布。
///
/// 优化：模板条预提取为连续 buffer；整数 u64 累加；切片直访（无 get_pixel 边界检查）；
/// 静止检测合并进主搜索（省一次预扫描）。
fn find_overlap_spatial_ext(
    ref_buf: &GrayBuf,
    curr_buf: &GrayBuf,
    x_start: u32,
    x_end: u32,
    eff_top: u32,
    eff_bottom: u32,
    max_scroll: u32,
    last_dy: Option<f64>,
    sad_accept: f64,
    strip_h: u32,
) -> Option<(f64, f64, f64)> {
    if eff_bottom <= eff_top + strip_h + 10 {
        return None;
    }
    // 防御性校验：ref_buf 必须至少有 strip_h 行，否则 row() 越界 panic
    if (ref_buf.data.len() / ref_buf.width) < strip_h as usize {
        return None;
    }
    let template_y = eff_bottom - strip_h;

    // 抽样列索引（只算一次）
    let sample_cols: Vec<usize> = (x_start as usize..x_end as usize)
        .step_by(SAMPLE_STEP_X)
        .collect();
    if sample_cols.is_empty() {
        return None;
    }

    // 模板条预提取（Canvas-Anchored：ref_buf 行号从 0 开始）
    let tpl = extract_template(ref_buf, 0, &sample_cols, strip_h);

    let min_y_offset = (template_y as i32 - max_scroll as i32).max(eff_top as i32) as u32;
    let max_y_offset = template_y;

    // 主搜索
    let (best_y_offset, best_sad_avg, stationary_sad_avg) = search_best_offset(
        &tpl,
        curr_buf,
        &sample_cols,
        min_y_offset,
        max_y_offset,
        template_y,
        last_dy,
        strip_h,
    );

    // 静止判定 + 置信度估计 + 接受门控
    let confidence = estimate_confidence(
        ref_buf, curr_buf, &sample_cols, best_y_offset.round() as u32, min_y_offset, max_y_offset, strip_h,
    );
    decide_match(best_y_offset, best_sad_avg, stationary_sad_avg, confidence, template_y, sad_accept)
}

/// 根据搜索结果做最终判定：静止 / 接受 / 拒绝。
fn decide_match(
    best_y_offset: f64,
    best_sad_avg: f64,
    stationary_sad_avg: f64,
    confidence: f64,
    template_y: u32,
    sad_accept: f64,
) -> Option<(f64, f64, f64)> {
    // 保留绝对静止快速路径（画面完全没动时 stationary_sad 极低）
    if stationary_sad_avg < STATIONARY_SAD {
        return Some((0.0, 1.0, 0.0));
    }
    // 移除 stationary < best + 1.0 硬覆盖——交由 is_stationary() 时序判断
    if best_sad_avg < sad_accept && confidence > MIN_CONFIDENCE {
        let dy = best_y_offset - template_y as f64;
        Some((dy, confidence, best_sad_avg))
    } else {
        None
    }
}

/// 提取模板条到连续 buffer（STRIP_H × n_cols）。
fn extract_template(ref_buf: &GrayBuf, template_y: u32, sample_cols: &[usize], strip_h: u32) -> Vec<u8> {
    let mut tpl = Vec::with_capacity(strip_h as usize * sample_cols.len());
    for dy in 0..strip_h {
        let row = ref_buf.row((template_y + dy) as usize);
        for &x in sample_cols {
            tpl.push(row[x]);
        }
    }
    tpl
}

/// 整数 SAD 主搜索 + 亚像素抛物线插值，返回 (best_y_offset_f64, best_sad_avg, stationary_sad_avg)。
/// stationary_sad_avg = y_offset == template_y 那次迭代的 SAD 均值。
fn search_best_offset(
    tpl: &[u8],
    curr: &GrayBuf,
    sample_cols: &[usize],
    min_y_offset: u32,
    max_y_offset: u32,
    template_y: u32,
    last_dy: Option<f64>,
    strip_h: u32,
) -> (f64, f64, f64) {
    let strip_h = strip_h as usize;
    let total = (strip_h * sample_cols.len()) as f64;

    let mut best_y_offset = min_y_offset;
    let mut min_penalized = f64::MAX;
    let mut best_sad_avg = f64::MAX;
    let mut stationary_sad_avg = f64::MAX;

    // 记录每个 y_offset 的原始 SAD（用于亚像素插值）
    let range_size = (max_y_offset - min_y_offset + 1) as usize;
    let mut sad_curve: Vec<f64> = Vec::with_capacity(range_size);
    let mut penalized_curve: Vec<f64> = Vec::with_capacity(range_size);
    let mut best_idx: usize = 0;

    for y_offset in min_y_offset..=max_y_offset {
        let mut sad: u64 = 0;
        let mut i = 0;
        for dy in 0..strip_h {
            let row = curr.row((y_offset as usize) + dy);
            for &x in sample_cols {
                let diff = (tpl[i] as i32 - row[x] as i32).unsigned_abs() as u64;
                sad += diff;
                i += 1;
            }
        }
        let sad_avg = sad as f64 / total;

        if y_offset == template_y {
            stationary_sad_avg = sad_avg;
        }

        let mut penalized = sad_avg;
        if let Some(ldy) = last_dy {
            let dy = y_offset as f64 - template_y as f64;
            penalized += SPEED_PENALTY * (dy - ldy).abs();
        }

        let idx = sad_curve.len();
        sad_curve.push(sad_avg);
        penalized_curve.push(penalized);

        if penalized < min_penalized {
            min_penalized = penalized;
            best_sad_avg = sad_avg;
            best_y_offset = y_offset;
            best_idx = idx;
        }
    }

    // 亚像素抛物线插值：在 best 处用 sad_curve（原始 SAD，非罚分值）拟合
    let best_f64 = if best_idx > 0 && best_idx + 1 < sad_curve.len() {
        let left = sad_curve[best_idx - 1];
        let center = sad_curve[best_idx];
        let right = sad_curve[best_idx + 1];
        let denom = left - 2.0 * center + right;
        if denom.abs() > 1e-10 {
            let delta = 0.5 * (left - right) / denom;
            // Clamp delta 到 [-0.5, +0.5]，防止极小 denom 导致大偏移
            let delta = delta.clamp(-0.5, 0.5);
            (best_y_offset as f64) + delta
        } else {
            best_y_offset as f64
        }
    } else {
        best_y_offset as f64
    };

    (best_f64, best_sad_avg, stationary_sad_avg)
}

/// 稀疏采样估计置信度：1 - best_sad / mean_sad。
fn estimate_confidence(
    ref_buf: &GrayBuf,
    curr_buf: &GrayBuf,
    sample_cols: &[usize],
    best_y_offset: u32,
    min_y_offset: u32,
    max_y_offset: u32,
    strip_h: u32,
) -> f64 {
    let sparse_cols: Vec<usize> = sample_cols.iter().step_by(2).copied().collect();
    if sparse_cols.is_empty() {
        return 0.0;
    }

    let mut sum_sad = 0.0f64;
    let mut sample_count = 0.0f64;
    for y_offset in (min_y_offset..=max_y_offset).step_by(10) {
        sum_sad += sparse_sad_at_offset(ref_buf, curr_buf, &sparse_cols, y_offset, strip_h);
        sample_count += 1.0;
    }

    if sample_count < 1.0 {
        return 0.0;
    }
    let mean_sad = sum_sad / sample_count;
    if mean_sad < 1e-5 {
        return 0.0;
    }

    let best_sad_avg = sparse_sad_at_offset(ref_buf, curr_buf, &sparse_cols, best_y_offset, strip_h);
    1.0 - (best_sad_avg / mean_sad)
}

/// 计算指定 y_offset 处的稀疏 SAD 均值（每隔 2 行 × 稀疏列）。
fn sparse_sad_at_offset(
    ref_buf: &GrayBuf,
    curr_buf: &GrayBuf,
    sparse_cols: &[usize],
    y_offset: u32,
    strip_h: u32,
) -> f64 {
    let strip_h = strip_h as usize;
    let mut sad: u64 = 0;
    let mut count = 0u64;
    for dy in (0..strip_h).step_by(2) {
        let ref_row = ref_buf.row(dy);
        let curr_row = curr_buf.row((y_offset as usize) + dy);
        for &x in sparse_cols {
            sad += (ref_row[x] as i32 - curr_row[x] as i32).unsigned_abs() as u64;
            count += 1;
        }
    }
    if count > 0 { sad as f64 / count as f64 } else { 0.0 }
}

/// 比较连续 RGBA buffer 的 ya 行 与 RgbaImage 的 yb 行是否逐像素相等。
fn rows_equal_buf(a: &[u8], a_w: u32, b: &RgbaImage, ya: u32, yb: u32) -> bool {
    let row_bytes = a_w as usize * 4;
    let a_start = ya as usize * row_bytes;
    let a_row = &a[a_start..a_start + row_bytes];
    let b_raw = b.as_raw();
    let b_start = yb as usize * row_bytes;
    let b_row = &b_raw[b_start..b_start + row_bytes];
    a_row == b_row
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

    /// 合成不同纹理密度的测试帧。
    /// texture_level: 0=纯色背景, 1=稀疏文字, 2=密集条纹
    fn make_frame_textured(width: u32, height: u32, scroll_offset: u32, texture_level: u32) -> RgbaImage {
        let mut img: ImageBuffer<Rgba<u8>, Vec<u8>> = ImageBuffer::new(width, height);
        for y in 0..height {
            for x in 0..width {
                let mut v = ((y + scroll_offset) % 256) as u8;
                match texture_level {
                    0 => {}, // 纯色，仅渐变
                    1 => { // 稀疏文字：每 20 行、每 50 列一个亮点
                        if y % 20 == 0 && x % 50 == 0 { v = v.saturating_add(100); }
                    }
                    2 => { // 密集条纹：每 5 行强对比
                        if (y + scroll_offset) % 5 == 0 { v = 255 - v; }
                        if x % 3 == 0 { v = v.saturating_add(60); }
                    }
                    _ => {},
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
    fn test_sticky_detection() {
        // 使用 make_frame_with_sticky 构造带固定顶/底区域的帧
        let top_h = 30;
        let bot_h = 25;
        let f0 = make_frame_with_sticky(TW, TH, top_h, bot_h, 0);
        let mut s = Stitcher::new(f0.clone(), StitchConfig::default());
        // init 帧：sticky 区域相同，中间内容也相同
        let f1 = make_frame_with_sticky(TW, TH, top_h, bot_h, 0);
        s.process_frame(&f1).unwrap();
        // 检测到的 sticky 应接近构造值（允许部分偏差）
        assert!(s.sticky_top >= top_h / 2, "sticky_top {} 应接近 {}", s.sticky_top, top_h);
        assert!(s.sticky_bottom >= bot_h / 2, "sticky_bottom {} 应接近 {}", s.sticky_bottom, bot_h);
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
        let h_after_init = s.height();
        // 第二次调用：实际滚动检测
        let f2 = make_frame(TW, TH, 40);
        let added = s.process_frame(&f2).unwrap();
        assert!(added, "滚动 40px 应追加内容");
        let h_after = s.height();
        // 追加后应 > init 后高度（init 会裁掉 sticky_bottom）
        assert!(
            h_after > h_after_init,
            "追加后画布高度 {} 应 > init 后 {}，确认有新行追加",
            h_after, h_after_init
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
    fn test_is_stationary_with_history() {
        let f0 = make_frame(TW, TH, 0);
        let s = Stitcher::new(f0, StitchConfig::default());

        // 无 dy_history → 不静止
        assert!(!s.is_stationary(), "空 history 不应判静止");

        // 手动注入 dy_history 模拟持续滚动 + 回弹
        let mut s2 = Stitcher::new(make_frame(TW, TH, 0), StitchConfig::default());
        s2.dy_history.extend(vec![-15.0, -12.0, -10.0, -3.0]);
        assert!(!s2.is_stationary(), "回弹帧 history 均值 -10 不应判静止");

        // 手动注入接近静止的 history
        let mut s3 = Stitcher::new(make_frame(TW, TH, 0), StitchConfig::default());
        s3.dy_history.extend(vec![-1.0, 0.0, -0.5, 1.0, 0.0]);
        assert!(s3.is_stationary(), "均值接近 0 应判静止");
    }

    #[test]
    fn test_dynamic_sad_accept_scales_with_texture() {
        let mut s = Stitcher::new(make_frame(TW, TH, 0), StitchConfig::default());

        // sad_baseline = 0 时，baseline_cap = 5.0
        // 低纹理且 baseline=0 → max(SAD_ACCEPT, min(bonus, 5)) = max(7.5, min(...)) = 7.5
        let low = s.dynamic_sad_accept(0.05);
        assert_eq!(low, SAD_ACCEPT, "低纹理且 baseline=0 应返回基础阈值");

        // 设定 baseline 后
        s.sad_baseline = 10.0;
        // baseline_cap = 10*1.5+5 = 20
        // 高纹理：texture=0.5 → bonus=15 → (7.5+15).min(20).max(7.5) = 20
        let high = s.dynamic_sad_accept(0.5);
        assert!(high > SAD_ACCEPT, "高纹理应放宽阈值: {}", high);
        assert!(high <= 20.0, "不应超过 baseline_cap: {}", high);
    }

    #[test]
    fn test_fallback_expanded_search_range() {
        // 构造超出 MAX_SCROLL 的快速滚动：init 后直接跳 300px
        let f0 = make_frame(TW, TH, 0);
        let mut s = Stitcher::new(f0, StitchConfig::default());
        let f1 = make_frame(TW, TH, 0);
        s.process_frame(&f1).unwrap(); // init
        // 300px 超出 MAX_SCROLL=220，主匹配应失败，降级 1 扩大到 440 应成功
        let f2 = make_frame(TW, TH, 300);
        let added = s.process_frame(&f2).unwrap();
        assert!(added, "快速滚动应通过降级 1（扩大搜索范围）匹配");
    }

    #[test]
    fn test_fallback_1d_projection_low_texture() {
        // 低纹理场景：纯色背景 + 稀疏文字
        let f0 = make_frame_textured(TW, TH, 0, 0);
        let mut s = Stitcher::new(f0, StitchConfig::default());
        let f1 = make_frame_textured(TW, TH, 0, 0);
        s.process_frame(&f1).unwrap(); // init
        let f2 = make_frame_textured(TW, TH, 30, 0);
        let _ = s.process_frame(&f2).unwrap(); // 验证不 panic
    }

    #[test]
    fn test_canvas_anchored_recovers_after_failures() {
        // Canvas-Anchored 核心验证：中间帧匹配失败后，后续帧能与画布底部正确对齐
        let f0 = make_frame(TW, TH, 0);
        let mut s = Stitcher::new(f0, StitchConfig::default());
        let f1 = make_frame(TW, TH, 0);
        s.process_frame(&f1).unwrap(); // init

        // 帧 2: 滚动 30px，成功追加
        let f2 = make_frame(TW, TH, 30);
        let added2 = s.process_frame(&f2).unwrap();
        assert!(added2);
        let h_after_2 = s.height();

        // 帧 3: 相同帧（静止），不追加
        let f3 = make_frame(TW, TH, 30);
        s.process_frame(&f3).unwrap();

        // 帧 4: 滚动到 60px，应能与画布底部正确对齐
        let f4 = make_frame(TW, TH, 60);
        let added4 = s.process_frame(&f4).unwrap();
        assert!(added4, "Canvas-Anchored 应在中间静止帧后恢复匹配");
        let h_after_4 = s.height();
        assert!(h_after_4 > h_after_2, "恢复后画布应继续增长: {} > {}", h_after_4, h_after_2);
    }

    #[test]
    fn test_extract_canvas_bottom_gray() {
        // 验证 extract_canvas_bottom_gray 提取的灰度与 canvas 底部 strip 一致
        let f0 = make_frame(TW, TH, 0);
        let mut s = Stitcher::new(f0.clone(), StitchConfig::default());
        let f1 = make_frame(TW, TH, 0);
        s.process_frame(&f1).unwrap(); // init

        let bottom_gray = s.extract_canvas_bottom_gray(STRIP_H);
        assert_eq!(bottom_gray.width, TW as usize);

        // 手动从 canvas 计算底部 strip 灰度比对
        let canvas = s.canvas();
        let canvas_h = canvas.height();
        assert!(canvas_h >= STRIP_H);
        for y in 0..STRIP_H {
            for x in 0..TW {
                let px = canvas.get_pixel(x, canvas_h - STRIP_H + y);
                let luma = (2126 * px[0] as u32 + 7152 * px[1] as u32 + 722 * px[2] as u32) / 10000;
                assert_eq!(bottom_gray.row(y as usize)[x as usize], luma as u8,
                    "底部 strip 灰度不一致 @ ({},{})", x, y);
            }
        }
    }
}
