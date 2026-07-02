# Canvas-Anchored 匹配设计

**日期**: 2026-07-02
**状态**: ✅ 实施完成（Canvas-Anchored 匹配落地，18 测试全绿，API 零改动）
**分支**: `optimize-capx`
**关联文档**: [`2026-07-02-capx-stitch-robustness-design.md`](./2026-07-02-capx-stitch-robustness-design.md)（健壮性优化前置）、[`2026-06-30-scroll-stitch-research.md`](./2026-06-30-scroll-stitch-research.md)（调研第四节首次提出）

---

## 一、背景与根因

健壮性优化（时序平滑 + 动态阈值 + 三级降级链）实施后，丢内容问题改善但仍存在。

### 根因（systematic-debugging Phase 1 确认）

**帧间比较的累积漂移**：`self.reference` 只在匹配成功时更新。匹配失败时 reference 不前进，后续帧与过时的 reference 比较 → 真实位移逐帧累积 → 最终超出搜索范围 → 内容永久丢失。

```
帧 N-1: 匹配成功，reference = N-1，dy=-30
帧 N:   匹配失败（模糊/回弹），reference 仍 = N-1
帧 N+1: 与 N-1 比较 → 真实位移 = 60px，可能还能匹配
帧 N+K: 位移 > 440px（降级 1 上限）→ 永远匹配不上 → 内容永久丢失
```

### 业界验证

| 工具 | 策略 | 结论 |
|------|------|------|
| **ShareX** | 帧-画布比较（每帧与 `ResultImage` 底部 strip 对齐） | 无累积漂移，接缝最干净 |
| **ScrollSnap** | 帧间比较，但失败时不前进参考帧 | 轻量级近似，下帧自动追赶 |
| **Scrollshot** | 帧间 + 时序平滑中位数 | 用统计补偿，非根治 |

ShareX 的帧-画布比较是最彻底的解决方案。[`2026-06-30-scroll-stitch-research.md`](./2026-06-30-scroll-stitch-research.md) 第四节早在调研阶段就提出了此方案（"Canvas-Anchored Matching"），但当时未实施。

---

## 二、目标与非目标

### 目标

1. **根治丢内容**：匹配输入源从 `self.reference`（上一帧）改为画布底部 strip，消除累积漂移
2. **对外 API 零改动**
3. **保留现有健壮性优化**：时序平滑、动态阈值、三级降级链不受影响

### 非目标

- 不做 Sobel 梯度特征（阶段二，验证后按需）
- 不做多 band 投票（阶段二）
- 不做文本主体区域检测（阶段二）

---

## 三、设计

### 3.1 核心改造：匹配输入源从 reference 帧改为画布底部 strip

#### 当前（帧间比较）

```
reference（上一帧完整灰度）↔ curr_gray（当前帧完整灰度）
匹配成功后：self.reference = curr_gray
匹配失败：self.reference 不变 → 下一帧与过时 reference 比较 → 位移突变
```

#### 改为（Canvas-Anchored）

```
canvas_bottom_gray（画布底部 STRIP_H 行灰度）↔ curr_gray（当前帧完整灰度）
每帧重新从 canvas_buf 提取 → 无论多少帧失败，画布底部始终是最新已确认内容
```

### 3.2 数据流变化

**移除 `self.reference: GrayBuf` 字段**。新增每帧即时提取的画布底部灰度。

```rust
// 每帧 process_frame 开始时，从 canvas_buf 提取底部 strip 转灰度
fn extract_canvas_bottom_gray(&self, strip_h: u32) -> GrayBuf {
    let row_bytes = self.canvas_w as usize * 4;
    let start_row = self.canvas_h.saturating_sub(strip_h);
    // 直接从 canvas_buf 底部 strip_h 行 RGBA 转灰度
    let mut data = Vec::with_capacity(strip_h as usize * self.canvas_w as usize);
    for y in start_row..self.canvas_h {
        let row_start = y as usize * row_bytes;
        for x in 0..self.canvas_w as usize {
            let off = row_start + x * 4;
            let r = self.canvas_buf[off] as u32;
            let g = self.canvas_buf[off + 1] as u32;
            let b = self.canvas_buf[off + 2] as u32;
            let luma = (2126 * r + 7152 * g + 722 * b) / 10000;
            data.push(luma as u8);
        }
    }
    GrayBuf { data, width: self.canvas_w as usize }
}
```

**关键区别**：这个 GrayBuf 的 height = strip_h（不是完整帧高度），因为只提取画布底部。匹配逻辑需要适配：模板条和搜索空间都基于这个"短"灰度图。

### 3.3 匹配逻辑调整

当前 `find_overlap_spatial_ext` 假设 ref_buf 和 curr_buf 是同样大小的完整帧。Canvas-Anchored 后，ref_buf 只有 strip_h 行（画布底部），curr_buf 是完整帧。

**调整搜索逻辑**：ref_buf 的全部 strip_h 行就是模板，在 curr_buf 的 `[eff_top, eff_bottom]` 范围内搜索 ref_buf 的最佳对齐位置。

```rust
// ref_buf: 画布底部 strip（strip_h 行）
// curr_buf: 当前帧完整灰度（h 行）
// 在 curr_buf 中搜索 ref_buf 的对齐位置
// y_offset = ref_buf 顶部在 curr_buf 中的 y 坐标
// dy = y_offset - eff_top（ref_buf 顶部 vs 有效区顶部）
//   → dy < 0 表示 curr 在 ref 下方有新内容（用户向下滚了）
```

具体来说，`search_best_offset` 的搜索范围从 `[eff_top, eff_bottom - strip_h]` 变为 `[eff_top, eff_bottom - strip_h]`，模板就是整个 ref_buf（strip_h 行），不再需要 `extract_template` 单独提取（ref_buf 本身就是模板）。

### 3.4 对现有改造的影响

| 组件 | 变化 |
|------|------|
| `self.reference` 字段 | **移除**，每帧从 canvas_buf 提取 |
| `GrayBuf::from_rgba(frame)` | 保留，仍用于 curr_buf |
| `find_overlap_spatial_ext` | ref_buf 高度 = strip_h（非完整帧）；搜索逻辑微调 |
| `extract_template` | 简化——ref_buf 本身就是模板，直接传 ref_buf.data |
| `search_best_offset` | 模板来源从 extract_template 变为 ref_buf 全量 |
| `try_match_1d_projection` | ref_proj 从画布底部提取 |
| `apply_fallback_match` | 不再 `self.reference = curr_buf.clone()` |
| `decide_match` | 不变 |
| `is_stationary` | 不变 |
| `dynamic_sad_accept` | 不变 |
| `estimate_texture_density` | 输入改为画布底部灰度 |
| `finalize` | ref_buf 也改为画布底部 |

### 3.5 初始化处理

首帧 `process_frame` 初始化时，canvas 就是首帧裁剪后内容。此时 `extract_canvas_bottom_gray` 从 canvas 底部提取，作为下一帧的匹配模板。**无需特殊初始化逻辑**——第二帧直接与画布底部比较，完全正确。

### 3.6 性能影响

- 每帧提取画布底部 STRIP_H=80 行 RGBA 转灰度：80 × canvas_w × (4 读 + 1 写) ≈ 80×2000×5 = 800K ops ≈ 0.1ms
- 相比之前 `GrayBuf::from_rgba(frame)` 转整帧灰度：600×2000×5 = 6M ops ≈ 0.6ms
- **反而更快**（只转 80 行而非整帧）

---

## 四、API 兼容性

对外 API 零改动。`reference` 字段是私有的，移除不影响调用方。

---

## 五、测试策略

### 现有 16 测试必须保持全绿

### 新增测试

1. **Canvas-Anchored 不丢内容**：构造 5 帧序列，第 3 帧是模糊帧（匹配失败），验证第 4 帧能与画布底部正确对齐（而非与第 3 帧比较）
2. **画布底部提取正确性**：构造已知画布内容，验证 `extract_canvas_bottom_gray` 提取的灰度与手动计算一致
3. **连续失败后恢复**：构造 3 帧连续匹配失败后，第 4 帧恢复正常，验证能正确拼接（不位移突变）

---

## 六、风险与缓解

| 风险 | 缓解 |
|------|------|
| Canvas-Anchored 后 ref_buf 高度变化导致搜索逻辑 bug | 新增"画布底部提取正确性"测试；搜索范围严格基于 ref_buf 高度 |
| 画布底部正好是 sticky 区域导致匹配异常 | 画布首帧已裁掉 sticky_bottom，底部始终是有效内容 |
| finalize 时画布可能很大，提取底部仍只取 STRIP_H 行 | 只取 strip_h 行，不随画布增长 |
