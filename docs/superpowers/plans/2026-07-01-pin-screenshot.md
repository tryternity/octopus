# 贴图功能（Pin to Desktop）实施计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 截图工具栏新增钉子按钮，点击后选区图片以原生 NSWindow 钉在桌面，支持拖拽/缩放/右键关闭

**Architecture:** objc2 创建原生 NSWindow + NSImageView（不创建 WebView，~3MB/个），PinWindow trait 跨平台抽象

**Tech Stack:** Rust + objc2/objc2-app-kit + objc2-foundation + Tauri 2

**Spec:** `docs/superpowers/specs/2026-07-01-pin-screenshot-design.md`

---

## 文件结构

| 文件 | 变更 | 责任 |
|---|---|---|
| `crates/desktop/src/pin_window.rs` | Create | PinWindow trait + macOS 实现（NSWindow 子类 + 拖拽/缩放/右键） |
| `crates/desktop/src/main.rs` | Modify | `mod pin_window` + 注册 `pin_screenshot` 命令 |
| `crates/desktop/src/screenshot_commands.rs` | Modify | 新增 `pin_screenshot` 命令（从 ALL_CAPTURES 裁剪选区 → PNG → PinWindow::create） |
| `crates/desktop/frontend/src/pages/Screenshot/index.tsx` | Modify | 工具栏钉子按钮 |
| `crates/desktop/frontend/public/icons/pin.svg` | Create | 钉子图标 |

---

### Task 1: pin_window.rs — PinWindow trait + macOS 基础窗口

**Files:**
- Create: `crates/desktop/src/pin_window.rs`
- Modify: `crates/desktop/src/main.rs`（加 `mod pin_window`）

- [ ] **Step 1: 创建 pin_window.rs 基本结构**

```rust
// crates/desktop/src/pin_window.rs
// 贴图功能：原生窗口钉在桌面，支持拖拽/缩放/右键关闭。
// 一期 macOS（NSWindow + NSImageView），二期 Win/Linux 替换实现。

/// 跨平台贴图窗口抽象。
pub trait PinWindow {
    /// 创建贴图窗口。
    /// png_data: PNG 字节
    /// x, y: 选区全局 Quartz 逻辑坐标（原点左下）
    /// width, height: 逻辑像素尺寸
    fn create(png_data: &[u8], x: f64, y: f64, width: f64, height: f64);
}

#[cfg(target_os = "macos")]
mod macos;
```

- [ ] **Step 2: main.rs 加 mod pin_window**

在 `crates/desktop/src/main.rs` 的 `mod` 声明区加：

```rust
mod pin_window;
```

- [ ] **Step 3: 验证编译**

Run: `cargo build -p octopus-desktop --features embedded`
Expected: 编译通过（macos 模块还没内容，先加空壳）

---

### Task 2: macOS NSWindow 创建 + NSImageView 显示图片

**Files:**
- Create: `crates/desktop/src/pin_window/macos.rs`（或在 pin_window.rs 内）

- [ ] **Step 1: 实现 macOS PinWindow**

在 `pin_window.rs` 中实现 macOS 版本：

```rust
#[cfg(target_os = "macos")]
mod macos_impl {
    use objc2::rc::Retained;
    use objc2::runtime::AnyObject;
    use objc2::{class, msg_send, sel, msg_send_id};
    use objc2_app_kit::{NSWindow, NSView, NSImageView, NSImage, NSWindowStyleMask};
    use objc2_foundation::{NSRect, NSPoint, NSSize, NSData, mainQueue};

    pub fn create_pin_window(png_data: &[u8], x: f64, y: f64, width: f64, height: f64) {
        unsafe {
            // 1. 创建 NSImage from PNG data
            let nsdata = NSData::with_bytes(png_data);
            let image: Retained<NSImage> = msg_send_id![
                msg_send_id![class!(NSImage), alloc],
                initWithData: &nsdata
            ].unwrap();

            // 2. 创建 NSImageView
            let frame = NSRect {
                origin: NSPoint::ZERO,
                size: NSSize { width, height },
            };
            let image_view: Retained<NSImageView> = msg_send_id![
                msg_send_id![class!(NSImageView), alloc],
                initWithFrame: frame
            ].unwrap();
            let _: () = msg_send![&image_view, setImage: &image];

            // 3. 创建 NSWindow（borderless + floating）
            let window_frame = NSRect {
                origin: NSPoint { x, y },
                size: NSSize { width, height },
            };
            let window: Retained<NSWindow> = msg_send_id![
                msg_send_id![class!(NSWindow), alloc],
                initWithContentRect: window_frame,
                styleMask: NSWindowStyleMask::Borderless,
                backing: 2, // NSBackingStoreBuffered
                defer: false
            ].unwrap();

            let _: () = msg_send![&window, setLevel: 3]; // NSFloatingWindowLevel = 3
            let _: () = msg_send![&window, setHasShadow: true];
            let _: () = msg_send![&window, setOpaque: false];
            let _: () = msg_send![&window, setBackgroundColor: msg_send_id![class!(NSColor), clearColor]];

            // 4. 设置 contentView 为 image_view
            let content_view: Retained<NSView> = msg_send_id![&window, contentView];
            let _: () = msg_send![&content_view, addSubview: &image_view];

            // 5. 显示窗口
            let _: () = msg_send![&window, makeKeyAndOrderFront: None];
        }
    }
}
```

- [ ] **Step 2: 实现 PinWindow trait**

```rust
#[cfg(target_os = "macos")]
impl PinWindow for () {
    fn create(png_data: &[u8], x: f64, y: f64, width: f64, height: f64) {
        macos_impl::create_pin_window(png_data, x, y, width, height);
    }
}
```

- [ ] **Step 3: 验证编译**

Run: `cargo build -p octopus-desktop --features embedded`
Expected: 编译通过（可能有 unused warning）

---

### Task 3: pin_screenshot 后端命令

**Files:**
- Modify: `crates/desktop/src/screenshot_commands.rs`
- Modify: `crates/desktop/src/main.rs`（注册命令）

- [ ] **Step 1: 新增 pin_screenshot 命令**

在 `screenshot_commands.rs` 中新增：

```rust
/// 贴图：从 ALL_CAPTURES 裁剪选区 → PNG → 创建贴图窗口 → 关闭截图窗口
#[tauri::command]
pub async fn pin_screenshot(
    label: String,
    x: f64, y: f64, w: f64, h: f64,
    app_handle: tauri::AppHandle,
) -> Result<(), String> {
    // 从 ALL_CAPTURES 获取截图数据
    let capture = {
        let captures = ALL_CAPTURES.lock().unwrap();
        captures.iter()
            .find(|(l, _)| l == &label)
            .map(|(_, c)| c.clone())
            .ok_or("找不到截图数据")?
    };

    // 裁剪选区（物理坐标）
    let scale = app_handle.get_webview_window(&label)
        .and_then(|w| w.scale_factor().ok())
        .unwrap_or(1.0) as f64;
    let px = (x * scale) as u32;
    let py = (y * scale) as u32;
    let pw = (w * scale) as u32;
    let ph = (h * scale) as u32;

    let png_bytes = octopus_capx::capture::crop_region(&ScreenCapture {
        rgba_bytes: capture.rgba_bytes,
        width: capture.width,
        height: capture.height,
        monitor_x: 0, // 裁剪不需要
        monitor_y: 0,
    }, px, py, pw, ph).map_err(|e| e.to_string())?;

    // 获取选区全局 Quartz 坐标
    let win = app_handle.get_webview_window(&label)
        .ok_or("窗口不存在")?;
    #[cfg(target_os = "macos")]
    let (qx, qy) = {
        let primary_h = get_primary_screen_height();
        if let Some((cx, cy, _, ch)) = get_window_cocoa_frame(&win) {
            (cx + x, primary_h - (cy + ch) + y) // Cocoa 左下 → Quartz 左上
        } else {
            (x, y)
        }
    };
    #[cfg(not(target_os = "macos"))]
    let (qx, qy) = (x, y);

    // 创建贴图窗口
    <() as crate::pin_window::PinWindow>::create(&png_bytes, qx, qy, w, h);

    // 关闭截图窗口
    close_all_screenshot_windows(&app_handle);

    Ok(())
}
```

- [ ] **Step 2: main.rs 注册命令**

在 `tauri::generate_handler!` 中加：

```rust
screenshot_commands::pin_screenshot,
```

- [ ] **Step 3: 验证编译**

Run: `cargo build -p octopus-desktop --features embedded`
Expected: 编译通过

---

### Task 4: 前端钉子按钮

**Files:**
- Create: `crates/desktop/frontend/public/icons/pin.svg`
- Modify: `crates/desktop/frontend/src/pages/Screenshot/index.tsx`

- [ ] **Step 1: 创建 pin.svg 图标**

一个简单的钉子图标 SVG。

- [ ] **Step 2: 前端工具栏加钉子按钮**

在 OCR 按钮后面、保存按钮前面加：

```tsx
<button onClick={doPin} title="贴图" style={{ padding: "4px", width: 32, height: 32, display: "flex", alignItems: "center", justifyContent: "center", borderRadius: 6, border: "none", background: "transparent", cursor: "pointer" }}>
  <img src="icons/pin.svg" alt="贴图" className="w-[18px] h-[18px]" />
</button>
```

新增 `doPin` 函数：

```tsx
function doPin() {
  if (!sel) return;
  invoke("pin_screenshot", {
    label: winLabel,
    x: sel.x, y: sel.y, w: sel.w, h: sel.h,
  }).catch(() => {});
}
```

- [ ] **Step 3: 验证前端编译**

Run: `cd crates/desktop/frontend && npm run build`
Expected: 编译通过

---

### Task 5: 拖拽移动

**Files:**
- Modify: `crates/desktop/src/pin_window.rs`

- [ ] **Step 1: 用 ClassBuilder 创建 NSWindow 子类处理鼠标事件**

需要用 objc2 的 `define_class!` 或 `declare::ClassBuilder` 创建自定义 NSWindow 子类，重写 `mouseDown:` / `mouseDragged:`。

```rust
// 在 mouseDown 中记录初始鼠标位置和窗口位置
// 在 mouseDragged 中计算 delta 并 setFrameOrigin
```

- [ ] **Step 2: 验证拖拽工作**

手动测试：创建贴图 → 鼠标拖拽 → 贴图跟随移动

---

### Task 6: 滚轮缩放

**Files:**
- Modify: `crates/desktop/src/pin_window.rs`

- [ ] **Step 1: 重写 scrollWheel: 事件**

```rust
// scrollWheel:
//   scale = 1.0 + deltaY * 0.01
//   限制 0.2×~5×
//   以鼠标位置为中心缩放（调整 origin）
```

- [ ] **Step 2: 验证缩放工作**

手动测试：滚轮向上放大，向下缩小，鼠标位置为中心

---

### Task 7: 右键菜单关闭 + Esc 关闭

**Files:**
- Modify: `crates/desktop/src/pin_window.rs`

- [ ] **Step 1: 重写 rightMouseDown: 弹出 NSMenu**

```rust
// rightMouseDown:
//   menu = NSMenu
//   menu.addItem("关闭")
//   menu.popUp(positioning:at:)
```

- [ ] **Step 2: 重写 keyDown: 处理 Esc**

```rust
// keyDown: 如果 keyCode == 53 (Esc) → close
```

- [ ] **Step 3: 验证关闭工作**

手动测试：右键弹菜单 → 点关闭 → 窗口消失

---

### Task 8: 端到端验证

- [ ] **Step 1: 全流程测试**

1. Cmd+Shift+D 触发截图
2. 框选区域
3. 点工具栏钉子按钮
4. 截图窗口关闭，贴图出现在选区位置
5. 拖拽贴图移动
6. 滚轮缩放
7. 右键弹菜单关闭
8. 多次钉图（多实例）

- [ ] **Step 2: 同步 spec/plan 偏差记录**

---

## Spec Coverage

| spec 章节 | 实现 task |
|---|---|
| §1 触发入口 | Task 4 |
| §2.2 PinWindow trait | Task 1 |
| §2.3 数据流 | Task 3 |
| §3.1 窗口创建 | Task 2 |
| §3.2 交互-拖拽 | Task 5 |
| §3.2 交互-缩放 | Task 6 |
| §3.2 交互-右键关闭 | Task 7 |
| §3.2 交互-Esc 关闭 | Task 7 |
| §3.3 事件处理 | Task 5-7 |
| §4 多实例 | Task 2（ARC 自动管理） |
| §5 坐标系 | Task 3 |
