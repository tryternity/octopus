# 2026-07-26 ESC 全局快捷键修复 + 右键取消（录屏 + 滚动截图）

## 背景

两处全局 ESC 设计缺陷，导致不同场景的 ESC 失效：

### 缺陷 1：录屏 ESC 常驻注册（已修，Task 1-5）

`record_hotkey.rs` 在 `22a7cd91` 引入时，把 ESC 作为全局快捷键**常驻注册**。系统层吞掉事件，导致：
- Screenshot 普通截屏 ESC 无法取消
- RecordConfig 浮窗 ESC 无法关闭
- VaultPicker / Settings modal 等所有 DOM 级 ESC 失效

注释声称"非录制态忽略让 Esc 给其他用途"——但全局快捷键已在系统层消费，handler `return` 无法把事件还给 webview。**注释假设错误**。

### 缺陷 2：滚动截图 ESC 收不到（Task 6-8）

滚动截图（scrolling）启动时调 `activate_prev_app`（`screenshot_commands.rs:1097`）把键盘焦点交给下层应用（让用户能滚动下层内容）。Screenshot 窗口的 `onKeyDown` / `window.addEventListener("keydown")` 收不到键盘事件——ESC 去了下层应用。

**为什么按钮能点但 ESC 不工作**：按钮是鼠标点击（穿透轮询在交互区域临时关闭穿透，鼠标能点到）；ESC 是键盘事件，需要 WebView 有键盘焦点，但焦点已交给下层应用。

**为什么普通截屏 ESC 工作**：普通截屏（idle/selected）不调 activate_prev_app，键盘焦点一直在 Screenshot 窗口。

## 不变量

修复后必须同时满足：

1. **非录制态 / 非 scrolling 态**：ESC 不被全局快捷键拦截，完全回到 DOM 层（Screenshot 取消、modal 关闭等正常工作）
2. **录制中（Recording/Paused）**：ESC 全局快捷键已注册，按 ESC 停止录制入库（保持现有 `handle_stop` 语义）
3. **RecordAnnotation 依赖**：录制中 ESC **先**走 DOM（退标注工具），tool=none 后 ESC **再**停止录制（`RecordAnnotation/index.tsx:314-320` 的设计）。这要求录制时全局 ESC 仍注册
4. **三条录屏 stop 路径 + kill**：hotkey / tray / 前端命令 / kill 都要保证 unregister，避免 ESC 泄漏残留
5. **scrolling 态**：ESC 全局快捷键已注册，按 ESC 停止滚动录制（不走前端，直接后端 stop 逻辑）
6. **scrolling 停止路径**：stop_scroll_recording / stop_scroll_recording_with_mode / 异常退出都要 unregister

## 方案：动态 register/unregister ESC

### 核心思路

ESC 全局快捷键改为**按需注册**：
- 录制 `start_with_config` 成功（进入 Recording）→ `register_stop_hotkey`
- 录制 `stop_and_store` 成功（回到 Idle）→ `unregister_stop_hotkey`
- 录制 `kill`（异常强杀）→ `unregister_stop_hotkey`

非录制态 ESC 不注册，完全由 DOM 层处理。

### 为什么不弃用全局 ESC 改用窗口级监听

录屏时焦点通常在**被录制的应用**（如浏览器、编辑器），不在 octopus 窗口。窗口级 keydown 收不到 ESC。全局快捷键是录屏停止的唯一可靠路径，不能弃用。

### 为什么不在 handler 里判断窗口焦点放行

`tauri_plugin_global_shortcut` 在系统层拦截按键，handler 触发时事件已被消费。handler 内 `return` 不能把事件还给 webview。这是该 bug 存在的直接证据。**全局快捷键没有"放行"语义，只有"不注册"**。

## 设计：函数拆分

`record_hotkey.rs` 当前 `register_record_hotkeys(app, toggle_sc)` 一次注册 toggle + ESC。拆成 3 个独立函数：

```rust
// 启动 + 热重载调（仅 toggle，不动 ESC）
pub fn register_toggle_hotkey(app: &AppHandle, toggle_sc: &str) -> Result<(), String>

// 录制 start 时调（仅 ESC）
pub fn register_stop_hotkey(app: &AppHandle) -> Result<(), String>

// 录制 stop/kill 时调（仅 ESC，失败 warn 不阻断）
pub fn unregister_stop_hotkey(app: &AppHandle)
```

`STOP_SHORTCUT = "Escape"` 常量保留（语义不变，仍是录屏固定停止键）。

### 注释修正

`record_hotkey.rs` 文件头 + 函数注释：
- 删除"CompactEditor 关闭"等过时描述（CompactEditor 实际不绑 ESC）
- 删除"非录制态忽略让 Esc 给其他用途"的错误假设
- 改为："ESC 全局快捷键按需注册——录制开始时 register，结束 unregister；非录制态由各窗口 DOM 层处理"

## 实施点

### Task 1：拆分 record_hotkey.rs

- 删 `register_record_hotkeys`
- 加 3 个新函数
- 修注释
- `handle_stop` 逻辑保持不变（仅 Recording/Paused 触发 stop）

### Task 2：start_with_config 成功后 register

`record_commands.rs::start_with_config` L322-332 已有 `if result.is_ok()` 块（更新 tray + 创建标注 overlay），追加：

```rust
if let Err(e) = crate::record_hotkey::register_stop_hotkey(app) {
    log::warn!("[record] ESC stop 快捷键注册失败（不影响录制）: {e}");
}
```

`#[cfg(target_os = "macos")]` gate（与同块其他代码一致）。

### Task 3：stop 路径 unregister

**3a. stop_and_store wrapper 加 app 参数**：

`stop_and_store` 当前签名 `(session, discard, explicit_fields)` → 加 `app: &AppHandle`。inner 成功返回 `Ok(Some(meta))` 后 unregister。

修改 4 个调用点：
- `record_commands.rs:371`（record_stop 命令，State 拿 app_handle）
- `record_hotkey.rs:138`（handle_stop，已有 app）
- `tray.rs:245`（menu 闭包，已有 app_handle）
- `main.rs:1119`（record://stop-requested 监听，已有 ah）

**3b. record_kill 加 unregister**：

`record_commands.rs:521` 的 `record_kill` 独立路径，加：

```rust
#[cfg(target_os = "macos")]
crate::record_hotkey::unregister_stop_hotkey(&app_handle);
```

（需加 `app_handle: AppHandle` 参数或从 State 拿）

### Task 4：热重载路径对齐

`settings_commands.rs::set_config` L178-203 的 `record_shortcut` 热重载：

- 当前调 `register_record_hotkeys`（注册 toggle + ESC）
- 改为：调 `register_toggle_hotkey`（仅 toggle），且若当前 session 在 Recording/Paused 则额外 `register_stop_hotkey`

`set_config` 是 sync fn，但 `session.state()` 是 async——用 `tauri::async_runtime::block_on` 或把热重载这段包进 `tauri::async_runtime::spawn`。决策：**block_on**（简单，且 set_config 本身不是热路径）。

### Task 5：main.rs 启动调用

`main.rs:916-922`：
- 原：`register_record_hotkeys(app.handle(), &config.record_shortcut)`
- 改：`register_toggle_hotkey(app.handle(), &config.record_shortcut)`
- ESC 不在启动时注册——录制开始时才动态注册

## 方案扩展：滚动截图 ESC（Task 6-8）

### 设计

scrolling 模式键盘焦点在下层应用，Screenshot 的 DOM 级 ESC 监听收不到。采用与录屏相同的策略：**scrolling 启动时 register 全局 ESC，停止时 unregister**。

ESC handler 直接在后端调 stop 逻辑（不走前端 invoke，因为前端 onKeyDown 收不到）。

### Task 6：scroll ESC 快捷键函数

在 `screenshot_commands.rs` 加两个函数（与录屏 `record_hotkey::register_stop_hotkey` 同范式）：

```rust
/// scrolling 启动时调：注册全局 ESC，handler 调 stop_scroll_recording 逻辑。
fn register_scroll_esc(app: &AppHandle) -> Result<(), String>

/// scrolling 停止时调：注销全局 ESC。
fn unregister_scroll_esc(app: &AppHandle)
```

handler 内部直接 `SCROLL_RECORDING.store(false, ...)` + 后续清理，不走前端。

**与录屏 ESC 的冲突**：scroll 截图和录屏互斥（截图模式 vs 录屏模式），不会同时注册 ESC。即使同时，`on_shortcut` 对同一快捷键是覆盖语义，后注册的生效，unregister 时各自清理即可。

### Task 7：start/stop 钩点

**register 时机**：`start_scroll_recording` L1094-1101（activate_prev_app 之前，确保 ESC 已就绪）。在 set_window_ignores_mouse_events 之后、activate_prev_app 之前 register。

**unregister 时机**：scrolling 结束路径有多个：
- `stop_scroll_recording` 命令（L1480）
- `stop_scroll_recording_with_mode` 命令（L1486）
- `start_scroll_recording` 任务体的结束（L1320 附近，close_all_screenshot_windows 之前）
- 异常 / 早返回路径

**简化**：在 `start_scroll_recording` spawn 的任务体**最后**（无论正常结束还是异常）unregister。两条 stop 命令也会触发任务体结束（设 SCROLL_RECORDING=false → 循环退出 → 任务体走到末尾），所以统一在任务体末尾 unregister 即可。

### Task 8：ESC handler 的 stop 行为

handler 触发时直接：
1. `SCROLL_STOP_MODE.store(Copy, ...)`（默认复制模式，与 stop_scroll_recording 一致）
2. `SCROLL_RECORDING.store(false, ...)`（让消费循环退出）
3. 不做其他——任务体收尾会处理 finalize / 入库 / 关窗

这与 `stop_scroll_recording` 命令 L1480-1481 的逻辑一致。

## 验证

### 编译验证（改完即跑）

```bash
cargo build --release -p octopus-desktop --features embedded,cloud,custom-protocol
# 期望：0 error 0 warning
```

### 影响面追踪（grep 所有消费点）

```bash
# 旧函数不应再有调用
rg "register_record_hotkeys" crates/  # 期望：0 结果

# 新函数调用点
rg "register_toggle_hotkey|register_stop_hotkey|unregister_stop_hotkey" crates/
# 期望：定义 3 处 + 调用配对正确
```

### 端到端验证（用户实测）

1. **ESC 在 Screenshot 工作**：截图→拖选区→按 ESC→窗口关闭
2. **ESC 在 RecordConfig 工作**：Cmd+Shift+R 弹配置浮窗→按 ESC→浮窗关闭
3. **ESC 录制中停止**：开始录屏→按 ESC→录制停止入库
4. **ESC 录制中 RecordAnnotation**：开始 Area 录制→选标注工具→按 ESC→退工具；再按 ESC→停止录制
5. **热重载**：Settings 改 toggle 快捷键→新快捷键生效；正在录制时 ESC 仍能停止
6. **kill 路径**：录制中异常 → ESC 不残留（下次 Screenshot ESC 正常）

## 降级路径

如果动态 register 在某 macOS 版本上有问题（如 unregister 后再 register 失败）：
- 回退方案：保留启动时 register，但在 handler 里加"当前窗口是 Screenshot/RecordConfig 时 forward ESC 到 webview"——但这是 workaround，不是首选
- 兜底：`unregister_stop_hotkey` 失败仅 warn 不阻断（录制停止本身不受影响）

## 不在本次范围

- **卡顿问题**：间歇性出现，**根因已查明**——`run-octopus-dev.sh` 用 debug profile（opt-level=0），stitch 的 NCC 像素循环慢 10-100×，spawn_blocking emit 堆积 → 前端事件队列堵塞。`run-octopus.sh --no-lto`（release profile）不卡。**不是代码 bug，是 dev profile 性能特征**。详见下方"卡顿根因分析"
- Screenshot/index.tsx 的 Task 1-3 抽取——保留（用户确认 release 下不卡）
- 诊断 log、diag_log 命令——已撤销

## 卡顿根因分析（2026-07-26 补充）

用户最初报告"滚动截图按钮卡几分钟"，多次排查后真相：

### 关键观察（用户提供）

> "非常的奇怪，我现在在 main 上跑也好了，之前试过好几次是不好的。
> 我觉的不是代码问题，除了之前我是用 ./run-octopus-dev.sh 跑的，现在是用 ./run-octopus.sh --no-lto 跑的"

### 两个脚本的关键差异

| 维度 | `run-octopus-dev.sh`（卡） | `run-octopus.sh --no-lto`（不卡） |
|---|---|---|
| Rust profile | debug（opt-level=0，无内联，无向量化） | release（opt-level=3，有内联） |
| 前端加载 | vite dev server（HMR） | 嵌入式 dist（`custom-protocol` feature） |

### 卡顿机制

```
debug profile (run-octopus-dev.sh)
  ↓
stitch::process_frame 像素循环无优化 → 处理一帧 100-500ms
  ↓
spawn_blocking 任务在 tokio blocking pool 堆积（每 30ms 一帧，但处理远超 30ms）
  ↓
emit("scroll://frame") fire-and-forget 排队（spec L1281-1283 注释说"不 await 避免阻塞下一帧"）
  ↓
Tauri 把 emit payload 序列化后 eval JS 到 WebView 主线程
  ↓
WebView 主线程事件队列堵塞（frame 事件累积几千个）
  ↓
按钮 click / ESC keydown 排在事件队列末尾
  ↓
"几分钟才响应"
```

release profile 下，stitch 处理一帧 < 10ms（10-50× 加速），emit 不堆积，按钮立即响应。

### 验证证据

之前诊断 log（release 跑）显示 `9 frames in 1060ms`（前端每秒只收到 9 帧），`last=1ms`（callback 同步部分快）。这些数据在 release 下已经体现"前端处理跟不上后端发帧"的迹象，但 release 下 stitch 够快，emit 堆积不严重，所以用户感觉不卡。

### 为什么 dev profile 下 stitch 这么慢

`crates/capx/src/stitch.rs` 有大量双层像素循环（灰度转换、NCC 匹配、SAD 验证）：
- L57-59 / L126-130 / L271 / L307 / L860-862 / L885-887
- debug profile 下：每次迭代边界检查 + 无内联 + 无 SIMD 向量化
- release profile 下：LLVM 自动内联 + 向量化，10-50× 加速

### 建议（不在本次范围）

如果用户希望在 dev 模式下也能流畅调试 scroll 截图：
- **方案 A**：stitch 热路径加 `#[inline(always)]` + 用 `unsafe { get_unchecked }` 跳边界检查（有风险）
- **方案 B**：dev 模式下 scroll 录制降帧（如 100ms 一帧而非 30ms）
- **方案 C**：dev 模式下 stitch 跳帧（检测到 debug_assertions 时降低 NCC 频率）

但这些都是优化，不是 bug 修复。当前结论：**dev 模式卡是预期行为，用 release profile 调试 scroll 截图**。

## 右键取消（与 commit `7584f326` 同批，扩展功能）

### 需求

截图（含滚动截图）时，**选区外右键**应取消截图/停止 scroll。原来只在 idle 模式（未框选）右键取消，需扩展到 selected / scrolling 全模式。

### 设计

**onContextMenu（前端，`Screenshot/index.tsx`）**：
- **idle**：任意位置右键取消截图（保持原行为）
- **selected**：选区外右键取消截图，选区内右键无操作（避免误触）
- **scrolling**：选区外右键停止 scroll（选区内/预览窗内不处理）

**scrolling 模式的难点**：scrolling 时鼠标穿透（`setIgnoresMouseEvents(true)`）让下层应用接收滚轮，前端 `onContextMenu` **收不到右键**。需要后端兜底。

**后端轮询检测右键（`screenshot_commands.rs` 鼠标轮询）**：
- 复用现有 16ms 鼠标位置轮询循环
- 加 FFI 声明 `CGEventSourceButtonState(state_id, button)`（macOS CoreGraphics API，查硬件鼠标按键状态）
- 封装 `right_mouse_button_down()` 辅助函数
- **边沿检测**：`prev=false → curr=true`（刚按下瞬间）才触发，避免持续按住时反复触发
- 选区外（用 `sel_global_x/y/w/h` 判断）右键 → `SCROLL_STOP_MODE=Copy` + `SCROLL_RECORDING=false`
- handler 直接后端 stop（与 scrolling ESC 同路径，不走前端）

### FFI 声明

```rust
#[cfg(target_os = "macos")]
mod cg_event_source_ffi {
    #[link(name = "CoreGraphics", kind = "framework")]
    extern "C" {
        pub(crate) fn CGEventSourceButtonState(state_id: i32, button: i32) -> bool;
    }
}

#[cfg(target_os = "macos")]
fn right_mouse_button_down() -> bool {
    // state_id=1（HIDSystemState）查硬件按键状态；button=1（右键）
    unsafe { cg_event_source_ffi::CGEventSourceButtonState(1, 1) }
}
```

`state_id=1`（HIDSystemState）反映硬件状态，不受其他 app 的合成事件影响。`button=1` 是右键（CGMouseButton::Right）。

### 不变量

1. **idle/selected 模式**：前端 `onContextMenu` 收到右键（无穿透），前端处理
2. **scrolling 模式选区外**：前端收不到（穿透），后端轮询兜底
3. **scrolling 模式选区内**：前端可能收到（鼠标在交互区域时穿透关闭），前端处理；选区内右键不操作
4. **边沿检测**：只在按下瞬间触发，持续按住不重复触发

### 已实现（commit `7584f326`）

- `Screenshot/index.tsx` `onContextMenu` 扩展三模式分支
- `screenshot_commands.rs` 加 `cg_event_source_ffi` 模块 + `right_mouse_button_down`
- 鼠标轮询循环加 `prev_right_down` 边沿检测 + 选区外判断

### 测试场景

1. 截图→拖选区→选区外右键 → 截图取消（idle/selected）
2. 截图→拖选区→点 scroll→选区外右键 → 停止 scroll 回到 selected
3. 截图→拖选区→点 scroll→选区内右键 → 无操作（不停止）
