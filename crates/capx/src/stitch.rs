use anyhow::Result;
use image::RgbaImage;
use std::collections::VecDeque;

// ===== 拼接算法常量（原散落在 find_overlap_spatial_ext 与 process_frame 中的魔法数字）=====

/// 静止判定阈值。dy=0 处的平均像素差值小于此值视为内容未滚动。
const STATIONARY_SAD: f64 = 2.0;
/// fallback（1D 投影 / best-guess）追加画布前的 2D 反向验证：重叠区每像素 SAD 均值上限。
/// 高于 STATIONARY_SAD 以吸收亚像素 .round() 误差 / 压缩噪声 / 渲染反锯齿差异。起步 15.0，
/// reject 日志（apply_fallback_match 内）便于线上标定后再收。详见 verify_alignment_2d。
const FALLBACK_VERIFY_SAD: f64 = 15.0;
/// 排除最左侧的比例（通常有图标/树状图）。
const X_START_RATIO: f64 = 0.10;
/// 排除最右侧的比例截止点（通常有滚动条/时间戳），即保留 10%~80% 横向区间。
const X_END_RATIO: f64 = 0.80;
/// 列抽样步长（像素）。每隔此值采样一列，提供双倍空间特征解析度。
const SAMPLE_STEP_X: usize = 2;
/// sticky 区域检测的最大高度（像素），顶部/底部各扫此高度。
const STICKY_DETECT_MAX: u32 = 80;
/// 首帧底部"无内容常数尾"检测：单行灰度（R 通道近似）max-min 上限。低于此值视为无内容
/// 行（纯黑/纯色空白，如暗色编辑器内容不到底下方的纯黑区）。连续无内容行 = 常数尾，
/// 裁掉以避免 canvas-anchored 底部 strip 锚点永久退化（常数模板 NCC 假匹配 score≈1.0 或
/// 失配死锁——2026-07-10 release 实测选区下半截纯黑时滚轮未动画布不增长）。
const CONTENT_ROW_MAXMIN: u8 = 30;
/// content_tail 判定的"暗"阈值：行内最亮像素 luma < 此值才算无内容暗尾（纯黑/暗背景）。
/// 与 max-min 双重判定，避免把高 luma 的低对比渐变行（如 make_frame 底部 luma>80、
/// 每行常数但亮）误判为纯黑尾——真实纯黑尾 luma≈0，渐变/文字行 luma 高。
const CONTENT_TAIL_MAX_LUMA: u8 = 40;
/// 自适应 strip 高度下限（像素）。内容极矮时也至少留此行数作 NCC 模板，防退化为单行匹配。
const MIN_STRIP: u32 = 8;

// ===== 健壮性优化常量 =====

/// 时序平滑：静止判断的 dy 均值阈值（近 N 帧 |dy| 均值 < 此值 → 静止）
/// dy 历史长度
const DY_HISTORY_LEN: usize = 8;

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

    /// 转为 image::GrayImage（供 imageproc 使用）。
    fn to_gray_image(&self) -> image::GrayImage {
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
fn to_feature_map(gray: &GrayBuf) -> (image::GrayImage, bool) {
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

// ===== NCC 匹配引擎 =====

use imageproc::definitions::Image;
use imageproc::template_matching::{find_extremes, match_template, MatchTemplateMethod};

/// NCC 匹配结果。
struct NccResult {
    /// 最佳偏移（response 坐标，即模板顶部在搜索区域中的 y 偏移）
    best_y: f64,
    /// NCC 分数 [0, 1]，越大越好
    best_score: f64,
    /// 完整 response map
    response: Image<image::Luma<f32>>,
}

/// 主 NCC 结果（大屏两阶段 / 小屏单阶段统一产出）。
enum PrimaryOutcome {
    /// 亚像素 refined_y（原分辨率坐标） + best_score
    Matched(f64, f64),
    /// NCC validate 失败（附 score 供日志/stuck 判断）
    Mismatch(f64),
    /// ncc_match 返回 None（template/search size 不匹配）
    SizeError,
}

/// NCC 匹配：在搜索区域中找模板的最佳对齐位置。
fn ncc_match(
    template: &image::GrayImage,
    search_region: &image::GrayImage,
) -> Option<NccResult> {
    // 模板必须严格小于搜索区域（match_template 的要求）
    if template.width() > search_region.width() || template.height() >= search_region.height() {
        return None;
    }
    let response = match_template(
        search_region,
        template,
        MatchTemplateMethod::CrossCorrelationNormalized,
    );
    let extremes = find_extremes(&response);
    let best_y = extremes.max_value_location.1 as f64;
    let best_score = extremes.max_value as f64;
    Some(NccResult { best_y, best_score, response })
}

/// 保边缘降采样（Triangle 双线性）。NCC+亚像素不能用 Nearest——锯齿破坏 response 峰值。
fn downsample_grayimage(img: &image::GrayImage, scale: f64) -> image::GrayImage {
    let nw = ((img.width() as f64 * scale).max(1.0)).round() as u32;
    let nh = ((img.height() as f64 * scale).max(1.0)).round() as u32;
    image::imageops::resize(img, nw, nh, image::imageops::FilterType::Triangle)
}

/// 限定 y 邻域 [y_min, y_max] 的 NCC + 亚像素 refine（两阶段 stage2 用）。
/// stage1 给出粗 dy_coarse，本函数在原分辨率 ±Npx 内精化，恢复 0.1px 亚像素。
/// 返回 (refined_y 原分辨率坐标, best_score)。范围太小 / size 不匹配 → None。
fn ncc_match_range(
    template: &image::GrayImage,
    search_region: &image::GrayImage,
    y_min: f64,
    y_max: f64,
) -> Option<(f64, f64)> {
    let th = template.height();
    let sh = search_region.height();
    if th >= sh {
        return None;
    }
    let lo = (y_min.max(0.0).floor() as u32).min(sh - th);
    let hi = (y_max.ceil() as u32).saturating_add(th).min(sh);
    if hi <= lo || hi - lo <= th {
        return None;
    }
    let sub = image::imageops::crop_imm(search_region, 0, lo, search_region.width(), hi - lo)
        .to_image();
    let ncc = ncc_match(template, &sub)?;
    let refined_sub = parabolic_refine_from_response(&ncc.response, ncc.best_y);
    Some((refined_sub + lo as f64, ncc.best_score))
}

/// 多道验证 NCC 匹配结果。返回 true 表示匹配可信。
fn validate_ncc_match(
    response: &Image<image::Luma<f32>>,
    _best_y: usize,
    best_score: f32,
    threshold: f32,
) -> bool {
    // 1. 最低分数
    if best_score < threshold {
        return false;
    }

    // 无区分度检测：response 的 max - min 差值 < 0.1 说明所有位置得分几乎相同，
    // NCC 无足够区分力来确定真实偏移（纯色/空白/极低纹理）。拒绝匹配。
    let h = response.height() as usize;
    let mut min_score = f32::MAX;
    let mut max_score = f32::MIN;
    for y in 0..h {
        let v = response.get_pixel(0, y as u32)[0];
        if v < min_score { min_score = v; }
        if v > max_score { max_score = v; }
    }
    if max_score - min_score < 0.1 {
        return false;
    }

    true
}

/// 从 NCC response map 在最佳 y 处做抛物线拟合，返回亚像素偏移。
fn parabolic_refine_from_response(response: &Image<image::Luma<f32>>, best_y: f64) -> f64 {
    let by = best_y as usize;
    if by == 0 || by + 1 >= response.height() as usize {
        return best_y;
    }
    let left = response.get_pixel(0, by as u32 - 1)[0] as f64;
    let center = response.get_pixel(0, by as u32)[0] as f64;
    let right = response.get_pixel(0, by as u32 + 1)[0] as f64;
    let denom = left - 2.0 * center + right;
    if denom.abs() > 1e-10 {
        let delta = 0.5 * (left - right) / denom;
        best_y + delta.clamp(-0.5, 0.5)
    } else {
        best_y
    }
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
    /// 模板条高度（像素）。从画布底部取此高度做 NCC 模板。
    pub strip_h: u32,
    /// 最大滚动位移搜索上界（像素）。
    pub max_scroll: u32,
    /// 最低 NCC 分数阈值。
    pub ncc_score_threshold: f32,
    /// NCC 降采样触发宽度（像素）。帧宽 > 此值才降采样；≤ 则原分辨率（小屏零影响）。
    pub ncc_downsample_width: u32,
}

impl Default for StitchConfig {
    fn default() -> Self {
        Self {
            min_scroll_px: 1.0,
            min_confidence: 0.15,
            strip_h: 80,
            max_scroll: 220,
            ncc_score_threshold: 0.65,
            ncc_downsample_width: 1920,
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
    /// 连续 best-guess 次数（主匹配成功时归零，超过 3 次熔断）。
    best_guess_streak: u32,
    /// 连续 NCC 验证失败且 score 几乎相同的次数（检测“画面静止但 NCC 不匹配”状态）。
    ncc_stuck_count: u32,
    /// 上一次成功追加的 dy（用于检测连续相同 dy → 周期性假匹配/静止）。
    last_appended_dy: Option<f64>,
    /// 连续相同 dy 追加次数。
    same_dy_count: u32,
    /// 上一帧的有效区灰度（相邻帧参考 fallback 用）。每帧 process_frame 末尾更新。
    prev_gray: Option<GrayBuf>,
    /// 首帧底部"无内容常数尾"高度（如选区下半截恒定纯黑空白）。与 sticky_bottom 同为
    /// 应排除的底部固定区，但 sticky_bottom 依赖首/次帧逐像素相等（光标闪烁/抗锯齿/scrollbar
    /// 差异会漏检），content_tail 直接看单行 max-min 补缺口。裁掉后画布底部停在真实内容底。
    content_tail: u32,
    /// 自适应 strip 高度。矮选区（内容高 < strip_h*3，如 162px 物理高含 80px 暗尾 → 内容 82px）
    /// 时固定 80 strip 会吃光 ROI 使 NCC 搜索范围≈0 → 首帧即失配死锁；故按 content_h/3 缩小，
    /// 留 2/3 作搜索范围。每帧基于 content_h 更新；模板提取与匹配几何统一读此值（非 config.strip_h）。
    eff_strip_h: u32,
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
            eff_strip_h: config.strip_h,
            config,
            last_dy: None,
            dy_history: VecDeque::with_capacity(DY_HISTORY_LEN),
            best_guess_streak: 0,
            ncc_stuck_count: 0,
            last_appended_dy: None,
            same_dy_count: 0,
            prev_gray: None,
            content_tail: 0,
        }
    }

    pub fn process_frame(&mut self, frame: &RgbaImage) -> Result<bool> {
        let (w, h) = (frame.width(), frame.height());

        // 防御性校验：帧宽度必须与画布一致，否则切片越界或数据污染
        if w != self.canvas_w {
            log::warn!("[stitch] frame width {} != canvas_w {}, skipping", w, self.canvas_w);
            return Ok(false);
        }

        // 每帧基于当前帧检测底部"无内容常数尾"高度（动态纯黑尾：前期内容填满选区时为 0，
        // 滚动后期内容上移、选区底部露出背景时增长）。sticky_bottom 仅首帧一次且依赖逐像素相等，
        // 无法应对动态纯黑尾；content_tail 每帧看单行 max-min，eff_bottom 每帧止于真实内容底 →
        // append 永不带入纯黑尾 → 画布底部 strip 始终有特征，避免 canvas-anchored 锚点退化死锁
        // （常数模板 NCC 假匹配 score≈1.0 / 失配 stuck）。
        self.content_tail = self.detect_content_tail(frame);

        if !self.detected {
            self.detect_sticky(frame);
            self.detected = true;
            // 用画布种子（首帧）【自身】暗尾裁剪画布，而非上方 self.content_tail（=当前第二帧暗尾）。
            // 首帧在 app 聚焦/滚动开始前由 setup 单独捕获，暗尾常大于已滚动后的第二帧；用第二帧
            // 小暗尾裁首帧大暗尾会留残余暗尾 → 画布底部常数 → canvas_has=false 首帧即死锁
            // （release 实测 296×160 矮选区"滚动不拼接"）。读 canvas_buf 测首帧自身暗尾根治。
            let seed_tail = self.scan_content_tail_in(&self.canvas_buf, self.canvas_h as usize);
            let eff_bottom0 = self
                .canvas_h
                .saturating_sub(self.sticky_bottom + seed_tail);
            if eff_bottom0 > self.sticky_top {
                self.canvas_buf.truncate(eff_bottom0 as usize * self.canvas_w as usize * 4);
                self.canvas_h = eff_bottom0;
                self.invalidate_cache();
            }
            // 自适应 strip：基于首帧内容高度（画布已截断到内容底）。矮选区缩小 strip 留搜索范围。
            self.eff_strip_h = self.effective_strip_for(self.canvas_h.saturating_sub(self.sticky_top));
            // 诊断：种子是否常数（首帧在 app 聚焦前捕获为空白时为 true → 触发下游 reseed 恢复）。
            log::info!(
                "[stitch] init: canvas_h={} sticky_top={} sticky_bottom={} seed_tail={} eff_strip_h={} seed_constant={}",
                self.canvas_h, self.sticky_top, self.sticky_bottom, seed_tail, self.eff_strip_h,
                self.canvas_bottom_constant()
            );
            // Canvas-Anchored：下一帧直接从 canvas 底部提取模板，无需存 reference

            return Ok(false); // 第二帧用于初始化，不拼接
        }

        let eff_top = self.sticky_top;
        let eff_bottom = h.saturating_sub(self.sticky_bottom + self.content_tail);
        if eff_bottom <= eff_top {
            return Ok(false);
        }
        // 每帧更新自适应 strip：content_tail 动态 → content_h 动态 → strip 随之自适应。
        self.eff_strip_h = self.effective_strip_for(eff_bottom - eff_top);
        // ROI 不足一个 strip（sticky_top + content_tail 几乎吃光整帧，如矮选区+大暗尾）→ 无法匹配，
        // 跳帧。否则下游 quick_stationary_check/NCC 会越界（curr ROI 行数 < strip）。
        if eff_bottom - eff_top < self.eff_strip_h {
            return Ok(false);
        }

        // 画布锚点维护（canvas-anchored 核心）：画布底部常数时 Sobel 退化、锚点失效。每帧检查
        // （非一次性闸门——滚动中画布底部可能【再次】变常数：滚到内容末尾露纯色背景、1D 假匹配
        // append 常数块、动态背景）。先轻量判画布底 strip 是否常数：
        //   常数 → 测常数尾高度 tail。tail 可裁（裁后仍 ≥ keep_min 行内容）→ 非破坏性裁掉常数尾
        //          （只丢空白/纯色背景，不丢内容），锚点回到真实内容底，本帧继续匹配；
        //   tail 不可裁（画布几乎全常数——种子空白 / 整帧污染）→ reseed 用当前内容帧重建锚点。
        // 第 7 次回归根因：旧 canvas_content_confirmed 一次性闸门确认后终身跳过检查，滚动中画布底部
        // 再次变常数时永久死锁（NCC stuck=5 stationary 到 finalize，finalize 灰度兜底对常数画布
        // score≈1.0 假匹配拼错）。改为每帧自愈——"治"而非一次"防"。
        if self.canvas_bottom_constant() {
            let tail = self.scan_canvas_constant_tail();
            let keep_min = self.eff_strip_h.max(MIN_STRIP);
            let new_h = self.canvas_h.saturating_sub(tail);
            if new_h >= keep_min {
                let row_bytes = self.canvas_w as usize * 4;
                self.canvas_buf.truncate(new_h as usize * row_bytes);
                self.canvas_h = new_h;
                self.invalidate_cache();
                // 锚点位移：旧 stuck/best-guess 基于死锚，作废给匹配重来的机会。
                self.ncc_stuck_count = 0;
                self.best_guess_streak = 0;
                log::info!(
                    "[stitch] canvas constant tail trimmed: {} rows, new canvas_h={}",
                    tail, self.canvas_h
                );
            } else {
                // 画布几乎全常数（无内容可留：种子空白 / 异常整帧污染）→ 从当前帧内容区重建锚点。
                self.reseed_canvas_from(frame, eff_top, eff_bottom);
                // 重建后本帧成为新基准，prev_gray 设为本帧内容灰度，供下一帧相邻帧 fallback。
                self.prev_gray = Some(GrayBuf::from_rgba_roi(frame, eff_top as usize, eff_bottom as usize));
                return Ok(false);
            }
        }

        // 全有效区域灰度转换（不限制 ROI——快速滚动时内容可能出现在有效区任意位置）
        let roi_top = eff_top as usize;
        let roi_bottom = eff_bottom as usize;
        let curr_gray = GrayBuf::from_rgba_roi(frame, roi_top, roi_bottom);
        let canvas_gray = self.extract_canvas_bottom_gray(self.eff_strip_h);

        let result = self.process_frame_inner(frame, &curr_gray, &canvas_gray, w, eff_top, eff_bottom);

        // 相邻帧参考 fallback：记录本帧有效区灰度，供下一帧用（突变时画布底部旧模板
        // 失配，改用紧邻前一帧——与当前帧重叠最大、突变边界共同特征——匹配）。
        self.prev_gray = Some(curr_gray);

        result
    }

    /// process_frame 的匹配主体（Sobel 特征 → NCC → 验证 → dy → 周期检测 → append）。
    /// 提取出来是为了让 process_frame 在调用后统一更新 prev_gray（避免散落多个 return 点）。
    fn process_frame_inner(
        &mut self,
        frame: &RgbaImage,
        curr_gray: &GrayBuf,
        canvas_gray: &GrayBuf,
        w: u32,
        eff_top: u32,
        eff_bottom: u32,
    ) -> Result<bool> {
        // 多候选 NCC：Sobel 特征优先，暗色/低纹理失配时灰度兜底
        let (refined_y, best_score) = match self.best_ncc_match(canvas_gray, curr_gray, w) {
            PrimaryOutcome::Matched(refined_y, score) => (refined_y, score),
            PrimaryOutcome::Mismatch(score) => {
                // NCC stuck 检测：连续失败且 score 几乎相同 → 画面静止但有渲染差异
                if self.ncc_stuck_count >= 5 {
                    log::info!("[stitch] NCC stuck (score={:.4}, count={}), treating as stationary", score, self.ncc_stuck_count);
                    self.dy_history.clear();
                    self.best_guess_streak = 0;
                    self.last_dy = None;
                    return Ok(false);
                }
                log::info!("[stitch] NCC match failed validation (score={:.4}, stuck={})", score, self.ncc_stuck_count);
                self.ncc_stuck_count += 1;
                return self.try_fallback(frame, curr_gray, canvas_gray, w, eff_top, eff_bottom);
            }
            PrimaryOutcome::SizeError => {
                log::info!("[stitch] ncc returned None (size mismatch)");
                return self.try_fallback(frame, curr_gray, canvas_gray, w, eff_top, eff_bottom);
            }
        };

        // NCC 成功：重置 stuck 计数
        self.ncc_stuck_count = 0;

        // 坐标推导（refined_y 已是亚像素 best_y）：
        // new_rows = ROI高度 - refined_y - strip_h；dy = -new_rows（负=向下滚动）
        let roi_height = (eff_bottom - eff_top) as f64;
        let new_rows_raw = roi_height - refined_y - self.eff_strip_h as f64;
        let dy = -new_rows_raw;

        // dy > 0 = 向上滚动（忽略）。dy≤0 不跳过，交给 min_scroll_px 过滤，
        // 避免慢速滚动时亚像素位移被 dy>=0.0 检查丢弃导致内容缺失。
        if dy > 0.0 {
            log::info!("[stitch] skipped frame: dy={:.1} > 0.0 (ncc={:.4})", dy, best_score);
            return Ok(false);
        }

        let new_rows = (-dy).round() as u32;
        let max_scroll_limit = (eff_bottom - eff_top) * 9 / 10;

        if new_rows < self.config.min_scroll_px as u32 || new_rows >= max_scroll_limit {
            log::info!("[stitch] skipped frame: new_rows={} invalid (min={}, max={}) (ncc={:.4})",
                new_rows, self.config.min_scroll_px, max_scroll_limit, best_score);
            return Ok(false);
        }

        // 周期性假匹配检测：连续 3 次以上 dy 相同 → 用 quick_stationary_check
        // 区分"均匀滚动"（画面在动，合法）和"周期性假匹配"（画面没动，NCC 在周期内容找假匹配）。
        let dy_rounded = (-dy).round();
        if self.same_dy_count >= 3 {
            if let Some(locked_dy) = self.last_appended_dy {
                if (dy_rounded - locked_dy).abs() < 2.0 {
                    return Ok(false);
                }
            }
            log::info!("[stitch] periodic lock released (dy={:.0})", dy_rounded);
            self.same_dy_count = 0;
        }
        if let Some(prev_dy) = self.last_appended_dy {
            if (dy_rounded - prev_dy).abs() < 2.0 {
                self.same_dy_count += 1;
                if self.same_dy_count >= 3 {
                    // 连续相同 dy：检查画面是否真的在动
                    let x_start = (w as f64 * X_START_RATIO) as u32;
                    let x_end = (w as f64 * X_END_RATIO) as u32;
                    let sample_cols: Vec<usize> = (x_start as usize..x_end as usize)
                        .step_by(SAMPLE_STEP_X)
                        .collect();
                    let stationary_sad = self.quick_stationary_check(curr_gray, canvas_gray, &sample_cols);
                    if stationary_sad < STATIONARY_SAD * 5.0 {
                        // 画面没动 → 周期性假匹配
                        log::info!("[stitch] periodic false match locked (dy={:.0}, sad={:.1})", dy_rounded, stationary_sad);
                        return Ok(false);
                    } else {
                        // 画面在动 → 合法均匀滚动，继续
                        log::info!("[stitch] uniform scroll detected (dy={:.0}, sad={:.1}), not locking", dy_rounded, stationary_sad);
                    }
                }
            } else {
                self.same_dy_count = 0;
            }
        }
        self.last_appended_dy = Some(dy_rounded);

        log::info!("[stitch] ncc={:.4} dy={:.1} new_rows={} canvas_h={}",
            best_score, dy, new_rows, self.canvas_h);

        // 主匹配成功：重置 best-guess 计数
        self.best_guess_streak = 0;

        // 画布追加（NCC + 抛物线插值已给出精准切割点，不需要额外接缝寻找）
        let crop_y = eff_bottom - new_rows;
        let row_bytes = w as usize * 4;
        let start = crop_y as usize * row_bytes;
        let end = start + new_rows as usize * row_bytes;
        let frame_raw = frame.as_raw();
        self.canvas_buf.extend_from_slice(&frame_raw[start..end]);
        self.canvas_h += new_rows;
        self.invalidate_cache();

        self.last_dy = Some(dy);
        self.dy_history.push_back(dy);
        if self.dy_history.len() > DY_HISTORY_LEN {
            self.dy_history.pop_front();
        }

        Ok(true)
    }

    /// 主 NCC：大屏走两阶段 refine（降采样粗定位 + 原分辨率 refine），小屏走单阶段。
    /// 封装 validate；失配语义（Mismatch/SizeError）交调用方走 stuck/fallback。
    fn primary_ncc(
        &self,
        template: &image::GrayImage,
        search_region: &image::GrayImage,
        w: u32,
    ) -> PrimaryOutcome {
        if w > self.config.ncc_downsample_width {
            // stage1: 降采样域粗定位
            let scale = self.config.ncc_downsample_width as f64 / w as f64;
            let tmpl_ds = downsample_grayimage(template, scale);
            let search_ds = downsample_grayimage(search_region, scale);
            let ncc_ds = match ncc_match(&tmpl_ds, &search_ds) {
                Some(r) => r,
                None => return PrimaryOutcome::SizeError,
            };
            if !validate_ncc_match(
                &ncc_ds.response,
                ncc_ds.best_y as usize,
                ncc_ds.best_score as f32,
                self.config.ncc_score_threshold,
            ) {
                return PrimaryOutcome::Mismatch(ncc_ds.best_score);
            }
            let dy_coarse = ncc_ds.best_y / scale;
            // stage2: 原分辨率 ±2px 邻域 refine（恢复亚像素）
            match ncc_match_range(template, search_region, dy_coarse - 2.0, dy_coarse + 2.0) {
                Some((refined_y, score)) => PrimaryOutcome::Matched(refined_y, score),
                None => PrimaryOutcome::SizeError,
            }
        } else {
            // 单阶段（小屏，原路径）
            let ncc = match ncc_match(template, search_region) {
                Some(r) => r,
                None => return PrimaryOutcome::SizeError,
            };
            if !validate_ncc_match(
                &ncc.response,
                ncc.best_y as usize,
                ncc.best_score as f32,
                self.config.ncc_score_threshold,
            ) {
                return PrimaryOutcome::Mismatch(ncc.best_score);
            }
            let refined_y = parabolic_refine_from_response(&ncc.response, ncc.best_y);
            PrimaryOutcome::Matched(refined_y, ncc.best_score)
        }
    }

    /// 多候选 NCC：双侧均有 Sobel 特征时，Sobel 优先，validate 失配再灰度 NCC 兜底；
    /// 任一侧 Sobel 退化（strip 常数）时**不兜底**，直接进降级链。
    ///
    /// 为何退化时不走灰度：Sobel 退化（canvas/curr 底部 strip `max_gradient==0`）= 该 strip
    /// 灰度是常数（暗色编辑器纯黑空白行）。灰度 NCC 对常数模板返回 score≈1.0 假匹配
    /// （imageproc 退化值；release 实测 dy=-644.4 重复假帧污染画布 + periodic false match
    /// sad=0.0）。常数 strip 无真实匹配可言，交降级链（相邻帧 prev_gray 有内容可救）。
    ///
    /// 灰度兜底仅在「双侧均有特征但 Sobel 分数失配」时触发：此时两侧都有纹理，灰度
    /// 对比度空间有时比 Sobel 梯度更稳。正常帧 Sobel matched 直接返回，不触达此分支。
    fn best_ncc_match(
        &self,
        canvas_gray: &GrayBuf,
        curr_gray: &GrayBuf,
        w: u32,
    ) -> PrimaryOutcome {
        let (canvas_feat, canvas_has) = to_feature_map(canvas_gray);
        let (curr_feat, curr_has) = to_feature_map(curr_gray);

        // 任一侧 Sobel 退化 = 常数 strip：灰度也必然常数，NCC 必假匹配（≈1.0）。不兜底。
        if !canvas_has || !curr_has {
            log::debug!(
                "[stitch] sobel degenerated (canvas_has={}, curr_has={}), skip gray fallback \
                 (constant strip would false-match ~1.0)",
                canvas_has, curr_has
            );
            return PrimaryOutcome::Mismatch(0.0);
        }

        // 双侧有特征：Sobel 优先
        match self.primary_ncc(&canvas_feat, &curr_feat, w) {
            PrimaryOutcome::Matched(y, s) => {
                log::debug!("[stitch] sobel-ncc matched (score={:.4})", s);
                PrimaryOutcome::Matched(y, s)
            }
            PrimaryOutcome::Mismatch(s) => {
                log::debug!("[stitch] sobel-ncc mismatch (score={:.4}), trying gray fallback", s);
                let canvas_gray_img = canvas_gray.to_gray_image();
                let curr_gray_img = curr_gray.to_gray_image();
                match self.primary_ncc(&canvas_gray_img, &curr_gray_img, w) {
                    PrimaryOutcome::Matched(y, s) => {
                        log::debug!("[stitch] gray-ncc fallback matched (score={:.4})", s);
                        PrimaryOutcome::Matched(y, s)
                    }
                    PrimaryOutcome::Mismatch(gs) => {
                        log::debug!("[stitch] gray-ncc fallback mismatch (score={:.4})", gs);
                        PrimaryOutcome::Mismatch(s.max(gs))
                    }
                    PrimaryOutcome::SizeError => PrimaryOutcome::SizeError,
                }
            }
            PrimaryOutcome::SizeError => PrimaryOutcome::SizeError,
        }
    }

    /// 相邻帧参考 fallback：用前一帧有效区底部 strip 当模板，在当前帧有效区做 NCC。
    /// 突变时画布底部旧模板（如文字）与当前帧（如图片）失配；前一帧与当前帧只差
    /// 一个 dy、突变边界是两帧共同特征、重叠最大 → 能求出正确 dy，避免 best-guess 盲 append。
    /// dy 推导与主匹配同公式（模板=上一时刻底部 strip，search=当前帧有效区）。
    fn try_match_prev_frame(
        &self,
        prev_gray: &GrayBuf,
        curr_gray: &GrayBuf,
        eff_top: u32,
        eff_bottom: u32,
    ) -> Option<f64> {
        let prev_h = prev_gray.data.len() / prev_gray.width;
        if prev_h < self.eff_strip_h as usize + 10 {
            return None;
        }
        // prev 底部 eff_strip_h 行裁为独立模板（y_offset 归零）
        let strip_rows = self.eff_strip_h as usize;
        let prev_strip = GrayBuf {
            data: prev_gray.data[(prev_h - strip_rows) * prev_gray.width..].to_vec(),
            width: prev_gray.width,
            y_offset: 0,
        };
        let (tmpl_feat, tmpl_has) = to_feature_map(&prev_strip);
        let (curr_feat, curr_has) = to_feature_map(curr_gray);
        // 任一侧 Sobel 退化（strip 常数，如选区下半截纯黑）：灰度也必然常数，
        // NCC 对常数模板返回 score≈1.0 假匹配（release 实测 dy=-247.5 画布疯涨）。
        // 与 best_ncc_match 同一坑——退化时放弃 prev-frame，交下游 1D/stationary。
        if !tmpl_has || !curr_has {
            log::debug!(
                "[stitch] prev-frame skip: strip degenerated (tmpl_has={}, curr_has={})",
                tmpl_has, curr_has
            );
            return None;
        }
        let ncc = ncc_match(&tmpl_feat, &curr_feat)?;
        if !validate_ncc_match(&ncc.response, ncc.best_y as usize, ncc.best_score as f32, self.config.ncc_score_threshold) {
            return None;
        }
        let roi_height = (eff_bottom - eff_top) as f64;
        let refined_y = parabolic_refine_from_response(&ncc.response, ncc.best_y);
        let new_rows_raw = roi_height - refined_y - self.eff_strip_h as f64;
        let dy = -new_rows_raw;
        if dy >= 0.0 {
            return None;
        }
        log::info!("[stitch] prev-frame NCC dy={:.1} (score={:.4})", dy, ncc.best_score);
        Some(dy)
    }

    /// 降级链：NCC 匹配失败时的兜底处理。
    fn try_fallback(
        &mut self,
        frame: &RgbaImage,
        curr_gray: &GrayBuf,
        canvas_gray: &GrayBuf,
        w: u32,
        eff_top: u32,
        eff_bottom: u32,
    ) -> Result<bool> {
        let x_start = (w as f64 * X_START_RATIO) as u32;
        let x_end = (w as f64 * X_END_RATIO) as u32;
        // 抽样列：2D 反向验证与静止检测共用，提前算一次复用（消除下方重复计算）
        let sample_cols: Vec<usize> = (x_start as usize..x_end as usize)
            .step_by(SAMPLE_STEP_X)
            .collect();
        let max_scroll = self.config.max_scroll;

        // 相邻帧参考 fallback（方向 1）：画布底部旧模板失配时，改用前一帧匹配当前帧。
        // 前一帧与当前帧重叠最大、突变边界共同特征 → 求出正确 dy，不盲 append 污染画布。
        if let Some(prev_gray) = &self.prev_gray {
            if let Some(dy) = self.try_match_prev_frame(prev_gray, curr_gray, eff_top, eff_bottom) {
                self.best_guess_streak = 0;
                self.ncc_stuck_count = 0;
                return self.apply_fallback_match(dy, 0.0, 0.0, frame, curr_gray, canvas_gray, &sample_cols, false, w, eff_top, eff_bottom);
            }
        }

        // 降级：1D 灰度投影匹配
        if let Some((dy, conf, sad)) = self.try_match_1d_projection(
            canvas_gray, curr_gray, x_start, x_end, eff_top, eff_bottom, max_scroll, 10.0,
        ) {
            log::info!("[stitch] fallback: 1D projection match, dy={:.1} conf={:.4}", dy, conf);
            self.best_guess_streak = 0;
            return self.apply_fallback_match(dy, conf, sad, frame, curr_gray, canvas_gray, &sample_cols, true, w, eff_top, eff_bottom);
        }

        // 静止检测：匹配全失败时检查画面是否实际没动
        let stationary_sad = self.quick_stationary_check(curr_gray, canvas_gray, &sample_cols);
        if stationary_sad < STATIONARY_SAD {
            log::info!("[stitch] stationary detected before best-guess (sad={:.2})", stationary_sad);
            self.dy_history.clear();
            self.best_guess_streak = 0;
            self.last_dy = None;
            return Ok(false);
        }

        // Best-Guess：历史 dy 中位数估算
        // 熔断后仍重试：当用户重新开始滚动时 NCC 恢复匹配会重置 streak
        if self.best_guess_streak < 3 {
            if let Some(dy) = self.estimate_dy_hint() {
                log::info!("[stitch] best-guess dy={:.1} (streak={})", dy, self.best_guess_streak + 1);
                self.best_guess_streak += 1;
                return self.apply_fallback_match(dy, 0.0, 0.0, frame, curr_gray, canvas_gray, &sample_cols, true, w, eff_top, eff_bottom);
            }
        }

        log::info!("[stitch] all fallbacks exhausted, skipping frame");
        self.last_dy = None;
        Ok(false)
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

    /// 画布底部 eff_strip_h 行是否常数（无内容、Sobel 必退化）。采样 ~8 列/行算全局 max-min，
    /// 低于 CONTENT_ROW_MAXMIN 即常数。用于死锚检测：首帧在 app 聚焦前捕获为空白时画布底部
    /// 常数，canvas-anchored 锚点失效。轻量（仅采样，非全量 Sobel）。
    fn canvas_bottom_constant(&self) -> bool {
        let strip_h = self.eff_strip_h as usize;
        let w = self.canvas_w as usize;
        let row_bytes = w * 4;
        let start_row = self.canvas_h.saturating_sub(strip_h as u32) as usize;
        let mut minv = u8::MAX;
        let mut maxv = 0u8;
        let step = (w / 8).max(1);
        for y in start_row..self.canvas_h as usize {
            let row_start = y * row_bytes;
            for x in (0..w).step_by(step) {
                let v = self.canvas_buf[row_start + x * 4];
                if v < minv {
                    minv = v;
                }
                if v > maxv {
                    maxv = v;
                }
            }
        }
        (maxv - minv) < CONTENT_ROW_MAXMIN
    }

    /// 画布底部连续"常数行"数（亮度无关——区别于 scan_content_tail_in 的「暗+常数」双判定）。
    /// 逐行从画布底往上累加抽样像素的运行 min/max，(max-min) ≥ CONTENT_ROW_MAXMIN 即命中内容行停止。
    /// 用于锚点自愈：画布底部常数（纯黑/纯白/纯灰背景，或 1D 假匹配 append 的常数块）时 Sobel 退化、
    /// 锚点失效；裁掉常数尾让锚点回到真实内容底。
    ///
    /// 运行 min/max 而非单行 max-min 的原因：垂直渐变（每行横向常数、但行间亮度递增）单行 max-min=0
    /// 会被误判常数，然其有 Sobel 垂直梯度、是可匹配内容。运行 min/max 累积多行后 diff≥阈值即停 →
    /// 渐变区不被误裁。纯色尾（所有行同值）diff 恒 0 → 全部计入尾。真实文字行（横向 max-min 大）
    /// 首行即触发停止。
    fn scan_canvas_constant_tail(&self) -> u32 {
        let w = self.canvas_w as usize;
        let row_bytes = w * 4;
        let step = (w / 8).max(1);
        let mut minv = u8::MAX;
        let mut maxv = 0u8;
        let mut tail = 0u32;
        for y in (0..self.canvas_h as usize).rev() {
            let row_start = y * row_bytes;
            for x in (0..w).step_by(step) {
                let v = self.canvas_buf[row_start + x * 4];
                if v < minv {
                    minv = v;
                }
                if v > maxv {
                    maxv = v;
                }
            }
            if maxv - minv >= CONTENT_ROW_MAXMIN {
                break; // 该行引入变化 → 内容起点，停止（不计入 tail）
            }
            tail += 1;
        }
        tail
    }

    /// 用当前帧内容区 [eff_top, eff_bottom) 重建画布锚点（死锚恢复，破坏性——丢弃整个画布）。
    /// canvas-anchored 架构下画布底部必须是真实内容；种子空白（首帧聚焦前捕获）或画布几乎全常数
    /// （异常整帧污染，非破坏性裁尾已无内容可留）时锚点失效、永久死锁，此处用首个到达的当前帧替换
    /// 画布，后续帧即可正常匹配。重置匹配历史/stuck（锚点变更，旧状态作废）。
    fn reseed_canvas_from(&mut self, frame: &RgbaImage, eff_top: u32, eff_bottom: u32) {
        let w = self.canvas_w as usize;
        let row_bytes = w * 4;
        let src = frame.as_raw();
        let top = eff_top as usize;
        let bottom = eff_bottom as usize;
        let new_h = bottom - top;
        let mut buf = Vec::with_capacity(new_h * row_bytes);
        for y in top..bottom {
            let s = y * row_bytes;
            buf.extend_from_slice(&src[s..s + row_bytes]);
        }
        self.canvas_buf = buf;
        self.canvas_h = new_h as u32;
        self.invalidate_cache();
        self.eff_strip_h = self.effective_strip_for(self.canvas_h.saturating_sub(self.sticky_top));
        // 锚点变更：旧 dy_history/stuck 基于死锚，全部作废。
        self.dy_history.clear();
        self.ncc_stuck_count = 0;
        self.best_guess_streak = 0;
        self.last_dy = None;
        log::info!(
            "[stitch] canvas reseeded from current frame (anchor was constant: blank seed or fully-corrupt canvas), new canvas_h={}",
            self.canvas_h
        );
    }


    /// 轻量静止检测：比较当前帧底部 strip 与画布底部 strip 的全局 SAD。
    fn quick_stationary_check(&self, curr: &GrayBuf, canvas_ref: &GrayBuf, sample_cols: &[usize]) -> f64 {
        let mut sad: u64 = 0;
        let mut count: u64 = 0;
        for dy in 0..self.eff_strip_h {
            let ref_row = canvas_ref.row(dy as usize);
            // curr 底部 strip：y_offset + (curr 行数 - eff_strip_h + dy)
            let curr_bottom_start = (curr.data.len() / curr.width).saturating_sub(self.eff_strip_h as usize);
            let curr_row = curr.row((curr_bottom_start + dy as usize) + curr.y_offset);
            for &x in sample_cols {
                sad += (ref_row[x] as i32 - curr_row[x] as i32).unsigned_abs() as u64;
                count += 1;
            }
        }
        if count > 0 { sad as f64 / count as f64 } else { f64::MAX }
    }


    /// fallback 2D 反向验证：候选 dy 下，画布底部 strip 与当前帧重叠区的 2D 抽样 SAD。
    /// 用于 fallback（1D/best-guess）追加画布前的正确性校验——1D 行投影对图文混排易假匹配，
    /// 追加前用 2D 像素复验：按 dy 算出重叠区（curr 中紧贴 crop 区上方的已见内容），与画布
    /// 底部 strip 比 SAD；SAD 大说明 dy 错位 → 拒绝追加（skip，靠 Canvas-Anchored 下一帧恢复）。
    ///
    /// 几何：crop_y = eff_bottom - new_rows（当前帧新内容起点，与主匹配 process_frame_inner 一致）；
    /// 重叠区在 curr 中是 [crop_y - verify_rows, crop_y)（紧挨 crop 区上方），对应画布底部 strip。
    /// 返回每像素 SAD 均值（越大越不对齐）；样本不足/越界返回 f64::MAX（调用方按拒绝处理）。
    fn verify_alignment_2d(
        &self,
        canvas_gray: &GrayBuf,
        curr_gray: &GrayBuf,
        dy: f64,
        eff_top: u32,
        eff_bottom: u32,
        sample_cols: &[usize],
    ) -> f64 {
        if dy >= 0.0 || sample_cols.is_empty() {
            return f64::MAX;
        }
        if canvas_gray.width != curr_gray.width {
            return f64::MAX;
        }
        let new_rows = (-dy).round() as u32;
        if new_rows == 0 {
            return f64::MAX;
        }
        let crop_y = eff_bottom.saturating_sub(new_rows);
        if crop_y <= eff_top {
            return f64::MAX;
        }
        let canvas_h_actual = canvas_gray.data.len() / canvas_gray.width;
        // 重叠区行数：strip_h / canvas 实际行数 / curr 重叠可用行数（crop_y - eff_top）三者取 min
        let verify_rows = self.eff_strip_h
            .min(canvas_h_actual as u32)
            .min(crop_y - eff_top);
        if verify_rows == 0 {
            return f64::MAX;
        }
        let mut sad: u64 = 0;
        let mut count: u64 = 0;
        for i in 0..verify_rows as usize {
            // canvas 最底 verify_rows 行 vs curr 重叠区 [crop_y-verify_rows, crop_y)
            let canvas_row = canvas_gray.row(canvas_h_actual - verify_rows as usize + i);
            let curr_row = curr_gray.row(crop_y as usize - verify_rows as usize + i);
            for &x in sample_cols {
                sad += (canvas_row[x] as i32 - curr_row[x] as i32).unsigned_abs() as u64;
                count += 1;
            }
        }
        if count > 0 { sad as f64 / count as f64 } else { f64::MAX }
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

    /// 降级 3：1D 灰度投影匹配。
    /// 将每行像素按抽样列取均值降为一维信号，对一维信号做 SAD 搜索。
    /// 对纯色/低纹理场景（2D SAD 缺乏特征）更鲁棒。
    #[allow(clippy::too_many_arguments)]
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
        let strip_h = self.eff_strip_h;
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
    #[allow(clippy::too_many_arguments)]
    fn apply_fallback_match(
        &mut self,
        dy: f64,
        confidence: f64,
        best_sad: f64,
        frame: &RgbaImage,
        curr_gray: &GrayBuf,
        canvas_gray: &GrayBuf,
        sample_cols: &[usize],
        verify: bool,
        w: u32,
        eff_top: u32,
        eff_bottom: u32,
    ) -> Result<bool> {
        if dy >= 0.0 {
            self.last_dy = None;
            return Ok(false);
        }
        let new_rows = (-dy).round() as u32;
        let max_scroll_limit = (eff_bottom - eff_top) * 9 / 10;
        if new_rows < self.config.min_scroll_px as u32 || new_rows >= max_scroll_limit {
            self.last_dy = None;
            return Ok(false);
        }

        // 2D 反向验证（1D/best-guess 路径）：按候选 dy 算重叠区 SAD，超阈值说明 dy 错位，
        // 拒绝追加——skip 该帧，靠 Canvas-Anchored 下一帧从画布底部恢复匹配。
        // prev_frame 路径 verify=false：其 dy 已过内部 validate_ncc_match，且上一帧 skip 时
        // prev≠画布底部，本验证会误杀这根救命稻草。
        if verify {
            let sad = self.verify_alignment_2d(canvas_gray, curr_gray, dy, eff_top, eff_bottom, sample_cols);
            if sad > FALLBACK_VERIFY_SAD {
                log::info!(
                    "[stitch] fallback rejected by 2D verify: dy={:.1} sad={:.1} thresh={:.1} (conf={:.3}, 1d_sad={:.1})",
                    dy, sad, FALLBACK_VERIFY_SAD, confidence, best_sad
                );
                self.last_dy = None;
                return Ok(false);
            }
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

        Ok(true)
    }

    pub fn canvas(&mut self) -> &RgbaImage {
        if self.canvas_cache.is_none() {
            let rebuilt = match RgbaImage::from_raw(self.canvas_w, self.canvas_h, self.canvas_buf.clone()) {
                Some(img) => img,
                None => {
                    log::error!("canvas_buf 长度与 canvas_w/h 不匹配: {}x{} buf_len={}",
                        self.canvas_w, self.canvas_h, self.canvas_buf.len());
                    RgbaImage::new(1, 1)
                }
            };
            self.canvas_cache = Some(rebuilt);
        }
        self.canvas_cache.as_ref().unwrap()
    }

    pub fn height(&self) -> u32 { self.canvas_h }
    pub fn canvas_w(&self) -> u32 { self.canvas_w }

    /// 消费 self 一次性 move 出 canvas——避免 `canvas().clone()` 复制整张画布。
    ///
    /// 2026-07-17 性能优化（P0-2）：screenshot_commands stop 路径原先 3 次
    /// `canvas().clone()`（每次复制 1920×5000 RGBA ≈ 38MB，3 次 ≈ 114MB 峰值）。
    /// 改用本方法后无 clone——优先 move canvas_cache（若已构建），否则从 canvas_buf
    /// 重建一次。调用方消费 self 后不能再访问 Stitcher。
    pub fn into_canvas(mut self) -> RgbaImage {
        // 优先复用已构建的 cache（避免重建）
        if let Some(img) = self.canvas_cache.take() {
            return img;
        }
        match RgbaImage::from_raw(self.canvas_w, self.canvas_h, std::mem::take(&mut self.canvas_buf)) {
            Some(img) => img,
            None => {
                log::error!(
                    "into_canvas: canvas_buf 长度不匹配 {}x{} buf_len={}",
                    self.canvas_w, self.canvas_h, self.canvas_buf.len()
                );
                RgbaImage::new(1, 1)
            }
        }
    }

    /// 从 canvas_buf 中提取指定行范围 [y_start, y_start+height) 的 RGBA 字节切片。
    /// 用于生成预览，避免 clone 整个 canvas_buf。
    pub fn canvas_buf_slice(&self, y_start: u32, height: u32) -> Vec<u8> {
        let row_bytes = self.canvas_w as usize * 4;
        let start = y_start as usize * row_bytes;
        let end = start + height as usize * row_bytes;
        self.canvas_buf[start..end].to_vec()
    }

    pub fn finalize(&mut self, last_frame: &RgbaImage) -> Result<()> {
        let h = last_frame.height();
        let w = last_frame.width();
        // 防御性校验：帧宽度必须与画布一致
        if w != self.canvas_w {
            log::warn!("[stitch] finalize: frame width {} != canvas_w {}, skipping", w, self.canvas_w);
            return Ok(());
        }
        let eff_top = self.sticky_top;
        let eff_bottom = h.saturating_sub(self.sticky_bottom + self.content_tail);
        if eff_bottom <= eff_top {
            return Ok(());
        }

        // 1. NCC 匹配：将最后一帧与画布底部对齐，补全剩余未拼接区域
        let last_gray = GrayBuf::from_rgba_roi(last_frame, eff_top as usize, eff_bottom as usize);
        let canvas_gray = self.extract_canvas_bottom_gray(self.eff_strip_h);

        // Sobel 特征图 + NCC 匹配
        let (canvas_feat, canvas_has_feat) = to_feature_map(&canvas_gray);
        let (last_feat, last_has_feat) = to_feature_map(&last_gray);
        let (template, search_region) = if canvas_has_feat && last_has_feat {
            (canvas_feat, last_feat)
        } else {
            (canvas_gray.to_gray_image(), last_gray.to_gray_image())
        };

        if let Some(ncc) = ncc_match(&template, &search_region) {
            if validate_ncc_match(&ncc.response, ncc.best_y as usize, ncc.best_score as f32, self.config.ncc_score_threshold) {
                let refined_y = parabolic_refine_from_response(&ncc.response, ncc.best_y);
                let roi_height = (eff_bottom - eff_top) as f64;
                let new_rows_raw = roi_height - refined_y - self.eff_strip_h as f64;
                let dy = -new_rows_raw;

                if dy < 0.0 {
                    let new_rows = (-dy).round() as u32;
                    if new_rows < eff_bottom - eff_top {
                        log::info!("[stitch] finalize: stitching remaining {} rows (ncc={:.4})", new_rows, ncc.best_score);
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

    /// 检测当前帧底部"无内容常数尾"高度：从帧底部（跳过 sticky_bottom 区）往上逐行算
    /// R 通道 max-min，连续 max-min ≤ CONTENT_ROW_MAXMIN 的行数。纯黑/纯色空白行（暗色编辑器
    /// 内容不到底下方的纯黑区、滚动后期选区底部露出的背景）max-min≈0、无滚动信息；若 append
    /// 或画布底部停在此处，canvas-anchored 底部 strip 锚点退化（常数模板 NCC 假匹配 score≈1.0
    /// 或失配死锁）。
    ///
    /// 每帧基于当前帧检测（非首帧一次）：纯黑尾会动态变化——前期内容填满选区时无纯黑尾，
    /// 滚动后期内容上移、选区底部露出背景时纯黑尾才出现/增长。每帧 eff_bottom 止于真实内容底，
    /// append 永不带入纯黑尾 → 画布底部 strip 始终有特征。
    ///
    /// 与 sticky_bottom 互补：sticky_bottom 仅首帧一次、依赖逐像素相等，无法应对动态纯黑尾；
    /// 本方法每帧看单行内容是否有信息，更鲁棒。从 sticky_bottom 之上起扫，遇首个有内容行即停
    /// （不误裁行间空白）。返回原始暗尾高度（不 clamp）——strip 自适应（`effective_strip_for`）
    /// 已保证 content_h≥3*strip 留足搜索范围，整帧纯黑的退化输入由 process_frame 的
    /// `eff_bottom<=eff_top` 检查兜底（返回 Ok(false) 跳过）。
    fn detect_content_tail(&self, frame: &RgbaImage) -> u32 {
        self.scan_content_tail_in(frame.as_raw(), frame.height() as usize)
    }

    /// 在任意 RGBA 缓冲（当前帧 或 画布种子首帧）底部扫描"无内容暗常数尾"高度：跳过
    /// sticky_bottom 区，从底部往上逐行算 R 通道 max-min，连续 max-min≤CONTENT_ROW_MAXMIN
    /// 且最亮 luma<CONTENT_TAIL_MAX_LUMA 的行数。
    ///
    /// 抽出缓冲参数化的原因：init 裁剪画布种子（首帧）必须读**首帧自身**的暗尾，而非当前
    /// 第二帧的暗尾。首帧在 app 聚焦/滚动开始前由 setup 单独捕获，暗尾常大于已滚动后的第二帧；
    /// 用第二帧暗尾裁首帧会留残余暗尾 → 画布底部常数 → canvas_has=false 首帧即死锁（release
    /// 实测 296×160 矮选区"滚动不拼接"）。故 init 读 canvas_buf（=首帧）、每帧检测读 frame。
    fn scan_content_tail_in(&self, buf: &[u8], h: usize) -> u32 {
        let w = self.canvas_w as usize;
        let scan_bottom = h.saturating_sub(self.sticky_bottom as usize);
        if scan_bottom == 0 {
            return 0;
        }
        let row_bytes = w * 4;
        let mut tail = 0u32;
        for y in (0..scan_bottom).rev() {
            let row_start = y * row_bytes;
            let mut minv = u8::MAX;
            let mut maxv = 0u8;
            for x in 0..w {
                // R 通道近似 luma。暗常数尾判定：行内最暗最亮差值小（常数）且最亮仍暗
                // （纯黑/暗背景，luma < CONTENT_TAIL_MAX_LUMA）。纯渐变行每行虽可能常数
                // （max-min=0）但 luma 高 → 不误判；真实纯黑尾 luma≈0 → 判定。
                let v = buf[row_start + x * 4];
                if v < minv {
                    minv = v;
                }
                if v > maxv {
                    maxv = v;
                }
                // 一旦超出"暗常数"任一条件 → 该行有内容，无需扫完整行
                if maxv - minv > CONTENT_ROW_MAXMIN || maxv >= CONTENT_TAIL_MAX_LUMA {
                    break;
                }
            }
            if maxv - minv > CONTENT_ROW_MAXMIN || maxv >= CONTENT_TAIL_MAX_LUMA {
                break;
            }
            tail += 1;
        }
        tail
    }

    /// 自适应 strip 高度：内容高 < strip_h*3 时按 content_h/3 缩小 strip，留 2/3 作 NCC 搜索范围；
    /// 否则用配置 strip_h。MIN_STRIP 下限防退化。矮选区（如 162px 物理高含 80px 暗尾 → 内容 82px）
    /// 固定 80 strip 会吃光 ROI 使搜索范围≈0 → 首帧即失配死锁（2026-07-10 release 实测"滚动没拼接"）；
    /// 自适应后 strip≈27、搜索范围≈55，首帧即可锁定 dy。
    fn effective_strip_for(&self, content_h: u32) -> u32 {
        self.config
            .strip_h
            .min((content_h / 3).max(MIN_STRIP))
            .max(MIN_STRIP)
    }
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
                if (y + scroll_offset).is_multiple_of(45) {
                    v = 255 - v;
                }
                // 每 7 列亮列
                if x % 7 == 0 {
                    v = v.saturating_add(80);
                }
                // 确定性格点噪点（(x*3+y*5) % 11 == 0 处加亮）
                if (x * 3 + (y + scroll_offset) * 5).is_multiple_of(11) {
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
                        if (y + scroll_offset).is_multiple_of(5) { v = 255 - v; }
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

    /// 图文混排测试帧：左半纯色（仅 y 渐变，1D 行投影主导、易假匹配）+
    /// 右半密集条纹（水平每 5 行翻转 + 垂直每 3 列加亮，2D 才能区分）。
    /// 复刻滚动截图真实场景（图文混排页面），验证 2D 反向验证能识破 1D 假匹配。
    fn make_frame_text_mixed(width: u32, height: u32, scroll_offset: u32) -> RgbaImage {
        let mut img: ImageBuffer<Rgba<u8>, Vec<u8>> = ImageBuffer::new(width, height);
        let half = width / 2;
        for y in 0..height {
            for x in 0..width {
                let mut v = ((y + scroll_offset) % 256) as u8;
                if x >= half {
                    // 右半密集条纹：2D 特征丰富
                    if (y + scroll_offset).is_multiple_of(5) { v = 255 - v; }
                    if x % 3 == 0 { v = v.saturating_add(60); }
                }
                // 左半保持纯色渐变（1D 行投影在此无横向区分度）
                let px = Rgba([v, v, v, 255]);
                img.put_pixel(x, y, px);
            }
        }
        img
    }

    /// 测试 helper：从帧底部取 strip_h 行构造 y_offset=0 的 GrayBuf
    /// （复刻 extract_canvas_bottom_gray 语义，供 verify_alignment_2d 测试直接控制 canvas 侧）。
    fn canvas_bottom_strip(frame: &RgbaImage, strip_h: u32) -> GrayBuf {
        let w = frame.width() as usize;
        let h = frame.height();
        let mut data = Vec::with_capacity(strip_h as usize * w);
        for y in (h - strip_h)..h {
            for x in 0..w {
                let p = frame.get_pixel(x as u32, y);
                data.push(((2126 * p[0] as u32 + 7152 * p[1] as u32 + 722 * p[2] as u32) / 10000) as u8);
            }
        }
        GrayBuf { data, width: w, y_offset: 0 }
    }

    /// 测试 helper：抽样列（复刻 try_fallback 的 sample_cols 构造）。
    fn verify_sample_cols(width: u32) -> Vec<usize> {
        let xs = (width as f64 * X_START_RATIO) as usize;
        let xe = (width as f64 * X_END_RATIO) as usize;
        (xs..xe).step_by(SAMPLE_STEP_X).collect()
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
                img.put_pixel(x, y, *sticky_top.get_pixel(x, y));
            }
        }
        for y in 0..bot_h {
            for x in 0..width {
                img.put_pixel(x, height - bot_h + y, *sticky_bot.get_pixel(x, y));
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
        let added = s.process_frame(&f2).unwrap();
        assert!(added, "低纹理 1D 正确匹配应被 2D 验证放行，不应 skip");
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

        let bottom_gray = s.extract_canvas_bottom_gray(s.config.strip_h);
        assert_eq!(bottom_gray.width, TW as usize);

        // 手动从 canvas 计算底部 strip 灰度比对（canvas() 借用 s，须先取出 strip_h）
        let strip_h = s.config.strip_h;
        let canvas = s.canvas();
        let canvas_h = canvas.height();
        assert!(canvas_h >= strip_h);
        for y in 0..strip_h {
            for x in 0..TW {
                let px = canvas.get_pixel(x, canvas_h - strip_h + y);
                let luma = (2126 * px[0] as u32 + 7152 * px[1] as u32 + 722 * px[2] as u32) / 10000;
                assert_eq!(bottom_gray.row(y as usize)[x as usize], luma as u8,
                    "底部 strip 灰度不一致 @ ({},{})", x, y);
            }
        }
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

    /// 合成「暗色代码编辑器」帧：近黑背景 + 稀疏亮文字行（等宽字体感）。
    /// - 背景 luma≈12（近黑）
    /// - 行周期 24px：16px 文字行 + 8px 纯黑行间
    /// - 文字行内字符周期 11px（6px 亮 luma=220 + 5px 黑），模拟代码字符
    /// `scroll_offset` 模拟向下滚动。
    /// 复刻真实暗色编辑器：高灰度对比但 Sobel 特征稀疏（大片纯黑行间）。
    fn make_frame_dark_editor(width: u32, height: u32, scroll_offset: u32) -> RgbaImage {
        let mut img: ImageBuffer<Rgba<u8>, Vec<u8>> = ImageBuffer::new(width, height);
        for y in 0..height {
            for x in 0..width {
                let mut v: u8 = 12; // 近黑背景
                let line_y = (y + scroll_offset) % 24;
                if line_y < 16 {
                    // 文字行：等宽字符周期 11px（6 亮 + 5 暗）
                    let col_group = x % 11;
                    if col_group < 6 { v = 220; }
                }
                img.put_pixel(x, y, Rgba([v, v, v, 255]));
            }
        }
        img
    }

    /// 暗色编辑器本身不是问题：中等密度暗色文字行，Sobel 特征充足，NCC 能高分配中。
    /// 排除「暗色一律 NCC 失效」的简单假设——实测 score≈0.978。
    #[test]
    fn test_dark_editor_moderate_density_ncc_works() {
        let strip_h = StitchConfig::default().strip_h;
        let dark0 = make_frame_dark_editor(TW, TH, 0);
        let dark1 = make_frame_dark_editor(TW, TH, 30);
        let dark_strip = GrayBuf::from_rgba_roi(&dark0, (TH - strip_h) as usize, TH as usize);
        let dark_curr = GrayBuf::from_rgba_roi(&dark1, 0, TH as usize);
        let (dt, _) = to_feature_map(&dark_strip);
        let (ds, _) = to_feature_map(&dark_curr);
        let score = ncc_match(&dt, &ds).unwrap().best_score;
        eprintln!("DARK moderate-density score={:.4}", score);
        assert!(score > 0.65, "中等密度暗色帧 NCC 应命中（>0.65），实际 {:.4}", score);
    }

    /// 真实根因入口：选区底部 strip 落在大片纯黑区（编辑器空行/代码块间空白）时，
    /// Sobel 梯度全 0 → to_feature_map 返回 has_feat=false → 退化回灰度 NCC。
    /// 灰度模板（近全黑）零方差 → NCC 归一化分母≈0 → response 无区分度 →
    /// validate_ncc_match 拒绝（max-min<0.1 或 score<0.65）→ 连续失配 stuck。
    /// 复刻：底部 100px 涂纯黑，canvas 底部 80px strip 必然全黑 → 触发退化。
    #[test]
    fn test_dark_editor_bottom_strip_degrades_sobel() {
        let strip_h = StitchConfig::default().strip_h as usize;
        let black_zone = 100usize;
        let mut f0 = make_frame_dark_editor(TW, TH, 0);
        // 底部 black_zone 行涂纯黑（模拟代码块末尾空白/空行区）
        for y in (TH as usize - black_zone)..TH as usize {
            for x in 0..TW as usize {
                f0.put_pixel(x as u32, y as u32, Rgba([12, 12, 12, 255]));
            }
        }
        // canvas 底部 strip（80px）完全落在纯黑区
        let canvas_strip = GrayBuf::from_rgba_roi(&f0, TH as usize - strip_h, TH as usize);
        let (_feat, has_feat) = to_feature_map(&canvas_strip);
        assert!(!has_feat, "底部纯黑 strip 应触发 Sobel 退化（has_feat=false），这是暗色编辑器 NCC 失效的入口");
    }

    /// best_ncc_match 不回归：正常纹理帧 Sobel 路径命中，score > 阈值。
    /// （注：「Sobel 失配 + 灰度命中」无法用确定性合成帧复现——平移图的梯度也平移，
    ///   两者在合成帧上同步命中/失配；灰度兜底的真实收益依赖暗色低纹理场景的渲染
    ///   差异，靠 e2e + RUST_LOG=debug 的 sobel-ncc/gray-ncc 日志验证。）
    #[test]
    fn test_best_ncc_match_normal_frame_matched() {
        let strip_h = StitchConfig::default().strip_h;
        let f0 = make_frame(TW, TH, 0);
        let f1 = make_frame(TW, TH, 30);
        let s = Stitcher::new(f0.clone(), StitchConfig::default());
        let canvas_gray = GrayBuf::from_rgba_roi(&f0, (TH - strip_h) as usize, TH as usize);
        let curr_gray = GrayBuf::from_rgba_roi(&f1, 0, TH as usize);
        let outcome = s.best_ncc_match(&canvas_gray, &curr_gray, TW);
        match outcome {
            PrimaryOutcome::Matched(_y, score) => assert!(score > 0.65, "正常帧应高分匹配，实际 {:.4}", score),
            _ => panic!("正常纹理帧 best_ncc_match 应 Matched（Sobel 路径），实际失配=回归"),
        }
    }

    /// 纯色帧（双侧 Sobel 退化 has_feat=false）：best_ncc_match 直接判 Mismatch，
    /// 不走灰度兜底（常数模板必然 score≈1.0 假匹配），不误追加、不 panic。
    #[test]
    fn test_best_ncc_match_solid_frame_no_panic() {
        let solid: image::ImageBuffer<Rgba<u8>, Vec<u8>> =
            image::ImageBuffer::from_pixel(TW, TH, Rgba([30, 30, 30, 255]));
        let s = Stitcher::new(solid.clone(), StitchConfig::default());
        let canvas_gray = GrayBuf::from_rgba_roi(&solid, (TH - 80) as usize, TH as usize);
        let curr_gray = GrayBuf::from_rgba_roi(&solid, 0, TH as usize);
        match s.best_ncc_match(&canvas_gray, &curr_gray, TW) {
            PrimaryOutcome::Mismatch(_) => { /* 预期：双侧退化直接 Mismatch */ }
            _ => panic!("纯色双侧退化应 Mismatch，不该走灰度兜底假匹配"),
        }
    }

    /// 回归（release 实测 bug）：canvas 底部 strip 落暗色编辑器纯黑空白区
    /// （Sobel 退化 max_gradient=0）时，旧 gray fallback 对常数模板返回 score≈1.0
    /// 假匹配（release 日志 dy=-644.4 重复假帧 append 污染画布 + periodic false match
    /// sad=0.0）。修正后退化直接 Mismatch，不假匹配。curr 有正常纹理内容。
    #[test]
    fn test_best_ncc_match_constant_canvas_strip_no_false_match() {
        let curr = make_frame(TW, TH, 50); // curr 有纹理内容
        let s = Stitcher::new(curr.clone(), StitchConfig::default());
        // canvas strip 纯黑常数（暗色编辑器空白行 luma≈12）
        let black_strip: image::ImageBuffer<Rgba<u8>, Vec<u8>> =
            image::ImageBuffer::from_pixel(TW, 80, Rgba([12, 12, 12, 255]));
        let canvas_gray = GrayBuf::from_rgba_roi(&black_strip, 0, 80);
        let curr_gray = GrayBuf::from_rgba_roi(&curr, 0, TH as usize);
        match s.best_ncc_match(&canvas_gray, &curr_gray, TW) {
            PrimaryOutcome::Mismatch(_) => { /* 预期：canvas 常数退化，不假匹配 */ }
            PrimaryOutcome::Matched(_, _) => {
                panic!("常数 canvas strip 不应假匹配 score≈1.0，应交降级链");
            }
            PrimaryOutcome::SizeError => { /* 尺寸边界，可接受 */ }
        }
    }

    /// 回归（release 实测 bug，画布疯涨）：选区下半截恒纯黑 → prev_gray 底部 strip 也
    /// 纯黑常数。旧 try_match_prev_frame 在 tmpl 退化时回退灰度 → 常数模板 NCC 假匹配
    /// score=1.0 dy=-247.5 每帧采纳 → append 纯黑 → 画布疯涨（滚轮未动）。修正后
    /// prev-frame 任一侧退化返回 None，交下游 1D(dy=0)/stationary。
    #[test]
    fn test_try_match_prev_frame_constant_strip_no_false_match() {
        let curr = make_frame(TW, TH, 50); // curr 有纹理内容
        let s = Stitcher::new(curr.clone(), StitchConfig::default());
        // prev 整帧纯黑（选区下半截纯黑 → prev 底部 strip 必纯黑常数）
        let prev_black: image::ImageBuffer<Rgba<u8>, Vec<u8>> =
            image::ImageBuffer::from_pixel(TW, TH, Rgba([12, 12, 12, 255]));
        let prev_gray = GrayBuf::from_rgba_roi(&prev_black, 0, TH as usize);
        let curr_gray = GrayBuf::from_rgba_roi(&curr, 0, TH as usize);
        assert!(
            s.try_match_prev_frame(&prev_gray, &curr_gray, 0, TH).is_none(),
            "prev 底部纯黑退化时 prev-frame 应返回 None，不该 score≈1.0 假匹配 dy=-247.5"
        );
    }

    #[test]
    fn test_content_tail_black_bottom_still_stitches() {
        // 回归：选区上半截有滚动内容、下半截恒定纯黑（暗色编辑器内容不到底下方的空白）。
        // 真实场景纯黑尾常有光标/渲染差异，detect_sticky 的逐像素相等会漏检（sticky_bottom≈0），
        // 画布底部停在纯黑 → canvas-anchored 底部 strip 锚点永久退化（常数模板假匹配/失配死锁，
        // 2026-07-10 release 实测滚轮未动画布不增长）。content_tail 直接看单行 max-min 补救，
        // 裁掉纯黑尾后画布底部停在内容底（有特征），主匹配恢复。
        let content_h = 300u32;
        let black_tail = 200u32;
        let h = content_h + black_tail;

        // 上 content_h 行用 make_frame 内容，下 black_tail 行暗噪声（0~5）：逐像素不等让
        // detect_sticky 的逐像素相等漏检（sticky_bottom≈0），但单行 max-min<30 让
        // detect_content_tail 仍识别为无内容尾。f0/f1 用不同 noise_seed 确保逐像素不等。
        let make_with_tail = |scroll: u32, noise_seed: u32| -> RgbaImage {
            let mut img = make_frame(TW, h, scroll);
            for y in content_h..h {
                for x in 0..TW {
                    let n = ((x as u32 * y as u32 + noise_seed) % 6) as u8;
                    img.put_pixel(x, y, Rgba([n, n, n, 255]));
                }
            }
            img
        };

        let f0 = make_with_tail(0, 0);
        let mut s = Stitcher::new(f0, StitchConfig::default());

        // 第二帧（init）：不同 noise_seed → 纯黑尾逐像素不等 → sticky_bottom 漏检
        let f1 = make_with_tail(0, 3);
        s.process_frame(&f1).unwrap();

        assert!(
            s.content_tail >= black_tail / 2,
            "content_tail {} 应接近纯黑尾 {}（sticky_bottom 逐像素相等漏检后补救）",
            s.content_tail,
            black_tail
        );

        // 第三帧：内容滚动 40，纯黑尾仍暗噪声 → 应成功拼接（不再退化死锁）
        let f2 = make_with_tail(40, 3);
        let added = s.process_frame(&f2).unwrap();
        assert!(
            added,
            "纯黑尾裁掉后滚动内容应拼接成功（不再 canvas 底部纯黑退化死锁）"
        );
    }

    #[test]
    fn test_detect_content_tail_frame_based() {
        // 每帧基于当前帧检测（非首帧画布缓存）：同一 Stitcher 对不同帧返回不同 content_tail。
        let h = 500u32;
        let s = Stitcher::new(make_frame(TW, h, 0), StitchConfig::default());
        // 无纯黑尾帧 → 0
        assert_eq!(s.detect_content_tail(&make_frame(TW, h, 40)), 0);
        // 底部 120 行纯黑 → ≈120（clamp 内）
        let mut black_tail = make_frame(TW, h, 40);
        for y in (h - 120)..h {
            for x in 0..TW {
                black_tail.put_pixel(x, y, Rgba([0, 0, 0, 255]));
            }
        }
        let t = s.detect_content_tail(&black_tail);
        assert!(t >= 100, "纯黑尾 120 帧应返回≈120，实际 {}（基于帧，非首帧画布）", t);
    }

    #[test]
    fn test_content_tail_updates_each_frame() {
        // 回归（2026-07-10 "拼接一部分后停止"）：content_tail 每帧基于当前帧更新（非首帧缓存）。
        // 首帧无纯黑尾（=0）、后期帧出现纯黑尾时应动态增长。若退回首帧缓存，后期 eff_bottom
        // 不变 → append 带纯黑污染画布底部 → canvas strip 退化 → stuck 死锁。
        let h = 500u32;
        let mut s = Stitcher::new(make_frame(TW, h, 0), StitchConfig::default());
        s.process_frame(&make_frame(TW, h, 0)).unwrap(); // init
        assert_eq!(s.content_tail, 0, "首帧无纯黑尾");

        // 后期帧：底部 200 行纯黑（动态出现，内容仍连续滚动有新内容）
        let mut f2 = make_frame(TW, h, 40);
        for y in 300..h {
            for x in 0..TW {
                f2.put_pixel(x, y, Rgba([0, 0, 0, 255]));
            }
        }
        s.process_frame(&f2).unwrap();
        assert!(
            s.content_tail >= 80,
            "后期帧出现纯黑尾后 content_tail 应动态增长，实际 {}（每帧检测，非首帧缓存）",
            s.content_tail
        );
    }

    #[test]
    fn test_short_selection_with_dark_tail_stitches() {
        // 回归（2026-07-10 "滚动没拼接"）：矮选区（物理 162px 高，其中 80px 恒定暗尾）。
        // 旧逻辑 strip_h=80 固定 + content_tail clamp strip_h*3=240 > 162 → content_tail 强制 0、
        // 画布底部 strip 落暗尾 → canvas_has=false 首帧即死锁（release 实测 finalize 只拼 210 行）。
        // 修法：strip 按 content_h 自适应（min(80, content_h/3)）+ 移除 *3 clamp。
        // 此处 content_h=82 → eff_strip≈27，搜索范围≈55，首帧即可锁定 dy。
        let content_h = 82u32;
        let dark_tail = 80u32;
        let h = content_h + dark_tail; // 162

        // 上 content_h 行 make_frame 内容，下 dark_tail 行暗噪声（0~5）：逐像素不等让 detect_sticky
        // 漏检（sticky_bottom≈0），单行 max-min<30 + luma<40 让 content_tail 识别为暗尾。
        let make = |scroll: u32, noise_seed: u32| -> RgbaImage {
            let mut img = make_frame(TW, h, scroll);
            for y in content_h..h {
                for x in 0..TW {
                    let n = ((x as u32 * y as u32 + noise_seed) % 6) as u8;
                    img.put_pixel(x, y, Rgba([n, n, n, 255]));
                }
            }
            img
        };

        let f0 = make(0, 0);
        let mut s = Stitcher::new(f0, StitchConfig::default());
        // init：内容滚动 10（帧间内容在动 → sticky_top 小，不至吃光内容区）+ 不同 noise_seed
        // （暗尾逐像素不等 → sticky_bottom 漏检）。
        s.process_frame(&make(10, 3)).unwrap();

        // 暗尾应被识别（不再因 clamp 强制为 0）
        assert!(
            s.content_tail >= dark_tail / 2,
            "矮选区暗尾 {} 应被识别，content_tail={}（旧 *3 clamp 会强制为 0）",
            dark_tail,
            s.content_tail
        );
        // strip 应自适应缩小（固定 80 会吃光 82 内容区）
        assert!(
            s.eff_strip_h < s.config.strip_h,
            "矮选区 eff_strip_h 应 < 配置 {}，实际 {}",
            s.config.strip_h,
            s.eff_strip_h
        );

        // 第三帧滚动 20：应成功拼接（不再首帧死锁）
        let added = s.process_frame(&make(20, 3)).unwrap();
        assert!(
            added,
            "矮选区滚动内容应拼接成功（strip 自适应后搜索范围充足，不再 canvas 暗尾退化死锁）"
        );
    }

    /// 回归（2026-07-10 第 5 次"滚动不拼接"）：init 用第二帧 content_tail 裁首帧 canvas。
    /// 首帧（种子）在 app 聚焦/滚动开始前由 setup 单独捕获，暗尾常大于已滚动后的第二帧；
    /// 旧代码用第二帧小暗尾裁首帧大暗尾 → 残余暗尾留画布底部 → canvas_has=false 首帧死锁。
    /// 修法：init 读 canvas 种子缓冲测其【自身】暗尾裁剪。此测试构造首帧暗尾(100) > 第二帧
    /// 暗尾(40) 的场景，直接断言画布按种子自身暗尾裁到内容高 60（而非第二帧暗尾的 120）。
    #[test]
    fn test_seed_dark_tail_trimmed_by_own_measurement() {
        let seed_content = 60u32; // 首帧：内容 60 行 + 100 行暗尾（暗尾大）
        let later_content = 120u32; // 第二/三帧：内容 120 行 + 40 行暗尾（内容上移、暗尾缩小）
        let h = seed_content + 100; // 160

        // 上 content_rows 行 make_frame 内容，其余行暗噪声(0~5)：单行 max-min<30 + luma<40
        // → 识别为暗尾；不同 noise_seed 让暗尾逐像素不等 → detect_sticky 漏检 sticky_bottom。
        let make2 = |content_rows: u32, scroll: u32, noise_seed: u32| -> RgbaImage {
            let mut img = make_frame(TW, h, scroll);
            for y in content_rows..h {
                for x in 0..TW {
                    let n = ((x as u32 * y as u32 + noise_seed) % 6) as u8;
                    img.put_pixel(x, y, Rgba([n, n, n, 255]));
                }
            }
            img
        };

        let seed = make2(seed_content, 0, 0);
        let mut s = Stitcher::new(seed, StitchConfig::default());
        // init 帧：内容更多（暗尾更小=40）+ 不同 noise_seed（sticky_bottom 漏检）。
        s.process_frame(&make2(later_content, 5, 3)).unwrap();

        // 关键断言：画布应按【种子自身】暗尾(100)裁到内容高 60。
        // 旧代码用第二帧暗尾(40)裁 → canvas_h=120（残留 60 行暗尾 → canvas_has=false 死锁）。
        assert_eq!(
            s.height(),
            seed_content,
            "画布应按种子自身暗尾裁剪到 {}，实际 {}（旧代码用第二帧暗尾会留残余致首帧死锁）",
            seed_content,
            s.height(),
        );

        // 行为断言：第三帧滚动应拼接成功（画布底部=内容、canvas_has=true，不再死锁）。
        let added = s.process_frame(&make2(later_content, 10, 3)).unwrap();
        assert!(
            added,
            "种子暗尾正确裁剪后滚动内容应拼接成功（不再 canvas_has=false 首帧死锁）"
        );
    }

    /// 回归（2026-07-10 第 6 次"滚动不拼接"）：首帧在 app 聚焦前捕获为**整帧空白**（canvas 锚点
    /// 常数），canvas-anchored 架构永久死锁——content_tail 无内容可裁（整帧常数）、画布底部永远
    /// 常数 → canvas_has=false 每帧。日志时序铁证：activated app for scroll focus 出现在首条
    /// stitch 日志"之后"。修法：画布锚点常数时用当前内容帧重建（reseed_canvas_from）。
    #[test]
    fn test_blank_seed_reseeded_from_content_frame() {
        // 种子：app 聚焦前捕获的全黑空白帧
        let blank = image::ImageBuffer::from_pixel(TW, TH, Rgba([12, 12, 12, 255]));
        let mut s = Stitcher::new(blank, StitchConfig::default());
        // init 帧（app 仍未聚焦）：也空白
        s.process_frame(&image::ImageBuffer::from_pixel(TW, TH, Rgba([12, 12, 12, 255])))
            .unwrap();
        // 画布仍常数（init 无法裁空白种子的"暗尾"——整帧无内容，无暗尾可言）
        assert!(
            s.canvas_bottom_constant(),
            "空白种子后画布底部应常数（死锚），实际非常数"
        );

        // 第三帧：app 已聚焦，真实内容出现 → 应触发 reseed 重建锚点
        s.process_frame(&make_frame(TW, TH, 0)).unwrap();
        assert!(
            !s.canvas_bottom_constant(),
            "内容帧到达后画布应重建为有内容锚点（不再常数），实际仍常数"
        );

        // 第四帧滚动 30：画布锚点已恢复，应正常拼接
        let added = s.process_frame(&make_frame(TW, TH, 30)).unwrap();
        assert!(
            added,
            "画布 reseed 后滚动内容应拼接成功（不再空白锚点永久死锁）"
        );
    }

    /// 测试专用：向画布底部注入 `rows` 行纯色常数尾（RGBA=[value,value,value,255]），
    /// 模拟「滚动中画布底部变常数」——1D 假匹配 append 常数块、或滚到内容末尾露纯色背景。
    /// 直接污染 canvas_buf（绕过匹配链），精准复刻第 7 次回归的死锚场景。
    #[cfg(test)]
    impl Stitcher {
        fn inject_constant_canvas_tail(&mut self, rows: u32, value: u8) {
            let mut row: Vec<u8> = Vec::with_capacity(self.canvas_w as usize * 4);
            for _ in 0..self.canvas_w {
                row.extend_from_slice(&[value, value, value, 255]);
            }
            for _ in 0..rows {
                self.canvas_buf.extend_from_slice(&row);
            }
            self.canvas_h += rows;
            self.invalidate_cache();
        }
    }

    /// 回归（2026-07-10 第 7 次「拼接一部分后停止」）：旧 canvas_content_confirmed 一次性闸门
    /// 确认有内容后终身跳过死锚检查 → 滚动中画布底部再次变常数（滚到内容末尾露纯色背景 / 1D 假匹配
    /// append 常数块）时永久死锁（NCC stuck=5 stationary 到 finalize，finalize 灰度兜底对常数画布
    /// score≈1.0 假匹配拼错）。修法：每帧检查画布底 strip，常数则非破坏性裁掉常数尾（只丢空白，不丢
    /// 内容）恢复锚点；仅画布几乎全常数才 reseed。此测试注入常数尾模拟污染后验证自愈。
    #[test]
    fn test_canvas_constant_tail_trimmed_mid_stream() {
        let f0 = make_frame(TW, TH, 0);
        let mut s = Stitcher::new(f0, StitchConfig::default());
        s.process_frame(&make_frame(TW, TH, 0)).unwrap(); // init
        let h_init = s.height();
        // 累积滚动内容（画布增长、锚点已确认有内容——旧闸门此处置位后终身跳过检查）
        s.process_frame(&make_frame(TW, TH, 50)).unwrap();
        let h_content = s.height();
        assert!(h_content > h_init, "应已拼接增长：{} > {}", h_content, h_init);

        // 注入 150 行常数尾（模拟 1D 假匹配 append 常数块 / 滚到内容末尾露纯色背景）
        s.inject_constant_canvas_tail(150, 10);
        assert_eq!(s.height(), h_content + 150);
        assert!(s.canvas_bottom_constant(), "注入常数尾后画布底应常数（死锚）");

        // 下一帧滚动 100：画布底部常数 → 裁掉常数尾 → 锚点回到内容 → 继续拼接（非死锁）
        let added = s.process_frame(&make_frame(TW, TH, 100)).unwrap();
        assert!(
            added,
            "常数尾裁掉后应恢复拼接（不再死锁 stationary 到 finalize）"
        );
        // 注入的 150 常数行被裁，画布回到内容区并继续增长（不低于拼接内容高）
        assert!(
            s.height() >= h_content,
            "裁掉常数尾后画布不应低于内容区 {}，实际 {}",
            h_content,
            s.height()
        );
    }

    #[test]
    fn test_ncc_matches_known_offset() {
        let f0 = make_frame(TW, TH, 0);
        let f1 = make_frame(TW, TH, 30);
        let canvas_strip = GrayBuf::from_rgba_roi(&f0, (TH - StitchConfig::default().strip_h) as usize, TH as usize);
        let template = canvas_strip.to_gray_image();
        let search_region = GrayBuf::from_rgba_roi(&f1, 0, TH as usize).to_gray_image();
        let result = ncc_match(&template, &search_region);
        assert!(result.is_some(), "NCC 应返回匹配结果");
        let ncc = result.unwrap();
        assert!(ncc.best_score > 0.75, "NCC 分数应 > 0.75: {}", ncc.best_score);
    }

    #[test]
    fn test_ncc_match_range_finds_known_offset() {
        // f0 底部 strip（y∈[TH-strip_h,TH)）在 f1(scroll=30) 中出现在 y=TH-strip_h-30 处
        let strip_h = StitchConfig::default().strip_h;
        let f0 = make_frame(TW, TH, 0);
        let f1 = make_frame(TW, TH, 30);
        let template = GrayBuf::from_rgba_roi(&f0, (TH - strip_h) as usize, TH as usize).to_gray_image();
        let search = GrayBuf::from_rgba_roi(&f1, 0, TH as usize).to_gray_image();
        let expected_y = (TH - strip_h - 30) as f64; // 490
        let (refined_y, score) = ncc_match_range(&template, &search, expected_y - 5.0, expected_y + 5.0)
            .expect("range 内应匹配");
        assert!(
            (refined_y - expected_y).abs() < 2.0,
            "refined_y 应≈{}, 实际 {}", expected_y, refined_y
        );
        assert!(score > 0.5, "range 内匹配 score 应 > 0.5: {}", score);
    }

    #[test]
    fn test_ncc_match_range_rejects_out_of_range_offset() {
        // 真偏移 y=490，range 只给 [0,10] → 返回 range 内峰（≠490），refined_y < 15
        let strip_h = StitchConfig::default().strip_h;
        let f0 = make_frame(TW, TH, 0);
        let f1 = make_frame(TW, TH, 30);
        let template = GrayBuf::from_rgba_roi(&f0, (TH - strip_h) as usize, TH as usize).to_gray_image();
        let search = GrayBuf::from_rgba_roi(&f1, 0, TH as usize).to_gray_image();
        let (refined_y, _) = ncc_match_range(&template, &search, 0.0, 10.0)
            .expect("range 内应有某峰");
        assert!(
            refined_y < 15.0,
            "range 外偏移不应被选, refined_y={}", refined_y
        );
    }

    #[test]
    fn test_two_stage_refine_preserves_subpixel() {
        // 帧宽 TW=400。ncc_downsample_width=9999 → 单阶段；=200 → 两阶段(scale=0.5)。
        // f0 底部 strip 在 f1(scroll=40) 中 y=TH-strip_h-40=480 处。
        // 两阶段与单阶段 refined_y 误差应 < 0.5px（保亚像素）。
        let strip_h = StitchConfig::default().strip_h;
        let f0 = make_frame(TW, TH, 0);
        let f1 = make_frame(TW, TH, 40);
        let canvas_strip = GrayBuf::from_rgba_roi(&f0, (TH - strip_h) as usize, TH as usize);
        let curr_full = GrayBuf::from_rgba_roi(&f1, 0, TH as usize);
        let (tmpl, _) = to_feature_map(&canvas_strip);
        let (search, _) = to_feature_map(&curr_full);

        let s_single = Stitcher::new(f0.clone(), StitchConfig { ncc_downsample_width: 9999, ..Default::default() });
        let s_two = Stitcher::new(f0.clone(), StitchConfig { ncc_downsample_width: 200, ..Default::default() });

        let ry_single = match s_single.primary_ncc(&tmpl, &search, TW) {
            PrimaryOutcome::Matched(y, _) => y,
            _ => panic!("单阶段应匹配成功"),
        };
        let ry_two = match s_two.primary_ncc(&tmpl, &search, TW) {
            PrimaryOutcome::Matched(y, _) => y,
            _ => panic!("两阶段应匹配成功"),
        };
        assert!(
            (ry_two - ry_single).abs() < 0.5,
            "两阶段 refined_y 与单阶段误差应 <0.5px: single={}, two={}", ry_single, ry_two
        );
    }

    #[test]
    fn test_prev_frame_match_continuous_scroll() {
        // 相邻帧连续滚动：prev scroll=100, curr scroll=130（向下滚 30px）
        // try_match_prev_frame 应求出 dy≈-30
        let prev = GrayBuf::from_rgba_roi(&make_frame(TW, TH, 100), 0, TH as usize);
        let curr = GrayBuf::from_rgba_roi(&make_frame(TW, TH, 130), 0, TH as usize);
        let s = Stitcher::new(make_frame(TW, TH, 0), StitchConfig::default());
        let dy = s.try_match_prev_frame(&prev, &curr, 0, TH)
            .expect("相邻帧连续滚动应匹配成功");
        assert!(dy < 0.0, "向下滚 dy 应为负: {}", dy);
        assert!(
            (-dy - 30.0).abs() < 5.0,
            "dy 应≈-30（向下滚 30px），实际: {}", dy
        );
    }

    #[test]
    fn test_prev_frame_match_short_prev_returns_none() {
        // prev 有效区过短（< STRIP_H+10）→ 无法取底部 strip 模板 → None
        let short = GrayBuf {
            data: vec![128u8; TW as usize * 10],
            width: TW as usize,
            y_offset: 0,
        };
        let curr = GrayBuf::from_rgba_roi(&make_frame(TW, TH, 0), 0, TH as usize);
        let s = Stitcher::new(make_frame(TW, TH, 0), StitchConfig::default());
        assert!(
            s.try_match_prev_frame(&short, &curr, 0, TH).is_none(),
            "过短的 prev 不应给出匹配"
        );
    }

    // ===== verify_alignment_2d 单元测试（fallback 2D 反向验证）=====

    #[test]
    fn test_verify_alignment_2d_correct_offset_low_sad() {
        // f0 → f2 scroll=30，正确 dy=-30：重叠区应完美对齐，SAD 极低
        let strip_h = StitchConfig::default().strip_h;
        let f0 = make_frame(TW, TH, 0);
        let f2 = make_frame(TW, TH, 30);
        let canvas_gray = canvas_bottom_strip(&f0, strip_h);
        let curr_gray = GrayBuf::from_rgba_roi(&f2, 0, TH as usize);
        let cols = verify_sample_cols(TW);
        let s = Stitcher::new(f0, StitchConfig::default());
        let sad = s.verify_alignment_2d(&canvas_gray, &curr_gray, -30.0, 0, TH, &cols);
        assert!(sad < 5.0, "正确对齐 SAD 应 <5，实际 {}", sad);
    }

    #[test]
    fn test_verify_alignment_2d_wrong_offset_high_sad() {
        // 同场景但 dy=-60（多估 30px）：错位，SAD 应高于阈值
        let strip_h = StitchConfig::default().strip_h;
        let f0 = make_frame(TW, TH, 0);
        let f2 = make_frame(TW, TH, 30);
        let canvas_gray = canvas_bottom_strip(&f0, strip_h);
        let curr_gray = GrayBuf::from_rgba_roi(&f2, 0, TH as usize);
        let cols = verify_sample_cols(TW);
        let s = Stitcher::new(f0, StitchConfig::default());
        let sad = s.verify_alignment_2d(&canvas_gray, &curr_gray, -60.0, 0, TH, &cols);
        assert!(sad > FALLBACK_VERIFY_SAD, "错位 30px SAD 应 >{}，实际 {}", FALLBACK_VERIFY_SAD, sad);
    }

    #[test]
    fn test_verify_alignment_2d_text_mixed_rejects_false_match() {
        // 图文混排：正确 dy 低 SAD，错位 dy 高 SAD（2D 能区分右半条纹，1D 不能）
        let strip_h = StitchConfig::default().strip_h;
        let f0 = make_frame_text_mixed(TW, TH, 0);
        let f2 = make_frame_text_mixed(TW, TH, 30);
        let canvas_gray = canvas_bottom_strip(&f0, strip_h);
        let curr_gray = GrayBuf::from_rgba_roi(&f2, 0, TH as usize);
        let cols = verify_sample_cols(TW);
        let s = Stitcher::new(f0, StitchConfig::default());
        let sad_ok = s.verify_alignment_2d(&canvas_gray, &curr_gray, -30.0, 0, TH, &cols);
        let sad_bad = s.verify_alignment_2d(&canvas_gray, &curr_gray, -60.0, 0, TH, &cols);
        assert!(sad_ok < 5.0, "图文混排正确对齐 SAD 应 <5：{}", sad_ok);
        assert!(sad_bad > FALLBACK_VERIFY_SAD, "图文混排错位 SAD 应 >{}：{}", FALLBACK_VERIFY_SAD, sad_bad);
    }

    #[test]
    fn test_verify_alignment_2d_new_rows_one_no_panic() {
        // 极小位移 dy=-1：不越界、不除零
        let strip_h = StitchConfig::default().strip_h;
        let f0 = make_frame(TW, TH, 0);
        let f1 = make_frame(TW, TH, 1);
        let canvas_gray = canvas_bottom_strip(&f0, strip_h);
        let curr_gray = GrayBuf::from_rgba_roi(&f1, 0, TH as usize);
        let cols = verify_sample_cols(TW);
        let s = Stitcher::new(f0, StitchConfig::default());
        let sad = s.verify_alignment_2d(&canvas_gray, &curr_gray, -1.0, 0, TH, &cols);
        assert!(sad.is_finite(), "dy=-1 不应 panic/NaN：{}", sad);
    }

    #[test]
    fn test_verify_alignment_2d_large_offset_no_oob() {
        // 大位移 scroll=200（>strip_h=80）：不越界 + 正确对齐 SAD 低
        let strip_h = StitchConfig::default().strip_h;
        let f0 = make_frame(TW, TH, 0);
        let f_big = make_frame(TW, TH, 200);
        let canvas_gray = canvas_bottom_strip(&f0, strip_h);
        let curr_gray = GrayBuf::from_rgba_roi(&f_big, 0, TH as usize);
        let cols = verify_sample_cols(TW);
        let s = Stitcher::new(f0, StitchConfig::default());
        let sad = s.verify_alignment_2d(&canvas_gray, &curr_gray, -200.0, 0, TH, &cols);
        assert!(sad.is_finite(), "大位移不应越界：{}", sad);
        assert!(sad < 5.0, "正确大位移 SAD 应 <5：{}", sad);
    }

    #[test]
    fn test_verify_alignment_2d_defenses_return_max() {
        // 防御分支：空列 / dy>=0 返回 f64::MAX（按拒绝处理）
        let strip_h = StitchConfig::default().strip_h;
        let f0 = make_frame(TW, TH, 0);
        let canvas_gray = canvas_bottom_strip(&f0, strip_h);
        let curr_gray = GrayBuf::from_rgba_roi(&f0, 0, TH as usize);
        let s = Stitcher::new(f0, StitchConfig::default());
        assert_eq!(s.verify_alignment_2d(&canvas_gray, &curr_gray, -30.0, 0, TH, &[]), f64::MAX);
        assert_eq!(s.verify_alignment_2d(&canvas_gray, &curr_gray, 0.0, 0, TH, &verify_sample_cols(TW)), f64::MAX);
        assert_eq!(s.verify_alignment_2d(&canvas_gray, &curr_gray, 5.0, 0, TH, &verify_sample_cols(TW)), f64::MAX);
    }

    // ===== fallback 链集成测试 =====

    #[test]
    fn test_fallback_1d_false_match_rejected_by_2d_verify() {
        // 核心回归：fallback 给出错位 dy 时，2D 验证拒绝追加，画布不被污染（C3 bug 场景）。
        // 直接测 apply_fallback_match(verify=true)：图文混排帧，真实 dy=-30，故意传 -60。
        let f0 = make_frame_text_mixed(TW, TH, 0);
        let mut s = Stitcher::new(f0, StitchConfig::default()); // 画布 = f0（不 init，避开合成帧 sticky 误检）
        let h_before = s.height();

        let f2 = make_frame_text_mixed(TW, TH, 30);
        let strip_h = StitchConfig::default().strip_h;
        let curr_gray = GrayBuf::from_rgba_roi(&f2, 0, TH as usize);
        let cols = verify_sample_cols(TW);

        // 错位 dy=-60（conf=0.3574 复刻 C3）：2D 验证应拒绝，画布不增长
        let canvas_gray = s.extract_canvas_bottom_gray(strip_h);
        let rejected = s.apply_fallback_match(
            -60.0, 0.3574, 8.0, &f2, &curr_gray, &canvas_gray, &cols, true, TW, 0, TH,
        ).unwrap();
        assert!(!rejected, "错位 dy 应被 2D 验证拒绝");
        assert_eq!(s.height(), h_before, "拒绝时画布不应增长（不污染）");

        // 正确 dy=-30 应被放行，画布增长 30
        let canvas_gray = s.extract_canvas_bottom_gray(strip_h);
        let accepted = s.apply_fallback_match(
            -30.0, 0.9, 2.0, &f2, &curr_gray, &canvas_gray, &cols, true, TW, 0, TH,
        ).unwrap();
        assert!(accepted, "正确 dy 应被 2D 验证放行");
        assert_eq!(s.height(), h_before + 30, "正确 dy 追加 30 行");
    }

    #[test]
    fn test_fallback_prev_frame_not_blocked_by_2d_verify() {
        // prev_frame 路径 verify=false：即使 canvas 与 curr 在该 dy 下不对齐（模拟上一帧 skip 后
        // prev≠画布底部），也不被 2D 验证拦截——信任 prev_frame 内部 validate_ncc_match。
        let f0 = make_frame_text_mixed(TW, TH, 0);
        let mut s = Stitcher::new(f0, StitchConfig::default());
        let f1 = make_frame_text_mixed(TW, TH, 0);
        s.process_frame(&f1).unwrap();
        let h_before = s.height();

        let f2 = make_frame_text_mixed(TW, TH, 30);
        let strip_h = StitchConfig::default().strip_h;
        let canvas_gray = s.extract_canvas_bottom_gray(strip_h);
        let curr_gray = GrayBuf::from_rgba_roi(&f2, 0, TH as usize);
        let cols = verify_sample_cols(TW);
        // 故意传与 canvas 不对齐的 dy=-60，但 verify=false → 不验证，直接追加
        let accepted = s.apply_fallback_match(
            -60.0, 0.0, 0.0, &f2, &curr_gray, &canvas_gray, &cols, false, TW, 0, TH,
        ).unwrap();
        assert!(accepted, "prev_frame 路径 verify=false 不应被 2D 验证拦截");
        assert!(s.height() > h_before, "prev_frame 应正常追加");
    }
}
