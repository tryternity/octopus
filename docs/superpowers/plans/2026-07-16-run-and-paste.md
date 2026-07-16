# Run And Paste 全局快捷键 + Silent 执行 实施计划

> spec: `docs/superpowers/specs/2026-07-16-run-and-paste-design.md`

## 文件结构

| 文件 | 职责 | 新增/改动 |
|------|------|----------|
| `crates/infra/src/db.rs` | v34→v35 迁移 + global_shortcut 读写函数 | 改动 |
| `crates/infra/src/db.sql` | action_bar_items 加 global_shortcut 列 | 改动 |
| `crates/desktop/src/action_hotkey.rs` | 全局快捷键注册 + silent 触发链路 | 新增 |
| `crates/desktop/src/overlay_window.rs` | overlay 窗口创建 + show/hide/toast | 新增 |
| `crates/desktop/src/activation.rs` | 加 activate_window_by_pid | 改动 |
| `crates/desktop/src/action_bar_commands.rs` | action_bar_run_and_paste 加 source_pid | 改动 |
| `crates/desktop/src/settings_commands.rs` | set_global_shortcut 命令 | 改动 |
| `crates/desktop/src/main.rs` | 启动注册 + 设置变更重注册 + overlay 窗口创建 | 改动 |
| `crates/desktop/frontend/src/pages/Overlay/index.tsx` | overlay 前端 | 新增 |
| `crates/desktop/frontend/src/pages/Settings/ActionBarPanel.tsx` | global_shortcut 录入 | 改动 |
| `crates/desktop/frontend/src/locales/*.yaml` | i18n | 改动 |

## Task 1: DB v35 + global_shortcut 读写

**文件**：`db.rs`, `db.sql`
**Steps**：

- [ ] 1.1 `db.sql` action_bar_items 建表 SQL 加 `global_shortcut TEXT NOT NULL DEFAULT ''`
- [ ] 1.2 `db.rs` v34→v35 迁移：`ALTER TABLE action_bar_items ADD COLUMN global_shortcut TEXT NOT NULL DEFAULT ''` + `PRAGMA user_version = 35`
- [ ] 1.3 `init_schema` 的 `if v >= 35 { return Ok(()); }` 更新
- [ ] 1.4 `db.rs` ActionBarItem struct 加 `pub global_shortcut: String` 字段
- [ ] 1.5 `row_to_action_bar_item` 映射加 global_shortcut 列
- [ ] 1.6 `ACTION_BAR_SELECT_COLS` 加 global_shortcut
- [ ] 1.7 `insert_action_bar_item` / `update_action_bar_item` 加 global_shortcut 参数
- [ ] 1.8 `set_global_shortcut(id, global_shortcut)` 函数（零行检查）
- [ ] 1.9 `list_action_hotkeys()` 函数：返回 `WHERE global_shortcut != '' AND auto_paste = 1 AND is_enabled = 1`
- [ ] 1.10 测试：v34→v35 迁移、global_shortcut 读写 round-trip、list_action_hotkeys 过滤
- [ ] 1.11 验证：`cargo test -p octopus-infra --lib db`
- [ ] 1.12 同步 `docs/superpowers/specs/2026-07-16-run-and-paste-design.md` 确认 v35 描述
- [ ] 1.13 提交

## Task 2: activate_window_by_pid

**文件**：`activation.rs`
**Steps**：

- [ ] 2.1 `activation.rs` 加 `pub fn activate_window_by_pid(pid: i32) -> bool`（macOS `#[cfg]`）
  - 用 `NSWorkspace::sharedWorkspace().runningApplications()` 遍历找 PID
  - 找到后 `activateWithOptions(NSApplicationActivateAllWindows)`
  - 返回 true/false
- [ ] 2.2 Windows/Linux `#[cfg(not(target_os = "macos"))]` 返回 false + log warn
- [ ] 2.3 验证：`cargo build -p octopus-desktop --features embedded`（编译通过）
- [ ] 2.4 提交

## Task 3: Overlay 窗口

**文件**：`overlay_window.rs`, `main.rs`, `Overlay/index.tsx`
**Steps**：

- [ ] 3.1 `overlay_window.rs` 加 `WINDOW_LABEL = "overlay_window"` + `create_overlay_window(app)`（透明无边框 always-on-top visible=false）
- [ ] 3.2 `show_overlay_window(app, x, y)` — set_position + show + emit `overlay://show {payload}`，不调 set_focus
- [ ] 3.3 `hide_overlay_window(app)` — hide + emit `overlay://hide`
- [ ] 3.4 `show_overlay_loading(app, action_name)` — 鼠标位置 show + loading payload
- [ ] 3.5 `show_overlay_toast(app, message, type, duration)` — 鼠标位置 show + toast payload，spawn 线程 sleep duration 后 hide
- [ ] 3.6 `main.rs` setup 闭包加 `overlay_window::create_overlay_window(app.handle())`
- [ ] 3.7 `capabilities/default.json` windows 数组加 `overlay_window`（Tauri 2 listen/invoke 权限）
- [ ] 3.8 前端 `Overlay/index.tsx`：listen `overlay://show` 渲染 loading/toast，listen `overlay://hide` 隐藏；toast 模式按 duration 自动 hide
- [ ] 3.9 前端样式：loading（spinner + 文字）、toast warn（黄色）、toast error（红色）——参考 ActionBar 视觉规格（圆角 10px / 透明度 90% / backdrop-blur-2xl）
- [ ] 3.10 验证：`npm run build` + `cargo build`
- [ ] 3.11 提交

## Task 4: action_bar_run_and_paste 加 source_pid + 焦点恢复 + LLM 超时

**文件**：`action_bar_commands.rs`, `crates/llm/src/client.rs`
**前置已完成**：`chat_text_with_prompt` 已加 `timeout_secs: Option<u64>` 参数（LLM 超时可配，默认 120s，silent 路径传 30s）。所有现有调用点已传 `None`。
**Steps**：

- [ ] 4.1 `action_bar_run_and_paste` 签名加 `source_pid: Option<i32>` 参数
- [ ] 4.2 去掉预写剪贴板（`write_clipboard_text`），让 `paste::paste` 统一处理
- [ ] 4.3 source_pid 有值时 `activate_window_by_pid` + sleep 150ms
- [ ] 4.4 source_pid 无值时保持原逻辑（浮窗路径，靠 hide 自动还焦）+ sleep 100ms
- [ ] 4.5 `action_bar_show_result_internal` 的 auto_paste 分支传 source_pid（默认 None）
- [ ] 4.6 三处 `action_bar_run_and_paste` 调用点（L1332/1354/1421）加 `None` 参数
- [ ] 4.7 验证：`cargo build -p octopus-desktop --features embedded`
- [ ] 4.8 提交

## Task 5: 全局快捷键注册 + Silent 触发链路

**文件**：`action_hotkey.rs`（新增）, `main.rs`, `settings_commands.rs`
**Steps**：

- [ ] 5.1 新建 `action_hotkey.rs`，加 `register_action_hotkeys(app)` 函数
  - 先注销所有 `action_hotkey_*` label 的快捷键
  - `list_action_hotkeys()` 查 DB
  - 逐个 `app.global_shortcut().register(label, shortcut, callback)`
  - callback → `spawn worker → silent_run_and_paste(item_id, app)`
- [ ] 5.2 `silent_run_and_paste(item_id, app)` 函数（worker 线程逻辑）：
  - 同步读源窗口 PID（NSWorkspace.frontmostApplication）
  - `detect_selection(app)` 读选中
  - 无选中 → `show_overlay_toast("请先选中文本", warn, 2000)` → return
  - 有选中 → 隐藏 ActionBar（如可见）+ `show_overlay_loading(action_name)` （含"按 Esc 取消"提示）
  - 读 DB 取 item（title 用于 overlay）
  - `execute_action_bar_inner` 的 silent 版（不弹 CompactEditor），LLM 调用传 `timeout_secs=Some(30)`（30s 超时）
  - 成功 → `action_bar_run_and_paste(result, app, Some(pid))`
  - 超时 → `show_overlay_toast("执行超时（30s）", error, 3000)`
  - 失败 → `show_overlay_toast(error, error, 3000)`
- [ ] 5.2b **Esc 取消**：overlay loading 期间监听 Esc → abort worker JoinHandle → 隐藏 overlay
  - overlay 前端 keydown handler 捕获 Esc → `invoke("cancel_silent_action")`
  - 后端 `cancel_silent_action` 命令 → `JoinHandle::abort()` + `hide_overlay_window`
  - LLM HTTP 可能仍在后台跑完（reqwest blocking 不可中断），但结果被丢弃
- [ ] 5.3 `execute_action_bar_inner` 的 silent 复用：现有 auto_paste 路径已走 `action_bar_run_and_paste`，只需确保 silent 调用时 `auto_paste=true` 生效。silent 路径调 `execute_action_bar` Tauri 命令（已有），但需要传一个 silent flag 避免弹 CompactEditor
  - 实际上 `execute_action_bar_inner` 里 auto_paste=true 时**已经不弹 CompactEditor**（走 run_and_paste return）。所以 silent 路径只需确保 item.auto_paste=true 即可
- [ ] 5.4 `main.rs` setup 闭包加 `action_hotkey::register_action_hotkeys(app.handle())`
- [ ] 5.5 `settings_commands.rs` 加 `set_global_shortcut(id, global_shortcut)` Tauri 命令
- [ ] 5.6 设置页保存菜单项后触发 `register_action_hotkeys` 重注册（emit 事件或直接调）
- [ ] 5.7 `main.rs` invoke_handler 注册 `set_global_shortcut`
- [ ] 5.8 验证：`cargo build -p octopus-desktop --features embedded`
- [ ] 5.9 提交

## Task 6: 设置页 global_shortcut UI

**文件**：`ActionBarPanel.tsx`, `locales/*.yaml`
**Steps**：

- [ ] 6.1 `ActionBarPanel.tsx` ActionBarItem interface 加 `globalShortcut?: string`
- [ ] 6.2 菜单编辑表单：`autoPaste=true` 时显示"全局快捷键"输入框（复用现有快捷键录入组件）
- [ ] 6.3 保存时调 `invoke("set_global_shortcut", { id, globalShortcut })`
- [ ] 6.4 编辑模式加载已有 global_shortcut 值
- [ ] 6.5 i18n：`settings.actionBar.globalShortcutLabel` / `globalShortcutHint`
- [ ] 6.6 验证：`npm run build` + `npm run test`
- [ ] 6.7 提交

## Task 7: 文档同步 + 全量验证

**Steps**：

- [ ] 7.1 `architecture.md` 补 Run And Paste 全局快捷键 + overlay 描述
- [ ] 7.2 `docs/superpowers/specs/2026-07-15-actionbar-search-design.md` §5 Run And Paste 补全局快捷键入口
- [ ] 7.3 全量：`cargo test --workspace` + `npm run build` + `npm run test`
- [ ] 7.4 提交
