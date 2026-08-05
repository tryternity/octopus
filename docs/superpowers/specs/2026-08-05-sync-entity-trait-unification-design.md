# Sync Entity Trait 统一设计——3 模块 merge 流水线抽象

- 日期：2026-08-05
- 分支：`refactor/too-many-arguments`
- 类型：架构重构（跨 crate trait 抽象 + schema 迁移）
- 触发：代码审查发现的跨模块架构级重复（vault/hotword/clipboard 3 模块 3-way merge 同构）

---

## 1. 背景与动机

octopus 有 3 个可同步模块（vault cipher/folder + hotword set/word + clipboard favorite），各自独立实现了同构的 3-way merge 逻辑：

| 模块 | merge 函数 | 行数 | 位置 |
|---|---|---:|---|
| vault cipher | `merge_vault` 内 cipher 段 | ~90 | `vault/src/sync/engine.rs:1211-1302` |
| vault folder | `merge_vault` 内 folder 段 | ~90 | `vault/src/sync/engine.rs:1114-1204` |
| hotword set | `merge_hotwords` 内 set 阶段 | ~80 | `sync/src/hotword.rs:848-924` |
| hotword word | `merge_hotword_words` | ~60 | `sync/src/hotword.rs:1005-1081` |
| clipboard favorite | `merge_clipboard_favorites` | ~110 | `sync/src/clipboard.rs:497-606` |

**共 ~430 行重复**——6 段 merge 主循环的控制流逐行同构（outline 有/无 × DB 有/无 → pull/push/conflict/skip）。

### 1.1 同构点（可抽象）

1. **merge 主循环骨架**：`for (key, entry) in &outline { match db_by_id.get(key) { None→pull, Some→{ tombstone优先 / 时间戳比较 / md5冲突 } } }` + DB-only → push
2. **3-way 判定顺序**：tombstone 单向优先 → `remote > local` pull → `local > remote` push → 相等 md5 比对 DB 赢
3. **tombstone 单向优先 fix**（2026-08-04/05）：远程 tombstone 时无条件 pull
4. **outline 增量索引**：`OutlineEntry { md5, updated_ms }`（已在 `sync/src/outline.rs` 共享）
5. **report 四字段**：pulled/pushed/conflicts/skipped
6. **pull report 累加模式**：`match pull { Ok(true)→pulled+=1, Ok(false)→skipped+=1, Err→skipped+=1 }`

### 1.2 差异点（模块特有，保留）

| 维度 | vault 特有 | hotword 特有 | clipboard 特有 |
|---|---|---|---|
| stamp/meta | stamp 校验 + meta.json + EmptyRecoveryNeedsPassword | — | — |
| FK 约束 | folder 先 cipher 后（FK constraint） | — | — |
| outline 结构 | 单层（cipher + folder 同级两 map） | 双层（set outline + 每 set word outline） | 单层 |
| 加解密 | — | — | AES-GCM + ClipboardKey |
| GC | 无（tombstone 永久堆积） | 完整（超期清理 + orphan + merge skip + export 过滤） | 无 |
| tombstone 类型 | bool（true/false） | i64（0/epoch） | i64（0/epoch） |
| md5 分隔 | 纯 `\|`（含 is_deleted） | set 纯 `\|`；word 长度前缀 | 纯 `\|` |
| word 不可变 | — | word 无 md5 冲突分支 | — |

### 1.3 目标

- 抽 `trait SyncEntity` + 泛型 `merge_three_way` + `run_sync_pipeline`，消除 6 段 merge 重复
- vault is_deleted 统一为 i64（bool→i64，schema v59→v60）
- 所有模块获得 GC 能力（超期 tombstone 清理）
- 3-way 判定维持现有 3 步（tombstone 优先 → updated_at → md5）

### 1.4 非目标

- ❌ 不改 git 操作逻辑（git pull/push/commit 保持现有 `sync/src/git.rs`）
- ❌ 不改加密算法（vault 加密 / clipboard AES-GCM 保持不变）
- ❌ 不改 outline 文件格式（JSON 结构不变）
- ❌ 不统一 md5 分隔策略（各模块保留各自的安全策略）

---

## 2. 架构

```
┌─────────────────────────────────────────────────────┐
│  octopus-sync/src/pipeline.rs（新建）                 │
│                                                       │
│  trait SyncEntity { 11 method + 2 assoc type }       │
│  fn merge_three_way<E: SyncEntity>(report, now)      │ ← 泛型 merge 骨架
│  fn pull_entity<E: SyncEntity>(key, report, now)     │ ← report 累加 helper
│  fn run_sync_pipeline<E: SyncEntity>()               │ ← GC→pull→merge→push
├─────────────────────────────────────────────────────┤
│  各模块 trait impl                                     │
│                                                       │
│  vault:   VaultCipher + VaultFolder impl              │
│  hotword: HotwordSet + HotwordWord impl               │
│  clipboard: ClipboardFavorite impl                    │
├─────────────────────────────────────────────────────┤
│  各模块外层编排（仓库级 + 跨实体协调）                  │
│                                                       │
│  vault: stamp/meta + merge_folder + merge_cipher     │
│  hotword: merge_set + foreach set { merge_word }     │
│  clipboard: key 加载 + merge_favorite                  │
└─────────────────────────────────────────────────────┘
```

---

## 3. trait SyncEntity 定义

```rust
/// 同步实体——每个可同步的实体类型实现此 trait，由 [`merge_three_way`] 驱动 3-way merge。
pub trait SyncEntity {
    type Row: Clone;     // DB 行类型
    type File;           // .sync 文件类型

    const LABEL: &'static str;

    /// tombstone 超期保留秒数。0 = 永久保留（不做 GC）。
    fn tombstone_retention_secs() -> i64 { 0 }

    // ── DB 操作 ──
    fn list_db_rows() -> Result<Vec<Self::Row>>;
    fn sync_key(row: &Self::Row) -> &str;
    fn updated_ms(row: &Self::Row) -> i64;
    fn is_tombstone(row: &Self::Row) -> bool;          // is_deleted > 0
    fn md5_of(row: &Self::Row) -> String;

    // ── 文件操作 ──
    fn read_file(key: &str) -> Result<Self::File>;
    fn file_to_row(file: &Self::File) -> Self::Row;
    fn file_is_tombstone(file: &Self::File) -> bool;
    fn file_tombstone_timestamp(file: &Self::File) -> i64; // 用于 GC 超期判定
    fn write_file(row: &Self::Row) -> Result<()>;

    // ── merge 操作 ──
    fn upsert_db_from_file(row: &Self::Row) -> Result<bool>;  // false = 拒绝复活

    // ── GC（默认 noop）──
    fn purge_expired_tombstones(_now: i64) -> Result<usize> { Ok(0) }

    // ── 导出 ──
    fn export_all() -> Result<()>;

    // ── outline ──
    fn read_outline_entries() -> Result<Vec<(String, OutlineEntry)>>;
}
```

---

## 4. 泛型 merge 骨架

### 4.1 merge_three_way

```rust
fn merge_three_way<E: SyncEntity>(report: &mut MergeReport, now: i64) -> Result<()> {
    let outline_entries = E::read_outline_entries()?;
    let db_rows = E::list_db_rows()?;
    let db_by_id: HashMap<&str, &E::Row> = db_rows.iter().map(|r| (E::sync_key(r), r)).collect();
    let outline_keys: HashSet<&str> = outline_entries.iter().map(|(k, _)| k.as_str()).collect();

    for (key, entry) in &outline_entries {
        let remote_updated = entry.updated_ms;
        match db_by_id.get(key.as_str()) {
            None => { pull_entity::<E>(key, report)?; }
            Some(db_row) => {
                let local_updated = E::updated_ms(db_row);
                let remote_file = E::read_file(key)?;  // Err → skipped
                let retention = E::tombstone_retention_secs();
                let remote_is_tombstone = E::file_is_tombstone(&remote_file)
                    && (retention <= 0 || !is_tombstone_expired(retention, E::file_tombstone_timestamp(&remote_file), now));

                if remote_is_tombstone {
                    pull_entity::<E>(key, report)?;
                } else if remote_updated > local_updated {
                    pull_entity::<E>(key, report)?;
                } else if local_updated > remote_updated {
                    push_or_skip::<E>(db_row, key, report);
                } else if E::md5_of(db_row) != entry.md5 {
                    if push_or_skip::<E>(db_row, key, report) { report.conflicts += 1; }
                }
                // md5 相同 → skip
            }
        }
    }

    // DB 有 + outline 无 → push
    for row in &db_rows {
        if !outline_keys.contains(E::sync_key(row)) {
            push_or_skip::<E>(row, E::sync_key(row), report);
        }
    }

    E::export_all()?;
    Ok(())
}
```

### 4.2 pull_entity

```rust
fn pull_entity<E: SyncEntity>(key: &str, report: &mut MergeReport, _now: i64) -> Result<()> {
    match E::read_file(key) {
        Ok(file) => {
            let row = E::file_to_row(&file);
            match E::upsert_db_from_file(&row) {
                Ok(true) => report.pulled += 1,
                Ok(false) => report.skipped += 1,
                Err(e) => log_warn_skip::<E>(key, "pull upsert", e, report),
            }
        }
        Err(e) => log_warn_skip::<E>(key, "read file", e, report),
    }
    Ok(())
}
```

### 4.3 run_sync_pipeline

```rust
fn run_sync_pipeline<E: SyncEntity>() -> Result<MergeReport> {
    let now = now_secs();
    if E::tombstone_retention_secs() > 0 {
        let purged = E::purge_expired_tombstones(now)?;
        if purged > 0 { log::info!("[sync] {} GC: purged {} tombstones", E::LABEL, purged); }
    }
    let mut report = MergeReport::default();
    merge_three_way::<E>(&mut report, now)?;
    log::info!("[sync] {} merge done: pulled={} pushed={} conflicts={} skipped={}",
        E::LABEL, report.pulled, report.pushed, report.conflicts, report.skipped);
    Ok(report)
}
```

> **注**：git pull/push 在外层编排中调用（各模块的 sync_now 命令），不在 trait 内——因为各仓库的 git 操作路径不同（.sync/vault vs .sync/hotword vs .sync/clipboard）。

---

## 5. 各模块外层编排

### 5.1 vault

```rust
pub(crate) fn merge_vault() -> Result<MergeReport, SyncError> {
    // vault 特有：stamp 校验 + meta.json（保留不变）
    ... // 阶段 A 不变

    let mut report = MergeReport::default();
    // FK 约束：folder 先 cipher 后
    octopus_sync::pipeline::merge_three_way::<VaultFolderEntity>(&mut report, now_secs())?;
    octopus_sync::pipeline::merge_three_way::<VaultCipherEntity>(&mut report, now_secs())?;

    // vault 特有：meta upsert（阶段 D 不变）
    ... // 阶段 D 不变
    Ok(report)
}
```

### 5.2 hotword

```rust
pub fn merge_hotwords() -> Result<HotwordMergeReport> {
    let now = now_secs();
    let mut report = MergeReport::default();

    // 阶段 1：set 层 merge
    octopus_sync::pipeline::merge_three_way::<HotwordSetEntity>(&mut report, now)?;

    // 阶段 2：word 层 merge（对每个 DB 存在的 set）
    let latest_sets = db::list_all_hotword_sets()?;
    for set in &latest_sets {
        // HotwordWordEntity 的 outline 读 word（需 set_id，通过 thread-local 或参数传入）
        octopus_sync::pipeline::merge_three_way::<HotwordWordEntity>(&mut report, now)?;
    }

    // 阶段 3：export（已在 merge_three_way 内调 E::export_all）
    Ok(report.into())
}
```

### 5.3 clipboard

```rust
pub fn merge_clipboard_favorites() -> Result<ClipboardMergeReport> {
    let key = load_or_create_clipboard_key()?;  // clipboard 特有：key 加载
    // ClipboardKey 通过 thread-local 传给 trait impl
    set_clipboard_key(key);

    let mut report = MergeReport::default();
    octopus_sync::pipeline::merge_three_way::<ClipboardFavoriteEntity>(&mut report, now_secs())?;
    Ok(report.into())
}
```

---

## 6. vault is_deleted: bool → i64 迁移

### 6.1 schema 迁移（v59→v60）

```sql
-- v59→v60: vault is_deleted bool→i64（0=活跃，>0=删除时刻 epoch）
UPDATE vault_ciphers SET is_deleted = CASE WHEN is_deleted = 1 THEN strftime('%s','now') ELSE 0 END;
UPDATE vault_folders SET is_deleted = CASE WHEN is_deleted = 1 THEN strftime('%s','now') ELSE 0 END;
```

### 6.2 代码变更

| 文件 | 变更 |
|---|---|
| `infra/src/db/vault.rs` | `VaultCipher.is_deleted` / `VaultFolder.is_deleted`: `bool` → `i64` |
| `infra/resources/sql/schema.sql` | 列已是 INTEGER（DML 类型不变，Rust 类型变） |
| `vault/src/sync/store.rs` | `CipherFile.is_deleted` / `FolderFile.is_deleted`: `bool` → `i64` |
| `vault/src/sync/engine.rs` | 所有 `is_deleted == true/false` → `is_deleted > 0` |
| `vault/src/sync/fingerprint.rs` | md5 拼接 `is_deleted as u8` → `is_deleted`（i64 直接拼） |
| `vault/src/storage/` | 所有读 is_deleted 的地方 |
| `desktop/src/vault/` | vault 命令层 is_deleted 检查 |

### 6.3 .sync 文件兼容

旧 .sync 文件的 CipherFile `"isDeleted": true/false` 需兼容反序列化。serde 默认 bool→i64 会失败。
解决：CipherFile/FolderFile 加 `#[serde(deserialize_with = "bool_to_i64")]` 兼容旧格式。

---

## 7. GC 统一

### 7.1 vault/clipboard 新增 GC

vault 和 clipboard 的 tombstone_retention_secs 从 0 改为建议值（如 30 天 = 2592000 秒）。
实现 `purge_expired_tombstones`——仿 hotword 的 `purge_expired_*_at` 范式：
- 硬删超期 tombstone DB 行
- 删对应 .sync 文件
- scheduler 注册 GC 定时任务

### 7.2 GC 阈值

| 模块 | retention_secs | 备注 |
|---|---:|---|
| hotword | 864000（10 天） | 现有值不变 |
| vault | 2592000（30 天） | 新增（用户可能误删后需要恢复期） |
| clipboard | 2592000（30 天） | 新增 |

---

## 8. 不变量

1. **零行为变更**——3-way 判定顺序、tombstone 单向优先、拒绝复活守卫语义不变
2. **vault is_deleted 迁移**——旧数据（0/1）正确转为（0/epoch）
3. **.sync 文件兼容**——旧 JSON 的 bool isDeleted 能被新代码反序列化
4. **GC 不丢活跃数据**——只删超期 tombstone（is_deleted>0 且超期），活跃条目（is_deleted=0）永不被 GC
5. **所有现有测试全过**——vault 262 + sync 145 + clipboard 24 + 各模块测试

---

## 9. 风险与缓解

| 风险 | 概率 | 影响 | 缓解 |
|---|---|---|---|
| vault schema 迁移破坏数据 | 中 | 密码库丢失 | 迁移前备份 DB；迁移 SQL 先跑测试；false positive 用 epoch |
| .sync bool→i64 反序列化失败 | 高 | 旧 sync 数据无法 merge | serde deserialize_with 兼容 bool |
| 泛型 trait 编译复杂 | 低 | 编译失败 | 逐模块迁移，每步编译验证 |
| merge 行为微妙变化 | 中 | 数据不一致 | TDD：迁移前后对同一 outline 做 merge，对比 report |
| GC 误删活跃数据 | 低 | 数据丢失 | GC 只删 is_deleted>0 且超期；活跃数据永不被 GC |

---

## 10. 实施路线

### 阶段 1：基础设施
- 新建 `sync/src/pipeline.rs`：trait SyncEntity + merge_three_way + pull_entity + run_sync_pipeline + MergeReport
- 共享类型 `MergeReport`（统一 vault/hotword/clipboard 的 report 结构）

### 阶段 2：vault is_deleted bool→i64
- schema v59→v60 迁移 SQL
- vault DB/File 类型变更 + 所有 `== true` → `> 0`
- .sync serde 兼容旧 bool 格式
- vault 测试全过

### 阶段 3：clipboard impl + GC
- impl SyncEntity for ClipboardFavorite（含 AES-GCM key 处理）
- clipboard purge_expired_tombstones 实现
- clipboard.rs merge 改调 merge_three_way
- clipboard 测试全过

### 阶段 4：hotword impl
- impl SyncEntity for HotwordSet + HotwordWord（含双层 outline + word key 派生）
- hotword GC 保留现有（已完整）
- hotword.rs merge 改调 merge_three_way
- hotword 测试全过

### 阶段 5：vault impl + GC
- impl SyncEntity for VaultCipher + VaultFolder
- vault GC 实现（新能力）
- vault engine.rs merge_vault 改调 merge_three_way
- vault 测试全过

### 阶段 6：清理
- 删除各模块的旧 merge 主循环代码（被 merge_three_way 取代）
- 删除各模块的 pull report 累加内联代码（被 pull_entity 取代）
- 全量 workspace 测试

---

## 11. 成功标准

1. `cargo build --workspace`：0 error 0 warning
2. `cargo test --workspace`：全过
3. trait SyncEntity 定义 + 5 个 impl（VaultCipher/Folder + HotwordSet/Word + ClipboardFavorite）
4. merge_three_way 泛型函数（~80 行）取代 6 段重复 merge 主循环（~430 行）
5. vault is_deleted 统一为 i64（schema v60）
6. vault/clipboard 获得 GC 能力（超期 tombstone 清理）
7. 现有 .sync 文件兼容（旧 bool 格式可反序列化）
