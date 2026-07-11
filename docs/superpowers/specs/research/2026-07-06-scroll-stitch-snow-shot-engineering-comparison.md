# 滚动截图拼接：octopus ↔ snow-shot 工程经验对比

- 日期：2026-07-06
- 分支：feature-0706
- 范围：分析性文档（非功能 spec，不含实施任务）。聚焦「工程实践」借鉴，**不含算法替换**。
- 对比对象：
  - octopus：`crates/capx/src/stitch.rs`（1151 行）+ `crates/desktop/src/screenshot_commands.rs`（调度层）
  - snow-shot：`/Users/wudarui/workspace/agent/snow-shot/src-tauri/src-crates/app-scroll-screenshot-service/`（3 文件，共 1081 行）
- 关联文档：
  - 订正了 snow-shot 分析（已归档至 `docs/superpowers/specs/2026-07-05-archived-design.md` §六 6.2）两处把 snow-shot 误标为 NCC 的描述
  - 性能向另见 `docs/superpowers/specs/2026-07-06-scrolling-screenshot-performance-optimization.md`

---

## 1. 前提订正：两端**不是**同一算法

调查前的工作假设是「octopus 与 snow-shot 用相同的滚动拼接算法」。读源码后证伪：

| | octopus | snow-shot |
|---|---|---|
| 核心方法 | **NCC 模板匹配**：从画布底部取 80px 条带 → Sobel 特征图 + 归一化 → 对当前帧整 ROI 做归一化互相关（`imageproc::template_matching::match_template`）→ 抛物线亚像素细化 | **角点 + 描述子 + 近邻索引**：FAST 角点（fast12/9 自适应）→ 自定义行/列池化描述子 → `hora` HNSW 近邻搜索 → 偏移投票 |
| 匹配粒度 | 连续位移（亚像素） | 离散像素偏移（整数投票） |
| 全仓 NCC/`template_matching` 痕迹 | 有 | **零**（`grep -rE 'template_matching\|MatchTemplate\|ncc\|cross_correlation'` 空） |
| 关键依赖 | `imageproc`（template_matching + gradients::sobel） | `hora`（ANN）、`rayon`、`imageproc::corners`（FAST）、`fast-image_resize` |

**结论**：算法族不同，但本分析关注的是**与算法无关的工程实践**（架构、健壮性、性能、测试、可配置性），借鉴照样成立。算法层面的差异（如 snow-shot 的角点法、octopus 的 NCC 法）不在借鉴范围内。

---

## 2. 算法层实现速览（背景，非借鉴项）

### octopus `Stitcher`（stitch.rs）

- Canvas-Anchored：每帧从**画布底部**取模板（`extract_canvas_bottom_gray`，STRIP_H=80），不存独立 reference 帧 → 中间帧失败后仍能从最新已确认内容恢复（`test_canvas_anchored_recovers_after_failures`）。
- Sobel 特征 + mean+3σ 归一化；纯色帧（max_gradient=0）退化为灰度（`to_feature_map`）。
- 多级降级链：NCC → 1D 行投影 SAD（`try_match_1d_projection`）→ best-guess（dy 历史中位数 `estimate_dy_hint`）→ 跳过。
- 熔断计数器：`best_guess_streak`（≤3）、`ncc_stuck_count`（≥5）、`same_dy_count`（≥3 周期假匹配检测）。
- sticky 顶/底检测（`detect_sticky`）、`finalize()` 末帧补缝 + 补 footer。

### snow-shot `ScrollScreenshotService`（992 行）

- 双向：`top_image_list` + `bottom_image_list`，双 HNSW 索引，`try_rollback` 尝试反方向。
- 每帧：灰度 → 按 `sample_rate` 缩放（clamp min/max）→ FAST 角点 → 描述子 → ANN 查询 → 偏移投票。
- 接受判据（`get_offsets`）：主偏移频次 ≥ 角点数/10 **且** ≥ 次高频次 ×2；72% 角点触发 min_diff 违例 → 判为原位（静止）。
- `export()`：按 `overlay_size` alpha 混合接缝。

---

## 3. 架构对照：调度层是最大差距

### snow-shot：三段式 + 队列解耦

```
CaptureService (34 行)  ──push──▶  ImageService (VecDeque 队列, 52 行)  ──drain──▶  ScreenshotService (算法, 992 行)
   只管截屏                            解耦缓冲                                       只管拼接
```

三个独立 `tauri::State<Mutex<_>>`，前端分别调 `scroll_screenshot_capture`（入队）/ `scroll_screenshot_handle_image`（出队处理）/ `save`。**截屏（I/O 敏感、不能丢帧）与拼接（CPU 密集）被队列隔开**：拼接慢一拍不会拖垮截帧节奏，burst 时队列吸收。

### octopus：单循环耦合（`screenshot_commands.rs:1089`）

```
while RECORDING {
  interval.tick().await;            // 30ms 节拍
  spawn_blocking(capture).await;    // 截屏（已隔离）
  stitcher.process_frame(&frame);   // ← 拼接跑在 async 任务线程上，同步阻塞，未 spawn_blocking
  spawn_blocking(preview encode);   // 预览编码（已隔离）
}
```

截屏与拼接串在同一 task 的同一循环，靠 `tick().await` 交替。`process_frame` 做灰度转换 + Sobel + NCC，是实打实的 CPU 活，一旦某帧偏慢，循环被拉长 → 截帧间隔漂移、潜在丢帧。

---

## 4. 可借鉴工程经验（A–G，按价值排序）

| # | 借鉴点 | snow-shot 做法（file:line） | octopus 现状（file:line） | 价值 |
|---|---|---|---|---|
| **A** | **捕获/拼接队列解耦** | `ScrollScreenshotImageService`（VecDeque，`scroll_screenshot_image_service.rs:15`）隔开 capture 与 process | 单循环串行（`screenshot_commands.rs:1089`），`process_frame` 未 spawn_blocking（`:1122`） | ⭐⭐⭐ 最高：截帧节奏不被拼接拖累，burst 不丢 |
| **B** | **主次比判据** | 接受偏移需 `max ≥ corners/10 且 max ≥ second_max×2`（`scroll_screenshot_service.rs:705-711`） | `validate_ncc_match` 只看 max-min 差值 0.1（`stitch.rs:176`）；周期假匹配靠 `same_dy_count` 多计数器（`:381-413`） | ⭐⭐⭐ 单一判据天然抗歧义/周期假匹配，可简化状态机 |
| **C** | **双向滚动** | `top/bottom_image_list` + 双索引 + `try_rollback`（`:819`） | 单向：`dy>0` 直接 skip（`stitch.rs:364`） | ⭐⭐ 功能缺口，半架构改造 |
| **D** | **缩放后匹配 / 原图导出** | `get_gray_image` 按 `sample_rate` 缩放后提特征（`:321`），导出用全分辨率 | 全分辨率匹配（ROI 灰度，`stitch.rs:312`） | ⭐⭐ 性能，大屏明显 |
| **E** | **热路径 rayon 并行** | `par_iter()` 算描述子（`:316`）+ 并发 ANN 搜索（`:605`） | 单线程（1D SAD 回退、灰度转换单线程；`match_template` 自身 native） | ⭐⭐ 性能 |
| **F** | **配置运行时可调** | `init()` 从前端收 8 个参数（`scroll_screenshot.rs:15`：sample_rate/corner_threshold/descriptor_patch_size…） | `StitchConfig` 仅 2 字段，其余全 `const` 写死（`stitch.rs:215`） | ⭐⭐ 免重编译调参、按机器适配 |
| **G** | **导出接缝羽化** | `export()` 用 `overlay_size` alpha 混接缝（`:922`） | 硬切（`stitch.rs:423`，注释明说「不需要额外接缝寻找」） | ⭐ 出图质量，高对比内容可见 |

---

## 5. 不该借鉴：octopus 已更优之处

避免反向 cargo-cult，以下维度 **octopus 领先，保持现状**：

| 维度 | octopus | snow-shot |
|---|---|---|
| **不 panic 纪律** | 处处降级：`to_gray_image`/`canvas()` 失败返 1×1、宽度不符早退、`process_frame().unwrap_or(false)` | 多处 `.unwrap()`（`from_slice_u8().unwrap()` :338、`ann_index.add().unwrap()` :443、`from_raw().unwrap()` :989），任一 panic 整功能挂 |
| **像素热访问** | `GrayBuf` 行主序切片直访，无 `get_pixel` 边界检查（`stitch.rs:43-85`） | `unsafe_get_pixel`（`:163`） |
| **测试覆盖** | ~15 个合成帧行为测试（`stitch.rs:813-1151`） | 992 行**无 `#[cfg(test)]`** |
| **魔法数字** | 顶部集中命名常量带注释（`stitch.rs:5-31`） | 散落裸数字 `0.72` / `/10` / `*2` / `0.1` |
| **亚像素精度** | 抛物线细化（`parabolic_refine_from_response`） | 整数像素投票 |
| **降级链** | NCC → 1D 投影 → best-guess 三级 | 单一 ANN 匹配，失败即止 |

---

## 6. 落地优先级与建议

按 ROI 排序（不含实施任务，仅方向；若要落地另起 plan）：

1. **A 队列解耦** —— 改动集中在 `screenshot_commands.rs` 循环，不动算法，收益最稳。仓内已有 `mpsc::channel` 现成模式（同文件 `:588`、`:808`）。把截屏线程化 `tx.send(frame)`，拼接消费 `rx.recv()`。
2. **B 主次比判据** —— 在 `validate_ncc_match` 加一项「response 主峰/次峰 ≥ 阈值」，可能简化掉 `same_dy_count` 周期检测状态机。改动小、风险低。
3. **F 配置外置** —— 把 `STRIP_H`/`MAX_SCROLL`/`NCC_SCORE_THRESHOLD` 等纳入 `StitchConfig`，前端可调，免重编译调参。
4. **D 缩放匹配** —— NCC 前对 template/search_region 降采样，性能提升，**须配测试防精度回归**。

C（双向）是较大改动，按需；E/G 是锦上添花。

---

## 7. 附：旧文档订正记录

snow-shot 分析（已归档至 `docs/superpowers/specs/2026-07-05-archived-design.md` §六 6.2）两处把 snow-shot 滚动截图误标为「NCC 模板匹配」，已订正为「FAST 角点 + 描述子 + HNSW 近邻索引」（归档版已含此订正）：

- §1.1 截图功能表「滚动截图」行
- §3.3 架构对比表「滚动截图」行（同时补准 octopus 列为「CAPX + Sobel/NCC」）
