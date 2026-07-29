# action_bar_commands.rs 拆分 spec（desktop crate 大文件重构 #2）

> **Status: 🔨 待实现**（2026-07-29，分支 `daily_refactor_action_bar`）

## 背景

`crates/desktop/src/action_bar_commands.rs` 2441 行（含 359 行测试），是 desktop crate 当前最大文件。承载 AI 命令面板的全部功能：上下文检测、窗口触发、命令项 CRUD、翻译、prompt 文件管理、脚本执行、agent 适配器。

coordinator.rs 拆分（重构 #1）已完成。action_bar_commands.rs 是下一个最大的复杂度债务。

## 现状结构分析

### 与 coordinator 的关键差异

action_bar_commands.rs **不是 actor 模式**，而是 ~50 个 `pub fn`（多数是 `#[tauri::command]`）+ helper 函数 + 少量 struct/enum。函数按功能域聚类清晰，但**跨文件引用密集**：

- **invoke_handler.rs** 注册 34 个命令（`action_bar_commands::xxx`）
- **10+ 个外部文件**通过 `crate::action_bar_commands::xxx` 引用 50 个符号（coordinator/agent.rs、overlay_window、password_generator_window、action_hotkey、paste.rs 等）

### 拆分策略：全量 re-export 保持路径不变

子模块用 `pub use self::<submodule>::*;` 全量 re-export 到 mod.rs，使 `crate::action_bar_commands::xxx` 路径**完全不变**——外部 10+ 文件和 invoke_handler.rs 零改动。这是与 coordinator 拆分的主要差异（coordinator 是内部模块，引用少，用精确 re-export；action_bar 引用密集，用 glob re-export）。

### 职责聚类（9 组 → 7 子模块 + mod.rs）

| 子模块 | 行数 | 内容 |
|---|---|---|
| `mod.rs`（留） | ~30 | mod 声明 + `pub use self::*::*;` 全量 re-export |
| `context.rs` | ~770 | ContextKind / ActionBarContext / Selection / detect_selection / clipboard helpers / 鼠标位置 / 上下文日志 |
| `window.rs` | ~200 | trigger_action_bar / show_action_bar_* / finalize / position 计算 / guard |
| `items.rs` | ~250 | 命令项 CRUD（list/create/update/delete/move/set_global_shortcut）+ script_runs 管理 + restore_prompt_from_seed |
| `translate.rs` | ~320 | do_translate / TranslateStrategy / streaming / session / cache / translate_text |
| `prompt_files.rs` | ~180 | render_agent_prompt / resolve_prompt_reference / list/open/create/save/read + format_paths/derive_cwd |
| `script.rs` | ~320 | ScriptResult / spawn_script / run_script_* / wait_* / runtime detect (js/ts) / execute_action_bar_inner / execute_action_bar |
| `agent.rs` | ~140 | agent 适配器 CRUD + trigger_agent_voice + agent_tasks 管理 |

## 目标目录结构

```
crates/desktop/src/action_bar_commands/
├── mod.rs           # ~30 行：mod 声明 + pub use self::*::*; 全量 re-export
├── context.rs       # 上下文检测（Selection / detect_selection / clipboard / 鼠标 / 日志）
├── window.rs        # 窗口触发 + 定位 + guard
├── items.rs         # 命令项 CRUD + script_runs
├── translate.rs     # 翻译（strategy / streaming / cache）
├── prompt_files.rs  # prompt 文件管理
├── script.rs        # 脚本执行（runtime detect / spawn / wait / execute）
└── agent.rs         # agent 适配器 + 任务
```

`action_bar_commands.rs` 从 2441 → mod.rs ~30 行 + 7 子模块（每个 140–770 行）。

## 拆分约束（不变量）

### 1. 全量 re-export（核心约束）

mod.rs 顶部：
```rust
mod context;
mod window;
mod items;
mod translate;
mod prompt_files;
mod script;
mod agent;

pub use context::*;
pub use window::*;
pub use items::*;
pub use translate::*;
pub use prompt_files::*;
pub use script::*;
pub use agent::*;
```

这样 `crate::action_bar_commands::xxx`（50 个符号）路径完全不变，外部 10+ 文件 + invoke_handler.rs 零改动。

### 2. 子模块间引用

子模块间用 `use super::<fn/type>` 引用（经 mod.rs re-export 可见）或 `use crate::action_bar_commands::<fn>` （等价，但前者更简洁）。

### 3. 可见性

- 被 re-export 的函数/类型保持原可见性（`pub` / `pub(crate)`）——re-export 不改可见性
- 子模块内部 helper 保持私有

### 4. 测试分布

359 行测试按被测函数搬到对应子模块。需先读测试内容确认每个测试引用哪些函数。

### 5. 逻辑完全不变

纯代码搬家——不改函数体、不改签名。每个子模块搬完后 `cargo build + cargo test` 验证。

## 风险

| 风险 | 等级 | 应对 |
|---|---|---|
| glob re-export 符号冲突（两个子模块有同名私有项） | 低 | 私有项不参与 re-export；如冲突编译器报错 |
| 测试搬错位置 | 低 | 测试跟着被测函数走，先读测试引用 |
| 子模块间循环依赖 | 低 | 函数已是扁平的，无循环；如出现用 super:: 而非 crate:: |
| `#[cfg(target_os)]` gate | 低 | macOS 专属函数（get_mouse_position）cfg 跟着函数搬 |

## 不做

- 不改函数逻辑（只搬家）
- 不改 Tauri 命令签名
- 不改 invoke_handler.rs 注册路径（靠 re-export 保持）
- 不改外部 10+ 文件的 `crate::action_bar_commands::xxx` 引用（靠 re-export 保持）
