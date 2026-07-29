# 实施计划：录制控制浮窗（P1-7）

> Spec: [`2026-07-26-record-control-window.md`](../specs/2026-07-26-record-control-window.md)
>
> **Status: ✅ 已完成**（2026-07-29 z-sync 回填 checkbox + 修复 tray pill bug）。原遗留 bug（tray 停止路径漏调 `close_control_window`）已修复，4 条停止路径（ESC/stop-requested/kill/tray）现全部正确关闭 pill。

## 任务分解

### Task 1：后端 record_control_window.rs

**新增** `crates/desktop/src/record_control_window.rs`（~110 行）：
- `pub const WINDOW_LABEL: &str = "record_control_window";`
- `pub fn create_control_window(app, source)` —— Area 过滤跳过；非 Area 创建 pill（destroy 重建保证单例）
- `pub fn close_control_window(app)` —— destroy 窗口
- `compute_position(app, source)` —— **录制所在屏**右下角 - 16px 内边距（2026-07-26 修复前 fallback 主屏 → 副屏 bug；改用 `CGDisplay::bounds()` 精确查逻辑边界，详见下方「后续修复」）
- 窗口属性：always_on_top / transparent / decorations:false / skip_taskbar / resizable:false / shadow:false / visible:true
- 固定尺寸 WIDTH=130 / HEIGHT=38（用户反馈原 200×56 太长，commit `bbfebf57` 调整）

### Task 2：前端 RecordControl 组件（3 新增）

- `record-control.html` —— 仿 record-annotation.html（主题恢复 script）
- `src/entries/record-control-main.tsx` —— `mountApp(<RecordControl />)`
- `src/pages/RecordControl/index.tsx` —— pill 组件：
  - `useRecordSession()` 拿 state/duration/pause/resume
  - 红点 pulse + 时长 mm:ss + 暂停/恢复 SVG 按钮 + 停止红方块
  - 停止：`emit("record://stop-requested", {from:"control"})`
  - 监听 `record://stop-failed` → hide 浮窗

### Task 3：配置接入（5 改动）

- `main.rs`：mod 声明 + stop-requested handler 加 close_control_window
- `record_commands.rs`：start_with_config 加 create_control_window；record_kill 加 close（修 RecordAnnotation 泄漏）
- `record_hotkey.rs`：handle_stop 加 close_control_window
- `vite.config.ts`：input 加 record-control
- `capabilities/default.json`：windows 数组加 record_control_window

### Task 4：文档

- 新建 spec/plan（本文件）
- 更新 architecture.md（录屏章节加 record_control_window 模块）
- 更新 screen-record-design.md §8.2（P1-7 已实现，去掉 dropdown 推迟）

## 实施记录

### 编译验证

```
cargo build --release -p octopus-desktop --features embedded,cloud,custom-protocol
Finished `release` profile [optimized] target(s) in 40.84s
# 0 error 0 warning

cd crates/desktop/frontend && npm run build
✓ built in 324ms
# 0 error
```

### 影响面追踪

- `create_control_window`：定义 record_control_window.rs + 调用 record_commands.rs start_with_config 成功块
- `close_control_window`：定义 record_control_window.rs + 3 调用（main.rs stop-requested + record_hotkey handle_stop + record_commands record_kill）

### 顺手修复

- `record_kill` 路径之前只 unregister ESC，不关窗口——RecordAnnotation 在 kill 路径会泄漏。本轮一并加 close_annotation_window + close_control_window。

### 待用户实测

- [x] display 录制 → **录制所在屏**右下角 pill（红点+时长）→ 点停止 → 消失 ✅ 2026-07-26 用户验证
  - **2026-07-26 修复**：原 compute_position 丢弃 display_id 永远 fallback 主屏，副屏录制 pill 跑到主屏右下角。改用 `CGDisplay::new(display_id).bounds()` 直接查逻辑边界。详见下方「后续修复」。
- [x] **副屏 display 录制 → pill 出现在副屏右下角** ✅ 2026-07-26 用户验证（核心回归项）
- [x] window 录制 → 主屏右下角 pill（fallback，window_id → display 查询推迟）
- [x] 暂停/恢复：红点变灰 + 时长停 / 恢复
- [x] ESC 停止：pill 消失
- [x] tray 停止：pill 消失（2026-07-29 修复：tray.rs 停止路径补调 `close_control_window`，与 ESC/stop-requested/kill 路径一致）
- [x] kill 路径：pill 不残留
- [x] area 录制：无 pill（只有 RecordAnnotation）

### 后续修复（2026-07-26，commit `8ab15558`）

**副屏定位 bug**：用户实测副屏录制时 pill 出现在主屏右下角。根因 `compute_position` 双重错误：
1. `let _ = display_id;` 丢弃 CGDirectDisplayID
2. `Monitor::position()` 物理像素未除 scale（AGENTS.md gotcha）

修复：用 `core_graphics::display::CGDisplay::new(display_id).bounds()` 拿逻辑 CGRect（CoreGraphics 原生 points，已含 scale）。新增 `pill_bottom_right` / `cg_display_logical_bounds` 函数 + 4 个单元测试（主屏 / 左侧副屏 origin_x<0 / 上方副屏 origin_y<0 / display_id=0 无效）。

详见 [`specs/2026-07-26-record-control-window.md`](../specs/2026-07-26-record-control-window.md)「位置算法」段。
