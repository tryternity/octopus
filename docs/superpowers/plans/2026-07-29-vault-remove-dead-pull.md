# 删除 vault pull_from_files 死代码 plan

> **Status: ✅ 已完成**（2026-07-29，分支 `daily_bugfix_0729`）
>
> **Spec**: [`2026-07-29-vault-remove-dead-pull.md`](../specs/2026-07-29-vault-remove-dead-pull.md)

## Phase A：迁移 11 个测试到 merge_vault

**Files:** `crates/vault/src/sync/engine.rs`

每个测试：改 `pull_from_files()` → `merge_vault()`，适配 `MergeReport` 返回类型。

- [x] **Task A.1: stamp 校验（2 个）** — pull_rejects_mismatched_security_stamp / pull_allows_matching_security_stamp
- [x] **Task A.2: meta 缺失（2 个）** — pull_rejects_when_local_has_vault_but_remote_meta_missing / pull_allows_when_both_local_and_remote_meta_missing
- [x] **Task A.3: weak KDF（1 个）** — pull_rejects_weak_kdf_params（2 处调用，同一测试函数）
- [x] **Task A.4: 软删（1 个，安全关键）** — pull_preserves_soft_deleted_at
- [x] **Task A.5: 容错（2 个）** — pull_skips_corrupted_cipher_file / pull_captures_folder_rename
- [x] **Task A.6: 其他（3 个）** — sync_recovers_data_when_db_emptied / pull_clears_local_enc_when_sync_enc_differs / weak KDF 重复
- [x] **Task A.7: cargo test -p octopus-vault 全过**

## Phase B：删 pull_from_files 死代码

- [x] **Task B.1: 删 pull_from_files 函数体（179 行）**
- [x] **Task B.2: 修引用 pull_from_files 的注释**（多处"沿用 pull_from_files 模式"改为自述）
- [x] **Task B.3: cargo check + cargo test 确认无残留引用**

## Phase C：全量验证 + 文档同步

- [x] **cargo test 全 workspace 0 failed**
- [x] **更新 architecture.md vault sync 段**
- [x] **review plan**
