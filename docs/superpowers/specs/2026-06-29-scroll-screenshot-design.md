# 截图三期：滚动截屏设计

**日期**: 2026-06-29（2026-06-30 大幅修订）
**状态**: manual 模式已实现并初步可用
**分支**: `feature/clipboard-research`

## 0. 概述

截图三期实现滚动截屏——用户框选区域后点击工具栏「滚动截图」按钮进入录制模式，在选区外手动滚动触控板/滚轮，后端高频截帧 + NCC 模板匹配拼接成长图。

### 核心架构

**置顶透明覆盖层事件穿透 + 独立线程增量捕获 + 底部 strip NCC 模板匹配拼接**

```
前端 React WebView ──1. 挖出透明孔/显示控制栏──▶ Tauri Rust 后端
                                                   │
                   ┌───────────────────────────────┘
                   ├─ 2. 释放 Key Focus / 穿透滚轮事件 ──▶ 目标滚动应用 (Chrome)
                   └─ 3. 高频截图 33fps (CGWindowListCreateImage) ──▶ 拼图模块 (stitch.rs)
                                                                    │
                                          4. 底部 strip NCC 匹配───┘
                                          5. 拼图画布实时更新 ──▶ 前端预览
```

### 已放弃的方案

1. **NSPanel（tauri-nspanel）**：`to_panel()` 的 `object_setClass` swizzling 在 WKWebView 创建后执行 → Trace/BPT trap 崩溃（exit 133）。
2. **auto 模式（CGEvent 模拟滚轮）**：体验差，用户明确需要手动滚动。
3. **简单 deactivate**：`NSApp.deactivate()` 无法可靠让 trackpad scrollWheel 路由到底层应用。
4. **双模板 + PLL 跟踪**：复杂且不稳定，周期性内容导致假匹配。改为单一底部 strip 全局搜索。

## 1. 架构

### 1.1 三层架构

| 层 | 文件 | 职责 |
|---|---|---|
| 前端渲染 | `frontend/src/pages/Screenshot/index.tsx` | 拉框交互；scrolling 模式用 `ctx.clearRect` 挖透明孔；控制栏 |
| 桌面系统管理 | `desktop/src/screenshot_commands.rs` | DPI 坐标映射；焦点让出 + 滚轮穿透；33fps 增量截图循环 |
| 拼图引擎 | `capx/src/stitch.rs` | Sobel 边缘 + 底部 strip NCC 匹配 + 距离惩罚 + 重复帧检测 |

### 1.2 前端：透明孔挖孔

```typescript
// scrolling 模式下的 draw()
if (mode === "scrolling" && sel) {
  ctx.drawImage(bg, 0, 0, cssW, cssH);      // 全屏冻结背景
  ctx.fillStyle = "rgba(0,0,0,0.5)";         // 选区外暗遮罩
  ctx.fillRect(/* 四周 */);
  ctx.clearRect(x, y, w, h);                 // 选区内 100% 透明
  ctx.strokeStyle = "#22c55e";               // 绿色边框
  ctx.strokeRect(x, y, w, h);
}
```

### 1.3 窗口创建

```rust
WebviewWindowBuilder::new(&app_handle, &label, WebviewUrl::default())
    .transparent(true)           // 关键：开启原生 OS 窗口透明度
    .always_on_top(true)         // 置顶
    .decorations(false)
    // ...
```

## 2. 核心问题与解决方案

### 2.1 浏览器滚动挂起（macOS Occlusion Throttling）

**现象**：开始滚动截图时，选区内的 Chrome 页面完全不动。

**根因**：Tauri 窗口默认不透明。当置顶的不透明窗口遮挡底层窗口时，macOS Window Server 的 **Occlusion Throttling** 机制自动挂起底层窗口的 GPU 渲染以省电。

**解决**：
1. `WebviewWindowBuilder` 初始化时 `.transparent(true)`
2. 前端 Canvas 在选区用 `ctx.clearRect` 清空为 100% 原生透明
3. Window Server 识别到底层窗口"可见"，保持 Chrome 高频 repainting

### 2.2 鼠标滚轮无法穿透（Key Window 焦点锁定）

**现象**：即使 `setIgnoresMouseEvents(true)`，滚轮仍无法穿透。

**根因**：macOS Window Server 路由事件时，若截图窗口仍是 **Key Window**，滚轮事件强行发送给 Key Window 的 WebView，不路由到目标窗口。

**解决——协同焦点让出**：
1. 截图启动时 `save_frontmost_app()` 暂存前台应用 PID
2. 滚动录制开始时 `activate_prev_app()` 在主线程激活该应用，使其夺回 Key Focus
3. 若无记录到前台应用，调用 `NSApp.deactivate()`

### 2.3 副屏与 Retina 屏坐标错位（Mixed DPI Mapping）

**解决——Cocoa frame 坐标转换**：
```rust
let (cx, cy, _, ch) = get_window_cocoa_frame(&sel_win);
let win_origin_y = primary_screen_height - (cy + ch);  // Cocoa(左下) → Quartz(左上)
```

裁切公式：
```
Phys_X = (Logic_X - Display_Origin_X) × Scale
Phys_Y = (Logic_Y - Display_Origin_Y) × Scale
```

### 2.4 截图排除 overlay 窗口（CGWindowListCreateImage）

```rust
CGDisplay::screenshot(
    rect,
    kCGWindowListOptionOnScreenBelowWindow,
    exclude_window_id,    // 截图窗口的 windowNumber
    kCGWindowImageDefault,
)
```

只截选区区域（`capture_region_excluding_window`），不截全屏。

## 3. 拼接引擎（stitch.rs）

### 3.1 核心思路

每次新帧到来时，从 **last_edges（上一帧 edges）底部**取一个 strip（模板），在当前帧中搜索最佳匹配位置。该位置即为"上一帧底部内容在当前帧中的位置"，之后到帧底部的内容就是真正新增的像素行。

```
新帧 RGBA
  │
  ├─ 1. 重复帧检测：稀疏采样比较 last_edges vs curr_edges 均值差 < 3.0 → 跳过
  ├─ 2. 计算 curr_edges (Sobel)
  ├─ 3. 从 last_edges 底部向上找首个边缘密度 > 4.0 的 strip 作为模板
  ├─ 4. NCC 匹配搜索：
  │     a. 静态帧短路：0 位移处 score > 0.975 → 判定未滚动
  │     b. 局部搜索：期望位置 ±窗口，距离惩罚 adjusted_score
  │     c. 全局搜索：局部不够好时，阈值提高到 0.85
  │     d. 加速度检查：匹配位置 vs 期望位置差 > 30px → 拒绝
  ├─ 5. 裁剪新内容：crop_start = best_offset + (eff_bottom - tpl_y_start)
  └─ 6. 追加到 canvas + 更新 last_edges / last_scroll
```

### 3.2 数据结构

```rust
pub struct Stitcher {
    canvas: RgbaImage,          // 拼接结果（不断增长的长图）
    last_edges: GrayImage,      // 上一帧 Sobel 边缘图
    match_cols: Range<u32>,     // 匹配列范围（排除窗口边框/滚动条）
    last_scroll: i32,           // 上次滚动位移（用于期望位置预测）
    sticky_top: u32,            // 粘性 header 行数
    sticky_bottom: u32,         // 粘性 footer 行数
    detected: bool,             // sticky/match_cols 是否已初始化
    low_conf_streak: u32,       // 连续匹配失败次数
    config: StitchConfig,
}

pub struct StitchConfig {
    template_ratio: f32,    // 模板高度比例 0.20
    min_confidence: f32,    // NCC 最低阈值 0.65
}
```

### 3.3 搜索窗口策略

- **首帧（last_scroll == 0）**：窗口 `[tpl_y - 20, tpl_y + 5]`，30ms 间隔内位移极小
- **后续帧**：期望位置 `expected_offset = tpl_y_start - last_scroll`，窗口 `[expected - 60, expected + 30]`
- **距离惩罚**：`adjusted_score = score - distance × 0.004`，偏向连续运动
- **全局搜索**：局部 score < 0.65 时全范围搜索，阈值提高到 0.85
- **加速度拒绝**：`|best_offset - expected_offset| > 30` → 拒绝匹配

### 3.4 失锁恢复

连续 3 帧匹配失败 → `last_edges = curr_edges`（用当前帧重锁）+ `last_scroll = 0`（重置位移）。

### 3.5 finalize

录制结束时补全最后一帧的 sticky_bottom 区域（eff_bottom 到 h）。

### 3.6 降级策略

| 场景 | 处理 |
|---|---|
| 画面静止 | 重复帧检测（均值差 < 3.0）→ 跳过 |
| 滚动太快 | 局部搜索失败 → 全局搜索 → 加速度检查拒绝 |
| 追踪连续丢失（≥3帧） | 重锁模板 + 重置位移 |
| 动态内容（广告/动画） | NCC 置信度低 → 帧跳过 |

## 4. 数据流与状态机

### 4.1 前端状态机

```
selected → 点击「滚动截图」→ scrolling
                        │
                 scrolling 模式：
                 - Canvas 选区挖透明孔
                 - 后端：激活前台应用 + 恒定 ignore_cursor_events
                 - 30ms 监视线程：工具栏区域切换 ignore
                 - 工具栏：停止按钮 + 预览
                        │
                 点击「停止」或托盘
                        │
                        ▼
              长图入库 → 关闭截图窗口
```

### 4.2 后端录制循环

```
start_scroll_recording(x, y, w, h, win_label, interactive_rects)
  │
  ├─ 1. set_ignore_cursor_events(true) 恒定
  ├─ 2. activate_prev_app()（让出 Key Focus）
  ├─ 3. 30ms 监视线程（工具栏区域切换 ignore）
  ├─ 4. 初始化 Stitcher（首帧）
  ├─ 5. 循环（30ms / 33fps）：
  │     a. spawn_blocking: capture_region_excluding_window → RGBA
  │     b. JPEG 编码选区画面 → emit("scroll://frame")
  │     c. stitcher.process_frame(rgba)
  │     d. 预览缩略图 → emit("scroll://frame")
  └─ 6. stitcher.finalize() → activate(self) → 长图入库 → 关窗口
```

### 4.3 区域化 cursor events

独立 30ms 轮询线程（与截图循环解耦）：
- 鼠标在 `interactive_rects`（工具栏/预览窗）→ `set_ignore_cursor_events(false)`（可点击）
- 离开 → `set_ignore_cursor_events(true)`（滚动穿透）

## 5. Tauri 命令

```rust
start_scroll_recording(x, y, w, h, win_label, interactive_rects) → ()
stop_scroll_recording() → ()
```

## 6. 依赖

- `imageproc = "0.25"`（Sobel 梯度 + NCC 模板匹配）
- `objc2` + `objc2-app-kit`（NSWindow / NSApplication / NSRunningApplication）
- `core-graphics = "0.24"`（CGDisplay screenshot + CGEvent 鼠标位置）
- 已有：`image`、`xcap`

## 7. 后续优化方向

1. **模板匹配并行化**：`rayon` 多线程或 Metal/Vulkan Compute Shader
2. **卡尔曼滤波位移预测**：基于最近 3 帧 dy 速度/加速度预测下一帧
3. **动态捕获间隔**：滚动事件监听驱动捕获，无滚动时降频防抖
4. **多窗口架构**（可选）：拆分全屏穿透窗口 + 独立工具栏窗口，消除 30ms 竞态
