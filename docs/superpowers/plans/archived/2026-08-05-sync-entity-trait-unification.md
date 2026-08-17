# Sync Entity Trait 统一实施计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 抽 `trait SyncEntity` + 泛型 `merge_three_way`，消除 vault/hotword/clipboard 3 模块共 ~430 行 3-way merge 重复代码；统一 tombstone 为 i64；所有模块获得 GC 能力。

**Architecture:** 在 `octopus-sync/src/pipeline.rs` 定义 `SyncEntity` trait（11 method + 2 assoc type）+ 泛型 `merge_three_way` + `pull_entity` helper。5 个实体类型（VaultCipher/Folder + HotwordSet/Word + ClipboardFavorite）各 impl trait。各模块保留外层编排（vault FK 顺序 / hotword 双层 outline / clipboard key 加载）。vault is_deleted bool→i64（schema v59→v60）。

**Tech Stack:** Rust（trait + 泛型 + associated type）/ SQLite（schema 迁移）/ serde（bool→i64 兼容反序列化）

> **实施记录（2026-08-05 执行完毕，2026-08-14 归档前回写）**：全部 Task 已完成（本文 checkbox 未逐项回写，**完整实施记录见 [spec §12](../../specs/archived/2026-08-05-sync-entity-trait-unification-design.md)**——含 6 项偏差表、行数统计、1777 tests passed）。核心产物：`crates/sync/src/pipeline.rs`（trait `SyncEntity` + `merge_three_way` + `pull_entity`）+ 5 个 impl（vault cipher/folder + hotword set/word + clipboard favorite）+ v59→v60 迁移（vault `is_deleted` bool→i64 epoch）+ 三模块 tombstone GC（vault/clipboard 30 天、hotword 10 天）。

## Global Constraints

- **零行为变更**：3-way 判定顺序（tombstone 优先 → updated_at → md5）、tombstone 单向优先、拒绝复活守卫语义不变
- **.sync 文件兼容**：旧 JSON 的 `"isDeleted": true/false` 必须能被新代码（i64）反序列化
- **GC 不丢活跃数据**：只删 `is_deleted > 0` 且超期的行；活跃行（is_deleted=0）永不被 GC
- **测试全过**：vault 262 + sync 145 + clipboard 24 + workspace 其余 crate 测试不回归

## Spec Coverage

| Spec Section | Task |
|---|---|
| §3 trait SyncEntity 定义 | Task 1 |
| §4 泛型 merge 骨架 | Task 1 |
| §6 vault is_deleted bool→i64 迁移 | Task 2 |
| §6.3 .sync 文件兼容 | Task 2 |
| §5.3 clipboard 外层编排 + impl | Task 3 |
| §7.1 clipboard GC | Task 3 |
| §5.2 hotword 外层编排 + impl | Task 4 |
| §5.1 vault 外层编排 + impl | Task 5 |
| §7.1 vault GC | Task 5 |
| §10 阶段 6 清理 | Task 6 |

---

## File Structure

| 文件 | 职责 | 创建/修改 |
|---|---|---|
| `crates/sync/src/pipeline.rs` | trait SyncEntity + merge_three_way + pull_entity + MergeReport | 创建 |
| `crates/sync/src/lib.rs` | 注册 pipeline 模块 | 修改 |
| `crates/infra/src/db/vault.rs` | VaultCipher/VaultFolder is_deleted bool→i64 | 修改 |
| `crates/infra/resources/sql/schema.sql` | v59→v60 迁移 | 修改 |
| `crates/vault/src/sync/store.rs` | CipherFile/FolderFile is_deleted bool→i64 + serde 兼容 | 修改 |
| `crates/vault/src/sync/fingerprint.rs` | md5 拼接 is_deleted as u8 → i64 直接拼 | 修改 |
| `crates/sync/src/clipboard.rs` | impl SyncEntity + merge 改调 merge_three_way | 修改 |
| `crates/sync/src/hotword.rs` | impl SyncEntity + merge 改调 merge_three_way | 修改 |
| `crates/vault/src/sync/engine.rs` | impl SyncEntity + merge_vault 改调 merge_three_way | 修改 |
| `crates/desktop/src/core/setup.rs` | scheduler 注册 vault/clipboard GC | 修改 |

---

## Task 1: pipeline.rs 基础设施（trait + merge_three_way + pull_entity）

**Files:**
- Create: `crates/sync/src/pipeline.rs`
- Modify: `crates/sync/src/lib.rs`

**Interfaces:**
- Produces: `pub trait SyncEntity` / `pub fn merge_three_way<E: SyncEntity>(report, now)` / `pub fn pull_entity<E>()` / `pub struct MergeReport`

- [ ] **Step 1: 创建 pipeline.rs 骨架**

写 trait SyncEntity + MergeReport struct + merge_three_way 泛型函数 + pull_entity helper。具体代码见 spec §3 + §4。trait method 先写签名 + 默认实现，不写 impl。

```rust
// crates/sync/src/pipeline.rs
use crate::outline::OutlineEntry;
use anyhow::Result;
use std::collections::{HashMap, HashSet};

/// 同步实体的 3-way merge 结果报告。
#[derive(Debug, Clone, Default)]
pub struct MergeReport {
    pub pulled: usize,
    pub pushed: usize,
    pub conflicts: usize,
    pub skipped: usize,
}

/// 同步实体 trait——每个可同步的实体类型实现此 trait。
pub trait SyncEntity {
    type Row: Clone;
    type File;

    const LABEL: &'static str;

    fn tombstone_retention_secs() -> i64 { 0 }

    fn list_db_rows() -> Result<Vec<Self::Row>>;
    fn sync_key(row: &Self::Row) -> &str;
    fn updated_ms(row: &Self::Row) -> i64;
    fn is_tombstone(row: &Self::Row) -> bool;
    fn md5_of(row: &Self::Row) -> String;

    fn read_file(key: &str) -> Result<Self::File>;
    fn file_to_row(file: &Self::File) -> Self::Row;
    fn file_is_tombstone(file: &Self::File) -> bool;
    fn file_tombstone_timestamp(file: &Self::File) -> i64;
    fn write_file(row: &Self::Row) -> Result<()>;

    fn upsert_db_from_file(row: &Self::Row) -> Result<bool>;

    fn purge_expired_tombstones(_now: i64) -> Result<usize> { Ok(0) }

    fn export_all() -> Result<()>;

    fn read_outline_entries() -> Result<Vec<(String, OutlineEntry)>>;
}

/// 3-way merge 泛型骨架（spec §4.1）
pub fn merge_three_way<E: SyncEntity>(report: &mut MergeReport, now: i64) -> Result<()> {
    // ... spec §4.1 的实现
}

/// pull 单个实体（spec §4.2）
pub fn pull_entity<E: SyncEntity>(key: &str, report: &mut MergeReport, now: i64) -> Result<()> {
    // ... spec §4.2 的实现
}

/// 检查 tombstone 是否超期
fn is_tombstone_expired(retention_secs: i64, deleted_at: i64, now: i64) -> bool {
    retention_secs > 0 && (now - deleted_at) > retention_secs
}

fn log_warn_skip<E: SyncEntity>(key: &str, op: &str, e: anyhow::Error, report: &mut MergeReport) {
    log::warn!("[sync] {} merge: {} {} 跳过：{}", E::LABEL, key, op, e);
    report.skipped += 1;
}
```

- [ ] **Step 2: 在 lib.rs 注册 pipeline 模块**

```rust
// crates/sync/src/lib.rs 追加
pub mod pipeline;
pub use pipeline::{MergeReport, SyncEntity, merge_three_way, pull_entity};
```

- [ ] **Step 3: 编译验证**

Run: `cargo build -p octopus-sync`
Expected: 编译通过（trait 未被 impl 会有 dead_code warning，暂时接受）

- [ ] **Step 4: Commit**

```bash
git add crates/sync/src/pipeline.rs crates/sync/src/lib.rs
git commit -m "feat(sync): pipeline.rs 基础设施——trait SyncEntity + 泛型 merge_three_way"
```

---

## Task 2: vault is_deleted bool→i64（schema v59→v60 + .sync 兼容）

**Files:**
- Modify: `crates/infra/src/db/vault.rs`（VaultCipher/VaultFolder is_deleted 类型）
- Modify: `crates/infra/resources/sql/schema.sql`（v59→v60 迁移 SQL）
- Modify: `crates/vault/src/sync/store.rs`（CipherFile/FolderFile is_deleted + serde 兼容）
- Modify: `crates/vault/src/sync/fingerprint.rs`（md5 拼接）
- Modify: `crates/vault/src/sync/engine.rs`（is_deleted == true → > 0）
- Modify: `crates/vault/src/storage/`（所有读 is_deleted 的地方）
- Modify: `crates/desktop/src/vault/`（vault 命令层）

**Interfaces:**
- Consumes: 无（独立 schema 迁移）
- Produces: VaultCipher/VaultFolder is_deleted 为 i64；.sync 文件兼容旧 bool

- [ ] **Step 1: TDD——写 vault is_deleted i64 测试**

在 vault 测试模块加测试：插入 is_deleted=当前 epoch 的 cipher，查询确认 is_deleted > 0。

- [ ] **Step 2: 改 infra VaultCipher/VaultFolder is_deleted bool→i64**

`crates/infra/src/db/vault.rs`：`pub is_deleted: bool` → `pub is_deleted: i64`，所有 `as i32` → 直接用 i64。

- [ ] **Step 3: 加 v59→v60 迁移 SQL**

`schema.sql` 追加迁移分支：`UPDATE vault_ciphers/folders SET is_deleted = CASE WHEN is_deleted != 0 THEN <epoch> ELSE 0 END`。更新 CURRENT_SCHEMA_VERSION 常量。

- [ ] **Step 4: 改 vault CipherFile/FolderFile is_deleted + serde 兼容**

`vault/src/sync/store.rs`：CipherFile/FolderFile `is_deleted: bool` → `i64`。加 `#[serde(deserialize_with = "...")]` 兼容旧 `"isDeleted": true`（true→epoch，false→0）。

- [ ] **Step 5: 改 vault fingerprint md5 拼接**

`fingerprint.rs`：`is_deleted as u8` → `is_deleted`（i64 直接拼入 format!）。

- [ ] **Step 6: 全工程 grep 替换 is_deleted 检查**

`rg "is_deleted == true|is_deleted == false|is_deleted != 0|\.is_deleted" crates/vault/ crates/desktop/src/vault/` → 所有 `== true` → `> 0`，`== false` → `== 0`。

- [ ] **Step 7: 编译 + vault 测试**

Run: `cargo test -p octopus-vault --lib`
Expected: 262 passed（可能需更新测试中的 is_deleted: false → 0）

- [ ] **Step 8: Commit**

---

## Task 3: clipboard impl SyncEntity + GC + merge 改调 merge_three_way

**Files:**
- Modify: `crates/sync/src/clipboard.rs`

**Interfaces:**
- Consumes: `SyncEntity` trait from Task 1, `merge_three_way` from Task 1
- Produces: `impl SyncEntity for ClipboardFavoriteEntity` + clipboard GC

- [ ] **Step 1: 定义 ClipboardFavoriteEntity struct + impl SyncEntity**

在 clipboard.rs 加 `struct ClipboardFavoriteEntity;` + impl SyncEntity。各 method 委托到现有函数（`pull_favorite` / `push_favorite` / `read_favorite_file` / `write_favorite_file` / `list_all_favorites` / `history_row_md5` / `export_all_favorites`）。

需处理 `ClipboardKey`——用 thread-local 或在 read_file/write_file 内 `load_or_create_clipboard_key()`。

- [ ] **Step 2: 加 clipboard GC**

实现 `purge_expired_tombstones`——硬删超期 favorite DB 行 + 删 .sync 文件。`tombstone_retention_secs = 30天`。

- [ ] **Step 3: merge_clipboard_favorites 改调 merge_three_way**

原 110 行 merge 主循环替换为：
```rust
pub fn merge_clipboard_favorites() -> Result<ClipboardMergeReport> {
    let key = load_or_create_clipboard_key()?;
    set_thread_clipboard_key(key);
    let mut report = MergeReport::default();
    merge_three_way::<ClipboardFavoriteEntity>(&mut report, now_secs())?;
    Ok(report.into())
}
```

- [ ] **Step 4: 编译 + sync 测试**

Run: `cargo test -p octopus-sync`
Expected: 145+ passed

- [ ] **Step 5: Commit**

---

## Task 4: hotword impl SyncEntity + merge 改调 merge_three_way

**Files:**
- Modify: `crates/sync/src/hotword.rs`

**Interfaces:**
- Consumes: SyncEntity trait, merge_three_way
- Produces: `impl SyncEntity for HotwordSetEntity + HotwordWordEntity`

- [ ] **Step 1: 定义 HotwordSetEntity + impl SyncEntity**

impl 委托到 `pull_set` / `read_hotword_set_file` / `write_hotword_set_file` / `list_all_hotword_sets` / `hotword_set_md5` / `export_all_hotwords`。`purge_expired_tombstones` 委托到现有 `purge_expired_hotword_tombstones_at`。`tombstone_retention_secs = 10天`。

- [ ] **Step 2: 定义 HotwordWordEntity + impl SyncEntity**

word 的特殊处理：key 编码 `set_id/word_uuid`，trait method 从 key 解析 set_id。`read_outline_entries` 读指定 set 的 word outline（需 thread-local 传 set_id）。

- [ ] **Step 3: merge_hotwords 改调 merge_three_way**

原 set + word 两阶段 merge 替换为：
```rust
merge_three_way::<HotwordSetEntity>(&mut report, now)?;
for set in &latest_sets {
    set_thread_current_set_id(&set.id);
    merge_three_way::<HotwordWordEntity>(&mut report, now)?;
}
```

- [ ] **Step 4: 编译 + sync 测试**

Run: `cargo test -p octopus-sync`
Expected: 145+ passed

- [ ] **Step 5: Commit**

---

## Task 5: vault impl SyncEntity + GC + merge_vault 改调 merge_three_way

**Files:**
- Modify: `crates/vault/src/sync/engine.rs`（或 sync crate——取决于 VaultCipher 定义位置）

**Interfaces:**
- Consumes: SyncEntity trait（Task 1）+ vault is_deleted i64（Task 2）
- Produces: `impl SyncEntity for VaultCipherEntity + VaultFolderEntity` + vault GC

- [ ] **Step 1: 定义 VaultFolderEntity + VaultCipherEntity + impl SyncEntity**

impl 委托到 `upsert_folder_from_file` / `push_folder_to_files` / `read_folder_file` / `list_vault_folders` / `folder_md5` / `export_all_to_files`。`tombstone_retention_secs = 30天`。

- [ ] **Step 2: 加 vault GC**

实现 `purge_expired_tombstones`——硬删超期 cipher/folder DB 行 + 删 .sync 文件 + outline。

- [ ] **Step 3: merge_vault 改调 merge_three_way**

保留 stamp 校验（阶段 A）+ meta upsert（阶段 D）不变。中间的 cipher/folder merge 主循环替换为：
```rust
merge_three_way::<VaultFolderEntity>(&mut report, now_secs())?;
merge_three_way::<VaultCipherEntity>(&mut report, now_secs())?;
```

- [ ] **Step 4: 编译 + vault 测试**

Run: `cargo test -p octopus-vault --lib`
Expected: 262 passed

- [ ] **Step 5: Commit**

---

## Task 6: 清理 + workspace 全量验证

**Files:**
- Modify: 各模块删除被 merge_three_way 取代的旧 merge 主循环代码
- Modify: `crates/desktop/src/core/setup.rs`（scheduler 注册 vault/clipboard GC）

- [ ] **Step 1: 删除各模块旧 merge 主循环**

grep 确认无残留的旧 merge 骨架代码（`for (uuid, entry) in &outline` 在 vault engine / hotword / clipboard 中应已替换）。

- [ ] **Step 2: scheduler 注册 vault/clipboard GC**

`desktop/src/core/setup.rs`：仿 hotword GC 注册，加 vault/clipboard GC 定时任务。

- [ ] **Step 3: workspace 全量构建 + 测试**

Run: `cargo build --workspace && cargo test --workspace`
Expected: 0 error 0 warning，全量测试通过

- [ ] **Step 4: clippy**

Run: `cargo clippy --workspace --all-targets`
Expected: 0 新增 warning

- [ ] **Step 5: Commit + review plan 回写 spec**

spec §8 追加实施记录（偏差表 + 行数统计 + 验证结果）。

---

## Self-Review

**1. Spec coverage:** ✅ §3→T1, §4→T1, §6→T2, §5.3+§7.1 clipboard→T3, §5.2 hotword→T4, §5.1+§7.1 vault→T5, §10 阶段6→T6。全覆盖。

**2. Placeholder scan:** Task 步骤用了 "委托到现有函数" 而非完整代码——这是**有意简化**（每步实现量大，完整代码会让 plan 超长）。实际执行时需读现有函数签名对接。标注了具体函数名（`pull_set` / `push_favorite` 等），非模糊引用。

**3. Type consistency:** trait SyncEntity 定义在 Task 1，后续 Task 3/4/5 各 impl——method 签名一致。VaultCipher is_deleted 在 Task 2 改 i64，Task 5 vault impl 用 i64——一致。
