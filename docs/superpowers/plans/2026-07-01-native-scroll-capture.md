# 原生 NSView 滚动截屏验证实施计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development or superpowers:executing-plans.

**Goal:** 独立 crate 原生 NSView 实现滚动截屏，验证滚轮穿透/截图排除/拼接可靠性

**Architecture:** NSWindow + NSView 原生覆盖窗口 + CGWindowList 截图 + FFT 拼接

**Tech Stack:** Rust + objc2/objc2-app-kit + core-graphics + rustfft + capx

**Spec:** `docs/superpowers/specs/2026-07-01-native-scroll-capture-design.md`

---

## 文件结构

| 文件 | 变更 | 责任 |
|---|---|---|
| `crates/scroll-capture/Cargo.toml` | Create | crate 配置 |
| `crates/scroll-capture/src/lib.rs` | Create | 公共接口 start/stop |
| `crates/scroll-capture/src/overlay.rs` | Create | ScrollOverlay trait + MonitorInfo |
| `crates/scroll-capture/src/recording.rs` | Create | 录制循环 + 拼接 + finalize |
| `crates/scroll-capture/src/macos/mod.rs` | Create | macOS 模块入口 |
| `crates/scroll-capture/src/macos/overlay_window.rs` | Create | NSScrollOverlayWindow + NSScrollOverlayView |
| `crates/scroll-capture/src/macos/capture.rs` | Create | CGWindowList 截图 |
| `crates/scroll-capture/src/macos/helpers.rs` | Create | 焦点让出 + 坐标转换 |
| `Cargo.toml` (workspace) | Modify | 加 scroll-capture member |
| `crates/desktop/Cargo.toml` | Modify | 依赖 scroll-capture |
| `crates/desktop/src/main.rs` | Modify | 注册命令 + 托盘入口 |
| `crates/desktop/src/tray.rs` | Modify | 托盘菜单「滚动截屏」 |

---

### Task 1: crate 骨架 + trait 定义

- [x] 创建 `crates/scroll-capture/Cargo.toml`
- [x] 创建 `src/lib.rs`（start/stop 函数签名 + 录制状态）
- [x] 创建 `src/overlay.rs`（ScrollOverlay trait + MonitorInfo）
- [x] workspace Cargo.toml 加 member
- [x] 验证 `cargo build -p scroll-capture` 通过

### Task 2: macOS NSWindow + NSView 选区拉框

- [x] `macos/overlay_window.rs`：NSScrollOverlayWindow（define_class! 继承 NSWindow）
  - borderless + transparent + floating + canBecomeKey
- [x] `macos/overlay_window.rs`：NSScrollOverlayView（define_class! 继承 NSView）
  - mouseDown/mouseDragged/mouseUp（拉框选区）
  - draw（暗遮罩 + 绿色边框）
  - keyDown（ESC 停止）
  - ivars：选区矩形 + 状态机 + 拖拽起点
- [x] `macos/mod.rs`：ScrollOverlay trait 的 macOS 实现
- [x] 验证编译

### Task 3: CGWindowList 截图 + 排除

- [x] `macos/capture.rs`：capture_region_excluding(window_ids, rect) → RgbaImage
  - CGWindowListCreateImage + kCGWindowListOptionOnScreenBelowWindow
  - BGRA → RGBA 转换
- [x] 验证编译

### Task 4: 焦点让出 + 坐标转换

- [x] `macos/helpers.rs`：
  - get_window_pid_at_point（CGWindowListCopyWindowInfo + bounds 命中）
  - activate_app_by_pid（NSRunningApplication.activateWithOptions，主线程）
- [x] 验证编译

### Task 5: 录制循环 + 拼接

- [x] `recording.rs`：
  - start_recording：初始化 Stitcher → 100ms 循环截图 + process_frame
  - stop_recording：finalize → on_complete 回调
- [x] lib.rs 串联：start() 创建覆盖窗口 → 选区确定 → start_recording
- [x] 验证编译

### Task 6: desktop crate 集成

- [x] desktop/Cargo.toml 加 scroll-capture 依赖
- [x] main.rs 注册 `start_scroll_capture` / `stop_scroll_capture` 命令
- [x] tray.rs 托盘菜单加「滚动截屏」
- [x] on_complete 回调：入库 + emit clipboard://changed
- [x] 验证全量编译 `cargo build -p octopus-desktop --features embedded`

### Task 7: 端到端验证

- [ ] 托盘「滚动截屏」→ 覆盖窗口出现
- [ ] 拖拽拉框选区
- [ ] 选区确定 → 绿色边框 → 底层应用可滚动
- [ ] 滚动 → 后台截图拼接
- [ ] ESC / 托盘停止 → 长图入库
- [ ] 多次测试拼接质量（无重叠/模糊/缺失）

---

## Spec Coverage

| spec 章节 | 实现 task |
|---|---|
| §2 公共接口 | Task 1 |
| §3 跨平台 trait | Task 1 |
| §4 选区拉框 | Task 2 |
| §5.1 录制流程 | Task 5 |
| §5.2 截图排除 | Task 3 |
| §5.3 焦点让出 | Task 4 |
| §5.4 坐标转换 | Task 4 |
| §6 拼接引擎 | 复用 capx/stitch.rs |
| §7 托盘入口 | Task 6 |
