# casing 统一 Task 6: vault DTO — 设计规格（spec）

> **Status: ✅ 已实现**（2026-07-27，分支 `feat/record-followup`）。
>
> **本 spec 范围**：vault 域所有命令返回 DTO 加 `#[serde(rename_all = "camelCase")]`，前端 vault 页面同步改 camelCase。含 vault_commands.rs 9 个 DTO + vault crate 6 个 struct。
>
> **关联文档**：AGENTS.md「序列化 casing 规范」+ Task 1-5 已完成。

---

## 实现注记（Implementation Notes）

### 2026-07-27 实施完成

| 检查项 | 结果 |
|---|---|
| A1 cargo build | 0 error 0 warning |
| A2 cargo test | 全套无回归（无 FAILED） |
| A3 **pnpm build**（含 tsc -b） | 0 error |
| A4 前端 vault 域 snake_case 残留 | 0（VaultPanel 2 处注释 `is_user_vault_unlocked` 是后端逻辑描述非字段访问，保留） |
| A5 后端 15 struct rename_all | vault_commands.rs 9 + vault crate 6（folder/health/importer/duplicate/sync engine.rs） |

**关键安全性确认**：
- `FolderDto` 加 rename_all 不破坏 export JSON（export 用独立 BitwardenFolder struct 转换，不直接序列化 FolderDto）
- `SyncStatus`/`SyncReport`/`HealthReport`/`ImportReport`/`DuplicateGroup` 不持久化（grep 无 to_string/from_str，即时计算）
- 所有 DTO 只在 vault_commands.rs / vault_sync_commands.rs 用（vault crate 内部有独立 Cipher/LoginData 类型，与 DTO 解耦）

---

## 0. 改动清单

### 后端 vault_commands.rs（9 struct）

| struct | 行 | snake_case 字段 |
|---|---|---|
| `VaultStatusDto` | 27 | `user_vault_unlocked` |
| `LoginUriDto` | 33 | `match_type` |
| `LoginDataDto` | 39 | （无） |
| `FieldDto` | 47 | `field_type` |
| `CipherDto` | 54 | `folder_id` / `deleted_at` / `created_at` / `updated_at` |
| `CipherInputDto` | 70 | `folder_id`（Deserialize 入参，也加 rename_all） |
| `TotpResultDto` | 81 | `seconds_remaining` |
| `PasswordStrengthDto` | 609 | `entropy_bits` |
| `AutoTypeResultDto` | 709 | `fallback_to_clipboard` |

### 后端 vault crate（6 struct）

| struct | 文件:行 | snake_case 字段 | 持久化风险 |
|---|---|---|---|
| `FolderDto` | `crates/vault/src/storage/folder.rs:20` | `sort_order` / `created_at` / `updated_at` | 无（从 VaultFolder DB 行转换，不直接序列化 FolderDto 到文件；export 用 BitwardenFolder 独立 struct） |
| `HealthReport` | `crates/vault/src/health/mod.rs:11` | 待查 | 无（即时计算） |
| `ImportReport` | `crates/vault/src/importer/bitwarden.rs:112` | 待查 | 无（即时计算） |
| `DuplicateGroup` | `crates/vault/src/health/duplicate.rs:15` | 待查 | 无（即时计算） |
| `SyncStatus` | `crates/vault/src/sync/engine.rs:92` | 待查 | 无（即时计算，grep 无 to_string/from_str） |
| `SyncReport` | `crates/vault/src/sync/engine.rs:625` | 待查 | 无（即时计算） |

### 前端（5 文件）

| 文件 | snake_case 字段访问数 |
|---|---|
| `pages/Settings/Vault/CipherEditor.tsx` | 12（match_type/field_type/folder_id/seconds_remaining/entropy_bits/deleted_at） |
| `pages/Settings/Vault/CipherList.tsx` | 19（match_type/field_type/folder_id/deleted_at/created_at） |
| `pages/Settings/Vault/folderTypes.ts` | 4（created_at/updated_at/folder_id） |
| `pages/Settings/Vault/ChangePasswordModal.tsx` | 1 |
| `pages/VaultPicker/index.tsx` | 1 |
| `pages/Settings/VaultPanel.tsx` | VaultStatusDto 的 `user_vault_unlocked` |

## 1. 验收

| # | 检查项 | 通过标准 |
|---|---|---|
| A1 | cargo build | 0 error 0 warning |
| A2 | cargo test | 全套无回归 |
| A3 | **pnpm build**（含 tsc -b） | 0 error |
| A4 | grep 前端 vault 域 snake_case 残留 | 0 |
| A5 | 后端 15 struct 都有 rename_all | 15 |
