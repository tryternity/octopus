# Quick Execute 全局快捷键实施计划（实施记录）

> spec: `docs/superpowers/specs/2026-07-16-run-and-paste-design.md`
>
> 状态：全部完成 ✅（含方案从 Run And Paste → Quick Execute 的重构）

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
- 无选中 fallback 到 ActionBar 浮窗
- `set_global_shortcut` / `cancel_silent_action` Tauri 命令（后者已删）

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
