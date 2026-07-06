# 滚动拼接 F 配置外置 + D 缩放匹配 — 设计

- 日期：2026-07-06
- 分支：`borrow/scroll-stitch-A-B`（worktree scroll-stitch-borrow）
- 范围：capx `stitch.rs` —— ① **F** 核心调参常量纳入 `StitchConfig`（字段化）；② **D** 大屏 NCC 两阶段降采样 refine
- 关联文档：
  - 借鉴依据 `2026-07-06-scroll-stitch-snow-shot-engineering-comparison.md` §3-F/D
  - snow-shot 参考 `/Users/wudarui/workspace/agent/snow-shot` `src-tauri/src-crates/app-scroll-screenshot-service/src/scroll_screenshot_service.rs`（`image_scale: f32` 配置 + `fast_image_resize` 整图降采样）
  - 前序 A&B（队列解耦）/ 方向1（相邻帧 fallback）均合 main
- 改动文件：`crates/capx/src/stitch.rs`（`Stitcher` 公共接口零变更）

> **实现状态（2026-07-06）**：✅ F + D 均已实现（borrow/scroll-stitch-A-B 分支 `e53b5fe` F 字段化 / `8053665` 辅助函数 / `f1477be` D 两阶段），capx 24 测全绿（含 `test_two_stage_refine_preserves_subpixel` 精度回归 + `ncc_match_range` ×2），desktop check 通过（接口零变更）。待合 main。

---

## 1. 背景与目标

对比 spec §3 把 F/D 列为后续优化。本次实施：

- **F**：常量散落在 `const`（`stitch.rs:8-31`），调参需重编译。用户选「**仅字段化**」最小方案——纳入 `StitchConfig`，Rust 调用方可覆盖，**默认值 = 原 const → 行为零变化**。
- **D**：NCC 全分辨率 `match_template`（imageproc），4K 大屏（3840 宽）每帧计算量大。用户选「**两阶段 refine**」——降采样域粗定位 dy + 原分辨率小邻域 refine，保亚像素精度。

### snow-shot 对照（关键差异，不可照搬）

| | snow-shot | octopus |
|---|---|---|
| 匹配算法 | FAST 角点 + 描述子 + HNSW 投票（整数像素） | NCC + 抛物线亚像素 refine |
| 降采样 | `fast_image_resize` **Nearest** 最近邻（:346） | 必须 **Triangle 双线性**（保边缘） |
| 为何差异 | 角点投票整数像素，Nearest 锯齿无所谓 | Nearest 破坏 NCC response 峰值 → 亚像素 refine 失准 |
| scale 配置 | `image_scale: f32` 字段（:107） | `ncc_downsample_width: u32` 字段（阈值宽度） |

**结论**：借鉴 snow-shot「整图降采样 + scale 外置」的思路，但缩放算法用 Triangle（非 Nearest），且补两阶段 refine（snow-shot 角点法不需要，octopus NCC 必需）。

### 目标

- F：`strip_h`/`max_scroll`/`ncc_score_threshold` + D 的 `ncc_downsample_width` 纳入 `StitchConfig`，默认值不变
- D：大屏（宽 > `ncc_downsample_width`）NCC 性能提升，亚像素精度不退化（两阶段保 ~0.1px）
- `Stitcher` 公共接口零变更（`process_frame`/`finalize`/`canvas` 等签名不变）

### 非目标

- 不做前端 UI / config.yaml 运行时读（用户选最小方案；调参改 `StitchConfig::default` 或调用处）
- 不外置采样/结构常量（`STATIONARY_SAD`/`SAMPLE_STEP_X`/`X_*`/`DY_HISTORY_LEN`/`STICKY_DETECT_MAX` 留 const，调参需求低）
- 不改相邻帧 fallback（方向1 `try_match_prev_frame` 保持原 NCC，fallback 少数帧 + `prev_gray` 小图）

---

## 2. F 字段化

`StitchConfig`（:215）当前仅 `min_scroll_px`/`min_confidence`。加 4 字段（默认 = 原 const）：

| 字段 | 类型 | 默认 | 原 const |
|---|---|---|---|
| `strip_h` | u32 | 80 | `STRIP_H` |
| `max_scroll` | u32 | 220 | `MAX_SCROLL` |
| `ncc_score_threshold` | f32 | 0.65 | `NCC_SCORE_THRESHOLD` |
| `ncc_downsample_width` | u32 | 1920 | （D 新增）|

**影响面**（全 `stitch.rs`）：

- `&self` 方法内：`STRIP_H` → `self.config.strip_h`（`process_frame:316`、`process_frame_inner:382`、`try_match_prev_frame`、`extract_canvas_bottom_gray` 调用处）；`MAX_SCROLL` → `self.config.max_scroll`（搜索范围上界）
- 自由函数 `validate_ncc_match`（:160）：加 `threshold: f32` 参数（替换硬编码 `NCC_SCORE_THRESHOLD`），调用处传 `self.config.ncc_score_threshold`
- 删除 `const STRIP_H`/`MAX_SCROLL`/`NCC_SCORE_THRESHOLD`；`Default` impl 补 4 字段
- 测试里 `StitchConfig::default()` 自动带默认值，无需改测试构造

**保留 const**：`STATIONARY_SAD`/`SAMPLE_STEP_X`/`X_START_RATIO`/`X_END_RATIO`/`DY_HISTORY_LEN`/`STICKY_DETECT_MAX`（不外置）。

---

## 3. D 两阶段 refine

主 NCC 路径（`process_frame_inner:348-383`）按帧宽分支：

```
w > config.ncc_downsample_width(1920)?
  是 → scale = ncc_downsample_width / w
       stage1: Triangle 降采样 template+search → ncc_match → validate
              失配 → Mismatch（交调用方走 stuck/fallback，语义同原）
              通过 → dy_coarse = best_y / scale（还原原分辨率坐标）
       stage2: ncc_match_range(template, search, dy_coarse-2, dy_coarse+2)
              → 原分辨率 ±2px 邻域 NCC + parabolic → refined_y（亚像素）
  否 → 现有 ncc_match → validate → parabolic（完全不变，小屏零影响）
```

### 降采样算法

`image::imageops::resize(img, nw, nh, FilterType::Triangle)` 双线性（保边缘，无新依赖）。**不用 Nearest**——snow-shot 用 Nearest 是角点法整数像素无所谓，octopus NCC+亚像素会被锯齿破坏 response 峰值。

### 新增函数

- `downsample_grayimage(img, scale) -> GrayImage`：Triangle 缩放，宽高 `× scale`（最小 1）。
- `ncc_match_range(template, search, y_min, y_max) -> Option<(refined_y, score)>`：crop search 到 `[y_min, y_max+tmpl_h)` → `ncc_match` → `parabolic_refine` → 加回 `lo` 偏移。范围太小/size 不匹配 → `None`。
- `primary_ncc(&self, template, search, w) -> PrimaryOutcome`：封装两阶段/单阶段 + validate。`PrimaryOutcome::Matched(refined_y, score)` / `Mismatch(score)` / `SizeError`。

### 重构

`process_frame_inner`（:348-383）改为调 `self.primary_ncc(...)`，按 `PrimaryOutcome` 分支：
- `Matched` → 继续 dy 推导 + append
- `Mismatch` → stuck 检测（≥5 归静止）+ `ncc_stuck_count += 1` + `try_fallback`（**语义同原 :357-369**）
- `SizeError` → `try_fallback`（**语义同原 :350-353**）

stuck/fallback 逻辑原样保留，只是匹配来源换成 `primary_ncc`。

### 不动

`try_match_prev_frame`（方向1）保持原 `ncc_match`——fallback 是少数失配帧 + `prev_gray` 本就是有效区小图，两阶段收益小。

---

## 4. 测试

| 项 | 做法 |
|---|---|
| **F 行为不变** | 现有 stitch 全部测试守护（默认值 = 原 const，默认 `ncc_downsample_width=1920` > 测试帧宽 400 → 走单阶段，零回归） |
| **D 大屏精度回归** | `ncc_downsample_width=200`（< 测试帧宽 400 触发 scale=0.5 两阶段），断言两阶段 `refined_y` vs 单阶段（`ncc_downsample_width=9999`）误差 < 0.5px |
| **ncc_match_range 单测** | 合成 template+search 已知偏移，range 内返回正确 `refined_y`；range 外偏移不被选 |
| **回归** | 现有 ~21 测全绿守护主路径（canvas-anchored NCC、相邻帧 fallback、亚像素）不回归 |

---

## 5. 风险

- **Triangle 精度不足**：若大屏精度回归测试误差 > 0.5px，升级 `Lanczos3` 或扩大 stage2 邻域到 ±3px。
- **stage2 邻域 NCC 开销**：±2px = ~5 行 response，原分辨率 `match_template` 但范围极小，开销可忽略；stage1 把全范围 NCC 从原分辨率降到降采样域（4K → 1920，计算量 ~1/4）。
- **降采样后 size 不匹配**：`ncc_match` 返回 `None` → `SizeError` → `try_fallback`（安全降级，同原语义）。
- **`ncc_downsample_width` 默认 1920**：4K(3840) → scale 0.5；2K(2560) → 0.75；1080p(1920) → 不触发（小屏零影响、零精度损失）。
