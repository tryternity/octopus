# 滚动截屏拼接技术调研方案

**日期**: 2026-06-30
**状态**: 调研完成，待实施
**分支**: `feature/clipboard-research`

---

## 一、问题诊断：当前实现为何重叠与丢帧

### 1.1 当前算法回顾

当前 `stitch.rs` 采用**底部 strip NCC（归一化互相关）模板匹配**：

1. 从上一帧 edges 底部取一个 strip（≈20% 选区高度）作为模板
2. 在当前帧中滑动搜索最佳 NCC 匹配位置
3. 从匹配位置之后裁剪新内容追加到画布

### 1.2 三个结构性缺陷

| 缺陷 | 根因 | 表现 |
|---|---|---|
| **亚像素精度缺失** | NCC 按整数像素滑窗，真实滚动位移往往是 12.7px、38.3px 等非整数 | 每帧 0.3-0.7px 累积误差 → 文字行逐渐模糊 |
| **模板条纹太窄** | `template_ratio=0.20` → 模板仅 ~100px 高。在周期性列表（如文件列表每行 45px）中，100px 模板在 d、d±45 处得分接近 | 匹配跳到隔壁行 → 行重复/错位 |
| **帧间比较而非帧-画布比较** | 当前是 `last_frame` vs `curr_frame`。如果某帧被丢弃（静止/低置信度），下一帧的 `last_frame` 是旧的 → 位移突变 | 丢帧后紧跟一帧大位移拼接 → 内容缺失或重叠 |

### 1.3 日志数据佐证

```
delta=424 tpl_h=114 new_h=36   ← 匹配太靠下，只追加 36px
delta=412 tpl_h=114 new_h=48   ← 下一帧位移突然变了 12px
delta=265 tpl_h=114 new_h=191  ← 突然跳到完全不同的位置
```

delta 在 265-455 之间剧烈跳动，说明模板在周期性内容上锁不稳定。

---

## 二、业界成熟方案调研

### 2.1 ScrollSnap（macOS 开源，Swift）

**来源**: [Brkgng/ScrollSnap](https://github.com/Brkgng/ScrollSnap) — macOS 上最接近的开源滚动截屏

**核心技术**: `VNTranslationalImageRegistrationRequest`（Apple Vision Framework）

```swift
let request = VNTranslationalImageRegistrationRequest(targetedCGImage: previousCG)
let handler = VNImageRequestHandler(cgImage: currentCG)
handler.perform([request])
// observation.alignmentTransform.ty → 亚像素级垂直位移
```

**关键设计**：
- 帧间比较（`current` vs `previous`），不是帧-画布比较
- Vision 框架内部用 **FFT 相位相关 + 亚像素插值**，精度达 0.01px
- 向下滚动 → `offset > 0` → 从新帧底部追加 `offset` 高度的内容
- 向上滚动 → `offset < 0` → 从画布底部裁剪 `|offset|` 高度
- offset == 0 → 静止，跳过

**优势**: 利用 Apple 硬件加速（Metal/GPU），无需手写匹配算法，亚像素精度。

### 2.2 ShareX（Windows 开源，C#）

**核心技术**: 纯像素行差异比较

**流程**:
1. 后端模拟滚轮 `SendInput(WM_MOUSEWHEEL)`
2. 等待 `scroll_delay`（默认 100ms）让应用渲染
3. 截图 → 与上一张比较
4. 从上一张底部逐行往下找最大相似度位置 → 确定重叠区域
5. 裁掉重叠部分，追加新内容

**关键参数**:
- `scroll_delay`: 等待渲染完成（太短→截到半渲染画面/撕裂）
- 重叠区域固定比例（~30%）

**优势**: 简单可靠，不依赖频率域计算。**劣势**: 整数像素精度，无亚像素。

### 2.3 Picsew / nocoo/image-stitch（离线拼接类）

**核心技术**: **ORB 特征点匹配**（Oriented FAST and Rotated BRIEF）

**流程**:
1. 对两张截图提取 ORB 特征点
2. BFMatcher 或 FLANN 匹配特征点对
3. RANSAC 过滤误匹配
4. 估计刚性变换矩阵（纯垂直平移）
5. 按平移量裁剪拼接

**优势**: 对亮度变化、轻微缩放鲁棒。**劣势**: 计算量大，实时性差（适合离线后处理）。

### 2.4 相位相关（Phase Correlation，频率域方法）

**数学原理**:

两张只差平移 `(dx, dy)` 的图像，其互相关功率谱的相位等于线性相位：

```
R = F₁ · F₂* / |F₁ · F₂*|
IFFT(R) → 在 (dx, dy) 处有一个 δ 峰值
```

**优势**:
- **亚像素精度**：对峰值做抛物线拟合，精度 0.01-0.1px
- **全局最优**：不依赖模板位置，不会卡在周期性假峰
- **O(N log N)**：FFT 比逐行 NCC 滑窗快
- 对周期性内容鲁棒（频率域中周期性体现为多个峰，但主峰仍是真实位移）

**Rust 生态**: `rustfft` crate 成熟稳定。

---

## 三、推荐方案

### 方案 A：FFT 相位相关（推荐）

**适用场景**: 我们是纯垂直 1D 平移，FFT 相位相关是最优数学工具。

**算法**:

```rust
// 1. 取两帧的灰度图（或 Sobel 边缘图）
let gray_a = grayscale(&frame_a);
let gray_b = grayscale(&frame_b);

// 2. 2D FFT（只算垂直方向即可，可降为 1D）
let fft_a = fft2d(&gray_a);
let fft_b = fft2d(&gray_b);

// 3. 互相关功率谱的归一化相位
let cross_power = conj(&fft_b) * &fft_a;
let normalized = cross_power / cross_power.norm();

// 4. IFFT → 峰值位置即位移
let correlation = ifft2d(&normalized);
let (dy, _) = find_peak(&correlation);  // 亚像素：抛物线拟合

// 5. dy > 0 → 向下滚了 dy 像素
//    从 frame_b 的 dy 位置开始到底部，追加到画布
```

**亚像素精化**:

```rust
// 在整数峰值 (px, py) 附近做抛物线拟合
let left  = corr[px - 1];
let peak  = corr[px];
let right = corr[px + 1];
let subpixel = px as f32 + 0.5 * (left - right) / (left - 2.0 * peak + right);
```

**性能估算**:
- 选区 ~2000×500px，1D FFT（沿垂直方向）: O(W × H log H) ≈ 2000 × 500 × 9 ≈ 9M ops
- RustFFT 在 release 模式下约 **2-5ms/帧**
- 对比当前 NCC：100 次滑窗 × 每次 ~200×100 像素块 ≈ 2M ops，但无亚像素

**实现路径**:
1. `Cargo.toml` 加 `rustfft = "3.0"` + `realfft = "3.0"`（实数 FFT 封装）
2. 在 `stitch.rs` 中用 `fft_phase_correlation(frame_a, frame_b) -> f32` 替换 `match_template`
3. 返回 `dy: f32`（亚像素），裁剪时 `round(dy)` 取整，但用 `dy` 的累积值跟踪偏移

### 方案 B：macOS Vision Framework（macOS 专属）

通过 `objc2` 调用 `VNTranslationalImageRegistrationRequest`：

```rust
// 伪代码
let request = VNTranslationalImageRegistrationRequest::new(&prev_cgimage);
let handler = VNImageRequestHandler::new(&curr_cgimage);
handler.perform(&[request]);
let observation = request.results()[0];
let dy = observation.alignment_transform.ty;  // 亚像素
```

**优势**: 零算法实现，Apple GPU 加速。
**劣势**: macOS 专属，Windows/Linux 需另写。但滚动截屏本身各平台的焦点/截屏机制已经平台分支了，拼接算法分支也可接受。

### 方案对比

| 维度 | 当前 NCC | FFT 相位相关 | Vision Framework |
|---|---|---|---|
| 亚像素精度 | 无（整数 px） | 0.01-0.1px | 0.01px |
| 周期性鲁棒 | 差（假匹配） | 好（频率域全局峰） | 好 |
| 计算速度 | ~15ms | ~3-5ms | ~1ms（GPU） |
| 跨平台 | ✅ | ✅ | macOS only |
| 实现复杂度 | 已有 | 中（FFT + 峰值拟合） | 低（调 API） |
| 丢帧处理 | 帧间比较→突变 | 帧间比较→突变 | 帧间比较→突变 |

---

## 四、丢帧问题的独立解决方案

无论用哪种匹配算法，**帧间比较** 都有一个致命问题：如果帧 N 被丢弃（静止/低置信度），帧 N+1 的 `previous` 还是帧 N-1 → 位移突变。

### 4.1 方案：画布底部比较（Canvas-Anchored Matching）

**核心改变**：不要比较 `frame[n]` vs `frame[n-1]`，而是比较 `frame[n]` vs **canvas 的底部 strip**。

```
Canvas:     [................... strip_bottom]
Frame N:    [strip_bottom候选位置 ... 帧底部]

匹配 strip_bottom 在 frame N 中的位置 → 确定新内容
```

**优势**：
- 无论中间多少帧被丢弃，canvas 底部始终是"已确认的最新内容"
- 不会因为丢帧产生位移突变

**实现**：
```rust
// 每次匹配时：
let canvas_bottom = crop_bottom_strip(&self.canvas, tpl_h);
let dy = phase_correlation(&canvas_bottom, &frame);
// 或 NCC: 在 frame 中搜索 canvas_bottom 的位置
```

**注意**：canvas 底部 strip 是 RGBA，frame 也是 RGBA，无需边缘转换。但如果页面底部有动态内容（如加载动画），strip 内容会变 → 需要更新 strip。

### 4.2 方案：动态 strip 更新

匹配成功后，用 **frame 底部的 `tpl_h` 行替换 canvas 底部 strip**（而非追加后的 canvas 底部）：

```rust
// 匹配成功后
let new_strip = crop_bottom(&frame, tpl_h);
self.reference_strip = new_strip;  // 下一帧用这个做比较
```

这样每帧的参考 strip 始终是"上一帧实际看到的内容底部"，即使某些帧被跳过也不影响。

---

## 五、综合推荐实施路径

### 第一优先级：Canvas-Anchored + FFT 相位相关

```
Phase 1: Canvas-Anchored 匹配（解决丢帧）
  ├─ 从 canvas 底部裁 strip 作为参考
  ├─ 每帧与 canvas strip 比较（不再帧间比较）
  └─ 匹配成功后更新 reference strip

Phase 2: FFT 相位相关替换 NCC（解决精度+周期性）
  ├─ 加 rustfft / realfft 依赖
  ├─ 实现 fft_phase_correlation(a, b) -> f32（亚像素 dy）
  └─ 裁剪用 round(dy)，累积偏移用 dy 浮点

Phase 3: 混合策略（鲁棒性兜底）
  ├─ FFT 主匹配
  ├─ 如果 FFT 峰值不明显（score < 阈值）→ fallback NCC 全局搜索
  └─ 如果 NCC 也不够好 → 跳过帧（等用户继续滚动）
```

### 可选：macOS 走 Vision Framework 捷径

如果只考虑 macOS，直接用 `VNTranslationalImageRegistrationRequest`：
- Phase 1（Canvas-Anchored）仍然需要
- Phase 2 用 Vision API 替代 FFT（更简单 + GPU 加速）
- Windows/Linux 后续再补 FFT 实现

### 预期效果

| 问题 | 当前 | 改进后 |
|---|---|---|
| 重叠 | NCC 整数误差累积 | FFT 亚像素 → 误差 < 0.1px |
| 丢帧 | 帧间比较→位移突变 | Canvas-Anchored → 无突变 |
| 周期性假匹配 | 模板窄→假峰 | FFT 全局主峰→鲁棒 |
| 模糊 | 亚像素误差→半像素错位 | 亚像素精度→清晰 |

---

## 六、参考资料

- [ScrollSnap 源码](https://github.com/Brkgng/ScrollSnap) — macOS Vision Framework 滚动截屏
- [Phase Correlation - Wikipedia](https://en.wikipedia.org/wiki/Phase_correlation) — FFT 位移估计原理
- [VNTranslationalImageRegistrationRequest](https://developer.apple.com/documentation/vision/vntranslationalimageregistrationrequest) — Apple Vision 图像配准
- [nocoo/image-stitch](https://github.com/nocoo/image-stitch) — ORB 特征匹配拼接
- [ShareX 滚动截屏文档](https://getsharex.com/docs/scrolling-screenshot) — 像素行差异比较
- [Subpixel Phase Correlation Methods](https://apps.dtic.mil/sti/tr/pdf/ADA519383.pdf) — 亚像素精度方法对比
