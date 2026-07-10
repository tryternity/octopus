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
2. CSS 把大部分区域 `transparent` + 全窗口 `setIgnoresMouseEvents(true)`
3. 8px 细条贴着吸附边缘高亮显示，用 NSWindow tracking area 标记可接收鼠标事件
4. 其余 292px 透明 + 穿透（看到下层 app / 桌面）

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

| 当前状态 | 触发 | 目标状态 | 说明 |
|---------|------|---------|------|
| Normal | 拖拽放手 + 边缘 ≤10px | Docked-Collapsed | 吸附 + 收缩 |
| Normal | 拖拽放手 + 边缘 >10px | Normal | 保存位置（现有逻辑） |
| Docked-Collapsed | 鼠标悬停细条 | Docked-Expanding | CSS 动画展开 |
| Docked-Expanding | 动画完成（300ms） | Docked-Expanded | |
| Docked-Expanded | 鼠标点击浮窗外 | Docked-Collapsing | CSS 动画收回 |
| Docked-Collapsing | 动画完成（300ms） | Docked-Collapsed | |
| Docked-* | 用户拖拽窗口（拖离边缘） | Normal | 解吸附 |
| 任意 | 快捷键 toggle / X 按钮 | 隐藏 | 再打开时恢复上次模式 |

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
| `clipboard://dock-changed` | Rust → 前端 | `{ edge: "right" \| "left" \| "none" }` | 前端据此切换模式 |
| `clipboard://expand` | Rust → 前端 | — | 鼠标悬停细条 → 展开 |
| `clipboard://collapse` | Rust → 前端 | — | 鼠标点击外部 → 收缩 |

### 5.2 前端状态（`Clipboard/index.tsx`）

```
dockEdge: "right" | "left" | null     // 吸附边缘
dockMode: "none" | "collapsed" | "expanding" | "expanded" | "collapsing"
```

### 5.3 CSS 行为

| dockMode | 容器样式 | ignoresMouseEvents |
|----------|---------|-------------------|
| none | 正常 300×600 | false |
| collapsed | 只渲染 8px 细条（贴 dockEdge 侧），其余 `display: none` | true |
| expanding | `width` transition 8px → 300px（300ms ease-out） | false |
| expanded | 完整 300×600 | false |
| collapsing | `width` transition 300px → 8px（300ms ease-out） | true |

### 5.4 展开方向

- 右吸附：内容从右侧展开（细条在最右，内容向左推）——`flex-direction: row-reverse` 或 `right: 0` 定位
- 左吸附：内容从左侧展开（细条在最左，内容向右推）——正常方向

### 5.5 外部点击收缩

- **仅移动离开不触发收缩**（防打字误触）
- Expanded 态下 Rust 侧用 `NSEvent.addGlobalMonitorForEvents(matching: .leftMouseDown | .rightMouseDown)` 监听全局鼠标点击
- 点击位置在窗口外 → emit `clipboard://collapse`

---

## 6. NSWindow 交互细节

### 6.1 setIgnoresMouseEvents

- Collapsed 态：`NSWindow.ignoresMouseEvents = true`（全窗口穿透）
- Expanding / Expanded / Collapsing 态：`false`（正常接收事件）

### 6.2 TrackingArea（细条可点击）

Collapsed 态下全窗口 `ignoresMouseEvents = true`，但 8px 细条需要可点击——用 NSTrackingArea：

- 在窗口的 8px 区域（右吸附 = 窗口右侧 8px；左吸附 = 窗口左侧 8px）创建 NSTrackingArea
- `options: .mouseEnteredAndExited | .activeAlways`
- `owner` 为自定义 NSView 子类，收到 `mouseEntered` → Rust 回调 → emit `expand`

### 6.3 全局鼠标监听（Expanded 态收缩触发）

- `NSEvent.addGlobalMonitorForEvents(matching: [.leftMouseDown, .rightMouseDown])`
- 回调中拿到鼠标位置 → 判断是否在窗口 frame 外 → emit `collapse`
- 只在 Expanded 态启用 monitor，Collapsed 态关闭（避免无谓监听）

---

## 7. 文件变更

### 7.1 Rust

| 文件 | 变更 |
|------|------|
| `clipboard_window.rs` | 吸附检测（Moved 事件）；dock 状态读写；初始打开模式判断；展开/收缩时的 `ignoresMouseEvents` 切换 |
| `window_position.rs` | 加 `save_dock_state(label, edge)` / `load_dock_state(label) -> Option<String>`（DB key `window_dock.{label}`） |
| `clipboard_commands.rs`（或新建 `clipboard_dock.rs`） | NSWindow 操作：`set_ignores_mouse_events` / `setup_dock_tracking_area` / `setup_global_click_monitor` / `teardown_global_click_monitor` |

### 7.2 前端

| 文件 | 变更 |
|------|------|
| `Clipboard/index.tsx` | dockEdge/dockMode 状态；listen 三个事件；CSS class 切换；展开方向 |
| `Clipboard/DockBar.tsx`（新建） | 8px 细条组件（高亮 voice 色 + 微阴影 + onMouseEnter） |

### 7.3 配置

无新增配置项——dock 状态纯运行时记忆（DB 存），不需用户配置。

### 7.4 文档更新（action_bar 定位策略明确）

| 文件 | 变更 |
|------|------|
| `2026-07-08-action-bar-design.md` §4.1 | 加说明：action_bar 定位策略固定为鼠标上方，不做位置记忆 / 吸附 / 拖拽 / 尺寸变更 |
| `architecture.md` 窗口管理表 | clipboard_window 行补注 dock 功能；action_bar 行补注固定定位 |

---

## 8. 不变式

- **窗口物理尺寸始终 300×600**——收缩/展开只改 CSS 可见区域 + 窗口位置 + `ignoresMouseEvents`，不改物理尺寸
- **窗口始终完全在当前屏幕内**——不越界到其他屏幕
- **`resizable(true)` 保留**——dock 功能与 resize 不冲突（resize 改的是物理尺寸，dock 只在用户主动吸附时触发）
- **位置记忆继续工作**——`window_position.rs` 存 x,y，dock 状态是额外维度（`window_dock.clipboard_window`）
- **仅 macOS**——`ignoresMouseEvents` / NSTrackingArea / NSEvent global monitor 是 macOS API

---

## 9. 边界场景

| 场景 | 处理 |
|------|------|
| 屏幕分辨率/显示器变化后打开 | 现有 `is_position_visible` 已检测；dock 状态也需类似检查——吸附边缘对应的显示器不在了 → dock=none |
| 拖拽中突然吸附 | 不做——吸附只在 Moved 最终位置（放手时）检测，拖拽过程中不中途吸附 |
| 快捷键唤出时已有 dock | 直接 Collapsed 态打开（贴边细条），用户 hover 展开 |
| 多个显示器拖拽 | Moved 事件中根据窗口中心找当前显示器，吸附到当前显示器的边缘 |
| Docked 态下窗口失焦 | 不自动收缩（Expanded 态靠点击外部收缩，不靠失焦——失焦可能因为切到其他 app，不代表用户想收缩） |

---

## 10. 信息来源

- KoBar Ghost Window 透明覆盖层模式（`~/.tolaria/桌面工具/kobar-*.md`）
- result_window 尺寸双模式踩坑（architecture.md §窗口管理 + `2026-07-06-archived-design.md`）
- 物理逻辑坐标转换（AGENTS.md 坐标踩坑章节）
- NSWindow ignoresMouseEvents + NSTrackingArea 文档（Apple developer docs）
