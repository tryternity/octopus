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
| H2 | 🔴 高 | 软删密码跨设备不同步 + clone 复活 | ✅ 成立 | 已修 |
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
- `test_very_long_repetitive_password_is_weak` / `test_very_long_low_unique_cycle_is_weak` / `test_very_long_diverse_password_is_strong`（M1 + N1）
- `test_empty_or_short_secret_rejected`（M2）
- `test_skip_deleted_ciphers`（L6）

**测试基线**：vault 214 passed（+5）/ desktop 396 passed / tsc 0 error。

---

## 第三轮新发现修复（2026-07-24，N1-N4）

### N1: 超长低唯一字符循环仍误报（M1 的补全）

**问题**：M1 的 `unique_count.log2() × char_count` 公式堵住 unique=1，但 `"ab"×1024`（unique=2, log2(2)=1 → 2048 bit → Score::4）仍误报。zxcvbn 本能识别循环模式但 >1KB 短路跳过了它。

**修复**：超长密码取前 256 字符跑 zxcvbn 做模式识别（zxcvbn 能识别重复/循环/键盘序列/字典词），用其 score；再用完整长度估熵做补充。取两者较低值（`pattern_score.min(entropy_score)`）——防"长但重复"或"短采样恰好高熵"误报。

### N2: AES key schedule 不 zeroize —— 反馈（修法不成立）

**报告建议**：「一行 Cargo feature 启用 aes 的 zeroize」。

**复查结论：不成立**。aes 0.8.4 的 `[features]` 只有 `hazmat`，**没有 zeroize feature**。zeroize 是 target-specific optional dependency（仅 aarch64 armv8），但没有 feature flag 暴露给用户启用。无法通过 `Cargo.toml` 一行修复。

**替代方案评估**：
- fork aes 0.8 加 zeroize feature → 维护成本高
- 调用方 `unsafe` 手动清零 → 拿不到 Aes256Gcm 内部 round keys 私有字段
- 升级 aes → aes-gcm 0.10 锁定 aes 0.8

**决策**：标记为 follow-up。H1 已堵住主密码这条线（最高价值的源秘密），AES round keys 是派生密钥的展开形式（value 低于主密码本身）。完整修复需待 aes-gcm 升级或迁移到其他加密库（如 RustCrypto 的新版本可能支持）。

### N3: rename 后 fsync 父目录（L4 的补全）

**问题**：L4 的 `write_atomically` 对 temp 文件 `sync_all` 了，但 rename 后未对父目录 fsync。POSIX 下目录项更新需 fsync 才能扛断电。

**修复**：rename 后 `File::open(parent)?.sync_all()`（best-effort，失败不阻断）。与 keychain.rs 的同样缺口暂不一并改（一致性，非新引入）。

### N4: resizable(true) 副作用加固

**问题**：L1 改 `resizable(true)` 理论上允许用户拖拽改尺寸（实践无碍——frameless 窗口无把手）。

**修复**：加 `min_inner_size(320.0, 360.0)` 防御——即使可拖拽，也不会缩到不可用。

---

## 第四轮审查修复（2026-07-24，H2/M5/L10/L11）

### H2: 软删密码跨设备不同步 + clone 复活（高，数据隐私）

**根因**：`VaultCipherInput` 结构体没有 `deleted_at` 字段，而 sync 的「应用」侧（pull/clone）全部经它落库 → 软删状态在同步链路上有去无回。

**证据链**（全部回源码核实）：
- push 写文件：store.rs:218/241 **保留** deleted_at ✅
- md5 指纹：fingerprint.rs:40 **包含** deleted_at → 软删会改 md5 触发对端 re-pull ✅
- pull 读文件→落库：engine.rs 构造 VaultCipherInput 时**丢弃** deleted_at ✗
- clone 落库：engine.rs:467 硬编码 `deleted_at: None`（注释「未软删」是错误判断）✗
- INSERT SQL：db.rs 无 deleted_at 列 → 默认 NULL ✗
- UPDATE SQL：db.rs SET 子句无 deleted_at → 粘性（不碰）✗

**失效模式**：
1. **clone 复活**：A 软删 X → B clone → X 在 B 上 deleted_at=NULL（live，可自动填充）
2. **md5 振荡**：B 已有 X(live) → A 软删 → B pull 更新 sync_md5=md5(T) 但 deleted_at 仍 NULL → B push 写文件 deleted_at=NULL → outline.md5=md5("") → A pull mismatch… 每次 sync md5 翻面 + 文件反复重写

**修复**：
- `VaultCipherInput` 加 `deleted_at: Option<String>` 字段
- INSERT/UPDATE SQL 加 deleted_at 列
- pull：从文件读出的 row 取 deleted_at 传入
- clone：从文件读出的 cipher 取 deleted_at（删除硬编码 None + 删除冗余 row 构造）
- `save_cipher`：编辑时读现有 deleted_at 传入（保留删除状态，不被覆盖成 None）
- `cipher_md5_from_input`：从 input 取 deleted_at（之前硬编码 ""）
- 新增 `pull_preserves_soft_deleted_at` + `clone_preserves_soft_deleted_at` 回归测试

### M5: 永久删除无 tombstone 可复活（中，设计缺口，未修）

**问题**：pull_from_files 只 upsert 从不删除；incremental_export(push) 会删 SQLite 无的文件。多设备时序：A permanent_delete X → A push 删文件 → 但 B 在 A push 前 pull（B outline 仍有 X）→ B push 把 X 文件写回 → A pull 复活。

**状态**：文档化为已知限制（与 last-write-wins 同类）。完整修复需 tombstone 机制（标记已删 uuid + 同步传播 + 清理策略），工作量大，Phase 2 自动同步时统一设计。

### L10: upsert_folder_with_sort O(N²)（低，未修）

**问题**：用 `list_vault_folders().iter().any()` 判存在，每 folder 全表扫。

**状态**：未修。报告自评「folder 通常很少，实际无碍」。优化需加 `load_vault_folder(id)` 单点查询，收益极低。

### L11: empty_trash 未持 SYNC_LOCK（低，已修）

**问题**：`vault_empty_trash` 与 sync_now 并发时，刚永久删的行可能被并发 pull 重新插入（M5 的本地并发表现）。

**修复**：`vault_empty_trash` 命令入口加 `try_sync_lock()`——sync 进行中返「同步进行中」。

---

## 第六轮审查修复（2026-07-24，M6/L12/L13-L16）

### M6: 导出/导入 passwordHistory + folder round-trip（中，数据完整性）

**问题**：export 端 `BitwardenItem` 无 passwordHistory / folderId，`folders: vec![]` 硬编码空；import 端 `password_history: vec![]` / `folder_id: None` 硬编码。导出→重新导入后密码历史清空 + 文件夹归属丢失。

**修复**（功能改动，含 spec + TDD）：

**export 端**（exporter.rs）：
- `BitwardenItem` 加 `folderId: Option<String>` + `passwordHistory: Vec<BitwardenPasswordHistory>`
- `BitwardenExport.folders` 不再硬编码空，输出实际 folders `[{id, name}]`
- `export_vault_json` 签名改为 `(&[Cipher], &[FolderDto])`——需 folder 数据（已解密明文）
- 新增 `BitwardenFolder` + `BitwardenPasswordHistory` struct

**import 端**（bitwarden.rs）：
- `BitwardenItem` 加 `folderId` + `passwordHistory`（`#[serde(default)]` 向后兼容）
- `BitwardenExport` 加 `folders`（`#[serde(default)]`）
- 导入逻辑：先导入 folders（建 folderId→本机 folder_id 映射，同名复用），再导入 items（folder_id 从映射取，passwordHistory 从 item 读）

**调用方**（vault_commands.rs vault_export）：额外读 folders 传给 export_vault_json

**Bitwarden JSON 格式映射**：
| octopus 字段 | Bitwarden JSON 字段 | 说明 |
|---|---|---|
| `PasswordHistoryEntry.password` | `passwordHistory[].password` | 明文密码 |
| `PasswordHistoryEntry.last_used_at` | `passwordHistory[].lastUsedDate` | ISO 8601 |
| `Cipher.folder_id` | `items[].folderId` | 引用 folders.id |
| `FolderDto { id, name }` | `folders[] { id, name }` | folder 定义 |

**TDD 测试**：
- `test_export_includes_password_history` / `test_export_includes_folders`
- `test_import_folders_and_password_history`（round-trip）
- `test_import_old_export_without_folders_still_works`（向后兼容）

### L12: matcher 等价域名大小写归一（低-中，8.4 同类遗漏）

**问题**：`matches_domain` 的 `group.contains(&cipher_domain)` 大小写敏感——用户配置含大写（Google.com）时等价域名组静默不生效。8.4 修了 Host 策略的小写归一，Domain 策略的 group 查找未对齐。

**修复**：cipher_domain + equivalent groups + target_domain 全部 `to_lowercase` 归一。

### L13-L16: 文档化已知限制（低，不改代码）

| 项 | 说明 | 处理 |
|---|---|---|
| L13 | rename_folder O(N) 读全表 | 报告自评可忽略（folder 量级小） |
| L14 | attempt_guard 退避基于 wall-clock | 报告自评可接受（单机威胁模型，调慢时钟只锁自己更久） |
| L15 | migrate UPDATE 无 NOT LIKE 守卫 | 首启无并发改 key，竞态窗口不存在 |
| L16 | load_or_create_machine_key 双进程首启竞态 | 单用户桌面极少并发，K_machine 复用无害 |

---

## 第七轮审查修复（2026-07-24，M7/L17/L18/L20）

### M7: import folder 创建在 cipher batch 事务外（中，残留）

**问题**：M6 的 folder 创建（逐个 autocommit）在 cipher batch 事务之前，batch 失败时已建 folder 不回滚 → 空文件夹残留。

**修复**：folder 必须先于 cipher 创建（FK 约束），但记录新建的 folder_id——batch 失败时**补偿删除**（`delete_folder`）。不是延后创建（FK 不允许 cipher 先于 folder），而是失败回滚。

### L17: vault_export 无 SYNC_LOCK（低，快照一致性）

**修复**：vault_export 命令入口加 `try_sync_lock`——避免 list_ciphers + list_folders 两次读期间 sync_now 并发写入导致快照不一致。

### L18: lastUsedDate 格式与真 Bitwarden 不兼容（低，互操作）

**问题**：octopus 的 `last_used_at` 来自 SQLite `datetime('now')`（`"2026-07-24 12:00:00"` 空格分隔），Bitwarden 标准是 ISO 8601（`"2026-07-24T12:00:00.000Z"`）。octopus 自身 round-trip 安全（透传），但导出到真 Bitwarden 日期显示错位。

**修复**：export 时 `normalize_to_iso8601` 归一化（空格→T + 补 `.000Z`）。import 透传（Bitwarden 的 lastUsedDate 本身是 ISO 8601）。

### L20: import folder 映射 N+1 全表解密（低，性能）

**问题**：M6 每个 folder 都调 `list_folders(key)` 全表解密 → K 个 folder = K 次全表。

**修复**：循环外 `list_folders` 一次建 `name→id HashMap`，循环内查 HashMap。

### L19: matches_domain 重复 to_lowercase（低，follow-up）

**问题**：L12 的 `lower_group` 每次 cipher×URI 匹配重算（group 不随 cipher 变化）。

**状态**：follow-up。报告自评「group 数量小，实际影响可忽略」。优化需改 4 个函数签名（find_matching_ciphers → matches_any_uri → match_uri_one → matches_domain 传预计算的 lower_equivalent），波及面 vs 收益不匹配。
