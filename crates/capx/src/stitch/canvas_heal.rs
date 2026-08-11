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

    /// 画布底部连续"常数行"数（亮度无关——区别于 scan_content_tail_in 的「暗+常数」双判定）。
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
    pub(crate) fn detect_content_tail(&self, frame: &RgbaImage) -> u32 {
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
    pub(crate) fn effective_strip_for(&self, content_h: u32) -> u32 {
        self.config
            .strip_h
            .min((content_h / 3).max(MIN_STRIP))
            .max(MIN_STRIP)
    }
}

/// 测试专用：向画布底部注入 `rows` 行纯色常数尾（RGBA=[value,value,value,255]），
/// 模拟「滚动中画布底部变常数」——1D 假匹配 append 常数块、或滚到内容末尾露纯色背景。
/// 直接污染 canvas_buf（绕过匹配链），精准复刻第 7 次回归的死锚场景。
#[cfg(test)]
impl crate::stitch::Stitcher {
    pub(super) fn inject_constant_canvas_tail(&mut self, rows: u32, value: u8) {
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

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::tests::{make_frame, make_frame_with_sticky};
    use image::{ImageBuffer, Rgba};
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
        let canvas = s.canvas().unwrap();
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

    /// 合成「暗色代码编辑器」帧：近黑背景 + 稀疏亮文字行（等宽字体感）。
    ///
    /// - 背景 luma≈12（近黑）
    /// - 行周期 24px：16px 文字行 + 8px 纯黑行间
    /// - 文字行内字符周期 11px（6px 亮 luma=220 + 5px 黑），模拟代码字符
    ///
    /// `scroll_offset` 模拟向下滚动。复刻真实暗色编辑器：高灰度对比但 Sobel 特征稀疏（大片纯黑行间）。
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
                    let n = ((x * y + noise_seed) % 6) as u8;
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
                    let n = ((x * y + noise_seed) % 6) as u8;
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
                    let n = ((x * y + noise_seed) % 6) as u8;
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
}
