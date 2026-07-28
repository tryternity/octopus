# Vault Sync 空库恢复——实施计划

- **Spec:** `docs/superpowers/specs/2026-07-28-vault-sync-empty-recovery.md`
- **Goal:** `merge_vault` 识别「本地空库 + stamp 不一致」场景，跳过 stamp 校验，让 `.sync` meta 覆盖本地，实现清库重建后的数据恢复。

## Global Constraints

- **只改 stamp 校验逻辑**，不改密码学模型、不改 `setup_vault` 的 stamp 生成、不改 Tauri 命令层、不改前端。
- **判定条件**：`db_ciphers.is_empty() && db_folders.is_empty()`（复用 `merge_vault` 行 1246-1247 已加载的变量，零额外 DB 调用）。
- **TDD**：先写测试（模拟空库 + stamp 不一致 → merge 应成功），再改代码让测试通过。
- **回归**：现有 `pull_rejects_mismatched_security_stamp`（非空库 + stamp 不一致）必须仍 pass。

## File Structure

| 文件 | 改动 |
|---|---|
| `crates/vault/src/sync/engine.rs` | `merge_vault`（行 1267-1274）+ `pull_from_files`（行 903-910）stamp 校验加空库旁路；新增 1 个测试 |

## Phase 1: TDD——先写测试

### Task 1.1: 新增空库恢复测试

**Files:**
- Modify: `crates/vault/src/sync/engine.rs`（test module）

- [x] **Step 1: 写 `merge_vault_recovers_when_local_empty_and_stamp_differs` 测试**

模拟清库重建场景：
1. `IntegrationGuard::new()` 初始化（vault_meta 用 stamp `"stamp-test"`）
2. push 一条 cipher 到 `.sync`（这样远程有数据）
3. 清空本地 cipher + folder（`db::delete_all_vault_ciphers()` 或直接 DELETE SQL）
4. 把本地 stamp 改成不同的值（`db::update_vault_security_stamp("DIFFERENT-STAMP")`）——模拟 `setup_vault` 生成新 stamp
5. 调 `merge_vault()`
6. 断言：返回 `Ok`（不是 `MasterPasswordMismatch`）+ cipher 从 `.sync` 恢复到 DB

- [x] **Step 2: 跑测试，确认失败（stamp 校验挡住了）**

```bash
cargo test -p octopus-vault --lib merge_vault_recovers_when_local_empty_and_stamp_differs -- --nocapture
```

预期：`Err(MasterPasswordMismatch)`，测试失败。这是 TDD 的 RED 阶段。

## Phase 2: 实现——加空库旁路

### Task 2.1: merge_vault stamp 校验加旁路

**Files:**
- Modify: `crates/vault/src/sync/engine.rs`（`merge_vault` 行 1267-1274）

- [x] **Step 1: 改 merge_vault 的 stamp 校验**

把 `return Err(SyncError::MasterPasswordMismatch)` 改成先检查 `db_ciphers.is_empty() && db_folders.is_empty()`，空库则 log + 继续而非返回。

- [x] **Step 2: 跑 Phase 1 的测试，确认通过（GREEN）**

```bash
cargo test -p octopus-vault --lib merge_vault_recovers_when_local_empty_and_stamp_differs -- --nocapture
```

### Task 2.2: pull_from_files 对称加旁路（保持一致）

**Files:**
- Modify: `crates/vault/src/sync/engine.rs`（`pull_from_files` 行 903-910）

- [x] **Step 1: 同样的空库旁路逻辑加到 pull_from_files**

理由：虽然 prod 不调 pull_from_files，但保持两条路径行为对称，便于未来若重新启用。且现有测试 `pull_rejects_mismatched_security_stamp` 是非空库场景，不受影响。

- [x] **Step 2: 跑回归测试**

```bash
cargo test -p octopus-vault --lib -- --nocapture
```

确认：
- `merge_vault_recovers_when_local_empty_and_stamp_differs` PASS（新测试）
- `pull_rejects_mismatched_security_stamp` PASS（非空库仍拦截）
- `sync_recovers_data_when_db_emptied` PASS（这个保留相同 stamp，不走旁路也 OK）
- 其他 merge_vault 测试全 PASS

## Phase 3: 全量回归 + commit

- [x] **Step 1: 全量 cargo test**

```bash
cargo test -p octopus-vault --lib
cargo test -p octopus-infra --lib
cargo test -p octopus-sync --lib
```

- [x] **Step 2: cargo build（确认 0 warning）**

```bash
cargo build -p octopus-vault
cargo build -p octopus-desktop --features embedded,vault
```

- [x] **Step 3: commit**

## Phase 4: 文档同步

- [x] **Step 1: 更新 spec 状态** 🔜 → ✅

`docs/superpowers/specs/2026-07-28-vault-sync-empty-recovery.md` 顶部 Status 改 ✅，加实现注记。

- [x] **Step 2: 更新 vault-sync-is-deleted-merge spec**

在「实现注记」加一条：空库恢复场景已支持（2026-07-28）。

- [x] **Step 3: 更新 architecture.md**

vault sync 章节（如有）补一句：空库 + stamp 不一致 → 跳过校验恢复。

## Phase 5: E2E（用户验证）

- [ ] **用户清库重建后同步验证**

用户执行：
1. `rm ~/.octopus/octopus.db*`
2. 重启 → 新建 vault（原主密码）
3. 设置 → 密码箱 → 同步 → 立即同步
4. 期望：同步成功 + cipher 恢复 + 解锁后解密成功
