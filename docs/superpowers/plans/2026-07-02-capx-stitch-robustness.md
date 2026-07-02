# 滚动拼接健壮性优化 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 通过时序平滑、动态自适应阈值、三级兜底降级链提升滚动截屏拼接健壮性，解决错位/丢内容/容易断，API 零改动。

**Architecture:** Stitcher 新增 `dy_history: VecDeque<f64>` 和 `sad_baseline: f64` 两个字段。`process_frame` 重构为主匹配 + 三级降级链（扩大范围→缩小模板→1D 投影）。`find_overlap_spatial_ext` 参数化 `sad_accept` 和 `strip_h`。`decide_match` 移除 `stationary < best + 1.0` 硬覆盖，静止判断改为 dy 时序双重校验上移到 `process_frame`。

**Tech Stack:** Rust 2021、image 0.25、std::collections::VecDeque。

**关联文档:** [spec](../specs/2026-07-02-capx-stitch-robustness-design.md)

---

## 关键约束（所有任务必须遵守）

1. **API 零改动**：`Stitcher::new/process_frame/finalize/canvas/height` 与 `capture::*` 签名不变。`desktop` 零改动。
2. **灰度公式不变**：`GrayBuf::from_rgba` 保持 `(2126*R + 7152*G + 722*B) / 10000`。
3. **dy 符号约定**：`dy < 0` = 用户向下滚动（内容上移）。
4. **现有 12 测试必须保持全绿**：每次改造后 `cargo test -p octopus-capx` 必须通过。
5. **worktree 路径**：所有命令在 `/Users/wudarui/workspace/agent/octopus/.worktrees/optimize-capx` 下执行。

---

## 文件结构

| 文件 | 职责 | 本次改动 |
|------|------|---------|
| `crates/capx/src/stitch.rs` | 滚动拼接 | 新增字段、常量、降级链、1D 投影、测试用例 |

---

## Task 1: 新增常量 + Stitcher 字段

**Files:**
- Modify: `crates/capx/src/stitch.rs`（常量块 + struct + new）

- [ ] **Step 1: 在常量块末尾（`STICKY_DETECT_MAX` 之后）追加新常量**

```rust
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
```

- [ ] **Step 2: 在文件顶部 `use` 语句中添加 `std::collections::VecDeque`**

旧：
```rust
use anyhow::Result;
use image::RgbaImage;
```

新：
```rust
use anyhow::Result;
use image::RgbaImage;
use std::collections::VecDeque;
```

- [ ] **Step 3: Stitcher struct 新增两个字段**

旧：
```rust
pub struct Stitcher {
    canvas_w: u32,
    canvas_h: u32,
    canvas_buf: Vec<u8>,
    canvas_cache: std::cell::UnsafeCell<Option<RgbaImage>>,
    reference: GrayBuf,
    sticky_top: u32,
    sticky_bottom: u32,
    detected: bool,
    config: StitchConfig,
    last_dy: Option<f64>,
}
```

新：
```rust
pub struct Stitcher {
    canvas_w: u32,
    canvas_h: u32,
    canvas_buf: Vec<u8>,
    canvas_cache: std::cell::UnsafeCell<Option<RgbaImage>>,
    reference: GrayBuf,
    sticky_top: u32,
    sticky_bottom: u32,
    detected: bool,
    config: StitchConfig,
    last_dy: Option<f64>,
    /// 最近若干帧的 dy 历史，用于时序平滑判断静止。
    dy_history: VecDeque<f64>,
    /// 历史成功匹配的 SAD 均值（EMA）。
    sad_baseline: f64,
}
```

- [ ] **Step 4: `new()` 初始化新字段**

旧：
```rust
    pub fn new(first_frame: RgbaImage, config: StitchConfig) -> Self {
        let w = first_frame.width();
        let h = first_frame.height();
        Self {
            canvas_w: w,
            canvas_h: h,
            canvas_buf: first_frame.into_raw(),
            canvas_cache: std::cell::UnsafeCell::new(None),
            reference: GrayBuf { data: Vec::new(), width: 0 },
            sticky_top: 0,
            sticky_bottom: 0,
            detected: false,
            config,
            last_dy: None,
        }
    }
```

新：
```rust
    pub fn new(first_frame: RgbaImage, config: StitchConfig) -> Self {
        let w = first_frame.width();
        let h = first_frame.height();
        Self {
            canvas_w: w,
            canvas_h: h,
            canvas_buf: first_frame.into_raw(),
            canvas_cache: std::cell::UnsafeCell::new(None),
            reference: GrayBuf { data: Vec::new(), width: 0 },
            sticky_top: 0,
            sticky_bottom: 0,
            detected: false,
            config,
            last_dy: None,
            dy_history: VecDeque::with_capacity(DY_HISTORY_LEN),
            sad_baseline: 0.0,
        }
    }
```

- [ ] **Step 5: 编译验证（新字段未使用会 warning，预期）**

Run:
```bash
cd /Users/wudarui/workspace/agent/octopus/.worktrees/optimize-capx && cargo check -p octopus-capx 2>&1 | tail -5
```
Expected: `Finished`，可能有 `unused` warning（后续 task 消费）。

- [ ] **Step 6: Commit**

```bash
cd /Users/wudarui/workspace/agent/octopus/.worktrees/optimize-capx
git add crates/capx/src/stitch.rs
git commit -m "feat(capx): 新增健壮性优化常量与 Stitcher 字段（dy_history/sad_baseline）"
```

---

## Task 2: 纹理密度评估 + 动态阈值

**Files:**
- Modify: `crates/capx/src/stitch.rs`（新增 `estimate_texture_density`、`dynamic_sad_accept`）

- [ ] **Step 1: 在 `GrayBuf` impl 块之后、`pub struct StitchConfig` 之前新增 `estimate_texture_density` 自由函数**

```rust
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
```

- [ ] **Step 2: 在 `impl Stitcher` 块内（`invalidate_cache` 之后、`canvas` 之前）新增 `dynamic_sad_accept` 方法**

```rust
    /// 根据当前帧纹理密度 + 历史 SAD 基线动态计算 SAD 接受阈值。
    fn dynamic_sad_accept(&self, texture: f64) -> f64 {
        // 纹理越丰富 → 绝对 SAD 天然更高 → 允许更高阈值
        let texture_bonus = texture * TEXTURE_BONUS_FACTOR;
        // 历史基线浮动：EMA 均值的倍数 + padding 作为上界
        let baseline_cap = self.sad_baseline * SAD_BASELINE_MULTIPLIER + SAD_BASELINE_PADDING;
        (SAD_ACCEPT + texture_bonus).min(baseline_cap).max(SAD_ACCEPT)
    }
```

- [ ] **Step 3: 编译验证**

Run:
```bash
cd /Users/wudarui/workspace/agent/octopus/.worktrees/optimize-capx && cargo check -p octopus-capx 2>&1 | tail -5
```
Expected: `Finished`，`estimate_texture_density` 和 `dynamic_sad_accept` 可能有 unused warning（后续 task 消费）。

- [ ] **Step 4: 现有测试验证无回归**

Run:
```bash
cd /Users/wudarui/workspace/agent/octopus/.worktrees/optimize-capx && cargo test -p octopus-capx 2>&1 | grep "test result"
```
Expected: 12 passed。

- [ ] **Step 5: Commit**

```bash
cd /Users/wudarui/workspace/agent/octopus/.worktrees/optimize-capx
git add crates/capx/src/stitch.rs
git commit -m "feat(capx): 新增纹理密度评估与动态 SAD 阈值计算"
```

---

## Task 3: 时序平滑静止判断

**Files:**
- Modify: `crates/capx/src/stitch.rs`（新增 `is_stationary` + 修改 `decide_match`）

- [ ] **Step 1: 在 `impl Stitcher` 块内（`dynamic_sad_accept` 之后）新增 `is_stationary` 方法**

```rust
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
```

- [ ] **Step 2: 修改 `decide_match` 签名和逻辑——移除 `stationary < best + 1.0` 硬覆盖，新增 `sad_accept` 参数**

旧：
```rust
fn decide_match(
    best_y_offset: u32,
    best_sad_avg: f64,
    stationary_sad_avg: f64,
    confidence: f64,
    template_y: u32,
) -> Option<(f64, f64)> {
    if stationary_sad_avg < STATIONARY_SAD || stationary_sad_avg < best_sad_avg + 1.0 {
        return Some((0.0, 1.0));
    }
    if best_sad_avg < SAD_ACCEPT && confidence > MIN_CONFIDENCE {
        let dy = best_y_offset as f64 - template_y as f64;
        Some((dy, confidence))
    } else {
        None
    }
}
```

新：
```rust
fn decide_match(
    best_y_offset: u32,
    best_sad_avg: f64,
    stationary_sad_avg: f64,
    confidence: f64,
    template_y: u32,
    sad_accept: f64,
) -> Option<(f64, f64)> {
    // 保留绝对静止快速路径（画面完全没动时 stationary_sad 极低）
    if stationary_sad_avg < STATIONARY_SAD {
        return Some((0.0, 1.0));
    }
    // 移除 stationary < best + 1.0 硬覆盖——交由 is_stationary() 时序判断
    if best_sad_avg < sad_accept && confidence > MIN_CONFIDENCE {
        let dy = best_y_offset as f64 - template_y as f64;
        Some((dy, confidence))
    } else {
        None
    }
}
```

- [ ] **Step 3: 修改 `find_overlap_spatial_ext` 签名——新增 `sad_accept` 和 `strip_h` 参数**

旧签名：
```rust
fn find_overlap_spatial_ext(
    ref_buf: &GrayBuf,
    curr_buf: &GrayBuf,
    x_start: u32,
    x_end: u32,
    eff_top: u32,
    eff_bottom: u32,
    max_scroll: u32,
    last_dy: Option<f64>,
) -> Option<(f64, f64)> {
```

新签名：
```rust
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
) -> Option<(f64, f64)> {
```

- [ ] **Step 4: 修改 `find_overlap_spatial_ext` 函数体——用 `strip_h` 替换 `STRIP_H`，用 `sad_accept` 传给 `decide_match`**

在函数体内，所有使用 `STRIP_H` 的地方改为参数 `strip_h`。具体替换点：
- `if eff_bottom <= eff_top + strip_h + 10`（原来是 `STRIP_H + 10`）
- `let template_y = eff_bottom - strip_h;`（原来是 `STRIP_H`）
- `extract_template(ref_buf, template_y, &sample_cols)` 内部仍用 `STRIP_H`——改为传 `strip_h` 参数（见 Step 5）

`decide_match` 调用处改为传入 `sad_accept`：
```rust
    // 旧
    let confidence = estimate_confidence(...);
    decide_match(best_y_offset, best_sad_avg, stationary_sad_avg, confidence, template_y)
    // 新
    let confidence = estimate_confidence(...);
    decide_match(best_y_offset, best_sad_avg, stationary_sad_avg, confidence, template_y, sad_accept)
```

- [ ] **Step 5: `extract_template`、`search_best_offset`、`estimate_confidence`、`sparse_sad_at_offset` 也参数化 `strip_h`**

这四个函数内部都用 `STRIP_H` 常量。改为接受 `strip_h: u32` 参数，调用时传入。逐个修改：

`extract_template`：
```rust
fn extract_template(ref_buf: &GrayBuf, template_y: u32, sample_cols: &[usize], strip_h: u32) -> Vec<u8> {
    let mut tpl = Vec::with_capacity(strip_h as usize * sample_cols.len());
    for dy in 0..strip_h {
```

`search_best_offset`：签名加 `strip_h: u32`，函数体 `let strip_h = STRIP_H as usize;` 改为 `let strip_h = strip_h as usize;`

`estimate_confidence`：签名加 `strip_h: u32`，函数体 `let strip_h = STRIP_H as usize;` 改为 `let strip_h = strip_h as usize;`

`sparse_sad_at_offset`：签名加 `strip_h: u32`，函数体 `let strip_h = STRIP_H as usize;` 改为 `let strip_h = strip_h as usize;`

所有调用点在 `find_overlap_spatial_ext` 内部，传入 `strip_h`。

- [ ] **Step 6: 临时修改 `process_frame` 和 `finalize` 的调用点以适配新签名**

`process_frame` 中的调用改为（临时直接传常量，Task 4 重构为降级链）：

旧调用：
```rust
        let (dy, confidence) = match find_overlap_spatial_ext(
            &self.reference,
            &curr_buf,
            x_start,
            x_end,
            eff_top,
            eff_bottom,
            max_scroll,
            self.last_dy,
        ) {
```

新调用：
```rust
        let (dy, confidence) = match find_overlap_spatial_ext(
            &self.reference,
            &curr_buf,
            x_start,
            x_end,
            eff_top,
            eff_bottom,
            max_scroll,
            self.last_dy,
            SAD_ACCEPT,  // 临时用硬编码，Task 4 改为动态阈值
            STRIP_H,     // 默认模板高度
        ) {
```

`finalize` 中的调用同样加 `SAD_ACCEPT, STRIP_H` 两个参数。

- [ ] **Step 7: 编译验证**

Run:
```bash
cd /Users/wudarui/workspace/agent/octopus/.worktrees/optimize-capx && cargo check -p octopus-capx 2>&1 | tail -5
```
Expected: `Finished`。

- [ ] **Step 8: 运行测试验证无回归**

Run:
```bash
cd /Users/wudarui/workspace/agent/octopus/.worktrees/optimize-capx && cargo test -p octopus-capx 2>&1 | tail -8
```
Expected: 12 passed。

**如果测试失败**：可能是参数化 `strip_h` 时漏改了某处 `STRIP_H` 引用。用 `grep -n "STRIP_H" crates/capx/src/stitch.rs` 确认所有引用都已参数化（常量定义本身除外）。

- [ ] **Step 9: Commit**

```bash
cd /Users/wudarui/workspace/agent/octopus/.worktrees/optimize-capx
git add crates/capx/src/stitch.rs
git commit -m "feat(capx): 时序平滑静止判断 + find_overlap_spatial_ext 参数化（strip_h/sad_accept）"
```

---

## Task 4: process_frame 重构——动态阈值 + 静止双重校验 + dy_history 更新

**Files:**
- Modify: `crates/capx/src/stitch.rs`（`process_frame` 主匹配分支）

- [ ] **Step 1: 修改 `process_frame` 主匹配分支——引入动态阈值、静止双重校验、dy_history 更新**

在 `process_frame` 中，找到主匹配调用（Task 3 Step 6 的临时版本），替换为：

旧（Task 3 临时版本）：
```rust
        let curr_buf = GrayBuf::from_rgba(frame);

        let x_start = (w as f64 * X_START_RATIO) as u32;
        let x_end = (w as f64 * X_END_RATIO) as u32;

        let max_scroll = MAX_SCROLL;
        let (dy, confidence) = match find_overlap_spatial_ext(
            &self.reference,
            &curr_buf,
            x_start,
            x_end,
            eff_top,
            eff_bottom,
            max_scroll,
            self.last_dy,
            SAD_ACCEPT,  // 临时用硬编码，Task 4 改为动态阈值
            STRIP_H,     // 默认模板高度
        ) {
            Some(v) => v,
            None => {
                log::info!("[stitch] find_overlap_spatial returned None");
                self.last_dy = None;
                return Ok(false);
            }
        };
```

新：
```rust
        let curr_buf = GrayBuf::from_rgba(frame);

        let x_start = (w as f64 * X_START_RATIO) as u32;
        let x_end = (w as f64 * X_END_RATIO) as u32;

        let max_scroll = MAX_SCROLL;
        let sample_cols: Vec<usize> = (x_start as usize..x_end as usize)
            .step_by(SAMPLE_STEP_X)
            .collect();

        // 动态阈值：根据当前帧纹理密度 + 历史基线计算
        let template_y = eff_bottom.saturating_sub(STRIP_H);
        let texture = estimate_texture_density(&curr_buf, &sample_cols, template_y);
        let sad_accept = self.dynamic_sad_accept(texture);

        let (dy, confidence) = match find_overlap_spatial_ext(
            &self.reference,
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
                // 降级链在 Task 5 实现
                log::info!("[stitch] main match failed, entering fallback (Task 5)");
                self.last_dy = None;
                return Ok(false);
            }
        };

        // 静止双重校验：dy ≈ 0 且时序也确认静止才跳过
        if dy.abs() < 0.5 && self.is_stationary() {
            log::info!("[stitch] stationary confirmed by temporal smoothing");
            return Ok(false);
        }
```

紧接在现有的 `dy >= 0.0` 检查和 `new_rows` 限制检查之后，追加内容追加成功后的 `dy_history` 和 `sad_baseline` 更新。找到现有的画布追加代码段：

旧（画布追加之后、`Ok(true)` 之前）：
```rust
        // 更新参考灰度与速度缓存
        self.reference = curr_buf;
        self.last_dy = Some(dy);

        Ok(true)
```

新：
```rust
        // 更新参考灰度与速度缓存
        self.reference = curr_buf;
        self.last_dy = Some(dy);

        // 更新 dy_history（时序平滑）和 sad_baseline（动态阈值 EMA）
        self.dy_history.push_back(dy);
        if self.dy_history.len() > DY_HISTORY_LEN {
            self.dy_history.pop_front();
        }
        if self.sad_baseline == 0.0 {
            self.sad_baseline = best_sad;  // 首次直接赋值
        } else {
            self.sad_baseline = SAD_BASELINE_ALPHA * best_sad + (1.0 - SAD_BASELINE_ALPHA) * self.sad_baseline;
        }

        Ok(true)
```

> **注意**：`best_sad` 变量名需要从 `find_overlap_spatial_ext` 的返回值获取。当前 `find_overlap_spatial_ext` 返回 `Option<(f64, f64)>` = `(dy, confidence)`，不含 `best_sad`。需要改为返回 `(dy, confidence, best_sad)` 三元组。

- [ ] **Step 2: 修改 `find_overlap_spatial_ext` 返回值包含 `best_sad_avg`**

旧返回类型：`Option<(f64, f64)>` = `(dy, confidence)`

新返回类型：`Option<(f64, f64, f64)>` = `(dy, confidence, best_sad_avg)`

`decide_match` 返回值改为 `Option<(f64, f64, f64)>`：

```rust
fn decide_match(
    best_y_offset: u32,
    best_sad_avg: f64,
    stationary_sad_avg: f64,
    confidence: f64,
    template_y: u32,
    sad_accept: f64,
) -> Option<(f64, f64, f64)> {
    if stationary_sad_avg < STATIONARY_SAD {
        return Some((0.0, 1.0, 0.0));  // 静止时 best_sad=0
    }
    if best_sad_avg < sad_accept && confidence > MIN_CONFIDENCE {
        let dy = best_y_offset as f64 - template_y as f64;
        Some((dy, confidence, best_sad_avg))
    } else {
        None
    }
}
```

`process_frame` 中的 match 改为解构三元组：
```rust
        let (dy, confidence, best_sad) = match find_overlap_spatial_ext(...) { ... };
```

`finalize` 中的调用也同步解构（只取 dy 和 confidence，忽略 best_sad）：
```rust
        if let Some((dy, confidence, _)) = find_overlap_spatial_ext(...) {
```

- [ ] **Step 3: 编译验证**

Run:
```bash
cd /Users/wudarui/workspace/agent/octopus/.worktrees/optimize-capx && cargo check -p octopus-capx 2>&1 | tail -5
```
Expected: `Finished`。

- [ ] **Step 4: 运行测试验证无回归**

Run:
```bash
cd /Users/wudarui/workspace/agent/octopus/.worktrees/optimize-capx && cargo test -p octopus-capx 2>&1 | tail -8
```
Expected: 12 passed。

**如果 `test_stationary_frame_returns_false` 失败**：因为静止判断现在需要 `dy_history` 攒够 3 帧。init 阶段（第一帧）`process_frame` 返回 false 不更新 `dy_history`，第二帧（真正静止帧）`dy_history` 仍空 → `is_stationary()` 返回 false → dy=0 但不判静止 → 但 `dy >= 0.0` 检查会 return false（dy=0 不追加）。确认：测试中 `dy.abs() < 0.5 && self.is_stationary()` 在 `dy_history` 为空时，`is_stationary()` 返回 false，所以不会进入静止分支；但 `dy >= 0.0` 会在后面 return false——这是正确行为，测试应通过。

- [ ] **Step 5: Commit**

```bash
cd /Users/wudarui/workspace/agent/octopus/.worktrees/optimize-capx
git add crates/capx/src/stitch.rs
git commit -m "feat(capx): process_frame 动态阈值 + 静止双重校验 + dy_history/sad_baseline 更新"
```

---

## Task 5: 三级兜底降级链

**Files:**
- Modify: `crates/capx/src/stitch.rs`（`process_frame` 降级分支 + `try_match` / `try_match_strip` / `try_match_1d_projection`）

- [ ] **Step 1: 在 `impl Stitcher` 块内（`is_stationary` 之后）新增 `try_match` 封装方法**

```rust
    /// 主匹配封装：调用 find_overlap_spatial_ext。
    fn try_match(
        &self,
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
            &self.reference,
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
```

- [ ] **Step 2: 新增 `try_match_1d_projection` 方法**

在 `try_match` 之后追加：

```rust
    /// 降级 3：1D 灰度投影匹配。
    /// 将每行像素按抽样列取均值降为一维信号，对一维信号做 SAD 搜索。
    /// 对纯色/低纹理场景（2D SAD 缺乏特征）更鲁棒。
    fn try_match_1d_projection(
        &self,
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
        let template_y = eff_bottom - strip_h;

        // 构建抽样列索引
        let cols: Vec<usize> = (x_start as usize..x_end as usize)
            .step_by(SAMPLE_STEP_X)
            .collect();
        if cols.is_empty() {
            return None;
        }

        // 计算行均值信号
        let ref_proj = row_projection_means(&self.reference, &cols, template_y, template_y + strip_h);
        let search_start = (template_y as i32 - max_scroll as i32).max(eff_top as i32) as u32;

        // 一维 SAD 搜索
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
        let stationary_sad = {
            let curr_proj = row_projection_means(curr, &cols, template_y, template_y + strip_h);
            let mut sad = 0.0f64;
            for i in 0..strip_h as usize {
                sad += (ref_proj[i] - curr_proj[i]).abs();
            }
            sad / total
        };
        if stationary_sad < STATIONARY_SAD {
            return Some((0.0, 1.0, 0.0));
        }

        // 置信度（简化版：1D 最佳与均值比）
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
```

- [ ] **Step 3: 在自由函数区（`estimate_texture_density` 附近）新增 `row_projection_means` helper**

```rust
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
```

- [ ] **Step 4: 修改 `process_frame` 的 `None` 分支——替换为三级降级链**

旧（Task 4 版本的 None 分支）：
```rust
            None => {
                // 降级链在 Task 5 实现
                log::info!("[stitch] main match failed, entering fallback (Task 5)");
                self.last_dy = None;
                return Ok(false);
            }
```

新：
```rust
            None => {
                // 进入三级降级链
                log::info!("[stitch] main match failed, entering fallback chain");

                // 降级 1：扩大搜索范围 ×2（快速滚动可能超出 MAX_SCROLL）
                if let Some((dy, conf, sad)) = self.try_match(
                    &curr_buf, x_start, x_end, eff_top, eff_bottom, max_scroll * 2, sad_accept, STRIP_H,
                ) {
                    log::info!("[stitch] fallback 1: expanded search range, dy={:.1} conf={:.4}", dy, conf);
                    return self.apply_fallback_match(dy, conf, sad, frame, &curr_buf, w, eff_top, eff_bottom);
                }

                // 降级 2：缩小模板到 FALLBACK_STRIP_H + 放宽阈值
                if let Some((dy, conf, sad)) = self.try_match(
                    &curr_buf, x_start, x_end, eff_top, eff_bottom, max_scroll, sad_accept * FALLBACK_SAD_MULTIPLIER, FALLBACK_STRIP_H,
                ) {
                    log::info!("[stitch] fallback 2: reduced strip height, dy={:.1} conf={:.4}", dy, conf);
                    return self.apply_fallback_match(dy, conf, sad, frame, &curr_buf, w, eff_top, eff_bottom);
                }

                // 降级 3：1D 灰度投影匹配
                if let Some((dy, conf, sad)) = self.try_match_1d_projection(
                    &curr_buf, x_start, x_end, eff_top, eff_bottom, max_scroll, sad_accept,
                ) {
                    log::info!("[stitch] fallback 3: 1D projection match, dy={:.1} conf={:.4}", dy, conf);
                    return self.apply_fallback_match(dy, conf, sad, frame, &curr_buf, w, eff_top, eff_bottom);
                }

                // 全部失败：不停止，等下一帧
                log::info!("[stitch] all fallbacks exhausted, skipping frame");
                self.last_dy = None;
                return Ok(false);
            }
```

- [ ] **Step 5: 新增 `apply_fallback_match` 方法——复用主匹配的 dy 检查 + 追加逻辑**

在 `try_match_1d_projection` 之后追加：

```rust
    /// 降级匹配结果的处理（复用主匹配的 dy 检查 + 画布追加 + 状态更新）。
    fn apply_fallback_match(
        &mut self,
        dy: f64,
        _confidence: f64,
        best_sad: f64,
        frame: &RgbaImage,
        curr_buf: &GrayBuf,
        w: u32,
        eff_top: u32,
        eff_bottom: u32,
    ) -> Result<bool> {
        // 与主匹配相同的 dy 方向 + 幅度检查
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

        // 画布追加
        let crop_y = eff_bottom - new_rows;
        let row_bytes = w as usize * 4;
        let start = crop_y as usize * row_bytes;
        let end = start + new_rows as usize * row_bytes;
        let frame_raw = frame.as_raw();
        self.canvas_buf.extend_from_slice(&frame_raw[start..end]);
        self.canvas_h += new_rows;
        self.invalidate_cache();

        // 更新状态
        self.reference = curr_buf.clone_buf();
        self.last_dy = Some(dy);
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
```

> **注意**：`apply_fallback_match` 中 `self.reference = curr_buf.clone_buf()` 需要 `GrayBuf` 支持 clone。因为 `curr_buf` 在主匹配中已被 `self.reference = curr_buf` 消费，但降级链中 `curr_buf` 是借用的。需要在 `GrayBuf` 上加 `clone_buf` 方法或 derive Clone。见 Step 6。

- [ ] **Step 6: 为 `GrayBuf` 添加 `clone_buf` 方法（或 derive Clone）**

在 `GrayBuf` struct 定义上方加 `#[derive(Clone)]`：
```rust
#[derive(Clone)]
struct GrayBuf {
    data: Vec<u8>,
    width: usize,
}
```

然后 `apply_fallback_match` 中的 `self.reference = curr_buf.clone_buf()` 改为 `self.reference = curr_buf.clone()`。

同时主匹配的 `process_frame` 中 `self.reference = curr_buf` 也需调整为 `self.reference = curr_buf.clone()`，因为降级链也要用 `curr_buf`。但主匹配成功时降级链不执行，`curr_buf` 只被赋值一次。**检查所有权**：

实际上在主匹配 `Some` 分支中，`curr_buf` 被 `self.reference = curr_buf` move 了。降级链在 `None` 分支中，`curr_buf` 仍可用（主匹配的 `find_overlap_spatial_ext` 只借用 `&curr_buf`）。所以：
- 主匹配 `Some`：`self.reference = curr_buf`（move，OK）
- 降级链：`curr_buf` 仍可用，`apply_fallback_match` 内 `self.reference = curr_buf.clone()`（clone，OK）

- [ ] **Step 7: 修改 `finalize` 的调用点——适配新签名（sad_accept + strip_h + 三元组返回）**

`finalize` 中的调用：

旧：
```rust
        if let Some((dy, confidence)) = find_overlap_spatial_ext(
            &self.reference,
            &last_buf,
            x_start,
            x_end,
            eff_top,
            eff_bottom,
            max_finalize_scroll,
            None,
        ) {
```

新：
```rust
        if let Some((dy, confidence, _)) = find_overlap_spatial_ext(
            &self.reference,
            &last_buf,
            x_start,
            x_end,
            eff_top,
            eff_bottom,
            max_finalize_scroll,
            None,
            SAD_ACCEPT,
            STRIP_H,
        ) {
```

- [ ] **Step 8: 编译验证**

Run:
```bash
cd /Users/wudarui/workspace/agent/octopus/.worktrees/optimize-capx && cargo check -p octopus-capx 2>&1 | tail -5
```
Expected: `Finished`。

- [ ] **Step 9: 运行测试验证无回归**

Run:
```bash
cd /Users/wudarui/workspace/agent/octopus/.worktrees/optimize-capx && cargo test -p octopus-capx 2>&1 | tail -8
```
Expected: 12 passed。

- [ ] **Step 10: Commit**

```bash
cd /Users/wudarui/workspace/agent/octopus/.worktrees/optimize-capx
git add crates/capx/src/stitch.rs
git commit -m "feat(capx): 三级兜底降级链（扩大范围→缩小模板→1D 投影）"
```

---

## Task 6: 增强测试用例

**Files:**
- Modify: `crates/capx/src/stitch.rs`（测试模块 + `make_frame` 工具增强）

- [ ] **Step 1: 增强 `make_frame` 支持可控纹理密度**

在 `make_frame` 函数之前新增 `make_frame_textured`：

```rust
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
```

- [ ] **Step 2: 新增时序平滑测试**

在测试模块末尾（最后一个测试之后、`}` 之前）追加：

```rust
    #[test]
    fn test_is_stationary_with_history() {
        let f0 = make_frame(TW, TH, 0);
        let mut s = Stitcher::new(f0, StitchConfig::default());
        let f1 = make_frame(TW, TH, 0);
        s.process_frame(&f1).unwrap(); // init

        // 无 dy_history → 不静止
        assert!(!s.is_stationary(), "空 history 不应判静止");

        // 手动注入 dy_history 模拟持续滚动
        s.dy_history.extend([−15.0, −12.0, −10.0, −3.0]);
        assert!(!s.is_stationary(), "回弹帧 history 均值 -10 不应判静止");

        // 手动注入接近静止的 history
        s.dy_history.clear();
        s.dy_history.extend([−1.0, 0.0, −0.5, 1.0, 0.0]);
        assert!(s.is_stationary(), "均值接近 0 应判静止");
    }
```

> **注意**：测试中直接操作 `s.dy_history` 需要 `dy_history` 在测试模块可访问。由于测试模块是 `mod tests` 在同一文件内，可以访问私有字段。

- [ ] **Step 3: 新增动态阈值测试**

```rust
    #[test]
    fn test_dynamic_sad_accept_scales_with_texture() {
        let f0 = make_frame(TW, TH, 0);
        let mut s = Stitcher::new(f0, StitchConfig::default());

        // sad_baseline = 0 时，baseline_cap = 5.0
        // 低纹理：texture=0.05 → bonus=1.5 → (7.5+1.5).min(5.0).max(7.5) = 7.5
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
```

- [ ] **Step 4: 新增降级链测试**

```rust
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
        let f0 = make_frame_textured(TW, TH, 0, 0); // 纯色
        let mut s = Stitcher::new(f0, StitchConfig::default());
        let f1 = make_frame_textured(TW, TH, 0, 0);
        s.process_frame(&f1).unwrap(); // init
        let f2 = make_frame_textured(TW, TH, 30, 0); // 滚动 30px
        // 2D SAD 在纯色页可能失败，降级 3 的 1D 投影应能匹配
        let added = s.process_frame(&f2).unwrap();
        // 注意：纯色背景可能 2D SAD 也能匹配（渐变特征），这个测试验证至少不报错
        let _ = added; // 不强制 assert，验证不 panic
    }
```

- [ ] **Step 5: 编译 + 运行全部测试**

Run:
```bash
cd /Users/wudarui/workspace/agent/octopus/.worktrees/optimize-capx && cargo test -p octopus-capx 2>&1 | tail -15
```
Expected: 现有 12 + 新增 ≥4 = ≥16 passed。

**如果 `test_fallback_expanded_search_range` 失败**：合成图在 300px 偏移下可能因渐变周期性（256）导致匹配混乱。尝试减小偏移到 250px 或增大 `make_frame` 的特征密度。

**如果 `test_is_stationary_with_history` 编译失败**：检查 `dy_history` 字段名拼写，以及 `extend` 方法接受 `VecDeque` 还是数组。可能需要 `s.dy_history.extend([−15.0, −12.0, −10.0, −3.0].into_iter())` 或 `s.dy_history.extend(vec![−15.0, −12.0, −10.0, −3.0])`。

- [ ] **Step 6: Commit**

```bash
cd /Users/wudarui/workspace/agent/octopus/.worktrees/optimize-capx
git add crates/capx/src/stitch.rs
git commit -m "test(capx): 新增时序平滑/动态阈值/降级链单元测试"
```

---

## Task 7: 文档同步

**Files:**
- Modify: `docs/superpowers/specs/2026-07-02-capx-stitch-robustness-design.md`（标注实施记录）
- Modify: `docs/architecture.md`（stitch 模块描述更新）

- [ ] **Step 1: 更新 spec 状态为实施完成**

在 spec 文件头部的状态字段后追加实施记录：
```
**状态**: ✅ 实施完成（3 改造 + 测试全部落地）
```

并在文件末尾追加实施记录段落（标注偏差，若有）。

- [ ] **Step 2: 更新 architecture.md 的 stitch 描述**

找到 architecture.md 中 stitch 模块描述行（包含"2D SAD 空间模板匹配"），追加健壮性优化的关键点：
- 时序平滑静止判断（替代单帧静态校验硬覆盖）
- 动态自适应 SAD 阈值（纹理密度 + EMA 基线）
- 三级兜底降级链（扩大范围→缩小模板→1D 投影）

- [ ] **Step 3: Commit**

```bash
cd /Users/wudarui/workspace/agent/octopus/.worktrees/optimize-capx
git add docs/
git commit -m "docs(capx): 同步拼接健壮性优化实施记录与 architecture 更新"
```

---

## 验收清单（全部任务完成后核对）

- [ ] `cargo test -p octopus-capx` 全绿（≥16 个测试）
- [ ] `cargo check -p octopus-capx -p octopus-desktop` 无错误
- [ ] API 零改动：`git diff main -- crates/capx/src/lib.rs` 为空，公开签名不变
- [ ] 源码无新增裸魔法数字
- [ ] 降级链有日志输出（`[stitch] fallback N: ...`）
- [ ] 文档同步完成
