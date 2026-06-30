# 滚动截屏 — Session Handoff

**最后更新**: 2026-06-30
**分支**: `feature/clipboard-research`（worktree: `.worktrees/clipboard-research`）
**任务**: 截图 Phase 3 滚动截屏，Task 4 端到端验证

## 当前状态：代码已实现，待手动验证

滚动截屏的 manual 模式代码已全部实现并编译通过（`cargo build --release -p octopus-desktop --features embedded` 无错误）。尚未手动验证滚动录制完整流程。

### 已放弃的方案

1. **NSPanel（tauri-nspanel）**：`to_panel()` 的 `object_setClass` swizzling 在 WKWebView 创建后执行 → Trace/BPT trap 崩溃（exit 133）。PanelBuilder 内部也调 `to_panel()`，同样崩。**已从代码中完全移除**。
2. **Window hide + auto scroll**：窗口隐藏后用户看不到选区，UX 差。
3. **auto 模式（CGEvent 模拟滚轮）**：机械上可行但截到截图窗口自身 + 副屏坐标错。

### 当前方案：CGWindowList 排除 + set_ignore_cursor_events

| 组件 | 实现 |
|---|---|
| 截屏排除 overlay | `CGWindowListCreateImage(bounds, kCGWindowListOptionOnScreenBelowWindow, windowNumber, ...)` |
| 获取 windowNumber | `get_window_number(win)` → `[nsWindow windowNumber]` → u32 |
| 滚轮穿透 | Tauri 原生 `win.set_ignore_cursor_events(true)` |
| 工具栏可交互 | CGEvent 轮询鼠标全局位置 → 转窗口局部坐标 → 工具栏区域临时关穿透 |
| 窗口类型 | 普通 `WebviewWindowBuilder`（非 NSPanel） |

### 未提交的改动文件

```
crates/capx/Cargo.toml                              +core-graphics, core-foundation (macOS)
crates/capx/src/capture.rs                          +capture_display_excluding_window()
crates/desktop/Cargo.toml                           -tauri-nspanel
crates/desktop/src/main.rs                          -tauri_nspanel::init()
crates/desktop/src/screenshot_commands.rs            重写：get_window_number + 录制循环
crates/desktop/frontend/src/pages/Screenshot/index.tsx  start_scroll_recording 传 winLabel
docs/superpowers/specs/2026-06-29-scroll-screenshot-design.md   方案变更记录
docs/superpowers/plans/2026-06-29-scroll-screenshot.md          Task 10 实施记录
```

## 关键信息

### 快捷键

- **截图**: `Cmd+Shift+D`（DB `app_config` 表 `screenshot_shortcut`，**不是** Alt+S——那是豆包的）
- ASR: `Cmd+Shift+A`，剪贴板: `Cmd+Shift+F`

### 坐标系（踩过多次坑）

```
前端 x, y              = CSS 逻辑像素（窗口局部）
选区全局逻辑坐标        = 窗口 outer_position() + (x, y)
Tauri Monitor position  = 物理像素（全局桌面）
显示器逻辑偏移          = monitor.position() / scale_factor()
CGDisplay bounds        = 全局逻辑坐标（points）
crop 物理坐标           = (全局逻辑 - 显示器逻辑偏移) × scale
CGEvent 鼠标位置        = 全局逻辑坐标
窗口局部坐标            = CGEvent 全局 - 窗口 outer_position()
```

### 构建运行

```bash
cd .worktrees/clipboard-research
cargo build --release -p octopus-desktop --features embedded
# 或用脚本（会清 WebView 缓存 + 含 cloud feature）：
./run-octopus.sh
```

### 测试步骤

1. `./run-octopus.sh` 启动
2. `Cmd+Shift+D` 触发截图 → 确认两个显示器窗口正常出现（无崩溃）
3. 框选一个区域 → 点工具栏滚动截图按钮（向下箭头图标）
4. 在选区外滚动触控板/滚轮 → 验证底层应用跟随滚动
5. 观察 Canvas 是否实时更新选区画面、右侧预览是否增长
6. 点「停止」→ 验证长图入库

### 可能的问题点

- ~~`get_window_number()` 的 `msg_send!` 调用是否正确返回 windowNumber~~ → 代码核查通过（objc2 0.6，NSInteger→isize），运行时待 GUI 验证
- ~~CGWindowList 排除是否真的去掉了截图窗口~~ → core-graphics 0.24 `screenshot()` 直接传入 `window_id` 给 `CGWindowListCreateImage`，配合 `kCGWindowListOptionOnScreenBelowWindow` 逻辑正确，运行时待 GUI 验证
- `set_ignore_cursor_events(true)` 后滚轮是否真的穿透（always_on_top 窗口可能仍抢焦点）→ 运行时验证重点
- ~~BGRA → RGBA 转换是否正确~~ → 核查通过（macOS 小端 ARGB，R=raw[+2] 映射正确）

### 已修复（本次 session）

- **自动停止阈值**：3 帧（360ms）→ 25 帧（~3s）。manual 模式下用户滚动间的自然停顿会误触 360ms 阈值导致录制中断，已修正为 3s 兜底。

## spec / plan 文档

- 设计规格：`docs/superpowers/specs/2026-06-29-scroll-screenshot-design.md`
- 实施计划：`docs/superpowers/plans/2026-06-29-scroll-screenshot.md`（含偏差记录）

## 给新 session 的指令模板

```
请阅读 .worktrees/clipboard-research/SCROLL_HANDOFF.md，
然后帮我手动验证滚动截屏功能。先 cargo build --release -p octopus-desktop --features embedded，
然后用 ./run-octopus.sh 启动，Cmd+Shift+D 触发截图。
```
