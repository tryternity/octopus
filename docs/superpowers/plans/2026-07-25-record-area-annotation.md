# 区域录屏 + 实时标注 — 实施计划（plan）

> **Spec**: `docs/superpowers/specs/2026-07-25-record-area-annotation-design.md`
> **预估**：1000+ 行新代码（4 entry + 2 后端模块 + 标注 UI 复用 lib/annotation）

## 全局约束

- 仅 macOS（cfg gate）
- 复用 `@/lib/annotation`（标注渲染/命中检测已抽离）
- 复用 `screenshot_geometry` + `screenshot_commands` 辅助函数（改 pub(crate)）
- helper 零改动（Area capture 已实现）
- 默认透传模式 / A 键切换标注 / 工具栏顶部居中

## 任务分解

### Task 1: screenshot_commands 辅助函数提升 pub(crate)

**Files:**
- Modify: `crates/desktop/src/screenshot_commands.rs`

**Steps:**
- [x] `get_window_cocoa_frame` 改 `pub(crate)`
- [x] `get_primary_screen_height` 改 `pub(crate)`
- [x] `active_display_for_point` 改 `pub(crate)`（如存在；否则按需新增）
- [x] 验证 `cargo check -p octopus-desktop --features embedded,custom-protocol` 0 error

### Task 2: 后端 `record_area_picker.rs`（选区 picker 窗口管理）

**Files:**
- Create: `crates/desktop/src/record_area_picker.rs`

**窗口创建**参考 `screenshot_commands::start_screenshot`（L67-227）：
- label 前缀 `record_area_picker_{session_id}_{i}`
- URL `area-picker.html`
- **不截图**（删 `capture_all_monitors` / `PENDING_IMAGES` / `ALL_CAPTURES` 相关）
- 保留并发门控 + READY_COUNT 同步
- picker 显示前 hide record_config_window

**坐标换算完全复用 screenshot 的调用链**（参考 `screenshot_commands::start_scroll_recording` L935-1010，这是唯一验证过的完整路径）：

`confirm_record_area_picker(app, win_label, x, y, w, h)` 实现步骤：
```rust
// 1. 拿 picker 窗口原点（Cocoa frame + Y 翻转，与 start_scroll_recording L937-948 完全一致）
let primary_h = crate::screenshot_commands::get_primary_screen_height();
let (cx, cy, _, ch) = crate::screenshot_commands::get_window_cocoa_frame(&sel_win)?;
let win_origin_x = cx;
let win_origin_y = primary_h - (cy + ch);  // Quartz 原点在左下，转屏幕左上原点

// 2. 选区全局化（复用 screenshot_geometry）
let sel = crate::screenshot_geometry::compute_selection_global(
    win_origin_x, win_origin_y, x, y, w, h,
);

// 3. 构造 MonitorRect[]（Tauri monitor 物理 → 逻辑 / scale，与 start_scroll_recording L966-979 完全一致）
let monitors: Vec<MonitorRect> = app.available_monitors()?.iter().map(|m| {
    let sf = m.scale_factor();
    MonitorRect {
        x: m.position().x as f64 / sf,
        y: m.position().y as f64 / sf,
        w: m.size().width as f64 / sf,
        h: m.size().height as f64 / sf,
        scale: sf,
    }
}).collect();

// 4. 命中检测（选区中心点，与 start_scroll_recording L980-984 一致）
let mon_idx = crate::screenshot_geometry::find_monitor_for_point(
    &monitors,
    sel.x + w / 2.0,
    sel.y + h / 2.0,
).or_else(|| (!monitors.is_empty()).then_some(0));

// 5. 物理裁剪（复用 screenshot_geometry）
let crop = crate::screenshot_geometry::compute_physical_crop(&sel, &monitors[mon_idx.unwrap()]);

// 6. 查 display_id（复用 Task 1 新增的 active_display_for_point）
let display_id = crate::screenshot_commands::active_display_for_point(
    sel.x + w / 2.0,
    sel.y + h / 2.0,
);

// 7. emit 给 record_config_window（物理像素，与 Source::Area 对齐）
app.emit("record-area://selected", json!({
    "display_id": display_id,
    "x": crop.px as i32, "y": crop.py as i32,
    "width": crop.pw, "height": crop.ph,
}))?;

// 8. 关 picker + show 配置浮窗
close_all_record_area_picker_windows(&app);
crate::record_window::show_record_window(&app);
```

**关键不变量**（与 screenshot 一致）：
- 选区中心点命中显示器（不是左上角）—— 避免跨屏选区命中错误显示器
- 物理像素输出（与 protocol.rs::Source::Area + DisplayInfo.width/height 同体系）
- Y 轴翻转（Quartz 左下原点 → 屏幕左上原点）

命令清单：
- [x] `start_record_area_picker(app)` — 创建多屏 picker 窗口
- [x] `show_record_area_picker_window(app)` — 前端 ready 后累加 READY_COUNT
- [x] `confirm_record_area_picker(app, win_label, x, y, w, h)` — 拖完即调（坐标换算如上）
- [x] `cancel_record_area_picker(app)` — Esc/右键，关 picker + show 配置浮窗
- [x] `close_all_record_area_picker_windows(app)` — 内部函数
- [x] 验证 `cargo check` 0 error

### Task 3: 前端 picker entry + AreaPicker 组件

**Files:**
- Create: `crates/desktop/frontend/area-picker.html`
- Create: `crates/desktop/frontend/src/entries/area-picker-main.tsx`
- Create: `crates/desktop/frontend/src/pages/AreaPicker/index.tsx`（约 200 行）

参考 Screenshot/index.tsx 精简版：
- [x] `area-picker.html`（参考 record-config.html 模板，透明浮窗）
- [x] `area-picker-main.tsx`（mountApp `<AreaPicker />`）
- [x] AreaPicker 组件：
  - mount 时 invoke `show_record_area_picker_window`
  - Canvas 全屏 + 半透明黑遮罩 `rgba(0,0,0,0.5)`
  - mousedown/move/up 拖框（normalize + clamp）
  - mouseup：选区 <10px 丢弃回 idle；≥10px 立即 invoke `confirm_record_area_picker`（拖完即确认）
  - draw：暗遮罩 + 蓝色边框 `#3b82f6` + 实时尺寸提示（物理像素）
  - Esc/右键 → invoke `cancel_record_area_picker`
- [x] 验证 `npm run build` 0 error

### Task 4: RecordConfig AreaPanel 接入

**Files:**
- Modify: `crates/desktop/frontend/src/pages/RecordConfig/index.tsx`

- [x] 主组件加 `listen("record-area://selected")` → setAreaSelection(payload)
- [x] AreaPanel 改造：无 selection 显示「选择区域」按钮（hide 浮窗 + invoke `start_record_area_picker`）；有 selection 显示摘要 + 「重新选择」/「清除」
- [x] locale 加 `areaPick` / `areaReselect` / `areaClear` 等 key（zh + en）
- [x] 验证 `npm run build` 0 error

### Task 5: 配置接入（picker 部分）

**Files:**
- Modify: `crates/desktop/frontend/vite.config.ts`（加 `area-picker` entry）
- Modify: `crates/desktop/capabilities/default.json`（windows 加 `record_area_picker_*`）
- Modify: `crates/desktop/src/main.rs`（mod record_area_picker + 注册 4 命令）
- [x] 验证：手动 e2e Cmd+Shift+R → area tab → 选择区域 → 拖框 → 浮窗回显摘要

### Task 6: 后端 `record_annotation_window.rs`（标注 overlay 窗口管理）

**Files:**
- Create: `crates/desktop/src/record_annotation_window.rs`

- [x] `create_annotation_window(app, selection)` — 按选区创建 overlay 窗口（label `record_annotation_window`，URL `record-annotation.html`）
- [x] `show_annotation_window(app)` / `hide_annotation_window(app)` / `close_annotation_window(app)`
- [x] `set_annotation_passthrough(app, passthrough: bool)` — 切换 `setIgnoreMouseEvents`
- [x] 调用时机：在 `record_commands::start_with_config` 成功后 + Source::Area 时调 create；stop 时 close
- [x] 验证 `cargo check` 0 error

### Task 7: 前端标注 entry + RecordAnnotation 组件

**Files:**
- Create: `crates/desktop/frontend/record-annotation.html`
- Create: `crates/desktop/frontend/src/entries/record-annotation-main.tsx`
- Create: `crates/desktop/frontend/src/pages/RecordAnnotation/index.tsx`（约 400 行）

**复用 `@/lib/annotation`**：Annotation / Tool 类型 + drawAnnotation / drawAnnotationScaled / hitTestAnnotationPrecise / annBounds 等。

- [x] `record-annotation.html` + `record-annotation-main.tsx`
- [x] RecordAnnotation 组件：
  - mount 时拉取选区参数（emit `record-annotation://init` 传 selection）
  - 全屏 Canvas（透明背景）
  - 顶部浮动工具栏（复用 ToolButton / ToolPropsPopover 样式）：
    - 9 种工具（rect/oval/diamond/line/arrow/pen/text/number/blur）
    - 颜色 / 线宽 / 字号 popover
    - undo / redo
    - 透传/标注 toggle 按钮
    - 关闭按钮（停止录制）
  - 默认透传模式（setIgnoreMouseEvents true）
  - A 键切换标注/透传（调 `set_annotation_passthrough`）
  - 标注模式：mousedown/move/up 画标注（复用 Screenshot 的 hitTest 逻辑）
  - 透传模式：工具栏半透明 + 浮动提示「按 A 切回标注模式」
- [x] 验证 `npm run build` 0 error

> **Follow-up（2026-07-26，commit `f1eeb455`）**：录制边框延迟显示 bug 修复。
> RecordAnnotation draw useEffect 依赖列表 `[annotations, drawingVer]` 漏了 `canvasRect`——
> URL 解析 setCanvasRect 后不触发 draw，导致录制开始时边框不画，要等到首次标注操作
> 才出现。修复：依赖列表加 `canvasRect`。纯 bugfix，无新 spec。

> **Follow-up（2026-07-26，commit `d587fc1f`/`acc65cbc`/`bbfebf57`/`323ba014`）**：
> 1. **RecordAnnotation 改全屏窗口**——窗口从"窄窗口 + 后端三选"改为选区所在显示器全屏（与截图 Screenshot 同模式）。Canvas CSS fixed 定位选区，工具栏用 `computeToolbarPosition`（`components/Annotation/position.ts`，与截图同算法）。
> 2. **新增 `set_toolbar_zone` 命令**——前端 mount 后把工具栏实际位置传给后端 poller（判穿透用），后端不再猜测工具栏位置。
> 3. **工具栏位置 bug 修复**——去掉 POPOVER_H（200px），与截图 computeToolbarPosition 完全对齐（只看 TOOLBAR_H=44px）。
> 4. **RecordAnnotation 工具栏加录制时长 + 暂停按钮**——与 RecordControl pill 同范式（红点 pulse + mm:ss + 暂停/继续按钮）。
> 5. **AreaPicker 实时跟随工具栏**——拖框中工具栏就显示（含尺寸 + 「开始录制」+ 「取消」），松手不自动 confirm，用户点按钮才确认。

### Task 8: 配置接入（annotation 部分）

**Files:**
- Modify: `vite.config.ts`（加 `record-annotation` entry）
- Modify: `capabilities/default.json`（windows 加 `record_annotation_window`）
- Modify: `main.rs`（mod record_annotation_window + 注册 set_annotation_passthrough 命令）
- [x] 验证：手动 e2e 完整流程（选区 → 录制 → 画标注 → 视频里有标注）

### Task 9: 文档同步

- [x] z-sync-superpowers：spec 已新建，回写 architecture.md + 主 screen-record spec 引用
- [x] manual e2e 验收（spec §5.1 完整流程）

## 实施顺序

Task 1（screenshot 辅助 pub）→ 2（picker 后端）→ 3（picker 前端）→ 4（RecordConfig 接入）→ 5（picker 配置）→ **手动 e2e 验证区域选区** → 6（annotation 后端）→ 7（annotation 前端）→ 8（annotation 配置）→ **手动 e2e 验证标注被录进视频** → 9（文档同步）

每步独立验证编译，避免大爆炸。Task 5 后停下让你 e2e 验证选区；Task 8 后停下让你 e2e 验证标注。
