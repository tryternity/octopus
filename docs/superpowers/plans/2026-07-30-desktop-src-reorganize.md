# crates/desktop/src 目录重组 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [x]`) syntax for tracking.

**Goal:** 把 `crates/desktop/src/` 下 70 个平铺 .rs 文件全部归入 11 个功能域 mod 目录，消除散落文件。

**Architecture:** 按功能域建顶级 mod 目录（core/engine/record/vault/clipboard/action_bar/platform/commands/ui），现有 7 个子目录整体搬入，每个域 mod.rs 用 glob re-export。关键工作量是路径迁移——约 1000 处 `crate::<file>::` 引用改成 `crate::<domain>::<file>::`（或经 re-export 简化为 `crate::<domain>::<symbol>`）。

**Tech Stack:** Rust 2021 / Tauri 2 / cargo

## Global Constraints

- **纯代码搬家**：不改函数逻辑/签名/注释
- **0 warning 硬性要求**：unused import 必须清理
- **路径迁移策略**：`crate::<file>::xxx` → `crate::<domain>::<file>::xxx`。每个域 mod.rs 加 `pub mod <file>;`（不加 glob re-export 到域级别——因为会符号冲突）。引用方显式写 `crate::<domain>::<file>::xxx`。
- **feature gate 跟随**：vault 域 `#[cfg(feature = "vault")]`、engine 的 cloud 部分 `#[cfg(feature = "cloud")]` 在 mod.rs 的子 mod 声明上保留
- **每域独立 commit + build + test 验证**
- **工作目录**：`/Users/wudarui/workspace/agent/octopus/.worktrees/daily_bugfix_0729`

## 域映射表（完整 70 文件 → 11 域）

| 域 | 文件（平铺 .rs） | 已有子目录 | 引用数 |
|---|---|---|---|
| `core/` | setup bootstrap config runtime_config db_queue invoke_handler error_util perf_log file_watcher extensions shortcut | — | 24+38+42+24+5+2+6 = ~141 |
| `engine/` | engine engine_embedded engine_ws engine_grpc engine_dispatch engine_aliyun cloud_pipeline pipeline transcript audio | coordinator/ | 14+2+1+1+1+2+1+14+13+7 = ~56 |
| `record/` | record_area_picker record_audio_probe record_hotkey record_window record_annotation_window record_control_window screenshot_geometry subtitle_polish | record_commands/ screenshot_commands/ | ~47 |
| `vault/` | vault_error vault_state vault_secret_access vault_sync_commands password_generator_window | vault_commands/ autotype/ | ~59 |
| `clipboard/` | clipboard_commands clipboard_window clipboard_queue clipboard_dock | — | ~41 |
| `action_bar/` | action_bar_window action_hotkey agent_adapter terminal_launcher | action_bar_commands/ | ~23 |
| `platform/` | input_source finder_selection keystroke paste sys_open activation focus_tracker | app_context/ | ~43 |
| `commands/` | model_commands settings_commands system_status_commands hotword_commands search_commands compact_editor_commands compact_editor_window translation_commands builtin_models model_migrate | — | ~77 |
| `ui/` | result_window pin_window settings_window onboarding_window overlay_window download_window tray i18n theme window_factory window_position | — | ~133 |

**实施顺序**：从引用数最少的域开始（action_bar ~23 → platform ~43 → clipboard ~41 → engine ~56 → vault ~59 → commands ~77 → ui ~133 → record ~47 → core ~141），减少早期域搬运对后续域的干扰。但实际上路径迁移是全局 grep 替换，顺序影响不大——按**逻辑独立性**排序更合理。

**推荐顺序**：先搬「自包含」的域（被引用少且不依赖其他域搬运），再搬「被广泛引用」的域（core/ui/engine 最后，因为它们的迁移影响面最大）。

---

## Task 1: 建 9 个域目录骨架 + main.rs mod 声明

**Files:**
- Create: `crates/desktop/src/{core,engine,record,vault,clipboard,action_bar,platform,commands,ui}/.gitkeep`（临时占位，后续 Task 填充）
- Modify: `crates/desktop/src/main.rs`（mod 声明区）

**Interfaces:**
- Consumes: 现有 main.rs 的 mod 声明（~97 行）
- Produces: 9 个空目录 + main.rs 暂时保持原 mod 声明不变（后续 Task 逐域迁移时改）

**说明**：此 Task 只建空目录，不改任何代码。确保 `cargo build` 仍通过（空目录不影响编译，因为还没在 main.rs 声明）。

- [x] **Step 1: 创建 9 个域目录**

```bash
cd /Users/wudarui/workspace/agent/octopus/.worktrees/daily_bugfix_0729
for d in core engine record vault clipboard action_bar platform commands ui; do
  mkdir -p "crates/desktop/src/$d"
done
```

- [x] **Step 2: 验证编译不受影响**

Run: `cargo build -p octopus-desktop --features embedded`
Expected: Finished（空目录不影响编译）

- [x] **Step 3: Commit**

```bash
git add -A && git commit -m "chore(desktop): 建 9 个功能域 mod 目录骨架（目录重组 Task 1）"
```

---

## Task 2: 迁移 action_bar/ 域（4 文件 + action_bar_commands/ 子目录）

**Files:**
- Move: `action_bar_window.rs` `action_hotkey.rs` `agent_adapter.rs` `terminal_launcher.rs` → `action_bar/`
- Move: `action_bar_commands/` → `action_bar/action_bar_commands/`（整体 git mv）
- Create: `action_bar/mod.rs`
- Modify: `main.rs`（`mod action_bar_window;` 等 4 行 → `mod action_bar;`）
- Modify: 全局 `crate::action_bar_window::` / `crate::action_hotkey::` / `crate::agent_adapter::` / `crate::terminal_launcher::` → `crate::action_bar::action_bar_window::` 等

**Interfaces:**
- Consumes: 无（第一个迁移的域）
- Produces: `crate::action_bar::<file>::xxx` 新路径

- [x] **Step 1: git mv 文件 + 子目录**

```bash
cd /Users/wudarui/workspace/agent/octopus/.worktrees/daily_bugfix_0729
git mv crates/desktop/src/action_bar_window.rs crates/desktop/src/action_bar/
git mv crates/desktop/src/action_hotkey.rs crates/desktop/src/action_bar/
git mv crates/desktop/src/agent_adapter.rs crates/desktop/src/action_bar/
git mv crates/desktop/src/terminal_launcher.rs crates/desktop/src/action_bar/
git mv crates/desktop/src/action_bar_commands crates/desktop/src/action_bar/
```

- [x] **Step 2: 创建 action_bar/mod.rs**

```rust
//! 命令面板功能域：action_bar_commands/ + 窗口 + 热键 + agent 适配器 + 终端启动。

pub mod action_bar_commands;
pub mod action_bar_window;
pub mod action_hotkey;
pub mod agent_adapter;
pub mod terminal_launcher;
```

- [x] **Step 3: 更新 main.rs mod 声明**

把原来的：
```rust
mod action_bar_window;
mod action_bar_commands;
// ...
mod action_hotkey;
mod agent_adapter;
mod terminal_launcher;
```
改为：
```rust
mod action_bar;
```

- [x] **Step 4: 全局路径迁移（4 文件 + 子目录内部）**

```bash
# action_bar_window（12 处）
find crates/desktop/src -name "*.rs" -exec sed -i '' 's/crate::action_bar_window::/crate::action_bar::action_bar_window::/g' {} +
# action_hotkey（2 处）
find crates/desktop/src -name "*.rs" -exec sed -i '' 's/crate::action_hotkey::/crate::action_bar::action_hotkey::/g' {} +
# agent_adapter（8 处）
find crates/desktop/src -name "*.rs" -exec sed -i '' 's/crate::agent_adapter::/crate::action_bar::agent_adapter::/g' {} +
# terminal_launcher（3 处）
find crates/desktop/src -name "*.rs" -exec sed -i '' 's/crate::terminal_launcher::/crate::action_bar::terminal_launcher::/g' {} +
```

注意：`crate::action_bar_commands::`（63 处）**不改**——它在 `action_bar/action_bar_commands/mod.rs` 里有 glob re-export，但路径从 `crate::action_bar_commands::` 变成 `crate::action_bar::action_bar_commands::`。需要改：

```bash
find crates/desktop/src -name "*.rs" -exec sed -i '' 's/crate::action_bar_commands::/crate::action_bar::action_bar_commands::/g' {} +
```

- [x] **Step 5: build + test 验证**

```bash
cargo build -p octopus-desktop --features embedded
cargo test -p octopus-desktop
```
Expected: 0 error 0 warning + 441 passed

- [x] **Step 6: Commit**

```bash
git add -A && git commit -m "refactor(desktop): action_bar/ 域迁移（4 文件 + action_bar_commands/ 子目录）"
```

---

## Task 3: 迁移 clipboard/ 域（4 文件）

**Files:**
- Move: `clipboard_commands.rs` `clipboard_window.rs` `clipboard_queue.rs` `clipboard_dock.rs` → `clipboard/`
- Create: `clipboard/mod.rs`
- Modify: `main.rs` + 全局路径迁移

路径迁移清单：
- `crate::clipboard_commands::`（25 处）→ `crate::clipboard::clipboard_commands::`
- `crate::clipboard_window::`（6 处）→ `crate::clipboard::clipboard_window::`
- `crate::clipboard_dock::`（7 处）→ `crate::clipboard::clipboard_dock::`
- `crate::clipboard_queue::`（3 处）→ `crate::clipboard::clipboard_queue::`

- [x] **Step 1-5**：同 Task 2 模式（git mv → mod.rs → main.rs → sed 全局替换 → build+test）

mod.rs 内容：
```rust
//! 剪贴板功能域：命令 + 窗口 + 队列 worker + dock。

pub mod clipboard_commands;
pub mod clipboard_window;
pub mod clipboard_queue;
pub mod clipboard_dock;
```

- [x] **Step 6: Commit**

```bash
git add -A && git commit -m "refactor(desktop): clipboard/ 域迁移（4 文件）"
```

---

## Task 4: 迁移 platform/ 域（7 文件 + app_context/ 子目录）

**Files:**
- Move: `input_source.rs` `finder_selection.rs` `keystroke.rs` `paste.rs` `sys_open.rs` `activation.rs` `focus_tracker.rs` → `platform/`
- Move: `app_context/` → `platform/app_context/`
- Create: `platform/mod.rs`
- Modify: `main.rs` + 全局路径迁移

路径迁移清单：
- `crate::app_context::`（32 处）→ `crate::platform::app_context::`
- `crate::activation::`（21 处）→ `crate::platform::activation::`
- `crate::keystroke::`（9 处）→ `crate::platform::keystroke::`
- `crate::sys_open::`（11 处）→ `crate::platform::sys_open::`
- `crate::focus_tracker::`（5 处）→ `crate::platform::focus_tracker::`
- `crate::input_source::`（2 处）→ `crate::platform::input_source::`
- `crate::finder_selection::`（2 处）→ `crate::platform::finder_selection::`
- `crate::paste::`（1 处）→ `crate::platform::paste::`

- [x] **Step 1-5**：同 Task 2 模式

mod.rs 内容：
```rust
//! 平台/输入辅助功能域：app_context（AX/ATSPI/UIA）+ 输入源 + 键盘 + 粘贴 + 系统打开 + 激活 + 焦点追踪。

pub mod app_context;
pub mod input_source;
pub mod finder_selection;
pub mod keystroke;
pub mod paste;
pub mod sys_open;
pub mod activation;
pub mod focus_tracker;
```

- [x] **Step 6: Commit**

```bash
git add -A && git commit -m "refactor(desktop): platform/ 域迁移（7 文件 + app_context/ 子目录）"
```

---

## Task 5: 迁移 commands/ 域（10 文件）

**Files:**
- Move: `model_commands.rs` `settings_commands.rs` `system_status_commands.rs` `hotword_commands.rs` `search_commands.rs` `compact_editor_commands.rs` `compact_editor_window.rs` `translation_commands.rs` `builtin_models.rs` `model_migrate.rs` → `commands/`
- Create: `commands/mod.rs`
- Modify: `main.rs` + 全局路径迁移

路径迁移清单（逐文件 sed）：
- `crate::model_commands::`（18）→ `crate::commands::model_commands::`
- `crate::settings_commands::`（18）→ `crate::commands::settings_commands::`
- `crate::compact_editor_commands::`（15）→ `crate::commands::compact_editor_commands::`
- `crate::hotword_commands::`（12）→ `crate::commands::hotword_commands::`
- `crate::vault_sync_commands::`（12）→ **不改**（属 vault 域，Task 7 处理）
- `crate::search_commands::`（9）→ `crate::commands::search_commands::`
- `crate::screenshot_geometry::`（9）→ **不改**（属 record 域）
- `crate::system_status_commands::`（5）→ `crate::commands::system_status_commands::`
- `crate::settings_window::`（5）→ **不改**（属 ui 域）
- `crate::builtin_models::`（4）→ `crate::commands::builtin_models::`
- `crate::compact_editor_window::`（3）→ `crate::commands::compact_editor_window::`
- `crate::translation_commands::`（2）→ `crate::commands::translation_commands::`
- `crate::model_migrate::`（1）→ `crate::commands::model_migrate::`

**注意**：`invoke_handler.rs` 也搬进 core/（Task 9），但它里面引用了大量 `crate::xxx_commands::`。invoke_handler 的迁移在 Task 9，此 Task 先不动它——但它的引用路径会被此 Task 的 sed 改到（因为它在 crates/desktop/src/ 下）。这是正确的。

- [x] **Step 1-5**：同 Task 2 模式

mod.rs 内容：
```rust
//! 独立命令文件域：模型/设置/系统状态/热词/搜索/紧凑编辑器/翻译 + builtin 模型 + 模型迁移。

pub mod model_commands;
pub mod settings_commands;
pub mod system_status_commands;
pub mod hotword_commands;
pub mod search_commands;
pub mod compact_editor_commands;
pub mod compact_editor_window;
pub mod translation_commands;
pub mod builtin_models;
pub mod model_migrate;
```

- [x] **Step 6: Commit**

```bash
git add -A && git commit -m "refactor(desktop): commands/ 域迁移（10 文件）"
```

---

## Task 6: 迁移 record/ 域（8 文件 + 2 子目录）

**Files:**
- Move: `record_area_picker.rs` `record_audio_probe.rs` `record_hotkey.rs` `record_window.rs` `record_annotation_window.rs` `record_control_window.rs` `screenshot_geometry.rs` `subtitle_polish.rs` → `record/`
- Move: `record_commands/` `screenshot_commands/` → `record/`（整体 git mv）
- Create: `record/mod.rs`
- Modify: `main.rs`（含 `#[cfg(target_os = "macos")]` gate）+ 全局路径迁移

路径迁移清单：
- `crate::record_commands::`（35）→ `crate::record::record_commands::`
- `crate::screenshot_commands::`（23）→ `crate::record::screenshot_commands::`
- `crate::subtitle_polish::`（8）→ `crate::record::subtitle_polish::`
- `crate::record_hotkey::`（7）→ `crate::record::record_hotkey::`
- `crate::record_annotation_window::`（7）→ `crate::record::record_annotation_window::`
- `crate::record_audio_probe::`（6）→ `crate::record::record_audio_probe::`
- `crate::screenshot_geometry::`（9）→ `crate::record::screenshot_geometry::`
- `crate::clipboard_dock::`→ **不改**
- `crate::record_window::`（5）→ `crate::record::record_window::`
- `crate::record_control_window::`（5）→ `crate::record::record_control_window::`
- `crate::record_area_picker::`（4）→ `crate::record::record_area_picker::`

- [x] **Step 1-5**：同 Task 2 模式。**注意 feature gate**：record 域文件多为 `#[cfg(target_os = "macos")]`，main.rs 原声明也带 gate，迁移后 `record/mod.rs` 的子 mod 声明要带 gate：

```rust
//! 录屏 + 截图功能域。

pub mod screenshot_commands;
pub mod screenshot_geometry;
pub mod subtitle_polish;
// macOS 独占：
#[cfg(target_os = "macos")]
pub mod record_commands;
#[cfg(target_os = "macos")]
pub mod record_area_picker;
#[cfg(target_os = "macos")]
pub mod record_audio_probe;
#[cfg(target_os = "macos")]
pub mod record_hotkey;
#[cfg(target_os = "macos")]
pub mod record_window;
#[cfg(target_os = "macos")]
pub mod record_annotation_window;
#[cfg(target_os = "macos")]
pub mod record_control_window;
```

- [x] **Step 6: Commit**

```bash
git add -A && git commit -m "refactor(desktop): record/ 域迁移（8 文件 + 2 子目录）"
```

---

## Task 7: 迁移 vault/ 域（5 文件 + 2 子目录）

**Files:**
- Move: `vault_error.rs` `vault_state.rs` `vault_secret_access.rs` `vault_sync_commands.rs` `password_generator_window.rs` → `vault/`
- Move: `vault_commands/` `autotype/` → `vault/`
- Create: `vault/mod.rs`
- Modify: `main.rs`（含 `#[cfg(feature = "vault")]` gate）+ 全局路径迁移

路径迁移清单：
- `crate::vault_commands::`（45）→ `crate::vault::vault_commands::`
- `crate::vault_state::`（19）→ `crate::vault::vault_state::`
- `crate::vault_error::`（15）→ `crate::vault::vault_error::`
- `crate::vault_sync_commands::`（12）→ `crate::vault::vault_sync_commands::`
- `crate::vault_secret_access::`（10）→ `crate::vault::vault_secret_access::`
- `crate::autotype::`（6）→ `crate::vault::autotype::`
- `crate::password_generator_window::`（3）→ `crate::vault::password_generator_window::`

**注意 feature gate**：vault 域全部 `#[cfg(feature = "vault")]`，但 `vault_secret_access` 是例外（总是编译）。main.rs 原声明：

```rust
#[cfg(feature = "vault")] pub mod vault_state;
#[cfg(feature = "vault")] pub mod vault_commands;
pub mod vault_secret_access;  // 总是编译
#[cfg(feature = "vault")] pub mod vault_error;
#[cfg(feature = "vault")] pub mod vault_sync_commands;
#[cfg(feature = "vault")] pub mod autotype;
#[cfg(feature = "vault")] pub mod password_generator_window;
```

迁移后 `vault/mod.rs`：
```rust
//! 密码库功能域。

#[cfg(feature = "vault")] pub mod vault_commands;
#[cfg(feature = "vault")] pub mod vault_state;
pub mod vault_secret_access;  // 总是编译（cloud 推理热路径用）
#[cfg(feature = "vault")] pub mod vault_error;
#[cfg(feature = "vault")] pub mod vault_sync_commands;
#[cfg(feature = "vault")] pub mod autotype;
#[cfg(feature = "vault")] pub mod password_generator_window;
```

main.rs 里 `vault` 整体不能简单 `#[cfg(feature = "vault")] mod vault;`——因为 `vault_secret_access` 要总是编译。所以 main.rs 保留 `mod vault;`（无 gate），gate 在 vault/mod.rs 内部。

- [x] **Step 1-5**：同 Task 2 模式

- [x] **Step 6: 额外验证 vault feature**

```bash
cargo build -p octopus-desktop --features embedded          # vault off
cargo build -p octopus-desktop --features embedded,cloud,vault  # vault on
```

- [x] **Step 7: Commit**

```bash
git add -A && git commit -m "refactor(desktop): vault/ 域迁移（5 文件 + 2 子目录）"
```

---

## Task 8: 迁移 engine/ 域（10 文件 + coordinator/ 子目录）

**Files:**
- Move: `engine.rs` `engine_embedded.rs` `engine_ws.rs` `engine_grpc.rs` `engine_dispatch.rs` `engine_aliyun.rs` `cloud_pipeline.rs` `pipeline.rs` `transcript.rs` `audio.rs` → `engine/`
- Move: `coordinator/` → `engine/coordinator/`
- Create: `engine/mod.rs`
- Modify: `main.rs`（含 `#[cfg(feature = "cloud")]` / `#[cfg(feature = "remote-ws")]` / `#[cfg(feature = "remote-grpc")]` gate）+ 全局路径迁移

路径迁移清单：
- `crate::coordinator::`（82）→ `crate::engine::coordinator::`
- `crate::pipeline::`（14）→ `crate::engine::pipeline::`
- `crate::engine::`（14）→ `crate::engine::engine::`（注意：`engine.rs` 是文件，搬到 engine/ 后路径是 `crate::engine::engine::`）
- `crate::transcript::`（13）→ `crate::engine::transcript::`
- `crate::audio::`（7）→ `crate::engine::audio::`
- `crate::cloud_pipeline::`（1）→ `crate::engine::cloud_pipeline::`
- `crate::engine_embedded::`（2）→ `crate::engine::engine_embedded::`
- `crate::engine_aliyun::`（2）→ `crate::engine::engine_aliyun::`
- `crate::engine_ws::`（1）→ `crate::engine::engine_ws::`
- `crate::engine_grpc::`（1）→ `crate::engine::engine_grpc::`
- `crate::engine_dispatch::`（1）→ `crate::engine::engine_dispatch::`

**注意**：`crate::engine::`（14 处）迁移成 `crate::engine::engine::` 容易和已有的 `crate::engine::coordinator::` 混淆。sed 替换要精确：先替换 `crate::engine_` 开头的（engine_embedded/ws/grpc/dispatch/aliyun），再替换 `crate::engine::`（精确匹配 `::` 后缀）。

- [x] **Step 1-5**：同 Task 2 模式

mod.rs 内容：
```rust
//! ASR 全栈功能域：引擎 trait + 实现 + 云端流水线 + pipeline + transcript + audio + coordinator。

pub mod engine;
pub mod engine_embedded;
#[cfg(feature = "remote-ws")] pub mod engine_ws;
#[cfg(feature = "remote-grpc")] pub mod engine_grpc;
pub mod engine_dispatch;
#[cfg(feature = "cloud")] pub mod engine_aliyun;
#[cfg(feature = "cloud")] pub mod cloud_pipeline;
pub mod pipeline;
pub mod transcript;
pub mod audio;
pub mod coordinator;
```

- [x] **Step 6: 验证 4 feature 组合**

```bash
cargo build -p octopus-desktop --features embedded
cargo build -p octopus-desktop --features embedded,cloud,vault
cargo build -p octopus-desktop --features remote-ws
cargo build -p octopus-desktop --features remote-grpc
cargo test -p octopus-desktop
```

- [x] **Step 7: Commit**

```bash
git add -A && git commit -m "refactor(desktop): engine/ 域迁移（10 文件 + coordinator/ 子目录）"
```

---

## Task 9: 迁移 ui/ 域（11 文件）

**Files:**
- Move: `result_window.rs` `pin_window.rs` `settings_window.rs` `onboarding_window.rs` `overlay_window.rs` `download_window.rs` `tray.rs` `i18n.rs` `theme.rs` `window_factory.rs` `window_position.rs` → `ui/`
- Create: `ui/mod.rs`
- Modify: `main.rs` + 全局路径迁移

路径迁移清单：
- `crate::tray::`（55）→ `crate::ui::tray::`
- `crate::result_window::`（48）→ `crate::ui::result_window::`
- `crate::window_position::`（21）→ `crate::ui::window_position::`
- `crate::theme::`（6）→ `crate::ui::theme::`
- `crate::window_factory::`（8）→ `crate::ui::window_factory::`
- `crate::settings_window::`（5）→ `crate::ui::settings_window::`
- `crate::pin_window::`（2）→ `crate::ui::pin_window::`
- `crate::onboarding_window::`（2）→ `crate::ui::onboarding_window::`
- `crate::download_window::`（2）→ `crate::ui::download_window::`
- `crate::overlay_window::`（1）→ `crate::ui::overlay_window::`

- [x] **Step 1-5**：同 Task 2 模式

mod.rs 内容：
```rust
//! 通用窗口 + UI 工具功能域。

pub mod result_window;
pub mod pin_window;
pub mod settings_window;
pub mod onboarding_window;
pub mod overlay_window;
pub mod download_window;
pub mod tray;
pub mod i18n;
pub mod theme;
pub mod window_factory;
pub mod window_position;
```

- [x] **Step 6: Commit**

```bash
git add -A && git commit -m "refactor(desktop): ui/ 域迁移（11 文件）"
```

---

## Task 10: 迁移 core/ 域（10 文件）

**Files:**
- Move: `setup.rs` `bootstrap.rs` `config.rs` `runtime_config.rs` `db_queue.rs` `invoke_handler.rs` `error_util.rs` `perf_log.rs` `file_watcher.rs` `extensions.rs` `shortcut.rs` → `core/`
- Create: `core/mod.rs`
- Modify: `main.rs` + 全局路径迁移

路径迁移清单：
- `crate::error_util::`（24）→ `crate::core::error_util::`
- `crate::config::`（38）→ `crate::core::config::`
- `crate::runtime_config::`（24）→ `crate::core::runtime_config::`
- `crate::perf_log::`（42）→ `crate::core::perf_log::`
- `crate::db_queue::`（5）→ `crate::core::db_queue::`
- `crate::extensions::`（6）→ `crate::core::extensions::`
- `crate::shortcut::`（3）→ `crate::core::shortcut::`
- `crate::file_watcher::`（2）→ `crate::core::file_watcher::`
- `crate::setup::`（1）→ `crate::core::setup::`

**注意**：`crate::config::` 要小心——`config.rs` 里可能有 `AppConfig` / `PolishMode` 等被广泛引用。还有 `invoke_handler.rs`——它里面引用了大量命令路径，但那些路径在前面 Task 已经迁移过了，此 Task 只需迁移 invoke_handler 自身的 `mod invoke_handler;` 声明（它在 main.rs 里是 `#[macro_use] mod invoke_handler;`）。

- [x] **Step 1-5**：同 Task 2 模式

mod.rs 内容：
```rust
//! 启动 + 基础设施功能域。

pub mod setup;
pub mod bootstrap;
pub mod config;
pub mod runtime_config;
pub mod db_queue;
#[macro_use]
pub mod invoke_handler;
pub mod error_util;
pub mod perf_log;
pub mod file_watcher;
pub mod extensions;
pub mod shortcut;
```

**注意**：`#[macro_use]` 要放在 `mod invoke_handler` 上方。但 `#[macro_use]` 在 `pub mod` 上可能需要调整——如果编译报错，改用 `pub use invoke_handler::handler;` 在 mod.rs 显式 re-export 宏。

- [x] **Step 6: 验证 + Commit**

```bash
cargo build -p octopus-desktop --features embedded
cargo build -p octopus-desktop --features embedded,cloud,vault
cargo test -p octopus-desktop
git add -A && git commit -m "refactor(desktop): core/ 域迁移（10 文件）"
```

---

## Task 11: 最终清理 + main.rs mod 声明整合 + 文档同步

**Files:**
- Modify: `main.rs`（确认 mod 声明只剩 9 行顶级 mod + feature gate）
- Verify: `desktop/src/` 下除 main.rs 外无平铺 .rs
- Update: `docs/architecture.md` + spec status

- [x] **Step 1: 确认无平铺文件残留**

```bash
find crates/desktop/src -maxdepth 1 -name "*.rs" ! -name "main.rs"
```
Expected: 无输出（全部已迁移）

- [x] **Step 2: 确认 main.rs mod 声明干净**

main.rs 应该只有 ~9 行顶级 mod 声明：
```rust
mod core;
mod engine;
mod record;
mod vault;
mod clipboard;
mod action_bar;
mod platform;
mod commands;
mod ui;
```
加上 `feature_flags` mod（在 main.rs 底部的测试模块，保留原位）。

- [x] **Step 3: 全量验证（4 feature 组合）**

```bash
cargo build -p octopus-desktop --features embedded
cargo build -p octopus-desktop --features embedded,cloud,vault
cargo build -p octopus-desktop --features remote-ws
cargo build -p octopus-desktop --features remote-grpc
cargo test -p octopus-desktop
```
Expected: 全部 0 error 0 warning + 441 passed

- [x] **Step 4: tsc + vite**

```bash
cd crates/desktop/frontend && npx tsc --noEmit && npm run build
```

- [x] **Step 5: 更新文档**

- spec `docs/superpowers/specs/2026-07-30-desktop-src-reorganize.md` status → ✅
- `docs/architecture.md`：更新 desktop crate 结构描述

- [x] **Step 6: Commit + push**

```bash
git add -A && git commit -m "refactor(desktop): 目录重组完成——70 文件归入 9 功能域 mod + 文档同步"
git push origin daily_refactor_record
```

---

## 验证 checklist（每个 Task 必跑）

- [x] `cargo build -p octopus-desktop --features embedded` — 0 error 0 warning
- [x] `cargo test -p octopus-desktop` — 441 passed, 0 failed, 1 ignored
- [x] git diff 确认：只搬文件 + 路径替换，无逻辑改动

## 回滚策略

每个 Task 独立 commit。失败 `git reset --hard HEAD~1` 回退该 Task。

## 自审

1. **Spec coverage**：spec 的 11 个域 → plan 的 Task 2-10 覆盖 9 个域（action_bar/clipboard/platform/commands/record/vault/engine/ui/core）+ Task 1 骨架 + Task 11 收尾。全覆盖。✅
2. **Placeholder scan**：无 TBD/TODO，每个 Task 有具体 sed 命令和 mod.rs 内容。✅
3. **Type consistency**：路径迁移模式统一（`crate::<file>::` → `crate::<domain>::<file>::`）。✅
