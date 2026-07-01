# 原生 NSView 滚动截屏验证设计

**日期**: 2026-07-01
**状态**: 设计完成，待实施
**分支**: `feature/clipboard-research`（验证期间禁止同步到 main）
**目标**: 用原生 NSView/NSWindow 替换 WebView 实现滚动截屏，验证滚轮穿透/截图排除/拼接的可靠性

## 0. 概述

独立 crate `crates/scroll-capture/` 实现原生 NSView 滚动截屏。不碰现有 `screenshot_commands.rs` 代码。托盘菜单「滚动截屏」直接触发。

验证完成后逐步替换现有 WebView 截图 UI。

## 1. 文件结构

```
crates/scroll-capture/
├── Cargo.toml          # 依赖 capx + rustfft + objc2 + objc2-app-kit
├── src/
│   ├── lib.rs          # 公共接口（start/stop）+ 回调类型
│   ├── overlay.rs      # ScrollOverlay trait + MonitorInfo
│   ├── recording.rs    # 录制循环（截图 + FFT 拼接 + finalize）
│   └── macos/
│       ├── mod.rs      # macOS 模块入口
│       ├── overlay_window.rs  # NSScrollOverlayWindow（NSWindow 子类）
│       ├── overlay_view.rs    # NSScrollOverlayView（NSView 子类，拉框 + 绘制）
│       └── capture.rs  # CGWindowList 截图 + 排除自身
```

## 2. 公共接口

```rust
// lib.rs

/// 启动滚动截屏：创建全屏覆盖窗口，等待用户拉框选区
/// on_complete: 录制完成后回调，传入最终 PNG bytes
pub fn start(on_complete: Box<dyn FnOnce(Vec<u8>) + Send + 'static>);

/// 停止滚动录制（托盘菜单/ESC 触发）
pub fn stop();
```

desktop crate 的 `main.rs` 注册一个 Tauri 命令调用 `start()`，托盘菜单加「滚动截屏」入口。

## 3. 跨平台 trait

```rust
// overlay.rs

pub struct MonitorInfo {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
    pub scale_factor: f64,
}

pub trait ScrollOverlay: Send {
    /// 为每个显示器创建全屏透明覆盖窗口
    fn create(monitors: &[MonitorInfo]) -> Self;
    /// 设置滚轮穿透（true = 滚轮穿透到底层应用）
    fn set_scroll_through(&self, enabled: bool);
    /// 获取所有覆盖窗口的 ID（用于 CGWindowList 排除）
    fn window_ids(&self) -> Vec<u64>;
    /// 销毁所有覆盖窗口
    fn destroy(self);
}
```

## 4. 选区拉框（NSView 原生）

### 4.1 NSScrollOverlayView（继承 NSView）

```
状态机：idle → selecting → selected → recording → done

mouseDown:
  → 状态 = selecting
  → 记录起点
  → setNeedsDisplay

mouseDragged:
  → 更新选区矩形
  → setNeedsDisplay（重绘遮罩 + 绿色边框）

mouseUp:
  → 如果选区 < 10px，状态 = idle，重绘
  → 否则状态 = selected
  → 通知 RecordingController 开始录制

draw:
  → idle: 全屏半透明黑色（rgba 0,0,0,0.5）
  → selecting/selected: 选区外半透明黑色，选区内透明 + 绿色边框
  → recording: 只画绿色边框（选区内外都透明）
```

### 4.2 NSScrollOverlayWindow（继承 NSWindow）

```objc
NSWindow:
  - styleMask: .borderless
  - level: .floating (3)
  - isOpaque: false
  - backgroundColor: clearColor
  - hasShadow: false
  - canBecomeKey: true
  - acceptsMouseMovedEvents: true
```

`canBecomeKey = true` 是关键——让覆盖窗口能接收键盘事件（ESC 停止）。

### 4.3 keyDown（ESC 停止）

```objc
keyDown:
  → 如果 keyCode == 53 (Escape)
  → 通知 RecordingController 停止
```

## 5. 滚动录制

### 5.1 录制流程

```
选区确定（selected 状态）
  → set_scroll_through(true) 所有覆盖窗口
  → 激活选区下方应用（get_window_pid_at_point + activateWithOptions）
  → sleep 120ms 等应用激活
  → 首帧截图 → Stitcher::new
  → 循环（100ms / 10fps）：
     a. spawn_blocking: CGWindowListCreateImage 截选区区域（排除覆盖窗口 windowNumber）
     b. stitcher.process_frame(rgba)
     c. （一期无预览 emit）
  → stop / ESC → finalize → on_complete(png_bytes)
  → destroy 覆盖窗口
```

### 5.2 截图排除

```rust
// macos/capture.rs
pub fn capture_region_excluding(
    exclude_window_ids: &[u64],  // 覆盖窗口的 windowNumber 列表
    rect_x: f64, rect_y: f64,    // 全局 Quartz 逻辑坐标
    rect_w: f64, rect_h: f64,
) -> Result<RgbaImage>
```

用 `CGWindowListCreateImage(rect, kCGWindowListOptionOnScreenBelowWindow, firstWindowID, ...)`。
多个覆盖窗口时用第一个排除（同一 app 的窗口在同一层级）。

### 5.3 焦点让出

```
get_window_pid_at_point(sel_center_x, sel_center_y)
  → CGWindowListCopyWindowInfo + bounds 命中
  → 跳过 kCGWindowLayer != 0
  → 跳过自己的 PID
  → 返回底层应用的 PID

activate_app_by_pid(pid)
  → NSRunningApplication.activateWithOptions(NSApplicationActivationOptions(1 << 1))
  → 在主线程执行（run_on_main_thread）
```

### 5.4 坐标转换

- 前端不用了（原生 NSView 处理）
- 选区坐标：NSView 的 `mouseDown.locationInWindow` → 窗口 frame 偏移 → 全局 Quartz 坐标
- CGWindowListCreateImage 用 Quartz 逻辑坐标
- crop 用物理坐标（× scale_factor）

## 6. 拼接引擎

复用 `crates/capx/src/stitch.rs`（FFT 相位相关），不做修改。
配置：`min_confidence = 0.5`，`min_scroll_px = 2.0`。

## 7. 托盘入口

```rust
// desktop/src/main.rs 新增命令
#[tauri::command]
fn start_scroll_capture(app_handle: tauri::AppHandle) {
    scroll_capture::start(Box::new(move |png_bytes| {
        // 入库：insert_image_data + insert_clipboard_item + emit clipboard://changed
    }));
}

#[tauri::command]
fn stop_scroll_capture() {
    scroll_capture::stop();
}
```

托盘菜单加「滚动截屏」→ `start_scroll_capture`。
托盘菜单加「停止滚动」→ `stop_scroll_capture`（录制时显示）。

## 8. 依赖

```toml
[dependencies]
octopus-capx = { path = "../capx" }       # capture + stitch
octopus-clipboard = { path = "../clipboard" }  # 入库（回调中使用）
rustfft = "6.2"
image = "0.25"
anyhow = "1"
log = "0.4"

[target.'cfg(target_os = "macos")'.dependencies]
objc2 = "0.6"
objc2-app-kit = { version = "0.3", features = [...] }
objc2-foundation = { version = "0.3", features = [...] }
core-graphics = "0.24"
core-foundation = "0.10"
```

## 9. 二期扩展

| 功能 | 方案 |
|---|---|
| 预览面板 | NSView 子类，底部固定，显示拼接进度 + 保存/复制/取消按钮 |
| Windows 实现 | HWND + WS_EX_LAYERED + WS_EX_TRANSPARENT + SetWindowDisplayAffinity |
| Linux/X11 | override-redirect window + XPutImage |
| 标注工具 | 原生 NSView draw 重写（矩形/椭圆/箭头/文字/序号） |
| 替换现有 WebView 截图 | 全部迁移到原生 NSView |
