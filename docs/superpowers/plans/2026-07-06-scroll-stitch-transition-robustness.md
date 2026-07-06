# 滚动拼接内容突变鲁棒性（方向 1 相邻帧参考）Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: superpowers:executing-plans（inline 执行，批量 + checkpoint）。单文件聚焦、需反复编译测试。

**Goal:** 给 `Stitcher` 加相邻帧参考 fallback 层——突变帧主 NCC 失配时，用**前一帧**有效区匹配当前帧求出正确 dy，消除 best-guess 盲 append 污染画布 + 熔断永久卡死，突变场景成功率 → 99%。

**Architecture:** 见 `docs/superpowers/specs/2026-07-06-scroll-stitch-transition-robustness-design.md` §3。改 `crates/capx/src/stitch.rs`：① +`prev_gray` 字段；② 提取 `process_frame_inner` 以统一更新 `prev_gray`；③ +`try_match_prev_frame` 方法；④ `try_fallback` 在 1D 投影前插入相邻帧层。`Stitcher` 公共接口零变更。

**Tech Stack:** Rust + imageproc。测试 `cargo test --manifest-path <WT>/crates/capx/Cargo.toml`。

**约定：** cargo 带 `--manifest-path` 指 worktree（worktree-cwd-trap）；git 用 `git -C <WT>` 绝对路径。

> **实现状态（2026-07-06 收尾）**：✅ Task 1-4 全部完成（`7cb9bb6` 合 main），capx 21 测绿 + desktop 编译通过；Task 5 e2e 通过——「白底黑字文字→图片」突变场景拼接完整不断裂，相邻帧参考 fallback 触发确认。

---

## File Structure

| 文件 | 责任 | 动作 |
|---|---|---|
| `crates/capx/src/stitch.rs` | +`prev_gray` 字段 / +`try_match_prev_frame` / 提取 `process_frame_inner` / `try_fallback` 插层 / +2 单测 | 修改 |

---

## Task 1: 加 `prev_gray` 字段 + 提取 `process_frame_inner`

**Files:** Modify `crates/capx/src/stitch.rs`（`Stitcher` 结构体 + `new` + `process_frame`）

- [ ] **Step 1: `Stitcher` 加字段**（在 `same_dy_count` 后，:254）

```rust
    /// 连续相同 dy 追加次数。
    same_dy_count: u32,
    /// 上一帧的有效区灰度（相邻帧参考 fallback 用）。每帧 process_frame 末尾更新。
    prev_gray: Option<GrayBuf>,
```

- [ ] **Step 2: `new` 初始化**（在 `same_dy_count: 0,` 后，:275）

```rust
            same_dy_count: 0,
            prev_gray: None,
```

- [ ] **Step 3: 提取 `process_frame_inner`**

把 `process_frame` 中 `curr_gray`/`canvas_gray` 构建（原 :312-313）之后的全部逻辑（原 :315-438）搬入新方法 `process_frame_inner`，`process_frame` 改为构建灰度后调用 inner、统一更新 `prev_gray`。

改后 `process_frame`（:279 起，保留到 :311 校验/首帧/eff 计算，:312 起重构）：

```rust
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

        // 全有效区域灰度转换（不限制 ROI——快速滚动时内容可能出现在有效区任意位置）
        let roi_top = eff_top as usize;
        let roi_bottom = eff_bottom as usize;
        let curr_gray = GrayBuf::from_rgba_roi(frame, roi_top, roi_bottom);
        let canvas_gray = self.extract_canvas_bottom_gray(STRIP_H);

        let result = self.process_frame_inner(frame, &curr_gray, &canvas_gray, w, eff_top, eff_bottom);

        // 相邻帧参考 fallback：记录本帧有效区灰度，供下一帧用（突变时画布底部旧模板
        // 失配，改用紧邻前一帧——与当前帧重叠最大、突变边界共同特征——匹配）。
        self.prev_gray = Some(curr_gray);

        result
    }

    /// process_frame 的匹配主体（Sobel 特征 → NCC → 验证 → dy → 周期检测 → append）。
    /// 提取出来是为了让 process_frame 在调用后统一更新 prev_gray（避免散落 8 个 return 点）。
    fn process_frame_inner(
        &mut self,
        frame: &RgbaImage,
        curr_gray: &GrayBuf,
        canvas_gray: &GrayBuf,
        w: u32,
        eff_top: u32,
        eff_bottom: u32,
    ) -> Result<bool> {
        // 【原 :315-438 逻辑原样搬入，不变】
        // ... Sobel 特征 → NCC → validate_ncc_match / try_fallback → 亚像素 → dy → 周期检测 → append
    }
```

> **搬移要点：** 原 :315 起的 `let (canvas_feat, canvas_has_feat) = to_feature_map(&canvas_gray);` 到 :438 `Ok(true)` 整段，逐字搬入 `process_frame_inner`，只把缩进调到方法体内。逻辑、变量名、日志、return 值全部不变。`try_fallback` 调用处（原 :329/:345）签名不变（它本就是 `&mut self` 方法，能读 `self.prev_gray`）。

- [ ] **Step 4: 编译确认搬移无误**

Run: `cargo build --manifest-path <WT>/crates/capx/Cargo.toml`
Expected: 通过（纯搬移，无逻辑变更）。

---

## Task 2: `try_match_prev_frame` + `try_fallback` 插层

**Files:** Modify `crates/capx/src/stitch.rs`

- [ ] **Step 1: 新增 `try_match_prev_frame`**（放在 `try_fallback` 之前，:441 前）

```rust
    /// 相邻帧参考 fallback：用前一帧有效区底部 strip 当模板，在当前帧有效区做 NCC。
    /// 突变时画布底部旧模板（如文字）与当前帧（如图片）失配；前一帧与当前帧只差
    /// 一个 dy、突变边界是两帧共同特征、重叠最大 → 能求出正确 dy，避免 best-guess 盲 append。
    /// dy 推导与主匹配同公式（模板=上一时刻底部，search=当前帧）。
    fn try_match_prev_frame(
        &self,
        prev_gray: &GrayBuf,
        curr_gray: &GrayBuf,
        eff_top: u32,
        eff_bottom: u32,
    ) -> Option<f64> {
        let prev_h = prev_gray.data.len() / prev_gray.width;
        if prev_h < STRIP_H as usize + 10 {
            return None;
        }
        // prev 底部 STRIP_H 行裁为独立模板（y_offset 归零）
        let strip_rows = STRIP_H as usize;
        let prev_strip = GrayBuf {
            data: prev_gray.data[(prev_h - strip_rows) * prev_gray.width..].to_vec(),
            width: prev_gray.width,
            y_offset: 0,
        };
        let (tmpl_feat, tmpl_has) = to_feature_map(&prev_strip);
        let (curr_feat, curr_has) = to_feature_map(curr_gray);
        let (template, search_region) = if tmpl_has && curr_has {
            (tmpl_feat, curr_feat)
        } else {
            (prev_strip.to_gray_image(), curr_gray.to_gray_image())
        };
        let ncc = ncc_match(&template, &search_region)?;
        if !validate_ncc_match(&ncc.response, ncc.best_y as usize, ncc.best_score as f32) {
            return None;
        }
        let roi_height = (eff_bottom - eff_top) as f64;
        let refined_y = parabolic_refine_from_response(&ncc.response, ncc.best_y);
        let new_rows_raw = roi_height - refined_y - STRIP_H as f64;
        let dy = -new_rows_raw;
        if dy >= 0.0 {
            return None;
        }
        log::info!("[stitch] prev-frame NCC dy={:.1} (score={:.4})", dy, ncc.best_score);
        Some(dy)
    }
```

- [ ] **Step 2: `try_fallback` 在 1D 投影前插入相邻帧层**（原 :455 `// 降级：1D 灰度投影匹配` 之前）

```rust
        // 相邻帧参考 fallback（方向 1）：画布底部旧模板失配时，改用前一帧匹配当前帧。
        // 前一帧与当前帧重叠最大、突变边界共同特征 → 求出正确 dy，不盲 append 污染画布。
        if let Some(prev_gray) = &self.prev_gray {
            if let Some(dy) = self.try_match_prev_frame(prev_gray, curr_gray, eff_top, eff_bottom) {
                self.best_guess_streak = 0;
                self.ncc_stuck_count = 0;
                return self.apply_fallback_match(dy, 0.0, 0.0, frame, curr_gray, w, eff_top, eff_bottom);
            }
        }

        // 降级：1D 灰度投影匹配
        if let Some((dy, conf, sad)) = self.try_match_1d_projection( ... ) {  // 原逻辑不变
```

- [ ] **Step 3: 编译**

Run: `cargo build --manifest-path <WT>/crates/capx/Cargo.toml`
Expected: 通过。

---

## Task 3: 单测（2 个）

**Files:** Modify `crates/capx/src/stitch.rs`（`#[cfg(test)] mod tests`，追加到 :1150 前）

- [ ] **Step 1: 写 2 个测试**

```rust
    #[test]
    fn test_prev_frame_match_continuous_scroll() {
        // 相邻帧连续滚动：prev scroll=S, curr scroll=S+30（向下滚 30px）
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
```

> **端到端突变测试说明：** 合成帧难确定性复现"主失配 + 相邻帧救场"（make_frame_textured 共享渐变基础，主 NCC 多半不失配），故不构造脆弱的端到端断言；突变场景真实效果靠手动 e2e 验证（Task 5）。单测锁定相邻帧匹配的核心正确性（求出正确 dy）+ 边界（过短返回 None）。

- [ ] **Step 2: 跑测试**

Run: `cargo test --manifest-path <WT>/crates/capx/Cargo.toml`
Expected: 全绿（现有 ~18 + 新 2）。

---

## Task 4: Commit

```bash
git -C <WT> add crates/capx/src/stitch.rs
git -C <WT> commit -m "feat(capx): 相邻帧参考 fallback——突变帧用前一帧匹配求正确 dy,消除盲 append 污染"
```

---

## Task 5: 手动 e2e（用户）

无 e2e 基建。交付用户后重点验证：
- [ ] 「白底黑字文字 → 图片」突变滚动：长图完整、不断在突变点
- [ ] 正常滚动不回归（现有行为不变）
- [ ] 停止/保存/取消三模式 finalize 正常
- [ ] 若仍偶发失败：贴 `[stitch]` 日志，看相邻帧 NCC 是否触发、dy 是否合理
