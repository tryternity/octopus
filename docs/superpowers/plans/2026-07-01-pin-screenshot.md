# 贴图功能（Pin to Desktop）实施计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 截图工具栏新增钉子按钮，点击后选区图片以原生 NSWindow 钉在桌面，支持拖拽/缩放/右键关闭

**Architecture:** objc2 创建原生 NSWindow + NSImageView 双类架构（PinNSImageView 拖拽 + PinNSWindow 缩放/右键菜单）

**Tech Stack:** Rust + objc2/objc2-app-kit + Tauri 2

**Spec:** `docs/superpowers/specs/2026-07-01-pin-screenshot-design.md`

---

## 文件结构

| 文件 | 变更 | 责任 |
|---|---|---|
| `crates/desktop/src/pin_window.rs` | Create | PinWindow trait + macOS 实现（PinNSWindow + PinNSImageView） |
| `crates/desktop/src/main.rs` | Modify | `mod pin_window` + 注册 `pin_screenshot` 命令 |
| `crates/desktop/src/screenshot_commands.rs` | Modify | 新增 `pin_screenshot` 命令 |
| `crates/desktop/frontend/src/pages/Screenshot/index.tsx` | Modify | 钉子按钮 |
| `crates/desktop/frontend/public/icons/pin.svg` | Create | 钉子图标 |

---

### Task 1: pin_window.rs — PinWindow trait + macOS 基础窗口 ✅

- [x] 创建 pin_window.rs
- [x] main.rs 加 mod pin_window
- [x] 验证编译

## Task 2: macOS NSWindow 创建 + NSImageView 显示图片 ✅

- [x] 实现 macOS PinWindow（NSImage + NSImageView + NSWindow）
- [x] 坐标转换：CSS → Quartz（Cocoa frame Y 翻转）
- [x] 多实例：static Mutex<Vec> 保持 ARC 引用

## Task 3: pin_screenshot 后端命令 ✅

- [x] 新增 pin_screenshot 命令（从 ALL_CAPTURES 裁剪选区 → PNG → PinWindow::create）
- [x] main.rs 注册命令
- [x] 坐标转换用 get_window_cocoa_frame

## Task 4: 前端钉子按钮 ✅

- [x] 创建 pin.svg 图标
- [x] 前端钉子按钮（选区右上角独立 DOM 元素）
- [x] doPin 函数

## Task 5: 拖拽移动 ✅

- [x] PinNSImageView（继承 NSImageView）处理 mouseDown → performWindowDragWithEvent
- [x] 系统原生拖拽，零抖动、跨屏正确

## Task 6: 滚轮缩放 ✅

- [x] PinNSWindow 重写 scrollWheel
- [x] 以鼠标为中心等比缩放（20~10000px）
- [x] NSImageView autoresizingMask 自动同步

## Task 7: 右键菜单关闭 ✅

- [x] PinNSWindow 重写 rightMouseDown → NSMenu + popUpContextMenu

## Task 8: 端到端验证 ✅

- [x] 贴图创建 + 拖拽 + 缩放 + 右键关闭全部正常工作

---

## 实施偏差记录

### 偏差 1：自定义 NSWindow 子类与 tao event loop 冲突 ❌→✅

最初在 PinNSWindow 中重写 mouseDown/mouseDragged 手动拖拽 → `msg_send![event, mouseLocation]` 触发 foreign exception 崩溃。

尝试的方案：
1. `catch_unwind` → 不能捕获 foreign exception
2. `locationInWindow` + 手动 delta → 抖动
3. `setMovableByWindowBackground` → 透明窗口不生效
4. `performDrag:` → 仍然崩溃

**最终方案**：双类架构
- `PinNSImageView`（继承 NSImageView）：mouseDown → `window.performWindowDragWithEvent(event)` 委托给系统
- `PinNSWindow`（继承 NSWindow）：scrollWheel 缩放 + rightMouseDown 右键菜单
- 拖拽在 View 层，缩放/关闭在 Window 层，职责分离

### 偏差 2：钉子按钮位置 ✅

从工具栏移出，改为选区右上角独立 DOM 元素（工具栏在上方时移到右下角）。

### 偏差 3：尺寸标注位置 ✅

从选区右下方移到左上角，避免和工具栏重叠。

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
| §4 多实例 | Task 2 |
| §5 坐标系 | Task 3 |
