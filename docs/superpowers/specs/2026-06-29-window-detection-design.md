# 窗口智能探测截图设计

**日期**: 2026-06-29
**状态**: 设计完成，待实施
**分支**: `feature/clipboard-research`

## 0. 概述

截图窗口弹出后、用户未框选时，鼠标悬停在应用窗口上自动高亮该窗口矩形。点击高亮窗口（无拖拽）直接用窗口矩形作为选区；如果应用处于失焦状态，点击时先激活到最前面。拖拽则取消高亮进入手动框选。

基于 xcap `Window::all()` 枚举窗口（含 x/y/w/h/z/pid/id/app_name），`capx` 新增窗口探测 + 跨平台激活。

## 1. capx 新增 API

### 1.1 窗口探测

```rust
pub struct WindowRect {
    pub x: f64,       // 逻辑坐标（物理 ÷ scale_factor）
    pub y: f64,
    pub w: f64,
    pub h: f64,
    pub pid: u32,
    pub id: u32,
    pub app_name: String,
}

/// 找到包含逻辑坐标 (x, y) 的最顶层应用窗口。
/// 排除：截图窗口自身、无标题窗口、桌面/Dock/菜单栏、极小窗口（< 50×50）。
pub fn window_at_point(x: f64, y: f64, scale_factor: f64) -> Result<Option<WindowRect>>;
```

实现：
1. `Window::all()` 枚举所有窗口
2. 过滤：跳过 `title` 为空或 `"Desktop"` / `"Dock"` 的、`width < 50 || height < 50` 的、`is_minimized` 的
3. 按 `z()` 降序排列（最顶层在前）
4. 找第一个矩形包含物理坐标 `(x * scale, y * scale)` 的窗口
5. 返回逻辑坐标（物理 ÷ scale_factor）

### 1.2 跨平台窗口激活

```rust
/// 激活指定窗口到最前面。
pub fn activate_window(pid: u32, window_id: u32) -> Result<()>;
```

平台实现：
- **macOS**：`pid` → `objc2` 调 `NSRunningApplication::processIdentifier(pid)` → `activateWithOptions(.ActivateAllWindows)`
- **Windows**：`window_id` 是 hwnd → `winapi::SetForegroundWindow(hwnd)`
- **Linux**：`pid` → `Command::new("xdotool").args(["windowactivate", window_id])` 或 `wmctrl`

## 2. 前端交互

### 2.1 窗口高亮

`idle` 模式下（无选区 + 工具未激活）：
- `mousemove` 200ms 防抖调 `get_window_at_point(x, y)` Tauri 命令
- 有结果时 Canvas 绘制高亮矩形（白色描边 2px + 半透明白色填充 0.1）
- 鼠标移出窗口区域 → 清除高亮

### 2.2 点击判定

mousedown 记录起始坐标 → mouseup 时：
- 鼠标移动 < 5px（点击而非拖拽）+ 有高亮窗口 → 用窗口矩形作为 sel，进入 `selected`
- 鼠标移动 ≥ 5px（拖拽）→ 取消窗口高亮，进入手动框选
- 点击高亮窗口时同时调 `activate_window` 激活应用

### 2.3 状态优先级

- 有 `sel`（选区已确定）→ 不探测窗口
- `tool !== "none"`（标注工具激活）→ 不探测窗口
- 有 `textDraft`（文字输入中）→ 不探测窗口
- 其余情况（idle + no tool）→ 探测窗口

## 3. Tauri 命令

```rust
/// 探测鼠标位置下的窗口
#[tauri::command]
pub fn get_window_at_point(x: f64, y: f64) -> Result<Option<WindowRect>, String>;

/// 激活窗口
#[tauri::command]
pub fn activate_window_cmd(pid: u32, window_id: u32) -> Result<(), String>;
```

## 4. 不变量

- 窗口探测仅在 idle 模式 + 无工具激活时生效
- 点击高亮窗口不等待激活完成（激活是 fire-and-forget，可能需要 100-200ms）
- 手动框选优先于窗口探测（拖拽即取消高亮）
- 多显示器：窗口坐标是虚拟桌面逻辑坐标，和选区坐标一致

## 5. 边界 case

| 场景 | 处理 |
|---|---|
| 鼠标在桌面空白处 | 无窗口命中，不显示高亮 |
| 鼠标在截图窗口自身 | 排除（不返回截图窗口） |
| 窗口部分在屏幕外 | 仍然高亮，选区使用完整窗口矩形 |
| 全屏应用 | 正常探测（全屏窗口 z-order 最高） |
| 防抖期间快速移动 | 只取最后一次 mousemove 的窗口 |
