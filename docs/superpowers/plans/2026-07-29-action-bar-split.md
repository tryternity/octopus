# action_bar_commands.rs 拆分 plan（desktop crate 大文件重构 #2）

> **对应 spec**: `docs/superpowers/specs/2026-07-29-action-bar-split.md`
> **分支**: `daily_refactor_action_bar`
> **原则**: 纯代码搬家 + glob re-export 保持路径不变。按行数从小到大推进。

## 阶段 0：目录化

### Task 0.1 — action_bar_commands.rs → action_bar_commands/mod.rs

**变更**：
- `mkdir -p crates/desktop/src/action_bar_commands && git mv crates/desktop/src/action_bar_commands.rs crates/desktop/src/action_bar_commands/mod.rs`
- main.rs 的 `mod action_bar_commands;` 无需改

**验证**：
```bash
cargo build -p octopus-desktop --features embedded && cargo test -p octopus-desktop
```
**预期**：编译通过，441 测试全绿。

---

## 阶段 1：子模块提取（按行数从小到大）

### Task 1.1 — agent.rs（~140 行）

**搬出函数**：
- `list_agent_adapters` / `create_agent_adapter` / `update_agent_adapter` / `set_default_agent` / `clear_default_agent` / `delete_agent_adapter` / `refresh_agent_detection`（L1944-2002）
- `trigger_agent_voice_core` / `trigger_agent_voice`（L2003-2057）
- `list_agent_tasks` / `delete_agent_task` / `retry_agent_task`（L2058-2082）

**验证**：build + test

### Task 1.2 — prompt_files.rs（~180 行）

**搬出函数**：
- `render_agent_prompt` / `resolve_prompt_reference`（L1229-1264）
- `PromptFileInfo` struct + `list_prompt_files` / `open_file_in_editor` / `create_prompt_file` / `save_file` / `read_file_text`（L1265-1370）
- `format_paths` / `derive_cwd`（L1371-1387）

**验证**：build + test

### Task 1.3 — window.rs（~200 行）

**搬出函数**：
- `trigger_action_bar`（L283-350）
- `show_action_bar_at_mouse_with_pos` / `primary_monitor_logical_rect` / `primary_monitor_center` / `show_action_bar_centered`（L351-434）
- `finalize_action_bar` / `finalize_action_bar_pub` / `set_pending_context` / `reset_trigger_guard` / `reset_trigger_guard_if_stale`（L435-469）
- `action_bar_get_context` / `snapshot_pending_context` / `action_bar_dismiss`（L470-499）

**验证**：build + test

### Task 1.4 — items.rs（~250 行）

**搬出函数**：
- `list_action_bar_items` / `derive_need_voice`（L771-783）
- `create_action_bar_item` / `update_action_bar_item` / `delete_action_bar_item` / `move_action_bar_item` / `set_global_shortcut`（L784-854）
- `list_script_runs` / `clear_script_runs` / `delete_script_runs`（L855-873）
- `restore_prompt_from_seed`（L874-887）

**验证**：build + test

### Task 1.5 — translate.rs（~320 行）

**搬出函数 + 类型**：
- `auto_translate_prompt` / `TranslateStrategy` enum / `resolve_translate_strategy` / `detect_translate_direction`（L888-948）
- `do_translate`（L949-1010）
- `TranslateEmitTarget` enum + impl / `TranslateSessionPayload` / `CachedTranslateResult` / `cache_translate_done` / `get_translate_result` / `forget_translate_result`（L1011-1139）
- `do_translate_streaming` / `translate_text` / `url_encode_param` / `url_encode_path`（L1140-1228）

**验证**：build + test

### Task 1.6 — script.rs（~330 行）

**搬出函数 + 类型**：
- `ScriptResult` struct / `script_error_msg`（L1388-1410）
- `detect_js_runtime` / `detect_ts_runtime`（L1411-1459）
- `spawn_script` / `wait_with_timeout` / `wait_forever` / `wait_with_timeout_secs` / `now_epoch_secs`（L1460-1664）
- `run_script_async` / `run_script_sync_blocking`（L1665-1709）
- `execute_action_bar_inner` / `execute_action_bar`（L1710-1943）

**验证**：build + test

### Task 1.7 — context.rs（~770 行，最大，放最后）

**搬出函数 + 类型**：
- `ContextKind` enum / `ActionBarContext` struct + impl（L1-60）
- `save_change_count_baseline` / `restore_change_count_baseline`（L61-76）
- `Selection` enum + impl / `common_parent_dir` / `detect_selection`（L77-282）
- `action_bar_show_result` / `action_bar_show_result_internal`（L500-562）
- `read_clipboard_text` / `write_clipboard_text` / `pasteboard_change_count`（L563-592）
- `log_app_context` / `context_log_path` / `format_context_entry` / `write_context_log` / `truncate_for_log` / `build_enriched_text`（L593-737）
- `get_mouse_position`（L738-770，`#[cfg(target_os)]`）

**验证**：build + test

---

## 阶段 2：测试整理

### Task 2.1 — 测试搬到对应子模块

读 mod.rs 剩余的 359 行测试，按被测函数分发到对应子模块的 `#[cfg(test)] mod tests`。

**验证**：
```bash
cargo test -p octopus-desktop 2>&1 | tail -3
# 预期：441 passed, 0 failed, 1 ignored
```

---

## 阶段 3：收尾

### Task 3.1 — 文档同步

- 更新 `docs/architecture.md`：action_bar_commands.rs → 目录描述
- 更新 spec status → ✅ 已实现
- review plan：回写实际偏差

### Task 3.2 — 全量验证

```bash
cargo build -p octopus-desktop --features embedded
cargo build -p octopus-desktop --features embedded,cloud,vault
cargo build -p octopus-desktop --features remote-ws
cargo build -p octopus-desktop --features remote-grpc
cargo test -p octopus-desktop
```

---

## 验证 checklist（每步必跑）

- [x] `cargo build -p octopus-desktop --features embedded` — 0 error 0 warning
- [x] `cargo build -p octopus-desktop --features embedded,cloud,vault` — 0 error 0 warning
- [x] `cargo build -p octopus-desktop --features remote-ws` — 0 error 0 warning
- [x] `cargo build -p octopus-desktop --features remote-grpc` — 0 error 0 warning
- [x] `cargo test -p octopus-desktop` — 441 passed, 0 failed, 1 ignored
- [x] git diff 确认：只搬函数 + re-export，无逻辑改动

## 关键：mod.rs re-export 模板

每个 Task 搬出函数后，mod.rs 加：
```rust
mod <submodule>;
pub use <submodule>::*;
```

最终 mod.rs 只有 mod 声明 + 7 个 `pub use`，~30 行。

## 回滚策略

每个 Task 独立 commit。失败 `git reset --hard HEAD~1` 回退。
