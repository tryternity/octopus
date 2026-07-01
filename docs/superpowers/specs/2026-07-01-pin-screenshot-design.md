# 贴图功能（Pin to Desktop）设计

**日期**: 2026-07-01
**状态**: 设计完成，待实施
**分支**: `feature/clipboard-research`
**分期**: 一期 macOS，二期 Windows/Linux

## 0. 概述

截图工具栏新增"钉子"图标按钮。用户框选区域后点击钉子，选区图片以原生窗口形式钉在桌面选区当前位置。支持拖拽移动、滚轮缩放、右键菜单关闭。可同时存在多个贴图。

### 核心架构

**原生 NSWindow + NSImageView**（不创建 WebView，内存 ~3MB/个）

## 1. 触发入口

截图工具栏中，OCR 按钮旁新增钉子图标按钮（`icons/pin.svg`）。点击后：
1. `invoke("pin_screenshot", { label, x, y, w, h })` 只传坐标，不传图片数据
2. 后端从 `ALL_CAPTURES` 裁剪选区 → PNG bytes → NSImage
3. 关闭截图窗口
4. 在选区当前位置创建贴图窗口

## 2. 架构

### 2.1 文件结构

```
crates/desktop/src/pin_window.rs          # PinWindow trait + macOS 实现
crates/desktop/src/screenshot_commands.rs  # pin_screenshot 命令
crates/desktop/frontend/.../Screenshot/index.tsx  # 工具栏钉子按钮
crates/desktop/frontend/public/icons/pin.svg     # 钉子图标
```

### 2.2 PinWindow trait（跨平台抽象）

```rust
pub trait PinWindow {
    /// 创建贴图窗口
    /// png_data: PNG 字节（从 ALL_CAPTURES 裁剪，无 base64）
    /// x, y: 选区全局逻辑坐标（Quartz 坐标系）
    /// width, height: 逻辑像素尺寸
    fn create(png_data: Vec<u8>, x: f64, y: f64, width: f64, height: f64);
}
```

### 2.3 数据流

```
用户点击钉子按钮
  → invoke("pin_screenshot", { label, x, y, w, h })
  → 后端：从 ALL_CAPTURES 匹配 label → crop_region → PNG bytes
  → PinWindow::create(png_bytes, sel_global_x, sel_global_y, w, h)
  → close_all_screenshot_windows()
```

零 base64、零 WebP、最小 CPU。

## 3. macOS 实现（NSWindow + NSImageView）

### 3.1 窗口创建

```objc
NSWindow:
  - styleMask: .borderless
  - level: .floating（置顶不抢焦点）
  - hasShadow: true（桌面贴附质感）
  - isMovable: false（自行处理拖拽）
  - acceptsMouseMovedEvents: true
  - backgroundColor: clear（透明）

NSImageView:
  - image: NSImage(data: png_data)
  - frame: 填满 contentView
```

### 3.2 交互

| 操作 | 实现 | 行为 |
|---|---|---|
| 左键拖拽 | `mouseDown` → `mouseDragged` → `setFrameOrigin` | 移动贴图 |
| 滚轮 | `scrollWheel` → delta × 0.01 缩放因子 → 以鼠标为中心 `setFrame` | 等比缩放 0.2×~5× |
| 右键 | `rightMouseDown` → 弹出 NSMenu（单项「关闭」）→ `orderOut` + 释放 | 关闭贴图 |
| Esc | `keyDown(Esc)` → 同右键关闭 | 关闭贴图（需焦点） |

### 3.3 事件处理

用 objc2 创建 NSWindow 子类（或用 `objc2::declare::ClassBuilder` 动态创建），重写：
- `mouseDown:` / `mouseDragged:` — 拖拽
- `scrollWheel:` — 缩放
- `rightMouseDown:` — 右键菜单
- `keyDown:` — Esc 关闭

拖拽细节：
```
mouseDown: 记录 initialMouseLocation + initialFrameOrigin
mouseDragged:
  delta = currentMouseLocation - initialMouseLocation
  newOrigin = initialFrameOrigin + delta
  setFrameOrigin(newOrigin)
```

缩放细节：
```
scrollWheel:
  scale = 1.0 + deltaY * 0.01
  newWidth = currentWidth * scale（限制 0.2×~5×）
  newHeight = currentHeight * scale
  // 以鼠标位置为中心：调整 origin 使鼠标下方的图片点不动
  ratio = (mouseLocation.x - frame.origin.x) / frame.size.width
  newOriginX = mouseLocation.x - ratio * newWidth
  setFrame(frame with newWidth, newHeight, newOriginX, ...)
```

### 3.4 右键菜单

```
rightMouseDown:
  menu = NSMenu
  menu.addItem("关闭", action: #selector(close))
  menu.popUp(positioning: at: mouseLocation)
```

## 4. 多实例管理

- 不维护全局列表（NSWindow 被 ARC 持有，close 后自动释放）
- 每次创建独立 NSWindow + 独立事件处理
- 关闭一个不影响其他

## 5. 坐标系

贴图窗口的位置用选区的**全局 Quartz 逻辑坐标**（与截图选区一致）：
- 前端传 `x, y, w, h`（CSS 逻辑像素，窗口局部）
- 后端转全局：`sel_global_x = win_origin_x + x`（同截图坐标映射）
- NSWindow 的 frame 用 Quartz 坐标（原点左下），需翻转 Y

## 6. 跨平台规划

| 平台 | 方案 | 内存/个 | 分期 |
|---|---|---|---|
| macOS | NSWindow + NSImageView（objc2） | ~3MB | 一期 |
| Windows | HWND + DirectComposition（windows-rs） | ~5MB | 二期 |
| Linux/X11 | override-redirect + XPutImage（x11rb） | ~2MB | 二期 |
| Linux/Wayland | gtk-layer-shell | ~5MB | 二期 |

二期实现替换 `PinWindow` trait 的 macOS 实现，上层调用不变。

## 7. 限制（一期）

- macOS only
- 不支持鼠标穿透
- 不支持标注编辑（贴图是最终图片）
- 不支持复制到剪贴板（截图时已入库）
