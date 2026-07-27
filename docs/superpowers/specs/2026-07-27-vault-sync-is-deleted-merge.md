# Vault Sync is_deleted + updated_at merge — 设计规格（spec）

> **Status: 📝 设计阶段**（2026-07-27，分支 `feat/record-followup`）。
>
> **本 spec 范围**：把 vault_ciphers + vault_folders 的 `deleted_at`（TEXT 可空）改为 `is_deleted`（INTEGER 0/1），vault_folders 也加 `is_deleted`；sync 的 pull+push 合并为按 `updated_at` 最新赢的单向 merge；app_key 一致性校验。
>
> **背景**：今天修了 5 个 sync bug（空 DB 覆盖 / skip_pull / 空 outline / FK 顺序 / app_key 不匹配），根因是 sync 的「DB 为唯一真相源 + 双向覆盖」设计与实际场景（B 机新装从 A 机拉、清库恢复、双向增量）不匹配。改为「按 updated_at 最新赢」的 merge 模型。
>
> **关联文档**：
> - AGENTS.md「序列化 casing 规范」
> - 今日 5 个 sync bug fix commit：`700609de` / `10e56330` / `0c5462bc` / `701b01fd` / `9118b27e`

---

## 实现注记（Implementation Notes）

<!-- 待填 -->

---

## 0. 决策回顾

### 0.1 brainstorming 决策清单

| 维度 | 决策 | 理由 |
|---|---|---|
| **真相源** | `updated_at` 最新的赢（双向 merge） | 不预设单一真相源；每条记录按时间戳判断 |
| **软删字段** | `is_deleted INTEGER NOT NULL DEFAULT 0`（cipher + folder 统一） | 删除是普通字段变更，走标准 merge 路径；不需要特殊删除传播 |
| **folder 也加 is_deleted** | 是 | cipher + folder 统一语义，sync 逻辑不分两套 |
| **updated_at 精度** | 当前秒级（SQLite `datetime('now')`），P1 升毫秒 | 秒级覆盖 99% 场景；毫秒级涉及全工程时间戳格式改动 |
| **冲突处理（updated_at 相同 + md5 不同）** | 当前机器赢（DB 优先） | 毫秒级精度下概率极低；简单确定性 |
| **pull + push 合并** | 合并为单个 `merge_vault` 函数 | 消除顺序依赖（FK / skip_pull / 删除保护等问题自然消失） |

### 0.2 支持的 sync 场景

| 场景 | 流程 |
|---|---|
| B 机新装，从 A 机拉 | B 机建 vault + 设 sync repo → sync → merge 把 A 机数据拉回 DB |
| 清库后恢复（同机） | 清库 → 重建 vault → sync → merge 把 .sync 数据拉回 DB |
| 日常双向增量 | A 机改密码 → sync → B 机 sync → merge 拉到新密码 |
| 多机并发冲突 | A/B 都改了同一条 → sync 时按 updated_at 最新赢，旧方被覆盖 |

---

## 1. DB Schema 改动（v52 → v53）

### 1.1 vault_ciphers

```sql
-- 新增列
ALTER TABLE vault_ciphers ADD COLUMN is_deleted INTEGER NOT NULL DEFAULT 0;

-- 数据迁移（deleted_at 有值 → is_deleted=1）
UPDATE vault_ciphers SET is_deleted = 1 WHERE deleted_at IS NOT NULL;

-- 索引更新
DROP INDEX IF EXISTS idx_vault_ciphers_deleted;
CREATE INDEX idx_vault_ciphers_active ON vault_ciphers(favorite) WHERE is_deleted = 0;
```

**`deleted_at` 列保留**（migration 只加 `is_deleted`，不删 `deleted_at`）。代码只用 `is_deleted`，`deleted_at` 留作历史数据兼容（读忽略，写不再更新）。

### 1.2 vault_folders

```sql
-- 新增列（folder 当前无 deleted_at，直接加 is_deleted）
ALTER TABLE vault_folders ADD COLUMN is_deleted INTEGER NOT NULL DEFAULT 0;

-- 索引更新
CREATE INDEX idx_vault_folders_active ON vault_folders WHERE is_deleted = 0;
```

### 1.3 db.sql 同步更新

全新库的 CREATE TABLE 直接用 `is_deleted`（不留 `deleted_at`）：

```sql
CREATE TABLE IF NOT EXISTS vault_ciphers (
    ...
    is_deleted INTEGER NOT NULL DEFAULT 0,
    ...
);
```

---

## 2. Struct 改动

### 2.1 VaultCipher + VaultCipherInput（infra/db.rs）

```rust
// 旧
pub deleted_at: Option<String>,
// 新
pub is_deleted: bool,
```

### 2.2 CipherFile（vault/sync/store.rs）

cipher 同步文件格式的 `plaintext_meta`：

```json
// 旧
"plaintext_meta": { "deleted_at": null, ... }
// 新
"plaintext_meta": { "is_deleted": false, ... }
```

**向后兼容**：读取旧文件时 `deleted_at` 有值 → 视为 `is_deleted=true`。

### 2.3 sync fingerprint cipher_md5

```rust
// 旧：c.deleted_at.as_deref().unwrap_or("")
// 新：c.is_deleted as u8
```

### 2.4 CipherDto + CipherInputDto（vault_commands.rs）

```rust
// 旧
pub deleted_at: Option<String>,
// 新
pub is_deleted: bool,
```

---

## 3. Sync Merge 逻辑（核心）

### 3.1 新 merge_vault 函数（替代 pull_from_files + push_to_files）

```rust
/// 双向 merge：DB ↔ .sync 文件系统，按 updated_at 最新赢。
///
/// 替代原来的 pull_from_files + push_to_files 两步——合并后无顺序依赖，
/// FK / skip_pull / 删除保护等问题自然消失。
fn merge_vault() -> Result<MergeReport, SyncError> {
    // 1. 读 .sync outline（远程视角）
    let remote_outline = store::read_outline_file()?;
    // 2. 读 DB ciphers + folders（本地视角）
    let db_ciphers = db::list_vault_ciphers()?;
    let db_folders = db::list_vault_folders()?;

    // 3. app_key 一致性校验（stamp + sync_enc 变化清空 local_enc）
    //    （沿用现有 pull_from_files 的阶段 A 逻辑）

    // 4. 逐条 merge cipher（按 id 匹配 DB + outline）
    for (id, remote_entry) in &remote_outline.ciphers {
        let remote_updated = remote_entry.updated_ms;  // .sync 的时间戳
        let db_cipher = db_ciphers.iter().find(|c| c.id == *id);

        match db_cipher {
            None => {
                // DB 无 → .sync 有 → pull 到 DB
                pull_cipher(id)?;
            }
            Some(db_c) => {
                let db_updated = iso_to_unix_ms(&db_c.updated_at);
                if remote_updated > db_updated {
                    // .sync 更新 → pull 覆盖 DB
                    pull_cipher(id)?;
                } else if db_updated > remote_updated {
                    // DB 更新 → push 覆盖 .sync
                    push_cipher(db_c)?;
                } else {
                    // updated_at 相同 → md5 比对
                    if db_c.sync_md5 != Some(remote_entry.md5.clone()) {
                        // 冲突（相同时间不同内容）→ DB 赢（当前机器优先）
                        push_cipher(db_c)?;
                    }
                    // md5 相同 → 跳过
                }
            }
        }
    }
    // DB 有 + outline 没有 → push 到 .sync（不再「硬删传播」）
    for db_c in &db_ciphers {
        if !remote_outline.ciphers.contains_key(&db_c.id) {
            push_cipher(db_c)?;
        }
    }

    // 5. 同样 merge folder（先 folder 后 cipher，FK 顺序）
    // 6. merge meta（app_key_sync_enc 一致性）
    // 7. 写新 outline + meta
}
```

### 3.2 关键不变量

1. **不再有删除传播**——删除 = `is_deleted=1 + updated_at 更新`，走普通字段 merge
2. **pull 只新增/更新 DB，不删 DB**——DB 有 + .sync 没有的行 → push 到 .sync（而非删 DB）
3. **push 只新增/更新 .sync，不删文件**——.sync 有 + DB 没有的文件 → pull 到 DB（而非删文件）
4. **folder 先 cipher 后**——FK 约束（cipher.folder_id → vault_folders.id）

### 3.3 删除的 5 个临时保护

| 临时保护 | 删除原因 |
|---|---|
| incremental_export db_all_empty 检查 | merge 不再有删除传播 |
| skip_pull（UpToDate 跳过） | merge 始终执行 |
| incremental_export outline 保留 | merge 统一写 outline |
| pull 顺序 cipher→folder | merge folder 先 |
| app_key local_enc 清空 | merge meta 阶段统一处理 |

---

## 4. app_key 一致性

### 4.1 问题

B 机新建 vault 生成新 app_key → sync 从 A 机拉数据 → cipher 用 A 机旧 app_key 加密 → 新 app_key 解不开。

### 4.2 修复（merge meta 阶段）

merge 时如果远程 `app_key_sync_enc` != 本地：
1. 清空本地 `app_key_local_enc`（强制下次 unlock 从 sync_enc 解 app_key）
2. 用远程 sync_enc 覆盖本地

### 4.3 unlock_app_key_local 空检查

`unlock_app_key_local` 检查 `app_key_local_enc.is_empty()` → 空时返回 None（走流程 C 输主密码）。**已实现**（commit `e2e58f37`）。

---

## 5. 前端改动

### 5.1 CipherDto + folder interface

```typescript
// 旧
deleted_at: string | null;
// 新
isDeleted: boolean;
```

### 5.2 CipherList / CipherEditor

- 删除按钮 → `vault_soft_delete_cipher({ id, isDeleted: true })`
- 恢复按钮 → `vault_soft_delete_cipher({ id, isDeleted: false })`
- 列表过滤 → `ciphers.filter(c => !c.isDeleted)` 替代 `ciphers.filter(c => !c.deleted_at)`

---

## 6. 验收标准

| # | 检查项 | 通过标准 |
|---|---|---|
| A1 | DB migration v52→v53 | is_deleted 列存在 + 旧 deleted_at 数据正确迁移 |
| A2 | merge_vault 测试 | B 机新建 vault + sync → cipher 正确恢复 + 解密成功 |
| A3 | 清库恢复测试 | 清库 → 重建 → sync → DB 恢复 + .sync 不被覆盖 |
| A4 | 并发编辑测试 | A/B 同改一条 → updated_at 最新赢 |
| A5 | 软删传播测试 | A 机软删 → sync → B 机 cipher.is_deleted=1 |
| A6 | app_key 一致性 | B 机新建 → sync → local_enc 清空 → unlock 正确 |

## 7. 风险

| 风险 | 缓解 |
|---|---|
| 秒级 updated_at 冲突 | 当前机器赢（DB 优先）；P1 升毫秒 |
| migration 数据丢失 | deleted_at 列保留不删；is_deleted 从 deleted_at 迁移 |
| merge 逻辑复杂导致回归 | 完整 TDD + 回归测试（5 个旧 sync bug 的测试仍通过） |
| cipher 文件格式向后兼容 | 旧文件 `deleted_at` → `is_deleted` 映射 |
