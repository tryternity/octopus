# Vault Sync is_deleted + updated_at merge 实施计划（plan）

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** vault_ciphers + vault_folders 的 `deleted_at`（TEXT 可空）→ `is_deleted`（INTEGER 0/1）；sync 的 pull+push 合并为按 `updated_at` 最新赢的单向 merge。

**Architecture:** db.sql is_deleted 改造（v53，2026-07-28 极简化重构后无 ALTER TABLE 迁移）→ struct 改动（VaultCipher/Input/CipherFile/fingerprint/Dto）→ `merge_vault` 函数替代 pull+push → 前端 `isDeleted`。

**Tech Stack:** Rust（rusqlite + serde）/ SQLite / React + TypeScript。

**关联文档：** spec `docs/superpowers/specs/2026-07-27-vault-sync-is-deleted-merge.md`

## Global Constraints

- **直接删 `deleted_at` 列**，不保留不兼容（用户确认可清库）。
- **`is_deleted INTEGER NOT NULL DEFAULT 0`**（SQLite 无 bool，用 0/1）。
- **cipher + folder 都加 `is_deleted`**（统一软删语义）。
- **真相源 = `updated_at` 最新的赢**；冲突（时间戳相同）→ DB 赢。
- **pull + push 合并为 `merge_vault`**——消除顺序依赖 + 删除传播。
- **删今天的 5 个临时保护**（db_all_empty / skip_pull / outline 保留 / pull 顺序 / app_key 清空）——merge 模型下自然消失。
- **每步 cargo build + cargo test -p <改动的 crate>**，最后 pnpm build。
- **app 启动由用户跑**（AI 不代跑）。

## File Structure

| 文件 | 改动 |
|---|---|
| `crates/infra/src/db.sql` | vault_ciphers + vault_folders CREATE TABLE 改 is_deleted |
| `crates/infra/src/db.rs` | VaultCipher/VaultCipherInput/VaultFolder struct 改 + list/insert/update/query SQL 改（2026-07-28 极简化重构后无 migrate_v52_to_v53，db.sql 是唯一真相源，`init_schema` 仅 3 分支 + `CURRENT_SCHEMA_VERSION = 53`） |
| `crates/vault/src/sync/store.rs` | CipherFile plaintext_meta 改 is_deleted + fingerprint cipher_md5 改 + incremental_export 改 |
| `crates/vault/src/sync/engine.rs` | merge_vault 替代 pull_from_files + push_to_files + sync_now 调用改 + 删 5 个临时保护 |
| `crates/vault/src/storage/cipher.rs` | decrypt_cipher_row / soft_delete / restore 改 is_deleted |
| `crates/vault/src/storage/folder.rs` | delete_folder → soft_delete_folder（is_deleted） |
| `crates/desktop/src/vault_commands.rs` | CipherDto/CipherInputDto 改 is_deleted + soft_delete/restore 命令改 |
| `crates/desktop/frontend/src/pages/Settings/Vault/CipherList.tsx` | deleted_at → isDeleted |
| `crates/desktop/frontend/src/pages/Settings/Vault/CipherEditor.tsx` | 同上 |

---

## Phase 1: DB Schema + Struct（基础设施）

### Task 1.1: db.sql is_deleted 改造 + CURRENT_SCHEMA_VERSION=53

> **2026-07-28 极简化重构注记**：原 Task 1.1 含 Step 3（写 migrate_v52_to_v53）+ Step 4（init_schema 加 v52 分支），重构后这两步已删除——db.sql 是唯一真相源，`init_schema` 仅 3 分支（v==0 全新库跑 db.sql / v==CURRENT_SCHEMA_VERSION no-op / 旧版 bail），不再有版本间迁移。Step 1/2（改 db.sql）已完成。

**Files:**
- Modify: `crates/infra/src/db.sql`（vault_ciphers + vault_folders CREATE TABLE 改 is_deleted）
- Modify: `crates/infra/src/db.rs`（仅 `CURRENT_SCHEMA_VERSION = 53` 常量；vault struct + SQL 改在 Task 1.2）

- [x] **Step 1: 改 db.sql vault_ciphers**

把 `deleted_at TEXT DEFAULT NULL` 改为 `is_deleted INTEGER NOT NULL DEFAULT 0`。索引 `WHERE deleted_at IS NULL` 改为 `WHERE is_deleted = 0`。删 `idx_vault_ciphers_deleted` 索引。

- [x] **Step 2: 改 db.sql vault_folders**

加 `is_deleted INTEGER NOT NULL DEFAULT 0`。加索引 `WHERE is_deleted = 0`。

- [~] **Step 3: ~~写 migrate_v52_to_v53~~**（2026-07-28 极简化重构删除）

原方案写 `migrate_v52_to_v53` 函数做 ALTER TABLE 迁移。重构后此函数已删除——db.sql 直接定义 `is_deleted`，全新库由 `init_schema` v==0 分支建出。保留此步骤为历史记录（划掉），不执行。

- [~] **Step 4: ~~init_schema 加 v52 分支~~**（2026-07-28 极简化重构删除）

原方案 `if v == 52 { migrate_v52_to_v53(conn)?; }`。重构后 `init_schema` 仅 3 分支，旧版本库（v<53）直接 bail 提示 `rm ~/.octopus/octopus.db*`。保留此步骤为历史记录（划掉），不执行。

- [x] **Step 5: 测试 + commit**

```bash
cargo test -p octopus-infra --lib
```

（注：原 `cargo test -p octopus-infra --lib migrate_v52` 过滤命令已无意义——`migrate_v52_*` 测试已删除）

---

### Task 1.2: VaultCipher + VaultCipherInput + VaultFolder struct 改 is_deleted

**Files:**
- Modify: `crates/infra/src/db.rs`（3 个 struct + 所有 SQL）

- [x] **Step 1: VaultCipher struct**

`pub deleted_at: Option<String>` → `pub is_deleted: bool`

- [x] **Step 2: VaultCipherInput struct**

同上。

- [x] **Step 3: 所有 SQL（list/insert/update/soft_delete/restore）改**

- `list_vault_ciphers`：SELECT 加 is_deleted 列（替 deleted_at）
- `insert_vault_cipher_at`：INSERT 加 is_deleted（替 deleted_at）
- `update_vault_cipher_at`：UPDATE SET 加 is_deleted
- `soft_delete_cipher`：`SET is_deleted = 1, updated_at = datetime('now')`
- `restore_cipher`：`SET is_deleted = 0, updated_at = datetime('now')`
- `list_deleted_ciphers`（回收站）：`WHERE is_deleted = 1`

- [x] **Step 4: VaultFolder struct + SQL**

加 `pub is_deleted: bool` + list/insert/update/delete SQL。

- [x] **Step 5: row_to_cipher / row_to_folder 解析改**

deleted_at → is_deleted（按列 index 调整）。

- [x] **Step 6: 测试 + commit**

```bash
cargo build -p octopus-infra
cargo test -p octopus-infra --lib
```

---

### Task 1.3: VaultCipher 所有构造点 + 调用点改 is_deleted

**Files:**
- Modify: `crates/vault/src/`（所有构造 VaultCipher/VaultCipherInput 的地方）
- Modify: `crates/desktop/src/vault_commands.rs`（CipherDto/CipherInputDto）

- [x] **Step 1: grep 所有构造点**

```bash
rg "VaultCipher \{|VaultCipherInput \{|deleted_at:" crates/vault/src/ crates/desktop/src/ crates/infra/src/ --type rust | grep -v test
```

逐个改 `deleted_at: None/Some(...)` → `is_deleted: false/true`。

- [x] **Step 2: CipherDto + CipherInputDto**

`pub deleted_at: Option<String>` → `pub is_deleted: bool`

- [x] **Step 3: cipher_to_dto / dto_to_input 映射**

`deleted_at: c.deleted_at` → `is_deleted: c.is_deleted`

- [x] **Step 4: build + commit**

---

## Phase 2: Sync 文件格式 + fingerprint

### Task 2.1: CipherFile 格式 + fingerprint 改 is_deleted

**Files:**
- Modify: `crates/vault/src/sync/store.rs`（CipherFile struct + read/write）
- Modify: `crates/vault/src/sync/fingerprint.rs`（cipher_md5）

- [x] **Step 1: CipherFile plaintext_meta**

`pub deleted_at: Option<String>` → `pub is_deleted: bool`

- [x] **Step 2: to_vault_cipher / from_vault_cipher 映射**

`deleted_at` → `is_deleted`

- [x] **Step 3: cipher_md5**

```rust
// 旧
c.deleted_at.as_deref().unwrap_or(""),
// 新
c.is_deleted as u8,
```

`cipher_md5_from_input` 同样改。

- [x] **Step 4: folder_md5**

folder 也加 `is_deleted` 到 md5（如果 folder 有 sync_md5 的话）。

- [x] **Step 5: 测试 + commit**

---

## Phase 3: merge_vault（核心）

### Task 3.1: merge_vault 函数

**Files:**
- Modify: `crates/vault/src/sync/engine.rs`

- [x] **Step 1: 写 merge_vault 替代 pull_from_files + push_to_files**

按 spec §3.1 实现。核心逻辑：
```rust
fn merge_vault() -> Result<MergeReport, SyncError> {
    // 1. 读 outline + DB
    // 2. app_key 一致性校验（stamp + sync_enc）
    // 3. merge folder（先，FK 被引用方）
    // 4. merge cipher（后，FK 引用方）
    // 5. merge meta
    // 6. 写 outline + meta
}
```

- [x] **Step 2: MergeReport struct**

```rust
pub struct MergeReport {
    pub pulled: usize,
    pub pushed: usize,
    pub conflicts: usize,
    pub skipped: usize,
}
```

- [x] **Step 3: sync_now 调用改**

```rust
// 旧
let (pulled, skipped) = pull_from_files()?;
let pushed = push_to_files()?;
// 新
let report = merge_vault()?;
```

- [x] **Step 4: 删 5 个临时保护**

- incremental_export db_all_empty 检查（store.rs）
- skip_pull（engine.rs sync_now）
- incremental_export outline 保留（store.rs）
- pull 顺序修复（engine.rs pull_from_files——整个函数被 merge_vault 替代）
- app_key local_enc 清空（engine.rs pull_from_files meta upsert——合并到 merge_vault meta 阶段）

- [x] **Step 5: 保留 pull_from_files + push_to_files 为 pub(crate)**

clone_initial（B 机首次 clone）仍用 pull_from_files。不删函数，只改 sync_now 不调用。

- [x] **Step 6: 测试 + commit**

---

### Task 3.2: 回归测试（merge_vault 核心）

**Files:**
- Modify: `crates/vault/src/sync/engine.rs`（test module）

- [x] **Step 1: 更新 sync_recovers_data_when_db_emptied 用 merge_vault**

- [x] **Step 2: 新增 merge_vault_updated_at_wins 测试**

DB 有 cipher（updated_at=T1）→ push → 改 DB cipher（updated_at=T2）→ 改 .sync 文件（updated_at=T3）→ merge → .sync 赢（T3 > T2）

- [x] **Step 3: 新增 merge_vault_conflict_db_wins 测试**

updated_at 相同 + md5 不同 → DB 赢

- [x] **Step 4: 新增 soft_delete_propagates 测试**

A 机 soft_delete → push → B 机 merge → cipher.is_deleted=true

- [x] **Step 5: cargo test -p octopus-vault 全过 + commit**

---

## Phase 4: storage + commands 改 is_deleted

### Task 4.1: cipher storage + folder storage 改

**Files:**
- Modify: `crates/vault/src/storage/cipher.rs`（decrypt/list/soft_delete/restore）
- Modify: `crates/vault/src/storage/folder.rs`（list/delete→soft_delete）

- [x] **Step 1: list_ciphers 返回所有（含 is_deleted=true）**

前端按 is_deleted 过滤显示（回收站 vs 列表）。

- [x] **Step 2: soft_delete_cipher**

`SET is_deleted = 1, updated_at = datetime('now')`

- [x] **Step 3: restore_cipher**

`SET is_deleted = 0, updated_at = datetime('now')`

- [x] **Step 4: folder delete_folder → soft_delete_folder**

`SET is_deleted = 1, updated_at = datetime('now')`

- [x] **Step 5: build + test + commit**

---

### Task 4.2: vault_commands 改

**Files:**
- Modify: `crates/desktop/src/vault_commands.rs`

- [x] **Step 1: vault_soft_delete_cipher / vault_restore_cipher**

调用 storage 的 soft_delete / restore（已改 is_deleted）。

- [x] **Step 2: vault_delete_folder → vault_soft_delete_folder**

- [x] **Step 3: build + commit**

---

## Phase 5: 前端

### Task 5.1: CipherList + CipherEditor 改 isDeleted

**Files:**
- Modify: `crates/desktop/frontend/src/pages/Settings/Vault/CipherList.tsx`
- Modify: `crates/desktop/frontend/src/pages/Settings/Vault/CipherEditor.tsx`

- [x] **Step 1: interface 改**

```typescript
// 旧
deleted_at: string | null;
// 新
isDeleted: boolean;
```

- [x] **Step 2: 列表过滤**

```typescript
// 旧
ciphers.filter(c => !c.deleted_at)
// 新
ciphers.filter(c => !c.isDeleted)
```

- [x] **Step 3: 回收站列表**

```typescript
// 旧
ciphers.filter(c => c.deleted_at)
// 新
ciphers.filter(c => c.isDeleted)
```

- [x] **Step 4: pnpm build + commit**

---

## Phase 6: 集成验证 + 文档

### Task 6.1: 全量回归

- [x] **Step 1: cargo build（全部）**
- [x] **Step 2: cargo test（全部）**
- [x] **Step 3: pnpm build**
- [x] **Step 4: 用户 e2e（清库 → 重建 vault → sync → 密码恢复 + 解密成功）**

### Task 6.2: 文档同步

- [ ] spec 实现注记回填
- [ ] architecture.md 更新（sync 章节）
- [ ] z-sync-superpowers

---

## 实施偏差（review plan）

<!-- 待填 -->
