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
