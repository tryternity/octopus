# 滚动拼接 F 配置外置 + D 缩放匹配 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: superpowers:executing-plans（inline 执行，批量 + checkpoint）。单文件聚焦、需反复编译测试，inline 比 subagent 往返快。

**Goal:** F 把 `STRIP_H`/`MAX_SCROLL`/`NCC_SCORE_THRESHOLD` 纳入 `StitchConfig`（字段化，默认值不变行为零变化）；D 大屏 NCC 两阶段降采样 refine（保亚像素精度）。`Stitcher` 公共接口零变更。

**Architecture:** 见 `docs/superpowers/specs/2026-07-06-scroll-stitch-config-downsample-design.md`。改 `crates/capx/src/stitch.rs`：① `StitchConfig` +4 字段 + `Default`；② 删 3 const + 引用替换 + `validate_ncc_match` 加 `threshold` 参数；③ +`downsample_grayimage` +`ncc_match_range` +`primary_ncc`/`PrimaryOutcome`；④ `process_frame_inner` 主 NCC 改走 `primary_ncc`。

**Tech Stack:** Rust + image（`imageops::resize` Triangle）+ imageproc。测试 `cargo test --manifest-path <WT>/crates/capx/Cargo.toml`。

**约定：** cargo 带 `--manifest-path` 指 worktree（worktree-cwd-trap）；git 用 `git -C <WT>` 绝对路径。`<WT>` = `/Users/wudarui/workspace/agent/octopus/.claude/worktrees/scroll-stitch-borrow`。

> **实现状态（2026-07-06）**：✅ Task 1-3 全部完成（F 字段化 `e53b5fe` + D 辅助 `8053665` + D 两阶段 `f1477be`），capx 24 测绿 + desktop check 通过。Task 4 文档同步中。

---

## File Structure

| 文件 | 责任 | 动作 |
|---|---|---|
| `crates/capx/src/stitch.rs` | StitchConfig +4 字段 / 删 3 const / validate 加参数 / +downsample +ncc_match_range +primary_ncc / process_frame_inner 改造 / +3 单测 | 修改 |

---

## Task 1: F 字段化（行为不变）

**Files:** Modify `crates/capx/src/stitch.rs`（`StitchConfig`:215 / `Default`:222 / `const`:8-31 / `validate_ncc_match`:160 / 引用处）

- [ ] **Step 1: `StitchConfig` 加 4 字段 + `Default`**（替换 :215-229）

```rust
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
```

- [ ] **Step 2: 删 3 const**（删 :8 `STRIP_H` / :10 `MAX_SCROLL` / :31 `NCC_SCORE_THRESHOLD`；保留 `STATIONARY_SAD`/`SAMPLE_STEP_X`/`X_*`/`DY_HISTORY_LEN`/`STICKY_DETECT_MAX`）

- [ ] **Step 3: `validate_ncc_match` 加 `threshold` 参数**（替换 :160-164）

```rust
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
    // …后续 min/max 区分度检测不变…
```

- [ ] **Step 4: 引用替换**（全文件 `STRIP_H`/`MAX_SCROLL`/`NCC_SCORE_THRESHOLD` → `self.config.*`；自由函数调用处传 threshold）

  - `process_frame:316` `extract_canvas_bottom_gray(STRIP_H)` → `extract_canvas_bottom_gray(self.config.strip_h)`
  - `process_frame_inner:382` `STRIP_H as f64` → `self.config.strip_h as f64`
  - `try_match_prev_frame` 内 `STRIP_H` → `self.config.strip_h`（:140-170 段，两处）
  - `MAX_SCROLL` 全部引用 → `self.config.max_scroll`（`grep -n MAX_SCROLL` 确认处数=5）
  - `validate_ncc_match` 调用处加 `self.config.ncc_score_threshold`（`process_frame_inner` + `try_match_prev_frame` 两处）

- [ ] **Step 5: 编译 + 全量测试**（行为不变）

Run: `cargo test --manifest-path <WT>/crates/capx/Cargo.toml`
Expected: 全绿（默认 `ncc_downsample_width=1920` > 测试帧宽 400 → 单阶段，零回归）。

- [ ] **Step 6: Commit**

```bash
git -C <WT> add crates/capx/src/stitch.rs
git -C <WT> commit -m "feat(capx): STRIP_H/MAX_SCROLL/NCC_SCORE_THRESHOLD 纳入 StitchConfig 字段化"
```

---

## Task 2: D 辅助函数（downsample + ncc_match_range，TDD）

**Files:** Modify `crates/capx/src/stitch.rs`（`ncc_match`:140 之后追加辅助函数 + 测试）

- [ ] **Step 1: 写 `ncc_match_range` 测试**（追加到 `#[cfg(test)] mod tests`）

```rust
#[test]
fn test_ncc_match_range_finds_known_offset() {
    // 构造 search（高 100）含已知模板偏移 y=40；range 覆盖 [35,45] 应返回 refined_y≈40
    let tmpl = make_textured_gray(20, 30);   // 辅助：纹理 GrayImage
    let mut search = image::GrayImage::new(20, 100);
    image::imageops::overlay(&mut search, &tmpl, 0, 40);
    let (refined_y, score) = ncc_match_range(&tmpl, &search, 35.0, 45.0)
        .expect("range 内应匹配");
    assert!((refined_y - 40.0).abs() < 1.0, "refined_y 应≈40, 实际 {}", refined_y);
    assert!(score > 0.5);
}

#[test]
fn test_ncc_match_range_rejects_out_of_range_offset() {
    // 偏移 y=80，range 只给 [0,10] → 返回的峰是 range 内最高（非 80），refined_y < 15
    let tmpl = make_textured_gray(20, 30);
    let mut search = image::GrayImage::new(20, 120);
    image::imageops::overlay(&mut search, &tmpl, 0, 80);
    let (refined_y, _) = ncc_match_range(&tmpl, &search, 0.0, 10.0)
        .expect("range 内应有某峰（即便非真偏移）");
    assert!(refined_y < 15.0, "range 外偏移不应被选, refined_y={}", refined_y);
}
```

> 辅助 `make_textured_gray(w, h)`：用渐变/噪声填 GrayImage（参考现有 `make_frame_textured` 的纹理生成，转 GrayImage）。若现有 helper 已够用则复用。

- [ ] **Step 2: 跑确认失败**（`ncc_match_range` 未定义）

Run: `cargo test --manifest-path <WT>/crates/capx/Cargo.toml test_ncc_match_range`
Expected: FAIL（函数未定义）。

- [ ] **Step 3: 实现 `downsample_grayimage` + `ncc_match_range`**（放 `ncc_match`:157 之后）

```rust
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
```

- [ ] **Step 4: 跑通过**

Run: `cargo test --manifest-path <WT>/crates/capx/Cargo.toml test_ncc_match_range`
Expected: PASS。

- [ ] **Step 5: Commit**

```bash
git -C <WT> add crates/capx/src/stitch.rs
git -C <WT> commit -m "feat(capx): +downsample_grayimage(Triangle) +ncc_match_range 邻域 NCC refine"
```

---

## Task 3: D primary_ncc + 主路径两阶段（精度回归 TDD）

**Files:** Modify `crates/capx/src/stitch.rs`（+`PrimaryOutcome`/`primary_ncc`；改 `process_frame_inner`:348-383）

- [ ] **Step 1: 写大屏精度回归测试**（追加到 `#[cfg(test)] mod tests`）

```rust
#[test]
fn test_two_stage_refine_preserves_subpixel() {
    // 帧宽 TW=400。ncc_downsample_width=200 → 触发两阶段(scale=0.5)；
    //                 ncc_downsample_width=9999 → 单阶段。两者 refined_y 误差应 < 0.5px。
    let f0 = make_frame_textured(TW, TH, 0);
    let f1 = make_frame_textured(TW, TH, 40); // 向下滚 40px
    let gray0 = GrayBuf::from_rgba_roi(&f0, 0, TH as usize);
    let gray1 = GrayBuf::from_rgba_roi(&f1, 0, TH as usize);
    let (tmpl, _) = to_feature_map(&gray0);
    let (search, _) = to_feature_map(&gray1);

    let s_single = Stitcher::new(f0.clone(), StitchConfig { ncc_downsample_width: 9999, ..Default::default() });
    let s_two = Stitcher::new(f0.clone(), StitchConfig { ncc_downsample_width: 200, ..Default::default() });

    let (ry_single, _) = match s_single.primary_ncc(&tmpl, &search, TW) {
        PrimaryOutcome::Matched(y, s) => (y, s),
        _ => panic!("单阶段应匹配成功"),
    };
    let (ry_two, _) = match s_two.primary_ncc(&tmpl, &search, TW) {
        PrimaryOutcome::Matched(y, s) => (y, s),
        _ => panic!("两阶段应匹配成功"),
    };
    assert!(
        (ry_two - ry_single).abs() < 0.5,
        "两阶段 refined_y 与单阶段误差应 <0.5px: single={}, two={}", ry_single, ry_two
    );
}
```

> `make_frame_textured` 用现有纹理帧 helper（若滚动 40px 的纹理帧 NCC score 不足，调大位移或换 `make_frame`）。`primary_ncc` 是 `&self`，构造 `Stitcher` 后直接调（不改状态）。

- [ ] **Step 2: 跑确认失败**（`primary_ncc`/`PrimaryOutcome` 未定义）

Run: `cargo test --manifest-path <WT>/crates/capx/Cargo.toml test_two_stage_refine_preserves_subpixel`
Expected: FAIL（编译错，`primary_ncc` 未定义）。

- [ ] **Step 3: 实现 `PrimaryOutcome` + `primary_ncc`**（放 `process_frame_inner` 之前）

```rust
/// 主 NCC 结果。
enum PrimaryOutcome {
    /// 亚像素 refined_y（原分辨率坐标） + best_score
    Matched(f64, f64),
    /// NCC validate 失败（附 score 供日志/stuck）
    Mismatch(f64),
    /// ncc_match 返回 None（template/search size 不匹配）
    SizeError,
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
        // stage2: 原分辨率 ±2px 邻域 refine
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
```

- [ ] **Step 4: `process_frame_inner` 改走 `primary_ncc`**（替换 :347-383 的「NCC 匹配 → 验证 → 亚像素 → 坐标推导」段）

  原 :347-383（`let ncc = match ncc_match...` 到 `let dy = -new_rows_raw;`）替换为：

```rust
        // 主 NCC（大屏两阶段 refine / 小屏单阶段）
        let (refined_y, best_score) = match self.primary_ncc(&template, &search_region, w) {
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
        let new_rows_raw = roi_height - refined_y - self.config.strip_h as f64;
        let dy = -new_rows_raw;
```

  > 后续 `if dy > 0.0` / `new_rows` / 周期检测 / append 段（原 :387-461）**不变**，仅 :439 日志的 `ncc.best_score` → `best_score`。

- [ ] **Step 5: 跑全量测试**

Run: `cargo test --manifest-path <WT>/crates/capx/Cargo.toml`
Expected: 全绿（含新精度回归 + ncc_match_range + 现有全部）。

- [ ] **Step 6: Commit**

```bash
git -C <WT> add crates/capx/src/stitch.rs
git -C <WT> commit -m "feat(capx): D 大屏 NCC 两阶段 refine——降采样粗定位+原分辨率邻域,保亚像素"
```

---

## Task 4: 文档同步 + 合并

- [ ] **Step 1: architecture.md 更新**（stitch 降级链补「大屏两阶段 refine」；StitchConfig 字段表补 4 字段）
- [ ] **Step 2: spec/plan 状态注释**（本 spec/plan 顶部加 ✅ 已实现合 main）
- [ ] **Step 3: 全量测试 + desktop 编译确认**

Run:
```
cargo test --manifest-path <WT>/crates/capx/Cargo.toml
CARGO_TARGET_DIR=/Users/wudarui/workspace/agent/octopus/target cargo build --manifest-path <WT>/crates/desktop/Cargo.toml
```
Expected: capx 全绿；desktop 编译通过（Stitcher 公共接口零变更）。

- [ ] **Step 4: 文档 commit**

```bash
git -C <WT> add docs/
git -C <WT> commit -m "docs: 同步 F 配置外置 + D 两阶段 refine 到 architecture/specs/plans"
```

- [ ] **Step 5: 合 main（用户确认后）**——finishing-a-development-branch 双向同步（worktree merge main → main ff-only）。
