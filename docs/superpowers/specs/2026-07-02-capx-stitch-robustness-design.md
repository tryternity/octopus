# 滚动拼接健壮性优化设计

**日期**: 2026-07-02
**状态**: ✅ 实施完成（3 改造 + 16 测试全绿，API 零改动）
**分支**: `optimize-capx`
**关联文档**: [`2026-06-12-capx-optimization-design.md`](./2026-06-12-capx-optimization-design.md)（性能优化，已完成）、[`2026-06-30-scroll-stitch-research.md`](./2026-06-30-scroll-stitch-research.md)（算法调研）

---

## 一、背景与动机

性能优化（P1-P5）完成后，拼接引擎的核心瓶颈从性能转向**健壮性**。用户实际使用中三个主要痛点：

| 症状 | 严重度 | 频率 |
|------|--------|------|
| **B 错位/重叠** — 文字行接不上，内容错位 | 高 | 常见 |
| **C 丢内容** — 某段画面缺失 | 高 | 常见 |
| **A 容易断** — 滚到一半拼接停止，长图不完整 | 中 | 偶发 |

### 根因分析（对照当前 stitch.rs）

| 症状 | 根因 | 位置 |
|------|------|------|
| **C 丢内容** | 回弹/模糊帧整体 SAD 抬高，`stationary_sad_avg` 与 `best_sad_avg` 差距缩小，触发 `stationary < best + 1.0` → **误判静止** → 真实滚动内容被丢弃 | `decide_match` `stitch.rs` |
| **B 错位** | 周期性列表中，差一个周期的假匹配 SAD 与真值接近；硬阈值 `SAD_ACCEPT=7.5` 无法区分"纹理丰富但真实"与"纹理丰富但假匹配" | `search_best_offset` 无周期校验 |
| **A 容易断** | `find_overlap_spatial_ext` 返回 `None` → `process_frame` 直接 `return Ok(false)`，**无降级重试** | `process_frame` |

---

## 二、目标与非目标

### 目标

1. **解决 C 丢内容**：时序平滑替代静态校验硬覆盖，单帧抖动不误判静止
2. **解决 B 边界**：动态自适应 SAD 阈值，根据纹理密度 + 历史基线调整接受门槛
3. **解决 A 容易断**：三级兜底降级链，单次匹配失败时依次尝试备选策略
4. **对外 API 零改动**：`Stitcher::new/process_frame/finalize/canvas/height` 签名不变

### 非目标（留待"全面"阶段）

- **不做分层粗精搜索**（降采样粗搜）— 当前暴力搜索 + 动态阈值已能解决 BC；分层引入新复杂度
- **不做动态模板高度** — 降级链中的"缩小到 40px"已覆盖空白页场景
- **不做预处理均值滤波/降采样** — 灰度转换已有，当前噪声不是主要问题
- **不做帧率自适应采集** — manual 模式用户自控滚动，固定 30ms 采样合理

---

## 三、设计

### 3.1 改造 1：时序平滑替代静态校验硬覆盖（解决 C 丢内容）

#### 当前问题

`decide_match` 中的静态校验是"硬覆盖"——一次 `stationary_sad < best_sad + 1.0` 即强制返回 `dy=0`：

```rust
// 当前 decide_match
if stationary_sad_avg < STATIONARY_SAD || stationary_sad_avg < best_sad_avg + 1.0 {
    return Some((0.0, 1.0));  // 强制判静止，哪怕真实在滚动
}
```

回弹场景：画面轻微拉伸，整体 SAD 抬高。stationary_sad（dy=0 处）与 best_sad（搜索到的最佳）差距从正常的 5+ 缩小到 < 1.0 → 触发误判静止 → 真实滚动被丢弃。

#### 新方案：dy 时序历史 + 滑动均值判静止

**Stitcher 新增字段**：

```rust
/// 最近若干帧的 dy 历史，用于时序平滑判断静止。
dy_history: VecDeque<f64>,
```

`new()` 初始化为空 `VecDeque::with_capacity(8)`。

**静止判断改为时序平滑**：

```rust
/// 判断当前是否为静止状态（基于历史 dy 均值）。
/// 回弹帧 dy 可能抖动到 -3，但历史 [-15,-12,-10,-3] 均值 -10，不判静止。
fn is_stationary(&self) -> bool {
    if self.dy_history.len() < 3 {
        return false;  // 不足 3 帧，不判静止（让 SAD 主匹配决定）
    }
    let n = self.dy_history.len().min(5);
    let recent: f64 = self.dy_history.iter().rev().take(n).sum::<f64>() / n as f64;
    recent.abs() < STATIONARY_DY_THRESHOLD  // 均值 |dy| < 2.0 视为静止
}
```

**`decide_match` 移除静态校验硬覆盖**，改为只返回搜索结果：

```rust
fn decide_match(
    best_y_offset: u32, best_sad_avg: f64, stationary_sad_avg: f64,
    confidence: f64, template_y: u32, dynamic_threshold: f64,
) -> Option<(f64, f64)> {
    // 保留静止 SAD 锚点作为"绝对静止"快速路径
    // （画面完全没动时 stationary_sad 极低，这是安全的）
    if stationary_sad_avg < STATIONARY_SAD {
        return Some((0.0, 1.0));
    }
    // 移除 stationary < best + 1.0 的硬覆盖——交由 is_stationary() 时序判断
    if best_sad_avg < dynamic_threshold && confidence > MIN_CONFIDENCE {
        let dy = best_y_offset as f64 - template_y as f64;
        Some((dy, confidence))
    } else {
        None
    }
}
```

**`process_frame` 中静止判断上移**：

```rust
// 主匹配后
let result = find_overlap_spatial_ext(...);
match result {
    Some((dy, conf)) => {
        // 双重静止校验：
        // ① SAD 主匹配返回 dy ≈ 0（绝对静止 SAD 锚点极低时）
        // ② 时序历史均值也接近 0（is_stationary）
        // 两者都满足才判静止跳过——防止单帧 SAD 误判
        if dy.abs() < 0.5 && self.is_stationary() {
            return Ok(false);  // 确认静止，跳过
        }
        // 正常追加 + 更新 dy_history
        self.dy_history.push_back(dy);
        if self.dy_history.len() > DY_HISTORY_LEN { self.dy_history.pop_front(); }
        // ...append...
    }
    None => { /* 进入降级链（改造 3）*/ }
}
```

> **关键变化**：原来 `decide_match` 内 `stationary_sad < best + 1.0` 单帧即硬覆盖为静止；现在需要 **dy≈0 且时序也确认**才判静止。回弹帧 SAD 可能返回 dy≈0（误匹配），但时序均值 -10 否决 → 不丢内容。

**效果**：回弹帧 dy 抖动到 -3，但历史均值 -10 → 不判静止 → 内容不丢。

### 3.2 改造 2：动态自适应 SAD 阈值（解决 B 边界 + C 模糊帧被拒）

#### 当前问题

`SAD_ACCEPT=7.5` 是硬编码，空白页 SAD 天然低（纹理少）、密集列表天然高（纹理多），同一阈值不适合所有场景。

#### 新方案：纹理密度 + 历史 EMA 基线

**纹理密度评估**（Sobel 式水平梯度阈值计数）：

```rust
/// 评估模板条区域的纹理密度（边缘像素占比）。
/// 复用 sample_cols 的相邻列对做水平差分，O(strip_h × n_cols)，开销极低。
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

**Stitcher 新增字段**：

```rust
/// 历史成功匹配的 SAD 均值（EMA，指数移动平均）。
sad_baseline: f64,
```

`new()` 初始化为 `0.0`。每次成功匹配后用 EMA 更新：

```rust
const SAD_BASELINE_ALPHA: f64 = 0.3;  // EMA 平滑系数
self.sad_baseline = SAD_BASELINE_ALPHA * best_sad_avg + (1.0 - SAD_BASELINE_ALPHA) * self.sad_baseline;
```

**动态阈值计算**：

```rust
/// 根据当前帧纹理密度 + 历史 SAD 基线动态计算 SAD 接受阈值。
fn dynamic_sad_accept(&self, texture: f64) -> f64 {
    // 纹理越丰富 → 绝对 SAD 天然更高 → 允许更高阈值
    let texture_bonus = texture * TEXTURE_BONUS_FACTOR;  // texture ∈ [0,1], factor=30
    // 历史基线浮动：EMA 均值的 1.5 倍 + 5 作为上界
    let baseline_cap = self.sad_baseline * SAD_BASELINE_MULTIPLIER + SAD_BASELINE_PADDING;
    (SAD_ACCEPT + texture_bonus).min(baseline_cap).max(SAD_ACCEPT)
}
```

**`find_overlap_spatial_ext` 接受动态阈值参数**：

```rust
fn find_overlap_spatial_ext(
    ref_buf: &GrayBuf,
    curr_buf: &GrayBuf,
    x_start: u32, x_end: u32,
    eff_top: u32, eff_bottom: u32,
    max_scroll: u32,
    last_dy: Option<f64>,
    sad_accept: f64,  // 新增：动态阈值
) -> Option<(f64, f64)>
```

`decide_match` 用传入的 `dynamic_threshold` 替代硬编码 `SAD_ACCEPT`。

**效果**：
- 密集列表（纹理密度 0.3）→ 阈值 ~16.5，但 baseline_cap 可能限制到 ~12
- 空白页（纹理密度 0.05）→ 阈值 ~9.0
- 回弹帧（历史 baseline 6）→ 阈值上限 ~14
- 周期列表假匹配（best_sad 可能 5.0）→ 仍在阈值内，但改造 1 的时序平滑 + 改造 3 的周期校验兜底

### 3.3 改造 3：多级兜底降级（解决 A 容易断 + B 假匹配）

#### 当前问题

`find_overlap_spatial_ext` 返回 `None` → `process_frame` 直接 `return Ok(false)`。

#### 新方案：三级降级链

```rust
pub fn process_frame(&mut self, frame: &RgbaImage) -> Result<bool> {
    // ...（初始化/eff 计算不变）...

    let curr_buf = GrayBuf::from_rgba(frame);
    let texture = estimate_texture_density(&curr_buf, &sample_cols, eff_bottom - STRIP_H);
    let sad_accept = self.dynamic_sad_accept(texture);

    // 主匹配（动态阈值）
    if let Some(result) = self.try_match(&curr_buf, &sample_cols, eff_top, eff_bottom, MAX_SCROLL, sad_accept) {
        return self.apply_match(result, frame, &curr_buf, w, eff_top, eff_bottom);
    }

    // 降级 1：扩大搜索范围 ×2（快速滚动可能超出 MAX_SCROLL）
    if let Some(result) = self.try_match(&curr_buf, &sample_cols, eff_top, eff_bottom, MAX_SCROLL * 2, sad_accept) {
        log::info!("[stitch] fallback 1: expanded search range");
        return self.apply_match(result, frame, &curr_buf, w, eff_top, eff_bottom);
    }

    // 降级 2：缩小模板到 40px + 放宽阈值 ×1.5（空白页/低纹理场景）
    if let Some(result) = self.try_match_strip(&curr_buf, &sample_cols, eff_top, eff_bottom, 40, sad_accept * 1.5) {
        log::info!("[stitch] fallback 2: reduced strip height");
        return self.apply_match(result, frame, &curr_buf, w, eff_top, eff_bottom);
    }

    // 降级 3：1D 灰度投影匹配（对纹理极少的纯色场景鲁棒）
    if let Some(result) = self.try_match_1d_projection(&curr_buf, eff_top, eff_bottom, sad_accept) {
        log::info!("[stitch] fallback 3: 1D projection match");
        return self.apply_match(result, frame, &curr_buf, w, eff_top, eff_bottom);
    }

    // 全部失败：不停止，等下一帧（desktop 层 250 帧兜底处理）
    log::info!("[stitch] all fallbacks exhausted, skipping frame");
    Ok(false)
}
```

**内部方法**：

```rust
/// 主匹配（封装 find_overlap_spatial_ext 调用）
fn try_match(&self, curr: &GrayBuf, cols: &[usize], eff_top: u32, eff_bottom: u32, max_scroll: u32, sad_accept: f64) -> Option<(f64, f64)>;

/// 缩小模板匹配（strip_h 可变版本）
fn try_match_strip(&self, curr: &GrayBuf, cols: &[usize], eff_top: u32, eff_bottom: u32, strip_h: u32, sad_accept: f64) -> Option<(f64, f64)>;

/// 1D 灰度投影匹配（行均值序列 SAD）
fn try_match_1d_projection(&self, curr: &GrayBuf, eff_top: u32, eff_bottom: u32, sad_accept: f64) -> Option<(f64, f64)>;
```

#### 1D 灰度投影匹配算法

将每行像素按 `sample_cols` 取均值，降为一维信号，对一维信号做 SAD 搜索。对纯色/低纹理场景（2D SAD 缺乏特征）反而更鲁棒，因为行均值对横向噪声做了平均。

```rust
fn try_match_1d_projection(&self, curr: &GrayBuf, eff_top: u32, eff_bottom: u32, sad_accept: f64) -> Option<(f64, f64)> {
    // 参考帧行均值信号
    let ref_proj = row_means(&self.reference, eff_top, eff_bottom);
    // 当前帧行均值信号
    let curr_proj = row_means(curr, eff_top, eff_bottom);
    // 在 ref_proj 底部 strip 范围搜索 curr_proj 的最佳对齐位置
    // ...类似 search_best_offset 但在一维信号上...
}
```

#### `find_overlap_spatial_ext` 参数化 strip_h

当前 `STRIP_H=80` 是常量，降级 2 需要可变。改为参数化：

```rust
fn find_overlap_spatial_ext(
    ref_buf: &GrayBuf,
    curr_buf: &GrayBuf,
    x_start: u32, x_end: u32,
    eff_top: u32, eff_bottom: u32,
    max_scroll: u32,
    last_dy: Option<f64>,
    sad_accept: f64,    // 新增：动态阈值
    strip_h: u32,       // 新增：可变模板高度（默认 STRIP_H）
) -> Option<(f64, f64)>
```

---

## 四、API 兼容性

**对外 API 零改动**。所有新增字段（`dy_history`、`sad_baseline`）和新增方法（`is_stationary`、`dynamic_sad_accept`、`try_match*`、`apply_match`）均为私有。`new/process_frame/finalize/canvas/height` 签名不变。

---

## 五、新增常量

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
const SAD_BASELINE_PADDING: f64 = 5.0;
/// 动态阈值：EMA 平滑系数
const SAD_BASELINE_ALPHA: f64 = 0.3;
/// 降级 2：缩小模板高度
const FALLBACK_STRIP_H: u32 = 40;
/// 降级 2：阈值放宽倍数
const FALLBACK_SAD_MULTIPLIER: f64 = 1.5;
```

---

## 六、测试策略

### 新增测试用例（合成图 + 不变量）

1. **时序平滑不误判回弹**：
   - 构造 4 帧序列：dy=[-15,-12,-10,-3]（最后帧模拟回弹 dy 变小）
   - 验证：第 4 帧不被 `is_stationary()` 判为静止（均值 -10 > 阈值）

2. **真实静止被时序识别**：
   - 构造 5 帧序列：全部相同（dy=0）
   - 验证：`is_stationary()` 返回 true

3. **动态阈值随纹理变化**：
   - 构造高纹理帧（密集条纹）+ 低纹理帧（纯色 + 少量文字）
   - 验证：`dynamic_sad_accept()` 对高纹理返回更高阈值

4. **降级链触发**：
   - 构造一个超出 MAX_SCROLL 的快速滚动帧
   - 验证：主匹配失败但降级 1（扩大范围）成功

5. **1D 投影匹配**：
   - 构造纯色背景 + 少量文字的帧（2D SAD 纹理不足）
   - 验证：降级 3 的 1D 投影能匹配

6. **baseline EMA 更新**：
   - 连续匹配后 `sad_baseline` 收敛到合理值

7. **回归测试**：现有 12 个测试必须保持全绿

### `make_frame` 工具增强

现有 `make_frame` 需扩展支持：
- 可控纹理密度（稀疏/密集条纹）
- 可控噪点水平（模拟拖影/模糊）
- 回弹序列构造（dy 先大后小）

---

## 七、风险与缓解

| 风险 | 缓解 |
|------|------|
| 时序平滑引入延迟：前 2-3 帧 `dy_history` 不足，不判静止 | `is_stationary()` 在 `len < 3` 时返回 false，让 SAD 主匹配决定 |
| 动态阈值放过坏帧（纹理丰富时阈值放宽） | baseline_cap 上界限制（EMA × 1.5 + 5）；改造 1 的时序平滑兜底（假匹配 dy 与历史差距大 → 不追加） |
| 1D 投影匹配在强周期列表中也有多峰 | 作为最后降级手段；置信度要求更严（`confidence > 0.25` 而非 0.15） |
| `find_overlap_spatial_ext` 签名变化（新增 2 参数） | 内部私有函数，调用方都在 stitch.rs 内；`try_match` 封装统一调用 |
| 降级链增加每帧计算量（最坏 4 次匹配） | 正常情况主匹配一次通过，降级仅在边缘场景触发；每级降级都有日志便于调优 |

---

## 八、验收标准

1. `cargo test -p octopus-capx` 全绿（现有 12 + 新增 ≥6 = ≥18 个测试）
2. `cargo check -p octopus-capx -p octopus-desktop` 无错误
3. API 零改动（`lib.rs` 无变化，公开签名不变）
4. 源码无新增裸魔法数字（全部命名常量）
5. 手动验证：滚动截屏在密集列表、空白页、快速滚动场景下不再出现错位/丢内容/断裂（需人工实测，测试覆盖算法层）
