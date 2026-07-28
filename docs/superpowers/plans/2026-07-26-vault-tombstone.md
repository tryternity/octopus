> **⚠️ 已废弃（2026-07-28）**：对应的 spec 已废弃，本 plan 不再推进。
> 替代方案：[`2026-07-27-vault-sync-is-deleted-merge.md`](../specs/2026-07-27-vault-sync-is-deleted-merge.md)
> + [`2026-07-27-vault-sync-is-deleted-merge.md` plan](../plans/2026-07-27-vault-sync-is-deleted-merge.md)。
> 保留本文件作为历史记录。

# Vault Tombstone 实施计划

**日期**：2026-07-26
**关联 spec**：[vault-tombstone-design](../specs/2026-07-26-vault-tombstone-design.md)
**状态**：待实施

## 任务分解

### Task 1: sync crate Outline v2 + TombstoneEntry

**文件**：`crates/sync/src/outline.rs`

**变更点**：
1. 新增 `TombstoneEntry { deleted_ms: i64 }` 结构（Serialize/Deserialize/PartialEq）。
2. `Outline` 新增 `tombstones: BTreeMap<String, TombstoneEntry>` 字段（`#[serde(default)]`）。
3. `Outline::default()` 初始化 `tombstones: BTreeMap::new()`。
4. `read_outline_file` 加 v1 → v2 升级逻辑：version < 2 时 log warn + 设 version=2 + tombstones=空。
5. 更新所有 `Outline { ... }` 构造点（grep 全 crate）——加 `tombstones` 字段。

**验证命令**：
```bash
rg "Outline \{" crates/ -t rust  # 找全所有构造点
cargo build -p octopus-sync 2>&1 | tail -10
cargo test -p octopus-sync outline 2>&1 | tail -10
```

**新增测试**：
- `outline_v1_upgrades_to_v2`：序列化 v1 JSON（无 tombstones）→ 反序列化 → 断言 version=2 + tombstones 空。
- `outline_v2_roundtrip`：构造含 tombstones 的 outline → 序列化 → 反序列化 → 断言一致。

### Task 2: vault store 加墓碑读写 helper

**文件**：`crates/vault/src/sync/store.rs`

**变更点**：
1. 定义 `TOMBSTONE_TTL_MS: i64 = 30 * 24 * 60 * 60 * 1000`（30 天）。
2. 新增 `add_tombstone(uuid: &str) -> Result<()>`——读 outline → 插入/更新 tombstone(deleted_ms=now) → 写 outline。
3. 新增 `gc_tombstones(outline: &mut Outline) -> usize`——删除过期墓碑，返回清理数。
4. `incremental_export` 合并 old_outline.tombstones 到 new_outline（应用 TTL gc）。

**验证命令**：
```bash
cargo build -p octopus-vault 2>&1 | tail -10
cargo test -p octopus-vault tombstone 2>&1 | tail -10
```

**新增测试**：
- `add_tombstone_writes_to_outline`：add_tombstone 后 outline.tombstones 含该 uuid。
- `gc_tombstones_removes_expired`：31 天前的墓碑被清理，29 天前的保留。
- `incremental_export_preserves_unexpired_tombstones`。

### Task 3: permanent_delete 写墓碑 + SYNC_LOCK

**文件**：`crates/vault/src/storage/cipher.rs`

**变更点**：
1. `permanent_delete` 加 `try_sync_lock()`（与 empty_trash 一致）。
2. SQLite DELETE 成功后调 `crate::sync::store::add_tombstone(id)`。
3. add_tombstone 失败时 log::error 但不回滚（用户意图是删除）。

**验证命令**：
```bash
cargo test -p octopus-vault permanent_delete 2>&1 | tail -10
```

**新增测试**：
- `permanent_delete_writes_tombstone`：permanent_delete 后 outline.tombstones 含该 uuid。
- `permanent_delete_takes_sync_lock`：sync 进行中时 permanent_delete 返 Err。

### Task 4: pull 阶段应用墓碑

**文件**：`crates/vault/src/sync/engine.rs`（sync_now 的 pull 阶段）

**变更点**：
1. 在 cipher upsert 循环**之前**新增"阶段 B-0：应用远程墓碑"。
2. 遍历 remote_outline.tombstones：
   - 若本地 SQLite 仍有该 uuid → permanent_delete_vault_cipher + delete_cipher_file。
   - 若 remote_outline.ciphers 同时含该 uuid（矛盾）→ log warn "删除赢" + 删除 cipher 条目。
   - 合并墓碑到本地 outline（取较新 deleted_ms）。
3. 更新 `db_cipher_md5` HashMap——应用墓碑后从 map 移除已删 uuid（防后续 upsert 循环误处理）。

**验证命令**：
```bash
cargo test -p octopus-vault --lib sync 2>&1 | tail -10
```

**新增测试**：
- `pull_applies_remote_tombstone_deletes_local_cipher`。
- `pull_delete_wins_on_conflict`（cipher 修改 vs 墓碑）。

### Task 5: 集成测试——跨设备复活场景

**文件**：`crates/vault/tests/tombstone_resurrection.rs`（新建）

**测试场景**：
```rust
#[test]
fn permanent_delete_does_not_resurrect_across_devices() {
    // 用两份独立 .sync 目录模拟设备 A / B
    // A: 创建 X + push
    // B: clone（拉到 X）
    // A: permanent_delete(X) + push（写墓碑）
    // B: pull（应用墓碑 → 删本地 X）
    // 断言 B 的 SQLite 无 X
    // B: push（不应把 X 推回远程）
    // 断言远程 outline 无 X 的 cipher 条目
}

#[test]
fn delete_wins_on_conflict() {
    // A: 永久删除 X
    // B: 修改 X
    // sync 后 X 应被永久删除，log 有冲突警告
}
```

**注意事项**：
- 集成测试需两份独立 git repo + DB——可能需 test fixture helper。
- 看 `crates/vault/tests/unlock.rs` 的 IntegrationGuard 模式作参考。

**验证命令**：
```bash
cargo test -p octopus-vault --test tombstone_resurrection 2>&1 | tail -10
```

### Task 6: 文档同步

**文件**：
- `docs/superpowers/specs/2026-07-24-vault-security-hardening.md`——M-TOMBSTONE 章节状态从"follow-up 未修"改为"已修（Phase 2）"，链接到本 spec/plan。
- `docs/architecture.md`——vault sync 模块描述加 tombstone 机制 + outline v2 说明。
- `docs/superpowers/specs/2026-07-21-vault-git-sync-design.md`——§2.4 增量同步章节补充 tombstone 协议。

## 验收清单

- [ ] Task 1: Outline v2 结构 + 升级逻辑
- [ ] Task 2: 墓碑读写 helper + TTL gc
- [ ] Task 3: permanent_delete 写墓碑 + SYNC_LOCK
- [ ] Task 4: pull 应用墓碑 + 删除赢
- [ ] Task 5: 集成测试（跨设备复活 + 冲突）
- [ ] Task 6: 文档同步

## 风险点

- **outline v1 → v2 升级的边界**：升级前已 permanent_delete 的 cipher 无法追溯墓碑——文档需明确提示用户"升级后建议手动检查回收站 + 已删 cipher"。
- **文件系统事务性**（spec 已声明限制）：SQLite + 文件系统跨存储原子性未解决，add_tombstone 失败时 log error 不回滚。
- **hotword 模块的对称性**：hotword 也用 Outline 结构（vault_version 字段复用）——加 tombstones 字段后，hotword outline 也会有此字段（空）。需确认 hotword 路径不受影响（hotword 无 permanent_delete 概念，tombstones 始终空）。

## 实施顺序建议

1. 先做 Task 1（Outline v2）——其他都依赖此。
2. Task 2（store helper）——纯逻辑，可独立测。
3. Task 3 + Task 4 一起做（permanent_delete + pull 应用墓碑，逻辑闭环）。
4. Task 5（集成测试）——验证端到端。
5. Task 6（文档）——最后同步。

## 后续（非本 plan 范围）

- **文件系统事务性**（B-SETUP-CRASH-WINDOW 同型问题）：需 write-ahead log + rename 模式，Phase 3。
- **folder tombstone**：当前不做（folder 复活危害低），若未来需要可对称扩展。
- **墓碑 UI**：settings 里显示"已永久删除的条目（墓碑）"列表，让用户能看到/清理——产品功能，非本 plan。
