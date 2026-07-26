# 实施计划：录制控制浮窗（P1-7）

> Spec: [`2026-07-26-record-control-window.md`](../specs/2026-07-26-record-control-window.md)

## 任务分解

### Task 1：后端 record_control_window.rs

**新增** `crates/desktop/src/record_control_window.rs`（~110 行）：
- `pub const WINDOW_LABEL: &str = "record_control_window";`
- `pub fn create_control_window(app, source)` —— Area 过滤跳过；非 Area 创建 pill（destroy 重建保证单例）
- `pub fn close_control_window(app)` —— destroy 窗口
- `compute_position(app, source)` —— 主屏右下角 - 16px 内边距（display_id 精确匹配推迟，MVP fallback 主屏）
- 窗口属性：always_on_top / transparent / decorations:false / skip_taskbar / resizable:false / shadow:false / visible:true
- 固定尺寸 WIDTH=200 / HEIGHT=56

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

- [ ] display 录制 → 右下角 pill（红点+时长）→ 点停止 → 消失
- [ ] window 录制 → 主屏右下角 pill（fallback）
- [ ] 暂停/恢复：红点变灰 + 时长停 / 恢复
- [ ] ESC 停止：pill 消失
- [ ] tray 停止：pill 消失
- [ ] kill 路径：pill 不残留
- [ ] area 录制：无 pill（只有 RecordAnnotation）
