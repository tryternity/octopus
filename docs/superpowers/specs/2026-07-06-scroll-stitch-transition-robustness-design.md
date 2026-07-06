# 滚动拼接内容突变鲁棒性改进 — 设计

- 日期：2026-07-06
- 分支：`scroll-stitch-borrow`（从 `feature-0706` 分叉）
- 范围：capx 滚动截图 fallback 链增强 —— 解决「白底黑字文字 → 图片」等内容突变场景的死亡螺旋，**目标成功率 99%**
- 关联文档：
  - 根因分析见本文件 §1
  - 前序 A+B：`docs/superpowers/specs/2026-07-06-scroll-stitch-borrow-A-B-design.md`（A 队列解耦已落地；B 主次比已验证对 NCC 不成立、回退）
  - 借鉴依据：`docs/superpowers/specs/2026-07-06-scroll-stitch-snow-shot-engineering-comparison.md`
- 改动文件：`crates/capx/src/stitch.rs`（`Stitcher` 公共接口零变更）

> **实现状态（2026-07-06 收尾）**：✅ 方向1 已实现合 main（`7cb9bb6`），capx 21 测绿 + e2e 通过（「文字→图片」突变场景拼接完整）。方向3（多锚点双向索引）仍为备选、未实施。

---

## 1. 背景与根因

e2e 暴露：「文字 → 图片」视觉突变场景拼接易**一断到底**（滚动到一半停止拼接）。确诊根因在 `stitch.rs` fallback 死亡螺旋，**与 A 队列解耦无关**（main 用同一套 `stitch.rs`，同病——成功靠突变帧 dy 恰好≈历史中位数的运气）。

死亡螺旋四步：

1. **突变帧 NCC 失配**：画布底部 80px strip（旧内容=文字）与当前帧同位置（新内容=图片）归一化互相关 score 0.51 < `NCC_SCORE_THRESHOLD` 0.65（`stitch.rs:334`）
2. **1D 投影 fallback 失败**：文字行投影 vs 图片行投影，SAD 大、置信度 < 0.25 → `None`（`try_match_1d_projection:637`）
3. **best-guess 盲 append**：`estimate_dy_hint` 取 `dy_history` 中位数（如 -18），`apply_fallback_match`（`:647`）**无正确性校验**直接把当前帧底部 new_rows 行接进画布：
   - dy 偏离时（突变瞬间手速变化）→ append 错位 → **画布底部被污染成错位片段**
4. **熔断后永久放弃**：best-guess 连错 3 次触发 `streak >= 3`（`:479`）→ 之后每帧走到 "all fallbacks exhausted, skipping"；`streak` 仅 NCC 成功才重置，而画布已被污染 NCC 再也不会成功 → **画布永久卡在突变点，之后内容全丢**

**致死双因**：① best-guess 盲 append 污染画布底部模板 ② 熔断后永久放弃（无自愈路径）。

---

## 2. 目标 / 非目标

**目标**
- 突变场景成功率 → **99%**（产品级门槛）
- 不丢突变过渡区内容
- append 必须基于真实匹配（非盲猜），不污染画布
- 保留 canvas-anchored 主路径（亚像素精度、不漂移、现有测试不回归）

**非目标**
- 不做双向滚动（向上滚也拼接）—— 方向 3 备选范畴，本阶段不做
- 不替换 NCC 主匹配算法（亚像素精度、降级链、测试覆盖保持）
- 不动 `Stitcher` 公共接口（`process_frame`/`finalize`/`canvas` 等签名不变）

---

## 3. 主方案：相邻帧参考 fallback（方向 1）

NCC 失配且非静止时，在 1D 投影 fallback **之前**插入一层「前一帧参考」匹配：

```
process_frame:
  NCC（画布底部 strip 锚定）          ← 主路径，完全不变
    失配 ↓
  相邻帧 NCC（前一帧有效区当模板，匹配当前帧）  ← 新增 fallback 层
    失配 ↓
  1D 投影 → best-guess（现有链，不动）
```

### 3.1 为何有效

前一帧已正确 append 进画布，与当前帧只差一个 dy；**突变边界在相邻两帧都存在，重叠区最大**。用前一帧匹配当前帧，突变边界成为共同特征，NCC 能锁定正确 dy → append 正确内容 → 画布不被污染 → 下一帧 canvas-anchored NCC 自愈。

把突变帧从「靠运气（dy≈中位数才成功）」变成「能算出正确 dy 的普通帧」。

### 3.2 数据与接口

- `Stitcher` 加字段 `prev_gray: Option<GrayBuf>`（前一帧有效区灰度，与 `curr_gray` 同来源 `GrayBuf::from_rgba_roi`）
- 每帧 `process_frame` 末尾（return 前）更新 `prev_gray = Some(curr_gray.clone())`
- 新增私有方法 `try_match_prev_frame(&self, prev_gray, curr_gray, eff_top, eff_bottom) -> Option<NccResult>`：复用 `ncc_match`，模板取 `prev_gray` 全有效区（或其底部 strip），搜索区取 `curr_gray`
- `try_fallback` 在 1D 投影之前插入此层；命中 → `apply_fallback_match`（复用现有 append + dy 校验，此时 dy 来自真实匹配非盲猜）

### 3.3 关键约束

- 相邻帧 NCC **只在主 NCC 失配时触发**，正常帧零开销
- 命中后走 `apply_fallback_match`，dy 校验（`min_scroll_px` ≤ new_rows < 90% ROI）照常，不通过则继续降级
- 未命中 → 继续走 1D → best-guess（best-guess 仍是最后兜底，但相邻帧层已拦截绝大多数突变）
- `prev_gray` 内存：有效区灰度 ~width×height 字节（1440×900 ≈ 1.3MB）/录制，可接受

---

## 4. 备选方案：多锚点双向索引（方向 3，暂不实施）

借鉴 snow-shot：top + bottom 内容列表 + FAST 角点（fast12/9 自适应）+ 行/列池化描述子 + `hora` HNSW 近邻索引 + 偏移投票 + `try_rollback` 反向。

### 4.1 何时考虑

- 方向 1 验证后突变场景仍 < 99%
- 或需要双向滚动（向上滚也拼接，`dy>0` 不再 skip）

### 4.2 为何暂缓

- 替换匹配核心栈：引入 `hora`/`rayon` 依赖，重写角点提取/描述子/索引/投票，~500+ 行新代码
- 牺牲现有优势：亚像素精度（角点投票是整数像素）、~15 个合成帧测试覆盖、不 panic 降级纪律需重建
- 回归风险高；单向突变场景下 ROI 远低于方向 1

### 4.3 与方向 1 的关系（非互斥）

方向 1 是给现有 NCC 加 fallback 层，方向 3 是替换匹配核心。若未来上方向 3，方向 1 的相邻帧 fallback 层可保留作「角点法之外的第二兜底」，两者递进而非二选一。

---

## 5. 方向 1 vs 方向 3 预期效果对比

| 维度 | 方向 1 相邻帧参考 | 方向 3 多锚点双向索引 |
|---|---|---|
| 突变场景 | ✅ 根治（相邻帧重叠最大，突变边界共同特征，求出正确 dy 不盲 append） | ✅ 强（FAST 角点局部特征对内容类型不敏感，重叠区角点投票） |
| 正常场景 | 不变（canvas-anchored NCC + 亚像素精度全保留） | 算法栈替换，亚像素精度需用角点重做（投票整数像素，精度下降） |
| 低纹理/纯色 | 继承现有 1D 投影 fallback | 角点稀少 → 需额外 fallback 兜底 |
| 双向滚动 | ❌ 仍单向（`dy>0` skip） | ✅ 原生（top/bottom list + try_rollback） |
| 工程量 | ~几十行，+1 字段 +1 fallback 层，纯增量 | ~500+ 行，新依赖（hora/rayon），重写匹配核心 + 重建测试 |
| 回归风险 | 低（不动主路径） | 高（替换核心算法） |
| 到 99% 把握 | 高（消除致死双因：盲 append + 永久熔断） | 高，但代价不成比例 |
| 关系 | 主方案 | 备选；与方向 1 不互斥 |

**量化判断**：
- 方向 1：突变帧从「靠运气（~75%）」→「求出正确 dy」，预计突变场景 → 95%+；剩余边缘 case（连续多帧失配、前一帧也无特征）由现有 best-guess 兜底，整体逼近 99%
- 方向 3：突变 + 双向理论均强，但单向突变场景下 ROI 远低于方向 1（~10× 工程量、精度下降、回归风险高）

---

## 6. 测试

| 项 | 做法 |
|---|---|
| **相邻帧 fallback 单测**（stitch.rs `#[cfg(test)]`） | 构造「文字帧序列 → 图片帧」过渡的合成帧：① 主 NCC 失配时相邻帧 NCC 命中、dy 正确、append 正确；② 画布底部不被污染（append 后底部 strip 与真实内容一致）；③ 不触发 best-guess 熔断卡死 |
| **边缘 case** | 前一帧也失配（prev_gray 不可信）→ 退化到 1D/best-guess，行为不劣于现状 |
| **回归** | 现有 stitch.rs 全部合成帧测试守护主路径（canvas-anchored NCC）不变；A 队列解耦的 e2e 行为不回归 |

---

## 7. 风险

- **相邻帧也失配的递归**：概率低（前一帧通常 NCC 成功 append）；发生时退化到现有 best-guess，不比现状差
- **失配帧 NCC 计算翻倍**：仅 fallback 路径，正常帧无开销；失配本就是少数帧
- **`prev_gray` 内存**：~1.3MB/录制（1440×900 有效区灰度），可接受
- **模板取全有效区 vs 底部 strip**：实现时取 `prev_gray` 底部 strip（与 canvas strip 同源同尺寸）匹配 curr，复用 `ncc_match` 路径，避免新写匹配逻辑
