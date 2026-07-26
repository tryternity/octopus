# 区域录屏 + 实时标注 — 设计规格（spec）

> **状态：✅ 已实现**（2026-07-26，区域选区 + 标注 overlay + 部分穿透，e2e 通过）。
> 实施 commit `33e321c1`..`a3769582`，详见 [plan](../plans/2026-07-25-record-area-annotation.md)。
>
> **2026-07-26 重构**（commit `d587fc1f`/`bbfebf57`/`323ba014`/`acc65cbc`）：
> - **窗口改全屏**——不再用"窄窗口 + 后端三选"（§1.3 已废弃），改用选区所在显示器全屏窗口 + Canvas CSS fixed 定位选区（与截图 Screenshot 同模式）。
> - **工具栏位置复用截图算法**——前端用 `computeToolbarPosition`（`components/Annotation/position.ts`），与截图完全同算法；位置算完 `invoke("set_toolbar_zone")` 回传给后端 poller 判穿透。
> - **RecordAnnotation 工具栏加暂停 + 时长**——与 RecordControl pill 同范式（红点 pulse + mm:ss + 暂停/继续按钮）。
> - **AreaPicker 实时跟随工具栏**——拖框中工具栏就显示（含尺寸 + 「开始录制」+ 「取消」），与截图同体验。
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

### 0.2 关键技术验证（最终结论）

**决定性验证**（2026-07-25，Tauri 真实窗口 e2e）：
- helper `Source::Area` + Tauri 创建 `always_on_top(true)` 透明窗口
- 用户区域录制 + 画矩形标注 + 停止 → **视频里有标注**（用户视觉确认）
- ✅ **always_on_top 窗口被 SCK 录到**（标注进视频，且总在最上）

**之前 PyObjC spike 的错误结论**（已推翻）：
- spike3/6 用 Python subprocess + NSWindow/NSPanel，结论「always_on_top 不被录」
- 实际原因：Python subprocess 没起 NSApplication.run()，窗口对象创建了但**没真正进入窗口服务器**
- 用 Tauri 真实窗口（与实际实现一致）验证后，always_on_top 正常被录到

**最终方案**：
- 标注 overlay 用 **always_on_top**（总在最上，满足「录制框+标注+工具栏永远浮在顶层」）
- 透传模式：`setIgnoreMouseEvents(true)` 鼠标穿透到下层应用
- 标注进视频（SCK 录到 always_on_top 窗口内容）
- **方案完全成立，无任何限制**

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

### 1.2 overlay 窗口属性（定稿）

| 属性 | 值 | 理由 |
|---|---|---|
| `transparent` | true | 透明背景，Canvas 区域只画标注 |
| `decorations` | false | 无标题栏/边框 |
| `always_on_top` | **true** | ✅ 总在最上（Tauri e2e 验证被 SCK 录到）|
| `resizable` | false | 尺寸固定 |
| `skip_taskbar` | true | 不出现在 Dock |
| `shadow` | false | 无阴影（透明窗口不需要）|
| `visible` | true | 直接显示（不像 picker 要 ready 同步）|
| **尺寸** | **选区所在显示器全屏**（2026-07-26 重构）| 与截图同模式——工具栏用 fixed 浮层，不受选区宽度限制 |

### 1.3 窗口尺寸策略（2026-07-26 重构：全屏窗口 + 前端算工具栏位置）

> **⚠️ 原"窄窗口 + 后端三选"已废弃**（commit `d587fc1f`）。
> 原方案窗口 = 选区 + 工具栏空间（252px 扩展），后端 Rust 算 below/above/inside。
> 问题：① 窄选区时工具栏被窗口边界截断；② Rust 三选算法与前端 `computeToolbarPosition` 重复且不对齐（POPOVER_H 差异导致位置 bug）。

**新方案**（与截图 Screenshot 完全同模式）：
- **窗口 = 选区所在显示器全屏**（`win_x/y/w/h = mon_x/y/w/h`）
- **Canvas 通过 CSS `position:fixed` 定位到选区位置**（`canvas_ox/oy/w/h` URL 参数注入）
- **工具栏位置前端算**——`computeToolbarPosition(canvasRect, viewportH)`（`components/Annotation/position.ts`，与截图同算法）
- **TOOLBAR_ZONE 前端回传**——mount 后 `invoke("set_toolbar_zone", {x,y,w,h})` 把工具栏实际位置传给后端 poller（判穿透用）

**URL 参数**（注入 `record-annotation.html`）：
```
?display_id=5&x=...&y=...&width=...&height=...&scale=2
&toolbar=auto                    // "auto" = 前端用 computeToolbarPosition
&canvas_ox=选区相对显示器原点X    // Canvas CSS left
&canvas_oy=选区相对显示器原点Y    // Canvas CSS top
&canvas_w=选区宽                  // Canvas CSS width
&canvas_h=选区高                  // Canvas CSS height
```

前端 RecordAnnotation.tsx 不再读 `toolbar` 参数做三分支，直接调 `computeToolbarPosition` + `invoke("set_toolbar_zone")`。

### 1.4 与 helper 的关系

**helper 零改动**——已经支持 Source::Area（commit `ebb43cc5`）。helper 只管录屏幕选区那块（`SCStreamConfiguration.sourceRect`），overlay 窗口是 macOS 合成层的事，helper 无感知。

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
- **工具栏实时跟随**（2026-07-26 加，与截图同体验）：sel 存在即显示工具栏（`computeToolbarPosition` 算位置），含尺寸提示 + 「开始录制」+ 「取消」按钮。mouseup 不自动 confirm——用户点「开始录制」才调 `confirm_record_area_picker`
- Esc / 右键 → invoke `cancel_record_area_picker`
- 实时尺寸提示（物理像素 `Math.round(w*dpr) × Math.round(h*dpr)`，工具栏内显示）

### 2.3 坐标转换（复用 screenshot_geometry）

`confirm_record_area_picker(app, win_label, x, y, w, h)`：
1. 拿窗口原点（macOS 用 `screenshot_commands::get_window_cocoa_frame`，改 pub(crate)）
2. 构造 `MonitorRect[]`
3. `compute_selection_global` → `find_monitor_for_point`（选区中心点命中）→ `compute_physical_crop`
4. 查 `display_id`（`CGDisplay::active_displays` + bounds 命中，复用 `active_display_for_point`）
5. emit `record-area://selected` 给 record_config_window
6. 关所有 picker + show 配置浮窗

## 3. 标注 overlay 窗口（核心功能）

### 3.1 后端 `record_annotation_window.rs`（定稿）

**模块级状态**：
```rust
static ANNOTATION_PASSTHROUGH: AtomicBool  // false=标注模式, true=穿透模式
static TOOLBAR_ZONE: Mutex<(f64, f64, f64, f64)>  // 工具栏在窗口内的逻辑坐标 (x, y, w, h)
```

**`create_annotation_window(app, selection)`**（2026-07-26 重构后）：
1. 从 `Source::Area` 提取 display_id + x/y/width/height（物理像素）
2. 匹配 Tauri monitor（物理 → 逻辑坐标 / scale）
3. **窗口 = 选区所在显示器全屏**（不再算三选/扩展空间）
4. `TOOLBAR_ZONE` 初始化为全 0（前端 mount 后通过 `set_toolbar_zone` 回传实际位置）
5. URL 注入 `canvas_ox/oy/w/h`（选区相对显示器原点的逻辑坐标）+ `toolbar=auto` + `scale`
6. 创建 `always_on_top(true)` 透明全屏窗口
7. 启动 poller

**`set_toolbar_zone(_app, x, y, w, h)`**（Tauri 命令，2026-07-26 新增）：
- 前端 mount 后调，把 `computeToolbarPosition` 算出的工具栏实际位置传给后端
- poller 据此判定穿透（工具栏区域不穿透，其他穿透）

**`start_annotation_click_through_poller(app)`**（33ms tick）：
- 标注模式（passthrough=false）→ 整个窗口不穿透（`setIgnoresMouseEvents(false)`）
- 穿透模式（passthrough=true）→ 按光标位置区分：
  - 光标在 `TOOLBAR_ZONE` 内（前端回传） → 不穿透（可点工具栏切回标注）
  - 光标不在 → 穿透（操作下层应用）

**`set_annotation_passthrough(_app, passthrough: bool)`**（Tauri 命令）：
- 写 `ANNOTATION_PASSTHROUGH` AtomicBool，poller 下一 tick 生效

**`close_annotation_window(app)`**：destroy 窗口（poller 自动退出）

**`set_annotation_ignores_mouse(win, ignore)`**：双保险——Tauri `set_ignore_cursor_events`（同步）+ NSWindow `setIgnoresMouseEvents`（`run_on_main_thread` 异步），与 `result_window` 同模式。

### 3.2 前端 `RecordAnnotation.tsx`（定稿）

**URL 参数解析**（mount 时）：
```tsx
const params = new URLSearchParams(window.location.search);
const canvasRect = { ox, oy, w, h };  // Canvas 在窗口内的位置 + 尺寸（选区相对显示器原点）
// toolbar=auto 标识前端用 computeToolbarPosition（不再读 below/above/inside）
```

**Canvas**：`position: fixed; left/top/width/height = canvasRect`。Canvas 像素 buffer = `canvasRect.w * dpr` / `canvasRect.h * dpr`。标注坐标 = `e.clientX - canvasRect.ox`（Canvas 局部坐标）。

**工具栏位置**（2026-07-26 重构后，与截图同算法）：
```tsx
const tbPos = computeToolbarPosition(canvasRect, window.innerHeight);
const toolbarTop = tbPos.y;
const popoverY = tbPos.belowOrAbove ? toolbarTop + TOOLBAR_H : Math.max(0, toolbarTop - 200);
const toolbarCenterX = computeToolbarCenterX(canvasRect, window.innerWidth, toolbarW);
// mount 后回传给后端 poller
useEffect(() => { invoke("set_toolbar_zone", {x, y, w, h}); }, [tbPos, ...]);
```

**工具栏按钮**（复用截图 `<AnnotationToolbar>`）：
- 9 种标注工具（none/rect/oval/diamond/line/arrow/pen/text/number/blur）
- undo / redo（Cmd+Z / Cmd+Shift+Z）
- **录制时长 mm:ss**（红点 pulse，2026-07-26 加，与 RecordControl pill 同范式）
- **暂停/继续按钮**（2026-07-26 加，invoke `record_pause`/`record_resume`）
- 停止录制按钮（红色，emit `record://stop-requested`）

**标注交互**（复用 `@/lib/annotation`）：
- mousedown/move/up 画标注（pen 的 points / text 的 textarea / number 序号递增）
- none 工具选中标注（`hitTestAnnotationPrecise`）+ 拖动 + Delete 删除

**穿透切换**（定稿）：
- `onToolSelect(t)` → `invoke("set_annotation_passthrough", { passthrough: t === "none" })`
- select 按钮 onClick → `passthrough: true`
- Esc 退出工具（回 none）→ `passthrough: true`
- mount 时默认 `passthrough: true`（tool 默认 none）

**录制状态/时长**（2026-07-26 加，与 RecordControl 同范式）：
- mount 时 `invoke("get_record_status")` 拿真实 state + elapsed_secs（修 duration=0 bug）
- 监听 `record://event` 的 pause/resume/stop 更新本地 recState
- 本地 setInterval 在 recState === "recording" 时累加 elapsed

### 3.3 鼠标穿透模型（定稿）

**最终方案**——按工具栏当前工具切换（非快捷键，非光标位置）：

| 工具 | 模式 | 工具栏区域 | 选区（Canvas）区域 | 用户能做什么 |
|---|---|---|---|---|
| **select**（none，默认） | 穿透 | 不穿透 | **穿透** | 操作下层应用（录屏内容） |
| **rect/arrow/pen/text...** | 标注 | 不穿透 | 不穿透 | 画标注、选/删/移动 |

**切换方式**：点工具栏「鼠标」按钮→穿透模式；点任何标注工具→标注模式。

**穿透模式下工具栏区域不穿透**（`TOOLBAR_ZONE` + poller）——用户在穿透模式下仍能点工具栏按钮切回标注模式。

**调试历程**（为什么是这个方案）：
1. ❌ A 键切换→穿透后键盘也穿透，A 进下层编辑器无法切回
2. ❌ 全局快捷键切换→需额外注册 + config 配置，且无法保证「工具栏永远可点」
3. ❌ poller 按光标在窗口内/外区分→光标在选区区域（窗口内）不穿透，无法操作下层
4. ❌ poller 按窗口底部 252px 判定工具栏区域→选区底部 200px 也被误判为工具栏
5. ✅ **按工具切换 passthrough + poller 按 TOOLBAR_ZONE 精确判定**——最终方案

**`TOOLBAR_ZONE` 高度**：`TOOLBAR_H(44) + MARGIN(8) = 52px`（不含 popover 200px）。
原因：select 状态下 popover 不弹出（只点工具栏按钮 44px）；标注模式下整个窗口不穿透，popover 自然可操作。

### 3.4 标注数据持久化（MVP 不做）

标注是实时合成进视频的（overlay 窗口内容被 SCK 录到），**不单独保存标注数据**（视频里已经有了）。如果要支持「编辑已有标注」，需要把 annotations 数组序列化（推迟到 P2）。

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
