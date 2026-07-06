# 重构优化实施计划 — 死代码/重复/超长代码清理

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 清理代码审查发现的 4 类问题（paddle-ocr 工具函数集中、clamp/clip 命名统一、start_scroll_recording 拆分、Coordinator 长函数拆分），行为零变化。

**Architecture:** 分层提取——纯函数严格 TDD（先写测试锁住边界值），平台/异步编排靠编译器+全量测试验证行为保持。4 个 task 按依赖顺序执行：Task 1（paddle-ocr 纯函数）→ Task 2（start_scroll_recording 纯逻辑提取）→ Task 3（Coordinator::new 拆分）→ Task 4（begin_recording 拆分）。

**Tech Stack:** Rust, paddle-ocr (vision 子模块), desktop (Tauri 2), tokio, core-graphics (macOS)

## Global Constraints

- **INV-1 行为保持**：`cargo test --workspace --exclude octopus-desktop` 全绿是每个 commit 的必要条件
- **INV-2 无 API 变化**：`#[tauri::command]` 签名、`pub fn` 接口不得变化
- **INV-3 平台守卫保留**：所有 `#[cfg(target_os = "macos")]` / `#[cfg(not(...))]` 分支等价保留
- **INV-4 无新依赖**：不引入新 crate
- **INV-5 单 commit 单任务**：每个子任务一个 commit
- **TDD 红线**：纯函数必须先写失败测试、看到失败、再写实现

---

## Task 1: paddle-ocr `vision/numeric.rs` 工具函数集中

**Files:**
- Create: `crates/paddle-ocr/src/vision/numeric.rs`
- Modify: `crates/paddle-ocr/src/vision/mod.rs`
- Modify: `crates/paddle-ocr/src/vision/resize.rs:46-72`（删除 3 个本地函数）
- Modify: `crates/paddle-ocr/src/vision/rotate_crop.rs:172-208,301-305`（删除 6 个本地函数）
- Modify: `crates/paddle-ocr/src/rec/word_boxes.rs:391-395`（删除本地 `l2`）
- Modify: `crates/paddle-ocr/src/det/postprocess/mod.rs:2014-2018`（删除本地 `l2`）

**Interfaces:**
- Produces: `crate::vision::numeric` 模块，含 8 个 `pub(crate)` 函数

**关键事实（实现差异说明）：**
- `l2` 3 处完全相同：`(a: [f32;2], b: [f32;2]) -> f32`
- `saturate_cast_i16_from_f32` 2 处实现略有不同但行为等价（Rust `f32 as i32` 自 1.45 起为饱和转换）：
  - resize.rs:60 走 `cv_round_ties_even_f32`（含 `is_finite` 检查）
  - rotate_crop.rs:204 直接 `round_ties_even() as i32`
  - **统一为 resize.rs 版本**（含显式 `is_finite` 检查，更安全）

---

- [ ] **Step 1: 创建 `numeric.rs` 并写失败测试**

创建 `crates/paddle-ocr/src/vision/numeric.rs`，只写测试 + 函数签名（用 `todo!()` 占位）：

```rust
//! 集中 paddle-ocr vision 子模块共用的数值转换与几何工具函数。

/// L2 欧氏距离（2D 点对）。
pub(crate) fn l2(a: [f32; 2], b: [f32; 2]) -> f32 {
    todo!()
}

/// f32 → i32 银行家舍入（NaN/Inf → 0，溢出饱和）。
pub(crate) fn cv_round_ties_even_f32(v: f32) -> i32 {
    todo!()
}

/// f64 → i32 银行家舍入（NaN/Inf → 0，溢出饱和）。
pub(crate) fn saturate_cast_i32_round(v: f64) -> i32 {
    todo!()
}

/// f32 → i16（先银行家舍入到 i32，再饱和到 i16 范围）。
pub(crate) fn saturate_cast_i16_from_f32(v: f32) -> i16 {
    todo!()
}

/// i32 → i16 饱和转换。
pub(crate) fn saturate_cast_i16(v: i32) -> i16 {
    todo!()
}

/// 三次样条插值系数（A=-0.75 的 bicubic kernel）。
pub(crate) fn interpolate_cubic_coeffs(x: f32) -> [f32; 4] {
    todo!()
}

/// 区间裁剪——上界为 exclusive（返回值 ∈ [lo, hi_exclusive-1]）。
pub(crate) fn clip_i32_exclusive_upper(x: i32, lo: i32, hi_exclusive: i32) -> i32 {
    todo!()
}

/// 区间裁剪——上界为 inclusive（返回值 ∈ [min_v, max_v]）。
pub(crate) fn clamp_i32_inclusive(v: i32, min_v: i32, max_v: i32) -> i32 {
    todo!()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn l2_basic_and_zero() {
        assert_eq!(l2([0.0, 0.0], [3.0, 4.0]), 5.0);
        assert_eq!(l2([1.0, 1.0], [1.0, 1.0]), 0.0);
    }

    #[test]
    fn cv_round_ties_even_f32_normal() {
        assert_eq!(cv_round_ties_even_f32(2.5), 2);  // ties → even
        assert_eq!(cv_round_ties_even_f32(3.5), 4);  // ties → even
        assert_eq!(cv_round_ties_even_f32(2.4), 2);
        assert_eq!(cv_round_ties_even_f32(2.6), 3);
    }

    #[test]
    fn cv_round_ties_even_f32_nan_inf() {
        assert_eq!(cv_round_ties_even_f32(f32::NAN), 0);
        assert_eq!(cv_round_ties_even_f32(f32::INFINITY), 0);
        assert_eq!(cv_round_ties_even_f32(f32::NEG_INFINITY), 0);
    }

    #[test]
    fn saturate_cast_i32_round_normal() {
        assert_eq!(saturate_cast_i32_round(2.5_f64), 2);
        assert_eq!(saturate_cast_i32_round(-2.5_f64), -2);
    }

    #[test]
    fn saturate_cast_i16_from_f32_normal() {
        assert_eq!(saturate_cast_i16_from_f32(0.0), 0);
        assert_eq!(saturate_cast_i16_from_f32(100.7), 101);
        assert_eq!(saturate_cast_i16_from_f32(-100.7), -101);
    }

    #[test]
    fn saturate_cast_i16_from_f32_saturation() {
        assert_eq!(saturate_cast_i16_from_f32(99999.0), i16::MAX);
        assert_eq!(saturate_cast_i16_from_f32(-99999.0), i16::MIN);
        assert_eq!(saturate_cast_i16_from_f32(f32::NAN), 0);
        assert_eq!(saturate_cast_i16_from_f32(f32::INFINITY), i16::MAX);
    }

    #[test]
    fn saturate_cast_i16_from_i32() {
        assert_eq!(saturate_cast_i16(0), 0);
        assert_eq!(saturate_cast_i16(32767), 32767);
        assert_eq!(saturate_cast_i16(32768), 32767);
        assert_eq!(saturate_cast_i16(-32768), -32768);
        assert_eq!(saturate_cast_i16(-32769), -32768);
    }

    #[test]
    fn interpolate_cubic_coeffs_sum_to_one() {
        // 在 x ∈ [0,1) 范围内，系数之和应接近 1.0
        for i in 0..10 {
            let x = i as f32 / 10.0;
            let c = interpolate_cubic_coeffs(x);
            let sum: f32 = c.iter().sum();
            assert!((sum - 1.0).abs() < 1e-5, "x={}, sum={}", x, sum);
        }
    }

    #[test]
    fn clip_exclusive_upper() {
        assert_eq!(clip_i32_exclusive_upper(5, 0, 10), 5);
        assert_eq!(clip_i32_exclusive_upper(-1, 0, 10), 0);
        assert_eq!(clip_i32_exclusive_upper(10, 0, 10), 9);  // 10 >= 10 → 9
        assert_eq!(clip_i32_exclusive_upper(15, 0, 10), 9);
    }

    #[test]
    fn clamp_inclusive() {
        assert_eq!(clamp_i32_inclusive(5, 0, 10), 5);
        assert_eq!(clamp_i32_inclusive(-1, 0, 10), 0);
        assert_eq!(clamp_i32_inclusive(10, 0, 10), 10);  // 10 是合法值
        assert_eq!(clamp_i32_inclusive(15, 0, 10), 10);
    }
}
```

在 `crates/paddle-ocr/src/vision/mod.rs` 末尾添加：
```rust
pub(crate) mod numeric;
```

- [ ] **Step 2: 运行测试验证失败**

Run: `cargo test -p octopus-paddle-ocr vision::numeric 2>&1 | tail -20`
Expected: 多个测试 FAIL（`not yet implemented` panic）

- [ ] **Step 3: 实现 numeric.rs 全部函数**

替换 `numeric.rs` 中所有 `todo!()` 为真实实现（从原文件逐字复制）：

```rust
//! 集中 paddle-ocr vision 子模块共用的数值转换与几何工具函数。

/// L2 欧氏距离（2D 点对）。
pub(crate) fn l2(a: [f32; 2], b: [f32; 2]) -> f32 {
    let dx = a[0] - b[0];
    let dy = a[1] - b[1];
    (dx * dx + dy * dy).sqrt()
}

/// f32 → i32 银行家舍入（NaN/Inf → 0，溢出饱和）。
pub(crate) fn cv_round_ties_even_f32(v: f32) -> i32 {
    if !v.is_finite() {
        return 0;
    }
    let r = v.round_ties_even();
    if r < i32::MIN as f32 {
        i32::MIN
    } else if r > i32::MAX as f32 {
        i32::MAX
    } else {
        r as i32
    }
}

/// f64 → i32 银行家舍入（NaN/Inf → 0，溢出饱和）。
pub(crate) fn saturate_cast_i32_round(v: f64) -> i32 {
    if !v.is_finite() {
        return 0;
    }
    let r = v.round_ties_even();
    if r < i32::MIN as f64 {
        i32::MIN
    } else if r > i32::MAX as f64 {
        i32::MAX
    } else {
        r as i32
    }
}

/// f32 → i16（先银行家舍入到 i32，再饱和到 i16 范围）。
pub(crate) fn saturate_cast_i16_from_f32(v: f32) -> i16 {
    cv_round_ties_even_f32(v).clamp(i16::MIN as i32, i16::MAX as i32) as i16
}

/// i32 → i16 饱和转换。
pub(crate) fn saturate_cast_i16(v: i32) -> i16 {
    v.clamp(i16::MIN as i32, i16::MAX as i32) as i16
}

/// 三次样条插值系数（A=-0.75 的 bicubic kernel）。
pub(crate) fn interpolate_cubic_coeffs(x: f32) -> [f32; 4] {
    const A: f32 = -0.75;
    let c0 = ((A * (x + 1.0) - 5.0 * A) * (x + 1.0) + 8.0 * A) * (x + 1.0) - 4.0 * A;
    let c1 = ((A + 2.0) * x - (A + 3.0)) * x * x + 1.0;
    let one_minus_x = 1.0 - x;
    let c2 = ((A + 2.0) * one_minus_x - (A + 3.0)) * one_minus_x * one_minus_x + 1.0;
    let c3 = 1.0 - c0 - c1 - c2;
    [c0, c1, c2, c3]
}

/// 区间裁剪——上界为 exclusive（返回值 ∈ [lo, hi_exclusive-1]）。
pub(crate) fn clip_i32_exclusive_upper(x: i32, lo: i32, hi_exclusive: i32) -> i32 {
    if x < lo {
        lo
    } else if x >= hi_exclusive {
        hi_exclusive - 1
    } else {
        x
    }
}

/// 区间裁剪——上界为 inclusive（返回值 ∈ [min_v, max_v]）。
pub(crate) fn clamp_i32_inclusive(v: i32, min_v: i32, max_v: i32) -> i32 {
    v.max(min_v).min(max_v)
}
```
（测试模块保持不变）

- [ ] **Step 4: 运行测试验证全绿**

Run: `cargo test -p octopus-paddle-ocr vision::numeric 2>&1 | tail -20`
Expected: 10 passed, 0 failed

- [ ] **Step 5: 迁移 resize.rs 调用点**

在 `crates/paddle-ocr/src/vision/resize.rs`：
1. 顶部添加 `use super::numeric::{clip_i32_exclusive_upper, cv_round_ties_even_f32, saturate_cast_i16_from_f32};`
2. 删除本地 `cv_round_ties_even_f32`（行 46-58）、`saturate_cast_i16_from_f32`（行 60-62）、`clip_i32`（行 64-72）
3. `resize_rows_into` 中（行 159-160）将 `clip_i32(...)` 改为 `clip_i32_exclusive_upper(...)`

```rust
// 行 159-160 改为：
        let sy0 = clip_i32_exclusive_upper(kernel.yofs[dy], 0, dims.src_h as i32) as usize;
        let sy1 = clip_i32_exclusive_upper(kernel.yofs[dy] + 1, 0, dims.src_h as i32) as usize;
```

- [ ] **Step 6: 编译验证 resize.rs**

Run: `cargo check -p octopus-paddle-ocr 2>&1 | tail -10`
Expected: 编译通过（可能有 unused import warning，后续清理）

- [ ] **Step 7: 迁移 rotate_crop.rs 调用点**

在 `crates/paddle-ocr/src/vision/rotate_crop.rs`：
1. 顶部添加 `use super::numeric::{clamp_i32_inclusive, interpolate_cubic_coeffs, saturate_cast_i16, saturate_cast_i16_from_f32, saturate_cast_i32_round, l2};`
2. 删除本地：`clamp_i32`（172-174）、`saturate_cast_i32_round`（176-188）、`saturate_cast_i16`（190-192）、`interpolate_cubic_coeffs`（194-202）、`saturate_cast_i16_from_f32`（204-208）、`l2`（301-305）
3. 替换调用：
   - 行 67-68：`saturate_cast_i32_round(...)` 不变（同名）
   - 行 70-71：`saturate_cast_i16(...)` 不变（同名）
   - 行 85,87：`clamp_i32(...)` → `clamp_i32_inclusive(...)`
   - 行 220：`saturate_cast_i16_from_f32(...)` 不变（同名）
   - （如有 `l2` 调用则不变）

- [ ] **Step 8: 迁移 word_boxes.rs 和 postprocess/mod.rs 的 l2**

在 `crates/paddle-ocr/src/rec/word_boxes.rs`：
1. 顶部添加 `use crate::vision::numeric::l2;`
2. 删除本地 `l2`（行 391-395）

在 `crates/paddle-ocr/src/det/postprocess/mod.rs`：
1. 找到合适的 import 位置，添加 `use crate::vision::numeric::l2;`
2. 删除本地 `l2`（行 2014-2018）

- [ ] **Step 9: 全量编译 + 测试**

Run:
```bash
cargo clippy -p octopus-paddle-ocr --all-targets 2>&1 | tail -10
cargo test -p octopus-paddle-ocr 2>&1 | tail -15
```
Expected: 0 warning（之前的 3 个 clippy nit 顺手修），32+10 passed, 0 failed

- [ ] **Step 10: 顺手修 clippy 3 个 nit（detector.rs:115, postprocess/mod.rs:1924, word_boxes.rs:76）**

```rust
// crates/paddle-ocr/src/det/detector.rs:115
// &model_path → model_path
OrtSession::new_with_contract(model_path, &config.runtime, SessionContract::Det)?;

// crates/paddle-ocr/src/det/postprocess/mod.rs:1924
// scores.into_iter() → scores
for (box_, score) in dt_boxes.into_iter().zip(scores) {

// crates/paddle-ocr/src/rec/word_boxes.rs:76
// mapped.into_iter() → mapped
.zip(mapped)
```

- [ ] **Step 11: 最终验证 + commit**

Run:
```bash
cargo clippy -p octopus-paddle-ocr --all-targets 2>&1 | grep "^warning:" | wc -l
cargo test -p octopus-paddle-ocr 2>&1 | tail -5
cargo test --workspace --exclude octopus-desktop 2>&1 | tail -5
```
Expected: 0 warnings, all tests pass

Commit:
```bash
git add crates/paddle-ocr/src/vision/numeric.rs crates/paddle-ocr/src/vision/mod.rs crates/paddle-ocr/src/vision/resize.rs crates/paddle-ocr/src/vision/rotate_crop.rs crates/paddle-ocr/src/rec/word_boxes.rs crates/paddle-ocr/src/det/postprocess/mod.rs crates/paddle-ocr/src/det/detector.rs
git commit -m "$(cat <<'EOF'
refactor(paddle-ocr): 集中 vision 工具函数到 numeric.rs + 清除重复

将 l2(3处)、saturate_cast_i16_from_f32(2处)、cv_round_ties_even_f32、
saturate_cast_i32_round、saturate_cast_i16、interpolate_cubic_coeffs、
clip_i32/clamp_i32 统一到 vision/numeric.rs。

同步修复 clamp/clip 命名混淆：clip_i32→clip_i32_exclusive_upper，
clamp_i32→clamp_i32_inclusive，让 inclusive/exclusive 语义在名字上可见。

新增 10 个参数化测试覆盖 NaN/Inf/饱和边界，顺手修 3 个 clippy nit。


💘 Generated with Crush
EOF
)"
```

---

## Task 2: `start_scroll_recording` 纯逻辑提取

**Files:**
- Create: `crates/desktop/src/screenshot_geometry.rs`
- Modify: `crates/desktop/src/main.rs`（注册 `mod screenshot_geometry;`）
- Modify: `crates/desktop/src/screenshot_commands.rs:834-1336`（调用提取出的辅助函数）

**Interfaces:**
- Produces: `screenshot_geometry` 模块，含纯函数 + `MonitorRect` struct

**关键约束：** desktop crate 在 worktree 无法编译（前端 dist 缺失），本 task 全程在主仓库验证。

---

- [ ] **Step 1: 在主仓库验证 desktop 基线可编译**

```bash
cd /Users/wudarui/workspace/agent/octopus
cargo check -p octopus-desktop 2>&1 | tail -5
```
Expected: 编译通过

- [ ] **Step 2: 创建 `screenshot_geometry.rs` 并写失败测试**

创建 `crates/desktop/src/screenshot_geometry.rs`：

```rust
//! start_scroll_recording 提取出的纯逻辑：坐标换算、显示器命中、preview 裁剪参数。
//! 所有函数不依赖 Tauri/Quartz 类型，输入输出均为纯数据。

/// 显示器矩形（从 Tauri Monitor 提取的纯数据，用于命中检测）。
#[derive(Debug, Clone, Copy)]
pub(crate) struct MonitorRect {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
    pub scale: f64,
}

/// 选区的全局逻辑坐标。
#[derive(Debug, Clone, Copy)]
pub(crate) struct SelectionGlobal {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

/// 选区在目标显示器内的物理像素裁剪参数。
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct PhysicalCrop {
    pub px: u32,
    pub py: u32,
    pub pw: u32,
    pub ph: u32,
}

/// 选区全局坐标 = 窗口原点 + CSS 偏移。
pub(crate) fn compute_selection_global(
    win_origin_x: f64,
    win_origin_y: f64,
    x: f64,
    y: f64,
    w: f64,
    h: f64,
) -> SelectionGlobal {
    todo!()
}

/// 找到包含点 (cx, cy) 的显示器索引，无命中返回 None。
pub(crate) fn find_monitor_for_point(
    monitors: &[MonitorRect],
    cx: f64,
    cy: f64,
) -> Option<usize> {
    todo!()
}

/// 计算选区在显示器内的物理像素裁剪参数。
pub(crate) fn compute_physical_crop(
    sel: &SelectionGlobal,
    mon: &MonitorRect,
) -> PhysicalCrop {
    todo!()
}

/// 计算预览裁剪参数：从 canvas 底部取最近 N 行用于生成预览缩略图。
/// 返回 (crop_src_h, crop_y)。
pub(crate) fn compute_preview_crop(
    canvas_h: u32,
    canvas_w: u32,
    preview_w: u32,
    max_preview_h: u32,
) -> (u32, u32) {
    todo!()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selection_global_adds_origin() {
        let s = compute_selection_global(100.0, 200.0, 10.0, 20.0, 300.0, 400.0);
        assert_eq!(s.x, 110.0);
        assert_eq!(s.y, 220.0);
        assert_eq!(s.w, 300.0);
        assert_eq!(s.h, 400.0);
    }

    #[test]
    fn find_monitor_center_hit() {
        let monitors = vec![
            MonitorRect { x: 0.0, y: 0.0, w: 1920.0, h: 1080.0, scale: 1.0 },
            MonitorRect { x: 1920.0, y: 0.0, w: 2560.0, h: 1440.0, scale: 2.0 },
        ];
        // 点在第二个显示器内
        assert_eq!(find_monitor_for_point(&monitors, 2000.0, 500.0), Some(1));
        // 点在第一个显示器内
        assert_eq!(find_monitor_for_point(&monitors, 960.0, 540.0), Some(0));
    }

    #[test]
    fn find_monitor_no_hit_returns_none() {
        let monitors = vec![
            MonitorRect { x: 0.0, y: 0.0, w: 1920.0, h: 1080.0, scale: 1.0 },
        ];
        assert_eq!(find_monitor_for_point(&monitors, 3000.0, 500.0), None);
    }

    #[test]
    fn physical_crop_basic() {
        let sel = SelectionGlobal { x: 100.0, y: 50.0, w: 200.0, h: 300.0 };
        let mon = MonitorRect { x: 0.0, y: 0.0, w: 1920.0, h: 1080.0, scale: 2.0 };
        let crop = compute_physical_crop(&sel, &mon);
        assert_eq!(crop.px, 200);
        assert_eq!(crop.py, 100);
        assert_eq!(crop.pw, 400);
        assert_eq!(crop.ph, 600);
    }

    #[test]
    fn physical_crop_with_monitor_offset() {
        let sel = SelectionGlobal { x: 2000.0, y: 100.0, w: 100.0, h: 100.0 };
        let mon = MonitorRect { x: 1920.0, y: 0.0, w: 2560.0, h: 1440.0, scale: 2.0 };
        let crop = compute_physical_crop(&sel, &mon);
        assert_eq!(crop.px, 160);  // (2000-1920)*2
        assert_eq!(crop.py, 200);
        assert_eq!(crop.pw, 200);
        assert_eq!(crop.ph, 200);
    }

    #[test]
    fn preview_crop_small_canvas() {
        // canvas 小于 max_preview 对应的 src_h，全取
        let (src_h, y) = compute_preview_crop(500, 800, 400, 1200);
        // src_h = min(500*800/400, 500) = min(1000, 500) = 500
        // crop_src_h = min(500, 1200*800/400) = min(500, 2400) = 500
        assert_eq!(src_h, 500);
        assert_eq!(y, 0);
    }

    #[test]
    fn preview_crop_large_canvas() {
        // canvas 很大，只取底部 max_preview_h 对应的行
        let (src_h, y) = compute_preview_crop(5000, 800, 400, 1200);
        // src_h = min(5000*800/400, 5000) = min(10000, 5000) = 5000
        // crop_src_h = min(5000, 1200*800/400) = min(5000, 2400) = 2400
        assert_eq!(src_h, 2400);
        assert_eq!(y, 5000 - 2400);
    }
}
```

在 `crates/desktop/src/main.rs` 添加 `mod screenshot_geometry;`（放在现有 mod 列表中，字母序）。

- [ ] **Step 3: 运行测试验证失败**

Run: `cd /Users/wudarui/workspace/agent/octopus && cargo test -p octopus-desktop screenshot_geometry 2>&1 | tail -20`
Expected: 多个测试 FAIL（`not yet implemented`）

- [ ] **Step 4: 实现 screenshot_geometry.rs 全部函数**

```rust
//! start_scroll_recording 提取出的纯逻辑：坐标换算、显示器命中、preview 裁剪参数。
//! 所有函数不依赖 Tauri/Quartz 类型，输入输出均为纯数据。

#[derive(Debug, Clone, Copy)]
pub(crate) struct MonitorRect {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
    pub scale: f64,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct SelectionGlobal {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct PhysicalCrop {
    pub px: u32,
    pub py: u32,
    pub pw: u32,
    pub ph: u32,
}

pub(crate) fn compute_selection_global(
    win_origin_x: f64,
    win_origin_y: f64,
    x: f64,
    y: f64,
    w: f64,
    h: f64,
) -> SelectionGlobal {
    SelectionGlobal {
        x: win_origin_x + x,
        y: win_origin_y + y,
        w,
        h,
    }
}

pub(crate) fn find_monitor_for_point(
    monitors: &[MonitorRect],
    cx: f64,
    cy: f64,
) -> Option<usize> {
    monitors.iter().position(|m| {
        cx >= m.x && cx < m.x + m.w && cy >= m.y && cy < m.y + m.h
    })
}

pub(crate) fn compute_physical_crop(
    sel: &SelectionGlobal,
    mon: &MonitorRect,
) -> PhysicalCrop {
    PhysicalCrop {
        px: ((sel.x - mon.x) * mon.scale) as u32,
        py: ((sel.y - mon.y) * mon.scale) as u32,
        pw: (sel.w * mon.scale) as u32,
        ph: (sel.h * mon.scale) as u32,
    }
}

pub(crate) fn compute_preview_crop(
    canvas_h: u32,
    canvas_w: u32,
    preview_w: u32,
    max_preview_h: u32,
) -> (u32, u32) {
    let src_h = ((canvas_h as u64 * canvas_w as u64 / preview_w as u64)
        .min(canvas_h as u64)) as u32;
    let crop_src_h = src_h
        .min(max_preview_h * canvas_w / preview_w)
        .min(canvas_h);
    let crop_y = canvas_h - crop_src_h;
    (crop_src_h, crop_y)
}
```
（测试模块保持不变）

- [ ] **Step 5: 运行测试验证全绿**

Run: `cargo test -p octopus-desktop screenshot_geometry 2>&1 | tail -15`
Expected: 6 passed, 0 failed

- [ ] **Step 6: 重构 screenshot_commands.rs — 替换坐标换算逻辑**

在 `start_scroll_recording` 的 `tokio::spawn` 块内（行 845-1333），将行 876-905 的坐标换算逻辑替换为调用 `screenshot_geometry` 函数。**不改变平台守卫分支**（行 858-874 的 `#[cfg]` 块保留原样）。

替换行 876-905 为：
```rust
        use crate::screenshot_geometry::{
            compute_selection_global, find_monitor_for_point,
            compute_physical_crop, MonitorRect,
        };

        let sel = compute_selection_global(win_origin_x, win_origin_y, x, y, w, h);

        let monitors: Vec<MonitorRect> = ah
            .available_monitors()
            .unwrap_or_default()
            .iter()
            .map(|m| {
                let sf = m.scale_factor();
                MonitorRect {
                    x: m.position().x as f64 / sf,
                    y: m.position().y as f64 / sf,
                    w: m.size().width as f64 / sf,
                    h: m.size().height as f64 / sf,
                    scale: sf,
                }
            })
            .collect();

        let mon_idx = find_monitor_for_point(
            &monitors,
            sel.x + w / 2.0,
            sel.y + h / 2.0,
        ).unwrap_or(0);
        let mon = &monitors[mon_idx];
        let crop = compute_physical_crop(&sel, mon);
        let scale = mon.scale;
        let (px, py, pw, ph) = (crop.px, crop.py, crop.pw, crop.ph);
        let mon_logical_x = mon.x;
        let mon_logical_y = mon.y;
        let _mon_phys_x = (ah.available_monitors().unwrap_or_default()[mon_idx].position().x) as i32;
        let _mon_phys_y = (ah.available_monitors().unwrap_or_default()[mon_idx].position().y) as i32;
        let sel_global_x = sel.x;
        let sel_global_y = sel.y;
```

- [ ] **Step 7: 重构 — 提取 preview crop 重复逻辑**

将行 1144-1151 和行 1212-1215 的重复 preview crop 逻辑替换为 `compute_preview_crop`：

行 1144-1151 替换为：
```rust
            let preview_w = 400u32;
            let max_preview_h = 1200u32;
            let (crop_src_h, crop_y) = crate::screenshot_geometry::compute_preview_crop(
                stitcher.height(), stitcher.canvas_w(), preview_w, max_preview_h,
            );
            let canvas_buf_slice = stitcher.canvas_buf_slice(crop_y, crop_src_h);
```

行 1212-1215 替换为：
```rust
                let preview_w = 400u32;
                let max_preview_h = 1200u32;
                let (crop_src_h, crop_y) = crate::screenshot_geometry::compute_preview_crop(
                    canvas.height(), canvas.width(), preview_w, max_preview_h,
                );
                let canvas_cropped = image::imageops::crop_imm(&canvas, 0, crop_y, canvas.width(), crop_src_h).to_image();
```

- [ ] **Step 8: 编译验证 + 行为保持 review**

Run: `cargo check -p octopus-desktop 2>&1 | tail -10`
Expected: 编译通过

`git diff` review checklist：
- [ ] `#[cfg(target_os = "macos")]` 分支全部保留
- [ ] `#[tauri::command]` 签名不变
- [ ] 坐标换算数值等价（手动验证 sel_global_x/y, px/py/pw/ph 计算路径）
- [ ] preview crop 参数等价

- [ ] **Step 9: 全量测试 + commit**

Run:
```bash
cd /Users/wudarui/workspace/agent/octopus
cargo test -p octopus-desktop 2>&1 | tail -10
cargo test --workspace 2>&1 | tail -10
```
Expected: all pass

Commit:
```bash
git add crates/desktop/src/screenshot_geometry.rs crates/desktop/src/main.rs crates/desktop/src/screenshot_commands.rs
git commit -m "$(cat <<'EOF'
refactor(desktop): 提取 start_scroll_recording 纯逻辑到 screenshot_geometry

将坐标换算（窗口原点+CSS偏移→全局逻辑坐标）、显示器命中检测、
物理像素裁剪参数计算、preview 裁剪参数计算提取为独立纯函数，
均不依赖 Tauri/Quartz 类型。

新增 6 个单元测试覆盖多显示器、偏移坐标、大小 canvas 边界。
start_scroll_recording 主体从 502 行降到约 430 行（后续截图 thunk
提取可进一步降低）。preview crop 两处重复逻辑同步消除。


💘 Generated with Crush
EOF
)"
```

---

## Task 3: `Coordinator::new` 拆分

**Files:**
- Modify: `crates/desktop/src/coordinator.rs:190-521`

**Interfaces:**
- Produces: `fn build_coordinator_loop(rx, tx_self, audio, engine, config, app_handle, runtime_config)`（私有函数）

**关键约束：** `Coordinator::new` 内部是一个 `std::thread::spawn(move || { loop {...} })`，拆分时必须保持 `move` 闭包所有权语义不变。

---

- [ ] **Step 1: 验证基线**

Run: `cd /Users/wudarui/workspace/agent/octopus && cargo test -p octopus-desktop 2>&1 | tail -5`
Expected: all pass

- [ ] **Step 2: 提取 `build_coordinator_loop` 函数**

在 `coordinator.rs` 中，将 `Coordinator::new` 内部行 206-516 的 `std::thread::spawn(move || { ... })` 内容提取为独立函数：

```rust
fn build_coordinator_loop(
    rx: Receiver<Command>,
    tx_self: Sender<Command>,
    audio: Arc<SharedAudioState>,
    engine: Arc<dyn TranscriptionEngine>,
    mut config: AppConfig,
    app_handle: tauri::AppHandle,
    _runtime_config: crate::runtime_config::SharedRuntimeConfig,
) {
    let use_streaming = config.engine_mode == "embedded" && crate::config::is_streaming_engine(&config);
    #[cfg(feature = "cloud")]
    let mut use_cloud_streaming = false;

    std::thread::spawn(move || {
        // ... 原 loop { match rx.recv() { ... } } 全部内容原样搬入 ...
    });
}
```

`Coordinator::new` 简化为：
```rust
pub fn new(
    engine: Arc<dyn TranscriptionEngine>,
    audio: Arc<SharedAudioState>,
    config: AppConfig,
    app_handle: tauri::AppHandle,
    runtime_config: crate::runtime_config::SharedRuntimeConfig,
) -> Self {
    let (tx, rx): (Sender<Command>, Receiver<Command>) = mpsc::channel();
    let tx_self = tx.clone();
    build_coordinator_loop(rx, tx_self, audio, engine, config, app_handle, runtime_config);
    Self {
        tx: parking_lot::Mutex::new(tx),
    }
}
```

**注意**：`use_streaming` 在原代码中是 `let use_streaming = ...; let mut use_streaming = use_streaming;`（行 200-202），在提取时合并为 `let mut use_streaming = ...`。

- [ ] **Step 3: 编译验证**

Run: `cargo check -p octopus-desktop 2>&1 | tail -10`
Expected: 编译通过

- [ ] **Step 4: 行为保持 review + 全量测试**

Run:
```bash
cargo test -p octopus-desktop 2>&1 | tail -10
cargo test --workspace 2>&1 | tail -10
```
Expected: all pass

`git diff` review：
- [ ] `move` 闭包内所有权转移不变
- [ ] `tx_self` 用途不变
- [ ] 所有 `Command::*` 分支保留

- [ ] **Step 5: Commit**

```bash
git add crates/desktop/src/coordinator.rs
git commit -m "$(cat <<'EOF'
refactor(desktop): 拆分 Coordinator::new，提取 build_coordinator_loop

将 new() 内 330 行的状态机循环体提取为独立函数 build_coordinator_loop。
new() 仅保留 channel 创建 + 调用 build_coordinator_loop + 返回 Self。
行为零变化——move 闭包所有权语义、Command 分支、tx_self 用途全部保留。


💘 Generated with Crush
EOF
)"
```

---

## Task 4: `begin_recording` 拆分

**Files:**
- Modify: `crates/desktop/src/coordinator.rs:712-940`

**Interfaces:**
- Produces 3 个私有函数（`#[cfg(feature)]` 守卫对称）

**关键约束：** `begin_recording` 有 3 个引擎分支（streaming / cloud streaming / VAD segmented），每个分支约 60-80 行。拆分时保持 `#[cfg(feature = "cloud")]` 守卫对称。

---

- [ ] **Step 1: 读取 begin_recording 完整内容，确认分支边界**

Run: Read `coordinator.rs` 行 712-940，确认三个分支的精确起止行。

- [ ] **Step 2: 提取 `prepare_streaming_session`**

将 `use_streaming && !use_cloud_streaming` 分支提取为：

```rust
#[allow(clippy::too_many_arguments)]
fn prepare_streaming_session(
    audio: &Arc<SharedAudioState>,
    engine: &Arc<dyn TranscriptionEngine>,
    config: &AppConfig,
    app_handle: &tauri::AppHandle,
    tx: &Sender<Command>,
    selection: Option<(String, usize, usize)>,
) -> Stage {
    // ... 原 streaming 分支内容 ...
}
```

- [ ] **Step 3: 提取 `prepare_cloud_streaming_session`（`#[cfg(feature = "cloud")]`）**

```rust
#[cfg(feature = "cloud")]
#[allow(clippy::too_many_arguments)]
fn prepare_cloud_streaming_session(
    audio: &Arc<SharedAudioState>,
    config: &AppConfig,
    app_handle: &tauri::AppHandle,
    tx: &Sender<Command>,
    selection: Option<(String, usize, usize)>,
) -> Stage {
    // ... 原 cloud 分支内容 ...
}
```

- [ ] **Step 4: 提取 `prepare_vad_segmented_session`**

```rust
#[allow(clippy::too_many_arguments)]
fn prepare_vad_segmented_session(
    audio: &Arc<SharedAudioState>,
    config: &AppConfig,
    app_handle: &tauri::AppHandle,
    tx: &Sender<Command>,
    selection: Option<(String, usize, usize)>,
) -> Stage {
    // ... 原 VAD 分支内容 ...
}
```

- [ ] **Step 5: 简化 `begin_recording` 主体**

```rust
fn begin_recording(
    stage: &mut Stage,
    audio: &Arc<SharedAudioState>,
    engine: &Arc<dyn TranscriptionEngine>,
    config: &AppConfig,
    app_handle: &tauri::AppHandle,
    tx: &Sender<Command>,
    use_streaming: bool,
    selection: Option<(String, usize, usize)>,
    #[cfg(feature = "cloud")] use_cloud_streaming: bool,
) {
    info!("Toggle: starting {}", {
        #[cfg(feature = "cloud")]
        { if use_cloud_streaming { "cloud streaming" } else if use_streaming { "streaming" } else { "VAD segmented" } }
        #[cfg(not(feature = "cloud"))]
        { if use_streaming { "streaming" } else { "VAD segmented" } }
    });

    if let Err(e) = audio.start(&config.microphone) {
        error!("Failed to start recording: {}", e);
        return;
    }

    #[cfg(feature = "cloud")]
    {
        if use_cloud_streaming {
            *stage = prepare_cloud_streaming_session(audio, config, app_handle, tx, selection);
            return;
        }
    }

    if use_streaming {
        *stage = prepare_streaming_session(audio, engine, config, app_handle, tx, selection);
    } else {
        *stage = prepare_vad_segmented_session(audio, config, app_handle, tx, selection);
    }
}
```

- [ ] **Step 6: 编译验证**

Run: `cargo check -p octopus-desktop 2>&1 | tail -10`
Expected: 编译通过（注意 `#[cfg(feature = "cloud")]` 守卫在参数和分支上对称）

- [ ] **Step 7: 行为保持 review + 全量测试**

Run:
```bash
cargo test -p octopus-desktop 2>&1 | tail -10
cargo test --workspace 2>&1 | tail -10
cargo clippy -p octopus-desktop --all-targets 2>&1 | grep "^warning:" | head
```
Expected: all pass, 0 warnings

`git diff` review：
- [ ] 三个分支逻辑等价（`audio.start` 在分支前调用）
- [ ] `#[cfg(feature = "cloud")]` 守卫对称
- [ ] `Stage` 赋值语义不变

- [ ] **Step 8: Commit**

```bash
git add crates/desktop/src/coordinator.rs
git commit -m "$(cat <<'EOF'
refactor(desktop): 拆分 begin_recording，按引擎分支提取 3 个 prepare 函数

将 228 行的 begin_recording 按引擎类型拆为：
- prepare_streaming_session（本地流式）
- prepare_cloud_streaming_session（云端流式，cfg=cloud）
- prepare_vad_segmented_session（VAD 分段伪流式）

begin_recording 主体仅保留 audio.start + 分支选择。行为零变化——
audio.start 调用时序、Stage 赋值、cfg(feature) 守卫全部保留。


💘 Generated with Crush
EOF
)"
```

---

## 最终验证

- [ ] **全量编译 + 测试（主仓库）**

```bash
cd /Users/wudarui/workspace/agent/octopus
cargo clippy --workspace --all-targets 2>&1 | grep "^warning:" | wc -l
cargo test --workspace 2>&1 | tail -20
```
Expected: 0 warnings, all tests pass

- [ ] **更新文档**

更新 `docs/architecture.md`：
- paddle-ocr 新增 `vision/numeric.rs` 说明
- desktop 新增 `screenshot_geometry.rs` 说明
- coordinator 内部结构变化

Commit 文档：
```bash
git add docs/architecture.md
git commit -m "$(cat <<'EOF'
docs: 同步 architecture.md 重构变化

记录 paddle-ocr vision/numeric.rs 集中、desktop screenshot_geometry.rs
提取、coordinator 内部函数拆分。


💘 Generated with Crush
EOF
)"
```

## Self-Review 清单

- [ ] **Spec coverage**: 4 项优化全部有对应 task
- [ ] **Type consistency**: `MonitorRect`/`SelectionGlobal`/`PhysicalCrop` 在 Task 2 定义后一致使用
- [ ] **No placeholders**: 每步都有完整代码
- [ ] **cfg 守卫对称**: Task 4 的 `#[cfg(feature = "cloud")]` 在参数和分支上对称
- [ ] **测试先行**: Task 1 和 Task 2 的纯函数都是先写失败测试再实现

## Execution Handoff

**Plan complete and saved to `docs/superpowers/plans/2026-06-12-refactor-deadcode-dup-long.md`. Two execution options:**

**1. Subagent-Driven (recommended)** - 每个 task 派 fresh subagent，task 间 review
**2. Inline Execution** - 当前 session 逐 task 执行，checkpoint review

**Which approach?**
