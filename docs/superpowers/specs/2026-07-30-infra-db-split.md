# infra/db.rs 拆分 spec（infra crate 大文件重构）

> **Status: ✅ 已完成**（2026-07-30，`crates/infra/src/db/` 9 子文件已落地）

## 背景

`crates/infra/src/db.rs` 5886 行（含 ~2100 行测试），是全项目最大的单文件。承载 SQLite 全部表的 CRUD：models / config / prompts / action_bar / agent / hotword / transcription / vault / launcher / search 等 10+ 个表的访问层。

lib.rs 用 `pub mod db;` 声明。外部引用统一 `octopus_infra::db::xxx`（`with_db` 被引用 62 次最多）。内部几乎没有跨模块依赖（只有 2 处 `use crate::config`）。

## 方案：按表域拆为 db/ 目录

把 5886 行按表域拆成 ~9 个子文件 + mod.rs。mod.rs 保留连接管理 + `with_db` + schema 初始化，用 **glob re-export**（`pub use <子文件>::*;`）保持 `octopus_infra::db::xxx` 路径不变。

**关键差异**：和 coordinator/action_bar 拆分一样，是单文件拆成多文件再 re-export。所有 `octopus_infra::db::with_db` / `octopus_infra::db::load_models` 等路径零改动。

## 域划分（8 子文件 + mod.rs）

| 子文件 | 行数（估） | 内容 | 表 |
|---|---|---|---|
| `mod.rs` | ~500 | 连接管理（`with_db`/`open_db_conn`/`db_path`/`set_test_db`/`clear_test_db`/`init_test_db`）+ schema 初始化（`ensure_db`/`init_schema`/`ensure_builtin_seed`/`fill_manifests`/`migrate_yaml_to_db`）+ `collect_rows` helper + `now_string`/`days_to_ymd`/`is_leap` 工具 | — |
| `models.rs` | ~900 | ModelEntry/AsrConfig/LocalAsrModelRow/ModelRow/ModelDetailRow/AsrEngineRow 等 struct + load/list/set/insert/update/delete/switch model 函数 | models / local_asr_models |
| `config.rs` | ~200 | load/save app_config + config_key + env_var | app_config / env_vars |
| `prompts.rs` | ~130 | PromptRecord + list/load/insert/update/delete prompt + active_prompt_id | prompts |
| `action_bar.rs` | ~450 | ActionBarItem + CRUD + shortcut + launcher + search_frequency + script_run | action_bar_items / launcher_index / search_frequency / script_runs |
| `agent.rs` | ~250 | AgentAdapterRecord + AgentTask + CRUD | agent_adapters / agent_tasks |
| `hotword.rs` | ~350 | HotwordSet + CRUD + words + hits + recent_text | hotword_sets / hotword_hits |
| `transcription.rs` | ~200 | insert/update/finalize/list/delete transcription + TranscriptionRecord | transcriptions（已废弃，并入 clipboard_history） |
| `vault.rs` | ~600 | VaultMeta/Cipher/Folder + meta + cipher + folder CRUD + secret migration | vault_meta / vault_ciphers / vault_folders |

## 拆分约束（不变量）

1. **glob re-export**：mod.rs `pub use <子文件>::*;`，`octopus_infra::db::xxx` 路径不变
2. **struct 也 re-export**：`ModelEntry` / `HotwordSet` 等被外部直接引用的 struct 通过 glob re-export 保持 `octopus_infra::db::ModelEntry` 路径
3. **`_at` 后缀的私有 helper**：很多函数有 `pub fn load_xxx()` → `fn load_xxx_at(conn: &Connection)` 模式。`_at` 版本是私有的，随对应的 pub 函数搬到同一子文件
4. **测试分布**：2 个测试模块（L3626 + L5730）按被测函数搬到对应子文件
5. **逻辑完全不变**：纯代码搬家

## 风险
低。与前 5 轮大文件拆分同模式（单文件拆多文件 + glob re-export）。唯一注意点是 struct/type 的 re-export。

## 不做
- 不改函数逻辑/签名
- 不改 SQL 语句
- 不改外部引用路径
