# 截图三期：滚动截屏设计

**日期**: 2026-06-29
**状态**: 引擎已完成（stitch.rs + 录制循环），auto 模式待实施
**分支**: `feature/clipboard-research`

## 0. 概述

截图三期实现滚动截屏——用户框选区域后点击工具栏「滚动截图」按钮进入录制模式。

### 两种滚动模式（通过 `scroll_mode` 配置切换）

**auto 模式**（默认，推荐）：
- 用户点击「滚动截图」后，后端用 `CGEventCreateScrollWheelEvent` 自动模拟滚轮
- 用户只需等录制完成或手动点击「停止」
- 完全绕过窗口焦点/穿透问题——不需要 `set_ignore_cursor_events`、不需要 cursor 区域切换
- 截图窗口保持显示（暗遮罩 + 选区绿色边框 + 实时画面 + 右侧预览）
- 恒定滚动速度，NCC 匹配率高

**manual 模式**（高级选项）：
- 截图窗口保持为普通 WebviewWindow（不用 NSPanel）
- 截屏时使用 `CGWindowListCreateImage` + `kCGWindowListOptionOnScreenBelowWindow` 排除截图窗口本身
- `set_ignore_cursor_events(true)` 让滚轮穿透到底层应用
- CGEvent 轮询鼠标位置，工具栏区域临时关闭穿透
- 用户手动滚动触控板，滚轮事件自然路由到底层应用
- 体验类似 Xnip

### 为什么放弃 NSPanel 方案

NSPanel 方案（`tauri-nspanel` PanelBuilder / `to_panel()`）在实现中验证发现：
- `to_panel()` 对已有 WebviewWindow 做 class swizzling（`object_setClass`），在 WKWebView 已创建后执行会导致 **Trace/BPT trap 崩溃**（exit code 133）
- `PanelBuilder` 内部也调用 `to_panel()`，同样崩溃
- 即使不崩溃，NonactivatingPanel + `always_on_top` 的焦点竞争问题仍存在

**新方案用 CGWindowList 排除截图窗口**，从截图中根本消除 overlay 干扰，无需改变窗口类型。

### 为什么 auto 优先

- macOS 的 `always_on_top` 窗口与滚轮穿透有天然冲突（焦点被截图窗口抢占）
- NSPanel 方案需要 `tauri-nspanel` 集成调试，复杂度高
- auto 模式拼接更平滑（恒定速度），用户体验一致
- manual 模式作为后续高级选项
- 用户在任意位置滚动触控板，底层应用跟着滚动，选区内的画面变化被拼接
- 点击工具栏「停止」或按 ESC 结束，长图入库

基于 `imageproc`（Sobel 梯度 + NCC 模板匹配）实现帧间拼接，参考 DigitShot 的 `stitch.rs`。`crates/capx/src/stitch.rs` 拼接引擎已实现。

## 1. 架构（修订）

```
选区确定 → 点击工具栏「滚动截图」按钮
         │
         ▼
┌─────────────────────────────────────────────────┐
│  1. 进入滚动录制模式                              │
│     - 截图窗口 set_ignore_cursor_events(true)     │
│     - 后端启动录制循环（spawn_blocking 截图）       │
│     - 工具栏按钮变为「停止」                        │
│     - 选区边框变为绿色（录制中）                    │
│     - 右侧弹出预览窗口                             │
├─────────────────────────────────────────────────┤
│  2. 用户手动滚动触控板（任意位置）                  │
│     - 滚轮穿透截图窗口到底层应用                    │
│     - 后端 10fps 截取选区 → NCC 匹配 → 拼接        │
│     - Canvas 每帧重绘选区（显示实时画面）           │
│     - 预览窗口实时更新拼接长图                      │
├─────────────────────────────────────────────────┤
│  3. 用户点击工具栏「停止」或按 ESC                  │
│     - set_ignore_cursor_events(false) 恢复         │
│     - 生成最终长图 → 入库 → 关窗口                  │
└─────────────────────────────────────────────────┘
```

### 核心难点：ignore cursor 后工具栏可交互

截图窗口整体 `ignore_cursor_events(true)` 后，工具栏/预览/停止按钮也无法点击。解决方案：

**方案：区域化 cursor events**（macOS 原生支持）
- 前端每帧 `mousemove` 时检测鼠标是否在工具栏/预览区域
- 在工具栏区域 → `set_ignore_cursor_events(false)`（可交互）
- 离开工具栏区域 → `set_ignore_cursor_events(true)`（滚轮穿透）

前端通过 invoke 调 `set_cursor_passthrough(bool)` 命令动态切换。

### 新增模块

```
crates/capx/src/stitch.rs           # 拼接引擎：NCC 匹配 + 粘性检测 + 图像拼接
crates/desktop/src/screenshot_commands.rs  # start/stop_scroll_recording 命令
crates/desktop/frontend/src/pages/Screenshot/
  ├── ScrollPreview.tsx             # 实时预览组件（DOM 浮层）
  └── index.tsx                     # 滚动模式状态机扩展
```

### 依赖

- `imageproc = "0.25"`（Sobel 梯度 + NCC 模板匹配）
- 已有：`image`、`xcap`

## 2. 拼接引擎（stitch.rs）

### 2.1 数据结构

```rust
pub struct Stitcher {
    canvas: RgbaImage,          // 当前拼接结果（不断增长的长图）
    last_frame: GrayImage,      // 上一帧的边缘图（Sobel 梯度）
    sticky_top: u32,            // 粘性 header 高度（像素行数）
    sticky_bottom: u32,         // 粘性 footer 高度
    active_cols: Range<u32>,    // 活跃列范围（排除静态侧边栏）
    last_delta: i32,            // 上次重叠量（惯性预测）
    low_conf_streak: u32,       // 连续低置信帧数
}

pub struct StitchConfig {
    template_ratio: f32,    // 模板高度 = 有效高度 × 0.2
    min_confidence: f32,    // NCC 最低阈值 0.5
    inertia_px: i32,        // 惯性搜索窗口 ±100
    max_lowconf_streak: u32,// 连续低置信上限 8 帧
}
```

### 2.2 处理一帧的流程

```
新帧 RGBA
  │
  ├─ 1. 预处理：RGBA → 灰度 → Sobel 梯度
  ├─ 2. 首帧：初始化 sticky header/footer + active cols
  │     - sticky_top：比较首帧和第二帧，找顶部不变的行数
  │     - sticky_bottom：同理底部
  │     - active_cols：比较两帧差异，定位变化的列范围
  ├─ 3. 重复帧检测：稀疏采样比较（step=8），均值 < 2.0 → 跳过
  ├─ 4. NCC 模板匹配：
  │     - 从上一帧底部取模板（高度 = 有效高度 × 20%）
  │     - 在当前帧顶部 ±inertia_px 范围内搜索
  │     - 置信度 ≥ 0.5 → 命中；< 0.5 → 全范围重搜
  ├─ 5. 拼接：裁剪当前帧的非重叠行 → 追加到 canvas
  └─ 6. 更新状态：last_delta、缓存边缘图
```

### 2.3 粘性 header/footer 处理

- 初始化时检测（首对帧逐行比较，相同的顶部行 = sticky header）
- 每帧裁掉 sticky 区域后再做匹配
- 最终长图只在最顶部和最底部各保留一次

### 2.4 活跃列检测

- 比较两帧差异，哪些列在变化 = 滚动内容区域
- 只在活跃列范围内做 NCC（排除静态侧边栏干扰）

### 2.5 降级策略

- 连续低置信帧 < 8：保持上次 delta 硬拼接
- 连续低置信帧 ≥ 8：停止拼接，emit 警告，等待用户停止

## 3. 实时预览窗口（ScrollPreview）

### 3.1 位置

默认选区右侧，空间不足放左侧（参考 Xnip 布局）：
- `previewRight = sel.x + sel.w + 12 + 200 <= window.innerWidth`
- 右侧：`x = sel.x + sel.w + 12`
- 左侧：`x = sel.x - 12 - 200`
- `y = sel.y`

```
┌────────┬──────────┐
│        │ 预览     │
│ 选区   │ ┌──────┐ │
│（实时） │ │ 长图  │ │
│        │ │      │ │
├────────│ └──────┘ │
│ 工具栏 │ 1234px   │
└────────┴──────────┘
```

### 3.2 属性

- 宽度固定 200px（选区宽度的缩略图）
- 高度自适应内容（随拼接增长，最大不超屏幕高度的 80%）
- 顶部状态条：绿色圆点 + 「录制中」+ 已拼接高度（px）
- 追踪丢失：红色圆点 + 「追踪丢失」
- 拼接图像通过 `canvas.toDataURL()` 缩放到预览宽度渲染

### 3.3 更新频率

- 后端拼接引擎每处理一帧 → emit `scroll://frame`（含选区实时画面 base64 + 预览 base64）
- 前端监听事件 → Canvas 重绘选区 + 预览 `<img>` 替换 src

## 4. 数据流与状态机（修订）

### 4.1 前端状态机

```
Mode: "scrolling"

selected → 点击「滚动截图」按钮 → scrolling
    │                                    │
    │     ┌──────────────────────────────┤
    │     │ scrolling 模式：              │
    │     │ - 后端：spawn_blocking 录制   │
    │     │ - 截图窗口 ignore_cursor=true │
    │     │ - Canvas 显示实时画面         │
    │     │ - 工具栏：停止 + 预览         │
    │     │ - 鼠标在工具栏区域时恢复交互   │
    │     └──────────────────────────────┤
    │                                    │
    │                              点击「停止」或 ESC
    │                                    │
    ├←───────────────────────────────────┘
    │
    ▼
长图入库 → 关闭截图窗口
```

### 4.2 后端录制循环（已实现，修订 emit 内容）

```
start_scroll_recording(x, y, w, h)
  │
  ├─ 1. set_ignore_cursor_events(true)
  ├─ 2. 初始化 Stitcher（首帧）
  ├─ 3. 循环（10fps，spawn_blocking）：
  │     a. capture_all_monitors → crop_region → RGBA
  │     b. stitcher.process_frame(rgba)
  │     c. emit("scroll://frame", {
  │          frame: base64,       // 选区实时画面（前端 Canvas 重绘）
  │          preview: base64,     // 拼接长图缩略图
  │          height: pixels       // 拼接高度
  │        })
  └─ 4. stop → ignore_cursor(false) → 长图入库 → 关窗口
```

### 4.3 区域化 cursor events

前端 mousemove 时：
```typescript
if (mode === "scrolling") {
  const inToolbar = mouseY >= toolbarY && mouseY <= toolbarY + 44;
  const inPreview = mouseX >= previewX && mouseX <= previewX + 200;
  const needInteractive = inToolbar || inPreview;
  invoke("set_cursor_passthrough", { passthrough: !needInteractive });
}
```

Tauri 命令：
```rust
#[tauri::command]
pub fn set_cursor_passthrough(passthrough: bool, app_handle: tauri::AppHandle) {
    if let Some(win) = /* 当前截图窗口 */ {
        let _ = win.set_ignore_cursor_events(passthrough);
    }
}
```

### 4.4 Tauri 命令

```rust
start_scroll_recording(x, y, w, h) → ()  // 已实现
stop_scroll_recording() → ()              // 已实现
set_cursor_passthrough(bool) → ()         // 新增
```

## 5. 错误处理与边界

| 场景 | 处理 |
|---|---|
| 用户滚动太快 | 帧间重叠 < 20% → 置信度低于阈值 → 该帧丢弃不拼接 |
| 追踪连续丢失（≥8帧） | 预览窗口红色「追踪丢失」+ 选区边框变红 |
| 动态内容（广告/动画） | NCC 置信度低 → 帧被跳过，长图可能有缺口 |
| 水平偏移 | 检测到水平 delta > 5px → 该帧丢弃（仅支持垂直） |
| 选区含固定 header | 初始化时检测并裁剪，每帧跳过 sticky 区域 |
| 反向滚动（向上） | delta 为负 → 跳过该帧（不支持向上修正） |
| 长图过大（> 10000px） | 拼接正常但预览窗口限制显示高度，最终入库不受限 |
| 停止后选区内容 | 长图替换原选区底图，可继续标注/确认/保存 |
| 截图窗口关闭 | 录制循环自动停止（窗口不存在检测） |

**降级**：滚动截图失败不影响普通截图功能。start_scroll_recording 返回 Err → 回到 selected 模式 + toast 提示。

## 6. Auto 模式实现（优先）

### 6.1 配置

```sql
INSERT OR IGNORE INTO app_config (config_key, config_value, description) VALUES
  ('scroll_mode', 'auto', '滚动截图模式: auto | manual');
```

AppConfig 新增 `scroll_mode` 字段（默认 `auto`）。

### 6.2 后端：CGEvent 模拟滚轮

```rust
/// auto 模式：模拟一次向下滚轮事件
#[cfg(target_os = "macos")]
fn send_scroll_event(lines: i32) {
    use core_graphics::event::{CGEvent, CGEventType, ScrollEventUnit};
    use core_graphics::event_source::{CGEventSource, CGEventSourceStateID};
    let source = CGEventSource::new(CGEventSourceStateID::HIDSystemState).unwrap();
    let event = CGEvent::new_scroll_wheel_event2(
        &source,
        ScrollEventUnit::Pixel, // 像素级滚动（更精细）
        lines,                  // 垂直方向
        0,                      // 水平方向
    ).unwrap();
    event.post(CGEventTapLocation::Session);
}
```

### 6.3 auto 模式录制流程

```
start_scroll_recording(x, y, w, h, mode="auto")
  │
  ├─ 1. 初始化 Stitcher（首帧）
  ├─ 2. 循环（每帧间隔 100ms）：
  │     a. send_scroll_event(scroll_step)  // 模拟滚轮（在截屏前）
  │     b. sleep(50ms)                     // 等待应用响应滚动
  │     c. spawn_blocking: capture + crop → RGBA
  │     d. stitcher.process_frame(rgba)
  │     e. emit("scroll://frame", { frame, preview, height })
  │     f. 如果连续 3 帧无新内容（到底了）→ 自动停止
  ├─ 3. stop → 长图入库 → 关窗口
  └─ 不需要 set_ignore_cursor_events、不需要 cursor 区域切换
```

**关键参数**：
- `scroll_step`：每次滚 40 像素（约 2 行），可调
- `auto_stop_threshold`：连续 3 帧无新内容自动停止

### 6.4 manual 模式（CGWindowList 排除 + set_ignore_cursor_events 实现）

**目标**：截图窗口保持普通 WebviewWindow，截屏时用 CGWindowList 排除自身，滚轮用 `set_ignore_cursor_events` 穿透。

#### 核心原理

1. **CGWindowListCreateImage 排除 overlay 窗口**
   - macOS 截屏 API `CGWindowListCreateImage(bounds, option, windowID, imageOption)`
   - `kCGWindowListOptionOnScreenBelowWindow` + 截图窗口的 `windowNumber` → 只截该窗口下方的所有窗口（排除截图窗口自身）
   - 截到的内容 = 底层应用的真实画面（不含截图 overlay）
   - `crates/capx/src/capture.rs::capture_display_excluding_window(display_id, exclude_window_id)`

2. **获取 NSWindow windowNumber**
   - `tauri::WebviewWindow::ns_window()` → NSWindow 指针
   - `[nsWindow windowNumber]` → u32 windowID
   - `screenshot_commands.rs::get_window_number()`

3. **set_ignore_cursor_events(true) → 滚轮穿透**
   - Tauri 原生 API，不需要 NSPanel
   - true 时所有鼠标事件（含滚轮）穿透到底层应用
   - 底层应用保持键盘焦点 → 滚轮事件到达

4. **工具栏可交互（区域化切换）**
   - 后端每帧用 CGEvent 获取全局鼠标位置
   - 转窗口局部坐标：全局 - `outer_position()`
   - 鼠标在工具栏区域 → `set_ignore_cursor_events(false)` 恢复点击
   - 离开工具栏 → `set_ignore_cursor_events(true)` 恢复穿透

#### 坐标系

- 前端 `x, y` = CSS 逻辑像素（窗口局部）
- 选区全局逻辑坐标 = `outer_position() + (x, y)`
- Tauri Monitor 物理坐标 → 逻辑坐标 = `position() / scale_factor()`
- crop 物理坐标 = `(全局逻辑 - 显示器逻辑偏移) × scale`
- CGDisplay bounds = 全局逻辑坐标（points）

#### 依赖变更

- **移除** `tauri-nspanel`（不再需要）
- **新增** `crates/capx` → `core-graphics = "0.24"` + `core-foundation = "0.10"`（macOS only）

## 7. 副屏坐标修正

诊断报告确认的坐标问题：
- `start_scroll_recording` 的 x/y 是窗口局部坐标
- CGEvent 鼠标位置是全局坐标系
- 录制时应记录 monitor 偏移，crop 时用偏移修正

修复：start 时获取对应 monitor 的 `position()`，crop 时用物理坐标匹配正确的 capture。

## 8. 实施分期（修订）

| 阶段 | 范围 | 状态 |
|---|---|---|
| **Step 1** | capx/stitch.rs 拼接引擎 | ✅ 已完成 |
| **Step 2** | 后端录制循环（spawn_blocking + emit） | ✅ 已完成 |
| **Step 3** | 前端 scrolling 模式 + 工具栏 | ✅ 已完成 |
| **Step 4** | auto 模式（CGEvent 模拟滚轮 + 自动停止 + 配置） | ✅ 已实现（体验待改善） |
| **Step 5** | 副屏坐标修正（monitor 偏移） | ✅ 已实现 |
| **Step 6** | manual 模式（NSPanel NonactivatingPanel + ignores_mouse_events + CGEvent 区域化） | 🔧 本轮实现 |

## 7. 风险

| 风险 | 概率 | 影响 | 缓解 |
|---|---|---|---|
| NCC 匹配性能不足 | 中 | 帧率下降 | coarse-to-fine 搜索 + 活跃列裁剪 |
| 粘性元素检测不准 | 中 | 长图有重复 | 手动阈值调整 + 用户可接受小幅重复 |
| xcap capture_region 权限 | 低 | 无法截取选区 | 已有一期权限验证 |
| 预览窗口性能 | 低 | 卡顿 | 限制预览更新频率到 15fps |
