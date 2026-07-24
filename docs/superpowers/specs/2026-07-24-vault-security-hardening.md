# Vault 安全加固（第二轮代码审查修复）

**日期**：2026-07-24
**状态**：已实现并测试通过（vault 213 / desktop 396）
**关联**：[vault-sync-code-review-fixes](./2026-07-24-vault-sync-code-review-fixes.md)（第一轮）

## 背景

用户贴了第二轮代码审查报告（1 高 + 4 中 + 9 低 + C agent M2 澄清）。逐条回源码核实后，**全部成立**。报告质量很高，无误报。特别地，M1 是第一轮我引入的 bug（超长密码短路误报 Score::4）。

---

## 复查结论总表

| # | 严重度 | 描述 | 结论 | 状态 |
|---|--------|------|------|------|
| H1 | 🔴 高 | 主密码 & 敏感 String 全程不 zeroize | ✅ 成立 | 已修 |
| M1 | 🟡 中 | 超长密码 Score::4 误报（第一轮引入） | ✅ 成立 | 已修 |
| M2 | 🟡 中 | totp secret 无长度下限 | ✅ 成立 | 已修 |
| M3 | 🟡 中 | setup_vault TOCTOU | ✅ 成立（低） | 已修 |
| M4 | 🟡 中 | 枚举非法值静默降级 | ✅ 成立 | 已修（log warn） |
| L1 | 🟢 低 | resizable(false) 忽略 setSize | ✅ 成立 | 已修 |
| L2 | 🟢 低 | AES key schedule 重算 | ✅ 成立 | ⏸ follow-up |
| L3 | 🟢 低 | 正则重编译 + ReDoS | ✅ 成立 | ⏸ follow-up |
| L4 | 🟢 低 | store.rs 非原子写 | ✅ 成立 | 已修 |
| L5 | 🟢 低 | attempt_guard 墙钟 | ✅ 成立（可接受） | 文档化 |
| L6 | 🟢 低 | find_duplicates 不过滤 deleted_at | ✅ 成立 | 已修 |
| L7 | 🟢 低 | health/search 全量解密 | ✅ 成立（已 debounce） | 文档化 |
| L8 | 🟢 低 | import 非事务 | ✅ 成立 | 已修 |
| L9 | 🟢 低 | 目录权限 0700 | ✅ 成立（泄露面小） | 文档化 |

---

## Task 1: H1 主密码全程 zeroize（高，核心）

**问题**：`DerivedKey` 全套包 `Zeroizing`，但派生它的源秘密（主密码）这条线裸奔。`kdf.rs:86` 注释承诺「调用者必须 zeroize password」，但 4 个 unlock 入口收 `&str`（只读借用，无法 zeroize），从未兑现。

**修复方案**：`Zeroizing<String>` 所有权转移。

### vault crate（6 个函数签名）
- `unlock.rs`：`setup_vault` / `unlock_with_master_password` / `change_master_password` / `verify_master_password` 都改收 `Zeroizing<String>`
- `sync/engine.rs`：`resolve_with_remote` / `resolve_with_local` 同样
- 函数结束时 owned 值 drop → heap 自动清零，**兑现 kdf.rs 注释承诺**
- `derive_master_root_key` 签名不动（仍 `&[u8]`，底层通用）；`validate_master_password` 不动（unlock 层 deref 传入）

### IPC 边界（6 个 Tauri 命令体）
- `vault_setup` / `vault_unlock` / `vault_change_password` / `vault_autotype` / `vault_copy_password`（5 个）+ `vault_sync_resolve_remote` / `vault_sync_resolve_local`（2 个）
- **Tauri 命令签名不动**（仍收 `String`）——命令体内 `Zeroizing::new(password)` 包裹后 move 进 vault 层。前端协议零影响。
- desktop/Cargo.toml 加 `zeroize` optional 依赖（vault feature gate）

### 测试适配
- unlock.rs 内联测试 ~25 处：加 `zstr(&str) -> Zeroizing<String>` helper，字面量/变量调用包裹
- desktop 测试 2 处 `setup_vault("...")` → `Zeroizing::new("...".into())`
- 新增 `h1_unlock_functions_accept_zeroizing_string` 编译期签名约定守护测试
- 删除了不稳定的 unsafe heap 读取测试（依赖 allocator 行为，不可靠）

### 关键决策
- 选方案 B（`Zeroizing<String>` 所有权）而非方案 A（`&Zeroizing<String>` 借用）——因为 (1) Tauri 命令签名不动 (2) 所有权语义与"函数接管密码并负责清零"承诺一致 (3) 绕开 `&str` 只读借用无法 zeroize 的根本限制
- 不改 `derive_master_root_key` / `validate_master_password` 签名（减少波及，在 unlock 层 deref）

---

## Task 2: M1 超长密码 Score::4 误报（中，第一轮引入的 bug）

**问题**：第一轮（8.5）的超长密码短路逻辑用 `chars().count() * 6.0`（按 64 字符集）估熵直接返 Score::4——对 `"a".repeat(2048)` 这种低熵重复序列误报极强。zxcvbn 本会识别重复模式给低分，短路反而绕过了这个检测。

**修复**：改用**唯一字符数估熵**：`unique_chars.log2() × char_count`
- `"a".repeat(2048)`：unique=1, log2(1)=0 → 熵=0 → Score::0（弱）
- 正常长密码（unique=70）：log2(70)≈6.13 × 1024 ≈ 6275 bit → Score::4（强）
- score 阈值对齐 zxcvbn 的 0-4（<28/28-36/36-60/60-128/>128 bit）
- 低熵时加 warning「密码虽长但字符重复度高」

---

## Task 3: M2 totp secret 长度下限（中，安全）

**问题**：`from_otpauth_url` 只 clamp step/digits/algorithm，没校验 secret 长度。文件头注释承诺 RFC 6238 80bit（10 字节）下限但未落地。`base32::decode("")` 返回 `Some(Vec::new())`，空 secret 会通过 → 完全可预测的 code。

**修复**：
- `from_base32`：加 `ensure!(bytes.len() >= 10)`
- `from_otpauth_url`：加 `ensure!(totp.secret.len() >= 10)`

---

## Task 4: M3 setup_vault TOCTOU（中低）

**问题**：`is_initialized()` 检查到 `save_vault_meta` 之间无锁，并发双 setup 有竞态窗口。schema `CHECK(id=1)` 兜底（第二个 INSERT 失败），实际不会数据损坏。

**修复**：`setup_vault` 入口加 `acquire_meta_write_lock()`，消除结构竞态。

---

## Task 5: M4 枚举非法值改可观测（中，一致性）

**问题**：`RepromptType::from(99)` → None（绕过二次验证）；`CipherType::from(99)` → Login。与 `MatchType::try_from`（bail）不一致。

**决策**：保留 `From<i64>` 兜底语义（serde/DB 需要 infallible），但对非法值加 `log::warn!` 让问题可观测。
- 不全面改 TryFrom（波及面大：7+ 调用点 + serde 属性 + DB 读取点）
- 报告自评「单机威胁模型下风险低」（假设 DB 不被直接改）
- log warn 至少让诊断时能发现 DB 被篡改的迹象

**已知限制**：降级 None 意味着绕过二次验证——威胁模型假设 DB 不被直接改。

---

## Task 6: L1 resizable(false) vs setSize（低，UI）

**问题**：`vault_picker_window` 用 `.resizable(false)`，但 Tauri 2 `resizable(false)` 会忽略后续 `setSize` 调用。

**修复**：`.resizable(false)` → `.resizable(true)`（当前固定 320×360，但保证 setSize 不被吞）。

---

## Task 7: L4 store 原子写（低，数据完整性）

**问题**：`write_meta_file` / `write_cipher_file` / `write_folder_file` / `write_outline_file` 全用裸 `std::fs::write`，非原子。崩溃/断电中途 → 截断 JSON。

**修复**：抽 `write_atomically(path, content)` helper（复用 keychain.rs 的 temp + sync_all + rename 模式），4 个 write 函数替换。
- temp 文件用 `.<name>.tmp`（隐藏 + 不匹配 walk_json_files 的 .json 扫描）
- 不设 0600（git 同步需正常权限）

---

## Task 8: L6 find_duplicates 过滤 deleted_at（低，API 卫生）

**问题**：`find_duplicates` 不过滤软删 cipher，依赖调用方预过滤。

**修复**：内置 `if c.deleted_at.is_some() { continue; }`。

---

## Task 9: L8 import 事务化（低→中，数据完整性）

**问题**：`import_bitwarden_json` 逐条 `create_cipher`（各自 autocommit），中途失败留部分数据。

**修复（两阶段）**：
1. infra/db.rs 加 `insert_vault_ciphers_batch`（`unchecked_transaction` + 循环 `insert_vault_cipher_at` + `commit`）
2. storage/cipher.rs 加 `prepare_cipher_input`（只加密+算 sync_md5，不落库）
3. importer 循环重构：阶段 1 加密收集 `Vec<VaultCipherInput>`（加密失败记 errors、跳过）→ 阶段 2 一次性 batch insert
- 既保证 DB 原子性（全成功或全回滚），又保留「跳过坏条目」容错

---

## follow-up（未修，已记录）

| 项 | 原因 |
|---|---|
| **L2 AES key schedule 缓存** | `DerivedKey` 包 Zeroizing，加缓存会让 Aes256Gcm key material 长期驻留（与 zeroize 理念冲突）；key schedule 是微秒级，批量导入开销有限 |
| **L3 正则缓存 + ReDoS** | 需 `Mutex<HashMap>` 并发设计；跨设备同步的正则需限制复杂度。需仔细设计 |
| **L5 attempt_guard 墙钟** | 报告自评「单机威胁模型下可接受」——改系统时间可绕过，但单机威胁模型假设攻击者无 root |
| **L7 health/search 全量解密** | 报告自评「前端已 debounce」——大 vault 时可优化（分页/索引） |
| **L9 目录权限 0700** | 创建点分散，收益低（machine-key.enc 已 0600）；报告自评「实际泄露面很小」 |

---

## 测试覆盖

新增测试（全部通过）：
- `h1_unlock_functions_accept_zeroizing_string`（H1 编译期签名约定守护）
- `test_very_long_repetitive_password_is_weak` / `test_very_long_diverse_password_is_strong`（M1）
- `test_empty_or_short_secret_rejected`（M2）
- `test_skip_deleted_ciphers`（L6）

**测试基线**：vault 213 passed（+4）/ desktop 396 passed / tsc 0 error。
