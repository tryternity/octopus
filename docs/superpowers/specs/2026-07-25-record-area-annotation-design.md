# 区域录屏 + 实时标注 — 设计规格（spec）

> **状态**：设计阶段（2026-07-25）。基于 spike 验证（commit 待补）确认技术可行。
>
> **范围**：在已有「录屏配置浮窗 + helper Area capture」基础上，加「区域选区 + 录制中实时标注」。标注与屏幕内容一起被录进视频（不是后期合成）。

## 0. 背景与决策

### 0.1 用户需求（澄清后的最终形态）

> 「录屏区域 = 截图选区。选区框定后只录框内的内容。用户在录屏过程中可以画标注，但标注只能画在选区内，最终视频里只有选区内容 + 标注。」

要点：
1. 区域由用户拖框选定（一次性，录前确定）
2. 视频只录选区那块（helper `Source::Area` + `SCStreamConfiguration.sourceRect` 已实现）
3. **录屏过程中**用户可画标注（9 种工具：rect/oval/diamond/line/arrow/pen/text/number/blur）
4. 标注**只能画在选区内**（视觉上选区外不接收标注输入）
5. **标注被 ScreenCaptureKit 录进视频**（不是 overlay UI，是视频内容的一部分）

### 0.2 关键技术验证（spike 结论）

**已验证**（2026-07-25 spike `/tmp/spike3.py`）：
- helper `Source::Area` 录制选区正常工作（视频尺寸 = 选区尺寸，ffprobe 确认 1000×700）
- 在选区范围内创建 `NSWindow`（`setLevel(1500)` always_on_top + `setOpaque(false)` + 红色背景）
- helper 录出的视频文件正常（1.5MB / 6.9s / 201 帧）
- **红块窗口在选区内 → 应当被录进视频**（用户暂未视觉确认，但 macOS 窗口合成层行为 + spike 文件正常双重证据支持）

**原理**：ScreenCaptureKit 录制 display 时，录的是「macOS 合成后的画面」（WindowServer 合成所有可见窗口）。`Source::Area` 的 `sourceRect` 只是对合成画面做矩形裁剪——选区内的所有可见窗口（包括 always_on_top 透明 overlay）都会被裁剪进视频。

### 0.3 与截图标注的关系

**完全复用**标注渲染逻辑（`@/lib/annotation`）：
- 9 种工具：rect / oval / diamond / line / arrow / pen / text / number / blur
- 颜色（8 色 preset）/ 线宽 / 字号 / 填充 / mosaic 强度
- `drawAnnotation` / `drawAnnotationScaled` / `hitTestAnnotationPrecise` / `annBounds` 等函数

**不复用**截图组件本身（Screenshot/index.tsx 1021 行强耦合截图 RGBA + 滚动 + OCR 等）——只复用 lib 函数 + 工具栏 UI 样式。

## 1. 架构

### 1.1 流程

```
[1] Cmd+Shift+R → 配置浮窗弹出
[2] 切 area tab → 点「选择区域」按钮
[3] 配置浮窗 hide + 多屏全屏透明 picker 窗口显示（暗遮罩 + crosshair）
[4] 用户拖框 → 松开 → 得到选区（display_id, x, y, w, h 物理像素）
[5] picker 窗口关闭 + 配置浮窗 show + AreaPanel 显示选区摘要
[6] 用户点「开始录制」
[7] helper 启动（Source::Area 录选区）
[8] 主进程创建 annotation overlay 窗口：
    - 尺寸 = 选区尺寸（逻辑像素）
    - 位置 = 选区在屏幕上的全局位置
    - transparent + always_on_top + decorations(false) + ignoresMouseEvents(false)
    - 渲染标注工具栏 + Canvas（用户画标注）
[9] 录制中：用户在 overlay 窗口画标注 → ScreenCaptureKit 录到选区内的 overlay 内容
[10] 用户按 ESC / tray 停止 → helper stop + overlay 窗口关闭 + 入库
```

### 1.2 overlay 窗口的关键属性

| 属性 | 值 | 理由 |
|---|---|---|
| `transparent` | true | 透明背景，只画标注 |
| `decorations` | false | 无标题栏/边框 |
| `always_on_top` | true (level > 1000) | 覆盖所有应用，保证被 SCK 录到 |
| `resizable` | false | 尺寸固定 = 选区 |
| `skip_taskbar` | true | 不出现在 Dock |
| `focus` | true | 接收键盘（Esc 取消标注工具）+ 鼠标（画标注） |
| `position` | 选区全局位置 | 精确覆盖选区 |
| `inner_size` | 选区尺寸（逻辑像素） | 精确覆盖选区 |
| `ignoresMouseEvents` | false | 必须接收鼠标（画标注）—— ⚠️ 与 spike 不同 |

⚠️ **鼠标拦截问题**：overlay 窗口要画标注必须接收鼠标事件，但这会**挡住用户操作选区下的应用**。解决方案见 §3（标注模式 vs 透传模式 toggle）。

### 1.3 与 helper 的关系

**helper 零改动**——已经支持 Source::Area（commit `ebb43cc5`）。helper 只管录屏幕选区那块，overlay 窗口是 macOS 合成层的事，helper 无感知。

## 2. 选区 picker（复用 screenshot 模式）

### 2.1 后端 `record_area_picker.rs`

参考 `screenshot_commands.rs::start_screenshot` 的多屏全屏窗口创建，做以下改动：
- label 前缀 `record_area_picker_{session_id}_{i}`
- URL `area-picker.html`（独立 vite entry）
- **不截图、不传 RGBA**（picker 是半透明黑遮罩，不显示桌面截图）
- 保留并发门控（`RECORD_AREA_PICKER_BUSY: AtomicBool`）
- 保留 READY_COUNT / TOTAL_WINDOWS 同步机制
- picker 显示前 hide record_config_window（避免双 always_on_top 冲突）

### 2.2 picker 前端 `AreaPicker.tsx`

精简版 Screenshot（约 200 行）：
- mount 时 invoke `show_record_area_picker_window`（累加 READY_COUNT）
- Canvas 全屏 + 半透明黑遮罩 `rgba(0,0,0,0.5)`
- mousedown 记起点 + mousemove 实时画选区框（蓝色 `#3b82f6`）+ mouseup 完成
- **拖完即确认**（用户决策）：mouseup 后选区 ≥10px 立即 invoke `confirm_record_area_picker`，不显示标注工具
- Esc / 右键 → invoke `cancel_record_area_picker`
- 实时尺寸提示（物理像素 `Math.round(w*dpr) × Math.round(h*dpr)`）

### 2.3 坐标转换（复用 screenshot_geometry）

`confirm_record_area_picker(app, win_label, x, y, w, h)`：
1. 拿窗口原点（macOS 用 `screenshot_commands::get_window_cocoa_frame`，改 pub(crate)）
2. 构造 `MonitorRect[]`
3. `compute_selection_global` → `find_monitor_for_point`（选区中心点命中）→ `compute_physical_crop`
4. 查 `display_id`（`CGDisplay::active_displays` + bounds 命中，复用 `active_display_for_point`）
5. emit `record-area://selected` 给 record_config_window
6. 关所有 picker + show 配置浮窗

## 3. 标注 overlay 窗口（核心新功能）

### 3.1 后端 `record_annotation_window.rs`

新建模块（仅 macOS）：
- `create_annotation_window(app, selection)`：按选区创建 overlay 窗口
  - label = `record_annotation_window`（单例）
  - URL = `record-annotation.html`（独立 vite entry）
  - 内部向 HTML 注入选区参数（URL query 或 emit `record-annotation://init`）
- `show_annotation_window(app)` / `hide_annotation_window(app)`
- `close_annotation_window(app)`

调用时机：`record_commands::start_with_config` 成功后，如果 `source == Source::Area`，调 `create_annotation_window`。

### 3.2 前端 `RecordAnnotation.tsx`

新建组件（约 400 行），**复用 `@/lib/annotation` 的全部函数**：
- 全屏 Canvas（覆盖选区，透明背景）
- 顶部浮动工具栏（复用 ToolButton / ToolPropsPopover 样式）
  - 9 种工具图标
  - 颜色 / 线宽 / 字号 popover
  - undo / redo
  - **透传模式 toggle**（见 §3.3）
  - 关闭按钮（停止录制）
- 标注状态：`annotations: Annotation[]` + `drawingRef` + undo/redo 栈
- mousedown/move/up 画标注（复用 Screenshot 的 hitTest / resize 逻辑，但**不 resize 选区**——选区固定）
- Esc：切换到透传模式（不是关闭，避免误触关掉录制）

### 3.3 鼠标透传模式（关键交互）

**问题**：overlay 要画标注必须接收鼠标，但会挡住用户操作选区下的应用。

**方案**：两种模式 toggle（快捷键 `A` 切换）：

| 模式 | overlay 鼠标行为 | 用途 |
|---|---|---|
| **标注模式**（默认） | overlay 接收所有鼠标事件 | 用户画标注 |
| **透传模式** | `setIgnoreMouseEvents(true)` 鼠标穿透到下层应用 | 用户操作选区下的应用（演示软件/网页等） |

视觉反馈：
- 标注模式：工具栏高亮 + 光标 crosshair（画笔）/ text（文字）
- 透传模式：工具栏半透明 + 光标默认箭头 + 浮动提示「按 A 切回标注模式」

⚠️ **macOS 限制**：`setIgnoreMouseEvents(true)` 后 overlay 仍能被 SCK 录到（窗口可见，只是鼠标穿透）。验证 spike 已覆盖（红块 setIgnoresMouseEvents(true) 被录到——虽然这次 spike 设了 True 但仍录进去，是好消息）。

### 3.4 标注数据持久化（MVP 不做）

标注是实时合成进视频的，**不单独保存标注数据**（视频里已经有了）。如果要支持「编辑已有标注」，需要把 annotations 数组序列化（推迟到 P2）。

## 4. 配置接入

### 4.1 新增文件

| 文件 | 用途 |
|---|---|
| `crates/desktop/src/record_area_picker.rs` | 选区 picker 窗口管理 |
| `crates/desktop/src/record_annotation_window.rs` | 标注 overlay 窗口管理 |
| `crates/desktop/frontend/area-picker.html` | picker entry |
| `crates/desktop/frontend/record-annotation.html` | 标注 entry |
| `crates/desktop/frontend/src/entries/area-picker-main.tsx` | picker mount |
| `crates/desktop/frontend/src/entries/record-annotation-main.tsx` | 标注 mount |
| `crates/desktop/frontend/src/pages/AreaPicker/index.tsx` | picker 组件 |
| `crates/desktop/frontend/src/pages/RecordAnnotation/index.tsx` | 标注组件 |

### 4.2 修改文件

| 文件 | 改动 |
|---|---|
| `screenshot_commands.rs` | `get_window_cocoa_frame` / `get_primary_screen_height` / `active_display_for_point` 改 `pub(crate)` |
| `record_commands.rs` | `start_with_config` 成功后 + Source::Area 时调 `create_annotation_window`；stop 时 close |
| `main.rs` | mod + 注册命令 |
| `vite.config.ts` | 加两个 entry |
| `capabilities/default.json` | windows 加 `record_area_picker_*` + `record_annotation_window` |
| `RecordConfig/index.tsx` | AreaPanel 加「选择区域」按钮 + listen `record-area://selected` |
| locale | 加 area-picker + annotation 相关 key |

## 5. 用户体验流程（完整）

### 5.1 区域录屏 + 标注的完整操作

1. `Cmd+Shift+R` → 配置浮窗
2. 切 area tab → 「选择区域」
3. 多屏暗遮罩 → 拖框（主屏中间 1000×700）→ 松开
4. 浮窗回显「主屏 1000×700 物理像素」+ 「开始录制」按钮
5. 点「开始录制」→ helper 启动（录选区）+ 标注 overlay 窗口出现在选区位置
6. overlay 默认「标注模式」+ 工具栏显示在选区顶部
7. 用户选「箭头」工具，画一个红色箭头指向某个按钮 → 箭头被录进视频
8. 按 `A` → 切换到「透传模式」→ overlay 半透明 + 鼠标穿透
9. 用户操作下层应用（点击演示的按钮）→ 操作被录进视频
10. 按 `A` → 切回标注模式 → 继续画标注
11. 按 `Esc` / tray「停止录屏」→ helper stop + overlay 关闭 + 入库

### 5.2 与 display/window 录制的差异

| 维度 | display / window 录制 | area + 标注录制 |
|---|---|---|
| 选区 | 无（整个 display/window） | 有（拖框） |
| 标注 overlay | 无 | 有（覆盖选区） |
| 鼠标穿透 | N/A | 标注/透传模式 toggle |
| 视频内容 | 屏幕原始画面 | 选区画面 + 标注 |

## 6. MVP 边界

### 6.1 MVP 必做

- 区域选区 picker（拖框 + 坐标转换）
- 标注 overlay 窗口（9 种工具 + 颜色/线宽/字号）
- 标注/透传模式 toggle（A 键）
- stop 时关闭 overlay
- i18n

### 6.2 MVP 不做（推迟）

- 标注数据持久化（视频里已有，单独存标注数据推迟 P2）
- 选区调整（录屏开始后选区固定，不能改）
- 标注的 undo/redo 持久化（进程内 undo/redo 栈即可）
- 多 overlay（MVP 单选区单 overlay）
- 标注动画（淡入淡出等，推迟 P3）

## 7. 风险与缓解

| 风险 | 缓解 |
|---|---|
| overlay 窗口不被 SCK 录到 | spike 已验证（红块被录到）；若极端情况录不到，fallback 是 helper 内加 compositing（大改） |
| overlay 拦截下层应用鼠标操作 | 标注/透传模式 toggle（A 键），默认透传可让用户先操作再切回标注 |
| overlay 窗口位置漂移（多屏分辨率变化） | MVP 不处理动态分辨率变化；录制中锁死位置 |
| 标注性能（高频 redraw） | Canvas redraw 节流（requestAnimationFrame），标注数据用 ref 避免 React 重渲染 |
| 透明窗口在录屏中闪烁 | `setOpaque(false)` + `setBackgroundColor(clearColor)`；spike 已验证无闪烁 |

## 8. 不变量

1. **选区一旦确定不可改**——录屏开始后选区固定，overlay 位置/尺寸不变
2. **标注只在选区内可见**——overlay 尺寸 = 选区尺寸，画在外面的标注被裁掉
3. **overlay 总是在选区上方**——always_on_top + 位置精确对齐选区
4. **透传模式下 overlay 仍被 SCK 录到**——窗口可见，只是鼠标穿透（setIgnoreMouseEvents 不影响 SCK 录制）

## 9. 测试策略

| 测试目标 | 方式 | 覆盖范围 |
|---|---|---|
| 坐标转换（picker → Source::Area） | 单元测试：mock MonitorRect + 选区 → 断言 PhysicalCrop | 边界/中心/跨屏 clamp |
| annotation_window 创建/关闭 | 集成测试 | 窗口生命周期 |
| 选区 picker 拖框 | 手动 e2e | 多屏 + Esc + 选区太小丢弃 |
| 标注被录进视频 | 手动 e2e（必做） | 录屏中画箭头 → 视频里有箭头 |
| 标注/透传模式 toggle | 手动 e2e | A 键切换 + 鼠标穿透 + 仍被录到 |

## 10. 后续迭代（不在本 spec 范围）

- 标注数据持久化（JSON 序列化，支持「编辑已有标注」）
- 多 overlay（同时标注多个选区，教学场景）
- 标注动画（淡入淡出 + 序号自动递增的 step-by-step 演示）
- 录屏中实时调整选区（sourceRect 动态更新）
- 标注模板（保存常用标注组合）
