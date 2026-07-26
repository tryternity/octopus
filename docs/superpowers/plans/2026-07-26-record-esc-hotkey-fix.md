# 实施计划：录屏 ESC 全局快捷键修复

> Spec: [`2026-07-26-record-esc-hotkey-fix.md`](../specs/2026-07-26-record-esc-hotkey-fix.md)

## 任务分解

### Task 1-5：录屏 ESC 修复（已完成，见下方"实施记录"）

### Task 6-8：滚动截图 ESC 修复（scrolling 时焦点在下层应用，DOM 收不到 ESC）

**Task 6**：在 `screenshot_commands.rs` 加 `register_scroll_esc` / `unregister_scroll_esc` 函数（与录屏 `record_hotkey::register_stop_hotkey` 同范式）。ESC handler 直接在后端 stop（不走前端 invoke）。

**Task 7**：`start_scroll_recording` 在 `activate_prev_app` 之前 `register_scroll_esc`；`ScrollRecordingGuard` 加 `app: Option<AppHandle>` 字段，drop 时若 Some 则 `unregister_scroll_esc`。guard 用 `let mut` 持有，register 成功后 set Some。

**Task 8**：handler 内 `SCROLL_STOP_MODE = Copy` + `SCROLL_RECORDING = false`（与 `stop_scroll_recording` 命令一致）。任务体收尾自动处理 finalize/入库/关窗。

### Task 1：拆分 record_hotkey.rs

**文件**：`crates/desktop/src/record_hotkey.rs`

**变更点**：
- 删 `pub fn register_record_hotkeys(app, toggle_sc) -> Result<(), String>`（L38-79）
- 加 `pub fn register_toggle_hotkey(app, toggle_sc) -> Result<(), String>`（仅注册 toggle，L40-55 的逻辑）
- 加 `pub fn register_stop_hotkey(app) -> Result<(), String>`（仅注册 ESC，L57-75 的逻辑）
- 加 `pub fn unregister_stop_hotkey(app)`（unregister ESC，失败 warn）
- 修文件头注释（L1-19）+ 函数注释（L15-16, L27, L57-59）：删"CompactEditor 关闭"过时描述，改为"按需注册"
- `handle_stop` / `handle_toggle` 逻辑不变

**验证命令**：
```bash
cargo check -p octopus-desktop --features embedded,cloud,custom-protocol
```
（此时会有调用点编译错误，Task 2-5 修完后 0 error）

### Task 2：start_with_config 成功后 register_stop_hotkey

**文件**：`crates/desktop/src/record_commands.rs`

**变更点**：`start_with_config` L322-332 的 `if result.is_ok()` 块内，在创建标注 overlay 后追加：

```rust
#[cfg(target_os = "macos")]
{
    // ... 现有 create_annotation_window ...
    // 录制开始 → register ESC stop 快捷键
    // （非录制态不注册，避免吞掉 Screenshot/RecordConfig 等 DOM 级 ESC）
    if let Err(e) = crate::record_hotkey::register_stop_hotkey(app) {
        log::warn!("[record] ESC stop 快捷键注册失败（不影响录制）: {e}");
    }
}
```

### Task 3：stop 路径 unregister_stop_hotkey

**文件**：`crates/desktop/src/record_commands.rs` + 4 个调用点

**3a. stop_and_store 加 app 参数**：

```rust
// 原：
pub(crate) async fn stop_and_store(
    session: &RecordSession,
    discard: bool,
    explicit_fields: Option<MetaFields>,
) -> Result<Option<RecordingMeta>, String>

// 改：
pub(crate) async fn stop_and_store(
    session: &RecordSession,
    app: &AppHandle,
    discard: bool,
    explicit_fields: Option<MetaFields>,
) -> Result<Option<RecordingMeta>, String>
```

在 `stop_and_store_inner` 成功返回 `Ok(Some(meta))` 后（或在 stop_and_store wrapper 末尾，inner 成功后），追加：

```rust
#[cfg(target_os = "macos")]
crate::record_hotkey::unregister_stop_hotkey(app);
```

**3b. 修改 4 个调用点**：

| 文件 | 行 | 原调用 | 改 |
|---|---|---|---|
| `record_commands.rs` | 371 | `stop_and_store(&state, discard, Some(fields))` | `stop_and_store(&state, &app_handle, discard, Some(fields))`（注：record_stop 命令需加 `app_handle: AppHandle` 参数） |
| `record_hotkey.rs` | 138 | `stop_and_store(&session, false, None)` | `stop_and_store(&session, app, false, None)`（app 已在 handle_stop 作用域） |
| `tray.rs` | 245 | `stop_and_store(&session, false, None)` | `stop_and_store(&session, &ah, false, None)`（ah 已在闭包） |
| `main.rs` | 1119 | `stop_and_store(&session, false, None)` | `stop_and_store(&session, &ah, false, None)`（ah 已在作用域） |

**3c. record_kill 加 unregister**：

`record_commands.rs:521` 的 `record_kill` 命令加 `app_handle: AppHandle` 参数：

```rust
#[command]
pub async fn record_kill(
    state: State<'_, RecordSession>,
    app_handle: AppHandle,
) -> Result<(), String> {
    let r = state.kill().await.map_err(e2s);
    #[cfg(target_os = "macos")]
    crate::record_hotkey::unregister_stop_hotkey(&app_handle);
    r
}
```

注：无论 kill 成功失败都 unregister（kill 是异常恢复，ESC 不应残留）。

### Task 4：settings_commands 热重载对齐

**文件**：`crates/desktop/src/settings_commands.rs` L178-203

**变更点**：
- `register_record_hotkeys(&app_handle, &cfg.record_shortcut)` → `register_toggle_hotkey(&app_handle, &cfg.record_shortcut)`
- 回滚路径同样改 `register_toggle_hotkey`
- 新增：热重载后检查 session 状态，若在 Recording/Paused 则 `register_stop_hotkey`

`set_config` 是 sync fn，但 `session.state()` 是 async——用 `tauri::async_runtime::block_on`：

```rust
// 注册成功后，若正在录制，重新注册 ESC（register_toggle 不动 ESC）
let session = app_handle.try_state::<octopus_record::RecordSession>();
if let Some(s) = session {
    let in_recording = tauri::async_runtime::block_on(async {
        matches!(
            s.state().await,
            octopus_record::SessionState::Recording | octopus_record::SessionState::Paused
        )
    });
    if in_recording {
        let _ = crate::record_hotkey::register_stop_hotkey(&app_handle);
    }
}
```

注：`block_on` 在 Tauri 命令上下文安全（不在 tokio runtime 内部）。

### Task 5：main.rs 启动调用

**文件**：`crates/desktop/src/main.rs` L916-922

**变更点**：
```rust
// 原：record_hotkey::register_record_hotkeys(app.handle(), &config.record_shortcut)
// 改：record_hotkey::register_toggle_hotkey(app.handle(), &config.record_shortcut)
// 注释更新：ESC 不在启动时注册——录制开始时动态 register（record_commands::start_with_config）
```

## 验证纪律

按 AGENTS.md：

### 1. 编译验证

```bash
cargo build --release -p octopus-desktop --features embedded,cloud,custom-protocol
# 期望：0 error 0 warning
```

### 2. 影响面追踪

```bash
# 旧函数无残留
rg "register_record_hotkeys" crates/  # 期望 0 结果

# 新函数调用点配对
rg "register_toggle_hotkey|register_stop_hotkey|unregister_stop_hotkey" crates/
```

期望调用点：
- `register_toggle_hotkey`: 1 定义 + 2 调用（main.rs 启动 + settings 热重载）+ 1 回滚
- `register_stop_hotkey`: 1 定义 + 2 调用（start_with_config + settings 热重载录制中）
- `unregister_stop_hotkey`: 1 定义 + 3 调用（stop_and_store + record_kill + 热重载不需要，因为没在录制）

### 3. 端到端验证（用户实测）

见 spec 验证章节。

### 4. 测试验证

本次改动是 macOS 全局快捷键生命周期管理，**无 Rust 单测**（涉及 AppHandle/全局快捷键插件，无法在 #[test] 上下文测）。验证靠编译 + 端到端实测。

## 文档同步

- [x] spec：`docs/superpowers/specs/2026-07-26-record-esc-hotkey-fix.md`
- [x] plan：本文档
- [ ] review plan（实现完成后回填偏差）
- [ ] `docs/architecture.md` 录屏章节（若有）

## 实施记录

### 编译验证

```
cargo build --release -p octopus-desktop --features embedded,cloud,custom-protocol
Finished `release` profile [optimized] target(s) in 43.45s
# 0 error 0 warning
```

### 影响面追踪结果

**旧函数残留**：`register_record_hotkeys` 全代码库 0 结果 ✓

**新函数调用点**（grep 验证）：

| 函数 | 定义 | 调用点 |
|---|---|---|
| `register_toggle_hotkey` | record_hotkey.rs:44 | main.rs:918（启动）+ settings_commands.rs:186（注册）+ 191（回滚） |
| `register_stop_hotkey` | record_hotkey.rs:71 | record_commands.rs:333（start_with_config 成功）+ settings_commands.rs:209（热重载时录制中） |
| `unregister_stop_hotkey` | record_hotkey.rs:99 | record_commands.rs:409（stop_and_store wrapper）+ 543（record_kill） |

### 偏差与决策

1. **settings_commands.rs 加 `use tauri::Manager`**：原文件未 import Manager trait，新增的 `try_state` 调用需要它。编译时发现，已修复。

2. **`stop_and_store` wrapper 末尾 unregister**：原 plan 写"在 inner 成功返回 Ok(Some(meta)) 后"。实际改为"无论 inner 成功失败都 unregister"——因为 inner 失败时录制已停止（session.stop 已执行），ESC 也应释放。这与 record_kill 的"无论 kill 成功失败都 unregister"一致。

3. **前端无影响**：
   - `record_stop` 命令加了 `app_handle: AppHandle` 参数，但 Tauri 2 自动注入，前端 invoke 不需改。
   - 前端不直接调 record_stop（走 emit `record://stop-requested`）。
   - `record_kill` 加 app_handle 参数同理；前端不调 record_kill。

### 待用户实测

- [ ] ESC 在 Screenshot 工作（截图→拖选区→ESC→窗口关闭）
- [ ] ESC 在 RecordConfig 工作（Cmd+Shift+R 弹浮窗→ESC→浮窗关闭）
- [ ] ESC 录制中停止（开始录屏→ESC→录制停止入库）
- [ ] ESC 录制中 RecordAnnotation（Area 录制→选工具→ESC→退工具；再 ESC→停止）
- [ ] **ESC 在 scrolling 模式停止滚动**（截图→拖选区→点 scroll→ESC→停止滚动回到 selected）
- [ ] 热重载（Settings 改快捷键→新快捷键生效）
- [ ] kill 路径（录制中异常→ESC 不残留）

## Task 6-8 实施记录（scrolling ESC）

### 改动文件

**`crates/desktop/src/screenshot_commands.rs`**：

1. **新增 `register_scroll_esc` / `unregister_scroll_esc`**（L66-108）：
   - `register_scroll_esc`：注册全局 ESC，handler 直接设 `SCROLL_STOP_MODE=Copy` + `SCROLL_RECORDING=false`（与 `stop_scroll_recording` 命令一致）
   - `unregister_scroll_esc`：注销 ESC，失败 warn 不阻断

2. **`ScrollRecordingGuard` 加 `app: Option<tauri::AppHandle>` 字段**（L748-762）：
   - Drop 时若 Some 则 `unregister_scroll_esc`（覆盖所有退出路径：早返回/正常/panic）
   - None 表示 register 前的早返回，drop 时跳过 unregister

3. **`start_scroll_recording`**：
   - guard 初始化改 `{ app: None }` + `let mut`（L1002）
   - 在 `set_window_ignores_mouse_events` 之后、`activate_prev_app` 之前 `register_scroll_esc`（L1153-1160）
   - register 成功后 `_scroll_guard.app = Some(ah.clone())`

### 设计决策

1. **RAII unregister**：guard 持有 `Option<AppHandle>`，drop 时 unregister。这样所有早返回路径（窗口已关闭/CG 失败/首帧失败）都自动清理 ESC，无需在每个 return 前手动 unregister。

2. **handler 不走前端**：scrolling 时焦点在下层应用，前端 onKeyDown 收不到 ESC。handler 直接在后端 stop（设 SCROLL_RECORDING=false），任务体收尾自动处理 finalize/入库/关窗。

3. **与录屏 ESC 互斥**：scroll 截图和录屏不会同时（截图模式 vs 录屏模式）。即使 `on_shortcut` 对同一快捷键是覆盖语义，各自 unregister 时只清自己的注册，安全。

4. **ScrollStopMode 默认 Copy**：ESC 触发时默认 copy 模式（与 `stop_scroll_recording` 命令 L1539 一致）。用户若要 save/cancel，用预览窗的按钮（走 `stop_scroll_recording_with_mode`）。

### 编译验证

```
cargo build --release -p octopus-desktop --features embedded,cloud,custom-protocol
Finished `release` profile [optimized] target(s) in 45.48s
# 0 error 0 warning
```

### 影响面追踪

```
register_scroll_esc: 定义 L75 + 调用 L1154（start_scroll_recording）
unregister_scroll_esc: 定义 L96 + 调用 L757（ScrollRecordingGuard::drop）
_scroll_guard.app: set Some L1157
```
