use anyhow::Result;
use image::RgbaImage;
use std::collections::VecDeque;

mod graybuf;
pub(crate) use graybuf::{GrayBuf, to_feature_map, row_projection_means};

mod ncc_match;
pub(crate) use ncc_match::{
    PrimaryOutcome,
    ncc_match, ncc_match_range, validate_ncc_match,
    parabolic_refine_from_response, downsample_grayimage,
};

mod canvas_heal;
mod fallback_chain;

// ===== 拼接算法调参常量 =====
// 按功能分组（2026-08-04 整理）：匹配阈值 / 采样几何 / 画布自愈 / 时序平滑。
// 改值前先读各 const 上方 doc 与对应实现（注释里有踩坑案例与历史决策）。

// --- 匹配阈值（fallback_chain 用）---

/// 静止判定阈值。dy=0 处的平均像素差值小于此值视为内容未滚动。
const STATIONARY_SAD: f64 = 2.0;
/// fallback（1D 投影 / best-guess）追加画布前的 2D 反向验证：重叠区每像素 SAD 均值上限。
/// 高于 STATIONARY_SAD 以吸收亚像素 .round() 误差 / 压缩噪声 / 渲染反锯齿差异。起步 15.0，
/// reject 日志（apply_fallback_match 内）便于线上标定后再收。详见 verify_alignment_2d。
const FALLBACK_VERIFY_SAD: f64 = 15.0;

// --- 采样几何（fallback_chain 2D 验证 / 静止检测共用）---

/// 排除最左侧的比例（通常有图标/树状图）。
const X_START_RATIO: f64 = 0.10;
/// 排除最右侧的比例截止点（通常有滚动条/时间戳），即保留 10%~80% 横向区间。
const X_END_RATIO: f64 = 0.80;
/// 列抽样步长（像素）。每隔此值采样一列，提供双倍空间特征解析度。
const SAMPLE_STEP_X: usize = 2;

// --- 画布锚点自愈（canvas_heal 用）---

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

// --- 时序平滑（dy_history 用）---

/// dy 历史长度（最近 N 帧位移，用于 best-guess 中位数估算与静止判断）。
const DY_HISTORY_LEN: usize = 8;

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
    pub(crate) canvas_w: u32,
    pub(crate) canvas_h: u32,
    /// 连续 RGBA 画布数据（真实数据源，增量 extend 追加）。
    pub(crate) canvas_buf: Vec<u8>,
    /// 惰性重建缓存。append 后置 None，canvas() 调用时按需重建。
    pub(crate) canvas_cache: Option<RgbaImage>,
    pub(crate) sticky_top: u32,
    pub(crate) sticky_bottom: u32,
    pub(crate) detected: bool,
    pub(crate) config: StitchConfig,
    /// 上一次成功拼接的滚动位移，用于软速度罚分防止周期跳变
    pub(crate) last_dy: Option<f64>,
    /// 最近若干帧的 dy 历史，用于时序平滑判断静止。
    pub(crate) dy_history: VecDeque<f64>,
    /// 连续 best-guess 次数（主匹配成功时归零，超过 3 次熔断）。
    pub(crate) best_guess_streak: u32,
    /// 连续 NCC 验证失败且 score 几乎相同的次数（检测“画面静止但 NCC 不匹配”状态）。
    pub(crate) ncc_stuck_count: u32,
    /// 上一次成功追加的 dy（用于检测连续相同 dy → 周期性假匹配/静止）。
    pub(crate) last_appended_dy: Option<f64>,
    /// 连续相同 dy 追加次数。
    pub(crate) same_dy_count: u32,
    /// 上一帧的有效区灰度（相邻帧参考 fallback 用）。每帧 process_frame 末尾更新。
    pub(crate) prev_gray: Option<GrayBuf>,
    /// 首帧底部"无内容常数尾"高度（如选区下半截恒定纯黑空白）。与 sticky_bottom 同为
    /// 应排除的底部固定区，但 sticky_bottom 依赖首/次帧逐像素相等（光标闪烁/抗锯齿/scrollbar
    /// 差异会漏检），content_tail 直接看单行 max-min 补缺口。裁掉后画布底部停在真实内容底。
    pub(crate) content_tail: u32,
    /// 自适应 strip 高度。矮选区（内容高 < strip_h*3，如 162px 物理高含 80px 暗尾 → 内容 82px）
    /// 时固定 80 strip 会吃光 ROI 使 NCC 搜索范围≈0 → 首帧即失配死锁；故按 content_h/3 缩小，
    /// 留 2/3 作搜索范围。每帧基于 content_h 更新；模板提取与匹配几何统一读此值（非 config.strip_h）。
    pub(crate) eff_strip_h: u32,
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
                        // 第三十轮 F1：补 same_dy_count = 0 复位——原缺复位导致第 4 帧 :318
                        // 永久命中（same_dy_count 恒 3）→ uniform 后每帧都被锁定，画布不再增长。
                        // 复位后每帧重新走 stationary check（多一道防线防 uniform 误判）。
                        log::info!("[stitch] uniform scroll detected (dy={:.0}, sad={:.1}), not locking", dy_rounded, stationary_sad);
                        self.same_dy_count = 0;
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

    /// 第二十九轮 P2-F3：canvas_buf 与 canvas_w/h 不匹配（数据严重损坏）时原返 1×1 黑图
    /// + log error——调用方拿到黑图继续编码/入库/剪贴板，用户得空白图且根因被掩盖。
    /// 改返 Result，让上层显式处理损坏（bail 中止截图流程）。
    pub fn canvas(&mut self) -> anyhow::Result<&RgbaImage> {
        if self.canvas_cache.is_none() {
            let rebuilt = match RgbaImage::from_raw(self.canvas_w, self.canvas_h, self.canvas_buf.clone()) {
                Some(img) => img,
                None => {
                    return Err(anyhow::anyhow!(
                        "canvas_buf 长度与 canvas_w/h 不匹配: {}x{} buf_len={}",
                        self.canvas_w, self.canvas_h, self.canvas_buf.len()
                    ));
                }
            };
            self.canvas_cache = Some(rebuilt);
        }
        Ok(self.canvas_cache.as_ref().unwrap())
    }

    pub fn height(&self) -> u32 { self.canvas_h }
    pub fn canvas_w(&self) -> u32 { self.canvas_w }

    /// 消费 self 一次性 move 出 canvas——避免 `canvas().clone()` 复制整张画布。
    ///
    /// 2026-07-17 性能优化（P0-2）：screenshot_commands stop 路径原先 3 次
    /// `canvas().clone()`（每次复制 1920×5000 RGBA ≈ 38MB，3 次 ≈ 114MB 峰值）。
    /// 改用本方法后无 clone——优先 move canvas_cache（若已构建），否则从 canvas_buf
    /// 重建一次。调用方消费 self 后不能再访问 Stitcher。
    /// 第二十九轮 P2-F3：同 canvas()——数据损坏时 bail 而非返 1×1 黑图。
    pub fn into_canvas(mut self) -> anyhow::Result<RgbaImage> {
        // 优先复用已构建的 cache（避免重建）
        if let Some(img) = self.canvas_cache.take() {
            return Ok(img);
        }
        match RgbaImage::from_raw(self.canvas_w, self.canvas_h, std::mem::take(&mut self.canvas_buf)) {
            Some(img) => Ok(img),
            None => Err(anyhow::anyhow!(
                "into_canvas: canvas_buf 长度不匹配 {}x{} buf_len={}",
                self.canvas_w, self.canvas_h, self.canvas_buf.len()
            )),
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
    pub(super) fn make_frame(width: u32, height: u32, scroll_offset: u32) -> RgbaImage {
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
    pub(super) fn make_frame_textured(width: u32, height: u32, scroll_offset: u32, texture_level: u32) -> RgbaImage {
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
    pub(super) fn make_frame_text_mixed(width: u32, height: u32, scroll_offset: u32) -> RgbaImage {
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
    pub(super) fn canvas_bottom_strip(frame: &RgbaImage, strip_h: u32) -> GrayBuf {
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
    pub(super) fn verify_sample_cols(width: u32) -> Vec<usize> {
        let xs = (width as f64 * X_START_RATIO) as usize;
        let xe = (width as f64 * X_END_RATIO) as usize;
        (xs..xe).step_by(SAMPLE_STEP_X).collect()
    }

    /// 构造一个带 sticky 顶/底区域的帧：顶部 `top_h` 行和底部 `bot_h` 行固定不变，
    /// 中间内容随 `scroll_offset` 变化。
    pub(super) fn make_frame_with_sticky(
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
        let canvas = s.canvas().unwrap().clone();
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
}
