//! 五层降级链：NCC 失败时的兜底处理。
//!
//! 拆分自原 stitch.rs（2026-08-04），机械迁移无行为变更。
//! dispatcher try_fallback 按序尝试：邻帧参考 NCC → 1D 投影 → 静止检测 → best-guess。
//! 所有方法为 inherent method（split-impl），签名一字不改。

use super::*;

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
