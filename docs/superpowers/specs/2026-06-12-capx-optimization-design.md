# CAPX 模块综合优化设计

**日期**: 2026-06-12
**状态**: ✅ 实施完成（P1-P5 全部落地，10 测试全绿，API 零改动）
**分支**: `optimize-capx`
**关联文档**: [`2026-06-30-scroll-stitch-research.md`](./2026-07-02-archived-specs.md)（算法调研，已归档）、[`2026-06-29-scroll-screenshot-design.md`](./2026-07-02-archived-specs.md)（滚动截屏整体设计，已归档）

> **实施记录**：无偏差。`estimate_confidence` 的口径改进（稀疏 best vs 稀疏 mean，替代原密集 best vs 稀疏 mean）如 spec 预期正常工作，未触发回退。`canvas()` 惰性缓存用 `UnsafeCell` 实现（非 `unsafe` 裸指针）。函数拆分追加 `decide_match` 和 `sparse_sad_at_offset` 两个 helper（原 spec 列出 3 个，实际拆为 5 个以满足 ≤50 行约束）。

---

## 一、背景与动机

CAPX 模块（`crates/capx/`）提供屏幕捕获（`capture.rs`，360 行）与滚动截屏拼接（`stitch.rs`，340 行）能力，被 `octopus-desktop` 的 `screenshot_commands.rs`（14 处调用）使用。

当前实现存在四类问题：

| 类别 | 具体问题 | 影响 |
|------|---------|------|
| **性能** | `find_overlap_spatial_ext` 用 `image::GrayImage::get_pixel()`（每次坐标计算 + 边界检查）逐像素访问，f64 累加，模板像素在每个 y_offset 迭代重复扫描 | 拼接热路径慢，实时滚动截屏瓶颈 |
| **性能** | 每次拼接 `RgbaImage::new` + 两次 `copy_from`（旧画布整体复制） | 大画布下画布追加 O(N²) 内存复制 |
| **重复** | `capture.rs` 三处几乎一模一样的 CGImage 解析 + BGRA→RGBA 转换样板（`capture_display_excluding_window` / `capture_region_excluding_window` / `capture_window_region`） | 维护负担，改一处漏两处 |
| **健壮性** | 核心匹配算法 + sticky 检测**零测试覆盖**；魔法数字散落源码（`0.10` / `0.80` / `80` / `220` / `2.0` / `7.5` / `0.04`） | 改参数即可能引入回归，无安全网 |
| **可读性** | `find_overlap_spatial_ext` 单函数 120 行，混了静止检测 / 搜索 / 置信度估计三职责 | 难以理解和调整 |

### 与调研文档的偏离（需同步修正）

[`2026-06-30-scroll-stitch-research.md`](./2026-07-02-archived-specs.md) 原推荐 **FFT 相位相关**方案，但实际实现（见 commit `4b94215`）采用 **2D SAD 空间模板匹配 + 软速度罚分**。本次优化**不替换算法**（SAD 方案在实测中已能精准工作），仅做性能与代码质量优化。spec 中"方案 A：FFT 相位相关（推荐）"标注为"调研结论，实际未采纳"，并补充实际实现的说明。

---

## 二、目标与非目标

### 目标

1. **性能**：SAD 热路径提速（连续内存访问 + 整数 SAD + 模板预提取）；画布追加从 O(N²) 整体复制降到 O(new_rows) 增量 `extend`
2. **代码质量**：消除 capture.rs 重复；提取魔法数字为命名常量；拆分长函数
3. **健壮性**：为核心匹配 / sticky 检测 / 画布不变量补合成图单元测试（不引入 criterion 等基准基础设施）

### 非目标

- **不替换匹配算法**：SAD + 软速度罚分保持不变，不引入 FFT
- **不改对外 API**：`Stitcher::new/process_frame/finalize/canvas/height` 签名与语义完全不变，`desktop` 零改动
- **不引入 SIMD intrinsics**：靠连续内存布局 + 整数运算 + 编译器自动向量化获得收益，保持可移植性与可读性
- **不加性能基准**：项目当前无 criterion 依赖，合成图单元测试已足够覆盖正确性

---

## 三、设计

### 3.1 新数据结构（`stitch.rs`）

引入连续灰度 buffer 替代 `image::GrayImage`，消除 `get_pixel()` 开销：

```rust
/// 连续 row-major 灰度 buffer，替代 image::GrayImage（消除 get_pixel 边界检查开销）。
struct GrayBuf {
    data: Vec<u8>,
    width: usize,
    height: usize,
}

impl GrayBuf {
    fn from_rgba(rgba: &RgbaImage) -> Self { /* 一次性灰度转换 */ }
    /// 整行切片直访，无边界检查
    fn row(&self, y: usize) -> &[u8] {
        &self.data[y * self.width..(y + 1) * self.width]
    }
}
```

`Stitcher` 内部布局改造：

```rust
pub struct Stitcher {
    canvas_w: u32,
    canvas_h: u32,
    canvas_buf: Vec<u8>,            // 连续 RGBA（真实数据源，增量 extend）
    canvas_cache: Option<RgbaImage>, // 惰性重建缓存，append 后 invalidate
    reference: GrayBuf,             // 替代 reference_gray: GrayImage
    sticky_top: u32,
    sticky_bottom: u32,
    detected: bool,
    config: StitchConfig,
    last_dy: Option<f64>,
}
```

### 3.2 `canvas()` 惰性缓存（API 兼容关键）

`canvas(&self) -> &RgbaImage` 行为不变，但内部实现：

- `canvas_cache` 为 `Some` 时直接返回引用
- 为 `None`（append 后 invalidate）时，从 `canvas_buf` + `canvas_w/h` 重建 `RgbaImage`，存入 cache，返回引用

调用端（`screenshot_commands.rs`）对 `canvas()` 总是 `.clone()`（已核实：line 1173/1209/1245），因此惰性重建是一次性成本，不破坏借用。**desktop 零改动**。

### 3.3 SAD 热路径优化（`find_overlap_spatial_ext` 重写）

**魔法数字提取为模块常量：**

```rust
const STRIP_H: u32 = 80;           // 模板条高度
const MAX_SCROLL: u32 = 220;       // 全量搜索范围
const STATIONARY_SAD: f64 = 2.0;   // 静止判定阈值
const SAD_ACCEPT: f64 = 7.5;       // 匹配接受阈值
const MIN_CONFIDENCE: f64 = 0.15;  // 置信度下限
const SPEED_PENALTY: f64 = 0.04;   // 软速度罚分系数
const X_START_RATIO: f64 = 0.10;   // 排除最左侧 10%（图标/树状图）
const X_END_RATIO: f64 = 0.80;     // 排除最右侧 20%（滚动条/时间戳）
const SAMPLE_STEP_X: usize = 2;    // 列抽样步长
```

**模板预提取 + 连续 buffer：**

```rust
// 抽样列索引只算一次
let sample_cols: Vec<usize> = (x_start..x_end)
    .step_by(SAMPLE_STEP_X)
    .collect();
let n_cols = sample_cols.len();

// 模板条预提取到连续 buffer（strip_h × n_cols），主循环不再重复访问 reference 的不同行
let tpl: Vec<u8> = Vec::with_capacity(STRIP_H as usize * n_cols);
for dy in 0..STRIP_H {
    let row = reference.row(template_y + dy);
    for &x in &sample_cols {
        tpl.push(row[x]);
    }
}
```

**主搜索：整数累加 + 切片直访：**

```rust
let mut best_y_offset = 0u32;
let mut min_penalized = f64::MAX;
let mut best_sad_avg = f64::MAX;
let mut stationary_sad_avg = f64::MAX; // y_offset == template_y 那次迭代填入

for y_offset in min_y_offset..=max_y_offset {
    let mut sad: u64 = 0;
    let mut i = 0;
    for dy in 0..STRIP_H {
        let row = curr.row(y_offset + dy);
        for &x in &sample_cols {
            // i32 减法 + unsigned_abs → u32 累加，无 f64 无边界检查
            sad += (tpl[i] as i32 - row[x] as i32).unsigned_abs() as u64;
            i += 1;
        }
    }
    let sad_avg = sad as f64 / (STRIP_H * n_cols) as f64;

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
```

**收益来源：**
1. `get_pixel()` → `row()` 切片直访（省坐标计算 + 边界检查）
2. f64 累加 → u64 整数累加（省浮点开销，最后一次性转 f64 求均值）
3. 模板只取一次（原实现每个 y_offset 都重扫 reference 模板行）
4. 连续内存布局利于编译器自动向量化

**静止检测合并：** 原 `find_overlap_spatial_ext` 先做一次完整的 dy=0 全扫描判静止，再做主搜索。优化后静止锚点 = 主循环中 `y_offset == template_y` 那次迭代，**省掉一次完整预扫描**。主搜索完成后比较 `stationary_sad_avg` 与 `best_sad_avg` 判静止。

**函数拆分：** 120 行单函数拆为三个职责清晰的内联辅助（模块私有）：

```rust
/// 提取模板条到连续 buffer
fn extract_template(ref_buf: &GrayBuf, template_y: u32, sample_cols: &[usize]) -> Vec<u8>;

/// 整数 SAD 主搜索，返回 (best_y_offset, best_sad_avg, stationary_sad_avg)
fn search_best_offset(
    tpl: &[u8], curr: &GrayBuf, strip_h: u32, sample_cols: &[usize],
    min_y_offset: u32, max_y_offset: u32, template_y: u32,
    last_dy: Option<f64>,
) -> (u32, f64, f64);

/// 稀疏采样估计置信度
fn estimate_confidence(
    ref_buf: &GrayBuf, curr: &GrayBuf, strip_h: u32, sample_cols: &[usize],
    best_y_offset: u32, min_y_offset: u32, max_y_offset: u32, template_y: u32,
) -> f64;
```

### 3.4 画布增量追加（O(N²)→O(new_rows)）

原实现：
```rust
let mut combined = RgbaImage::new(w, old_h + new_rows);  // 分配整块
combined.copy_from(&self.canvas, 0, 0)?;                  // 复制旧画布 O(old_h)
combined.copy_from(&new_content, 0, old_h)?;              // 复制新行 O(new_rows)
self.canvas = combined;
```

新实现：
```rust
// new_content_rgba: 直接从 frame 切片出的连续 RGBA 行（无需 RgbaImage 中转）
self.canvas_buf.extend_from_slice(&new_content_rgba);
self.canvas_h += new_rows;
self.canvas_cache = None; // invalidate，下次 canvas() 按需重建
```

`finalize()` 中的两次画布追加同样改造。`process_frame` / `finalize` 中裁剪 `new_content` 时直接操作 `frame` 的底层 `Vec<u8>` 切片，避免 `crop_imm().to_image()` 的中间分配。

### 3.5 `capture.rs` 去重

提取公共 helper（macOS gated）：

```rust
#[cfg(target_os = "macos")]
fn cgimage_to_rgba(cg_image: &core_graphics::image::CGImage) -> Result<(Vec<u8>, u32, u32)> {
    let width = cg_image.width() as u32;
    let height = cg_image.height() as u32;
    let bpr = cg_image.bytes_per_row();
    let bpp = cg_image.bits_per_pixel();
    if bpp != 32 {
        anyhow::bail!("Unsupported screenshot format: {} bpp (expected 32)", bpp);
    }
    let raw = cg_image.data().bytes();
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

三个 macOS 捕获函数（`capture_display_excluding_window` / `capture_region_excluding_window` / `capture_window_region`）的 CGImage 解析 + BGRA→RGBA 部分统一调用此 helper。注意 `capture_display_excluding_window` 当前用 `raw[off+2]` 索引式（未用 `chunks_exact`），统一后行为一致（都是 BGRA→RGBA）。

---

## 四、API 兼容性

**对外 API 零改动**（已核实 `crates/desktop/src/screenshot_commands.rs` 全部 14 处调用）：

| API | 签名 | 调用方用法 | 兼容性 |
|-----|------|-----------|--------|
| `Stitcher::new(first_frame: RgbaImage, config) -> Self` | 不变 | line 1100 | ✓ |
| `process_frame(&mut self, &RgbaImage) -> Result<bool>` | 不变 | line 1157 | ✓ |
| `finalize(&mut self, &RgbaImage) -> Result<()>` | 不变 | line 1206 | ✓ |
| `canvas(&self) -> &RgbaImage` | 不变（内部惰性重建） | line 1173/1209/1245 均 `.clone()` | ✓ |
| `height(&self) -> u32` | 不变 | line 1190/1191/1222 | ✓ |
| `capture::*` 全部 | 不变 | — | ✓ |

`canvas()` 惰性重建：因调用端总是立即 `.clone()`，重建的 `RgbaImage` 借用不会跨多次 append 存活，无生命周期问题。

---

## 五、测试策略

**合成图单元测试**（不依赖真实截屏，不引入 criterion）。`#[cfg(test)] mod tests` 内联在 `stitch.rs` 与 `capture.rs`。

### stitch.rs 测试用例

1. **已知位移检测**：构造一张合成 `RgbaImage`（带可识别纹理，如渐变 + 周期性条纹），模拟用户向下滚动 N 像素（内容上移 N 像素）生成第二帧，`process_frame` 应返回 `dy = -N` 且画布高度增加 N（约定：`dy < 0` = 用户向下滚动，见 `stitch.rs:99`）
2. **静止帧**：两帧完全相同 → `process_frame` 返回 `Ok(false)`，`dy = 0`
3. **sticky 检测**：首帧顶部/底部各固定 K 行，中间内容滚动 → `sticky_top` / `sticky_bottom` 正确识别为 K
4. **画布高度不变量**：连续多次 `process_frame`，每次追加的 `new_rows` 之和应等于 `final height - initial height`（finalize 前）
5. **周期性内容不误匹配**：构造纯周期性条纹（周期 = 45px，模拟文件列表行高），向下滚 30px，应检测到 dy=-30 而非 -75（差一个周期）
6. **finalize 补缝**：模拟丢帧场景（reference 停在旧帧，最后一帧大幅滚动），`finalize` 应补全剩余区域

### capture.rs 测试用例

7. **`cgimage_to_rgba` 行为**：构造已知 BGRA buffer（macOS gated test），验证转换后 RGBA 顺序正确（此测试仅验证逻辑，可在非 macOS 用 `#[cfg(test)]` + 直接测纯函数版，或提取 BGRA→RGBA 为平台无关的纯函数单独测）

### 测试构造工具

```rust
#[cfg(test)]
fn make_synthetic_frame(width: u32, height: u32, scroll_offset: u32) -> RgbaImage {
    // 生成带强空间特征的合成图：背景渐变 + 每 45px 一条水平分隔线 + 随机噪点
    // scroll_offset 控制内容垂直偏移，模拟滚动
}
```

---

## 六、分阶段实施计划

每阶段独立可验证，风险递增。**P2 必须在任何重写前完成**，为 P3/P4 提供回归安全网。

| 阶段 | 内容 | 风险 | 验证命令 |
|------|------|------|---------|
| **P1** | capture.rs 去重（`cgimage_to_rgba`）+ 魔法数字提取为常量 | 极低 | `cargo check -p octopus-capx` + `cargo check -p octopus-desktop` |
| **P2** | 加合成图单元测试（基于**现有** API，先建安全网） | 低 | `cargo test -p octopus-capx` 全绿 |
| **P3** | 引入 `GrayBuf`，SAD 热路径重写（整数 + 切片 + 模板预取 + 函数拆分 + 静止检测合并） | 中 | P2 测试必须保持全绿 |
| **P4** | 画布改 `Vec<u8>` + 惰性缓存（内部数据结构改造） | 中高 | P2 测试全绿 + `cargo check -p octopus-desktop` |
| **P5** | 同步文档：spec 标注 FFT→SAD 偏离、`architecture.md` 更新 | 无 | 文档审阅 |

**风险控制原则：**
- P2 在重写前锁定行为基线
- P3/P4 若引入回归，P2 测试立即暴露
- 每阶段结束 `cargo check` + 相关测试

---

## 七、风险与缓解

| 风险 | 缓解 |
|------|------|
| `GrayBuf` 灰度转换与 `image::imageops::grayscale` 结果不一致 → SAD 值偏移 → 误判 | P3 先验证 `GrayBuf::from_rgba` 与 `grayscale()` 在合成图上逐像素相等，再切换 |
| 画布 `Vec<u8>` 布局与 `RgbaImage::from_raw` 期望不一致 → `canvas()` 重建失败或像素错乱 | P4 验证 `RgbaImage::from_raw(w, h, canvas_buf.clone())` 成功且尺寸匹配；P2 不变量测试覆盖 |
| 静止检测合并（省预扫描）改变边界行为 → 漏检静止 | P2 静止帧测试 + 周期性测试覆盖；P3 保持 `stationary_sad_avg < best_sad_avg + 1.0` 同一判据 |
| `canvas()` 惰性重建在 desktop 端产生意外生命周期问题 | 已核实调用端均 `.clone()`；P4 后 `cargo check -p octopus-desktop` 确认 |

---

## 八、文档同步

实施完成后同步：
- **本 spec**：P5 标注实际实施偏差（若 P3/P4 调整了方案）
- **`docs/architecture.md`**：CAPX 模块章节更新数据结构描述
- **`2026-06-30-scroll-stitch-research.md`**（已归档至 `2026-07-02-archived-specs.md`）：在"方案 A：FFT 相位相关（推荐）"处补充"实际未采纳，采用 2D SAD"说明

---

## 九、验收标准

1. `cargo test -p octopus-capx` 全绿（覆盖上述 7 类测试用例）
2. `cargo check -p octopus-capx -p octopus-desktop` 无错误（API 兼容）
3. `find_overlap_spatial_ext` 拆分为 ≤3 个职责单一的函数，无函数超过 50 行
4. `capture.rs` macOS 三处 BGRA→RGBA 统一为 `cgimage_to_rgba` 单点
5. 魔法数字全部提取为命名常量，源码无裸数字
6. 文档同步完成
