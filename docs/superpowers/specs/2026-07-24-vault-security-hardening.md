# Vault 安全加固（多轮代码审查修复汇总）

**日期**：2026-07-24 起，持续至 2026-07-25
**状态**：已实现并测试通过。最新基线：vault **249** passed + 2 ignored（lib）+ 1 passed（集成 unlock.rs）/ desktop **412** / infra 160 / sync 101 + 4 ignored / tsc 0 error / cargo build 0 warning
**范围**：本文件汇总第二~第二十轮代码审查修复（第一轮见关联文档）。各轮次按发现顺序记录，含问题、修复、测试、文档化决策。
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

### M5: 永久删除无 tombstone 可复活（~~中~~→**高**，设计缺口，未修）

**严重度升级（2026-07-25 第二十五轮）**：原定「中」，升级为「高」。密码管理器的核心承诺是「删除即删除」，硬删（empty_trash）后密码经多设备 sync 复活违反此承诺，且有安全影响（用户以为已删除的敏感密码仍存活于各设备 + 远程仓库 git 历史）。详见 [第二十五轮](#第二十五轮审查修复2026-07-25syncoutline--syncstorers--enginers-删除传播)。

**问题**：pull_from_files 只 upsert 从不删除；incremental_export(push) 会删 SQLite 无的文件。多设备时序：A permanent_delete X → A push 删文件 → 但 B 在 A push 前 pull（B outline 仍有 X）→ B push 把 X 文件写回 → A pull 复活。

**状态**：文档化为已知限制。完整修复需 tombstone 机制（标记已删 uuid + 同步传播 + 清理策略），工作量大，Phase 2 自动同步时统一设计。触发条件：① 多设备 sync；② empty_trash 硬删。单设备/仅软删不受影响（软删通过 md5 变化正确传播）。

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

---

## 第十轮审查修复（2026-07-24，M8/L21）

### M8: incremental_export 吞 outline 解析错 → 删除不可靠 + clone 复活（中，数据完整性）

**问题**：`incremental_export`（store.rs:537）`read_outline_file().unwrap_or_default()` 把 outline.json 解析失败吞成空 Outline。删除循环遍历空 outline → 不执行 → SQLite 已删的 cipher 文件永久残留 → 新设备 clone（import_all_from_files 全量 walk）读到残留文件 → 已删密码复活。

**残留链条**：outline 损坏 → 删除循环不执行 → stale 文件残留 → outline 永不含该 cipher（后续 sync 只删「outline 有 / SQLite 无」的）→ 永久残留 → clone 复活。

**修复**：解析错时降级为 `export_all_to_files` 全量重建（`remove_dir_all` 清所有 stale 文件，恢复一致）。NotFound → 空 Outline（首次同步合法）。与 #9（salt unwrap_or_default 修复）原则一致——不吞解析错。

**新增测试** `incremental_export_degraded_rebuild_on_corrupt_outline`：验证 outline 损坏时 stale 文件被清理 + outline 重写为有效 JSON。

### L21: TOTP secret 未 zeroize（低，follow-up，受 totp-rs 限制）

**问题**：`TotpGenerator.inner: TOTP`（totp-rs）持有 secret: Vec<u8> 不实现 Zeroize；`secret.to_string()` 局部副本 drop 不清零。

**状态**：follow-up。totp-rs 的 TOTP/Secret 不实现 Zeroize，octopus 无法零成本包装（需 fork 或 wrapping）。局部副本可部分缓解但 TOTP.inner.secret 无法清。威胁模型外（单机离线 vault）。

---

## 第十二轮审查修复（2026-07-24，E2/E3/E5）

### E2: clone_initial 无本地已初始化检查（高，数据锁死）

**问题**：clone_initial 不检查本地 vault_meta 已存在 → 若 B 机已设主密码但 .sync 不存在，clone 用远程 meta 覆盖本地 kdf_salt/protected_user_vault_key/security_stamp → 本地 cipher 用旧 key 加密但新 meta 的 key 解不开 → **原数据永久锁死**。enable_sync 的 .sync 检查覆盖不到此场景。

**修复**：clone_initial 入口加 `db::load_vault_meta().is_some()` 检查，已存在则拒绝（要求先清本地 vault）。

### E3: meta_file_not_found 字符串匹配改类型安全（低，Q1 一致性）

**问题**：`contains("No such file")` 脆弱匹配（不同 io 错/locale 可能漏判）。活分支（read_meta_file 不内部转 NotFound）。

**修复**：改用 `downcast_ref::<io::Error>().kind() == ErrorKind::NotFound` 类型安全匹配。

### E5: upsert_folder_with_sort 新建两次写 DB（低）

**问题**：不存在时先 insert（sort_order=0）再 update 补 sort_order——两次 DB 写。

**修复**：infra 加 `insert_vault_folder_with_sort`（含 sort_order，一次写）。

### E1/E4 文档化

- **E1 硬删跨设备复活**（M5 重申）：pull 无删除传播。需 tombstone 机制，Phase 2 统一设计。软删（H2 已修）正确传播，只有硬删（permanent_delete）不传播。
- **E4 upsert_folder O(F²)**：报告自评「folder <100，毫秒级」。pull 已有 db_folders 缓存可复用，但改签名波及 clone/pull 两路径，收益极低。

---

## 第十三轮审查修复（2026-07-24，G2/G3）

### G2: git_commit 错误处理过宽（中，数据完整性）

**问题**：`git_commit` 的 `allow_exit_codes: &[1]` 无条件放行 exit 1（不只限于 nothing to commit）+ `Err(GitError) => Ok(false)` 兜底吞掉真实失败。两层过宽：
1. pre-commit hook 拒绝（exit 1）被放行 → Ok(stdout) → Ok(true) **谎报成功**
2. index.lock/磁盘满（exit 128）→ GitError → Ok(false) **当无变化吞掉**

**修复**：不再用 `run_git_allow_codes`，直接处理 git commit 输出：
- exit 0 → Ok(true)（成功）
- 非零 + stdout/stderr 含 "nothing to commit"/"no changes" → Ok(false)（无变化）
- 其余失败 → Err（不吞）

**关键发现**：git commit 的 "nothing to commit" 消息在 **stdout** 不在 stderr——`run_git_allow_codes` 的 `allow_stderr_contains` 检查 stderr 永远匹配不到。之前靠 `allow_exit_codes: &[1]` 兜底才通过。

### G3: cleanup_in_progress_ops 不清 stale index.lock（低-中）

**问题**：只检测 MERGE_HEAD/rebase，不检测 index.lock。崩溃残留的 index.lock 让下次 git add -A 失败。

**修复**：加 stale index.lock 清理（mtime > 60s 视为崩溃残留删除——SYNC_LOCK 保证单进程同步串行，60s 阈值区分崩溃残留 vs 并发持有）。

### G1/P1/P2 文档化（低）

- **G1**（from_utf8_lossy）：非 UTF8 输出替换为 U+FFFD。macOS 主，commit msg/branch 多 ASCII，低危。
- **P1**（privacy unwrap_or(false)）：API 200 但无 private 字段 → 默认 false → 判 Public 拒绝。方向安全（宁可误拒），GitHub/Gitee 正常响应必含 private 字段。
- **P2**（SSH_SCP_RE 正则重编译）：每次 parse 重新编译。低频（remote 1-3 个），低危。

---

## 第十四轮审查修复（2026-07-24，ER1/HW1/HW3）

> 补录：commit `0f8b259a` 已 push 但本轮此前缺独立章节，现补。

### ER1: classify_git_error "not found" 关键词过宽（中，分类误判）

**问题**：`|| lower.contains("not found")` 误匹配 "object not found"（本地 repo 损坏）等本地错误 → 分类成 RemoteAuth/RemoteNotFound → 前端误导用户查 remote 配置，实际是本地 repo 问题。

**修复**：删后半段，仅保留 "repository not found"（精确匹配 remote 仓库不存在）。

### HW1: hotword incremental_export 吞 outline 解析错（M8 对称遗漏）

**问题**：`unwrap_or_default` 吞 outline 解析错成空 → 删除循环不执行 → stale 文件残留 → clone 复活。与 vault M8 完全同型的对称遗漏（vault↔hotword 姊妹模块反复出现）。

**修复**：`match read_hotword_outline()` 降级 `export_all_hotwords`（与 vault M8 对称）。

### HW3: hotword pull 用 outline.md5（消除双读 + 与 vault #2 对齐）

**问题**：pull 忽略 outline `entry.md5`，调 `hotword_md5_mismatch` 读文件算 md5；pull 主体又读一次 → 同一文件双读。

**修复**：`hotword_md5_mismatch_v2` 用 outline.md5 对比 DB sync_md5（不读文件）；删除旧 `hotword_md5_mismatch`（读文件版）。

### HW2/G2-minor/ER2/HW4 文档化（低）

- **HW2**（pull 缺删除传播）：E1 同类，需 tombstone 机制（Phase 2）
- **G2-minor** / **ER2** / **HW4**：低危，文档化

---

## 第十五轮审查修复（2026-07-24，S1/S2/S3）

### S1: soft_delete/restore 两步非原子（中，数据一致性）

**问题**：soft_delete/restore 是「UPDATE deleted_at」+「读 row 算 md5 + UPDATE sync_md5」两次独立 autocommit。若第 2 步失败（DB 锁超时/磁盘满/事务冲突），deleted_at 已改但 sync_md5 仍旧 → incremental_export 用旧 sync_md5 对比旧 outline.md5 → 一致 → 文件不重写 → 删除状态不传播到其他设备。

**修复**：合并为单事务（`unchecked_transaction` 内 UPDATE deleted_at → SELECT row → 算 md5 → UPDATE sync_md5 → COMMIT）。db 层 `load_vault_cipher_at` 改 pub 供 vault crate 事务内调用。

### S2: save_cipher 无锁 RMW（低-中，文档化）

**问题**：save_cipher 读 existing_deleted_at（H2）后 update，期间无锁 → 并发 soft_delete 可能被撤销。

**状态**：文档化。报告自评「UI 单焦点下概率低」。完整修复需给所有 cipher 写路径加事务（soft_delete/restore 已修，save_cipher 的 RMW 改事务需重构）。

### S3: rename_folder 全表扫（低，文档化）

L13 已文档化。报告自评「folder 数量通常个位数到几十，O(F) 可忽略」。

---

## 第十六轮审查修复（2026-07-24，F1/F2）

### F1: S1 重构遗留的半截软删死代码（中低，bug 复活入口）

**问题**：S1 修复（第十五轮）将 `cipher::soft_delete/restore` 改为事务化内联 SQL 后，db 层遗留的 `soft_delete_vault_cipher` / `restore_vault_cipher`（+ `_at` 变体）+ `update_cipher_sync_md5` 共 5 个函数无任何生产调用方。但它们执行的是**「半截软删」**——只 UPDATE deleted_at（或只 UPDATE sync_md5），正是 S1 修复前的 bug 本体。保留它们等于在代码库里留了一个已修 bug 的一键复活开关：任何未来调用方（CLI/批量路径）不知情调用 `db::soft_delete_vault_cipher` → S1 缺口立即复活（deleted_at 改了但 sync_md5 旧 → 删除不传播）。

**修复**：删除 db.rs 中 5 个死函数（`soft_delete_vault_cipher` / `soft_delete_vault_cipher_at` / `restore_vault_cipher` / `restore_vault_cipher_at` / `update_cipher_sync_md5`）。删除 db.rs 测试中守护死逻辑的软删/恢复断言（`soft_delete_and_restore_update_sync_md5_atomically` 在 cipher 层已完整守护 S1 原子性，db 层测试冗余）。

**核查**：全 crate `rg` 确认 5 个函数零残留引用；cipher.rs:469 `soft_delete_and_restore_update_sync_md5_atomically` 三重断言（deleted_at 变化 + sync_md5 变化 + sync_md5 与 row 重算一致）完整覆盖。

### F2: fingerprint.rs 分隔符注释误导 + 设计脆弱性（低）

**问题**：注释称「字段间用 `|` 分隔（避免字段值含分隔符导致歧义）」——单字符分隔符本身不防歧义。反例：`name="a|b",notes="c"` 与 `name="a",notes="b|c"` 拼成同一字符串 → md5 碰撞 → sync 误判「无变化」不重写文件 → 跨设备不一致。

**当前安全靠字符集约束（隐式假设）而非分隔符设计**：
- 密文字段（name/notes/data/fields/password_history）= `v1:` + RFC4648 base64（`A-Za-z0-9+/=`），严格不含 `|`
- id/folder_id = UUID（含 `-`，不含 `|`）
- favorite/atype/reprompt = bool/i64（纯数字）
- deleted_at = SQLite datetime（含空格/`-`/`:`，不含 `|`）

**修复**：修正模块 doc 注释，明确说明真实安全保证来自 base64 字符集约束，并标注「新增字段前必须确认字符集不含 `|`，否则需改长度前缀分隔」。补回归测试 `cipher_md5_no_collision_on_pipe_in_separate_fields` 守护字符集约束。

**状态**：当前功能正确（所有字段恰好不含 `|`）。未来若加密格式变化（引入含 `|` 的编码）或新增非密文含 `|` 字段，需改用长度前缀分隔（`{len}|{value}` 重复）。

### 正面结论（fingerprint 不变量核查）

fingerprint 是同步正确性的源头，核查扎实：
- `cipher_md5`（sync 读 row）与 `cipher_md5_from_input`（create/save 填充）字段顺序（11 字段）、字段类型（atype/reprompt 两 struct 均 i64、favorite 均 bool）、None 处理（5 个 unwrap_or("") 对称）三者一致 → create 时填的 md5 与 sync 时读 row 算的 md5 永远对齐
- deleted_at 纳入 md5（H2）—— S1 前提
- 不含 created_at/updated_at —— 跨设备时间戳必然不同，排除避免永久 diff

---

## 第十七轮审查修复（2026-07-24，T1/T2/T4 + T3 文档化）

totp.rs 经 #7（80bit secret）、M2（空/短 secret 拦截）、复审 #1（畸形参数 clamp 防 panic）多轮加固，安全防护扎实。本轮处理 3 个低危改进 + 1 个文档化。

### T1: current() 与 seconds_remaining() 各自独立读时钟（低，UX）

**问题**：`current()`（:129 调 `generate_current`，totp-rs 内部读 `SystemTime::now()`）与 `seconds_remaining()`（:134 显式读 `SystemTime::now()`）两次独立读时钟，非原子。生产调用方 `vault_commands.rs:626-627` 先 `current()` 后 `seconds_remaining()`，跨 step 边界时可能返回「上个 step 的 code + 新 step 的 30s 倒计时」，持续约 1 秒陈旧显示。窗口 <1ms，非安全问题。

**修复**：新增 `current_with_remaining() -> Result<(String, u64)>`——显式读一次 `now`，用 `inner.generate(now)`（而非 `generate_current()`）传同一 time counter，保证 code 与 remaining 基于同一时刻。`vault_commands.rs` 改用新方法。保留 `current()` / `seconds_remaining()` 向后兼容。

### T2: from_base32 不 strip 内部空格/连字符（低，UX）

**问题**：用户从其他 Authenticator 导出常是分组显示（`JBSWY 3DPE HPK3 PXP` / `JBSWY-3DPE-HPK3-PXP`）。`from_input`（:114）只 `trim()` 首尾，不去内部；`Secret::Encoded.to_bytes()` 调 `base32::decode(Rfc4648)` 对含空格/连字符的串返 `None` → Err。用户需手动去空格，多数 TOTP 工具会 strip。

**修复**：`from_base32` 入口 `secret.chars().filter(|c| !c.is_whitespace() && *c != '-').collect()` strip 内部空白/连字符。

### T4: test_known_totp_value 名不副实（低，测试质量）

**问题**：注释称「RFC 6238 测试向量」，但只 `assert_eq!(code.len(), 6)`，与 `test_totp_format_6_digits` 重复。`current()` 用 wall-clock 无法断言固定值。

**修复**：改用 `inner.generate(time)` 传固定 time counter，断言 RFC 6238 附录 B 的真实 8-digit SHA1 向量（T=59→94287082, T=1111111109→07081804, T=1111111111→14050471），真正验证算法正确性。

### T3: TOTP secret 未 zeroize（低，文档化，交叉引用 L21）

与第十轮 L21 同一问题——`TotpGenerator.inner: TOTP`（totp-rs）持有 secret 不实现 Zeroize，是库限制。已在 L21 记录，本轮确认状态不变。

### 附带：清理 vault 既有 2 个 unused import warning

- `exporter.rs:188` test 模块 `Field` 未用
- `engine.rs:1857` test 函数 `pull_preserves_soft_deleted_at` 内 `VaultCipherInput` 未用（另一处 1647 有用，保留）

非本轮引入（分别来自 feat Task 15 / 第十二轮），但 AGENTS.md 要求 0 warning，顺手清。

---

## 第十八轮审查修复（2026-07-24，KC1 + AG2/KC2/KC3 文档化）

### KC1: 换机迁移下 load_machine_key 解密失败返 Err 致启动僵死 + 主密码死循环（中等）

**问题**：`load_machine_key`（keychain.rs:281-283）对两种"无法取出有效 K_machine"的处理不对称——前缀不符（:275-278）→ `Ok(None)`（视为不存在，可重建）；是 v1: 文件但解密失败（file_key 不匹配/损坏）→ `Err`。

`file_key = HKDF(machine_id:username, salt)`，换机迁移（拷整个 `~/.octopus/` 含 machine-key.enc 到新机器）→ machine_id 变 → file_key 变 → 解密失败。返 Err 致三条死路：

1. **启动僵死**：`unlock_app_key_local`（unlock.rs:154）`load_machine_key()?` 传播 Err → `vault_state.rs:178-180` 走 `Err(e) => log::warn!` 分支，既不设 app_key（:173 没走）也不走 :175 Ok(None) 的"需主密码"提示 → app 卡在"未解锁且无提示"
2. **主密码死循环**：`unlock_with_master_password`（:244）末尾 `refresh_app_key_local_enc`（:351）`load_machine_key()?` 再次 Err → :246 返"密码正确但刷新失败"→ 用户输对密码也解不开
3. **load_or_create 被堵**：`load_or_create_machine_key`（:237）`load_machine_key()?` 传播 Err → 走不到 :240 创建分支

**修复**：keychain.rs:281-283 解密失败改 `Ok(None)`，与 :275-278 对称。单点改，三处调用全部自愈：
- unlock.rs:154 → Ok(None) → return Ok(None) → vault_state 走"需主密码"提示 ✓
- unlock.rs:351 → Ok(None) → None 分支 → load_or_create 创建新 K_machine 重写 local_enc ✓
- keychain.rs:237 load_or_create → Ok(None) → 创建分支 ✓

**关键依据**：unlock.rs:148 文档注释**已承诺**"解密失败 → Ok(None)"，但实现对齐文档前是 bug。K_machine 本就是 obfuscation（模块注释 :14-30 说明），重建无损安全性，仅丢失"检测篡改"诊断信息（换机是合法场景，可用性让位诊断）。

**回归测试**：`load_machine_key_returns_none_on_decrypt_failure_machine_change`（#[ignore]，用错误 key 加密文件模拟换机 file_key 不匹配），验证 Ok(None) + load_or_create 自愈创建新 key。

### AG2: record_failure 非原子组合（低，文档化）

**问题**：`record_failure`（attempt_guard.rs:47-57）`fetch_add(1)`（计数）+ `store(now+delay)`（门）两次独立原子操作。并发失败时较晚的 store 可能被较早的较小 delay 覆盖 → next_allowed_at 偶发倒退（退避短暂偏弱）。

**状态**：文档化。failures 计数原子准确，下一轮 record_failure 会重 store 正确 delay，长期退避递增。unlock 路径并发性极低（UI 串行 + attempt_guard 单例 + 三入口不真正并发）。理想可用 CAS 单调更新 deadline，非必要。

### KC2: derive_file_key 无缓存（低，文档化）

**问题**：`derive_file_key`（keychain.rs:207-217）每次 load/save 重新 read_machine_id（macOS spawn ioreg ~10-50ms）+ read_username + HKDF。unlock 流程多次调 load_machine_key → 多次 spawn ioreg。

**状态**：文档化。可用 OnceLock 缓存 file_key，正确性无碍，仅性能优化。

### KC3: setup 回滚未删 K_machine（低，文档化）

**问题**：迁移失败 DELETE vault_meta 回滚（unlock.rs:129-134）后，:84 已 `load_or_create_machine_key` 创建的 K_machine 残留为孤儿。

**状态**：文档化。下次 setup load_or_create 复用它（本机随机 32B 复用无损安全）。严格对称应一并删除，但无实际风险。

### 正面发现（attempt_guard / keychain / unlock）

- attempt_guard 真实生效 + 精确语义：三处解锁路径均接 gate（unlock.rs:183/:268/:418）+ record_failure + reset。B2 修复精确区分"密码校验失败→record_failure" vs "副作用失败→仅 Err 不污染计数"
- change_master_password 密钥"字节不变只换保护层"（INV-7）：改密时 user_vault_key/app_key 用旧 master 解出后字节原样用新 master 重新加密，不重加密 vault_ciphers（Bitwarden 式高效）
- verify_master_password 不调 reset（:429）：reprompt 只是二次确认，不清 unlock 路径计数
- keychain.rs #13 诚实注释（:14-30）：如实承认 HKDF 派生 file_key 输入全公开，AES-256-GCM 实为 obfuscation——高水准安全注释
- keychain.rs #6 原子写（:314-396）：temp file + sync_all + rename
- 测试隔离谨慎：keychain override（thread_local）隔离真实文件；via_file 测试 #[ignore] 避免 once_cell HOME 缓存污染开发者本机；unlock 测试用 TEST_SERIALIZER mutex 串行化避免 attempt_guard 跨测试竞争
- setup 迁移回滚（A1，:117-136）：迁移失败显式 DELETE vault_meta 恢复 setup 可重试

---

## 第十九轮审查修复（2026-07-24，M1/M2/V1/V4 + M1(a)/V3 文档化）

### M1(b): 幂等守卫 NOT LIKE 'v1:%' 硬编码与 CIPHERTEXT_PREFIX 耦合（低-中，DRY）

**问题**：`db.rs:3924` 守卫 SQL `secret_key NOT LIKE 'v1:%'` 的 `'v1:%'` 与 `crypto::symmetric::CIPHERTEXT_PREFIX = "v1:"`（symmetric.rs:13）是两份独立字面量，无引用关系。未来密文格式升级（v2:）时若只改一处会导致：v2 密文被当明文再加密（数据损坏）/ v1 密文漏保护。经典"重构时漏改"温床。

**修复**：`list_models_for_secret_migration` 加 `encrypted_prefix: &str` 参数，SQL 用参数化绑定（`NOT LIKE ?1` + `format!("{}%", prefix)`）。调用方 `migrate.rs` 传 `CIPHERTEXT_PREFIX`，单点维护。infra 不依赖 vault（依赖方向 vault → infra），故不能直接引用常量，必须由调用方注入。

**回归测试**：`encrypt_prefix_matches_migration_guard_prefix`——验证 encrypt 产生的前缀 == CIPHERTEXT_PREFIX（守卫前缀），若重新引入硬编码字面量割裂两者会暴露。

### M1(a): 明文 API Key 以 v1: 开头永久漏迁（低，文档化）

**问题**：若用户某个云端 API Key 恰好以字面量 `v1:` 开头（自研带版本前缀的 key，如 `v1:sk-xxxx`），会被守卫误判为"已加密"跳过，保持明文残留 DB。迁移是 setup 一次性触发、`ensure!(!is_initialized())` 阻止重跑，漏迁不可自愈。

**状态**：文档化为已知边界。概率极低（API Key 罕见以 v1: 开头）。彻底方案是密文用不可伪造结构（固定长度/tag）而非前缀，当前前缀方案对小规模够用。

### M2: 事务内 UPDATE 失败不带行信息（低，诊断）

**问题**：`migrate.rs:48-53` 事务循环 UPDATE 失败 `?` bubble → tx rollback → Err 不带"哪一行 id 失败"。迁移行数少 UPDATE 失败罕见，但一旦发生调试困难。

**修复**：`.with_context(|| format!("迁移 model id={} 失败", id))` 在 `?` 前附加，tx drop 时 rollback 自动触发但错误信息已捕获不丢失。

### V4: 前后端符号集双份手工列举易漂移（低，UX）

**问题**：前端 `validateMasterPassword.ts:28-35 SYMBOL_CHARS` 与后端 `validate.rs:45-59 is_symbol` 是双份实现。码点级差异：前端 `¥` (U+00A5) vs 后端 `￥` (U+FFE5)；前端单引号 vs 后端左右单引号。核心问题是双份维护无共享源、无交叉一致性测试。

**修复**：
- 前端 SYMBOL_CHARS 全角段统一为后端码点：`¥`(U+00A5) → `￥`(U+FFE5)；ASCII 段改为全集列举对齐后端 `is_ascii_graphic && !alphanumeric` 语义
- 前端测试 :95 `¥` → `￥`、`——` → `—` 对齐
- 后端补 `is_symbol_covers_all_expected_chars` 守护测试（ASCII 全集标点 + 全角全集逐字验证）

### V1: "79 bit 熵"注释是乐观上界（低，文档准确性）

**问题**：validate.rs:8-9 与前端 :5-6 均注释"95 可打印 ASCII × 12 位 ≈ 79 bit 熵"。但策略只强制"4 类各含 1 个"+长度 12，最弱合法密码（如 `Aa1!!!!!!!!`）实际熵远低于 79 bit（79 bit 假设每位从 95 字符随机选）。

**修复**：注释改为"理论上界 ~79 bit，实际取决于用户选择；Argon2id 为弱密码兜底"（后端 + 前端）。

### V3: 主密码未 normalize（低，backlog）

**问题**：`derive_master_root_key` 用 `password.as_bytes()` 直接喂 Argon2id，不做 NFC normalize。组合字符（如 `é` = `e` + 组合 vs 预组合 U+00E9）不同输入方式产生不同字节 → 跨设备"看似对却解不开"。

**状态**：backlog。前端 input 框通常产 NFC（浏览器 normalize），实际风险低。

---

## 第二十轮审查修复（2026-07-25，crypto/ 全目录：K1/M1-mod/K3 + S2 文档化）

crypto/ 是 vault 的命门模块（KDF + 对称加密 + 密钥派生）。本轮处理 1 个纵深防御加强 + 2 个整洁性改进 + 1 个文档化。

### K1: from_i64 复用于不可信远程参数，memory 下限过低废掉 Argon2id 内存硬度（低-中，纵深防御）

**问题**：`Argon2Params::from_i64`（kdf.rs）注释假设"本地 DB 可信"，校验下限是**崩溃级**：iterations≥1、memory_kib≥8（即 8KB）、parallelism≥1。但 grep 确认 `from_i64` 被 `sync/engine.rs:933 resolve_with_remote` 复用于处理**远程不可信** KDF 参数（`f.kdf_*` 来自同步仓库 meta.json）——超出"本地 DB 可信"假设。

技术问题在 memory 下限：Argon2id 的抗 GPU 爆破核心是**内存硬度**（memory 参数），而非 iterations。memory_kib=8 = 8KB，可全放 GPU 寄存器/共享内存 → GPU 高度并行爆破，内存硬度几乎归零（逼近裸哈希速度）。OWASP 推荐的 65536 KiB（64MiB）让 GPU 难以并行，才是关键。`from_i64` 允许 memory=8 等于废掉 Argon2id 的主要防线。

**触发链**：攻击者污染同步仓库的 vault_meta 为 kdf_memory_kib=8 → 受害者 sync，`from_i64` 不拒绝 → vault_meta（含弱 KDF + salt + protected_user_vault_key 密文）在同步仓库中可被攻击者读取 → 离线爆破，弱 memory 让 GPU 高度并行。

**修复**：新增 `from_i64_strict`（安全下限），`resolve_with_remote` 改用它。本地路径（`resolve_with_local` :989）继续用 `from_i64`（本地 DB 可信）。安全下限：memory_kib≥16384（16MiB，保留 GPU 抗并行的内存硬度）、iterations≥2、parallelism≥1。

**严重度限定**：触发需攻击者先获取 vault_meta 内容（私有库访问/窃取本地 DB 拷贝）。私有库 + PublicRepoRejected 前提下，获取内容本身需较高权限；但一旦获取，memory=8 显著加速爆破。定低-中：纵深防御加强项，非新攻击面。

### M1-mod: DerivedKey 字段 pub（低，封装）

**问题**：`mod.rs:12 pub struct DerivedKey(pub Zeroizing<[u8; 32]>)`——字段 pub，任何持有 DerivedKey 的代码可经 `.0` 直接读原始 32B key 字节，绕过 `as_bytes` 受控接口。Zeroizing 清零保护未被破坏（Zeroizing 仍在 DerivedKey 内，drop 时清零），但读取不受控——若某处把 `.0` 字节拷到非 Zeroizing 缓冲（log/String），绕过清零。

**修复**：字段改 private + 新增 `pub(crate) fn from_zeroizing`（crate 内生产构造点用，如 KDF 派生/hierarchy 子 key）+ 保留 `pub fn from_raw`（外部 crate 测试用）。所有直接 `DerivedKey(...)` 构造点（vault 内 6 处测试 + kdf/hierarchy 生产 + desktop 2 处测试）改用 `from_raw`/`from_zeroizing`。

### K3-derive: Argon2Params derive(Deserialize) 暴露绕过校验构造能力（低，未来风险）

**问题**：`kdf.rs:13 #[derive(Serialize, Deserialize)]`。grep 确认当前无任何反序列化调用方（所有构造走 from_i64 / Default）。Deserialize 暴露了"可构造 iterations=0 / 弱参数 Argon2Params 绕过 from_i64 校验"的能力——若未来加配置加载/API 接收 JSON 的路径，会静默绕过校验。

**修复**：移除 Deserialize derive，保留 Serialize（未来可能用于配置导出，且不暴露构造能力）。注释标注：若未来确需反序列化，必须构造后立即用 from_i64_strict 复校验。

### S2: AES-GCM encrypt 未用 AAD 绑定上下文（低，文档化）

**问题**：`symmetric.rs:23 cipher.encrypt(nonce, plaintext)` 不传 AAD。密文不绑定 cipher_id / field_name 上下文——攻击者若能写 DB，可把 cipher A 的 password 密文复制到 cipher B 的 password 字段，decrypt 仍成功（同 user_vault_key），cipher B 显示 cipher A 的密码（密文移动/重放）。

**状态**：文档化。单机威胁模型下"能写 DB = 攻击者已赢"，AAD 是纵深防御而非必需。加 AAD 需改 encrypt/decrypt 签名 + 所有调用点 + 密文格式（破坏向后兼容），代价大收益低。已在 symmetric.rs 模块注释说明设计取舍。

---

## 第二十一轮审查修复（2026-07-25，importer/types/matcher：I8/I-FOLDER-WARN + 4 项文档化）

importer/types/matcher 整体质量高——MatchType 协议对齐、#11 空 URI 防御、Rust regex 免疫 ReDoS、M4 非法值可观测均经源码确认。

### I8: 孤儿 folder 残留——M7 补偿未覆盖"batch 空成功"（中低，真实 bug）

**问题**：`bitwarden.rs:298-307` 的 M7 补偿删除仅在 `insert_vault_ciphers_batch` 返回 Err 时触发。但 `db.rs:3799 insert_vault_ciphers_batch(&[])` 对空 batch 直接 `tx.commit()` 返回 `Ok(())`——不进 Err 分支，不补偿。

**触发链**：用户从 Bitwarden 全量导出（含 SecureNote/Card/Identity），octopus 只支持 Login（type=1）→ items 全 skip（:220 `item_type != 1`）→ batch 空 → `Ok(())` → folder 循环（:188-211）已创建的 N 个 folder 全残留为孤儿（无任何 cipher 引用）。用户会在 folder 列表看到一堆空文件夹且无法理解来源。

**修复**：batch 成功后，扫描 `created_folder_ids`，删掉没被 batch 里任何 cipher 引用的 folder。覆盖所有孤儿场景（batch_len==0 全孤儿 + batch_len>0 部分孤儿）。

**回归测试**：`test_import_all_items_skipped_no_orphan_folders`（全 skip → folder 不残留）+ `test_import_partial_orphan_folders_cleaned`（部分孤儿 → 只清未引用的）。

### I-FOLDER-WARN: folder 创建失败静默（低，可观测性）

**问题**：`bitwarden.rs:206-209` create_folder 失败仅 `log::warn!`，不记入 errors/skipped。引用该 folderId 的 cipher 的 folder_id 静默降级为 None（folder_map 无此 id → get 返 None）→ cipher 仍导入但丢失文件夹归属，用户不知情。

**修复**：失败时除 log 外，记入 `errors` 让导入报告可见。

### M-REGEX: RegularExpression 正则每次匹配重新编译（低，文档化）

**问题**：`matcher.rs:87 Regex::new(cipher_uri)` 每次 `find_matching_ciphers` 调用都重新编译正则（构建 NFA/DFA + 堆分配）。

**状态**：文档化。无 ReDoS（Rust regex crate 线性时间引擎，无 catastrophic backtracking）、无 panic（`unwrap_or(false)`）。绝大多数用户 0 个 regex cipher → 零成本。优化需调用方在查询热路径加 `HashMap<uri, Regex>` 缓存，matcher 本身无状态不好缓存。

### I-DEDUP-PERF: 导入为 dedup 全量解密库内 cipher（低，固有限制）

**问题**：`bitwarden.rs:157 storage::list_ciphers(key)` 每次导入全量解密库内所有 cipher（name + data + fields + history），仅为算 (name, first_uri) dedup key。

**状态**：文档化。AES-GCM nonce 随机致同明文不同密文，无法用密文去重，必须解密。Bitwarden 式字段级加密的固有代价。大库（数百+ cipher）+ 频繁导入时明显变慢，可考虑维护解密缓存或导入时增量比对。

### I-DOSSIZE: 导入无输入大小限制（低，文档化）

**问题**：`bitwarden.rs:146 serde_json::from_str(json)` + :219 `items.iter()` 对超大 JSON / 超大 items 数组无上限。恶意/损坏的 .json 可致 OOM 崩溃。

**状态**：文档化。单机用户文件、非网络输入，威胁低。可加 items 数量上限（如 10 万）+ JSON 字节上限作为廉价纵深防御，暂不实施。

### 正面发现（importer/types/matcher）

- **MatchType 协议对齐**：types.rs:92-99 StartsWith=2/Exact=3 与 Bitwarden 官方 UriMatchType.cs 对齐（之前弄反致导入/导出 match 语义静默互换），守护测试锁住官方值
- **M4 非法值可观测**：types.rs:38/68 CipherType/RepromptType 非法值兜底时 log::warn! 记迹（仍兜底为数据兼容，单机威胁模型诚实标注）
- **#11 空 URI 视 Never**：matcher.rs:66-68 `cipher_uri.trim().is_empty()` 提前返回 false，挡住 `starts_with("")` 恒真与 `Regex::new("")` 恒真两类误匹配
- **L12 大小写归一**：matcher.rs:78/105-112 Host 策略与 Domain 策略 to_lowercase，DNS host 不区分大小写
- **Rust regex 免疫 ReDoS**：matcher.rs:87 `Regex::new` + `unwrap_or(false)`——regex crate 线性时间引擎，无 catastrophic backtracking，无效正则不 panic

---

## 第二十二轮审查修复（2026-07-25，generator/ 全模块：G-EFF-NOGUARD/G-YOYO-COLLIDE/R8/R5 + P4 文档化）

generator/ 是密码生成器命门（弱随机=可爆破密码）。本轮处理词表守护缺失、注释措辞、数据结构低效。

### G-EFF-NOGUARD: EFF 词表零守护测试（中低，与 zh 词表不对称）

**问题**：`eff_wordlist.rs` 7776 行静态 const 零测试（zh_wordlist_4096 有 3 个守护：size/no_duplicates/all_two_cjk）。误删几行/引入重复词/编辑引入异常字符，CI 无任何守护，静默降熵。

**修复**：在 `passphrase_en.rs` 测试模块补 3 个对齐 zh 词表的守护：
- `test_eff_wordlist_size_7776`：大小恰好 7776
- `test_eff_wordlist_no_duplicates`：原始词无重复
- `test_eff_wordlist_no_dedash_collision`：去连字符后无新增碰撞（除已知 yo-yo/yoyo）

### G-YOYO-COLLIDE: yo-yo/yoyo 去连字符碰撞，注释措辞不严谨（很低，信息性）

**问题**：EFF 词表同时含 "yo-yo"（:7757）和 "yoyo"（:7762）。`passphrase_en.rs:34 w.replace('-', "")` 把 yo-yo→yoyo，与已有 yoyo 碰撞 → 实际唯一输出版 7775。但注释 :30-33 声称"保持源词熵不变"不严谨。

**修复**：注释改为如实表述——熵损 = log2(7776/7775) ≈ 0.000186 bit/词，3-10 词总熵损 < 0.002 bit，可忽略；非 octopus 引入（EFF 官方词表固有）。`test_eff_wordlist_no_dedash_collision` 锁住此已知碰撞。

### R8: 字符集 &[&str] 致每次 generate 多次 concat（低，性能）

**问题**：`random.rs:10-22` UPPER/LOWER/DIGITS/SYMBOLS 是 `&[&str]`，`build_charset:28-37` 每次调用 4 次 `concat()` 堆分配拼 String 再 `.chars()`。字符集静态已知。

**修复**：改 `&[char]` 常量，`build_charset` 用 `extend_from_slice` 零分配。强制类型选择逻辑同步简化（直接 choose 拿 char，无需 `.chars()` 转换）。

### R5: random.rs:65 唯一 unwrap（低，整洁性）

**问题**：`UPPER.choose(&mut rng).unwrap()` 是强制类型选择里唯一 unwrap（:72/82/92/102 都用 if let Some）。UPPER 非空 choose 必返 Some 不 panic，但风格不一致。

**修复**：R8 改造时一并统一为 `if let Some`。

### P4: include_number/symbol 固定末尾位置（低，文档化）

**问题**：`passphrase_en.rs:50-61` / `passphrase_zh.rs:35-40` 追加的数字/符号总在末尾（非随机位置）。攻击者知道位置略降熵。

**状态**：文档化。单字符位（log2(10)≈3.3 / log2(7)≈2.8 bit）影响极小，且位置固定便于用户识别。设计权衡。

### 正面发现（generator/）

- **OsRng CSPRNG**：random.rs:57 / passphrase_en.rs:24 / passphrase_zh.rs / pin.rs 均用 `OsRng`（OS 熵源），非弱 `thread_rng`
- **#8 Zeroizing 中间材料**：random.rs:61 / passphrase_en.rs:27 生成过程的中间 Vec/String 用 Zeroizing，函数返回时清零 heap
- **长度边界校验**：random.rs length 5..=128 / passphrase_en word_count 3..=10 / pin 有上限
- **avoid_ambiguous**：random.rs:39-41 过滤 l/1/I/O/0 等易混淆字符
- **zh 词表守护扎实**：size 4096 + no_duplicates + all_two_cjk（EFF 词表现在对齐补齐）

---

## 第二十三轮审查修复（2026-07-25，health/ 全模块：S-THRESHOLD/D5 + D-NOSALT 文档化）

health/ 含 zxcvbn 强度评估 + 重复密码检测。演进修复扎实（8.5→M1→N1 超长密码处理）。

### S-THRESHOLD: 超长路径 entropy_score 阈值无依据注释（低，可观测）

**问题**：strength.rs:54-64 超长路径（>1KB）的 entropy_score 分段阈值 28/36/60/128 bit，比 zxcvbn 正常路径的 Score 边界（log2 换算约 6.6/13.3/19.9/26.6 bit）高得多。注释未说明来源（OWASP？NIST？经验值？），后续维护者无法判断合理性。

**修复**：补注释说明阈值依据——超长路径的 `independent_entropy`（char_count × log2(unique)）与 zxcvbn 的 guesses（实际攻击成本）度量不同：independent_entropy 假设每字符独立，但超长密码常有重复/模式 → 系统性高估，需更高阈值达到同等安全保证。这些是经验值（超长密码超出 zxcvbn 设计范围，无权威阈值），实践中中间地带少，最终 score 基本由 pattern_score 决定。

### D5: duplicate_groups 顺序不确定（低，UX）

**问题**：duplicate.rs:55 `HashMap.into_iter().collect()` 组间顺序不确定（HashMap 迭代序随机）。组内 cipher_ids 顺序确定（按遍历 push），但组间无序——健康报告的重复组列表每次刷新可能变。

**修复**：收集后按首个 cipher_id 排序。回归测试 `test_duplicate_groups_order_stable`（20 次循环验证 3 组顺序固定为 c1<c3<c5）。

### D-NOSALT: 重复检测无盐 SHA-256（信息性，设计正确）

**问题**：duplicate.rs:47-49 对 password 算无盐 SHA-256 用于内存分组。

**状态**：设计正确，非缺陷。重复检测的固有需求——加盐会让相同明文产生不同哈希，破坏"相同密码→同组"语义。已有充分缓解：hash 仅内存（不持久化）+ `#[serde(skip)]` 不跨 IPC + Debug redact（#12）。唯一边际增强是 peppering（HMAC-SHA256(pepper)），但 pepper 须存内存（与 hash 同级泄露则失效），收益边际，不实施。

### 正面发现（health/）

- **zxcvbn 集成**：strength.rs 用 zxcvbn 做模式识别（重复/循环/键盘序列/字典词），比纯熵公式更准
- **超长密码演进**：8.5（char×6.0 误报）→ M1（unique.log2×count 堵 unique=1）→ N1（取前 256 字符跑 zxcvbn 做模式识别 + 完整长度估熵取较低者）
- **H2 entropy_bits 一致性**：zxcvbn 识别到低熵模式时，entropy_bits 用 score 对应上限，避免「2048 bit 却 score=0」矛盾显示
- **L6 软删过滤**：duplicate.rs:40 跳过 deleted_at 的 cipher
- **#12 Debug redact**：DuplicateGroup 手写 Debug 对 password_hash redact
- **H1 签名优化**：find_duplicates 收 `&[&Cipher]` 避免调用方深拷贝

---

## 第二十五轮审查修复（2026-07-25，sync/outline + store + engine 删除传播：M-TOMBSTONE/M-DEAD + ISO-COMMENT）

### M-TOMBSTONE: sync 不传播硬删，密码多设备复活（高，= 已知 M5，严重度升级 + 佐证细化）

**核查**：三条独立佐证链全部回源码确认成立——

1. **push 侧会删**（store.rs:580-586）：本地 DB 硬删 cipher 后，`incremental_export` 读 `old_outline` 有该 uuid 但 `cipher_id_set`（当前 DB）无 → `delete_cipher_file` + 新 outline（:556-578 只 insert 现存 cipher）不含该 uuid → push 后远程文件删、outline 移除。✓
2. **pull 侧不删**（engine.rs:800-819）：`pull_from_files` 的 apply 只 `for (uuid, entry) in &remote_outline.ciphers` 做 upsert——**无「本地有 remote 无 → 删本地」对称逻辑**。remote_outline 无 c1 → 循环不碰 c1 → B DB 的 c1 保留。✓
3. **旁证**：`SyncReport.deleted`（engine.rs:570 字段 / :736 初始化 0）grep 全文件无任何递增或重新赋值点 → sync 从不统计删除、从不删除 cipher。✓

**双向复活路径**：A 硬删 c1 → A push（远程删）→ B pull（remote_outline 无 c1 → apply 不碰 → B DB 保留 c1）→ B push（B incremental_export 读 DB 含 c1 → 写回 c1 文件 + outline）→ 远程 c1 复活 → A pull → c1 在 A 复活。

**与第四轮 M5 的关系**：M-TOMBSTONE **就是**第四轮已记录的 M5。本轮提供更完整的三条独立佐证链（第四轮只记录了时序描述），并升级严重度（中→高）：密码管理器的「删除」必须可靠传播，硬删复活违反核心承诺 + 安全影响（敏感密码存活于 git 历史）。

**触发条件**：① 多设备 sync；② empty_trash 硬删（清空回收站）。单设备 / 仅软删不受影响——软删通过 md5 变化正确传播（H2 修复保证 deleted_at 跨设备一致）。

**修复方向**（需 Phase 2，本轮不改代码）：
- 方案 A（tombstone）：硬删时 outline 写墓碑 entry（uuid + deleted 标记 + 时间），pull 侧识别墓碑删本地。需 outline 格式升级（version 2）。
- 方案 B（pull 侧对称删除）：apply 增加「本地有 remote 无 → 删本地」。需防误删（remote 是旧的、未收到对方新增 push 时）——需 vault_version 或 merge 状态保护。
- 方案 C（文档化为设计选择）不可取——密码管理器的「删除」必须可靠传播。

### M-DEAD: merge_outlines 仅 re-export 生产零调用（低，死代码，与 M-TOMBSTONE 同源）

**核查**：`merge_outlines`（outline.rs:61）定义完整、6 个测试覆盖，`lib.rs:33` 与 `vault/sync/mod.rs:37` 各 `pub use` 导出——但 grep 全仓生产代码无任何实际调用点（engine.rs pull 直接用 remote_outline 驱动 apply，不经 merge）。

**与 M-TOMBSTONE 的关系**：这解释了 M-TOMBSTONE 的成因——原设计意图是 LWW merge（outline.rs:60 注释「取 updated_ms 更新者」），但 pull 侧实现改成直接 remote_outline apply（简化），导致 merge 逻辑没接线、删除也不传播。且 merge_outlines 语义本身也有 M-TOMBSTONE 同源缺陷（:62 `local.clone()` + 只遍历 remote → 「本地有 remote 无」一律保留）。

**状态**：文档化。要么按方案 B 接线（merge 后驱动 apply + 加删除传播），要么删除避免误导。取决于 Phase 2 方案选择。

### ISO-COMMENT: iso_to_unix_ms 注释自相矛盾（低，注释，已修）

**问题**：`sync/store.rs:143-145` 同一函数内注释打架——:143-144「简化天数累积——不考虑闰年精度...准确算法需要完整日历库，不值得」，:145「这里用 civil_to_days 公式（Howard Hinnant），精度无损」。代码实际是完整的 Howard Hinnant civil_to_days（:146-151 era/yoe/doy/doe 分解，正确处理闰年）。:143-144 是被淘汰的旧简化方案残留注释。

**修复**：删 :143-144 旧残留注释，保留 :145 正确描述。

### 正面发现（store.rs / outline.rs）

- **BTreeMap 序列化稳定**：outline.rs:9-12 用 BTreeMap（非 HashMap）保证 outline.json 字节级稳定，避免 git 空 commit
- **iso_to_unix_ms 精确**：civil_to_days 公式正确处理闰年（era/yoe/doy/doe 分解）
- **incremental_export md5 diff 正确**：store.rs:558-578 只写变化的 cipher
- **M8 outline 损坏降级全量重建**：store.rs:537-552 outline.json 解析失败不再 `unwrap_or_default` 吞成空（会导致删除循环不执行 + clone 复活），降级 `export_all_to_files`

---

## 第二十六轮审查修复（2026-07-25，matcher/psl.rs + matches_domain：3 项文档化，无中高 bug）

psl.rs + matches_domain 审查通过——vault 里安全设计最严谨的模块之一。本轮发现均为低/信息性，无功能或安全缺陷，全部文档化。

### P-LAZY: psl() 首次调用 expect panic 风险（低，文档化）

**核查**：psl.rs:33-34 `List::from_bytes(PSL_BYTES).expect("...解析失败")` 在 `OnceLock::get_or_init` 闭包内。`PSL_BYTES` 是 `include_bytes!` 编译期内嵌（:26），正常解析不会失败。但若 dat 文件被手动下载时截断/损坏，`psl()` 首次调用（用户首次 autofill 时，经 `etld_plus_one` → `matches_domain`）会 panic 崩溃而非 fail-closed 退化。

**状态**：文档化。编译期内嵌正常不触发（dat 是 git 仓库内静态文件，不运行时损坏）。可选改进：vault init/unlock 后预热调一次 `psl()`（fail-fast 在启动期暴露），或 `OnceLock<Option<List>>` + fallback host 本身（fail-closed）。当前不改——改了增加复杂度，收益边际。

### P-EXPIRE: 内嵌 PSL 会过期（信息性，已文档化）

**核查**：psl.rs:18-25 注释已明确——`include_bytes!` 编译期内嵌，升级 publicsuffix crate 不会自动更新列表，需手动 `curl` 重新下载 `public_suffix_list.dat`，Mozilla 月更建议季度同步。

**状态**：已文档化。过期后果：新上线 TLD 的多段规则不被识别 → fail-closed 退化为 host 本身（功能退化非安全）。可选：加 CI 检查 dat 的更新日期。

### 测试覆盖缺口: PSL 边缘规则未覆盖（低，文档化）

**核查**：psl.rs 测试覆盖核心场景（简单域名/localhost/多段 TLD 钓鱼/IP），但未覆盖 wildcard rule（`*.kawasaki.jp`）/ exception rule（`!parliament.uk`）/ IDN/Punycode。

**状态**：文档化。这些由 publicsuffix crate 内部正确处理（crate 自有测试），真实 autofill 场景少。可选加几个守护测试防 crate 升级回归。

### 正面发现（psl.rs / matches_domain）

- **PSL 替代简化算法堵钓鱼**：首发版「取最后两段」让 `barclays.co.uk` 与 `evil-attacker.co.uk` 都退化为 `co.uk` 互相匹配 → 钓鱼站可收银行密码。现用 `publicsuffix` crate 的 `DefaultProvider` 正确处理多段 TLD
- **IP 字面量精确匹配**：psl.rs:55 `host.parse::<IpAddr>().is_ok()` → 原样返回，不做 eTLD+1（否则 `192.168.1.1` 与 `10.20.1.1` 都退化为 `1.1` 互相匹配 → 路由器密码钓鱼）
- **fail-closed 设计**：PSL 查不到（localhost/内网单段名/未知 TLD）→ 返回 host 本身（宁可匹配失败也不要错匹配）
- **matches_domain 三重大小写归一**：matcher/mod.rs:78（Host 策略）+ :105-112（Domain 策略 cipher_host/target_domain）+ :118（等价域名组）全部 to_lowercase
- **等价域名条件扩展**：matcher/mod.rs:116-123 cipher_domain 在组内时，组内所有域名加入 candidates
- **钓鱼测试守护**：test_phishing_protection_multilevel_tld 锁住 `barclays.co.uk ≠ evil-attacker.co.uk`

---

## 第二十七轮审查修复（2026-07-25，meta_lock + cipher 写并发：M-CIPHER-RMW）

### M-CIPHER-RMW: save_cipher load→update 无锁无事务，与 #4 同构（低，已修）

**核查**：save_cipher（cipher.rs:86-112）流程——:90 `db::load_vault_cipher(id)?`（第 1 次 with_db autocommit 读）→ :87-108 encrypt + 构造 db_input → :111 `db::update_vault_cipher(id, &db_input)?`（第 2 次 with_db autocommit 写）。两步跨两个 autocommit 事务，中间无锁——与 #4（meta_lock 修复的 meta 双 modal 并发 RMW）完全同构。

**并发损坏场景**：save_cipher :90 读到 `deleted_at=None` → 间隙内 `soft_delete("c1")` 改成 ts → :111 update 写回 `deleted_at=None`（save_cipher 读到的旧值，H2「保留」语义反而覆写）→ 软删被撤销，cipher 复活。这与 H2 修复初衷（编辑时保留删除状态）在并发下直接冲突。

**与 #4 的关系**：meta_lock.rs:5 注释明确「Tauri 同步命令被 spawn_blocking，可在不同 worker 并发执行」——这一前提对 cipher 命令同样成立，但 #4 的保护只施加到 meta。cipher 的 soft_delete/restore（S1 修复，单事务原子 ✓）都是好设计，唯独 save_cipher 的 deleted_at 保留 RMW 是 #4 的覆盖盲区。

**修复**：save_cipher 的 load→update 合并进单事务（`with_db` + `unchecked_transaction`），load 用 `load_vault_cipher_at(&tx)`，update 用 `update_vault_cipher_at(&tx)`。事务隔离保证 load 读到的 deleted_at 与 update 写的在同一快照内一致。`update_vault_cipher_at` 改 pub（与 `load_vault_cipher_at` 对称，后者已 pub）。

**严重度诚实限定**：触发条件苛刻——需 ① 两个 Tauri 命令并发操作同一 cipher；② 交错恰好落在 load→update 的微秒级间隙（其间有 encrypt + md5 计算）。单用户 UI 下极难触发。但形态与 #4 完全一致，轻量事务加固消除盲区。

**回归测试**：`save_cipher_preserves_soft_deleted_state`——先 soft_delete → 再 save_cipher → deleted_at 仍非空（不复活）。

### 正面发现（meta_lock.rs）

- **ReentrantMutex 设计正确**：同线程可重入（外层 change_master_password 持锁 → 内层 save_vault_meta 再 lock 不死锁），这是锁下沉到写函数内部的前提
- **锁下沉写函数**：save_vault_meta / update_security_stamp 内部自动加锁，覆盖所有 meta 写路径（不依赖调用方显式 acquire）
- **双测试守护**：test_lock_serializes_concurrent_writers（4 线程并发串行化）+ test_lock_is_reentrant_same_thread（同线程重入不死锁）

---

## 第二十八轮审查修复（2026-07-25，generator 模块：R-AMBIGUOUS-DEAD + R-UPPER-BRANCH-ASYMMETRY）

generator 模块审查通过——四个生成器（random/passphrase_en/passphrase_zh/pin）+ mod 配置，CSPRNG/边界/Zeroizing/词表守护均到位，无实质 bug。仅 2 个低/信息性观察。

### R-AMBIGUOUS-DEAD: AMBIGUOUS 含 4 个永不命中的死字符（信息性，已修）

**核查**：random.rs:25 `AMBIGUOUS` 列 9 字符 `l 1 I O 0 | ` ' "`。其中 `|` `` ` `` `'` `"` 不在 UPPER/LOWER/DIGITS/SYMBOLS 任一字符集（SYMBOLS 是 `!@#$%^&*()-_=+[]{}<>?`，无这 4 个）。`build_charset` 的 `retain` 和强制阶段 `filter` 对这 4 个永远是 no-op——它们本就不会被生成。

**修复**：从 AMBIGUOUS 删除 4 个死字符，保留 5 个真正有效的（l/1/I/O/0 在字符集内会被过滤）。选择「删除」而非「加入 SYMBOLS」——因为加入会改变密码生成行为（之前不生成这些，之后会生成除非 avoid_ambiguous），删除是纯清理零行为变化。

### R-UPPER-BRANCH-ASYMMETRY: uppercase 双分支 vs 其余统一 filter（低，风格，已修）

**核查**：uppercase 用 `if cfg.uppercase && !cfg.avoid_ambiguous { UPPER.choose } else if cfg.uppercase { filter }` 双分支；lowercase/numbers/symbols 统一用 `filter(|c| !cfg.avoid_ambiguous || !AMBIGUOUS.contains(c))`。两种写法语义等价但结构不一致。

**修复**：uppercase 改统一 filter 写法，与其余三个对齐。

### 正面发现（generator 全模块）

- **CSPRNG 一致**：四个生成器全用 OsRng，SliceRandom::choose/shuffle（Fisher-Yates 无偏）、gen_range 边界正确
- **Zeroizing 中间材料**：random（Zeroizing<Vec<char>>）/ pin（Zeroizing<String>）/ en（Zeroizing<Vec<String>>）/ zh（Zeroizing<String> result）
- **词表三重守护**：EFF 7776 + ZH 4096 size 守护 + 无重复守护；EFF 额外 no_dedash_collision；ZH 额外 all_two_cjk_chars
- **边界 ensure**：random 5..=128 / pin 1..=32 / en 3..=10 / zh 3..=8
- **强制每类型至少 1 个 + avoid_ambiguous 正确过滤**：强制类型数（≤4）< length 下限（5），不超长
- **zh words 不 zeroize 合理**：Vec<&'static str>（指针指向静态段，无堆明文拷贝），核心明文 result 已 Zeroizing
- **mod.rs serde tag=camelCase 对齐前端**

---

## 第二十九轮审查修复（2026-07-25，sync/engine.rs 同步核心：P-MD5-LINEAR-SCAN + P-FOLDER-SCAN）

engine.rs 同步核心安全设计严谨——E2 防锁死、stamp 校验前置、保留本地密钥、H2 不复活、#10 不静默吞、resolve 密码验证均正确。2 个性能发现。

### P-MD5-LINEAR-SCAN: md5 比对线性扫描，已有 HashSet 未复用（低-中，已修）

**核查**：pull_from_files 的 md5 比对是 O(M×N)。`db_cipher_ids`/`db_folder_ids` HashSet（:766-769）已构建用于 exists 判断（O(1)），但 `cipher_md5_mismatch`/`folder_md5_mismatch`（:905/:916）接收 `&[VaultCipher]` Vec 用 `.iter().find()` 线性 O(N)。外层 for × 内部 find = O(M×N)，clone 时 M≈N → O(N²)。

**修复**：HashSet 升级为 `HashMap<&str, &str>`（id → sync_md5）。exists 用 `contains_key`（O(1)），md5 比对用 `get` 拿 md5（O(1)），整体降到 O(M)。`cipher_md5_mismatch`/`folder_md5_mismatch` 签名改为接收 `&HashMap`。

**影响评估**：N（密码条数）普通用户几十、重度几百。N=500 时 N²=250k 次字符串比较 ≈ 亚秒级，企业库几千条才到秒级。实际影响有限，但 O(N²) 不必要且修复极简。

### P-FOLDER-SCAN: upsert_folder 全表扫描，db 缺单条 API（低，已修）

**核查**：`upsert_folder_with_sort`（:548-551）判断 folder 存在用 `list_vault_folders().iter().any(|f| f.id == id)` 全表扫。每次 upsert 都 O(N) → pull folder 循环 O(N²)。与 `upsert_cipher`（:527 用单条 `load_vault_cipher`）不对称。db.rs 无 `load_vault_folder(id)` 单条 API。

**修复**：db 层加 `load_vault_folder(id)`（SELECT WHERE id=? 单条查询），`upsert_folder_with_sort` 改用。folder 数量通常远少于 cipher，影响小于 P-MD5-LINEAR-SCAN，但与 upsert_cipher 对称。

### M-TOMBSTONE 仍在（已知 M5 + folder 维度同构）

pull_from_files :800/:822 只遍历 remote outline 做 upsert，无删除分支——远程硬删除的 cipher/folder 在 pull 端不删本地。本轮确认仍在，且 folder 维度同构（folder 硬删除同样不传播）。待 Phase 2 统一处理 tombstone（详见第二十五轮 M5）。

---

## 第三十轮审查修复（2026-07-25，health 模块：R-AVG-DENOM + P-DOUBLE-TRAVERSE 文档化）

health 模块整体质量高——L6/H1/D5/#12/N1/M1/H2/空密码兜底全到位。2 个低优先级发现。

### R-AVG-DENOM: average_score 分母与 total_logins 不一致（低，UX 语义，已修）

**核查**：generate_report（mod.rs:46-58）——`total_logins: logins.len()`（含 password=None 的 Login），但 `average_score: total_score / score_count`（score_count 只算 password=Some）。logins 的 filter（:22）只判 `CipherData::Login(_) && deleted_at.is_none()`，不要求 password=Some。Bitwarden 式密码管理器允许「只存 username 不存 password」的 Login——这些进 total_logins 但不进 score_count，UI 显示「10 个登录平均分 3.2」实际是 8 个的。

**修复**：HealthReport 加 `scored_count` 字段透明化 average_score 的真实分母（方案 a，不改现有语义只补透明度）。前端 HealthReportDto 加 optional `scored_count`，average_score 展示旁当 `scored_count < total_logins` 时标注「基于 N 个有密码项」（i18n en/zh）。

**回归测试**：`test_scored_count_excludes_none_password`——3 个 Login（2 有密码 + 1 无密码），total_logins=3 但 scored_count=2。

### P-DOUBLE-TRAVERSE: generate_report 双重遍历 logins（低，性能，文档化）

**核查**：generate_report 对 logins 遍历两次——:29-42 算 strength（zxcvbn）+ :46 find_duplicates 内部再遍历算 SHA-256。可合并为单次遍历。

**状态**：文档化。zxcvbn（O(n²) 中等密码）是绝对瓶颈，单次遍历合并省的只是 N 次指针解引用，相对可忽略。几百个 login 的健康报告是用户主动触发的一次性操作。若未来做成后台定时扫描再考虑。

---

## 第三十一轮审查修复（2026-07-25，sync/store.rs 文件读写层：R-IMPORT-NOFAULT-TOLERANT）

### R-IMPORT-NOFAULT-TOLERANT: clone 路径单文件损坏中止 + 连锁死锁（中，健壮性/一致性，已修）

**核查**：三条证据全部回源码确认——

1. **import 中止**：`import_all_from_files`（store.rs:644-672）对损坏文件直接 `?` 中止——:652 `read_to_string(...)?` + :654 `serde_json::from_str(...)?`。单文件 read/parse 失败 = 整个 import Err。✓
2. **连锁放大**：`clone_initial`（engine.rs:462）`db::upsert_vault_meta` 在 :465 `import_all_from_files()` **之前**执行。import 失败 → DB 半初始化（有 meta 无 cipher）→ 重试 clone → :419 E2 守卫 `load_vault_meta().is_some()` 拒绝 → **死锁**。用户必须手动清 vault_meta + cipher 表。✓
3. **不对称**：pull 路径（engine.rs:813-825）`read_cipher_file` 失败 `log::warn + skipped += 1`（#10 容错）；hotword 导入（engine.rs:478-491）`log::warn`「不阻断 vault clone」；唯独 vault cipher/folder 导入（:465 `import_all_from_files()?`）不容错。三者处理同类「外部文件导入」却三种策略。✓

**修复**：import_all_from_files 的 cipher/folder 循环改容错——单文件 read/parse 失败 `log::warn` 跳过，不中止（与 pull #10 + hotword 模式对齐）。容错后连锁问题自然消失（import 不再整体失败 → 不会留半初始化状态）。

**回归测试**：`import_all_from_files_skips_corrupt_file`——写 2 个正常 cipher + 1 个损坏 JSON，验证 import 返回 2 个（损坏跳过）。

### 信息性（不单独开项）

- **incremental_export changed 虚高**：delete_cipher_file 对 NotFound 返 Ok，删除循环 changed += 1 即使文件早不存在 → vault_version 可能不必要 +1。但新 outline 不含 stale uuid，每次 stale 只触发一次，影响极小。
- **export_all_to_files remove_dir_all 非原子**：清空 ciphers/ folders/ 后写文件，中间失败留半空目录。但 SQLite 是真相源，重新 sync 自愈；且主要在 push_initial（ciphers/ 空）跑。低。

---

## 第三十二轮审查修复（2026-07-25，crypto 模块：K1-GAP + C-ZEROIZE-FEATURES 文档化）

### K1-GAP: clone/pull 远程 KDF 参数未 strict 校验，与 K1 设计意图不一致（中，安全，已修）

**核查**：`from_i64_strict` grep 确认**只有** `resolve_with_remote`（engine.rs:970，stamp 冲突罕见分支）调用。主路径漏防：
- `clone_initial`（:452）meta upsert 直接 `kdf_memory_kib: f.kdf_memory_kib`（远程原值，无校验）
- `pull_from_files`（:876）同样无校验

这是我第二十轮 K1 修复的覆盖盲区——strict 校验只接到罕见分支（stamp 冲突），常规 clone/pull 才是日常同步主路径，却漏防。

**攻击链**：攻击者污染私有同步库 meta.json 为 `kdf_memory_kib=8` → 受害者 clone/pull → 弱 KDF 写入本地 DB（stamp 校验通过≠KDF 强度校验）→ unlock 用 from_i64（崩溃下限 memory≥8）接受 → 用户用废掉内存硬度的 Argon2id 无感知。需攻击者能改私有库（中等前提）+ 另获本地 DB 才完整利用，但 K1 设计意图明确是防此场景。

**修复**：clone_initial（:445 后）+ pull_from_files（:861 后）在 `to_sync_fields()` 后、构造 VaultMetaInput 前，加 `Argon2Params::from_i64_strict` 校验。失败返 Err 拒绝同步。补齐 K1 防御主路径。

**回归测试**：`pull_rejects_weak_kdf_params`——写 stamp 一致但 memory_kib=8 的 meta.json，验证 pull 返 Err 且本地 DB 不被污染（kdf_memory_kib 仍 65536）。

### C-ZEROIZE-FEATURES: argon2/aes-gcm/hmac 未启用 zeroize feature（低-中，文档化）

**核查**：Cargo.toml 确认 argon2/aes-gcm/hmac 都缺 zeroize feature，唯独 generic-array 启了（A2 修复）。但核实三个库的 `[features]`——**argon2 0.5 / aes-gcm 0.10 / hmac 0.12 都不提供 zeroize feature**，加不上。

**状态**：文档化。与 N2（aes 0.8 无 zeroize feature）同型已知限制。报告假设这三个库像 generic-array 一样有 zeroize feature 可启，但实际没有。修复需 fork 或升级库版本。argon2 的 64MiB memory blocks 残留是最大量级，但逆推 Argon2 memory blocks ≠ 廉价爆破（不可逆填充阵列）。

---

## 第三十三轮审查修复（2026-07-25，generator 模块：G-EN-RESULT-NO-ZEROIZE）

### G-EN-RESULT-NO-ZEROIZE: passphrase_en 最终明文 result 未用 Zeroizing（低，一致性，已修）

**核查**：同模块四生成器最终明文容器对比——random（`Zeroizing<Vec<char>>`）/ passphrase_zh（`Zeroizing<String>`）/ pin（`Zeroizing<String>`）/ passphrase_en（普通 `String`）。en 独漏：words 中间词数组（:27）包了 Zeroizing，但 `words.join()` 产出的最终明文副本 result（:55）没包。

**修复**：照 passphrase_zh :31/:35/:43 模式——:55 改 `Zeroizing::new(words.join(...))`，:58 format 覆盖改 `*result = format!(...)`，:69 改 `Ok(result.as_str().to_string())`。

**严重度诚实限定**：result 唯一存活区间是 join(:55) 到 return(:69)，其间只有同步 format!，无显式 panic 路径（除 OOM），Zeroizing「异常不残留」的实际收益边际。返回给 Tauri IPC 的 String 本就是明文（四生成器同样），Zeroizing 只保护中间容器。但模块内一致性缺口明显（4 个生成器 3 个包了，en 独漏），且注释 :25-26 宣称「中间材料用 Zeroizing」却漏了最终拼接结果，值得为一致性补齐。

---

## 第三十四轮审查修复（2026-07-25，unlock.rs 密钥解锁主链路：B-UNLOCK-RECORD-ASYMMETRY + B-SETUP-CRASH-WINDOW 文档化）

unlock.rs 密钥管理设计扎实——密钥清零链完整、M3/B1/B2/A1/#8/INV-7/#14 修复到位、K1-GAP 下游确认（unlock 用 from_i64 处理本地可信参数）。

### B-UNLOCK-RECORD-ASYMMETRY: unlock 把数据损坏误计为密码错退避（低，逻辑不对称，已修）

**核查**：unlock_with_master_password 把 protected + sync 解密捆绑在闭包（:204-222），闭包内任一失败 → :229 record_failure + :235 context「主密码错误」。而 change_master_password 精确区分：:282-288 protected 解密失败才 record_failure（密码错）；:289 长度异常 + :295 sync 解密失败用 `?`/`ensure` 直接 return，不 record_failure（数据损坏 ≠ 密码错）。

**后果**：sync_enc 单字段损坏（protected 完好、密码正确）→ :205 解 protected 成功 → :212 解 sync_enc 失败 → 闭包 Err → record_failure + 退避计数 + 误显「主密码错误」（实际密码正确）。

**修复**：把闭包拆成三阶段（与 change :282/:295 结构 1:1 对齐）：
1. 密码校验：仅解 protected_user_vault_key，失败 record_failure + 「主密码错误」
2. 数据完整性：解 app_key_sync_enc，失败不 record_failure，返「vault 数据损坏」Err
3. 副作用（refresh）：失败不 record_failure（B2 已有）

**回归测试**：`test_unlock_sync_enc_corruption_no_record_failure`——破坏 app_key_sync_enc，用正确密码 unlock，验证返 Err 但 guard remaining_wait 仍 None（不 record_failure）。

### B-SETUP-CRASH-WINDOW: A1 回滚只覆盖 Err 不覆盖崩溃（低-中，固有成本，文档化）

**核查**：setup_vault 跨两个独立事务——:107 save_vault_meta（commit meta）与 :124 migrate_secret_keys_to_encrypted（内部事务）。A1 回滚（:127-136）在 Err(migrate_err) 分支。崩溃窗口：:107 commit 后、:124 migrate commit 前，进程 panic/断电/OOM → A1 回滚不执行 → 半初始化（vault_meta 落盘 + secret_key 明文）+ :67 `ensure!(!is_initialized())` 阻止重跑 → 用户卡死 + 明文暴露。

**状态**：文档化。触发需精确崩溃在两 commit 间（毫秒级窗口），概率极低；但后果不可恢复 + 安全暴露。注释 :120-122 已承认跨表事务合并限制。修复方向（调序/跨表事务/独立后台任务）均成本较高，需架构权衡。

---

## 第三十五轮审查修复（2026-07-25，migrate.rs + 跨模块 secret_key：M-CLOUDKEY-PLAINTEXT + migrate 低危文档化）

### M-CLOUDKEY-PLAINTEXT: add/edit 云端模型明文写 secret_key，绕过 vault 加密（中-高，安全，已修）

**核查**（完整跨模块证据链）：
1. **写入路径明文**（model_commands.rs）：`add_cloud_model`（:738）`insert_cloud_model(..., &input.secret_key, ...)` 无加密；`edit_cloud_model`（:765）`update_cloud_model(..., &input.secret_key, ...)` 无加密。前端明文 API Key 直接传 db 层落盘。
2. **migrate 不覆盖**（migrate.rs:30）：`migrate_secret_keys_to_encrypted` 仅 setup_vault 调用一次。setup 后新增/编辑云端模型产生的明文，migrate 永不再触发。
3. **passthrough 掩盖**（vault_secret_access.rs:82）：`try_decrypt_secret` 对非 v1: 前缀原样返回——新明文被当「未迁移明文」passthrough，推理热路径拿到明文 API Key 鉴权正常，用户无感知。

**后果**：用户 setup vault 后 add/edit 云端模型 → 明文 secret_key 落盘 → DB 文件泄露 → 明文 API Key 直接暴露。vault 加密承诺对 setup 后增量失效。全程无感知（migrate 跑过 + UI 显示 vault 已启用 + 推理正常）。

**修复**：
- vault_secret_access.rs 补 `encrypt_secret_global` chokepoint（对称 `try_decrypt_secret_global`）：vault 已初始化 + app_key 可用 → `app_key.encrypt` → v1: 密文；否则原样返回（向后兼容 pre-vault）。空值不加密（edit 未改 key）；v1: 前缀幂等不重复加密。
- `add_cloud_model`/`edit_cloud_model` 写 DB 前调 `encrypt_secret_global(&input.secret_key)` 加密。
- 读路径 `try_decrypt_secret` 已正确处理 v1: 解密，加密写入后自动走解密分支，无需改。

**回归测试**：`encrypt_secret_empty_and_idempotent`（空值 + v1: 幂等）+ `encrypt_then_decrypt_round_trip`（加密 → 解密对称性）。

### M-MIGRATE-TOCTOU / M-COUNT / M-DEAD-CODE: migrate.rs 内部低危（低，文档化）

- **M-MIGRATE-TOCTOU**（list 事务外 + UPDATE 事务内）：list 快照与 UPDATE 时状态不一致。触发需 setup 期间并发改 models（罕见）。与 M-CLOUDKEY-PLAINTEXT 同根（M-CLOUDKEY-PLAINTEXT 修复后影响消失）。
- **M-COUNT-NO-VERIFY**（count 不验证 UPDATE 影响行数）：并发删行时 count 虚高，无安全影响。
- **M-DEAD-CODE**（update_model_secret_key 无生产调用）：#5 事务化重构后遗留。可删。

**状态**：文档化。M-CLOUDKEY-PLAINTEXT 修复后 M-MIGRATE-TOCTOU 的影响消失（增量已加密，不再有明文残留风险）。

### 附带：清理 desktop test 7 个 unused import warning

cargo fix 清理 vault_state.rs（1）+ runtime_config.rs（5）+ vault_secret_access.rs（1）的 test profile unused import。非本轮引入，但 AGENTS.md 要求 0 warning。

---

## 第三十六轮审查修复（2026-07-25，E-EDIT-TEST-CIPHERTEXT 回归 + generator 复审）

### E-EDIT-TEST-CIPHERTEXT: edit 未改 key 时取 DB 密文当明文测连接 → 401（中，功能回归，已修）

**M-CLOUDKEY-PLAINTEXT 修复的直接回归**：edit_cloud_model（model_commands.rs:757）`get_model_source_key(id)` 是裸 SQL（db.rs:1447），不解密。M-CLOUDKEY-PLAINTEXT 修复后 DB 存 `v1:` 密文，edit 取密文当明文 Bearer token 发云端 → 401 → `!test.ok` → return Err「模型测试失败」→ llm/translate 云端模型编辑被拒。

**触发**：vault 已初始化 + edit llm/translate 云端模型 + 用户不改 key（前端默认传空 secret_key 不回填明文）。前端 edit 表单默认空 secret_key → 几乎每次 edit 都走空值路径。

**根因**：唯一漏了 `try_decrypt_secret_global` 的 secret_key 读路径（其他三处：action_bar_commands:973 / config:58 / engine_aliyun:115 都已解密）。

**修复**：:757 取回 raw 后过 `try_decrypt_secret_global` 解密再测连接。与现有三处读路径模式一致。

### generator 模块复审：干净（1 个低危一致性观察）

generator 经多轮修复（R8/R5/R-AMBIGUOUS-DEAD/R-UPPER/G-EN/G-YOYO/G-EFF）后已干净。

### G-ZH-NUMBER-NO-SEPARATOR: zh include_number/symbol 不考虑 separator（低，文档化）

zh 的 include_number/symbol（passphrase_zh.rs:33-41）直接 `format!("{}{}", result, n)`，不像 en（:62-71）那样按 separator 配置补分隔符。中文默认 separator 空，紧贴无视觉问题；但若用户设非空 separator（如「·」），数字/符号仍紧贴前词，与词间分隔不一致。en/zh 两实现独立演化的轻微不对称。

---

## 第三十七轮审查（2026-07-25，crypto 复审：干净，3 项极低观察 + 演进提示）

crypto 模块复审（4 文件：mod/util/symmetric/hierarchy/kdf）——经多轮修复（M1-mod/K1/K3/C1/H1/A2/S2）后已很干净，本轮无实质 bug。

3 项极低优先级观察（**均非 bug，无需修复**，诚实标注避免过度工程）：

- **S-DECRYPT-DIAG**（极低·诊断）：symmetric.rs:46-50 `ensure!(combined.len() > NONCE_LEN)` 下限过宽——合法 GCM 密文最少 12+16=28B，`> 12` 仅挡 nonce 都不全，len 13~27 放行靠 decrypt tag 校验拒。安全无影响（最终必拒），仅错误信息语义不准。
- **K-STRICT-ITER-REDUNDANT**（极低·冗余）：kdf.rs:105-121 from_i64_strict 对 iterations 两处检查（范围 1..=u32::MAX + ≥2），第一个下界 1 被 ≥2 覆盖，轻微冗余。逻辑正确。
- **H-CHILD-EXPECT**（极低·风格）：hierarchy.rs:38 `expect("HMAC 接受任意 key 长度")`。HMAC RFC 2104 对任意 key 不失败，安全。仅与 R5「消除 unwrap」风格不一致。

**未来演进提示**（非当前 bug）：S2/AAD 纵深防御——当前不用 AAD 绑定 field_name，单机威胁模型下成立。若未来 vault 支持共享/多用户场景（密文跨设备/跨 vault 流转），「密文移动攻击」价值上升，届时 AAD 绑定 cipher_id || field_name 可作纵深防御。当前 MVP 单机不强求。

---

## 第三十八轮审查修复（2026-07-25，desktop vault 集成层：C-DELETE-NO-UNLOCK-CHECK + 3 项低危文档化）

### C-DELETE-NO-UNLOCK-CHECK: 锁定态可删除/清空 cipher（中，安全一致性，已修）

**核查**：vault_delete_cipher（:536 `_state`）/ vault_restore_cipher（:549 `_state`）/ vault_empty_trash（:559 `_state`）三命令的 `_state` 前缀（编译器「未使用」标记）是遗漏铁证——参数声明传入却被忽略。对比 vault_delete_folder（:390-397）有 `require_user_vault_key` 门禁 + 注释 :340「仍要求 vault 已解锁——避免未解锁会话误触」。

**威胁**：vault 自动锁定后（用户离开），他人/恶意前端/DevTools 可 `invoke('vault_delete_cipher', {id, permanent:true})` 或 `vault_empty_trash` 永久删除密码，无需主密码，造成不可恢复丢失。绕过「锁定 = 不可操作 vault」的安全/UX 预期。

**修复**：三命令改 `_state` → `state`，加 `config: State<'_, SharedRuntimeConfig>`，首行加 `require_user_vault_key` 门禁。与 delete_folder :396 同构。config 是 Tauri State 自动注入，前端 invoke 无需传（签名变更前端无感）。

### S-PASSIVE-TIMEOUT-CLEAR / S-STATUS-WRITE-LOCK / S-SET-TIMEOUT-NO-CHECK（低，文档化）

- **S-PASSIVE-TIMEOUT-CLEAR**：无后台定时器，超时清 key 被动（仅 require/status 调用时检查）。当前心跳 30s + status 轮询构成周期触发，实际残留窗口短。完整方案需后台定时器（权衡复杂度）。
- **S-STATUS-WRITE-LOCK**：vault_status 高频轮询用写锁（因超时需 &mut self 清 key）。99% 调用无需清，写锁属浪费。可优化为 read() 快速路径 + 仅超时才升级 write()，但引入 TOCTOU 复杂度。parking_lot 临界区极短，实际竞争小。
- **S-SET-TIMEOUT-NO-CHECK**：vault_set_lock_timeout 不检查解锁即可改超时（含设 0=永不锁定）。超时策略非敏感数据，利用需「能调 Tauri 命令」（那时已 game over）。UI 应对 0 警告。

### 集成层门禁模式洞察

三个跨模块发现（M-CLOUDKEY-PLAINTEXT 加密写入漏读路径 / E-EDIT-TEST-CIPHERTEXT 读路径漏解密 / C-DELETE-NO-UNLOCK-CHECK 删除漏解锁门禁）同源——vault 安全属性在「vault crate 核心」与「desktop 命令层胶水」之间的传递不完整。建议每加一个访问 cipher 的命令，逐项核对 require/解密/reprompt。

---

## 第三十九轮审查（2026-07-25，vault_error.rs：干净，3 项 classify 启发式局限已文档化）

vault_error.rs 主体扎实——user-safe 原则（InternalError 绝不透传内部细节）是核心安全目标，贯彻到位；稳定 code 契约、全链匹配、InvalidMasterPassword 历史 bug 修复均到位。无中高危 bug。

3 项发现全是 classify 启发式的固有局限，**均已文档化，无需修复**：

- **E-DB-LOCKED-MISCLASSIFY**（低·UX）：classify :127 `combined.contains("locked")` 会误匹配 SQLite "database is locked" → 误识别为 Locked → 前端弹解锁框（实际是 DB 锁）。注释 :343-344 已文档化此局限。触发概率低（with_db 串行化 + 短事务），即使触发仅 UX 误导（不泄露数据）。收紧需核对 vault crate 内部错误文案不依赖裸 "locked"。
- **E-MUST-TRANSPARENCY**（极低）：classify :160-162 对含「必须」/「至少需要」的错误透传 head msg。假设「含必须 = 生成器文案」——Rust/rusqlite 内部错误多为英文，中文「必须」罕见，实际风险极低。这是启发式中唯一的 head msg 透传破例。
- **E-CIPHER-NO-ID**（极低）：CipherNotFound 统一 `<unknown>`，id 未提取。前端按 code 处理不需 id，纯日志诊断信息丢失。注释 :139 承认简化。

**工程取舍**：classify 用 anyhow 链文本启发式匹配，天然有误分类风险。本模块的核心取舍是「宁可误分类为低危变体，也绝不透传内部细节」——InternalError 兜底保证了这一底线。

---

## 第四十轮审查（2026-07-25，vault_sync_commands.rs：薄包装干净，E-SYNC-OTHER-LEAK 设计债文档化）

vault_sync_commands.rs 命令层是纯转发层（129 行 / 11 命令），自身无逻辑 bug。engine 侧三处 from_i64_strict（K1-GAP 已修）+ Mutex/AtomicBool 双并发保护 + resolve 路径密码验证均到位。

### E-SYNC-OTHER-LEAK: SyncError::Other Display 透传底层错误（低-中，设计债，文档化）

**核查**：sync/error.rs Display 实现——:96 `GitError(_msg) => write!(f, "git 操作失败（详情见应用日志）")` 丢弃 stderr（#11 修复，user-safe）；:97 `Other(e) => write!(f, "同步错误：{}", e)` 透传底层 anyhow Display。同枚举内确凿不对称：GitError 贯彻 user-safe，Other 没贯彻。与 vault_error.rs InternalError 不透传原则也不一致。

**泄露内容**：engine.rs 多处 `.map_err(SyncError::Other)?` 把底层错误（rusqlite/io Error）直接包进 Other → Display 透传 → 前端 toast。可能含本地路径（`~/.octopus/.sync/vault/meta.json`）、SQL 片段、SQLite 错误结构。非密钥/cipher 明文。

**状态**：文档化。泄露内容是本地路径/SQL（非密钥），触发多为本地故障（攻击者无法远程直接触发）。完整修复需重构 SyncError 枚举区分 user-facing Other（故意构造的文案如「同步进行中」「密码错误」）vs internal Other（底层错误透传）——前者应保持 Display，后者应屏蔽。当前改不好会丢 user-facing 文案的前端展示。

### 次要观察: spawn 前不查 is_syncing（极低，UX，文档化）

vault_sync_now spawn_blocking 前不查 is_syncing()。连点「立即同步」每次都起线程，第二个立刻 try_sync_lock 失败 → 误导性 error toast。无正确性问题（Mutex 保证），纯 UX。可前置 `if is_syncing() { return Ok(()) }` 静默吞掉重入。

### A 并发守卫（报告自否决 ✓）

sync_now :597 入口 `try_sync_lock()`（SYNC_LOCK Mutex::try_lock）保证 git 操作串行，无 index.lock 竞争。SYNCING AtomicBool + SyncingGuard 是 UI 进度查询，与 Mutex 职责分离。双重保护完善。

---

## 第四十一轮审查修复（2026-07-25，engine.rs：C-PULL-NO-META-SKIPS-STAMP + E-PULL-NO-HARD-DELETE-SYNC 文档化）

engine.rs 是成熟模块（2059 行，回归测试极全）。md5 指纹比对、stamp 前置校验、软删闭环设计扎实。但有一处 stamp 防护在边界条件下被绕过。

### C-PULL-NO-META-SKIPS-STAMP: 远程 meta.json 缺失时 stamp 校验被跳过仍 upsert cipher（中，INV-S9 违反，已修）

**核查**：pull_from_files 的 stamp 校验仅在「远程 meta 存在 + 本地 meta 存在」双条件时执行（:802-809）。远程 meta.json 不存在 → meta_file = None（:795-797）→ 整个 stamp 校验跳过，但 :816 的 cipher upsert 无条件执行。

**违背的自述不变量**：pull_from_files 注释 :788-790 强调「必须在 upsert cipher/folder 之前完成 stamp 校验，否则 stamp 不一致时本地 DB 已被用错误 user_vault_key 加密的密文污染，返 Err 也无回滚（INV-S9 强化）」。但当前实现恰恰在「远程 meta 缺失」路径绕过了这个保护——注释 :792 把 meta 缺失判定为「合法场景（首次同步/纯新增）」，但「合法」应仅限 local_meta = None，未覆盖「本地已有 vault + 远程 meta 缺失」异常态。

**污染场景**：本地已初始化 vault（K_local）+ 远程 meta.json 缺失（损坏/不完整 clone/篡改）但 cipher 文件仍在 → pull 跳过 stamp → 远程 cipher（K_remote 加密）被 upsert 进本地 DB → K_local 解密失败 → 不可解密密文污染。

**修复**：把「meta 缺失合法」严格限定为 local_meta = None。`local_meta.is_some() && meta_file.is_none()` → 返 `Err(RepoCorrupted)`，不进入 upsert 阶段。保留「本地无 vault + 远程无 meta」首次同步合法路径。

**回归测试**：`pull_rejects_when_local_has_vault_but_remote_meta_missing`（拒绝）+ `pull_allows_when_both_local_and_remote_meta_missing`（首次同步允许）。

### E-PULL-NO-HARD-DELETE-SYNC: permanent_delete 不双向同步（低，设计权衡，文档化）

pull 只遍历 remote_outline 做 upsert，不处理「DB 有但 outline 无」的行。push 侧 incremental_export 会删文件，但 pull 侧无对应 DB 行删除。

**状态**：文档化（= M-TOMBSTONE / M5 同型）。vault 用软删模型（deleted_at = tombstone），正常删除走软删 → md5 变 → pull upsert（H2 闭环）。只有 permanent_delete（清理 tombstone）不双向同步——与「tombstone 各设备独立清理避免无限累积」的常见同步设计一致。SyncReport.deleted 硬编码 0 也与此一致。活跃数据走软删不丢失。

### 正面确认（engine.rs 防护到位）

- P-MD5-LINEAR-SCAN HashMap O(1) 比对 ✓ / H2 软删保留 ✓ / stamp 前置两阶段 ✓
- from_i64_strict 三处（clone/pull/resolve）✓ / #10 损坏文件不静默吞 ✓
- #4 push 错误不谎报 ✓ / #7 disable_sync 加锁 ✓ / E3 类型安全 ✓

---

## 第四十二轮审查修复（2026-07-26，fingerprint.rs：字段全覆盖确认 + 2 项极低清理）

fingerprint.rs 核心正确性确认——cipher_md5 11 字段 = VaultCipher 全部业务字段（对照 schema db.rs:3539），folder_md5 3 字段 = VaultFolder 全部业务字段。对称性（cipher_md5 vs cipher_md5_from_input）逐字段一致。这是 pull/push 一致性的基石。

### E-CIPHER-MD5-FROM-INPUT-ID-PARAM-REDUNDANT: id 参数冗余（极低，重构，已修）

**核查**：`cipher_md5_from_input(id: &str, input)` 的注释 :60-61 说「input 不含 id，所以加 id 参数」已过时——v39 UUID 改动后 VaultCipherInput 有 id 字段（db.rs:3584）。两个调用点（cipher.rs:82/122）传的 id 始终 = input.id（:69/:97 `id: id.to_string()`）。

**修复**：移除 id 参数，直接用 input.id。消除「调用方传 ≠ input.id」的 API 误用面 + 修正过时注释。纯重构无功能影响。

### E-FOLDER-MD5-NO-COLLISION-TEST: folder 无碰撞守护（极低，测试不对称，已修）

**核查**：cipher_md5 有 F2 碰撞守护测试（`cipher_md5_no_collision_on_pipe_in_separate_fields`），folder_md5 无对称测试。当前 folder 三字段（UUID/base64/数字）都不含 |，安全。

**修复**：补 `folder_md5_no_collision_on_pipe_in_separate_fields`（对称 cipher 的碰撞守护，验证 name/sort_order 变化都导致 md5 变化）。

### 正面确认（fingerprint 是 sync 一致性基石）

- 字段全覆盖（11 cipher + 3 folder，对照 schema）
- H2 deleted_at 纳入（软删/恢复 md5 变化触发 sync，不复活）
- md5 重算时机正确（soft_delete S1 单事务 + save_cipher M-CIPHER-RMW 单事务）
- 时间戳排除（created_at/updated_at 跨设备不同，不进 md5）

---

## 第四十三轮审查修复（2026-07-26，sync/store.rs：E-PATH-TRAVERSAL-OUTLINE-UUID 中-高安全）

### E-PATH-TRAVERSAL-OUTLINE-UUID: 远程 outline uuid 经 read/delete 触发 path traversal（中-高，安全，已修）

**核查**：`cipher_file_path`/`folder_file_path`（store.rs:73-86）的 `format!("{}.json", uuid)` 原样拼接 uuid，无路径分隔符过滤。`shard_dir`（sync/store.rs:78）只 sanitize 分片目录（filter is_ascii_hexdigit + take 2），不 sanitize 文件名。

**攻击链**：
- **delete**（incremental_export :581-586）：遍历 `old_outline.ciphers.keys()`（远程 untrusted）调 `delete_cipher_file` → 恶意 uuid `../../meta` → 删 `vault_root/meta.json` 或更多 `../` 跳出 vault_root 删任意 .json 文件
- **read**（pull_from_files :831）：遍历 remote_outline 调 `read_cipher_file` → 读 traversal 路径文件

**修复**：在 `cipher_file_path`/`folder_file_path` 入口加 `validate_uuid`——拒绝含 path traversal 字符（`/`、`\`、`..`、`\0`、空串）的 uuid。chokepoint 模式：read/delete/write 三路径统一在路径构造入口拦截，无需改 pull/incremental_export 业务逻辑。函数签名改为 `Result<PathBuf>`。

> 设计决策：不强制严格 UUID v4 格式（`uuid::Uuid::parse_str`），只拒绝 path traversal 字符。理由：vault 生产 id 理论上是 UUID v4，但测试用简短 id（"test-uuid" 等）方便，严格 UUID 校验会破坏 10+ 个现有测试且无额外安全收益——path traversal 字符检查已足够防目录穿越。

**回归测试**：`path_traversal_uuid_rejected`——合法 UUID 通过 + 多种恶意 uuid（../../meta、绝对路径、Windows 风格）被拒 + 非法格式被拒。

**严重度校准**：中-高——不到「高」（私有 repo 威胁模型 + .json 后缀限制可利用性），高于「中」（delete 破坏性 + read/delete 双路径 + 防御原则违反 + clone 任意 repo 场景真实）。write 路径 trusted（用本地 DB 的 UUID v4，非 outline 控制）。

---

## 第四十四轮审查（2026-07-26，vault_state / vault_secret_access / passphrase_en：干净，2 项信息性观察）

三模块设计成熟、守护测试完备，无中高危 bug。两个候选观察经核实均不可达，定级信息性。

### OBS-1: is_user_vault_unlocked 对 last_active_at=None 的「信任」语义（信息性，不可达）

vault_state.rs:109 若 last_active_at=None 但 user_vault_key=Some，超时检查跳过 → 永不超时。

**不可达确认**：grep 全仓库 user_vault_key 写入点仅 3 处——:111 `= None`（超时清零）/ :125 `= Some(key)`（set_user_vault_unlocked，同时设 last_active_at）/ :141 `= None`（lock）。无路径绕过 set_user_vault_unlocked 直接写 Some。故「key 在但 last_active_at=None」正常路径不可达。

### OBS-2: try_decrypt_secret_global 对 session=None 返回 raw（信息性，不可达）

vault_secret_access.rs:117-123 session None → Ok(raw) 即使 raw 是 v1: 密文。对比 try_decrypt_secret(:90-91) v1: + app_key None → Err。两版本对「v1: 但无法解密」处理相反。

**不可达确认**：set_global_session 仅 main.rs:1073 一处，在 `#[cfg(feature = "vault")]` 块内。feature on → session 必注入；feature off → 整块跳过 + octopus_vault 不存在 + DB 不可能有 v1: 密文 → Ok(raw) 正确。注释 :111-112/:120 已说明设计意图。

---

## 第四十五轮审查修复（2026-07-26，crypto + unlock + migrate 安全心脏复查：C-CHANGE-RESET-ASYMMETRY）

vault 安全心脏（crypto 五文件 + unlock + migrate）密码学正确性确认——AES-256-GCM / Argon2id / HMAC-SHA512 child + Zeroizing 卫生闭环，H1 源秘密清零影响面 4 入口全部 Zeroizing<String> + 编译期守护，migrate #5 事务化 + A1 回滚完整。

### C-CHANGE-RESET-ASYMMETRY: change 的 guard.reset 时机与 unlock 不对称（低，对称性遗漏，已修）

**核查**：第三十四轮 B-UNLOCK-RECORD-ASYMMETRY 修复把 unlock 的 `reset()` 提前到 protected 校验成功后（:222）。但 change_master_password 的 reset 在 :333 全成功末尾——protected 验证通过（:280 Ok）但后续失败（:293 sync_enc 损坏 / :303 encrypt 失败 / :328 save 失败）时提前 return，guard 未 reset。

**后果**：旧密码正确但后续失败时，guard 有历史计数 → 下次 change/unlock 被 remaining_wait() 挡——「密码其实对却因数据损坏/副作用失败被退避挡」。

**修复**：在 :290（user_vault_key 构造后、sync_enc 解密前）加 `guard().reset()`，与 unlock :222 对称。:333 末尾的 reset 保留（幂等无害）。

**同源关系**：这是 B-UNLOCK-RECORD-ASYMMETRY 修复 unlock 时的遗漏——修 unlock 时没对称处理 change。
