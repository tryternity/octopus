# 剪贴板浮窗边缘吸附 + 迷你模式设计

> **状态**：设计完成，待实现
> **日期**：2026-07-10
> **scope**：剪贴板浮窗（`clipboard_window`）边缘吸附 + 收缩/展开 + 位置记忆
> **平台**：仅 macOS（Windows/Linux 后续）

---

## 1. 背景与动机

### 1.1 问题

剪贴板浮窗（300×600）当前行为：
- 快捷键唤出，显示在上次位置（已有位置记忆 `window_position.rs`）
- 用户可拖拽（`data-tauri-drag-region="deep"`），放手保存位置
- 不用时快捷键 toggle 隐藏

痛点：浮窗占用 300×600 屏幕空间，用户想常驻查看但不想遮挡其他 app。

### 1.2 目标

借鉴 KoBar 的边缘吸附体验：
- 拖到屏幕边缘 → 自动吸附 + 收缩为细条（~8px）
- 鼠标悬停细条 → 展开完整浮窗
- 操作完点击外部 → 收回细条
- 高度不变（600），只宽度动画展开/收缩，视觉自然

### 1.3 不做

- **action_bar** 定位策略不变——维持鼠标上方定位，不做位置记忆 / 吸附 / 拖拽 / 尺寸变更（详见 §7 文档更新）
- **`setSize` 物理缩窗**——透明 + decorations(false) 窗口上 `setSize` 不可靠（result_window 踩坑确认），此路不通
- **越过屏幕边界**——跨主副屏时越界部分会漏到副屏，处理复杂度高

---

## 2. 核心方案：完全屏内 + 局部透明穿透

### 2.1 核心思路

窗口物理尺寸始终 300×600 不变。收缩态下：

1. 窗口位置完全在当前屏幕内（不越界）
2. CSS 把大部分区域 `transparent`（无 border/shadow/圆角）
3. 8px 细条贴着吸附边缘高亮显示（voice 色 + 微阴影）
4. Rust 后台轮询线程（`start_edge_poll`）读全局鼠标位置：鼠标在细条区域 → `setIgnoresMouseEvents(false)`（可交互），其余区域 → `setIgnoresMouseEvents(true)`（穿透到下层 app）
5. 其余 292px 透明 + 穿透（看到下层 app / 桌面）

### 2.2 视觉示意（右吸附收缩态）

```
屏幕右边缘
     │█              ← 8px 高亮细条（voice 色 + 微阴影）
     │█  ← 其余 292px 透明穿透（看到桌面 / 下层 app）
     │█
     │█  600 高
```

### 2.3 为什么不用透明越界方案

如果窗口越过屏幕边界到副屏——
- macOS 窗口级 `setIgnoresMouseEvents(true)` 是全窗口开关，开了之后细条也收不到事件
- 部分穿透需要 NSTrackingArea + 全窗口穿透的组合，但越界部分在副屏上仍会渲染（需额外透明处理）
- 多屏幕排列不规则时越界位置不可预测

**完全屏内方案**零越界风险，多屏幕安全。

### 2.4 为什么不用物理窄窗口（setSize）

`setSize` 在 `transparent + decorations(false)` 的 Tauri 窗口上不可靠——result_window 尺寸双模式踩坑（architecture.md §4 记录）确认物理 `setSize` 被 NSWindow 拒绝。剪贴板窗口同为透明无装饰窗口，行为一致。

---

## 3. 状态机与生命周期

### 3.1 三种模式

```
Normal（完整态 300×600）
  ↓ 用户拖拽到屏幕边缘 ≤10px 后放手
Docked-Collapsed（收缩态，仅 8px 细条）
  ↓ 鼠标悬停到细条
Docked-Expanded（展开态，完整 300×600）
  ↓ 鼠标点击浮窗外（左 / 右键 down）
Docked-Collapsed（收回）
```

### 3.2 状态转换规则

实际实现简化为三种状态（Rust 侧 `DOCK_EXPANDED: AtomicBool` 真相源，前端镜像同步）：

| 当前状态 | 触发 | 目标状态 | 说明 |
|---------|------|---------|------|
| Normal | 拖拽放手 + 边缘 ≤10px | Docked-Collapsed | 吸附 + 收缩 + 启动穿透轮询 |
| Normal | 拖拽放手 + 边缘 >10px | Normal | 保存位置（现有逻辑） |
| Docked-Collapsed | 鼠标悬停/点击细条 | Docked-Expanded | 停穿透轮询 + 展开获焦 |
| Docked-Expanded | 失焦（用户切到其他 app） | Docked-Collapsed | 启动穿透轮询 + 收缩 |
| Docked-Expanded | 快捷键（`DOCK_EXPANDED=true`） | Docked-Collapsed | 启动穿透轮询 + 收缩 |
| Docked-Collapsed | 快捷键（`DOCK_EXPANDED=false`） | Docked-Expanded | 停穿透轮询 + 展开获焦 |
| Docked-* | 用户拖拽窗口（拖离边缘） | Normal | 解吸附 |

> ⚠️ toggle 不用 `is_focused()`——macOS 收缩态焦点不可靠，用 `DOCK_EXPANDED` 原子状态判断。

**防护机制（审查修复）**：
- **防重入**：Moved 事件中 `is_already_collapsed`（同 edge 且 collapsed）跳过 save_dock/start_poll，防高频 DB 写 + 线程重建
- **多屏横跳防护**：已吸附某 edge 收缩态时不切换到另一个 edge（`is_docked_on_other_edge`），需先解吸附
- **解吸附状态一致性**：undocked 分支重置 `DOCK_EXPANDED.store(false)`
- **窗口隐藏不空转**：`Focused(false)` 中 `is_visible()` 只保护 `start_edge_poll`，`DOCK_EXPANDED.store(false)` + emit 始终执行
- **位置保存节流**：Moved 中 `LAST_SAVE_SEC: AtomicI64` 秒级节流（同一秒最多写 1 次），失焦时无视节流强制兜底写

### 3.3 窗口关闭后恢复

快捷键 toggle / X 按钮关闭后，下次打开：
- dock 状态从 DB 读取（`window_dock.clipboard_window`）
- dock ≠ none → 直接以 Docked-Collapsed 态打开（贴边 + 细条）
- dock = none → Normal 态打开（位置记忆，现有逻辑）

---

## 4. 吸附检测与位置计算

### 4.1 吸附触发

用户拖拽窗口放手时（`WindowEvent::Moved` 最终位置）检测：

```
窗口外边框距当前屏幕边缘距离 ≤ 10px
→ 吸附到该边缘 + 进入 Docked-Collapsed
否则
→ 保持 Normal
```

### 4.2 确定吸附边缘

1. 找窗口中心所在显示器（`available_monitors` + contains 检测）
2. 检测窗口外边框距该显示器左/右边缘距离
3. 同时靠近两边时取更近的一边
4. 上下边缘不吸附（只做左右）

### 4.3 吸附后位置

- 右吸附：`x = monitor.right() - 300`（窗口右边贴屏幕右边），`y = 放手时的 y`
- 左吸附：`x = monitor.left()`（窗口左边贴屏幕左边），`y` 同上

y 坐标保留用户拖到的位置。窗口高度 600 固定，y 超出屏幕底部时碰撞检测修正（复用现有逻辑）。

### 4.4 物理坐标 → 逻辑坐标

吸附位置计算在逻辑坐标系完成。Monitor 的 `position()` / `size()` 返回物理像素，必须除以 `scale_factor()` 转逻辑（AGENTS.md 坐标踩坑章节）。

---

## 5. 前端交互与状态管理

### 5.1 Rust ↔ 前端事件

| 事件 | 方向 | payload | 说明 |
|------|------|---------|------|
| `clipboard://dock-changed` | Rust → 前端 | `"right" \| "left" \| "none"`（字符串） | 前端据此切换模式 |
| `clipboard://expand` | Rust → 前端 | — | 展开细条 |
| `clipboard://collapse` | Rust → 前端 | — | 收缩 |

### 5.2 前端状态（`Clipboard/index.tsx`）

```
dockEdge: "right" | "left" | null     // 吸附边缘
dockMode: "none" | "collapsed" | "expanded"
```

### 5.3 CSS 行为

| dockMode | 容器样式 |
|----------|----------|
| none | 正常 300×600 + border + shadow + 圆角 |
| collapsed | 无 border/shadow/圆角，背景透明，只渲染 8px 细条（`absolute` 定位贴 dockEdge 侧） |
| expanded | 同 none |

### 5.4 展开触发

- **hover**（`onMouseEnter`）：窗口有焦点时 hover 细条即展开
- **点击**（`onMouseDown`）：macOS 非 key window 不交付 hover 事件，需点击作为 fallback

### 5.5 收缩触发

- **失焦**（`Focused(false)` 事件）：docked 态下窗口失焦 → Rust emit `collapse`。不用全局鼠标点击监听（简化为失焦触发）

---

## 6. 鼠标穿透（与 result_window 统一方案）

收缩态透明区域需要穿透到下层 app。核心矛盾：`setIgnoresMouseEvents(true)` 是全窗口开关，细条也收不到事件。解法与 `result_window::start_click_through_poller` 完全统一：

### 6.1 轮询线程（`clipboard_dock::start_edge_poll`）

- `tokio::interval(33ms)` 轮询 Tauri `cursor_position()`（物理坐标，跨平台）
- 鼠标在细条区域（边缘 10px 容差）→ `setIgnoresMouseEvents(false)`（可交互）
- 鼠标在透明区域 → `setIgnoresMouseEvents(true)`（穿透）
- macOS：NSWindow `setIgnoresMouseEvents` via `run_on_main_thread`
- 其他平台：Tauri `set_ignore_cursor_events`
- **线程安全（`POLL_ID: AtomicU64`）**：每次 `start_edge_poll` 递增 ID，轮询线程检测 ID 不匹配自动退出——防快捷键+失焦双触发导致多线程竞态

### 6.2 为什么不用前端 setIgnoreCursorEvents

一旦 `setIgnoreCursorEvents(true)`，窗口完全不收鼠标事件（NSWindow 连 tracking area 都禁），前端 mousemove 不再触发 → 无法检测光标重入 → 重入失效。必须 Rust 后端读全局位置。

### 6.3 为什么不用 CGEvent

CGEvent 是 macOS 专属，且返回逻辑坐标需 scale 换算。`cursor_position()` 跨平台（Windows GetCursorPos / Linux X11 XQueryPointer / macOS NSEvent），返回物理坐标直接比较，多显示器不同 DPI 安全。Wayland 已知限制：返回 (0,0)，穿透失效（协议层无解）。

---

## 7. 文件变更

### 7.1 Rust

| 文件 | 变更 |
|------|------|
| `clipboard_window.rs` | 吸附检测（`detect_dock_edge`）；dock 状态读写；初始打开模式判断；`DOCK_EXPANDED` 原子状态；toggle 逻辑（docked 模式不同于普通 show/hide）；`clipboard_dock_expand`/`clipboard_dock_collapse` Tauri 命令 |
| `window_position.rs` | `save_dock_state(label, edge)` / `load_dock_state(label) -> Option<String>`（DB key `window_dock.{label}`） |
| `clipboard_dock.rs`（新建） | `start_edge_poll` / `stop_edge_poll`——tokio interval 33ms 轮询 `cursor_position()`，动态切换 `setIgnoresMouseEvents` |
| `main.rs` | 加 `mod clipboard_dock;`；invoke_handler 注册两个命令 |

### 7.2 前端

| 文件 | 变更 |
|------|------|
| `Clipboard/index.tsx` | dockEdge/dockMode 状态；listen 三个事件；CSS class 切换（collapsed 移除 border/shadow）；细条 `onMouseEnter`+`onMouseDown` 展开触发 |
| `Clipboard/DockBar.tsx` | 未独立文件——细条内联在 `index.tsx`（`absolute` 定位 + voice 色 + `pointer-events: auto`） |

### 7.3 配置

无新增配置项——dock 状态纯运行时记忆（DB 存），不需用户配置。

### 7.4 文档更新（action_bar 定位策略明确）

| 文件 | 变更 |
|------|------|
| `2026-07-08-action-bar-design.md` §4.1 | 加说明：action_bar 定位策略固定为鼠标上方，不做位置记忆 / 吸附 / 拖拽 / 尺寸变更 |
| `architecture.md` 窗口管理表 | clipboard_window 行补注 dock 功能；action_bar 行补注固定定位 |

---

## 8. 不变式

- **窗口物理尺寸始终 300×600**——收缩/展开只改 CSS 可见区域 + 鼠标穿透，不改物理尺寸
- **窗口始终完全在当前屏幕内**——不越界到其他屏幕
- **`resizable(true)` 保留**——dock 功能与 resize 不冲突
- **位置记忆继续工作**——`window_position.rs` 存 x,y，dock 状态是额外维度（`window_dock.clipboard_window`）
- **`DOCK_EXPANDED: AtomicBool`** Rust 侧真相源——不依赖 `is_focused()`（macOS 收缩态焦点不可靠）
- **跨平台**——穿透用 Tauri `cursor_position()`（macOS NSEvent / Windows GetCursorPos / Linux X11 XQueryPointer），非 macOS 用 `set_ignore_cursor_events` 替代 NSWindow。**Wayland 限制**：cursor_position 恒返回 (0,0)，穿透失效（无解）

---

## 9. 边界场景

| 场景 | 处理 |
|------|------|
| 屏幕分辨率/显示器变化后打开 | 现有 `is_position_visible` 已检测；dock 状态也需类似检查——吸附边缘对应的显示器不在了 → dock=none |
| 拖拽中突然吸附 | 不做——吸附只在 Moved 最终位置（放手时）检测，拖拽过程中不中途吸附 |
| 快捷键唤出时已有 dock | 直接 Collapsed 态打开（贴边细条），用户 hover 展开 |
| 多个显示器拖拽 | Moved 事件中根据窗口中心找当前显示器，吸附到当前显示器的边缘 |
| Docked 态下窗口失焦 | `DOCK_EXPANDED=true` 时收缩为 Collapsed + 启动穿透轮询（`is_visible()` 只保护轮询启动，不阻断状态重置） |
| 窗口 hide（X 按钮）后失焦 | `DOCK_EXPANDED.store(false)` + emit 始终执行；`is_visible()=false` 时跳过 `start_edge_poll`（防空转） |
| 拖拽时高频 DB 写 | `LAST_SAVE_SEC` 秒级节流；失焦时无视节流强制兜底写 |
| 非 macOS 平台 | Moved/Focused/create 的 dock 逻辑全部 `#[cfg(target_os = "macos")]` gate |

---

## 10. 信息来源

- KoBar Ghost Window 透明覆盖层模式（`~/.tolaria/桌面工具/kobar-*.md`）
- result_window 尺寸双模式踩坑（architecture.md §窗口管理 + `2026-07-06-archived-design.md`）
- 物理逻辑坐标转换（AGENTS.md 坐标踩坑章节）
- NSWindow ignoresMouseEvents + NSTrackingArea 文档（Apple developer docs）
