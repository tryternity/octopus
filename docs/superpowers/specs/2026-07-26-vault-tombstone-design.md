# Vault Tombstone 设计——跨设备永久删除一致性

**日期**：2026-07-26
**状态**：设计阶段（待 plan + 实现）
**类型**：功能增强（sync 协议 + 数据模型）
**关联**：
- [vault-git-sync-design](./2026-07-21-vault-git-sync-design.md) §2.4——sync 增量同步原设计
- [vault-security-hardening](./2026-07-24-vault-security-hardening.md) M-TOMBSTONE（M5，已知高优先级未修项）
- [vault-git-sync plan](../plans/archived/2026-07-21-vault-git-sync.md)——sync 实施计划

## 背景：跨设备硬删复活 bug

### 复活链（已验证）

当前 vault sync 的 pull 路径（`engine.rs::sync_now` → pull 阶段）逻辑：

```rust
// engine.rs:877-895 pull 阶段
for (uuid, entry) in &remote_outline.ciphers {
    let needs_update = !db_cipher_md5.contains_key(uuid.as_str())
        || cipher_md5_mismatch(uuid, &entry.md5, &db_cipher_md5);
    if needs_update {
        // 读远程 cipher 文件 → upsert 到本地 SQLite
        let cipher_file = store::read_cipher_file(uuid)?;
        let input = build_cipher_input_from_file(&row);
        upsert_cipher(&input)?;  // ← 复活点
    }
}
```

**复活场景**：
1. 设备 A 有 cipher X，设备 B 也有（已 sync 过）。
2. 设备 A 执行 `permanent_delete(X)`——SQLite 行删除 + cipher 文件删除 + outline 移除 X 条目。
3. 设备 A `sync_now` push——远程 outline 移除 X，远程 cipher 文件删除。
4. **但设备 B 还没 sync**——B 的 SQLite 仍有 X，B 的 outline 仍有 X。
5. 设备 B `sync_now` pull——拉到远程 outline（无 X）→ pull 阶段不处理 X（远程无）。
6. 设备 B `sync_now` push 阶段——B 的 outline 有 X → `incremental_export` 把 X 写回远程 → **X 复活到远程**。
7. 设备 A 下次 `sync_now` pull → 远程又有 X → upsert → **X 在 A 上复活**。

### 根因

sync 协议**只传递"存在"状态**（outline.ciphers 列出存在的 uuid），**不传递"已删除"状态**。永久删除后，设备无法告诉其他设备"这个 uuid 已被永久删除，别再同步回来"。

软删（soft_delete）不复活——因为软删只设 `deleted_at`，cipher 行 + 文件 + outline 条目都还在，sync 正常传播 `deleted_at` 字段。**只有 permanent_delete（硬删）会复活**。

### 与 H2 修复的关系

H2（第二轮审查，已修）修的是**软删跨设备不同步**——之前 deleted_at 不在 sync_md5 里，软删状态不传播。H2 修复后软删正常同步。

但 H2 **没修 permanent_delete 复活**——permanent_delete 直接删行，连软删状态都没了，sync 无从知道这个 uuid 曾存在过。M-TOMBSTONE 是 H2 之后的下一层防御。

## 设计目标

### 必须达成

1. **permanent_delete 后跨设备不复活**：设备 A 永久删除 X 后，设备 B sync 时知道 X 已被永久删除，不会把 X push 回远程。
2. **墓碑有 TTL**：墓碑不能无限保留（否则 outline 无限膨胀）。超过 TTL 后墓碑可被清理，此时若设备 B 仍持有 X（很久没 sync），X 可能复活——这是可接受的退化（很久没 sync 的设备视为"新设备"，需手动处理冲突）。
3. **与现有软删语义不冲突**：soft_delete（回收站）保持不变，tombstone 只管 permanent_delete。

### 可以接受

4. **TTL 内未 sync 的设备复活**：如果设备 B 在 TTL 内从未 sync，墓碑可能已被清理，B 持有的 X 会复活。这是边界场景（用户长期不用某设备后又启用），通过文档提示用户"长期未 sync 的设备重新启用前先手动检查冲突"。
5. **需 outline 版本升级**：新增 tombstones 字段，outline version 从 1 升到 2。旧客户端读到 v2 outline 应报错（而非静默忽略 tombstones）。

### 不做

6. **不做 CRDT 式的删除赢逻辑**：不实现"删除 vs 修改"的自动冲突解决。如果 A 删了 X、B 改了 X（都未 sync），冲突时**删除赢**（安全优先——用户主动删的优先级高于被动同步的修改）。冲突通过日志 + UI 提示，不自动合并。
7. **不做文件夹 tombstone**：folder 是轻量结构（cipher 的归属容器），folder 被永久删除时 cipher 的 folder_id 被 FK SET NULL（回根目录），不丢数据。folder 复活危害低（顶多多一个空 folder），不引入 tombstone 复杂度。

## 数据模型

### Outline v2（新增 tombstones 字段）

```rust
// crates/sync/src/outline.rs

/// outline.json 完整结构（v2）。
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Outline {
    /// outline 格式版本（v2 = 含 tombstones；v1 = 旧格式，读时报错要求升级）。
    pub version: u32,
    pub vault_version: u64,
    pub ciphers: BTreeMap<String, OutlineEntry>,
    pub folders: BTreeMap<String, OutlineEntry>,
    /// v2 新增：已永久删除的 cipher uuid → 墓碑条目。
    ///
    /// pull 时，本地若持有墓碑中的 uuid（SQLite 仍有该行），需删除本地行 + 文件。
    /// push 时，本地墓碑合并到远程（取较新的 deleted_ms）。
    /// TTL 过期后（默认 30 天）墓碑可被清理。
    #[serde(default)]
    pub tombstones: BTreeMap<String, TombstoneEntry>,
}

/// 墓碑条目——uuid 永久删除的时间戳。
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq)]
pub struct TombstoneEntry {
    /// 永久删除时间——Unix 毫秒时间戳（i64）。
    pub deleted_ms: i64,
}
```

**设计要点**：

- **`#[serde(default)]`**：让旧 v2 outline（无 tombstones 字段）反序列化时不报错——但 version 字段会拒绝 v1。
- **`BTreeMap`**：与 ciphers/folders 一致，保证 JSON 序列化顺序稳定（git diff 干净）。
- **`TombstoneEntry` 只有 `deleted_ms`**：不存删除原因 / 删除者（简化协议；诊断信息在 log 里）。

### TTL 常量

```rust
// crates/vault/src/sync/store.rs

/// 墓碑保留时长——超过此时间的墓碑可被清理（gc）。
///
/// 30 天的权衡：
/// - 太短（如 7 天）：用户一周没用某设备，墓碑已清理，复活风险高。
/// - 太长（如 180 天）：outline 膨胀（每个墓碑约 80 字节 JSON，1000 个墓碑 = 80KB，
///   对 git 同步可接受，但 outline.json diff 噪音大）。
/// - 30 天：覆盖绝大多数用户的"周期性 sync"频率，且 outline 不会过度膨胀。
pub const TOMBSTONE_TTL_MS: i64 = 30 * 24 * 60 * 60 * 1000; // 30 天
```

## 同步协议改动

### permanent_delete 写墓碑

```rust
// crates/vault/src/storage/cipher.rs

pub fn permanent_delete(id: &str) -> Result<()> {
    // SYNC_LOCK 下沉（与 empty_trash 一致）——防 sync 并发复活
    let _sync_guard = crate::sync::engine::try_sync_lock()
        .map_err(|e| anyhow::anyhow!("{}", e))?;

    db::with_db(|conn| {
        let tx = conn.unchecked_transaction()?;
        // 1. 删除 SQLite 行
        tx.execute("DELETE FROM vault_ciphers WHERE id = ?1", params![id])?;
        // 2. 删除 cipher 文件（已在原逻辑）
        // 3. 写墓碑到 outline（新增）
        //    注意：outline 是文件，不在 DB 事务里——见下方"事务边界"讨论
        tx.commit()?;
        Ok(())
    })?;

    // 写墓碑（文件操作，在 DB 事务外）
    crate::sync::store::add_tombstone(id)?;
    Ok(())
}
```

### pull 阶段处理墓碑

```rust
// crates/vault/src/sync/engine.rs pull 阶段

// 阶段 B-0（新增）：应用远程墓碑——删除本地仍持有的已永久删除 cipher
for (uuid, tombstone) in &remote_outline.tombstones {
    if db::load_vault_cipher(uuid)?.is_some() {
        // 本地仍持有已永久删除的 cipher → 删除（不进回收站，直接硬删——
        // 远程已确认永久删除，本地跟随）
        db::permanent_delete_vault_cipher(uuid)?;
        let _ = store::delete_cipher_file(uuid);
        log::info!("[sync] 墓碑应用：本地 cipher {} 已按远程永久删除", uuid);
    }
    // 合并墓碑到本地 outline（取较新的 deleted_ms）
    local_outline.tombstones.insert(uuid.clone(), tombstone.clone());
}
```

### push 阶段合并墓碑

```rust
// crates/vault/src/sync/store.rs incremental_export

// 合并旧 outline 的墓碑（保留未过期的）
for (uuid, tombstone) in &old_outline.tombstones {
    let now_ms = sync_store::iso_to_unix_ms(&chrono::Utc::now().to_rfc3339());
    if now_ms - tombstone.deleted_ms < TOMBSTONE_TTL_MS {
        new_outline.tombstones.insert(uuid.clone(), tombstone.clone());
    }
    // 过期的墓碑不插入 → 自然清理（gc）
}
```

## 冲突解决：删除 vs 修改

**场景**：设备 A 永久删除 X（写墓碑），设备 B 修改 X（更新 cipher 内容）。两者都未 sync。下次 sync 时：

- 远程 outline 有 X 的 cipher 条目（B push 的修改）+ X 的墓碑（A push 的删除）。
- **同时存在 cipher 条目和墓碑是矛盾状态**。

**解决策略：删除赢（delete-wins）**

```rust
// pull 阶段：墓碑优先级高于 cipher 条目
for (uuid, tombstone) in &remote_outline.tombstones {
    if remote_outline.ciphers.contains_key(uuid) {
        // 矛盾状态——删除赢。删除 cipher 条目，保留墓碑。
        log::warn!(
            "[sync] 冲突：cipher {} 既有修改又有墓碑，删除赢（永久删除优先）",
            uuid
        );
        // 从本地 outline 删除 cipher 条目，删除本地 SQLite 行 + 文件
    }
}
```

**为什么删除赢**：
- 用户主动 permanent_delete 是明确意图，优先级高于被动 sync 的修改。
- 安全优先——删除敏感数据比保留更安全（万一用户删除的是泄露的凭证）。
- 冲突通过 log + UI 提示，用户可手动恢复（从备份/Bitwarden 导出重新导入）。

## 事务边界问题

**已知限制**（设计阶段接受，不修）：

`permanent_delete` 涉及两个存储：
- SQLite（vault_ciphers 行）——事务性
- 文件系统（cipher 文件 + outline.json）——非事务性

如果 SQLite 删成功、文件系统写墓碑失败，会出现"SQLite 无 X 但 outline 无墓碑"的不一致状态——此时 X 不会复活（SQLite 无 X，push 时 incremental_export 会删远程 X 文件），但墓碑未传播，其他设备仍可能 push X 回来。

**缓解措施**：
- `add_tombstone` 失败时 log::error，但**不回滚 SQLite 删除**（用户意图是删除，文件系统失败不应让数据复活）。
- 下次 sync 时，本地 outline 的 cipher 条目已无（incremental_export 删了），远程若有 X，pull 会 upsert X 回来——**这是无墓碑的退化场景**，需要用户手动再删一次。

**彻底修复需文件系统事务**（如 write-ahead log + rename），属 Phase 3，不在本 spec 范围。

## Outline 版本升级

### v1 → v2 迁移

```rust
// crates/sync/src/outline.rs

pub fn read_outline_file() -> Result<Outline> {
    let text = std::fs::read_to_string(outline_path())?;
    let outline: Outline = serde_json::from_str(&text)?;
    if outline.version < 2 {
        // v1 outline——无 tombstones 字段。就地升级：
        // 1. version 改 2
        // 2. tombstones 默认空（无历史墓碑信息——已永久删除的 cipher 无法追溯）
        log::warn!("[sync] outline v{} → v2 升级（无历史墓碑，已删 cipher 无法追溯）", outline.version);
    }
    Ok(outline)
}
```

**关键决策**：v1 → v2 不报错（让现有用户平滑升级），但 log 警告"已删 cipher 无法追溯"——即升级前已 permanent_delete 的 cipher，墓碑无法追溯，若其他设备仍持有，可能复活。用户需手动检查。

### 旧客户端兼容

旧客户端（只认 v1）读到 v2 outline 时，`serde` 反序列化会**忽略未知字段**（tombstones）——但旧客户端的 pull 逻辑不会处理墓碑，会导致：
- 新客户端写的墓碑，旧客户端忽略 → 旧客户端仍持有已删 cipher → push 回去 → 复活。

**缓解**：要求所有设备升级到支持 v2 的版本后再使用 permanent_delete。文档提示用户"混合版本环境下，永久删除可能不稳定"。

## 测试策略

### 单元测试

- `add_tombstone` 写入 outline.tombstones。
- `read_outline_file` v1 → v2 升级路径。
- TTL gc：过期的墓碑被清理，未过期保留。
- 墓碑合并：本地 + 远程墓碑取较新 deleted_ms。

### 集成测试（关键）

```rust
#[test]
fn permanent_delete_does_not_resurrect_across_devices() {
    // 设备 A：创建 cipher X + sync（push 到远程）
    // 设备 B：clone（拉到 X）
    // 设备 A：permanent_delete(X) + sync（push 墓碑到远程）
    // 设备 B：sync（pull 墓碑 → 删除本地 X）
    // 断言：设备 B 的 SQLite 无 X，文件无 X
    // 设备 B：sync（push——不应把 X 推回远程）
    // 断言：远程 outline 无 X 的 cipher 条目
}

#[test]
fn delete_wins_on_conflict() {
    // 设备 A：永久删除 X（写墓碑）
    // 设备 B：修改 X（更新 cipher 内容）
    // 两设备 sync 后：X 应被永久删除（删除赢），log 有冲突警告
}

#[test]
fn expired_tombstone_gc() {
    // 写一个 31 天前的墓碑 → gc 后应被清理
    // 写一个 29 天前的墓碑 → gc 后保留
}
```

## 风险与权衡

### 收益

- **修复已知高优先级 bug**（M-TOMBSTONE）：跨设备永久删除不再复活。
- **协议语义完整**：sync 同时传递"存在"和"已删除"状态。

### 成本

- **outline 格式升级**（v1 → v2）：需迁移逻辑 + 版本检查。
- **permanent_delete 加 SYNC_LOCK**：与 empty_trash 一致，但 permanent_delete 路径变重（之前直接 DELETE）。
- **outline 膨胀**：墓碑占空间（每个 ~80 字节），TTL 30 天 + 高频删除场景下 outline.json 会增大。可接受（用户场景下 permanent_delete 是低频操作）。

### 不解决的问题

- **文件系统事务性**（见"事务边界问题"）：SQLite + 文件系统跨存储原子性未解决。
- **混合版本兼容**：旧客户端忽略墓碑会导致复活——需文档提示。

## 实施顺序

1. **sync crate**：Outline v2 结构 + TombstoneEntry + 版本升级逻辑。
2. **vault crate**：`permanent_delete` 写墓碑 + pull 处理墓碑 + push 合并墓碑 + TTL gc。
3. **集成测试**：跨设备复活测试 + 冲突测试 + TTL 测试。
4. **文档**：架构文档 + 用户提示（v1→v2 升级 + 混合版本风险）。

详见 plan：`docs/superpowers/plans/2026-07-26-vault-tombstone.md`。
