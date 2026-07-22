# Quick Execute 全局快捷键实施计划（实施记录）

> spec: `docs/superpowers/specs/2026-07-16-run-and-paste-design.md`
>
> 状态：全部完成 ✅（含方案从 Run And Paste → Quick Execute 的重构，以及 2026-07-17 快捷键残留 + fallback 误触两个 bug 修复）

## 实施记录

### Task 1: DB v36 + global_shortcut ✅
- v35（search_frequency）已被 main 占用，用 v36
- `ALTER TABLE action_bar_items ADD COLUMN global_shortcut TEXT NOT NULL DEFAULT ''`
- `set_global_shortcut` / `list_action_hotkeys` 函数 + 测试

### Task 2: activate_window_by_pid ✅
- `NSWorkspace.runningApplications` 遍历找 PID + `activateWithOptions`
- 当前 Quick Execute 不使用（不粘贴），保留供未来 silent 模式

### Task 3: Overlay 窗口 ✅
- 透明 always-on-top 窗口 + loading/toast 模式
- 当前 Quick Execute 不使用（结果走 CompactEditor），保留基础设施

### Task 4: execute_action_bar_inner is_silent 参数 ✅
- 加 `is_silent: bool` 参数，silent=true 跳过 ActionBar 窗口管理
- `action_bar_show_result_internal` 加 `is_silent` + `_write_clipboard` 参数
- 当前两个调用方都传 `false`（Quick Execute 走正常 CompactEditor 路径）

### Task 5: 全局快捷键注册 + Quick Execute 链路 ✅
- `action_hotkey.rs`：`register_action_hotkeys`（DB 驱动注册/注销）
- `quick_execute`：detect（baseline 隔离）→ execute_action_bar_inner → CompactEditor
- ~~无选中 fallback 到 ActionBar 浮窗~~ → **2026-07-17 删除**：改为静默失败（详见下方 Bug 修复）
- `set_global_shortcut` / `cancel_silent_action` Tauri 命令（后者已删）

### Bug 修复：快捷键残留 + fallback 误触（2026-07-17）✅

**现象**：用户给「Google」菜单项配了 `CmdOrCtrl+Shift+G`，在 Finder 选中文件夹按下时本想触发系统"前往文件夹"，却被 octopus 吞掉并弹出 ActionBar；之后即使用户在设置里删除该快捷键，按键仍被 octopus 拦截。

**根因 1 — 删除快捷键不注销**：
- `register_action_hotkeys` 旧实现遍历 `list_action_hotkeys()` 结果（已 `WHERE global_shortcut != ''`）逐个 unregister，**删空场景下旧值不在结果集里 → 永远残留**
- ~~修复：开头改用 `app.global_shortcut().unregister_all()` 全量清空再重注册~~（**回归**，见下方根因 1b）
- 文件：`crates/desktop/src/action_hotkey.rs::register_action_hotkeys`

**根因 1b — `unregister_all()` 误清其他模块（2026-07-17 二次修复）✅**：
- 上条根因 1 改用 `unregister_all()` 有 Critical 回归：它清的是整个 global_shortcut plugin 持有的**所有**快捷键不分注册者。启动顺序：asr/clipboard/edit_global/polish_global/screenshot 先注册 → `register_action_hotkeys` 调 `unregister_all()` 清光 → 只有后注册的 action_bar_shortcut 幸存，前 5 个全失效
- 原注释错误声称「对其他模块无副作用」——作废
- 修复：维护进程内 `REGISTERED_SHORTCUTS: HashSet<String>` 清单，「重建时遍历清单逐个 unregister + 清单重置 + 重注册时回填」。既覆盖根因 1 的残留场景（DB 已删的只要曾在清单就能精确注销），又不误伤其他模块
- 文件：`crates/desktop/src/action_hotkey.rs`（commit `6a2e7c05`）
- 验证：cargo build 0 error 0 warning；cargo test 311 passed

**根因 2 — fallback 误触 ActionBar**：
- `quick_execute` 旧实现「无选中 → fallback `trigger_action_bar`」，但菜单项热键语义是「对这段文本执行动作」，没文本就不该继续
- Finder 选中文件夹属于 `Selection::Folder`（非 `Text`）→ fallback 弹浮窗 → 劫持了 Finder `Cmd+Shift+G`「前往文件夹」+ 误导交互
- 修复：删除 fallback 分支，改为 `log::info!` + `return`（静默失败）
- 文件：`crates/desktop/src/action_hotkey.rs::quick_execute`
- 附带新增 `selection_kind_name` helper（`Selection` 未 derive Debug，日志要可读）

**验证**：`cargo build --release -p octopus-desktop` 0 error 0 warning；`cargo test -p octopus-desktop --bin octopus-desktop` 311 passed。

### Task 6: 设置页 global_shortcut UI ✅
- ShortcutButton 组件（从 GeneralPanel 抽出共享）
- 录制模式（check_shortcut 冲突检测 + Esc 退出 + Backspace 清除）
- 所有非 submenu 类型显示

### 方案重构：Run And Paste → Quick Execute ✅
- 去掉粘贴替换（不友好——浏览器/PDF 不支持替换）
- 改为 CompactEditor 展示（与 ActionBar 路径一致）
- 删 action_bar_run_and_paste / auto_paste 分支 / paste silent 路径
- auto_paste 全面清理（struct/SQL/命令/前端/i18n）

### auto_paste 清理 ✅
- DB: struct/SELECT_COLS/row mapper 删字段
- 后端: set_auto_paste 命令删除、action_bar_run_and_paste 删除
- 前端: autoPaste 字段和所有 invoke 调用全删
- DB 列保留（存量兼容），代码不再读写

### LLM 超时可配 ✅
- `chat_text_with_prompt` 加 `timeout_secs: Option<u64>`
- 通过 `reqwest::blocking::RequestBuilder::timeout()` per-request 覆盖

## 验证
- 114 infra + 308 desktop + 227 frontend 测试全过
- release build 0 warning 0 error
- 实测：选中文本 → 全局快捷键 → CompactEditor 展示翻译结果
