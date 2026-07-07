# 重构优化设计 — 死代码/重复/超长代码清理

> 日期：2026-06-12
> 状态：**✅ 已完成**（merge `903a66a` + 后续扩展 `9bc0b53`/`0c0ae93`，84+42 tests + tsc 0 errors 全绿，0 warnings）
> 来源：[代码审查报告](../../code-review/2026-06-12-code-review.md)
> 决策：Q1=A（分层提取 + 局部 TDD）、Q2=A（coordinator 仅拆内部长函数，先不拆子目录）
> whisper_mel_matrix.rs 不纳入范围（保持现状，等价于 bin 文件）

## 目标

清理代码审查发现的 4 类问题，**行为零变化**为最高约束：

1. **paddle-ocr 工具函数集中**（重复代码）—— `l2` 3 处、`saturate_cast_i16_from_f32` 2 处完全相同
2. **clamp/clip 命名统一**（语义混淆）—— `clip_i32` hi_exclusive vs `clamp_i32` hi_inclusive
3. **`start_scroll_recording` (502 行) 拆分**（超长函数）
4. **`Coordinator::new` (331 行) / `begin_recording` (228 行) 拆分**（超长函数）

## 不变量（全任务共用）

- **INV-1 行为保持**：所有重构不得改变任何可观察行为。`cargo test --workspace` 全绿是必要条件（非充分）。
- **INV-2 无 API 变化**：`#[tauri::command]` 签名、`pub fn` 接口签名不得变化。
- **INV-3 平台守卫保留**：所有 `#[cfg(target_os = "macos")]` / `#[cfg(not(target_os = "macos"))]` 分支结构必须等价保留。
- **INV-4 无新依赖**：不引入新 crate。允许使用已有 `bytemuck`/`ndarray` 等。
- **INV-5 单 commit 单任务**：每个独立子任务一个 commit，便于 bisect 与回滚。

## TDD 策略矩阵

| 任务 | 类型 | TDD 适用度 | 策略 |
|---|---|---|---|
| #1 paddle-ocr `l2`/`saturate_cast` 集中 | 纯函数迁移 | ★★★ | **严格 TDD**：先写参数化测试锁住所有调用点的边界行为，再迁移到 `vision/numeric.rs` |
| #2 clamp/clip 命名统一 | 纯函数改名 | ★★★ | **严格 TDD**：先写测试覆盖 inclusive/exclusive 两种语义，再改名 |
| #3 `start_scroll_recording` 拆分 | 混合（纯逻辑 + 平台/异步） | ★☆☆ | **分层**：可提取的纯函数（坐标换算、preview crop、显示器命中、截图 thunk）严格 TDD；平台/异步编排靠 `cargo check` + `cargo test --workspace` + 行为保持 review |
| #4 `Coordinator::new` / `begin_recording` 拆分 | 混合（状态机 + 平台） | ★☆☆ | **分层**：状态机辅助逻辑（如引擎模式判定字符串、Stage 转换）提纯函数 TDD；Tauri State/channel/spawn 靠编译器 + 全量测试验证 |

### TDD 红线（适用 #3 / #4 平台层）

- **不引入 trait 抽象 mock 平台 API**（过度工程化，违反 YAGNI）
- **不修改 `#[tauri::command]` 函数签名**（前端 invoke 契约）
- **纯逻辑提取后必须立即写测试**，不允许"先重构后补测试"
- **每个 commit 后必须 `cargo test --workspace --exclude octopus-desktop`** 全绿（desktop 因前端 dist 缺失无法在 worktree 编译，验证方式见 §验证策略）

## 文件结构变化

### 新增
- `crates/paddle-ocr/src/vision/numeric.rs` — 集中 `l2`、`saturate_cast_*`、`cv_round_*`、`clamp/clip` 系列工具函数
- `crates/desktop/src/screenshot_geometry.rs` — `start_scroll_recording` 提取出的纯逻辑（坐标换算、显示器命中、preview crop 参数）

### 修改
- `crates/paddle-ocr/src/vision/mod.rs` — 注册 `pub(crate) mod numeric;`
- `crates/paddle-ocr/src/vision/resize.rs` — 删除本地 `cv_round_ties_even_f32`/`saturate_cast_i16_from_f32`/`clip_i32`，改用 `super::numeric::*`
- `crates/paddle-ocr/src/vision/rotate_crop.rs` — 删除本地 `l2`/`clamp_i32`/`saturate_cast_i16`/`saturate_cast_i16_from_f32`/`saturate_cast_i32_round`/`interpolate_cubic_coeffs`，改用 `super::numeric::*`
- `crates/paddle-ocr/src/rec/word_boxes.rs` — 删除本地 `l2`，改用 `crate::vision::numeric::l2`
- `crates/paddle-ocr/src/det/postprocess/mod.rs` — 删除本地 `l2`，改用 `crate::vision::numeric::l2`
- `crates/desktop/src/screenshot_commands.rs` — `start_scroll_recording` 调用提取出的辅助函数，主体降到 ~150 行
- `crates/desktop/src/coordinator.rs` — `Coordinator::new` 拆出 `build_coordinator_loop`（状态机循环）；`begin_recording` 拆出 `prepare_streaming_session`/`prepare_vad_segmented_session`/`prepare_cloud_streaming_session`

### 不变
- `crates/asr-local/src/whisper_mel_matrix.rs` — 保持现状（预生成常量表）
- `crates/desktop/src/coordinator.rs` — 文件路径不变（Q2=A，不拆子目录）

## 各任务设计

### Task 1: paddle-ocr `vision/numeric.rs`

**现状**（完全相同的实现）：

```rust
// rec/word_boxes.rs:391, det/postprocess/mod.rs:2014, vision/rotate_crop.rs:301
fn l2(a: [f32; 2], b: [f32; 2]) -> f32 {
    let dx = a[0] - b[0];
    let dy = a[1] - b[1];
    (dx * dx + dy * dy).sqrt()
}

// vision/resize.rs:60 与 vision/rotate_crop.rs:204 完全相同
fn saturate_cast_i16_from_f32(v: f32) -> i16 {
    cv_round_ties_even_f32(v).clamp(i16::MIN as i32, i16::MAX as i32) as i16
}
```

**目标**：新建 `vision/numeric.rs`，集中以下函数（均 `pub(crate)`）：

| 函数 | 签名 | 来源 |
|---|---|---|
| `l2` | `(a: [f32;2], b: [f32;2]) -> f32` | 合并 3 处 |
| `cv_round_ties_even_f32` | `(v: f32) -> i32` | resize.rs:46 |
| `saturate_cast_i32_round` | `(v: f64) -> i32` | rotate_crop.rs:176（原 f64 版本，保留） |
| `saturate_cast_i16_from_f32` | `(v: f32) -> i16` | 合并 2 处 |
| `saturate_cast_i16` | `(v: i32) -> i16` | rotate_crop.rs:190 |
| `interpolate_cubic_coeffs` | `(x: f32) -> [f32;4]` | rotate_crop.rs:194 |
| `clip_i32_exclusive_upper` | `(x: i32, lo: i32, hi_exclusive: i32) -> i32` | resize.rs:64 clip_i32 改名 |
| `clamp_i32_inclusive` | `(v: i32, min_v: i32, max_v: i32) -> i32` | rotate_crop.rs:172 clamp_i32 改名 |

**TDD 步骤**：先在 `numeric.rs` 写 `#[cfg(test)] mod tests` 覆盖每个函数的边界（含 NaN/Inf/MIN/MAX），再迁移调用点。

### Task 2: clamp/clip 命名统一

与 Task 1 合并执行（改名发生在迁移过程中）。改名后所有调用点同步更新。

### Task 3: `start_scroll_recording` 拆分

**提取目标**（新建 `screenshot_geometry.rs`）：

| 函数 | 签名 | 来源行 | 可 TDD |
|---|---|---|---|
| `SelectionGeometry` | struct（含 sel_global_x/y, scale, px/py/pw/ph） | 855-905 | ★★★ |
| `compute_selection_global` | `(win_origin_x, win_origin_y, x, y) -> (f64, f64)` | 877-878 | ★★★ |
| `find_monitor_for_point` | `(monitors: &[MonitorRect], cx: f64, cy: f64) -> Option<usize>` | 883-891 | ★★★（输入为纯 struct `MonitorRect { x, y, w, h, scale }`，不含 Tauri 类型） |
| `compute_physical_crop` | `(sel_global_x/y, mon_logical_x/y, scale, w, h) -> (u32,u32,u32,u32)` | 902-905 | ★★★ |
| `compute_preview_crop` | `(canvas_h: u32, canvas_w: u32, preview_w: u32, max_preview_h: u32) -> (crop_src_h: u32, crop_y: u32)` | 1144-1151 + 1212-1215（重复逻辑） | ★★★ |
| `capture_selection_frame` | `fn(target_wid: Option<u32>, exclude_wid: u32, sel_global_x/y, w, h, mon_phys_x/y, px/py/pw/ph) -> Result<RgbaImage>` | 1051-1074 + 1100-1123（重复代码） | ★★（macOS cfg 分支需覆盖，`#[cfg]` 内调用 capx） |

**不提取**（平台/异步/状态，保留在 `start_scroll_recording` 主体）：
- Quartz `CGDisplay::active_displays` / `get_window_number` / `get_window_pid_at_point`
- `tokio::spawn` 生产/消费/watch 编排
- Tauri `set_ignore_cursor_events` / `set_always_on_top` / `emit`
- 鼠标轮询线程
- PNG/WebP 编码 + DB 入库 + 剪贴板写入

**预期效果**：`start_scroll_recording` 主体从 502 行降到 ~150 行（编排 + 平台调用），纯逻辑移到 `screenshot_geometry.rs`（~150 行 + 测试）。

### Task 4: `Coordinator::new` / `begin_recording` 拆分

**`Coordinator::new` (331 行)**：

提取 `build_coordinator_loop`（行 206-520 的 `std::thread::spawn(move || { loop { ... } })` 内部逻辑）为独立函数。`new` 仅做：channel 创建 + config 预处理 + spawn `build_coordinator_loop`。

**`begin_recording` (228 行)**：

按引擎分支拆出 3 个 prepare 函数：
- `prepare_streaming_session` — 行 753-840（`use_streaming && !use_cloud_streaming`）
- `prepare_cloud_streaming_session` — 行 735-752 + cloud 分支（`#[cfg(feature="cloud")]`）
- `prepare_vad_segmented_session` — VAD 分段伪流式分支

`begin_recording` 主体仅做：audio.start + 分支选择 + 调对应 prepare。

**验证**：`coordinator.rs` 文件保持单一，行数从 2439 降到 ~2000（提取后函数更紧凑）。文件路径不变。

## 验证策略

### Worktree 内（desktop 无法编译）

```bash
cargo test --workspace --exclude octopus-desktop
cargo clippy --workspace --exclude octopus-desktop --all-targets
```

### 主仓库（最终验收，desktop 需前端 dist）

```bash
cd /Users/wudarui/workspace/agent/octopus
cargo test --workspace  # desktop 也能编译（dist 存在）
```

### 行为保持 review 检查清单（每个 commit 后）

- [ ] `git diff` 确认无逻辑变化（仅结构重组）
- [ ] 所有 `#[cfg(target_os = "macos")]` 分支保留
- [ ] 所有 `#[tauri::command]` 签名不变
- [ ] 无新增 `unsafe` 块
- [ ] 无新依赖

## 风险与降级

| 风险 | 影响 | 降级 |
|---|---|---|
| paddle-ocr 调用点迁移遗漏 | 编译失败 | 编译器立即报错，逐个修 |
| `start_scroll_recording` 提取后坐标系错位 | 截图区域偏移 | Task 3 前先写坐标换算测试锁住数值 |
| `Coordinator` 状态机拆分后 channel 死锁 | 录音无法启停 | 拆分时保持 `tx`/`rx` 所有权转移不变；全量 `cargo test` |
| desktop crate 无法在 worktree 编译 | 无法跑 desktop 测试 | worktree 跑其他 crate；desktop 测试在主仓库最终验收 |

## 不做

- 不拆 `coordinator.rs` 为子目录（Q2=A，成功后再议）
- 不引入 trait 抽象 mock 平台 API（YAGNI）
- 不动 `whisper_mel_matrix.rs`
- 不合并 `compute_fbank_features`（高风险，AGENTS.md 警告需数值回归）

## 后续扩展（原计划范围外，已实施）

以下两项在初始 4 task 完成后追加，仍遵守行为零变化约束：

### 5. `postprocess/mod.rs` (2226行) 拆分

拆为 7 个子模块：`threshold.rs`（二值化+SIMD）、`contour.rs`（轮廓+膨胀）、`box_score.rs`（得分计算）、`geometry.rs`（最小外接矩形+凸包）、`unclip.rs`（多边形扩展）、`filter.rs`（过滤/排序）、`tests.rs`。`mod.rs` 仅保留 `DbPostProcess` struct/impl + `CandidateScratch`/`ScaleTarget` + 模块声明。纯文件搬移，无逻辑变更。

### 6. 前端超长组件拆分

| 组件 | 之前 | 之后 | 提取 |
|---|---|---|---|
| `Screenshot/index.tsx` | 1170 | 960 | `ToolButton.tsx`、`ToolPropsPopover.tsx`、`ScrollPreview.tsx` |
| `Result/index.tsx` | 869 | 787 | `shortcut.ts`（parseShortcut/matchShortcut）、`CaretBlink.tsx` |
| `ImagePreview/index.tsx` | 850 | 832 | `zoom.ts`（常量+工具函数） |
