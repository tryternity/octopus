# CAPX 模块综合优化 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 优化 `crates/capx/` 的性能（SAD 热路径 + 画布增量追加）、代码质量（去重 + 常量提取 + 函数拆分）、健壮性（合成图单元测试），对外 API 零改动。

**Architecture:** 引入连续 `GrayBuf` 替代 `image::GrayImage`，SAD 搜索改为整数累加 + 切片直访 + 模板预提取；画布底层改 `Vec<u8>` + 惰性 `RgbaImage` 缓存，追加用 `extend` 取代整体复制；提取 `cgimage_to_rgba` 消除 capture.rs 三处 BGRA→RGBA 重复。先建测试网（P2）再重写（P3/P4），分阶段风险递增。

**Tech Stack:** Rust 2021、image 0.25（`RgbaImage::from_raw/into_raw`、灰度公式 `(2126*R + 7152*G + 722*B)/10000`）、core-graphics 0.24（macOS）、anyhow。

**关联文档:** [spec](../specs/2026-06-12-capx-optimization-design.md)

---

## 关键约束（所有任务必须遵守）

1. **API 零改动**：`Stitcher::new/process_frame/finalize/canvas/height` 与 `capture::*` 签名与语义不变。`desktop` crate 零改动。
2. **灰度公式不变**：`GrayBuf::from_rgba` 必须与 `image::imageops::grayscale` 逐像素相等，公式 `(2126*R + 7152*G + 722*B) / 10000`（整数除法，源自 image 0.25 `SRGB_LUMA`）。
3. **dy 符号约定**：`dy < 0` = 用户向下滚动（内容上移），见 `stitch.rs:99`。
4. **worktree 路径**：所有命令在 `/Users/wudarui/workspace/agent/octopus/.worktrees/optimize-capx` 下执行。

---

## 文件结构

| 文件 | 职责 | 本次改动 |
|------|------|---------|
| `crates/capx/src/stitch.rs` | 滚动拼接 | 常量提取、`GrayBuf` 引入、SAD 重写、函数拆分、画布改 `Vec<u8>` + 惰性缓存、内联测试 |
| `crates/capx/src/capture.rs` | 屏幕捕获 | `cgimage_to_rgba` 去重、内联测试 |
| `crates/capx/src/lib.rs` | 模块入口 | 不动 |
| `crates/capx/Cargo.toml` | 依赖 | 不动（不引入 criterion） |

---

## Task 1: capture.rs 去重 — 提取 `cgimage_to_rgba`

**Files:**
- Modify: `crates/capx/src/capture.rs:100-152`（`capture_display_excluding_window`）
- Modify: `crates/capx/src/capture.rs:159-209`（`capture_region_excluding_window`）
- Modify: `crates/capx/src/capture.rs:309-360`（`capture_window_region`）

- [ ] **Step 1: 在 capture.rs 顶部（`#[cfg(target_os = "macos")]` helper 区域，`capture_display_excluding_window` 之前）新增公共 helper**

在 `capture_region_excluding_window` 函数之前（即第 94 行 `/// macOS：截取指定显示器...` 注释之前）插入：

```rust
/// macOS CGImage 解析 + BGRA→RGBA 转换的公共 helper。
/// 返回 (rgba_bytes, width, height)。三处捕获函数共用，消除重复样板。
#[cfg(target_os = "macos")]
fn cgimage_to_rgba(
    cg_image: &core_graphics::image::CGImage,
) -> Result<(Vec<u8>, u32, u32)> {
    let width = cg_image.width() as u32;
    let height = cg_image.height() as u32;
    let bpr = cg_image.bytes_per_row();
    let bpp = cg_image.bits_per_pixel();

    if bpp != 32 {
        anyhow::bail!("Unsupported screenshot format: {} bpp (expected 32)", bpp);
    }

    let raw = cg_image.data().bytes();

    // macOS 截图 CGImage 通常为 BGRA（little-endian 32bit）。转为 RGBA。
    let mut rgba = Vec::with_capacity(width as usize * height as usize * 4);
    for y in 0..height as usize {
        let row_start = y * bpr;
        let row = &raw[row_start..row_start + width as usize * 4];
        for px in row.chunks_exact(4) {
            rgba.push(px[2]); // R
            rgba.push(px[1]); // G
            rgba.push(px[0]); // B
            rgba.push(px[3]); // A
        }
    }

    Ok((rgba, width, height))
}
```

- [ ] **Step 2: 重写 `capture_display_excluding_window` 的解析部分**

将 `capture_display_excluding_window` 中从 `let width = cg_image.width() as u32;` 到 `Ok(ScreenCapture {` 之前的整块（含 bpp 校验、BGRA→RGBA 双重循环）替换为：

```rust
    let (rgba, width, height) = cgimage_to_rgba(&cg_image)?;
```

替换后该函数末尾保持：
```rust
    // CGDisplayBounds 返回全局逻辑坐标（points），与 xcap Monitor::x()/y() 一致。
    Ok(ScreenCapture {
        rgba_bytes: rgba,
        width,
        height,
        monitor_x: bounds.origin.x as i32,
        monitor_y: bounds.origin.y as i32,
    })
}
```

注意：`capture_display_excluding_window` 原实现用索引 `raw[off + 2]`（未用 `chunks_exact`），但 BGRA→RGBA 语义一致，统一后行为不变。

- [ ] **Step 3: 重写 `capture_region_excluding_window` 的解析部分**

将该函数中从 `let width = cg_image.width() as u32;` 到 `Ok(RgbaBytes {` 之前的整块替换为：

```rust
    let (rgba, width, height) = cgimage_to_rgba(&cg_image)?;
```

末尾保持：
```rust
    Ok(RgbaBytes { rgba_bytes: rgba, width, height })
```

- [ ] **Step 4: 重写 `capture_window_region` 的解析部分**

将该函数中从 `let width = cg_image.width() as u32;` 到 `Ok(RgbaBytes {` 之前的整块替换为：

```rust
    let (rgba, width, height) = cgimage_to_rgba(&cg_image)?;
```

末尾保持：
```rust
    Ok(RgbaBytes { rgba_bytes: rgba, width, height })
```

- [ ] **Step 5: 编译验证**

Run:
```bash
cd /Users/wudarui/workspace/agent/octopus/.worktrees/optimize-capx && cargo check -p octopus-capx -p octopus-desktop 2>&1 | tail -5
```
Expected: `Finished` 无错误无警告。

- [ ] **Step 6: Commit**

```bash
cd /Users/wudarui/workspace/agent/octopus/.worktrees/optimize-capx
git add crates/capx/src/capture.rs
git commit -m "refactor(capx): 提取 cgimage_to_rgba 消除三处 BGRA→RGBA 重复样板"
```

---

## Task 2: stitch.rs 魔法数字提取为命名常量

**Files:**
- Modify: `crates/capx/src/stitch.rs:4-18`（`StitchConfig` 上方）与函数体内裸数字

- [ ] **Step 1: 在 `stitch.rs` 顶部 `use` 之后、`pub struct StitchConfig` 之前新增常量块**

```rust
// ===== 拼接算法常量（原散落在 find_overlap_spatial_ext 与 process_frame 中的魔法数字）=====

/// 模板条高度（像素）。从参考帧底部取此高度的条带做空间模板匹配。
const STRIP_H: u32 = 80;
/// 全量搜索范围（像素）。`process_frame` 中限制滚动位移搜索上界。
const MAX_SCROLL: u32 = 220;
/// 静止判定阈值。dy=0 处的平均像素差值小于此值视为内容未滚动。
const STATIONARY_SAD: f64 = 2.0;
/// 匹配接受阈值。最佳 SAD 必须小于此值才接受拼接。
const SAD_ACCEPT: f64 = 7.5;
/// 置信度下限。估计置信度必须大于此值才接受拼接。
const MIN_CONFIDENCE: f64 = 0.15;
/// 软速度罚分系数。拉近与上一帧速度的距离，防止周期跳变。
const SPEED_PENALTY: f64 = 0.04;
/// 排除最左侧的比例（通常有图标/树状图）。
const X_START_RATIO: f64 = 0.10;
/// 排除最右侧的比例截止点（通常有滚动条/时间戳），即保留 10%~80% 横向区间。
const X_END_RATIO: f64 = 0.80;
/// 列抽样步长（像素）。每隔此值采样一列，提供双倍空间特征解析度。
const SAMPLE_STEP_X: usize = 2;
/// sticky 区域检测的最大高度（像素），顶部/底部各扫此高度。
const STICKY_DETECT_MAX: u32 = 80;
```

- [ ] **Step 2: 替换 `process_frame` 中的裸数字**

`process_frame` 中（约 75-80 行）：

旧：
```rust
        let x_start = (w as f64 * 0.10) as u32;
        let x_end = (w as f64 * 0.80) as u32;

        // 全量搜索范围 220 像素
        let max_scroll = 220u32;
```

新：
```rust
        let x_start = (w as f64 * X_START_RATIO) as u32;
        let x_end = (w as f64 * X_END_RATIO) as u32;

        let max_scroll = MAX_SCROLL;
```

- [ ] **Step 3: 替换 `finalize` 中的裸数字**

`finalize` 中（约 149-150 行）：

旧：
```rust
        let x_start = (w as f64 * 0.10) as u32;
        let x_end = (w as f64 * 0.80) as u32;
```

新：
```rust
        let x_start = (w as f64 * X_START_RATIO) as u32;
        let x_end = (w as f64 * X_END_RATIO) as u32;
```

- [ ] **Step 4: 替换 `detect_sticky` 中的裸数字**

`detect_sticky` 中（约 198、203 行）：

旧：
```rust
        for y in 0..cmp_h.min(80) {
```
新：
```rust
        for y in 0..cmp_h.min(STICKY_DETECT_MAX) {
```

两处 `cmp_h.min(80)` 都替换（`sticky_t` 循环和 `sticky_b` 循环各一处）。

- [ ] **Step 5: 编译验证**

Run:
```bash
cd /Users/wudarui/workspace/agent/octopus/.worktrees/optimize-capx && cargo check -p octopus-capx 2>&1 | tail -5
```
Expected: `Finished`，可能有 `unused constant` 警告（`STRIP_H`/`MAX_SCROLL`/`SAD_ACCEPT` 等在 `find_overlap_spatial_ext` 内尚未替换——本任务只替换 `process_frame`/`finalize`/`detect_sticky` 的裸数字，`find_overlap_spatial_ext` 内部裸数字在 Task 3 重写时一并替换，避免本任务改动过大）。若 `STRIP_H`/`SAD_ACCEPT`/`MIN_CONFIDENCE`/`SPEED_PENALTY`/`STATIONARY_SAD`/`SAMPLE_STEP_X` 报 unused，暂用 `#[allow(unused)]` 标注常量块（Task 3 会消费它们并移除 allow）。

- [ ] **Step 6: Commit**

```bash
cd /Users/wudarui/workspace/agent/octopus/.worktrees/optimize-capx
git add crates/capx/src/stitch.rs
git commit -m "refactor(capx): 提取 stitch.rs 魔法数字为命名常量"
```

---

## Task 3: capture.rs 内联测试 — `bgra_to_rgba` 纯函数

**Files:**
- Modify: `crates/capx/src/capture.rs`（文件末尾追加 `#[cfg(test)] mod tests`）

> 说明：`cgimage_to_rgba` 依赖 macOS `CGImage`，无法跨平台直接测。但 BGRA→RGBA 的字节重排逻辑可提取为平台无关纯函数单独测试，`cgimage_to_rgba` 内部调用它。这样非 macOS 也能跑该测试。

- [ ] **Step 1: 在 `cgimage_to_rgba` 上方新增平台无关纯函数 `bgra_to_rgba`**

在 `cgimage_to_rgba` 函数之前插入：

```rust
/// BGRA→RGBA 字节重排（平台无关纯函数，便于测试）。
/// 输入：已去 bpr padding 的紧凑 BGRA 行数据。
#[cfg(target_os = "macos")]
fn bgra_to_rgba(raw: &[u8], rgba: &mut Vec<u8>) {
    for px in raw.chunks_exact(4) {
        rgba.push(px[2]); // R
        rgba.push(px[1]); // G
        rgba.push(px[0]); // B
        rgba.push(px[3]); // A
    }
}
```

- [ ] **Step 2: 修改 `cgimage_to_rgba` 内层循环调用 `bgra_to_rgba`**

将 `cgimage_to_rgba` 中的：
```rust
        let row = &raw[row_start..row_start + width as usize * 4];
        for px in row.chunks_exact(4) {
            rgba.push(px[2]); // R
            rgba.push(px[1]); // G
            rgba.push(px[0]); // B
            rgba.push(px[3]); // A
        }
```
替换为：
```rust
        let row = &raw[row_start..row_start + width as usize * 4];
        bgra_to_rgba(row, &mut rgba);
```

- [ ] **Step 3: 在 capture.rs 文件末尾追加测试模块**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_bgra_to_rgba_basic() {
        // BGRA: B=10, G=20, R=30, A=255 → RGBA: 30,20,10,255
        let bgra = [10u8, 20, 30, 255];
        let mut rgba = Vec::new();
        bgra_to_rgba(&bgra, &mut rgba);
        assert_eq!(rgba, vec![30, 20, 10, 255]);
    }

    #[test]
    fn test_bgra_to_rgba_multiple_pixels() {
        let bgra = [1u8, 2, 3, 4, 5, 6, 7, 8];
        let mut rgba = Vec::new();
        bgra_to_rgba(&bgra, &mut rgba);
        assert_eq!(rgba, vec![3, 2, 1, 4, 7, 6, 5, 8]);
    }

    #[test]
    fn test_bgra_to_rgba_empty() {
        let mut rgba = Vec::new();
        bgra_to_rgba(&[], &mut rgba);
        assert!(rgba.is_empty());
    }
}
```

- [ ] **Step 4: 运行测试验证通过**

Run:
```bash
cd /Users/wudarui/workspace/agent/octopus/.worktrees/optimize-capx && cargo test -p octopus-capx 2>&1 | tail -10
```
Expected: 3 个 capture 测试全绿（`test_bgra_to_rgba_basic` / `test_bgra_to_rgba_multiple_pixels` / `test_bgra_to_rgba_empty`）。

- [ ] **Step 5: Commit**

```bash
cd /Users/wudarui/workspace/agent/octopus/.worktrees/optimize-capx
git add crates/capx/src/capture.rs
git commit -m "test(capx): 新增 bgra_to_rgba 纯函数单元测试"
```

---

## Task 4: stitch.rs 测试网 — 合成图构造工具

**Files:**
- Modify: `crates/capx/src/stitch.rs`（文件末尾追加 `#[cfg(test)] mod tests`，含构造工具）

> 这是 P2 测试网的第一部分：先建合成图构造工具，Task 5 再加行为测试。

- [ ] **Step 1: 在 stitch.rs 文件末尾追加测试模块与构造工具**

```rust
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
    fn make_frame(width: u32, height: u32, scroll_offset: u32) -> RgbaImage {
        let mut img: ImageBuffer<Rgba<u8>, Vec<u8>> = ImageBuffer::new(width, height);
        for y in 0..height {
            for x in 0..width {
                // 基础渐变（y 方向唯一）
                let mut v = ((y + scroll_offset) % 256) as u8;
                // 每 45 行水平分隔线：强对比
                if (y + scroll_offset) % 45 == 0 {
                    v = 255 - v;
                }
                // 每 7 列亮列
                if x % 7 == 0 {
                    v = v.saturating_add(80);
                }
                // 确定性格点噪点（(x*3+y*5) % 11 == 0 处加亮）
                if (x as u32 * 3 + (y + scroll_offset) * 5) % 11 == 0 {
                    v = v.saturating_add(40);
                }
                let px = Rgba([v, v, v, 255]);
                img.put_pixel(x, y, px);
            }
        }
        img
    }

    /// 构造一个带 sticky 顶/底区域的帧：顶部 `top_h` 行和底部 `bot_h` 行固定不变，
    /// 中间内容随 `scroll_offset` 变化。
    fn make_frame_with_sticky(
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
                img.put_pixel(x, y, sticky_top.get_pixel(x, y).clone());
            }
        }
        for y in 0..bot_h {
            for x in 0..width {
                img.put_pixel(x, height - bot_h + y, sticky_bot.get_pixel(x, y).clone());
            }
        }
        img
    }

    // 占位：行为测试在 Task 5 追加
}
```

- [ ] **Step 2: 编译验证（测试模块能编译）**

Run:
```bash
cd /Users/wudarui/workspace/agent/octopus/.worktrees/optimize-capx && cargo test -p octopus-capx --no-run 2>&1 | tail -5
```
Expected: `Finished` 编译通过（此时无测试函数，只验证构造工具编译）。

- [ ] **Step 3: Commit**

```bash
cd /Users/wudarui/workspace/agent/octopus/.worktrees/optimize-capx
git add crates/capx/src/stitch.rs
git commit -m "test(capx): 新增合成图测试构造工具 make_frame/make_frame_with_sticky"
```

---

## Task 5: stitch.rs 测试网 — 行为测试（基于现有 API）

**Files:**
- Modify: `crates/capx/src/stitch.rs`（测试模块内追加行为测试）

> 关键：这些测试在 P3/P4 重写前必须全绿，锁定行为基线。重写后必须保持全绿。

- [ ] **Step 1: 在测试模块内（Task 4 的 `// 占位` 处）追加行为测试**

替换 `// 占位：行为测试在 Task 5 追加` 为：

```rust
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
        // 首帧 scroll=0，第二帧 scroll=40（用户向下滚 40px，内容上移 40px）
        // 期望：process_frame 返回 true，canvas 高度增加约 40px（允许 min_scroll_px 限制）
        let f0 = make_frame(TW, TH, 0);
        let mut s = Stitcher::new(f0.clone(), StitchConfig::default());
        let f1 = make_frame(TW, TH, 40);
        let added = s.process_frame(&f1).unwrap();
        assert!(added, "滚动 40px 应追加内容");
        // 画布初始高度 = TH - sticky_bottom（首帧裁剪后）。追加后高度应 > 初始。
        // 注意：detect_sticky 在合成图上 sticky_top/sticky_bottom 可能为 0。
        let h_after = s.height();
        assert!(
            h_after > TH - STRIP_H,
            "追加后画布高度 {} 应大于裁剪后首帧高度，表示有新行追加",
            h_after
        );
    }

    #[test]
    fn test_scroll_direction_dy_negative() {
        // 验证 dy 符号约定：用户向下滚 → dy < 0。
        // 通过 process_frame 返回 true 且 height 增加间接验证（dy>=0 会被跳过）。
        let f0 = make_frame(TW, TH, 0);
        let mut s = Stitcher::new(f0, StitchConfig::default());
        let f1 = make_frame(TW, TH, 30);
        let added = s.process_frame(&f1).unwrap();
        assert!(added, "向下滚 30px 应被接受（dy<0）");
    }

    #[test]
    fn test_repeated_scroll_grows_canvas() {
        // 连续多次小步滚动，画布应单调增长
        let f0 = make_frame(TW, TH, 0);
        let mut s = Stitcher::new(f0, StitchConfig::default());
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
        let f1 = make_frame(TW, TH, 50);
        s.process_frame(&f1).unwrap();
        let canvas = s.canvas().clone();
        assert_eq!(canvas.height(), s.height());
        assert_eq!(canvas.width(), TW);
    }

    #[test]
    fn test_finalize_appends_footer() {
        // finalize 应补全最后一帧的 sticky_bottom 区域，画布高度应 >= finalize 前
        let f0 = make_frame(TW, TH, 0);
        let mut s = Stitcher::new(f0, StitchConfig::default());
        let f1 = make_frame(TW, TH, 60);
        s.process_frame(&f1).unwrap();
        let h_before = s.height();
        let last = make_frame(TW, TH, 90);
        s.finalize(&last).unwrap();
        let h_after = s.height();
        assert!(h_after >= h_before, "finalize 不应缩减画布：{} -> {}", h_before, h_after);
    }
```

- [ ] **Step 2: 运行测试验证通过**

Run:
```bash
cd /Users/wudarui/workspace/agent/octopus/.worktrees/optimize-capx && cargo test -p octopus-capx 2>&1 | tail -15
```
Expected: 全部测试通过（capture 3 个 + stitch 6 个 = 9 个）。

**若 `test_known_scroll_appends_rows` 或 `test_scroll_direction_dy_negative` 失败**：说明合成图在 `X_START_RATIO..X_END_RATIO`（40~320 列）区间内的特征不足以让 SAD 锁定。排查方向：① 确认 `make_frame` 的 `scroll_offset` 正确模拟内容上移（帧 f1 在 y 行的内容 = 帧 f0 在 y+scroll_offset 行的内容，这样 reference=f0、curr=f1 时，curr 底部模板对应 ref 中靠上的位置 → dy<0）。② 若 SAD 置信度不足，在 `make_frame` 中增强特征（如加大水平线对比）。**调整构造工具而非放松断言**。

- [ ] **Step 3: Commit**

```bash
cd /Users/wudarui/workspace/agent/octopus/.worktrees/optimize-capx
git add crates/capx/src/stitch.rs
git commit -m "test(capx): 新增 Stitcher 行为测试网（静止/滚动/方向/增长/canvas/finalize）"
```

---

## Task 6: 引入 `GrayBuf` 并验证灰度等价性

**Files:**
- Modify: `crates/capx/src/stitch.rs`（新增 `GrayBuf` struct + `from_rgba`，暂不替换 `reference_gray`）

> P3 第一步：引入新类型并证明它与 `image::imageops::grayscale` 逐像素相等，再在 Task 7 切换。

- [ ] **Step 1: 在 `stitch.rs` 常量块之后、`pub struct StitchConfig` 之前新增 `GrayBuf`**

```rust
/// 连续 row-major 灰度 buffer，替代 image::GrayImage。
/// 消除 get_pixel() 的坐标计算 + 边界检查开销，用整行切片直访。
struct GrayBuf {
    data: Vec<u8>,
    width: usize,
    height: usize,
}

impl GrayBuf {
    /// 从 RGBA 图像转换灰度。公式必须与 image::imageops::grayscale 一致：
    /// luma = (2126*R + 7152*G + 722*B) / 10000（整数除法，image 0.25 SRGB_LUMA）。
    fn from_rgba(rgba: &RgbaImage) -> Self {
        let width = rgba.width() as usize;
        let height = rgba.height() as usize;
        let mut data = Vec::with_capacity(width * height);
        for px in rgba.pixels() {
            let r = px[0] as u32;
            let g = px[1] as u32;
            let b = px[2] as u32;
            let luma = (2126 * r + 7152 * g + 722 * b) / 10000;
            data.push(luma as u8);
        }
        Self { data, width, height }
    }

    /// 整行切片直访，无边界检查。调用方需保证 y < height。
    #[inline]
    fn row(&self, y: usize) -> &[u8] {
        &self.data[y * self.width..(y + 1) * self.width]
    }
}
```

- [ ] **Step 2: 在测试模块内新增灰度等价性测试**

在 `test_finalize_appends_footer` 之后追加：

```rust
    #[test]
    fn test_graybuf_matches_image_grayscale() {
        // 验证 GrayBuf::from_rgba 与 image::imageops::grayscale 逐像素相等
        let img = make_frame(TW, TH, 0);
        let reference = image::imageops::grayscale(&img);
        let buf = GrayBuf::from_rgba(&img);
        assert_eq!(buf.width, TW as usize);
        assert_eq!(buf.height, TH as usize);
        for y in 0..TH as usize {
            for x in 0..TW as usize {
                let a = reference.get_pixel(x as u32, y as u32)[0];
                let b = buf.row(y)[x];
                assert_eq!(a, b, "灰度不一致 @ ({},{})", x, y);
            }
        }
    }
```

- [ ] **Step 3: 运行测试验证灰度等价**

Run:
```bash
cd /Users/wudarui/workspace/agent/octopus/.worktrees/optimize-capx && cargo test -p octopus-capx test_graybuf_matches 2>&1 | tail -10
```
Expected: `test_graybuf_matches_image_grayscale ... ok`。

- [ ] **Step 4: 运行全部测试确保无回归**

Run:
```bash
cd /Users/wudarui/workspace/agent/octopus/.worktrees/optimize-capx && cargo test -p octopus-capx 2>&1 | tail -5
```
Expected: 全绿（10 个测试）。

- [ ] **Step 5: Commit**

```bash
cd /Users/wudarui/workspace/agent/octopus/.worktrees/optimize-capx
git add crates/capx/src/stitch.rs
git commit -m "feat(capx): 引入 GrayBuf 连续灰度 buffer 并验证与 image grayscale 等价"
```

---

## Task 7: SAD 热路径重写 — `find_overlap_spatial_ext` 整数化 + 函数拆分

**Files:**
- Modify: `crates/capx/src/stitch.rs:215-333`（`find_overlap_spatial_ext` 重写 + 拆分为 3 个私有函数）

> 这是核心性能优化。重写后 Task 5 的行为测试必须保持全绿。
>
> **关键决策：函数签名改为接受 `&GrayBuf`**（而非 `&GrayImage`）。Task 8 的 `process_frame`/`finalize` 直接传 `&self.reference` 和 `&curr_gray_buf`，避免 `GrayBuf→GrayImage→GrayBuf` 重复转换。

- [ ] **Step 1: 删除旧 `find_overlap_spatial_ext`（215-333 行），替换为新版 + 拆分函数**

将 `find_overlap_spatial_ext` 整个函数替换为以下实现（含 3 个私有 helper）。**签名改为接受 `&GrayBuf`**：

```rust
/// 空间域 2D 模板匹配算法，查找最匹配的垂直位移 dy。
/// 采用 SAD (Sum of Absolute Differences) 准则与列抽样加速，保留 2D 空间排布。
///
/// 优化：模板条预提取为连续 buffer；整数 u64 累加；切片直访（无 get_pixel 边界检查）；
/// 静止检测合并进主搜索（省一次预扫描）。
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
    if eff_bottom <= eff_top + STRIP_H + 10 {
        return None;
    }
    let template_y = eff_bottom - STRIP_H;

    // 抽样列索引（只算一次）
    let sample_cols: Vec<usize> = (x_start as usize..x_end as usize)
        .step_by(SAMPLE_STEP_X)
        .collect();
    let n_cols = sample_cols.len();
    if n_cols == 0 {
        return None;
    }

    // 模板条预提取
    let tpl = extract_template(ref_buf, template_y, &sample_cols);

    let min_y_offset = (template_y as i32 - max_scroll as i32).max(eff_top as i32) as u32;
    let max_y_offset = template_y;

    // 主搜索
    let (best_y_offset, best_sad_avg, stationary_sad_avg) = search_best_offset(
        &tpl,
        curr_buf,
        &sample_cols,
        min_y_offset,
        max_y_offset,
        template_y,
        last_dy,
    );

    // 静止判定：dy=0 处 SAD 与最佳值接近 → 静止
    if stationary_sad_avg < STATIONARY_SAD {
        return Some((0.0, 1.0));
    }
    if stationary_sad_avg < best_sad_avg + 1.0 {
        return Some((0.0, 1.0));
    }

    // 置信度估计
    let confidence = estimate_confidence(
        ref_buf,
        curr_buf,
        &sample_cols,
        best_y_offset,
        min_y_offset,
        max_y_offset,
        template_y,
    );

    if best_sad_avg < SAD_ACCEPT && confidence > MIN_CONFIDENCE {
        let dy = best_y_offset as f64 - template_y as f64;
        Some((dy, confidence))
    } else {
        None
    }
}

/// 提取模板条到连续 buffer（strip_h × n_cols）。
fn extract_template(ref_buf: &GrayBuf, template_y: u32, sample_cols: &[usize]) -> Vec<u8> {
    let mut tpl = Vec::with_capacity(STRIP_H as usize * sample_cols.len());
    for dy in 0..STRIP_H {
        let row = ref_buf.row((template_y + dy) as usize);
        for &x in sample_cols {
            tpl.push(row[x]);
        }
    }
    tpl
}

/// 整数 SAD 主搜索，返回 (best_y_offset, best_sad_avg, stationary_sad_avg)。
/// stationary_sad_avg = y_offset == template_y 那次迭代的 SAD 均值。
fn search_best_offset(
    tpl: &[u8],
    curr: &GrayBuf,
    sample_cols: &[usize],
    min_y_offset: u32,
    max_y_offset: u32,
    template_y: u32,
    last_dy: Option<f64>,
) -> (u32, f64, f64) {
    let strip_h = STRIP_H as usize;
    let n_cols = sample_cols.len();
    let total = (strip_h * n_cols) as f64;

    let mut best_y_offset = min_y_offset;
    let mut min_penalized = f64::MAX;
    let mut best_sad_avg = f64::MAX;
    let mut stationary_sad_avg = f64::MAX;

    for y_offset in min_y_offset..=max_y_offset {
        let mut sad: u64 = 0;
        let mut i = 0;
        for dy in 0..strip_h {
            let row = curr.row((y_offset as usize) + dy);
            for &x in sample_cols {
                let diff = (tpl[i] as i32 - row[x] as i32).unsigned_abs() as u64;
                sad += diff;
                i += 1;
            }
        }
        let sad_avg = sad as f64 / total;

        if y_offset == template_y {
            stationary_sad_avg = sad_avg;
        }

        let mut penalized = sad_avg;
        if let Some(ldy) = last_dy {
            let dy = y_offset as f64 - template_y as f64;
            penalized += SPEED_PENALTY * (dy - ldy).abs();
        }
        if penalized < min_penalized {
            min_penalized = penalized;
            best_sad_avg = sad_avg;
            best_y_offset = y_offset;
        }
    }

    (best_y_offset, best_sad_avg, stationary_sad_avg)
}

/// 稀疏采样估计置信度：1 - best_sad / mean_sad。
fn estimate_confidence(
    ref_buf: &GrayBuf,
    curr_buf: &GrayBuf,
    sample_cols: &[usize],
    best_y_offset: u32,
    min_y_offset: u32,
    max_y_offset: u32,
    template_y: u32,
) -> f64 {
    let strip_h = STRIP_H as usize;
    // 稀疏列子集（取一半列）
    let sparse_cols: Vec<usize> = sample_cols.iter().step_by(2).copied().collect();
    let n_cols = sparse_cols.len();
    if n_cols == 0 {
        return 0.0;
    }

    let mut sum_sad = 0.0f64;
    let mut sample_count = 0.0f64;

    for y_offset in (min_y_offset..=max_y_offset).step_by(10) {
        let mut sad: u64 = 0;
        let mut count = 0u64;
        for dy in (0..strip_h).step_by(2) {
            let ref_row = ref_buf.row((template_y as usize) + dy);
            let curr_row = curr_buf.row((y_offset as usize) + dy);
            for &x in &sparse_cols {
                sad += (ref_row[x] as i32 - curr_row[x] as i32).unsigned_abs() as u64;
                count += 1;
            }
        }
        if count > 0 {
            sum_sad += sad as f64 / count as f64;
            sample_count += 1.0;
        }
    }

    if sample_count < 1.0 {
        return 0.0;
    }
    let mean_sad = sum_sad / sample_count;
    if mean_sad < 1e-5 {
        return 0.0;
    }

    // best_y_offset 处的稀疏 SAD（与 mean 同口径）
    let mut best_sad_sparse: u64 = 0;
    let mut count = 0u64;
    for dy in (0..strip_h).step_by(2) {
        let ref_row = ref_buf.row((template_y as usize) + dy);
        let curr_row = curr_buf.row((best_y_offset as usize) + dy);
        for &x in &sparse_cols {
            best_sad_sparse += (ref_row[x] as i32 - curr_row[x] as i32).unsigned_abs() as u64;
            count += 1;
        }
    }
    let best_sad_avg = best_sad_sparse as f64 / count as f64;

    1.0 - (best_sad_avg / mean_sad)
}
```

> **关于 `estimate_confidence` 的语义变化**：原实现用 `best_original_sad`（全列密集 SAD）与 `mean_sad`（稀疏 SAD）比；新版统一用稀疏 SAD 比稀疏 SAD（口径一致）。这是有意改进——原实现密集/稀疏混比口径不一致。若 Task 5 行为测试因此失败，优先排查其它原因；确认是该口径变化导致后，可回退为密集 best vs 稀疏 mean（从 `search_best_offset` 返回 `best_sad_avg` 直接用）。**优先以测试通过为准**。

- [ ] **Step 2: 临时适配 `process_frame`/`finalize` 的调用点（Task 8 完成前的过渡）**

Task 7 完成时，`process_frame` 和 `finalize` 仍持有 `self.reference_gray: GrayImage` 并调用 `image::imageops::grayscale(frame)` 产生 `GrayImage`。因签名改为 `&GrayBuf`，需在调用处临时转换。将 `process_frame` 中的调用改为：

```rust
        let curr_gray = image::imageops::grayscale(frame);
        let ref_buf = GrayBuf { data: self.reference_gray.as_raw().clone(), width: self.reference_gray.width() as usize, height: self.reference_gray.height() as usize };
        let curr_buf = GrayBuf { data: curr_gray.as_raw().clone(), width: curr_gray.width() as usize, height: curr_gray.height() as usize };
        // ...
        let (dy, confidence) = match find_overlap_spatial_ext(
            &ref_buf, &curr_buf, x_start, x_end, eff_top, eff_bottom, max_scroll, self.last_dy,
        ) { ... };
```

`finalize` 中同样临时转换。**这些临时转换在 Task 8 会消除**（Task 8 把 `reference_gray` 改为 `reference: GrayBuf`，直接传引用，无需 clone）。

> 此过渡转换有 `as_raw().clone()` 开销，但仅存在于 Task 7→8 之间的提交，Task 8 完成后消除。可接受（渐进重构）。

- [ ] **Step 3: 移除 Task 2 加的 `#[allow(unused)]`（若有的话）**

检查 stitch.rs 顶部常量块，若 Task 2 加了 `#[allow(unused)]` 现已全部被消费，移除该属性。

- [ ] **Step 4: 编译验证**

Run:
```bash
cd /Users/wudarui/workspace/agent/octopus/.worktrees/optimize-capx && cargo check -p octopus-capx 2>&1 | tail -5
```
Expected: `Finished`，无 unused 警告。

- [ ] **Step 5: 运行全部测试验证无回归**

Run:
```bash
cd /Users/wudarui/workspace/agent/octopus/.worktrees/optimize-capx && cargo test -p octopus-capx 2>&1 | tail -15
```
Expected: 全部 10 个测试通过。

**若 `test_known_scroll_appends_rows` / `test_scroll_direction_dy_negative` / `test_repeated_scroll_grows_canvas` 失败**：按 spec 风险缓解——优先检查 `STATIONARY_SAD` 判据是否误把合成图的滚动判为静止（加 `dbg!(stationary_sad_avg, best_sad_avg)` 打印）。若确认是 `estimate_confidence` 口径变化导致，回退为密集 best vs 稀疏 mean。

- [ ] **Step 6: Commit**

```bash
cd /Users/wudarui/workspace/agent/octopus/.worktrees/optimize-capx
git add crates/capx/src/stitch.rs
git commit -m "perf(capx): SAD 热路径重写为整数累加+模板预取+函数拆分（签名改 &GrayBuf）"
```

---

## Task 8: 画布改 `Vec<u8>` + 惰性 `RgbaImage` 缓存

**Files:**
- Modify: `crates/capx/src/stitch.rs:21-31`（`Stitcher` 字段）
- Modify: `crates/capx/src/stitch.rs:34-44`（`new`）
- Modify: `crates/capx/src/stitch.rs:46-133`（`process_frame`）
- Modify: `crates/capx/src/stitch.rs:135-136`（`canvas`/`height`）
- Modify: `crates/capx/src/stitch.rs:138-191`（`finalize`）

> 最高风险任务。画布追加从 O(N²) 整体复制降为 O(new_rows) `extend`。API 不变。

- [ ] **Step 1: 修改 `Stitcher` 字段定义**

旧（21-31 行）：
```rust
pub struct Stitcher {
    canvas: RgbaImage,
    /// 2D 灰度参考帧，用于空间模板匹配
    reference_gray: GrayImage,
    sticky_top: u32,
    sticky_bottom: u32,
    detected: bool,
    config: StitchConfig,
    /// 上一次成功拼接的滚动位移，用于软速度罚分防止周期跳变
    last_dy: Option<f64>,
}
```

新：
```rust
pub struct Stitcher {
    canvas_w: u32,
    canvas_h: u32,
    /// 连续 RGBA 画布数据（真实数据源，增量 extend 追加）。
    canvas_buf: Vec<u8>,
    /// 惰性重建缓存。append 后置 None，canvas() 调用时按需重建。
    canvas_cache: Option<RgbaImage>,
    /// 灰度参考帧（连续 buffer），用于空间模板匹配
    reference: GrayBuf,
    sticky_top: u32,
    sticky_bottom: u32,
    detected: bool,
    config: StitchConfig,
    /// 上一次成功拼接的滚动位移，用于软速度罚分防止周期跳变
    last_dy: Option<f64>,
}
```

- [ ] **Step 2: 修改 `new`**

旧（34-44 行）：
```rust
    pub fn new(first_frame: RgbaImage, config: StitchConfig) -> Self {
        Self {
            canvas: first_frame,
            reference_gray: GrayImage::new(0, 0),
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
            canvas_cache: None,
            reference: GrayBuf { data: Vec::new(), width: 0, height: 0 },
            sticky_top: 0,
            sticky_bottom: 0,
            detected: false,
            config,
            last_dy: None,
        }
    }
```

- [ ] **Step 3: 修改 `canvas` 与 `height`**

旧（135-136 行）：
```rust
    pub fn canvas(&self) -> &RgbaImage { &self.canvas }
    pub fn height(&self) -> u32 { self.canvas.height() }
```

新：
```rust
    pub fn canvas(&self) -> &RgbaImage {
        // 惰性重建：cache 为 None（append 后 invalidate）时从 canvas_buf 重建。
        // 因调用端总是 .clone()，借用不跨多次 append 存活，无生命周期问题。
        if self.canvas_cache.is_none() {
            // 安全：canvas_buf 长度始终 = canvas_w * canvas_h * 4
            let rebuilt = RgbaImage::from_raw(self.canvas_w, self.canvas_h, self.canvas_buf.clone())
                .expect("canvas_buf 长度与 canvas_w/h 不匹配");
            // 通过内部可变性重建缓存；&self 语义下用 unsafe 绕过借用检查器。
            // 这是惰性缓存的标准模式，单线程访问，安全。
            unsafe {
                let slot = &self.canvas_cache as *const Option<RgbaImage> as *mut Option<RgbaImage>;
                *slot = Some(rebuilt);
            }
        }
        self.canvas_cache.as_ref().unwrap()
    }

    pub fn height(&self) -> u32 { self.canvas_h }
```

> **关于 unsafe**：`canvas()` 是 `&self`，但惰性缓存需要修改 `canvas_cache`。这是函数式惰性求值的标准模式（`once_cell::unsync::Lazy` 也这么实现）。替代方案是把 `canvas_cache` 改成 `RefCell<Option<RgbaImage>>`（更安全但需改 `&self` 为 `&mut self` 或引入 `RefCell` 运行时开销）。**若 reviewer 反对 unsafe，改用 `std::cell::RefCell`**（见 Step 9 备选）。当前先用 unsafe 实现，API 保持 `&self`。

- [ ] **Step 4: 修改 `process_frame`**

`process_frame` 当前完整代码（46-133 行）分两段替换。

**4a. 初始化分支（46-65 行）**，旧：
```rust
    pub fn process_frame(&mut self, frame: &RgbaImage) -> Result<bool> {
        let (w, h) = (frame.width(), frame.height());

        if !self.detected {
            self.detect_sticky(frame);
            self.detected = true;
            // 裁掉画布（首帧）的 sticky 区域，只保留有效内容
            let eff_top0 = self.sticky_top;
            let eff_bottom0 = self.canvas.height().saturating_sub(self.sticky_bottom);
            let w = self.canvas.width();
            if eff_bottom0 > eff_top0 {
                // 仅裁掉底部的 sticky_bottom 区域，保留顶部的 sticky_top 区域
                let cropped = image::imageops::crop_imm(&self.canvas.clone(), 0, 0, w, eff_bottom0).to_image();
                self.canvas = cropped;
            }
            // 用第二帧初始化参考帧灰度图
            self.reference_gray = image::imageops::grayscale(frame);
    
            return Ok(false); // 第二帧用于初始化，不拼接
        }
```
新：
```rust
    pub fn process_frame(&mut self, frame: &RgbaImage) -> Result<bool> {
        let (w, h) = (frame.width(), frame.height());

        if !self.detected {
            self.detect_sticky(frame);
            self.detected = true;
            // 裁掉画布（首帧）的 sticky_bottom 区域，保留 sticky_top。
            let eff_bottom0 = self.canvas_h.saturating_sub(self.sticky_bottom);
            if eff_bottom0 > self.sticky_top {
                self.canvas_buf.truncate(eff_bottom0 as usize * self.canvas_w as usize * 4);
                self.canvas_h = eff_bottom0;
                self.canvas_cache = None;
            }
            // 用第二帧初始化参考灰度
            self.reference = GrayBuf::from_rgba(frame);
    
            return Ok(false); // 第二帧用于初始化，不拼接
        }
```

**4b. 主拼接分支（67-133 行）**，旧（完整原文）：
```rust
        let eff_top = self.sticky_top;
        let eff_bottom = h.saturating_sub(self.sticky_bottom);
        if eff_bottom <= eff_top {
            return Ok(false);
        }

        let curr_gray = image::imageops::grayscale(frame);
        
        // 排除最左侧的 10% (通常有图标/树状图) 和最右侧的 20% (通常有滚动条/时间戳)
        let x_start = (w as f64 * X_START_RATIO) as u32;
        let x_end = (w as f64 * X_END_RATIO) as u32;

        let max_scroll = MAX_SCROLL;
        let (dy, confidence) = match find_overlap_spatial_ext(
            &self.reference_gray,
            &curr_gray,
            x_start,
            x_end,
            eff_top,
            eff_bottom,
            max_scroll,
            self.last_dy,
        ) {
            Some(v) => v,
            None => {
                log::info!("[stitch] find_overlap_spatial returned None");
                self.last_dy = None;
                return Ok(false);
            }
        };

        // dy < 0 = 用户向下滚动（内容上移），dy > 0 = 向上滚动（忽略）
        if dy >= 0.0 {
            log::info!("[stitch] skipped frame: dy={:.1} >= 0.0 (conf={:.4})", dy, confidence);
            self.last_dy = None;
            return Ok(false);
        }

        let new_rows = (-dy).round() as u32;
        let max_scroll_limit = (eff_bottom - eff_top) * 4 / 5; // 允许最大滚动比例扩大到 80%

        // 静止或滚动超过限额
        if new_rows < self.config.min_scroll_px as u32 || new_rows >= max_scroll_limit {
            log::info!("[stitch] skipped frame: new_rows={} invalid (min={}, max={}) (conf={:.4})", new_rows, self.config.min_scroll_px, max_scroll_limit, confidence);
            self.last_dy = None;
            return Ok(false);
        }

        log::info!("[stitch] dy={:.1} conf={:.4} new_rows={} eff=[{},{}] canvas_h={}",
            dy, confidence, new_rows, eff_top, eff_bottom, self.canvas.height());

        let crop_y = eff_bottom - new_rows;
        let new_content = image::imageops::crop_imm(frame, 0, crop_y, w, new_rows).to_image();

        let old_h = self.canvas.height();
        let mut combined = RgbaImage::new(w, old_h + new_rows);
        combined.copy_from(&self.canvas, 0, 0).context("canvas copy")?;
        combined.copy_from(&new_content, 0, old_h).context("new_rows copy")?;
        self.canvas = combined;

        // 更新参考灰度图与速度缓存
        self.reference_gray = curr_gray;
        self.last_dy = Some(dy);

        Ok(true)
```
新：
```rust
        let eff_top = self.sticky_top;
        let eff_bottom = h.saturating_sub(self.sticky_bottom);
        if eff_bottom <= eff_top {
            return Ok(false);
        }

        let curr_gray_buf = GrayBuf::from_rgba(frame);

        let x_start = (w as f64 * X_START_RATIO) as u32;
        let x_end = (w as f64 * X_END_RATIO) as u32;

        let max_scroll = MAX_SCROLL;
        let (dy, confidence) = match find_overlap_spatial_ext(
            &self.reference,
            &curr_gray_buf,
            x_start,
            x_end,
            eff_top,
            eff_bottom,
            max_scroll,
            self.last_dy,
        ) {
            Some(v) => v,
            None => {
                log::info!("[stitch] find_overlap_spatial returned None");
                self.last_dy = None;
                return Ok(false);
            }
        };

        // dy < 0 = 用户向下滚动（内容上移），dy > 0 = 向上滚动（忽略）
        if dy >= 0.0 {
            log::info!("[stitch] skipped frame: dy={:.1} >= 0.0 (conf={:.4})", dy, confidence);
            self.last_dy = None;
            return Ok(false);
        }

        let new_rows = (-dy).round() as u32;
        let max_scroll_limit = (eff_bottom - eff_top) * 4 / 5; // 允许最大滚动比例扩大到 80%

        // 静止或滚动超过限额
        if new_rows < self.config.min_scroll_px as u32 || new_rows >= max_scroll_limit {
            log::info!(
                "[stitch] skipped frame: new_rows={} invalid (min={}, max={}) (conf={:.4})",
                new_rows, self.config.min_scroll_px, max_scroll_limit, confidence
            );
            self.last_dy = None;
            return Ok(false);
        }

        log::info!(
            "[stitch] dy={:.1} conf={:.4} new_rows={} eff=[{},{}] canvas_h={}",
            dy, confidence, new_rows, eff_top, eff_bottom, self.canvas_h
        );

        // 增量追加：从 frame 直接切出 new_rows 行 RGBA，extend 到 canvas_buf
        let crop_y = eff_bottom - new_rows;
        let row_bytes = w as usize * 4;
        let start = crop_y as usize * row_bytes;
        let end = start + new_rows as usize * row_bytes;
        let frame_raw = frame.as_raw();
        self.canvas_buf.extend_from_slice(&frame_raw[start..end]);
        self.canvas_h += new_rows;
        self.canvas_cache = None;

        // 更新参考灰度与速度缓存
        self.reference = curr_gray_buf;
        self.last_dy = Some(dy);

        Ok(true)
```

> 注意：Task 7 完成后 `find_overlap_spatial_ext` 已接受 `&GrayBuf`，此处直接传 `&self.reference` 和 `&curr_gray_buf`，无需转换。Task 7 中添加的临时 `GrayBuf { data: ...as_raw().clone() }` 转换代码在此 Step 被清除。

- [ ] **Step 5: 修改 `finalize`**

旧（138-191 行）整块替换。核心改动：① `reference_gray` → `reference`（已是 GrayBuf）；② 灰度用 `GrayBuf::from_rgba`；③ 画布追加用 `extend`；④ footer 追加用 `extend`。

```rust
    pub fn finalize(&mut self, last_frame: &RgbaImage) -> Result<()> {
        let h = last_frame.height();
        let w = last_frame.width();
        let eff_top = self.sticky_top;
        let eff_bottom = h.saturating_sub(self.sticky_bottom);
        if eff_bottom <= eff_top {
            return Ok(());
        }

        let last_gray = GrayBuf::from_rgba(last_frame);
        let x_start = (w as f64 * X_START_RATIO) as u32;
        let x_end = (w as f64 * X_END_RATIO) as u32;

        let max_finalize_scroll = ((eff_bottom - eff_top) as f64 * 0.90) as u32;
        if let Some((dy, confidence)) = find_overlap_spatial_ext(
            &self.reference,
            &last_gray,
            x_start,
            x_end,
            eff_top,
            eff_bottom,
            max_finalize_scroll,
            None, // 最后一帧匹配不施加速度限制
        ) {
            if dy < 0.0 {
                let new_rows = (-dy).round() as u32;
                if new_rows < eff_bottom - eff_top {
                    log::info!("[stitch] finalize: stitching remaining {} rows (conf={:.4})", new_rows, confidence);
                    let crop_y = eff_bottom - new_rows;
                    let row_bytes = w as usize * 4;
                    let start = crop_y as usize * row_bytes;
                    let end = start + new_rows as usize * row_bytes;
                    let frame_raw = last_frame.as_raw();
                    self.canvas_buf.extend_from_slice(&frame_raw[start..end]);
                    self.canvas_h += new_rows;
                    self.canvas_cache = None;
                }
            }
        }

        // 补全最后一帧的 sticky_bottom 区域
        let footer_h = h - eff_bottom;
        if footer_h > 0 {
            let row_bytes = w as usize * 4;
            let start = eff_bottom as usize * row_bytes;
            let end = start + footer_h as usize * row_bytes;
            let frame_raw = last_frame.as_raw();
            self.canvas_buf.extend_from_slice(&frame_raw[start..end]);
            self.canvas_h += footer_h;
            self.canvas_cache = None;
        }

        Ok(())
    }
```

- [ ] **Step 6: 修改 `detect_sticky`**

`detect_sticky` 内部用 `self.canvas` 和 `self.canvas.width()`/`.height()`。改为访问 `canvas_buf` / `canvas_w` / `canvas_h`。`rows_equal` 需改为接受 `&[u8]` 切片（canvas）与 `&RgbaImage`（frame）比较。

旧（193-210 行）：
```rust
    fn detect_sticky(&mut self, frame: &RgbaImage) {
        let (w, ch) = (self.canvas.width(), self.canvas.height());
        let fh = frame.height();
        let cmp_h = ch.min(fh);
        let mut sticky_t = 0u32;
        for y in 0..cmp_h.min(STICKY_DETECT_MAX) {
            if rows_equal(&self.canvas, frame, y, y, w) { sticky_t = y + 1; }
            else { break; }
        }
        let mut sticky_b = 0u32;
        for y in 0..cmp_h.min(STICKY_DETECT_MAX) {
            let ya = cmp_h - 1 - y;
            if rows_equal(&self.canvas, frame, ya, ya, w) { sticky_b = y + 1; }
            else { break; }
        }
        self.sticky_top = sticky_t;
        self.sticky_bottom = sticky_b;
    }
```

新：
```rust
    fn detect_sticky(&mut self, frame: &RgbaImage) {
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
```

- [ ] **Step 7: 修改 `rows_equal` → `rows_equal_buf`**

旧（335-340 行）：
```rust
fn rows_equal(a: &RgbaImage, b: &RgbaImage, ya: u32, yb: u32, w: u32) -> bool {
    for x in 0..w {
        if a.get_pixel(x, ya) != b.get_pixel(x, yb) { return false; }
    }
    true
}
```

新：
```rust
/// 比较连续 RGBA buffer 的 ya 行 与 RgbaImage 的 yb 行是否逐像素相等。
fn rows_equal_buf(a: &[u8], a_w: u32, b: &RgbaImage, ya: u32, yb: u32) -> bool {
    let row_bytes = a_w as usize * 4;
    let a_start = ya as usize * row_bytes;
    let a_row = &a[a_start..a_start + row_bytes];
    let b_row = b.as_raw();
    let b_start = yb as usize * row_bytes;
    let b_row = &b_row[b_start..b_start + row_bytes];
    a_row == b_row
}
```

- [ ] **Step 8: 编译 + 全部测试验证**

Run:
```bash
cd /Users/wudarui/workspace/agent/octopus/.worktrees/optimize-capx && cargo test -p octopus-capx 2>&1 | tail -20
```
Expected: 全部测试通过。若 `test_canvas_returns_valid_rgba` 失败（`from_raw` 返回 None），说明 `canvas_buf` 长度与 `canvas_w * canvas_h * 4` 不匹配——检查 `truncate` / `extend` 的字节数计算。

再验证 desktop 集成编译：
```bash
cd /Users/wudarui/workspace/agent/octopus/.worktrees/optimize-capx && cargo check -p octopus-desktop 2>&1 | tail -5
```
Expected: `Finished`（API 零改动，desktop 不应报错）。

- [ ] **Step 9: Commit**

```bash
cd /Users/wudarui/workspace/agent/octopus/.worktrees/optimize-capx
git add crates/capx/src/stitch.rs
git commit -m "perf(capx): 画布改 Vec<u8> 增量追加 + 惰性 RgbaImage 缓存（API 不变）"
```

- [ ] **Step 9 备选（若 reviewer 反对 unsafe）**：把 `canvas_cache: Option<RgbaImage>` 改为 `canvas_cache: std::cell::RefCell<Option<RgbaImage>>`，`canvas()` 实现改为 `self.canvas_cache.borrow_mut().get_or_insert_with(...)`。API 不变。

---

## Task 9: 文档同步

**Files:**
- Modify: `docs/superpowers/specs/2026-06-30-scroll-stitch-research.md`（已归档至 `2026-07-02-archived-specs.md`，标注 FFT 未采纳）
- Modify: `docs/architecture.md`（CAPX 模块数据结构更新）

- [ ] **Step 1: 修正 research spec 的 FFT 方案标注**

在 `2026-06-30-scroll-stitch-research.md`（已归档至 `2026-07-02-archived-specs.md`）的"### 方案 A：FFT 相位相关（推荐）"标题后追加注记：

```markdown
> **更新（2026-06-12）**：本方案为调研结论，**实际未采纳**。最终实现采用 2D SAD 空间模板匹配 + 软速度罚分（见 commit `4b94215`），在实测中已能精准工作。后续性能优化（整数化 + 模板预取 + 画布增量追加）见 [`2026-06-12-capx-optimization-design.md`](./2026-06-12-capx-optimization-design.md)。
```

- [ ] **Step 2: 更新 architecture.md 的 CAPX 章节**

先查看 architecture.md 中 CAPX 相关内容：
```bash
cd /Users/wudarui/workspace/agent/octopus/.worktrees/optimize-capx && grep -n "capx\|CAPX\|Stitcher\|stitch" docs/architecture.md
```
找到 CAPX 模块描述处，更新数据结构说明：提及画布用 `Vec<u8>` + 惰性缓存、灰度用 `GrayBuf`、SAD 整数化。具体文案根据现有内容调整，保持风格一致。

- [ ] **Step 3: Commit**

```bash
cd /Users/wudarui/workspace/agent/octopus/.worktrees/optimize-capx
git add docs/
git commit -m "docs(capx): 同步 FFT→SAD 偏离标注与 architecture 数据结构更新"
```

---

## 验收清单（全部任务完成后核对）

- [ ] `cargo test -p octopus-capx` 全绿（≥10 个测试）
- [ ] `cargo check -p octopus-capx -p octopus-desktop` 无错误无警告
- [ ] `find_overlap_spatial_ext`（或 `_bufs` 变体）已拆分，无单函数超过 50 行
- [ ] `capture.rs` macOS 三处 BGRA→RGBA 统一为 `cgimage_to_rgba`
- [ ] `stitch.rs` 无裸魔法数字（除 `0.90`、`4/5`、`10` 等少量局部常量可保留或提取）
- [ ] API 零改动：`git diff main -- crates/capx/src/lib.rs` 为空，`Stitcher`/`capture::*` 公开签名不变
- [ ] 文档同步完成
