# 热词 set 级软删 + tombstone sync 实施计划

> **日期**：2026-08-02
> **spec**：`docs/superpowers/specs/2026-08-02-hotword-set-soft-delete.md`
> **分支**：`feature/hotword-set-soft-delete`（worktree `.worktrees/hotword-set-soft-delete`，从 main `795a05d2`）
> **状态**：🔄 实施中

## 任务分解（TDD）

### Task 1：schema v57→v58 迁移（infra）
- `db.sql`：`hotword_sets` 加 `is_deleted INTEGER NOT NULL DEFAULT 0` + UNIQUE 改 `UNIQUE(name, is_deleted)`（去 name 单列 UNIQUE）
- `db/mod.rs`：`CURRENT_SCHEMA_VERSION` 57→58；加 `57 => { 建新表+复制+DROP+RENAME }` 迁移分支
- 更新 mod.rs 版本注释
- **TDD**：建 v57 测试库 → 迁移 → 验证数据完整 + UNIQUE(name,is_deleted) 语义

### Task 2：DB 层软删（infra/db/hotword.rs）
- `HotwordSet` struct + `HOTWORD_SET_COLS` + `row_to_hotword_set`：加 `is_deleted: i64`
- `delete_hotword_set_at`：硬删→软删（`UPDATE SET is_deleted=now_secs, updated_at=now` + 级联软删词记录）
- `list_hotword_sets_at`：加 `WHERE is_deleted=0`
- `list_active_words_at`：JOIN 加 `AND s.is_deleted=0`
- `upsert_hotword_set_at`：ON CONFLICT(id) SET 加 `is_deleted=excluded.is_deleted`
- 新增 `list_all_hotword_sets()`（不过滤，sync export 用）
- `get_hotword_set_at` **不加过滤**
- **TDD**：软删后 list 不见 + get 能读 + 级联词软删 + 重建同名不冲突 + upsert 传播 is_deleted

### Task 3：sync 层 tombstone（sync/hotword.rs）
- `HotwordSetMeta` 加 `is_deleted: i64`（version 1→2，serde(default) 兼容）
- `HotwordSet` 的 from/to 转换带 is_deleted
- `hotword_set_md5_from_fields(name, enabled, is_deleted)`：md5 输入含 is_deleted
- `export_all_hotwords` / `incremental_export_hotwords_with`：改用 `list_all_hotword_sets`（含 tombstone）
- `pull_set`：`upsert_hotword_set` 传播 is_deleted
- **TDD**：`merge_set_soft_delete_propagates`（A 软删集 → B merge 后 B 的集变软删）

### Task 4：desktop + 文档
- desktop 命令层 + 前端无改动（db::delete 已软删，list 已过滤）
- spec `2026-08-01-hotword-sync-merge-model.md` §5 `[ ]` → `[x]`
- architecture.md 热词段加 set 软删 + schema v58
- AGENTS.md schema v57→v58

## 验证
```bash
cargo test -p octopus-infra --lib
cargo test -p octopus-sync --lib
cargo build --workspace
cargo test --workspace --lib
```

## 实施记录（review 阶段回写实际偏差）

### 实际偏差（vs 计划）

1. **新增 `hard_delete_hotword_set`（test-only 真删）**：计划没提。实际发现测试清理样板（27 处 `let _ = db::delete_hotword_set(&h.id)`）依赖硬删清空 DB，软删后行还在破坏测试隔离。加 `hard_delete_hotword_set`（pub，文档标「仅测试/重置用，生产用 delete_hotword_set 软删」），测试清理全部改用它。vault engine 测试 1 处清理也改。

2. **`delete_propagates_through_sync` 测试重写**：原测硬删语义（文件消失 + 活跃集数减少）。软删后重写为：软删 tombstone 传播——meta.json 仍在（is_deleted>0），list 过滤掉，但不再是「文件消失」。新增 `merge_set_soft_delete_propagates` 测跨设备 tombstone 传播（对称 word 级 `merge_soft_delete_propagates`）。

3. **`write_remote_set` 扩展为 `_with` 版**：加 `write_remote_set_with(id, name, updated_ms, is_deleted)` 支持 tombstone 测试（原 `write_remote_set` 委托，is_deleted=0）。

### 验证结果（全过）

| 验证项 | 结果 |
|---|---|
| `cargo test -p octopus-infra --lib` | ✅ 172 passed（+2 新：set 软删 + 重建同名 + upsert tombstone） |
| `cargo test -p octopus-sync --lib` | ✅ 122 passed（+1 新：merge_set_soft_delete_propagates；delete_propagates 重写） |
| `cargo build --workspace` | ✅ 0 error |
| `cargo test --workspace --lib` | ✅ 全 crate 0 failed |
| desktop `hotword_commands` 测试 | ✅ 12 passed |
| tsc | N/A（无前端改动——全 Rust + 文档） |

