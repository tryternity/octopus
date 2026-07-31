# 合并 result_window 与 instant_overlay 为单 WebView 实例 — 设计规格

- **日期**：2026-08-01
- **类型**：重构（窗口架构 + 前端视图合并）
- **范围**：把现有两个 WebView 实例（`result_window` + `instant_overlay`）合并为一个，减少内存占用；窗口内部按录音模式渲染不同视图
- **动机**：两个透明穿透窗实例各占一份 WebView 内存；两者都经 `build_float_window` 同样的透明/无边框/置顶/skip-taskbar 处理，基础设施重复。合并为单实例 + 视图切换可省一份 WebView 开销

## 背景：现状两窗口

| 维度 | `result_window` | `instant_overlay` |
|---|---|---|
| label | `result_window` | `instant_overlay` |
| HTML | `result.html` → Result page（CM6 + 工具栏） | `instant-overlay.html` → InstantOverlay page（4 态指示） |
| 尺寸 | 720×480（固定，CSS 伪装精简/长篇） | 400×80 |
| 位置 | 顶部居中（y=80），按屏记忆，首次显示跟鼠标屏 | 底部居中（8px margin），每次 show 跟鼠标屏 |
| 穿透 | 完整 poller（顶部 720×116 小条可点，下方透明区穿透） | 无（小窗整窗可点） |
| 用途 | toggle 模式（可编辑结果） | PTT/hands-free 模式（只读指示） |
| 分流 | `INSTANT_MODE: AtomicBool`——true 用 instant_overlay，false 用 result_window |

## 核心设计

**一个 WebView 实例**（label `result_window`，固定 720×480 透明穿透窗），内部 React 按 `record-mode` 事件渲染两种视图：

- **toggle 视图**：现有 Result 页（CM6 编辑器 + 工具栏 + 精简/长篇态），顶部居中
- **instant 视图**：InstantOverlay 4 态指示卡（listening/processing/polishing/done），底部居中渲染在透明区内

物理窗口位置由后端按模式决定：toggle→顶部居中、instant→底部贴屏底。穿透 poller 复用现有那套（透明区穿透）。

**删除**：`instant_overlay.rs` + `instant-overlay.html` + `InstantOverlay/` page 目录（逻辑搬进 result_window 体系）。

### 关键决策（brainstorming 确认）

1. **尺寸**：固定 720×480 + 前端 CSS 切视图（不运行时 setSize——透明无边框窗口 setSize 踩过多次坑，NSWindow 拒绝）
2. **位置**：instant 态贴底（指示卡在 720×480 透明区底部居中渲染）、toggle 态顶部居中
3. **前端**：单 HTML + React 条件渲染（`display:none` 切换，不卸载，保留 CM6 状态）
4. **后端主体**：result_window.rs 吸收 instant 逻辑（基础设施最全），instant_overlay.rs 删除

## 后端改动

### `result_window.rs` 作为主体

吸收 instant 逻辑，保留全部现有基础设施（位置记忆 + click-through poller + 快捷键注册）：

- **新增 `show_instant(state, text)`**：show 同一个窗口，emit `instant-state { state, text }` + 按 instant 模式设位置（底部贴底）
- **`show_result(text)` 保留**：emit `show-result` + 按 toggle 模式设位置（顶部居中）
- **位置策略**：show 时按 `INSTANT_MODE` 决定——true→底部贴底（沿用 instant_overlay 的 `position_bottom_center` 逻辑，搬到 result_window）、false→顶部居中（现有 `reposition_to_mouse_monitor`）。**仅首次显示 reposition**（已有逻辑，instant 态也遵守）
- **emit `record-mode`**：show 前 emit `record-mode`（payload: `"toggle" | "instant"`），前端据此渲染对应视图。值来自 `INSTANT_MODE`（true→"instant"、false→"toggle"）

### 穿透 poller 适配

现有 poller 检查光标是否在顶部 720×116 小条（BAR 区域）。合并后按模式切两组 BAR 坐标：

- **toggle 精简态**：可交互区在顶部 720×116（不变）
- **toggle 长篇态**：整窗可交互（不变）
- **instant 态**：可交互区改为底部（如 720×80 指示卡区域）

poller 的 `BAR_W`/`BAR_H`/`BAR_OFFSET` 需按模式 + `RESULT_CLICK_THROUGH` 决定区域（顶部 vs 底部）。

### `INSTANT_MODE` 语义变更

从"选哪个窗口"→"窗口渲染哪个视图 + 位置策略"。仍由 InstantStart/HandsFreeStart 设 true、PasteDone/Cancel 清 false。前端读它决定视图，后端读它决定位置 + BAR 区域。

### 删除 `instant_overlay.rs`

`show_instant_overlay`/`hide_instant_overlay` 的所有调用点（coordinator 约 15 处）改转调 result_window：

- `instant_overlay::show_instant_overlay(state, text)` → `result_window::show_instant(state, text)`
- `instant_overlay::hide_instant_overlay` → `result_window::hide_result`（统一隐藏）
- `instant_overlay::precreate` 删除（result_window 启动时已创建）

## 前端改动

### 新建 `AsrWindow` 根组件

承载两套视图（改造现有 Result page，或新建根组件）：

- 监听 `record-mode` 事件（`"toggle" | "instant"`）→ 切换视图显隐（`display:none`，**不卸载**，保留 CM6 状态）
- **toggle 视图**：现有 Result 的全部（CM6 + 工具栏 + 精简/长篇态切换 + 翻译双语）
- **instant 视图**：现有 InstantOverlay 的全部（4 态指示 + 波形/spinner），渲染在 720×480 透明区的底部居中（CSS `position:absolute; bottom; center`）
- 现有 `show-result`（文本）+ `instant-state`（状态）事件保留，前端按当前 mode 分发到对应视图

### 删除独立 instant 页

- 删除 `instant-overlay.html`
- 删除 `src/pages/InstantOverlay/`（组件逻辑搬进 AsrWindow 的 instant 视图分支）
- `result.html` 加载 AsrWindow（或新建 `asr-window.html`）

## 不变量

1. toggle 模式行为完全不变（CM6 编辑、工具栏、翻译、位置记忆）
2. PTT/hands-free 指示视觉接近现状（底部小卡，4 态）
3. 外部 app 粘贴路径不受影响
4. 全局快捷键（edit/polish shortcut）不变
5. WebView 实例从 2→1，内存下降
6. ASR 回写（self-webview paste-text）不受影响——窗口 label 仍是 `result_window`，paste-text 路径不指向本窗（被 `focused_self_webview_label` 排除）

## 风险

- **穿透 poller 双区域**：顶部 + 底部两组 BAR 坐标，逻辑变复杂。需仔细测 instant 态下指示卡可点 + 透明区穿透
- **CM6 状态保留**：toggle→instant→toggle 切换时 CM6 不能卸载（`display:none` 而非条件渲染卸载），否则编辑内容/光标丢失
- **位置切换抖动**：同会话内 toggle（顶部）→ instant（底部）位置切换窗口会跳。但实际不会同会话切换（toggle 录音中不会变 instant），可接受
- **instant 态视觉**：720×480 透明区里只渲染底部 80px 指示卡，中间大片透明——需确认穿透生效（否则挡操作）。穿透 poller 已有，但 instant 态需切到底部 BAR 区域

## 文件改动

| 文件 | 操作 |
|---|---|
| `crates/desktop/src/ui/result_window.rs` | 加 `show_instant` + 位置策略（底部贴底）+ emit `record-mode` + poller 双 BAR 区域 |
| `crates/desktop/src/ui/instant_overlay.rs` | **删除**（逻辑搬进 result_window） |
| `crates/desktop/src/engine/coordinator/` | ~15 处 `instant_overlay::show/hide` → `result_window::show_instant/hide_result` |
| `crates/desktop/src/core/setup.rs` | 删 `instant_overlay::precreate` 调用 |
| `crates/desktop/frontend/src/pages/` | Result page 改造为 AsrWindow（加 instant 视图分支）或新建 AsrWindow |
| `crates/desktop/frontend/src/pages/InstantOverlay/` | **删除**（搬进 AsrWindow） |
| `crates/desktop/frontend/instant-overlay.html` | **删除** |
| `crates/desktop/frontend/result.html` | 加载 AsrWindow（或新建 asr-window.html） |
| `crates/desktop/capabilities/default.json` | 删 `instant_overlay` window 权限（如需） |
| `docs/architecture.md` | 更新窗口说明 |

## 验证

- cargo build + cargo test（coordinator 调用点迁移全过）
- tsc + vite build（前端合并编译）
- e2e：① toggle 录音→顶部 result 视图（CM6 可编辑）② PTT→底部 instant 指示卡 ③ hands-free→底部指示卡 ④ 穿透：instant 态透明区可点穿 ⑤ toggle 精简/长篇态穿透不变 ⑥ ASR 回写外部窗口不受影响
