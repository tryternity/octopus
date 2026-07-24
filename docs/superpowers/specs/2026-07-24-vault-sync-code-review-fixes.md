# Vault Sync 代码审查报告修复

**日期**：2026-07-24
**状态**：已实现并测试通过
**关联**：[vault-git-sync-design](./2026-07-21-vault-git-sync-design.md)（Phase 1 同步模块）

## 背景

用户在 session 末尾贴了一份 vault/sync 代码审查报告，含 14 个问题（4 致命 + 6 高 + 4 中 + 若干低）。本 spec 记录每个问题的复查结论 + 修复决策。

**核心原则**：不轻信报告，逐条回源码核实。问题成立才修，不成立给出反馈。

---

## 复查结论总表

| # | 严重度 | 报告描述 | 复查结论 | 状态 |
|---|--------|---------|---------|------|
| 1 | 🔴 致命 | MatchType 枚举值与 Bitwarden 协议相反 | **❌ 误报** | 反馈 |
| 2 | 🔴 致命 | sync pull 忽略 outline md5，用 updated_at 比较 | ✅ 成立 | 已修 |
| 3 | 🔴 致命 | stamp 校验在 cipher/folder upsert 之后 | ✅ 成立 | 已修 |
| 4 | 🔴 致命 | sync_now 吞掉 push 失败，谎报「已推送」 | ✅ 成立 | 已修 |
| 5 | 🟠 高 | folder 只在 DB 不存在时 pull，rename 丢失 | ✅ 成立 | 已修 |
| 6 | 🟠 高 | folder sort_order 硬编码 0 | ✅ 成立 | 已修 |
| 7 | 🟠 高 | enable/disable/clone/resolve 不取 SYNC_LOCK | ✅ 成立 | 已修 |
| 8 | 🟠 高 | generator 明文密码未 Zeroizing | ✅ 成立 | 已修 |
| 9 | 🟠 高 | MetaFile salt base64 解码失败静默返空 | ✅ 成立 | 已修 |
| 10 | 🟠 高 | pull 静默吞 cipher 文件读取失败 | ✅ 成立 | 已修 |
| 11 | 🟡 中 | SyncError Display 透传 git stderr | ✅ 成立 | 已修 |
| 12 | 🟡 中 | vault_update_cipher 对不存在 id 静默成功 | ⏸ 未修（follow-up） | 待定 |
| 13 | 🟡 中 | machine-key.enc 注释夸大为「真加密」 | ✅ 成立（仅注释） | 已修 |
| 14 | 🟡 中 | Argon2Params i64→u32 无完整性校验 | ✅ 成立 | 已修 |

---

## #1 MatchType —— 误报（不成立）

**报告声称**：`Exact = 2` 应是 `StartsWith`，`StartsWith = 3` 应是 `Exact`，与 Bitwarden `UriMatchType.cs` 互换。

**复查证据**：

当前代码（`types.rs:65-72`）：
```rust
pub enum MatchType {
    Domain = 0,
    Host = 1,
    Exact = 2,
    StartsWith = 3,
    RegularExpression = 4,
    Never = 5,
}
```

Bitwarden 官方协议（[UriMatchType.cs](https://github.com/bitwarden/server/blob/main/src/Core/Enums/UriMatchType.cs)）：
```csharp
public enum UriMatchType : byte {
    Domain = 0,
    Host = 1,
    Exact = 2,
    StartsWith = 3,
    RegularExpression = 4,
    Never = 5,
}
```

**完全一致**。报告把协议值记反了——Bitwarden 协议 2 就是 Exact，3 就是 StartsWith。

importer（`bitwarden.rs:176`）和 exporter（`exporter.rs:74`）都走标准 `i64::from(MatchType)` / `MatchType::try_from(i64)`，round-trip 正确。

**处理**：不修改代码。但加固 `test_match_type_round_trip` 测试，补 `try_from(2) == Exact` / `try_from(3) == StartsWith` 断言，防未来误改 + 防此类误报回归。

---

## Task 1: pull 改用 md5 比对（#2 + #5 + #6）+ stamp 前置（#3）+ 静默吞错（#10）

### #2 pull 忽略 outline md5

**问题**：`pull_from_files` 把 outline 当「uuid 列表」用，`_entry` 丢弃，改用 `file.updated_at > db.updated_at` 字符串比较。跨设备时间戳格式不可控。

**修复**：新增 `cipher_md5_mismatch(uuid, outline_md5, db_ciphers)`，对比 outline.md5 vs DB sync_md5。参照 hotword.rs 的正确模式（hotword pull 本就用文件 md5 vs DB sync_md5）。

### #5 folder rename 丢失

**问题**：folder 分支 `if !db_folder_ids.contains(uuid)` 整个跳过已有 folder → 远程 rename 静默丢失。

**修复**：folder 与 cipher 对称，加 `folder_md5_mismatch`，已有 folder 也比对 md5。

### #6 sort_order 硬编码 0

**问题**：`folder_md5_from_fields(&id, &name, 0)` 硬编码 0，忽略文件实际 sort_order → md5 永远算错。

**修复**：
- `pull_from_files`：用 `folder_file.sort_order` 实际值
- `clone_initial`：同上
- 新增 `upsert_folder_with_sort`（带 sort_order 的 upsert）
- infra db 层新增 `update_vault_folder_fields`（同时更新 name + sort_order + sync_md5）
- `rename_folder`（folder.rs）的 `unwrap_or_default()` 吞错改为 `?`

### #3 stamp 校验前置

**问题**：执行顺序 cipher upsert → folder upsert → 才校验 stamp。不一致时 DB 已被污染无回滚。

**修复**：重构为两阶段：
- **阶段 A（校验）**：先读 meta.json 校验 stamp，不一致直接返 `MasterPasswordMismatch`，不触碰 cipher/folder DB
- **阶段 B（应用）**：stamp 一致后才 upsert cipher/folder/meta

加固测试 `pull_rejects_mismatched_security_stamp`：断言不一致时 cipher DB 数量未变（不只是 stamp 没被覆盖）。

### #10 静默吞错

**问题**：`if let Ok(cipher_file) = store::read_cipher_file(uuid)` 无 else 分支，损坏文件静默跳过。

**修复**：改为 `match`，Err 分支 `log::warn!` + 累计 `skipped` 计数。`pull_from_files` 返回 `(pulled, skipped)`。

---

## Task 2: SyncReport push 失败可见（#4）

**问题**：push 失败只 `log::warn!`，`SyncReport` 无条件报「已推送到远程」→ 用户以为已备份，实际未上云。

**修复**：
- `SyncReport` 加 `push_errors: Vec<(String, String)>`（remote 名 + 错误消息）+ `skipped: usize`
- push 循环收集失败到 `push_errors`
- `message` 措辞根据 `push_errors` 分支：非空时显「N 个 remote 推送失败，本地已保存未上云」
- 前端 `SyncPanel.tsx`：TS 接口加字段 + `pushErrors.length > 0` 时用 warning toast
- 新增 `ToastVariant::warning`（琥珀色，不自动消失）

---

## Task 3: SYNC_LOCK 覆盖写入口（#7）

**问题**：`SYNC_LOCK` 只在 `sync_now` 入口获取。sync 进行中点 disable → `remove_dir_all(.sync/)` → sync_now 后续命中 ENOENT，留半提交残留。

**修复**：`enable_sync` / `disable_sync` / `clone_from` / `resolve_with_remote` / `resolve_with_local` 入口加 `let _guard = try_sync_lock()?;`。

**副作用**：多个集成测试都持锁，多线程并发竞争失败。修复：测试模块加 `TEST_SERIALIZER` mutex + `test_lock()` helper，所有持锁测试串行化。

---

## Task 4: hotword sync 一并修（#10 同源）

经 Explore 核查，hotword.rs 的 `pull_hotwords_from_files` 有相同的静默吞错问题（`if let Ok(file)` 无 else + `hotword_md5_mismatch` 内 `Err(_) => false`）。

**修复**：
- `pull_hotwords_from_files`：加 `else { log::warn! }` 分支
- `hotword_md5_mismatch`：`Err` 分支加 `log::warn!` 后再返 false（保留「不 pull 损坏文件」语义，只加日志）

**不引入冲突检测**（用户决策：last-write-wins 记为已知限制，Phase 2 自动同步时考虑）。

---

## Task 5: generator 中间材料 zeroize（#8）

**问题**：generator 的明文密码/PIN/passphrase 以裸 `String`/`Vec<char>` 返回，中间材料不清零。

**修复决策**：仅清零中间材料（返回值保持 String——Tauri IPC 边界保护意义有限，密码产生就进 JS heap）。

- `random.rs`：`result: Vec<char>` → `Zeroizing<Vec<char>>`，`charset` 同理
- `passphrase_en.rs`：`words: Vec<String>` → `Zeroizing<Vec<String>>`
- `passphrase_zh.rs`：`result` 用 `Zeroizing<String>`
- `pin.rs`：`s` 用 `Zeroizing<String>`

---

## Task 6: MetaFile salt 解码报错（#9）

**问题**：`to_sync_fields` 的 `base64::decode(&self.kdf_salt).unwrap_or_default()` —— 空 Vec 静默通过 → Argon2 用空 salt 派生 → 解密失败 → 误导用户反复输错密码。

**修复**：
- 返回类型从 9-tuple 改为 `Result<MetaSyncFields>`（新建 struct，字段自文档化）
- base64 解码失败 `.with_context()?` 显式报错
- 3 个调用点（pull_from_files / clone_initial / resolve_with_remote）适配

---

## Task 7: 安全模型表述 + KDF 完整性（#11 + #13 + #14）

### #11 SyncError Display 不透传 stderr

**问题**：各变体 Display 直接输出 git stderr（`write!(f, "...：{}", msg)`），若 remote URL 含 PAT 则泄露。

**修复**：`NetworkUnreachable` / `SshPermissionDenied` / `RemoteNotFound` / `ConflictNeedsManual` / `GitError` / `CredentialsRequired` 的 Display 不再输出原始 msg，只给分类提示。msg 保留在 enum 内供 `Debug`/log 诊断。

**保留**：`PublicRepoRejected(url)` 仍输出 URL（用户需要知道哪个 repo 被拒，且是用户自输非 stderr 泄露）；`RepoCorrupted`/`OutlineDamaged` 保留 msg（不含 PAT 风险）。

### #13 machine-key.enc 注释如实

**问题**：注释称「AES-256-GCM 加密」夸大防护强度。

**修复**（仅注释）：改为如实表述「HKDF 派生的混淆 key（非真加密——四个输入公开/硬编码，防护等价文件权限 0600）」。

### #14 Argon2Params i64→u32 校验

**问题**：`as u32` 截断发生在 Params::new 之前，负值/超大值静默回绕成弱参数。

**修复**：新增 `Argon2Params::from_i64(iters, mem, par) -> Result<Self>`，检查范围 + 最小值（iterations ≥ 1, memory_kib ≥ 8, parallelism ≥ 1）。3 个调用点（unlock.rs meta_to_kdf_params + resolve_with_remote + resolve_with_local）改用。

---

## Task 8: 低优先级清理项

| 项 | 状态 |
|---|---|
| 8.1 Cipher::encrypt_strings 与 CipherInput::encrypt_strings 重复 | ✅ 提取 `encrypt_cipher_fields` 共用 |
| 8.2 字符集 `&[&str]` → `&[char]` | ⏸ follow-up（random.rs 刚改 zeroize，避免叠加重构） |
| 8.3 eff_wordlist 注释纠正 | ✅ 「include_str! 外部文件」→「字面量硬编码」 |
| 8.4 matcher 正则缓存 + Host 小写归一 | ✅ Host 小写归一已做；正则缓存 ⏸ follow-up（需并发设计） |
| 8.5 health/strength 长密码短路 | ✅ > 1KB 短路返 Score::4 |
| 8.6 change_master_password 换 salt | ⏸ 保留现状（Bitwarden 也不换） |
| 8.7 消除 2N 次 read_cipher_file | ✅ Task 1 改用 outline.md5 后自然消除 |

---

## #12 vault_update_cipher 静默成功 —— follow-up

**问题**：`update_vault_cipher_at` 的 `UPDATE ... WHERE id=?` 影响 0 行也 `Ok(())`。

**决策**：用户选择了「返回 affected rows + 0 行报错」，但实施时发现影响面较大（save_cipher 的多个调用方需适配），且当前 session 工作量已饱和。标记为 follow-up，不纳入本次。

---

## 已知限制（未修复，记入设计）

1. **last-write-wins**（vault + hotword 共有）：本地未 push 的修改可能被远程盲覆盖（pull 无条件 upsert）。用户决策：不引入冲突检测，Phase 2 自动同步时考虑引入 timestamp 仲裁 + 用户冲突提示 UI。
2. **machine-key.enc 是 obfuscation**（#13）：同机进程能解出 K_machine。生产签名后应切回 OS Keychain 方案。

---

## 测试覆盖

新增测试（全部通过）：
- `cipher_md5_mismatch_compares_outline_vs_db` / `folder_md5_mismatch_compares_outline_vs_db`（#2/#5）
- `pull_uses_md5_not_updated_at`（#2 回归守护）
- `pull_captures_folder_rename`（#5 回归守护）
- `pull_rejects_mismatched_security_stamp` 加强（#3——断言 cipher DB 未被污染）
- `pull_skips_corrupted_cipher_file`（#10）
- `folder_md5_includes_sort_order`（#6）
- `display_does_not_leak_pat_from_stderr`（#11）
- `to_sync_fields_errors_on_invalid_base64_salt`（#9）
- `test_meta_to_kdf_params_rejects_invalid`（#14）
- `test_host_match_case_insensitive`（8.4）
- `test_very_long_password_short_circuits`（8.5）

加固测试：
- `test_match_type_round_trip` 待补 `try_from(2) == Exact` 断言（防 #1 类误报）

**测试基线**：vault 209 passed / sync 97 passed / desktop 待最终验证。
