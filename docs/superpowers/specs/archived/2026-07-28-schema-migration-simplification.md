# Schema Migration 极简化重构

- **Status:** ✅ 已实现（2026-07-28）
- **Date:** 2026-07-28
- **关联：** `docs/architecture.md` §「迁移机制」；`crates/infra/src/db.rs` + `db.sql`

## 背景

历史 `init_schema` 维护了 v17→v53 的完整迁移链（`migrate_v47_to_v48` ... `migrate_v52_to_v53` 共 6 个函数）+ 多处自愈逻辑（`vault_ciphers_drop_deleted_at`、recordings 表缺失自愈、`app_index` 残留自愈等）。这套机制的问题：

1. **代码膨胀**：`db.rs` 7000+ 行，迁移代码占大头，全是死代码（单用户开发库，唯一用户 DB 早已 ≥v38）
2. **半完成 bug 难修**：2026-07-28 实测 `migrate_v52_to_v53` 的 DROP COLUMN 顺序 bug——SQLite 执行 DROP COLUMN 时重建依赖索引，旧索引 `idx_vault_ciphers_favorite WHERE deleted_at IS NULL` 重建时报 `no such column: deleted_at`。修复需要「先 DROP INDEX 再 DROP COLUMN」的精细顺序，维护成本高
3. **自愈逻辑不可靠**：多处分支检测 + 重复执行路径，容易遗漏（如自愈路径的 DROP COLUMN 顺序 bug 与 migration 路径不同步）

## 决策

**删除所有 migration 代码 + 自愈逻辑，db.sql 作为唯一 schema 真相源。** 旧版本库一律清库重建（`rm ~/.octopus/octopus.db*`），不支持自动迁移。

理由：单用户开发库，schema 变更频率低，每次变更直接改 db.sql + 升 `CURRENT_SCHEMA_VERSION` 即可。维护冗长迁移链的成本远高于偶尔清库（数据可从 `.sync` git 仓库恢复）。

## 设计

### `init_schema` 三分支（极简）

```rust
fn init_schema(conn: &Connection) -> Result<()> {
    let v: u32 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    if v == CURRENT_SCHEMA_VERSION { return Ok(()); }           // 最新，no-op
    if v == 0 {
        // 全新库：db.sql 建表 + seed + manifest + 外置 seed + yaml 迁移
        conn.execute_batch(INIT_SQL)?;
        migrate_yaml_to_db(conn)?;
        fill_manifests(conn)?;
        crate::seeds::load_external_seeds(conn)?;
        conn.execute(&format!("PRAGMA user_version = {}", CURRENT_SCHEMA_VERSION), [])?;
        return Ok(());
    }
    // 1 <= v < CURRENT_SCHEMA_VERSION：旧版本库，bail 提示清库
    anyhow::bail!(
        "DB schema version {} is outdated (current {}). \
         Run: rm ~/.octopus/octopus.db*",
        v, CURRENT_SCHEMA_VERSION
    );
}

pub const CURRENT_SCHEMA_VERSION: u32 = 53;
```

### db.sql 结构重组

db.sql 分两个清晰的区块：
- **§A 表结构（DDL）**：所有 `CREATE TABLE` / `CREATE INDEX` / `CREATE TRIGGER` / `CREATE VIRTUAL TABLE`
- **§B 初始化数据（seed）**：所有 `INSERT OR IGNORE` seed

所有 CREATE 在 §B 之前完成，所有 INSERT 在 §A 之后才开始。

### 删除清单

| 删除项 | 原位置 |
|---|---|
| `migrate_v47_to_v48` | db.rs（models.is_local → source_type） |
| `migrate_v48_to_v49` | db.rs（action_bar_items.app_bundle_ids） |
| `migrate_v49_to_v50` | db.rs（prompts content → 文件名引用） |
| `migrate_v50_to_v51` | db.rs（recordings 表） |
| `migrate_v51_to_v52` | db.rs（recordings.audio_tracks） |
| `migrate_v52_to_v53` | db.rs（vault is_deleted） |
| `vault_ciphers_drop_deleted_at` | db.rs（v53 自愈） |
| recordings 表缺失自愈 | db.rs（v53+ 分支） |
| 所有 migration 测试 | db.rs tests（v26/v32/v38/v42/v45/v47/v48/v50/v51/v52 共 10+ 个） |

净删除 **1122 行代码**。

## 验收标准

| # | 检查项 | 通过标准 |
|---|---|---|
| A1 | 全新库建库 | `init_schema`（v==0）→ 所有表创建 + 60 行 app_config seed + `user_version = 53` |
| A2 | 旧版本库 bail | `0 < v < 53` → `init_schema` 返回 Err（提示清库），不执行任何 ALTER |
| A3 | 最新库 no-op | `v == 53` → `init_schema` 立即返回 Ok，不读 db.sql |
| A4 | 测试全过 | infra 154 + vault 261 + sync 108 + desktop 422 = 945 tests pass |

## 顺带修复

本次重构同时修复了两个独立 bug：

1. **快捷键 seed 缺失 + 不一致**：db.sql 补上 `vault_autotype_shortcut` / `vault_generator_shortcut` / `vault_lock_timeout_secs`；config.rs 默认值与 db.sql seed 对齐（6 个快捷键原不一致）
2. **tray 菜单 copy-paste bug**：`tray.rs` 截图菜单项误用 ASR 的 `sc` 变量，改为 `config.screenshot_shortcut`；`rebuild_tray_labels` 改为接受 `&AppConfig` 参数；set_config 后刷新 tray 文案
3. **删除死数据 `record_stop_shortcut` seed**：Escape 是硬编码常量（`record_hotkey.rs::STOP_SHORTCUT`），不是配置项；同步简化 `settings_commands.rs` 的判断逻辑
