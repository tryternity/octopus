//! 五层降级链：NCC 失败时的兜底处理。
//!
//! 拆分自原 stitch.rs（2026-08-04），机械迁移无行为变更。
//! dispatcher try_fallback 按序尝试：邻帧参考 NCC → 1D 投影 → 静止检测 → best-guess。
//! 所有方法为 inherent method（split-impl），签名一字不改。
//!
//! 2026-08-04 阶段 3：try_fallback 重写为迭代 5 个 `FallbackStep` trait 实现。
//! 每个 step 封装一种匹配策略 + 副作用（reset streak / clear history 等）。

use super::*;

// ===== 降级链 trait 抽象（2026-08-04 阶段 3）=====

/// 降级链单步的输出。dispatcher 据此决定链路走向。
#[derive(Debug)]
pub(crate) enum StepOutcome {
    /// 本步已应用（副作用 + apply_fallback_match 已在 step 内调用）。
    Applied(Result<bool>),
    /// 本步求出 dy 但未 apply，请求 dispatcher 走 apply_fallback_match(verify)。
    /// 保留扩展点——本次所有步骤都用 Applied。
    Candidate {
        dy: f64,
        confidence: f64,
        sad: f64,
        verify: bool,
    },
    /// 本步判定画面静止，链路应短路返回 Ok(false)。
    Stationary,
    /// 本步未匹配，继续下一步。
    Skip,
}

/// 步骤执行上下文。聚合步骤所需输入 + Stitcher 可变引用。
/// 显式列出字段，限制 step 只触与本步相关的输入。
pub(crate) struct FallbackCtx<'a> {
    pub stitcher: &'a mut Stitcher,
    pub frame: &'a RgbaImage,
    pub curr_gray: &'a GrayBuf,
    pub canvas_gray: &'a GrayBuf,
    pub w: u32,
    pub eff_top: u32,
    pub eff_bottom: u32,
    pub sample_cols: &'a [usize],
}

/// 降级链单步。每个实现封装一种 fallback 策略 + 其副作用。
pub(crate) trait FallbackStep {
    /// 步骤名（日志用）。
    fn name(&self) -> &'static str;
    /// 尝试本步降级。
    fn try_step(&mut self, ctx: &mut FallbackCtx) -> StepOutcome;
}

impl super::Stitcher {
    /// 相邻帧参考 fallback：用前一帧有效区底部 strip 当模板，在当前帧有效区做 NCC。
    /// 突变时画布底部旧模板（如文字）与当前帧（如图片）失配；前一帧与当前帧只差
    /// 一个 dy、突变边界是两帧共同特征、重叠最大 → 能求出正确 dy，避免 best-guess 盲 append。
    /// dy 推导与主匹配同公式（模板=上一时刻底部 strip，search=当前帧有效区）。
    pub(crate) fn try_match_prev_frame(
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
    pub(crate) fn try_fallback(
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

    /// 轻量静止检测：比较当前帧底部 strip 与画布底部 strip 的全局 SAD。
    pub(crate) fn quick_stationary_check(&self, curr: &GrayBuf, canvas_ref: &GrayBuf, sample_cols: &[usize]) -> f64 {
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
    pub(crate) fn verify_alignment_2d(
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
    pub(crate) fn estimate_dy_hint(&self) -> Option<f64> {
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
    pub(crate) fn try_match_1d_projection(
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
    pub(crate) fn apply_fallback_match(
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::tests::{
        canvas_bottom_strip, make_frame, make_frame_text_mixed, make_frame_textured, verify_sample_cols,
    };
    use image::Rgba;
    /// 默认测试尺寸：宽度需足够大让 X_START_RATIO..X_END_RATIO 区间有意义，
    /// 高度需远大于 STRIP_H + MAX_SCROLL。
    const TW: u32 = 400;
    const TH: u32 = 600;

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
