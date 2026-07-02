# NCC + Sobel 梯度匹配引擎重写 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development to implement this plan task-by-task.

**Goal:** 用 `imageproc` 库的 NCC + Sobel 梯度替换手写 SAD + 灰度，根治周期性假匹配。

**Architecture:** 保留 Canvas-Anchored 架构。每帧提取画布底部 strip → Sobel 梯度特征图 → `match_template` NCC 匹配 → 多道验证 → 抛物线亚像素插值。移除手写 SAD/纹理密度/动态阈值等调参补丁。

**关联文档:** [spec](../specs/2026-07-02-capx-ncc-sobel-design.md)

---

## 关键约束

1. **API 零改动**：`new/process_frame/finalize/canvas/height` 签名不变
2. **现有 18 测试必须保持全绿**（或调整测试以适应 NCC 特性）
3. **禁止同步到 main**，直到 e2e 实测通过
4. **imageproc 0.25 API 已确认**：`match_template`、`find_extremes`、`sobel_gradients` 均可用
5. **worktree**: `/Users/wudarui/workspace/agent/octopus/.worktrees/optimize-capx`

### imageproc API 关键细节

- `match_template(image: &GrayImage, template: &GrayImage, method)` → `Image<Luma<f32>>`
  - `image` = 搜索区域（大），`template` = 模板（小）
  - response 尺寸 = `(image.w - template.w + 1, image.h - template.h + 1)`
  - 模板和搜索区域宽度相同时 response 只有 1 列
- `MatchTemplateMethod::CrossCorrelationNormalized` = NCC，越大越好（1.0 完美）
- `find_extremes(&Image<Luma<T>>)` → `Extremes { max_value_location, min_value_location, ... }`
- `sobel_gradients(&GrayImage)` → `Image<Luma<u16>>`（注意是 u16 不是 u8）

---

## Task 1: GrayBuf 增强 + Sobel 特征图生成

**Files:** `crates/capx/src/stitch.rs`

- [ ] **Step 1: 在 `GrayBuf` impl 中新增 `to_gray_image` 方法**

```rust
impl GrayBuf {
    // ... 现有方法 ...

    /// 转为 image::GrayImage（供 imageproc 使用）
    fn to_gray_image(&self) -> image::GrayImage {
        let h = (self.data.len() / self.width) as u32;
        image::GrayImage::from_raw(self.width as u32, h, self.data.clone())
            .expect("GrayBuf → GrayImage 失败")
    }
}
```

- [ ] **Step 2: 在自由函数区新增 `to_feature_map`（Sobel + 归一化 + 纯色退化）**

```rust
use imageproc::gradients::sobel_gradients;

/// 将 GrayBuf 转为 Sobel 梯度特征图 + 归一化。
/// 纯色区域（max_gradient=0）返回 (空白, false)，调用方退回灰度。
fn to_feature_map(gray: &GrayBuf) -> (image::GrayImage, bool) {
    let luma_img = gray.to_gray_image();
    let gradients = sobel_gradients(&luma_img);

    let max_gradient = gradients.iter().map(|p| p[0]).max().unwrap_or(0);
    if max_gradient == 0 {
        return (image::GrayImage::new(luma_img.width(), luma_img.height()), false);
    }

    // 归一化：mean + 3σ
    let (mean, stddev) = mean_stddev(&gradients);
    let normalizer = (mean + 3.0 * stddev).max(1.0);

    let normalized = image::GrayImage::from_fn(gradients.width(), gradients.height(), |x, y| {
        let g = gradients.get_pixel(x, y)[0] as f32;
        let scaled = (g / normalizer) * 255.0;
        image::Luma([scaled.round().clamp(0.0, 255.0) as u8])
    });
    (normalized, true)
}

/// 计算灰度图的均值和标准差。
fn mean_stddev(img: &imageproc::definitions::Image<image::Luma<u16>>) -> (f32, f32) {
    let n = (img.width() * img.height()) as f32;
    let sum: f32 = img.iter().map(|p| p[0] as f32).sum();
    let mean = sum / n;
    let var: f32 = img.iter().map(|p| {
        let d = p[0] as f32 - mean;
        d * d
    }).sum::<f32>() / n;
    (mean, var.sqrt())
}
```

- [ ] **Step 3: 新增常量**

在常量块中追加：

```rust
// ===== NCC 匹配参数 =====
/// 最低 NCC 分数阈值
const NCC_SCORE_THRESHOLD: f32 = 0.75;
/// 局部置信度 delta：best vs 次优差值
const LOCAL_CONFIDENCE_DELTA: f32 = 0.005;
/// 全局置信度 delta：best vs 距离≥4px 的差值
const GLOBAL_CONFIDENCE_DELTA: f32 = 0.002;
/// 全局置信度最小距离（像素）
const GLOBAL_CONFIDENCE_MIN_DIST: usize = 4;
```

- [ ] **Step 4: 编译验证**

Run: `cargo check -p octopus-capx 2>&1 | tail -5`
Expected: `Finished`（可能有 unused warning，后续 task 消费）

- [ ] **Step 5: 测试验证**

Run: `cargo test -p octopus-capx 2>&1 | grep "test result"`
Expected: 18 passed

- [ ] **Step 6: Commit**

```bash
git add crates/capx/src/stitch.rs
git commit -m "feat(capx): GrayBuf::to_gray_image + Sobel 特征图生成 + NCC 常量"
```

---

## Task 2: NCC 匹配 + 多道验证

**Files:** `crates/capx/src/stitch.rs`

- [ ] **Step 1: 新增 `ncc_match` 自由函数**

```rust
use imageproc::template_matching::{match_template, find_extremes, MatchTemplateMethod};
use imageproc::definitions::Image;
use image::Luma;

/// NCC 匹配结果。
struct NccResult {
    best_y: f64,        // 最佳偏移（response 坐标）
    best_score: f64,    // NCC 分数 [0, 1]
    response: Image<Luma<f32>>,  // 完整 response map
}

/// NCC 匹配：在搜索区域中找模板的最佳对齐位置。
fn ncc_match(
    template: &image::GrayImage,
    search_region: &image::GrayImage,
) -> Option<NccResult> {
    // 模板必须严格小于搜索区域
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
```

- [ ] **Step 2: 新增 `validate_ncc_match` 多道验证函数**

```rust
/// 多道验证 NCC 匹配结果。
/// 返回 true 表示匹配可信。
fn validate_ncc_match(response: &Image<Luma<f32>>, best_y: usize, best_score: f32) -> bool {
    // 1. 最低分数
    if best_score < NCC_SCORE_THRESHOLD {
        return false;
    }

    let h = response.height() as usize;

    // 2. 局部置信度：best vs best±1 的最大值差
    let local_alt = {
        let mut alt = 0.0f32;
        if best_y > 0 {
            alt = alt.max(response.get_pixel(0, best_y as u32 - 1)[0]);
        }
        if best_y + 1 < h {
            alt = alt.max(response.get_pixel(0, best_y as u32 + 1)[0]);
        }
        alt
    };
    if best_score - local_alt < LOCAL_CONFIDENCE_DELTA {
        return false;
    }

    // 3. 全局置信度：best vs 距离≥GLOBAL_CONFIDENCE_MIN_DIST 的最大值差
    let distant_alt = {
        let mut alt = 0.0f32;
        for y in 0..h {
            if (y as isize - best_y as isize).unsigned_abs() >= GLOBAL_CONFIDENCE_MIN_DIST as isize {
                alt = alt.max(response.get_pixel(0, y as u32)[0]);
            }
        }
        alt
    };
    if best_score - distant_alt < GLOBAL_CONFIDENCE_DELTA {
        return false;
    }

    true
}
```

- [ ] **Step 3: 新增 `parabolic_refine_from_response` 亚像素插值**

```rust
/// 从 NCC response map 在最佳 y 处做抛物线拟合，返回亚像素偏移。
fn parabolic_refine_from_response(response: &Image<Luma<f32>>, best_y: f64) -> f64 {
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
```

- [ ] **Step 4: 编译 + 测试**

Run: `cargo test -p octopus-capx 2>&1 | grep "test result"`
Expected: 18 passed（新函数未调用，unused warning 预期）

- [ ] **Step 5: Commit**

```bash
git add crates/capx/src/stitch.rs
git commit -m "feat(capx): NCC 匹配 + 多道验证 + 亚像素插值（imageproc 库）"
```

---

## Task 3: process_frame 接入 NCC 匹配

**Files:** `crates/capx/src/stitch.rs`

> 这是核心改造——替换 `process_frame` 中的主匹配从 SAD 到 NCC。

- [ ] **Step 1: 修改 `process_frame` 主匹配分支**

找到当前的匹配代码段（`let curr_buf = GrayBuf::from_rgba_roi(...)` 到 `let (dy, confidence, best_sad) = match find_overlap_spatial_ext(...)`），替换为 NCC 流程：

```rust
        // ROI 灰度转换：覆盖最大可能搜索范围
        let roi_top = eff_top.max(eff_bottom.saturating_sub(STRIP_H + MAX_SCROLL * 2)) as usize;
        let roi_bottom = eff_bottom as usize;
        let curr_gray = GrayBuf::from_rgba_roi(frame, roi_top, roi_bottom);
        let canvas_gray = self.extract_canvas_bottom_gray(STRIP_H);

        // Sobel 特征图 + 纯色退化
        let (canvas_feat, canvas_has_feat) = to_feature_map(&canvas_gray);
        let (curr_feat, curr_has_feat) = to_feature_map(&curr_gray);
        let (template, search_region) = if canvas_has_feat && curr_has_feat {
            (canvas_feat, curr_feat)
        } else {
            (canvas_gray.to_gray_image(), curr_gray.to_gray_image())
        };

        // NCC 匹配
        let ncc = match ncc_match(&template, &search_region) {
            Some(r) => r,
            None => {
                // 尺寸不合法，进入降级链
                log::info!("[stitch] ncc_match returned None (size mismatch)");
                return self.try_fallback(frame, &curr_gray, w, eff_top, eff_bottom);
            }
        };

        // 多道验证
        if !validate_ncc_match(&ncc.response, ncc.best_y as usize, ncc.best_score as f32) {
            log::info!("[stitch] NCC match failed validation (score={:.4})", ncc.best_score);
            return self.try_fallback(frame, &curr_gray, w, eff_top, eff_bottom);
        }

        // 亚像素插值
        let refined_y = parabolic_refine_from_response(&ncc.response, ncc.best_y);

        // 计算 dy：template 顶部在 curr 坐标系中的位置 - roi_top
        // response 的 y=0 对应 search_region 顶部 = roi_top
        let dy = -(refined_y + STRIP_H as f64); // 负号：用户向下滚动
```

> **注意 dy 计算**：NCC response 的 y 坐标表示模板在搜索区域中的对齐位置。response y=0 对应搜索区域顶部（roi_top）。模板高度 = STRIP_H。如果模板在搜索区域 y=dy_offset 处匹配，则意味着当前帧的 `[roi_top + dy_offset, roi_top + dy_offset + STRIP_H)` 行与画布底部一致 → 新增内容 = `[roi_top + dy_offset + STRIP_H, eff_bottom)` → new_rows = eff_bottom - (roi_top + dy_offset + STRIP_H)。但我们的 dy 约定是位移量（负值=向下滚），所以 dy = -(new_rows)。需要仔细推导坐标关系。

**坐标推导**（关键）：
- 画布底部 strip = canvas 最后 STRIP_H 行，在 canvas 坐标系中是 `[canvas_h - STRIP_H, canvas_h)`
- 当前帧 ROI = `[roi_top, eff_bottom)`
- NCC 搜索：模板（canvas strip）在搜索区域（curr ROI）中滑动
- response y = 模板顶部在 curr ROI 中的偏移量
- response y = 0：模板对齐 curr ROI 顶部 → canvas 底部 = curr ROI 顶部 → 无新内容
- response y = eff_bottom - roi_top - STRIP_H：模板对齐 curr ROI 底部 → 全是新内容
- **new_rows = (eff_bottom - roi_top) - response_y - STRIP_H**
- **dy = -(new_rows)**（负值=向下滚动）

```rust
        let roi_height = (eff_bottom - roi_top as u32) as f64;
        let new_rows_raw = roi_height - refined_y - STRIP_H as f64;
        let dy = -new_rows_raw;
```

- [ ] **Step 2: 移除 `find_overlap_spatial_ext` 调用及相关变量**

移除 `find_overlap_spatial_ext`、`decide_match`、`estimate_confidence`、`search_best_offset`、`extract_template`、`sparse_sad_at_offset` 的调用。但**先不删除函数定义**（Task 5 清理），只是不再调用。

- [ ] **Step 3: 调整后续检查逻辑**

主匹配成功后，dy 方向 + 幅度检查保留（`dy >= 0.0` 跳过、`new_rows` 范围检查），但：
- **移除 `is_stationary()` 双重校验**——NCC 在静止帧上会返回 score≈1.0 且 y 对齐正确（dy≈0），自然被处理
- **保留 dy_history 更新**

```rust
        // dy 方向检查
        if dy >= 0.0 {
            log::info!("[stitch] skipped frame: dy={:.1} >= 0.0 (ncc={:.4})", dy, ncc.best_score);
            self.dy_history.push_back(dy);
            if self.dy_history.len() > DY_HISTORY_LEN { self.dy_history.pop_front(); }
            return Ok(false);
        }

        let new_rows = (-dy).round() as u32;
        let max_scroll_limit = (eff_bottom - eff_top) * 4 / 5;

        if new_rows < self.config.min_scroll_px as u32 || new_rows >= max_scroll_limit {
            log::info!("[stitch] skipped frame: new_rows={} invalid (min={}, max={}) (ncc={:.4})",
                new_rows, self.config.min_scroll_px, max_scroll_limit, ncc.best_score);
            self.dy_history.push_back(dy);
            if self.dy_history.len() > DY_HISTORY_LEN { self.dy_history.pop_front(); }
            return Ok(false);
        }

        log::info!("[stitch] ncc={:.4} dy={:.1} new_rows={} canvas_h={}",
            ncc.best_score, dy, new_rows, self.canvas_h);

        // 主匹配成功：重置 best-guess 计数
        self.best_guess_streak = 0;

        // 画布追加 + 状态更新（不变）
        ...
```

- [ ] **Step 4: 抽取 `try_fallback` 方法**

把当前降级链（降级 1/2/3 + best-guess）封装为方法：

```rust
    fn try_fallback(
        &mut self,
        frame: &RgbaImage,
        curr_gray: &GrayBuf,
        w: u32,
        eff_top: u32,
        eff_bottom: u32,
    ) -> Result<bool> {
        // 保留 1D 投影降级 + best-guess
        // 移除降级 1（扩大搜索范围）和降级 2（缩小模板）——NCC 已覆盖

        // 降级：1D 灰度投影匹配
        let canvas_ref = self.extract_canvas_bottom_gray(STRIP_H);
        let x_start = (w as f64 * X_START_RATIO) as u32;
        let x_end = (w as f64 * X_END_RATIO) as u32;
        if let Some((dy, conf, sad)) = self.try_match_1d_projection(
            &canvas_ref, curr_gray, x_start, x_end, eff_top, eff_bottom, MAX_SCROLL, 0.0,
        ) {
            log::info!("[stitch] fallback: 1D projection match, dy={:.1} conf={:.4}", dy, conf);
            self.best_guess_streak = 0;
            return self.apply_fallback_match(dy, conf, sad, frame, curr_gray, w, eff_top, eff_bottom);
        }

        // 静止检测 + best-guess
        let sample_cols: Vec<usize> = (x_start as usize..x_end as usize)
            .step_by(SAMPLE_STEP_X)
            .collect();
        let stationary_sad = self.quick_stationary_check(curr_gray, &canvas_ref, &sample_cols);
        if stationary_sad < STATIONARY_SAD {
            log::info!("[stitch] stationary detected before best-guess (sad={:.2})", stationary_sad);
            self.dy_history.clear();
            self.best_guess_streak = 0;
            self.last_dy = None;
            return Ok(false);
        }

        if self.best_guess_streak < 3 {
            if let Some(dy) = self.estimate_dy_hint() {
                log::info!("[stitch] best-guess dy={:.1} (streak={})", dy, self.best_guess_streak + 1);
                self.best_guess_streak += 1;
                return self.apply_fallback_match(dy, 0.0, 0.0, frame, curr_gray, w, eff_top, eff_bottom);
            }
        } else {
            log::info!("[stitch] best-guess circuit breaker tripped");
        }

        log::info!("[stitch] all fallbacks exhausted, skipping frame");
        self.last_dy = None;
        Ok(false)
    }
```

- [ ] **Step 5: 编译 + 测试**

Run: `cargo test -p octopus-capx 2>&1 | tail -15`
Expected: 可能有些测试失败——NCC 在合成图上的行为可能不同。

**如果测试失败**：
- `test_known_scroll_appends_rows`：NCC 在渐变+条纹合成图上应能匹配。检查 dy 计算是否正确（坐标关系）。
- `test_stationary_frame_returns_false`：静止帧 NCC 应返回高 score 但 dy≈0，被 `dy >= 0.0` 跳过。
- 如果合成图缺乏 Sobel 特征（纯渐变无边缘），`to_feature_map` 会退化到灰度——这应该仍然工作。

- [ ] **Step 6: Commit**

```bash
git add crates/capx/src/stitch.rs
git commit -m "feat(capx): process_frame 接入 NCC + Sobel 匹配（替换 SAD）"
```

---

## Task 4: finalize 接入 NCC 匹配

**Files:** `crates/capx/src/stitch.rs`

- [ ] **Step 1: 修改 `finalize` 中的匹配为 NCC**

```rust
        // ROI 灰度转换
        let roi_top = eff_top as usize;
        let last_gray = GrayBuf::from_rgba_roi(last_frame, roi_top, eff_bottom as usize);
        let canvas_gray = self.extract_canvas_bottom_gray(STRIP_H);

        // Sobel 特征图 + NCC 匹配
        let (canvas_feat, canvas_has_feat) = to_feature_map(&canvas_gray);
        let (last_feat, last_has_feat) = to_feature_map(&last_gray);
        let (template, search_region) = if canvas_has_feat && last_has_feat {
            (canvas_feat, last_feat)
        } else {
            (canvas_gray.to_gray_image(), last_gray.to_gray_image())
        };

        if let Some(ncc) = ncc_match(&template, &search_region) {
            if validate_ncc_match(&ncc.response, ncc.best_y as usize, ncc.best_score as f32) {
                let refined_y = parabolic_refine_from_response(&ncc.response, ncc.best_y);
                let roi_height = (eff_bottom - eff_top) as f64;
                let new_rows_raw = roi_height - refined_y - STRIP_H as f64;
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
```

- [ ] **Step 2: 编译 + 测试**

Run: `cargo test -p octopus-capx 2>&1 | grep "test result"`

- [ ] **Step 3: Commit**

```bash
git add crates/capx/src/stitch.rs
git commit -m "feat(capx): finalize 接入 NCC 匹配"
```

---

## Task 5: 清理废弃代码

**Files:** `crates/capx/src/stitch.rs`

- [ ] **Step 1: 删除不再调用的函数**

- `find_overlap_spatial_ext`（旧 SAD 主搜索）
- `decide_match`
- `search_best_offset`（整数 SAD 搜索）
- `extract_template`
- `estimate_confidence`
- `sparse_sad_at_offset`
- `estimate_texture_density`（Sobel 替代）
- `dynamic_sad_accept`（NCC 固定阈值）

- [ ] **Step 2: 删除不再使用的常量**

- `SAD_ACCEPT`
- `MIN_CONFIDENCE`（被 NCC_SCORE_THRESHOLD 替代）
- `SPEED_PENALTY`
- `TEXTURE_EDGE_THRESHOLD`
- `TEXTURE_BONUS_FACTOR`
- `SAD_BASELINE_MULTIPLIER`
- `SAD_BASELINE_PADDING`
- `SAD_BASELINE_ALPHA`
- `FALLBACK_STRIP_H`
- `FALLBACK_SAD_MULTIPLIER`
- `sad_baseline` 字段（不再需要 EMA 基线）

> **注意**：`STATIONARY_SAD`、`STATIONARY_DY_THRESHOLD`、`DY_HISTORY_LEN`、`MAX_SCROLL`、`STRIP_H`、`SAMPLE_STEP_X`、`X_START_RATIO`、`X_END_RATIO`、`STICKY_DETECT_MAX` 保留（仍在使用）。

- [ ] **Step 3: 编译 + 测试 + 零 warning**

Run: `cargo test -p octopus-capx 2>&1 | grep -E "test result|warning"`
Expected: 18+ passed，0 warning

- [ ] **Step 4: Commit**

```bash
git add crates/capx/src/stitch.rs
git commit -m "refactor(capx): 清理 SAD 废弃代码（search_best_offset/decide_match/estimate_confidence 等）"
```

---

## Task 6: 新增 NCC 特性测试

**Files:** `crates/capx/src/stitch.rs`

- [ ] **Step 1: 新增 Sobel 特征图测试**

```rust
    #[test]
    fn test_sobel_pure_color_degrades() {
        // 纯色帧：Sobel 无梯度 → 返回 (blank, false)
        let f = make_frame_textured(TW, TH, 0, 0); // texture_level=0 纯色
        let gray = GrayBuf::from_rgba_roi(&f, 0, TH as usize);
        let (feat, has_feat) = to_feature_map(&gray);
        assert!(!has_feat, "纯色帧应无 Sobel 特征");
    }

    #[test]
    fn test_sobel_textured_has_features() {
        // 密集条纹帧：Sobel 有梯度 → 返回 (有内容, true)
        let f = make_frame_textured(TW, TH, 0, 2); // texture_level=2 密集
        let gray = GrayBuf::from_rgba_roi(&f, 0, TH as usize);
        let (feat, has_feat) = to_feature_map(&gray);
        assert!(has_feat, "密集条纹帧应有 Sobel 特征");
    }
```

- [ ] **Step 2: 新增 NCC 匹配精度测试**

```rust
    #[test]
    fn test_ncc_matches_known_offset() {
        // 构造已知位移帧，验证 NCC 返回正确偏移
        let f0 = make_frame(TW, TH, 0);
        let f1 = make_frame(TW, TH, 30); // 滚动 30px
        let gray0 = GrayBuf::from_rgba_roi(&f0, 0, TH as usize);
        let gray1 = GrayBuf::from_rgba_roi(&f1, 0, TH as usize);
        let template_gray = gray0.to_gray_image(); // 整帧作为模板太大，用底部 strip
        // 提取 f0 底部 80 行作为模板
        let canvas_strip = GrayBuf::from_rgba_roi(&f0, (TH - STRIP_H) as usize, TH as usize);
        let template = canvas_strip.to_gray_image();
        let search_region = gray1.to_gray_image();
        let result = ncc_match(&template, &search_region);
        assert!(result.is_some(), "NCC 应返回匹配结果");
        let ncc = result.unwrap();
        assert!(ncc.best_score > 0.75, "NCC 分数应 > 0.75: {}", ncc.best_score);
    }
```

- [ ] **Step 3: 编译 + 运行全部测试**

Run: `cargo test -p octopus-capx 2>&1 | tail -15`
Expected: 20+ passed

- [ ] **Step 4: Commit**

```bash
git add crates/capx/src/stitch.rs
git commit -m "test(capx): 新增 Sobel 特征图 + NCC 匹配精度测试"
```

---

## Task 7: desktop 集成编译 + 文档同步

**Files:** `docs/architecture.md`

- [ ] **Step 1: desktop 编译验证**

Run: `cargo check -p octopus-desktop 2>&1 | grep -E "error|Finished"`

- [ ] **Step 2: 更新 architecture.md**

stitch 描述更新为 NCC + Sobel 版本。

- [ ] **Step 3: Commit**

```bash
git add docs/architecture.md
git commit -m "docs(capx): 同步 NCC + Sobel 匹配引擎到 architecture"
```

---

## 验收清单（e2e 实测前）

- [ ] `cargo test -p octopus-capx` 全绿（≥20）
- [ ] `cargo check -p octopus-desktop` 无错误
- [ ] API 零改动
- [ ] 0 warning
- [ ] 废弃 SAD 代码全部清理
