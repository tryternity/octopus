# 截图三期：滚动截屏设计

**日期**: 2026-06-29（2026-06-30 大幅修订）
**状态**: manual 模式已实现并初步可用
**分支**: `feature/clipboard-research`

## 0. 概述

截图三期实现滚动截屏——用户框选区域后点击工具栏「滚动截图」按钮进入录制模式，在选区外手动滚动触控板/滚轮，后端高频截帧 + NCC 模板匹配拼接成长图。

### 核心架构

**置顶透明覆盖层事件穿透 + 独立线程增量捕获 + 双模板特征动态锁定 + 锁相环滚动跟踪拼图**

```
前端 React WebView ──1. 挖出透明孔/显示控制栏──▶ Tauri Rust 后端
                                                   │
                   ┌───────────────────────────────┘
                   ├─ 2. 释放 Key Focus / 穿透滚轮事件 ──▶ 目标滚动应用 (Chrome)
                   └─ 3. 定期截图 (CGWindowListCreateImage) ──▶ 拼图模块 (stitch.rs)
                                                                    │
                                          4. 特征投影定位 / PLL 跟───┘
                                          5. 拼图画布实时更新 ──▶ 前端预览
```

### 已放弃的方案

1. **NSPanel（tauri-nspanel）**：`to_panel()` 的 `object_setClass` swizzling 在 WKWebView 创建后执行 → Trace/BPT trap 崩溃（exit 133）。NonactivatingPanel 还会阻断 IME 输入。
2. **auto 模式（CGEvent 模拟滚轮）**：机械上可行但体验差（截到截图窗口自身 + 副屏坐标错 + 不可控速度）。
3. **简单 deactivate**：`NSApp.deactivate()` 无法可靠让 trackpad scrollWheel 路由到底层应用。

## 1. 架构

### 1.1 三层架构

| 层 | 文件 | 职责 |
|---|---|---|
| 前端渲染 | `frontend/src/pages/Screenshot/index.tsx` | 拉框交互；scrolling 模式用 `ctx.clearRect` 挖透明孔；控制栏 |
| 桌面系统管理 | `desktop/src/screenshot_commands.rs` | DPI 坐标映射；焦点让出 + 滚轮穿透；增量截图循环 |
| 拼图引擎 | `capx/src/stitch.rs` | Sobel 边缘 + NCC 模板匹配 + PLL 位移跟踪 + 双模板一致性校验 |

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

**根因**：Tauri 窗口默认不透明。当置顶的不透明窗口遮挡底层窗口时，macOS Window Server 的 **Occlusion Throttling** 机制自动挂起底层窗口的 GPU 渲染以省电。Chrome 停止 repainting 后看起来卡死，也不响应滚轮渲染。

**解决**：
1. `WebviewWindowBuilder` 初始化时 `.transparent(true)`
2. 前端 Canvas 在选区用 `ctx.clearRect` 清空为 100% 原生透明
3. Window Server 识别到底层窗口"可见"，保持 Chrome 高频 repainting

### 2.2 鼠标滚轮无法穿透（Key Window 焦点锁定）

**现象**：即使 `setIgnoresMouseEvents(true)`，滚轮仍无法穿透。

**根因**：macOS Window Server 路由事件时，若截图窗口仍是 **Key Window**，即使设置 Click-through（忽略鼠标事件），滚轮事件仍强行发送给 Key Window 的 WebView，不路由到目标窗口。

**解决——协同焦点让出**：
1. 截图启动时 `save_frontmost_app()` 暂存前台应用 PID（如 Chrome）
2. 滚动录制开始时 `activate_prev_app()` 在主线程激活该应用，使其夺回 Key Focus
3. 若无记录到前台应用，调用 `NSApp.deactivate()`
4. 窗口失去 Key 状态后，滚轮事件穿透透明孔到达 Chrome

```rust
// save_frontmost_app: NSWorkspace.sharedWorkspace().frontmostApplication()
// activate_prev_app: app.activateWithOptions(NSApplicationActivationOptions(1 << 1))
```

### 2.3 副屏与 Retina 屏坐标错位（Mixed DPI Mapping）

**根因**：前端 WebView 用逻辑像素坐标；macOS CGEvent/Quartz 用物理像素或基于主屏左上角的点坐标。副屏 origin 偏移 + Retina scale 导致坐标膨胀。

**解决——Cocoa frame 坐标转换**：
```rust
// 获取 NSWindow 的 Cocoa frame（原点在左下角）
let (cx, cy, _, ch) = get_window_cocoa_frame(&sel_win);
// 翻转为 Quartz 坐标（原点在左上角）
let win_origin_y = primary_screen_height - (cy + ch);
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
    kCGWindowListOptionOnScreenBelowWindow,  // 只截该窗口下方
    exclude_window_id,                        // 截图窗口的 windowNumber
    kCGWindowImageDefault,
)
```

只截选区区域（`capture_region_excluding_window`），不截全屏，性能提升约 10×。

## 3. 拼接引擎（stitch.rs）

### 3.1 数据结构

```rust
pub struct Stitcher {
    canvas: RgbaImage,          // 拼接结果（不断增长的长图）
    last_edges: GrayImage,      // 上一帧 Sobel 边缘图
    sticky_top: u32,            // 粘性 header 行数
    sticky_bottom: u32,         // 粘性 footer 行数
    active_cols: Range<u32>,    // 活跃列范围
    match_cols: Range<u32>,     // 黄金匹配列（边缘投影最密集的 200px）
    last_dy: i32,               // 上次滚动位移（PLL 跟踪基准）
    low_conf_streak: u32,       // 连续低置信帧数
    config: StitchConfig,
    frame_count: u32,
}
```

### 3.2 黄金列锁定（match_cols）

第二帧时，在 `active_cols` 内对 Sobel 边缘灰度做水平投影，找边缘能量最密集的 200px 列宽区域。后续所有 NCC 匹配锁定在此列，避开空旷区域（如 Commit 备注、日期列），提高唯一性。

### 3.3 双模板动态 Y 寻优（find_best_template_y）

在有效区域底部 1/3 和中上部 2/3 各找一个边缘能量最大的 Y 坐标作为模板。双模板必须同时匹配成功且位移一致才追加帧——防撕裂和周期性混淆。

### 3.4 锁相环位移跟踪（PLL-Style match_template）

```
局部跟踪：dy ∈ [last_dy - 20, last_dy + 80]
  → 窗口宽度 100px < 列表行周期（~45px × N）
  → 物理上排除匹配到隔壁行的假极值

失锁重捕：局部置信度 < min_confidence
  → 降级全局搜索重锁
  → 锁死后恢复局部跟踪
```

### 3.5 严格置信度过滤 + 裁剪修正

- 低置信帧一律丢弃（不再猜测位移）
- 裁剪起点 `new_start + tpl_h`（修正了原版模板重叠导致的行重复）

### 3.6 降级策略

| 场景 | 处理 |
|---|---|
| 用户滚动太快 | 重叠 < 20% → 置信度低 → 帧丢弃 |
| 追踪连续丢失（≥8帧） | 更新 last_edges，等待重锁 |
| 动态内容（广告/动画） | NCC 置信度低 → 帧跳过 |
| 反向滚动（向上） | delta 为负 → PLL 窗口排除 → 帧丢弃 |
| 重复帧 | 稀疏采样均值 < 2.0 → 跳过 |

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
  ├─ 5. 循环（120ms / 8fps）：
  │     a. spawn_blocking: capture_region_excluding_window → RGBA
  │     b. JPEG 编码选区画面 → emit("scroll://frame")
  │     c. stitcher.process_frame(rgba)
  │     d. 预览缩略图 → emit("scroll://frame")
  └─ 6. stop → activate(self) → 长图入库 → 关窗口
```

### 4.3 区域化 cursor events

独立 30ms 轮询线程（与截图循环解耦）：
- 鼠标在 `interactive_rects`（工具栏/预览窗）→ `set_ignore_cursor_events(false)`（可点击）
- 离开 → `set_ignore_cursor_events(true)`（滚动穿透）

前端启动时传递 `interactiveRects: [{x, y, width, height}, ...]`。

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
2. **卡尔曼滤波位移预测**：基于最近 3 帧 dy 速度/加速度预测下一帧，自适应搜索窗口
3. **动态捕获间隔**：滚动事件监听驱动捕获，无滚动时降频防抖
4. **多窗口架构**（可选）：拆分全屏穿透窗口 + 独立工具栏窗口，消除 30ms 竞态
