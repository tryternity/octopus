//! Canvas 锚点自愈机制。
//!
//! 拆分自原 stitch.rs（2026-08-04），机械迁移无行为变更。
//! 6 个自愈机制：seed-tail trim / content_tail 检测 / canvas-bottom-constant 修剪 /
//! destructive reseed / adaptive strip_h / sticky 顶底检测。
//! 所有方法为 inherent method（split-impl），签名一字不改。

use super::*;

impl super::Stitcher {
    /// 使画布缓存失效。每次 append/truncate 后调用。
    #[inline]
    pub(crate) fn invalidate_cache(&mut self) {
        self.canvas_cache = None;
    }

    /// 从画布底部提取 strip_h 行 RGBA 转灰度，作为 Canvas-Anchored 匹配模板。
    /// 无论多少帧匹配失败，画布底部始终是最新已确认内容 → 消除累积漂移。
    pub(crate) fn extract_canvas_bottom_gray(&self, strip_h: u32) -> GrayBuf {
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
    pub(crate) fn canvas_bottom_constant(&self) -> bool {
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

    /// 画布底部连续“常数行”数（亮度无关——区别于 scan_content_tail_in 的「暗+常数」双判定）。
    /// 逐行从画布底往上累加抽样像素的运行 min/max，(max-min) ≥ CONTENT_ROW_MAXMIN 即命中内容行停止。
    /// 用于锚点自愈：画布底部常数（纯黑/纯白/纯灰背景，或 1D 假匹配 append 的常数块）时 Sobel 退化、
    /// 锚点失效；裁掉常数尾让锚点回到真实内容底。
    ///
    /// 运行 min/max 而非单行 max-min 的原因：垂直渐变（每行横向常数、但行间亮度递增）单行 max-min=0
    /// 会被误判常数，然其有 Sobel 垂直梯度、是可匹配内容。运行 min/max 累积多行后 diff≥阈值即停 →
    /// 渐变区不被误裁。纯色尾（所有行同值）diff 恒 0 → 全部计入尾。真实文字行（横向 max-min 大）
    /// 首行即触发停止。
    pub(crate) fn scan_canvas_constant_tail(&self) -> u32 {
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
    pub(crate) fn reseed_canvas_from(&mut self, frame: &RgbaImage, eff_top: u32, eff_bottom: u32) {
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

    pub(crate) fn detect_sticky(&mut self, frame: &RgbaImage) {
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

    /// 检测当前帧底部“无内容常数尾”高度：从帧底部（跳过 sticky_bottom 区）往上逐行算
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
    pub(crate) fn detect_content_tail(&self, frame: &RgbaImage) -> u32 {
        self.scan_content_tail_in(frame.as_raw(), frame.height() as usize)
    }

    /// 在任意 RGBA 缓冲（当前帧 或 画布种子首帧）底部扫描“无内容暗常数尾”高度：跳过
    /// sticky_bottom 区，从底部往上逐行算 R 通道 max-min，连续 max-min≤CONTENT_ROW_MAXMIN
    /// 且最亮 luma<CONTENT_TAIL_MAX_LUMA 的行数。
    ///
    /// 抽出缓冲参数化的原因：init 裁剪画布种子（首帧）必须读**首帧自身**的暗尾，而非当前
    /// 第二帧的暗尾。首帧在 app 聚焦/滚动开始前由 setup 单独捕获，暗尾常大于已滚动后的第二帧；
    /// 用第二帧暗尾裁首帧会留残余暗尾 → 画布底部常数 → canvas_has=false 首帧即死锁（release
    /// 实测 296×160 矮选区”滚动不拼接”）。故 init 读 canvas_buf（=首帧）、每帧检测读 frame。
    pub(crate) fn scan_content_tail_in(&self, buf: &[u8], h: usize) -> u32 {
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
                // 一旦超出“暗常数”任一条件 → 该行有内容，无需扫完整行
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
    /// 固定 80 strip 会吃光 ROI 使搜索范围≈0 → 首帧即失配死锁（2026-07-10 release 实测“滚动没拼接”）；
    /// 自适应后 strip≈27、搜索范围≈55，首帧即可锁定 dy。
    pub(crate) fn effective_strip_for(&self, content_h: u32) -> u32 {
        self.config
            .strip_h
            .min((content_h / 3).max(MIN_STRIP))
            .max(MIN_STRIP)
    }
}
