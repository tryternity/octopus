# 识别记录 voice 软删 设计

> 2026-08-14 · 修复 delete_transcriptions_at 物理删 voice 绕过 bigram 语料保留

## 问题

设置页「识别记录」tab 删 voice 记录走 `delete_transcriptions`（infra），直接物理 DELETE 所有类型——绕过 clipboard 的 voice-aware 分流（长 voice 软删供 bigram 挖掘 INV-C1）。

**两条删除路径语义不一致**：
- 剪贴板历史删 voice（`delete_item`）：长 voice 软删（is_deleted=1，保留 bigram 语料）+ 短 voice 物理删
- 设置页识别记录删 voice（`delete_transcriptions`）：直接物理删（语料丢失）

**跨 crate 约束**：voice 软删逻辑（`is_voice_worth_keeping` / `soft_delete` / `enforce_voice_trash_limit`）在 clipboard crate；`delete_transcriptions_at` 在 infra crate（低层，不能依赖 clipboard）。

## 方案：改调用方

调用方在 desktop（依赖 clipboard + infra），改调用方即可绕过跨 crate 约束——无需移动代码到其他 crate。

### 删除路径（2 处 desktop 调用方）

| 文件 | 当前 | 改为 |
|---|---|---|
| `db_queue.rs:134` | `octopus_infra::db::delete_transcriptions(&[id])` | `octopus_infra::db::with_db(\|conn\| octopus_clipboard::store::delete_items(conn, &[id]))` |
| `settings_commands.rs:583` | `octopus_infra::db::delete_transcriptions(&ids)` | `octopus_infra::db::with_db(\|conn\| octopus_clipboard::store::delete_items(conn, &ids))` |

`delete_items` 已有完整 voice 分流 + favorite 孤儿清理 + enforce_trash_limit。infra 的 `delete_transcriptions_at` 保留（低层原语，测试用）。

### 列表查询（3 处 infra 查询加 `is_deleted = 0`）

切换到 soft-delete 后，软删的 voice 项（is_deleted=1）需从设置页列表隐藏（但保留 DB 供 bigram）。三个查询都缺 `is_deleted` 过滤：

| 查询 | 位置 | 当前 WHERE | 改为 |
|---|---|---|---|
| 基础列表 | `list_transcriptions_at` | `WHERE item_type = 'voice'` | `WHERE item_type = 'voice' AND is_deleted = 0` |
| FTS5 搜索 | `list_transcriptions_search_at` | `WHERE c.item_type = 'voice' AND c.rowid IN (...)` | 加 `AND c.is_deleted = 0` |
| LIKE 搜索 | `list_transcriptions_search_at` | `WHERE c.item_type = 'voice' AND c.content LIKE ?1` | 加 `AND c.is_deleted = 0` |

这与 clipboard 历史的 `query_history` 行为一致（过滤 `is_deleted = 0`，软删 voice 隐藏但保留语料）。

## 不涉及

- infra 的 `delete_transcriptions` / `delete_transcriptions_at` 保留不动（低层原语，测试用）
- 剪贴板历史删除路径（已正确走 `delete_items`）
- `list_transcriptions_at` 的 `ORDER BY` / `LIMIT` / 列映射不变

## 验证

- `cargo build -p octopus-desktop --features "cloud,embedded,vault"` —— 0 error 0 warning
- `cargo test -p octopus-infra --lib` —— 全过（含 transcription 测试）
- `cargo test -p octopus-desktop --features "cloud,embedded,vault"` —— 全过
