# NCC + Sobel 梯度匹配引擎重写

**日期**: 2026-07-02
**状态**: ✅ 实施完成（NCC + Sobel 匹配落地，19 测试全绿，API 零改动）
**分支**: `optimize-capx`
**关联文档**: [`2026-07-02-capx-canvas-anchored-design.md`](./2026-07-02-capx-canvas-anchored-design.md)、[`2026-06-30-scroll-stitch-research.md`](./2026-06-30-scroll-stitch-research.md)

---

## 一、背景与根因

经过多轮 SAD + 灰度框架下的调参（动态阈值、三级降级链、best-guess、亚像素插值），拼接质量仍不稳定。核心问题是 **SAD 在周期性内容（文件列表、表格线）中产生多峰假匹配**，而我们的置信度/阈值/best-guess 机制本质是在补这个算法缺陷的洞。

### 业界对照

Scrollshot（Rust 开源滚动截屏）的源码级分析揭示了一条成熟的匹配管线：

| 维度 | 我们（SAD + 灰度） | Scrollshot（NCC + Sobel） |
|------|-------------------|---------------------------|
| 特征源 | 原始灰度值 | Sobel 边缘梯度（对渲染差异免疫） |
| 匹配准则 | 手写整数 SAD | `imageproc::template_matching::match_template`（NCC） |
| 周期内容 | 多峰 → 需要置信度/降级/best-guess 补丁 | NCC 峰更锐利，自然区分真假匹配 |
| 模板 | 固定 80px | 5 种高度并行（`{1,2,3,5,8} × min_overlap`） |
| 验证 | 单一 conf > 0.5 | 5 道独立检查 |

### 为什么 NCC 更好

SAD 在"差一个周期"的位置 SAD 差异很小（都是 ~20），而 NCC 在正确位置给出 0.95+，错误位置给出 0.3——数学上的归一化天然区分真假匹配。

### 为什么 Sobel 梯度更好

原始灰度值受抗锯齿、Retina 子像素渲染、JPEG 压缩影响。Sobel 梯度只保留结构性边缘特征，对这些像素级差异免疫。

---

## 二、目标与非目标

### 目标

1. **替换匹配核心**：用 `imageproc` 的 NCC + Sobel 梯度替代手写 SAD + 灰度
2. **保留 Canvas-Anchored 架构**：每帧从画布底部提取模板
3. **保留健壮性设计**：dy_history 时序平滑、best-guess 熔断（但简化——NCC 更准后大部分降级可移除）
4. **API 零改动**

### 非目标

- 不做多模板并行（rayon）——保持单模板但高度自适应
- 不做文本主体检测（Otsu/墨水密度）——当前固定 10%/80% 裁剪已够用
- 不做滚动条排除——固定裁剪覆盖
- 不替换抛物线插值——我们的实现比 Scrollshot 的更完整

---

## 三、设计

### 3.1 匹配管线

```
Canvas-Anchored（画布底部 80px strip）
  → Sobel 梯度（imageproc::gradients::sobel_gradients）
  → 归一化（mean + 3σ，纯色退化）
  ↓
当前帧 ROI 灰度
  → Sobel 梯度
  → 归一化（同上）
  ↓
imageproc NCC 模板匹配（CrossCorrelationNormalized）
  → 最佳 y 偏移 + NCC 分数
  → 抛物线亚像素插值（已有实现）
  → 多道验证（分数 + 局部 delta + 全局 delta）
```

### 3.2 特征图生成（Sobel + 归一化 + 纯色退化）

```rust
use imageproc::gradients::sobel_gradients;
use imageproc::stats::histogram;

/// 将 GrayBuf 转为 Sobel 梯度特征图 + 归一化。
/// 纯色区域（max_gradient=0）返回 (空白, false)，调用方退回灰度。
fn to_feature_map(gray: &GrayBuf) -> (GrayImage, bool) {
    let luma_img = gray.to_gray_image();  // GrayBuf → image::GrayImage
    let gradients = sobel_gradients(&luma_img);

    let max_gradient = gradients.iter().map(|p| p[0]).max().unwrap_or(0);
    if max_gradient == 0 {
        return (GrayImage::new(luma_img.width(), luma_img.height()), false);
    }

    // 归一化：mean + 3σ
    let mean = mean_of(&gradients);
    let stddev = stddev_of(&gradients, mean);
    let normalizer = (mean + 3.0 * stddev).max(1.0);

    let normalized = GrayImage::from_fn(gradients.width(), gradients.height(), |x, y| {
        let g = gradients.get_pixel(x, y)[0] as f32;
        let scaled = (g / normalizer) * 255.0;
        image::Luma([scaled.round().clamp(0.0, 255.0) as u8])
    });
    (normalized, true)
}
```

### 3.3 NCC 匹配

```rust
use imageproc::template_matching::{match_template, find_extremes, MatchTemplateMethod};

/// NCC 匹配：在搜索区域中找模板的最佳对齐位置。
/// 返回 (best_y_offset, ncc_score, response_map)
fn ncc_match(
    template: &GrayImage,  // 画布底部 strip 的特征图（模板）
    search_region: &GrayImage,  // 当前帧 ROI 的特征图（搜索区域）
) -> (f64, f64, ImageBuffer<Luma<f32>, Vec<f32>>) {
    let response = match_template(
        search_region,
        template,
        MatchTemplateMethod::CrossCorrelationNormalized,
    );
    let extremes = find_extremes(&response);
    let best_y = extremes.max_value_location.1 as f64;
    let best_score = extremes.max_value as f64;
    (best_y, best_score, response)
}
```

### 3.4 多道验证（替代单一 conf > 0.5）

Scrollshot 的 5 道验证，我们精简为 3 道：

```rust
const NCC_SCORE_THRESHOLD: f32 = 0.75;       // 最低 NCC 分数
const LOCAL_CONFIDENCE_DELTA: f32 = 0.005;    // best vs 次优差值
const GLOBAL_CONFIDENCE_DELTA: f32 = 0.002;   // best vs 远处差值（≥4px）

fn validate_match(
    response: &ImageBuffer<Luma<f32>, Vec<f32>>,
    best_y: usize,
    best_score: f32,
) -> bool {
    // 1. 最低分数
    if best_score < NCC_SCORE_THRESHOLD { return false; }

    // 2. 局部置信度：best vs best±1 的最大值差
    let local_alt = max_adjacent(response, best_y);
    if best_score - local_alt < LOCAL_CONFIDENCE_DELTA { return false; }

    // 3. 全局置信度：best vs 距离≥4px 的最大值差
    let distant_alt = max_distant(response, best_y, 4);
    if best_score - distant_alt < GLOBAL_CONFIDENCE_DELTA { return false; }

    true
}
```

### 3.5 process_frame 核心流程

```rust
pub fn process_frame(&mut self, frame: &RgbaImage) -> Result<bool> {
    // ... 宽度校验、eff_top/eff_bottom 计算（不变）...

    // 1. Canvas-Anchored：从画布底部提取 strip
    let canvas_gray = self.extract_canvas_bottom_gray(STRIP_H);
    let canvas_ref_map = to_feature_map(&canvas_gray);

    // 2. 当前帧 ROI 灰度 + 特征图
    let roi_top = ...;
    let curr_gray = GrayBuf::from_rgba_roi(frame, roi_top, eff_bottom);
    let curr_map = to_feature_map(&curr_gray);

    // 3. 纯色退化：任一帧无特征 → 回退灰度
    let (template, search_region) = if canvas_ref_map.1 && curr_map.1 {
        (canvas_ref_map.0, curr_map.0)
    } else {
        (canvas_gray.to_gray_image(), curr_gray.to_gray_image())
    };

    // 4. NCC 匹配
    let (best_y, ncc_score, response) = ncc_match(&template, &search_region);

    // 5. 多道验证
    if !validate_match(&response, best_y as usize, ncc_score as f32) {
        // 降级链（简化为 best-guess only）
        return self.try_best_guess(frame, ...);
    }

    // 6. 抛物线亚像素插值（已有实现，复用 response map）
    let refined_y = parabolic_refine(&response, best_y);

    // 7. 追加 + 状态更新（dy_history 等，不变）
    ...
}
```

### 3.6 降级链简化

NCC + Sobel 更准后，大幅简化降级链：

- **移除降级 1**（扩大搜索范围）：NCC 失败通常意味着真的没对齐内容
- **移除降级 2**（缩小模板）：固定模板 + 纯色退化已覆盖
- **保留降级 3**（1D 投影）：作为最后的图像匹配尝试
- **保留降级 4**（best-guess）：带熔断的历史估算

### 3.7 GrayBuf 增强

```rust
impl GrayBuf {
    /// 转为 image::GrayImage（供 imageproc 使用）
    fn to_gray_image(&self) -> image::GrayImage {
        image::GrayImage::from_raw(self.width as u32, (self.data.len() / self.width) as u32, self.data.clone())
            .expect("GrayBuf → GrayImage 失败")
    }
}
```

### 3.8 移除的代码

- `search_best_offset`（整数 SAD 主搜索）→ 替换为 `ncc_match`
- `extract_template`（模板预提取）→ NCC 直接用 GrayImage
- `estimate_confidence`（稀疏采样置信度）→ 替换为多道验证
- `sparse_sad_at_offset` → 删除
- `estimate_texture_density`（纹理密度评估）→ Sobel 梯度天然提供
- `dynamic_sad_accept`（动态 SAD 阈值）→ NCC 固定阈值 0.75
- `SAD_ACCEPT`、`MIN_CONFIDENCE`、`SPEED_PENALTY` 等常量 → 删除或替换

### 3.9 保留的代码

- Canvas-Anchored 架构（`extract_canvas_bottom_gray`）
- `dy_history` 时序平滑 + `is_stationary`
- `estimate_dy_hint` + best-guess 熔断
- `try_match_1d_projection`（降级 3）
- 抛物线插值（`parabolic_refine`，从 response map 提取 ±1 分数拟合）
- ROI 灰度转换（`from_rgba_roi` + `y_offset`）
- 画布 `Vec<u8>` + 惰性缓存
- `quick_stationary_check`（best-guess 前静止检测）

---

## 四、依赖

`Cargo.toml` 已有 `imageproc = "0.25"`。需确认：
- `imageproc::template_matching::match_template` — ✅ 0.25 有
- `imageproc::gradients::sobel_gradients` — ✅ 0.25 有
- `imageproc::definitions::Image`（response map 类型）— ✅

可能需要升级 `imageproc` 到 `0.26`（Scrollshot 用的版本）以获得最新 API。

---

## 五、API 兼容性

对外 API 零改动。所有替换在私有函数内部。

---

## 六、测试策略

### 现有 18 测试

- 合成图测试（`make_frame`）必须保持全绿
- 注意：`make_frame` 生成的是灰度渐变 + 周期条纹，Sobel 梯度对其的特征提取可能与原始灰度不同——需要验证 NCC 在这些合成图上也能正确匹配

### 新增测试

1. **Sobel 特征图生成**：纯色输入返回 `(blank, false)`，正常输入返回有特征的图
2. **NCC 匹配精度**：已知位移的合成帧，NCC 应返回正确偏移 + 高分数
3. **纯色退化**：两帧纯色输入，匹配应回退灰度
4. **多道验证**：构造低 NCC 分数 / 低 delta 的响应图，验证被拒绝

---

## 七、风险与缓解

| 风险 | 缓解 |
|------|------|
| `imageproc::match_template` 计算量大于手写 SAD | NCC 用 FFT 优化（imageproc 内部），且我们只搜索单列（垂直一维），实际计算量可控。若超 30ms 可降采样 |
| Sobel 预处理增加每帧开销 | `sobel_gradients` 是 O(W×H) 的简单卷积，比 SAD 主搜索本身快 |
| NCC 在我们的合成测试帧上表现不同 | 先验证现有 18 测试全绿，失败则调整 `make_frame` 特征密度 |
| `imageproc` 0.25 vs 0.26 API 差异 | 先 check 0.25，不够再升级 |

---

## 八、验收标准

1. `cargo test -p octopus-capx` 全绿（现有 + 新增）
2. `cargo check -p octopus-desktop` 无错误
3. API 零改动
4. **e2e 实测**：滚动截屏在文件列表、代码编辑器、网页（含纯色区域）场景下无重复/丢内容/断裂（需人工实测确认后才同步 main）
