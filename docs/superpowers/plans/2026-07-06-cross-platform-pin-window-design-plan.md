# 跨平台贴图窗口（Pin Window）实施计划

**日期**：2026-07-06  
**作者**：Antigravity  
**范围**：在 `crates/desktop/src/pin_window.rs` 和 `crates/desktop/src/screenshot_commands.rs` 中实现 Windows (Win32) 和 Linux (GTK3) 的原生贴图支持。

---

## 任务分解

### 任务 1：更新 `crates/desktop/Cargo.toml` 依赖
- **目的**：为 Windows 和 Linux 原生开发引入必须的底层绑定库。
- **变更点**：
  - 添加 `[target.'cfg(target_os = "windows")'.dependencies]`，引入 `windows` crate 0.61 及相关 Windows API feature（`Win32_Foundation`, `Win32_UI_WindowsAndMessaging`, `Win32_Graphics_Gdi`, `Win32_System_LibraryLoader`, `Win32_UI_Input_KeyboardAndMouse`）。
  - 添加 `[target.'cfg(target_os = "linux")'.dependencies]`，引入 `gtk = "0.18"`, `gdk = "0.18"`, `cairo-rs = { version = "0.18", features = ["use_glib"] }`, `glib = "0.18"`.

---

### 任务 2：实现 `pin_window.rs` 中的 Windows 原生实现 (Win32)
- **目的**：创建无边框透明分层窗口并用 DIB section 与 GDI+ 绘制。
- **变更点**：
  - 新增 `mod windows` 部分，用 `#[cfg(target_os = "windows")]` 门控。
  - 定义 `struct WinPinWindow` 并实现 `PinWindow` trait。
  - 实现逻辑：
    1. 在 `create()` 中，生成包含原始尺寸、缩放比、HBITMAP 的状态结构体，并 `Box::into_raw` 转为裸指针。
    2. 注册 `OctopusPinWindow` 窗口类并 spawn 后台线程运行独立的 `GetMessageW` 循环。
    3. `WndProc` 中处理：
       - `WM_LBUTTONDOWN` -> 系统拖曳 `PostMessageW(WM_NCLBUTTONDOWN, HTCAPTION)`。
       - `WM_MOUSEWHEEL` -> 局部居中缩放计算，`SetWindowPos` 物理缩放窗口，GDI `StretchBlt` 高清拉伸图片并用 `UpdateLayeredWindow` 渲染。
       - `WM_RBUTTONUP` -> `TrackPopupMenu` 创建右键“关闭”菜单。
       - `WM_DESTROY` -> 回收状态结构体、销毁 `HBITMAP`，`PostQuitMessage(0)`。

---

### 任务 3：实现 `pin_window.rs` 中的 Linux 原生实现 (GTK3 + Cairo)
- **目的**：利用 GTK3 Window + Cairo Image Surface 绘制置顶透明窗口。
- **变更点**：
  - 新增 `mod linux` 部分，用 `#[cfg(target_os = "linux")]` 门控。
  - 定义 `struct LinuxPinWindow` 并实现 `PinWindow` trait。
  - 实现逻辑：
    1. 在 `create()` 中，由于 GTK3 需要运行在主线程上，此函数由 Tauri 外部的 `run_on_main_thread` 调度。
    2. 创建 `gtk::Window` 并配置为无边框、置顶、跳过任务栏，开启 `rgba_visual` 支持透明底色。
    3. 将 PNG 转换为 pre-multiplied BGRA，创建 Cairo `ImageSurface`。
    4. 监听 `connect_draw` 回调进行 cairo 透明度清屏与缩放绘制。
    5. 监听鼠标按下事件（如果是左键，调用 `window.begin_drag_move` 进入系统级拖拽）。
    6. 监听 `scroll-event` 实现滚轮缩放，并通过 `window.resize` 物理大小变更与重新排版。
    7. 监听右键按下事件弹出 `gtk::Menu`，点击“关闭”调用 `window.close()`。

---

### 任务 4：重构 `screenshot_commands.rs` 以支持跨平台贴图
- **目的**：移除 macOS 的独占门控，统一跨平台的坐标位置算法。
- **变更点**：
  - 移除 `pin_screenshot` 原有的 `#[cfg(target_os = "macos")]` 物理像素裁剪和 Cocoa 定位。
  - 获取截图窗口的 `scale_factor` 并进行裁剪。
  - 坐标换算：
    - macOS 继续使用 Quartz 的 `get_window_cocoa_frame`（Y轴自下而上）。
    - Windows/Linux 使用 `outer_position` 配合 `scale_factor`（Y轴自上而下），将 `x`, `y` 统一换算为逻辑坐标。
  - 跨平台调用 `<crate::pin_window::PinWindowImpl as crate::pin_window::PinWindow>::create`。

---

## 验证与测试命令

- 编译桌面应用（本地 macOS）：
  `cargo build -p octopus-desktop --features embedded`
- 运行桌面应用（验证 macOS 贴图是否依然完全正常）：
  `cargo run -p octopus-desktop --features embedded`
- 语法与静态检查 Windows 目标：
  `cargo check --target x86_64-pc-windows-msvc -p octopus-desktop`
- 语法与静态检查 Linux 目标：
  `cargo check --target x86_64-unknown-linux-gnu -p octopus-desktop`
