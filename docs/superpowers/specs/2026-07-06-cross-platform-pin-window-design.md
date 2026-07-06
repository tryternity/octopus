# 跨平台贴图窗口（Pin Window）设计规格

**日期**：2026-07-06  
**作者**：Antigravity  
**范围**：在 `crates/desktop/src/pin_window.rs` 中为 Windows 和 Linux 实现原生的贴图窗口（Pin Window）功能，与已实现的 macOS 方案对齐。

---

## 1. 架构设计与接口

已有的 `PinWindow` trait 定义如下：

```rust
pub trait PinWindow {
    fn create(png_data: &[u8], x: f64, y: f64, width: f64, height: f64);
}
```

所有的平台实现都需要实现该接口：
- `png_data`：截图裁剪后的 PNG 字节数据。
- `x`, `y`：贴图窗口在屏幕上的初始**逻辑坐标**（Top-Left 起点）。
- `width`, `height`：贴图窗口的初始**逻辑宽高**。

所有系统中的拖拽、缩放、右键关闭都是通过操作系统底层的原生事件驱动，绕过任何 Webview 渲染，保持单窗口内存增量极低（< 5MB）。

---

## 2. Windows 原生实现设计 (Win32)

### 2.1 窗口属性配置
使用 Windows 的原生 Win32 API：
- **Window Class**：注册一个名为 `OctopusPinWindow` 的窗口类。
- **Window Style**：`WS_POPUP`（无边框，无标题栏）。
- **Extended Style**：
  - `WS_EX_TOPMOST`：置顶。
  - `WS_EX_LAYERED`：分层窗口，支持 per-pixel 物理透明通道（Alpha 混合）。
  - `WS_EX_TOOLWINDOW`：不在任务栏和 Alt+Tab 中显示。
- **多线程消息循环**：为每个 Pin 窗口实例启动一个独立的后台线程。每个线程拥有自己独立的 `GetMessageW` 消息循环。当窗口收到 `WM_DESTROY` 时，调用 `PostQuitMessage(0)` 退出当前线程的消息循环。

### 2.2 像素处理与分层渲染
1. **DPI 缩放处理**：
   - 外部传入的 `x`, `y`, `width`, `height` 为逻辑坐标，在窗口创建前，查询系统 DPI（例如通过 `GetDpiForSystem`）。
   - `scale = dpi / 96.0`。
   - 将逻辑尺寸乘以 `scale` 转换为物理像素，传给 `CreateWindowExW`。
2. **像素预处理**：
   - 使用 Rust `image` 库解码 `png_data` 得到 RGBA 像素数组。
   - `UpdateLayeredWindow` 对半透明混合格式（`AC_SRC_ALPHA`）要求极高：必须使用 **预乘 Alpha (Pre-multiplied Alpha) 的 BGRA8888** 格式。
   - 编写一段快速循环进行转换：
     ```rust
     let r_p = ((r * a) / 255) as u8;
     let g_p = ((g * a) / 255) as u8;
     let b_p = ((b * a) / 255) as u8;
     ```
3. **分层绘制**：
   - 在内存中创建一个设备上下文 (HDC) 和一个 32 位的 DIB Section 位图 (`CreateDIBSection`)。
   - 将预乘 BGRA 像素复制到 DIB Section 中，调用 `UpdateLayeredWindow` 将其与桌面合成为完美透明的窗口。
   - **无需处理 `WM_PAINT` 消息**：分层窗口在 DWM（桌面管理器）中由操作系统缓存，静止时 0% 渲染开销。

### 2.3 交互实现
1. **拖拽**：
   - 拦截 `WM_LBUTTONDOWN` 消息，执行：
     ```rust
     ReleaseCapture();
     SendMessageW(hwnd, WM_NCLBUTTONDOWN, WPARAM(HTCAPTION as usize), LPARAM(0));
     ```
     让 Windows 系统自带的拖拽算法原生接管，确保 100% 流畅。
2. **缩放 (Zoom)**：
   - 拦截 `WM_MOUSEWHEEL`。
   - 获取滚轮 Delta（`GET_WHEEL_DELTA_WPARAM`）。
   - 计算新缩放比例（以鼠标在屏幕上的当前位置为中心进行缩放，计算相对窗口的坐标比例 `rx`, `ry`）。
   - 计算缩放后的物理尺寸，并调用 `SetWindowPos` 调整窗口物理尺寸。
   - **GDI 硬件加速缩放**：创建一个与新尺寸匹配的临时 HBITMAP，并使用 `StretchBlt`（设置 `SetStretchBltMode` 为 `HALFTONE`）将原始大图拉伸到新尺寸，然后通过 `UpdateLayeredWindow` 更新。这比在 Rust 里面写 CPU 重采样要快得多，且占用内存极小。
3. **关闭**：
   - 拦截 `WM_RBUTTONUP`，在当前鼠标位置弹出包含“关闭”的 Win32 Native Popup Menu。
   - 点击“关闭”调用 `DestroyWindow`，销毁窗口资源并自动结束关联线程。

---

## 3. Linux 原生实现设计 (GTK3 + Cairo)

### 3.1 窗口与透明设置
Tauri 2 在 Linux 下使用 GTK3。我们直接利用现有 `gtk` 库依赖，在 GTK 主线程（与 Tauri 共享的 Glib Context）中创建窗口。
- **Window Type**：`gtk::WindowType::Toplevel`。
- **配置**：
  - `window.set_decorated(false)`（无边框）。
  - `window.set_keep_above(true)`（置顶）。
  - `window.set_skip_taskbar_hint(true)` 和 `window.set_skip_pager_hint(true)`（隐藏任务栏/切换器）。
- **透明度支持**：
  - 获取当前屏幕 of RGBA 视觉格式：
    ```rust
    if let Some(screen) = gdk::Screen::default() {
        if let Some(visual) = screen.rgba_visual() {
            window.set_visual(Some(&visual));
        }
    }
    window.set_app_paintable(true);
    ```

### 3.2 绘制渲染 (Cairo)
1. **像素流转换**：
   - 同理，使用 `image` 库解码 PNG，并将 RGBA 转换为预乘 Alpha 的 BGRA8888 格式。
   - 创建一个 Cairo ARgb32 格式的 `ImageSurface`，将字节流写入该 Surface 中保存。
2. **绘图事件绑定**：
   - 监听窗口的 `draw` 信号。在回调中使用 `cairo::Context`：
     - 用 `cairo::Operator::Source` 清除背景为透明，防止重影或边框发黑。
     - 用 `cairo::Operator::Over` 配合 `cr.scale(win_w / img_w, win_h / img_h)`，将保存的 Image Surface 高质量缩放渲染到窗口。

### 3.3 交互实现
1. **拖拽**：
   - 监听鼠标按下事件（`button-press-event`）。如果是左键，获取事件按钮和时间，调用 GTK 原生的拖拽接管：
     ```rust
     window.begin_drag_move(button_num, root_x as i32, root_y as i32, event_time);
     ```
2. **缩放**：
   - 监听 `scroll-event` 信号，获取滚轮滚动方向。
   - 以鼠标指针为中心，动态计算窗口应该改变的 `width` 和 `height`，然后通过 `window.resize` 修改窗口大小，并触发 `queue_draw`。
3. **关闭**：
   - 监听右键按下事件，创建一个带有“关闭”菜单项的 `gtk::Menu`。
   - 单击“关闭”项时调用 `window.close()`。

---

## 4. 关键接口统一与配置变动

### 4.1 Cargo.toml 变动
向 `crates/desktop/Cargo.toml` 中添加 `windows` 平台专属依赖。Linux 下需要的 `gtk`、`gdk`、`cairo-rs`、`glib` 直接在此处引入 direct bindings：

```toml
[target.'cfg(target_os = "windows")'.dependencies]
windows = { version = "0.61", features = [
    "Win32_Foundation",
    "Win32_UI_WindowsAndMessaging",
    "Win32_Graphics_Gdi",
    "Win32_System_LibraryLoader",
    "Win32_UI_Input_KeyboardAndMouse"
] }

[target.'cfg(target_os = "linux")'.dependencies]
gtk = "0.18"
gdk = "0.18"
cairo-rs = { version = "0.18", features = ["use_glib"] }
glib = "0.18"
```

### 4.2 pin_window.rs 统一结构
`crates/desktop/src/pin_window.rs` 的最后导出根据系统选择实现：

```rust
#[cfg(target_os = "macos")]
pub use macos::MacPinWindow as PinWindowImpl;

#[cfg(target_os = "windows")]
pub use windows::WinPinWindow as PinWindowImpl;

#[cfg(target_os = "linux")]
pub use linux::LinuxPinWindow as PinWindowImpl;
```

---

## 5. 降级方案与边界处理

1. **PNG 解码失败**：如果 PNG 解码或转换失败，写错误日志直接返回，不创建任何窗口，防止内存溢出或 panic。
2. **DPI 变动/多屏切换**：
   - Windows 方案在缩放和创建时查询系统 DPI。**实现偏差**（2026-07-06）：spec 原写 `GetDpiForSystem`，实际用 `GetDeviceCaps(LOGPIXELSX)`——功能等价（后者更传统），`scale = dpi_x / 96.0`。
   - Linux 方案中 GTK 已经内置了系统高 DPI（Logical Coordinates），GTK 的 `window.resize` 会自动在 Wayland/X11 混合 DPI 环境下工作，无需人工介入。

## 6. 前端合成双路径（2026-07-06 增补）

`pin_screenshot` 命令签名新增 `img_base64: Option<String>` 参数，支持两条数据路径：

- **`Some(base64)`**（主路径）：前端 `composeAndCropBytes()` 在 Canvas 上合成带标注/马赛克的完整图片 → `FileReader.readAsDataURL` 原生转 base64 → 后端直接解码生成贴图。这保证了贴图包含前端的涂鸦备注。
- **`None`**（fallback）：后端从 `ALL_CAPTURES` 缓存的原始截图裁剪（不含标注）。

前端 `doPin()` 使用 `isPinningRef` 防重复点击锁，在整个异步链（compose → base64 → invoke）期间阻止重复触发。FileReader 的 `onerror` 正确传递 `DOMException`。

## 7. 健壮性加固（2026-07-06 代码审计后增补）

代码审计后发现并修复的问题：

1. **Windows GDI 资源泄漏**：`update_layered_window_view` 内用 `run_gdi_calls` 闭包 + defer 清理模式，确保 `StretchBlt`/`UpdateLayeredWindow` 任一 `?` 失败后仍执行 `SelectObject`/`DeleteDC`/`DeleteObject`/`ReleaseDC`。`CreateCompatibleBitmap` 失败路径单独清理已分配资源。
2. **Windows `CreateWindowExW` 失败**：失败时手动 `Box::from_raw(state_ptr)` + `DeleteObject(hbitmap)` 回收，避免 `WM_DESTROY` 不触发的泄漏。
3. **Windows `GetMessageW` 返回值**：显式判 `status.0 == 0 || -1`（WM_QUIT / 错误），不再用 `as_bool()` 把 `-1` 误判为 true。
4. **macOS 生命周期**：`window.setReleasedWhenClosed(false)` 配合静态 `PIN_WINDOWS: Mutex<Vec<SendWindow>>` 管理——防止 Cocoa 默认自动释放使 `Retained` 指针悬空。
5. **三平台回调 panic 清零**：macOS `MainThreadMarker`/`contentView`、Windows `CreatePopupMenu`、Linux Cairo 操作的 `expect()`/`unwrap()` 全部替换为 `Option`/`Result` 模式匹配。
6. **Linux 缩放鼠标锚定**：`scroll-event` 回调中用 `event.coords()` + `win.position()` + `win.move_()` 实现以鼠标为中心缩放，与 macOS/Windows 行为对称。
7. **Linux GTK 菜单闭包循环引用**：重构 `win_menu` clone 避免 GTK 窗口内存泄漏。

