# Vault Sync 空库恢复——需输源机密码确认

- **Status:** ✅ 已实现（2026-07-28，方案迭代 v2）
- **Date:** 2026-07-28
- **关联：** `docs/superpowers/specs/archived/2026-07-27-vault-sync-is-deleted-merge.md`（merge_vault 设计）；`docs/superpowers/specs/archived/2026-07-18-password-vault-design.md`（vault 密码学模型）

## 实现注记（2026-07-28）

### v1（已废弃）：无条件跳过 stamp 校验

初版方案是「本地空库 + stamp 不一致 → 跳过 stamp 校验，直接用远程 meta 覆盖本地」。问题：**不校验主密码**，如果用户清库重建时输了不同主密码，会进入「数据恢复了但解不开」的死状态（meta 被远程覆盖，但用户密码解不开 remote protected_user_vault_key）。

已实现的 v1 代码（空库旁路直接放行）**将被 v2 替换**。

### v2（进行中）：返回 `EmptyRecoveryNeedsPassword` 错误 + 前端弹窗输源机密码

用户实测发现：清库重建时如果输错主密码（与源机器不同），同步「成功」但 cipher 解密失败。v2 要求**密码校验**——复用现有 `resolve_with_remote` 函数（已含密码校验 + meta 覆盖）。

| Phase | 完成 |
|---|---|
| 1 新增 `SyncError::EmptyRecoveryNeedsPassword` 变体 | ✅ Display = "本地空库恢复需确认源机器主密码"（含「主密码」关键字，复用前端 UI） |
| 2 merge_vault + pull_from_files 空库旁路改为返回该错误 | ✅ |
| 3 前端 SyncPanel 复用现有密码 UI | ✅ 零改动（现有 `includes("主密码")` 自动匹配新 Display） |
| 4 TDD：修改旧测试 | ✅ `merge_vault_recovers_when_local_empty_and_stamp_differs` 从 expect 成功改为 expect `EmptyRecoveryNeedsPassword` |
| 5 全量回归 | ✅ sync 108 + vault 263 + desktop build 0 warning |

## 背景

清库重建 vault 后同步失败。根因：`setup_vault` 每次都 `Uuid::new_v4()` 生成新 `security_stamp`（`unlock.rs:94`），清库重建后本地 stamp 必然与 `.sync/meta.json` 的旧 stamp 不同。`merge_vault` 阶段 A 的 stamp 校验（`engine.rs:1267-1274`）检测到不一致直接返回 `SyncError::MasterPasswordMismatch`，即使主密码相同。

## 密码学约束（重要）

本次澄清了一个根本性的设计约束：**现架构中所有机器必须共享同一个主密码**。

原因：
- `app_key_sync_enc = master_root_key.encrypt(app_key)`（`unlock.rs:92`）
- `master_root_key = Argon2id(主密码, kdf_salt)`（`unlock.rs:77`）
- `kdf_salt` 跨机同步（`.sync/meta.json` 里的 `kdf_salt`，`clone_initial` 行 516 直接写入 B 机 DB）

所以 B 机要解开 `app_key_sync_enc`，必须用与 A 机**相同的主密码**（salt 是同一个）。`change_master_password`（行 318）也沿用旧 salt——改主密码后所有机器同步新 meta，所有机器都用新密码。

**用户决策（2026-07-28）**：接受这个约束，保持现架构（主密码全局共享），不重构为「AB 独立主密码」。

## 问题场景

```
用户清库（rm ~/.octopus/octopus.db*）→ 重启应用 → 新建 vault（输原主密码）
→ setup_vault 生成：
    - 新 kdf_salt_b（随机）
    - 新 stamp_b = Uuid::new_v4()
    - 新 master_root_key_b = Argon2id(原主密码, salt_b)
    - 新 app_key_b = master_root_key_b.child()
    - 新 app_key_sync_enc_b = master_root_key_b.encrypt(app_key_b)
→ vault_meta 表有 1 行（stamp_b + salt_b + app_key_sync_enc_b）
→ vault_ciphers / vault_folders 表 0 行（空）

用户点同步 → merge_vault：
  阶段 A：local.stamp_b != remote.stamp_a → 返回 MasterPasswordMismatch ❌
```

**正确行为**：本地空库 + `.sync` 有数据 + stamp 不一致 → 返回 `EmptyRecoveryNeedsPassword` 错误，前端弹窗「请输入源机器主密码」。用户输密码后调 `resolve_with_remote`（已存在，会校验密码 + 用远程 meta 覆盖本地），成功后重新 sync 走 merge 拉回 cipher。

## 设计（v2）

### 修复点 1：新增 `SyncError::EmptyRecoveryNeedsPassword`

`crates/sync/src/error.rs`：

```rust
pub enum SyncError {
    // ...
    MasterPasswordMismatch,
    /// 空库恢复场景：本地空库（cipher=0 + folder=0）+ 远程有数据 + stamp 不一致。
    /// 需要用户输源机器主密码确认（前端弹窗），调 resolve_with_remote 校验 + 覆盖本地。
    EmptyRecoveryNeedsPassword,
    // ...
}
```

`Display` impl：`SyncError::EmptyRecoveryNeedsPassword => write!(f, "本地空库恢复需确认源机器主密码")`

⚠️ Display 字符串需包含「主密码」关键字，复用前端现有的 `syncError.includes("主密码")` check（无需改前端逻辑分支）。

### 修复点 2：merge_vault + pull_from_files 空库旁路改为返回错误

`crates/vault/src/sync/engine.rs` merge_vault 阶段 A + pull_from_files 阶段 A：

```rust
// v1（无条件放行——已废弃）：
let local_empty = db_ciphers.is_empty() && db_folders.is_empty();
if local_empty {
    log::info!("[sync] 空库恢复场景，跳过 stamp 校验");
} else {
    return Err(SyncError::MasterPasswordMismatch);
}

// v2（返回错误，要求密码确认）：
let local_empty = db_ciphers.is_empty() && db_folders.is_empty();
if local_empty {
    log::info!("[sync] 空库恢复场景——返回 EmptyRecoveryNeedsPassword，等待用户输源机密码");
    return Err(SyncError::EmptyRecoveryNeedsPassword);
} else {
    return Err(SyncError::MasterPasswordMismatch);
}
```

### 修复点 3：前端复用现有密码输入 UI

`crates/desktop/frontend/src/pages/Settings/Vault/SyncPanel.tsx` 现有的冲突解决 UI（`syncError.includes("主密码")`）已经包含：
- 「以远程为准」/「以本地为准」两个按钮
- 密码输入框
- 调 `vault_sync_resolve_remote` / `vault_sync_resolve_local`

由于新错误的 Display 字符串包含「主密码」，现有 substring check 自动匹配，**前端零改动**（或仅调整文案让「空库恢复」场景的提示更准确）。

用户流程：
1. 同步 → 失败，toast「本地空库恢复需确认源机器主密码」
2. 冲突解决 UI 出现 → 用户点「以远程为准」
3. 输源机器主密码 → 调 `vault_sync_resolve_remote`
4. `resolve_with_remote` 用远程 KDF + 用户密码派生 master_root_key，解 protected_user_vault_key 校验
   - 密码对 → 用远程 meta 覆盖本地 → 重新 sync → merge 拉回 cipher → 成功
   - 密码错 → 返回「密码错误」，用户重试

### 不改的地方

- `resolve_with_remote` / `resolve_with_local` 函数本身（已完整，含密码校验 + meta 覆盖）
- `vault_sync_resolve_remote` / `vault_sync_resolve_local` Tauri 命令（已注册）
- 密码学模型（主密码全局共享）

## 判定逻辑总结

| 场景 | local cipher/folder | stamp 一致？ | 行为 |
|---|---|---|---|
| 正常同步 | 有数据 | 一致 | merge 正常 |
| 主密码真不同 | 有数据 | 不一致 | `MasterPasswordMismatch`（保护数据，前端冲突 UI） |
| **清库恢复（v2）** | **空** | **不一致** | **`EmptyRecoveryNeedsPassword`（前端弹窗输源机密码 → resolve_with_remote）** |
| 首次同步（B 机 clone_initial） | 空 | local_meta=None | 本就不进 stamp 校验（行 1269 `if let Some` 跳过） |

## 验收标准

| # | 检查项 | 通过标准 |
|---|---|---|
| A1 | 清库恢复（主密码一致） | 清库 → 新建 vault（原主密码）→ sync → `EmptyRecoveryNeedsPassword` → 弹窗输原主密码 → resolve_with_remote 成功 → 重新 sync → cipher 恢复 → 解锁解密成功 |
| A2 | 清库恢复（主密码不一致） | 清库 → 新建 vault（不同主密码）→ sync → `EmptyRecoveryNeedsPassword` → 弹窗输错密码 → resolve_with_remote 返回「密码错误」→ 用户重试或确认主密码 |
| A3 | 真冲突仍拦截 | 本地有 cipher + stamp 不一致 → 仍返回 `MasterPasswordMismatch`（不破坏现有保护） |
| A4 | 回归 | 现有 `pull_rejects_mismatched_security_stamp`（非空库 + stamp 不一致）仍 pass |

## 风险

| 风险 | 缓解 |
|---|---|
| 用户看不懂「源机器主密码」 | 前端文案明确：「请输入原 vault 的主密码（与同步仓库加密时使用的一致）」 |
| Display 字符串匹配冲突 | `EmptyRecoveryNeedsPassword` 的 Display 包含「主密码」，与 `MasterPasswordMismatch` 共用前端 UI——意图一致（都是要求输密码），无冲突 |

## 用户操作流程（v2 修复后）

```
1. rm ~/.octopus/octopus.db*（清库，含 machine-key.enc）
2. 重启应用 → 新建 vault（输任意主密码 P_local）
3. 设置 → 密码箱 → 同步 → 点「立即同步」
4. merge_vault 检测空库 + stamp 不一致 → 返回 EmptyRecoveryNeedsPassword
5. 前端 toast + 冲突解决 UI 出现（因 Display 含「主密码」）
6. 用户点「以远程为准」→ 输源机器原主密码 P_remote
7. resolve_with_remote 校验：
   - P_remote == 源主密码 → 用远程 meta 覆盖本地 → 重新 sync → cipher 恢复 → 成功
   - P_remote != 源主密码 → 「密码错误」，用户重试
8. 解锁输 P_remote（现在本地主密码 = 源主密码）→ 解密成功
```
```
