# Vault Git 同步实施计划（Phase 1）

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 为 octopus 密码箱增加多设备同步——用 git repo（GitHub/Gitee private repo）作为后端，SSH key 认证（系统已配），每 cipher 单独加密文件 + 256 桶分片 + outline.json 增量索引。

**Architecture:** 复用现有 vault 加密层（user_vault_key + AES-256-GCM，零改动）；sync 模块最初在 `crates/vault/src/sync/`，Task 13 后通用部分抽到独立 `crates/sync/`（octopus-sync），vault 只保留业务 sync 逻辑；新增 `crates/desktop/src/vault_sync_commands.rs`（Tauri 命令）；新增前端 `Settings/Vault/SyncPanel.tsx`（同步配置 UI）。前置改造：cipher/folder id 从 i64 改 UUID 字符串（v43→v44），热词 id 同样改 UUID（v45→v46）。

**Tech Stack:** Rust（uuid / serde / anyhow——已用）+ shell out 系统 `git` 命令（无新依赖）+ Tauri 2 + React 19 + TypeScript + Tailwind 4。

**Spec:** [2026-07-21-vault-git-sync-design.md](../specs/2026-07-21-vault-git-sync-design.md)

> **状态**：Phase 1（vault 同步 T1-T12）+ Phase 2（热词同步 + sync crate 抽离，Task 13）均已完成并 e2e 验证通过（2026-07-22）。

## Global Constraints

（从 spec §1-§5 摘录的硬约束，所有任务隐式遵守）

- **加密层零改动**：复用现有 `user_vault_key`（派生自 master_password），密文格式 `v1:<base64(nonce||ct||tag)>` 与 SQLite 完全一致
- **git 实现**：shell out 系统 `git` 命令（不嵌入 libgit2）；无 git 则同步功能禁用
- **认证**：Phase 1 只支持 SSH key（用户系统已配），octopus 完全不接触凭证
- **cipher/folder/hotword id**：UUID v4 字符串（cipher/folder v43→v44，hotword v45→v46），不再用 i64 自增
- **分片**：`ciphers/<uuid 前 2 hex>/<full-uuid>.json`，256 桶（热词 `sets/<2hex>/<uuid>.json` 同样分桶）
- **outline.json**：`{version, vault_version, ciphers: {uuid: {md5, updated_ms}}, folders: {...}}`——增量同步索引（md5 内容指纹，非 sha256；updated_ms Unix 毫秒，非 ISO 字符串。§4.12 修订）
- **同步触发**：手动（Phase 1）；Phase 2 才加自动
- **冲突处理**：UUID 隔离 + `git merge --ff-only` + rebase 兜底
- **多 remote**：支持 GitHub + Gitee 双 remote（用户自配）
- **commit message**：统一 `sync` 或 `init vault`，不暴露操作细节
- **跨 crate 依赖方向**：infra ← sync ← vault ← desktop；通用 sync 代码在独立 `crates/sync/`（octopus-sync），vault 通过依赖 sync crate 复用（Task 13 抽离）
- **错误返回**：Tauri 命令统一 `Result<T, String>`（与现有 vault_commands 一致）；vault crate 内部用 `anyhow::Result`
- **feature gate**：sync 模块在 vault feature 下（继承现有 vault feature gate）
- **平台范围**：macOS / Linux 优先；Windows 测试覆盖（shell out git 跨平台一致）

---

## File Structure

### 新增文件

> 注：下表是 Phase 1（T1-T6）设计时的文件清单。Task 13 后 git/outline/error/privacy/store(通用) 搬到 `crates/sync/`，vault::sync 只保留 engine/fingerprint/store(业务)。当前真实结构详见 spec §6。

**crates/vault/src/sync/**（Phase 1 新模块，Task 13 后部分搬到 sync crate）

| 文件 | 职责 |
|---|---|
| `mod.rs` | 模块入口 + 公共 API + SyncState 锁 |
| `error.rs` | SyncError enum（无 git / 网络不可达 / SSH 失败 / 冲突 / etc.）|
| `git.rs` | git 命令 wrapper（shell out `Command::new("git")`）|
| `store.rs` | 文件存储（meta.json / outline.json / ciphers/<桶>/<uuid>.json 读写）|
| `outline.rs` | Outline 数据结构 + merge 算法 |
| `engine.rs` | 同步引擎（pull_merge_push / push_initial / clone_initial）|

**crates/desktop/src/**

| 文件 | 职责 |
|---|---|
| `vault_sync_commands.rs` | Tauri 命令（sync_now / status / enable / disable / test_connection）|

**crates/desktop/frontend/src/pages/Settings/Vault/**

| 文件 | 职责 |
|---|---|
| `SyncPanel.tsx` | 同步配置 UI（状态 / remote URL / 测试连接 / 立即同步 / 禁用）|

### 修改文件

| 文件 | 改动 |
|---|---|
| `crates/infra/src/db.sql` | vault_ciphers / vault_folders 的 id 改 TEXT PRIMARY KEY |
| `crates/infra/src/db.rs` | i64 → String 类型；v43→v44 schema 迁移逻辑（设计阶段估 v38→v39，实施时已到 v43）|
| `crates/vault/src/types.rs` | Cipher.id / CipherInput / Folder.id 改 String |
| `crates/vault/src/storage/*.rs` | 所有 CRUD 签名 i64 → String |
| `crates/vault/src/lib.rs` | re-export sync 模块 |
| `crates/desktop/src/vault_commands.rs` | Tauri 命令签名 cipher_id i64 → String |
| `crates/desktop/src/main.rs` | 注册新 vault_sync_commands |
| `crates/desktop/frontend/src/pages/VaultPicker/index.tsx` | CipherDto.id 类型 number → string |
| `crates/desktop/frontend/src/pages/Settings/Vault/*.tsx` | CipherDto.id 类型 + folder id 类型 |
| `crates/desktop/frontend/src/locales/{zh-CN,en}.yaml` | 同步相关 i18n |

---

## Task 1: cipher/folder id 改 UUID 字符串（v43→v44 前置改造）

> **注**：设计阶段估的是 v38→v39，但实施时 user_version 已经到 v43，实际是 v43→v44。详见「实施记录 → 关键决策变化」第 1 条。

**目标**：把 vault_ciphers / vault_folders 的 id 从 INTEGER AUTOINCREMENT 改成 TEXT（UUID v4），让跨设备无冲突。

**Files:**
- `crates/infra/src/db.sql`
- `crates/infra/src/db.rs`
- `crates/vault/src/types.rs`
- `crates/vault/src/storage/cipher.rs`
- `crates/vault/src/storage/folder.rs`
- `crates/desktop/src/vault_commands.rs`
- 前端所有 CipherDto 引用

### Steps

- [x] **1.1 db.sql schema 改 TEXT PRIMARY KEY**
  - vault_ciphers.id 从 `INTEGER PRIMARY KEY AUTOINCREMENT` 改 `TEXT PRIMARY KEY`
  - vault_folders.id 同样改
  - vault_ciphers.folder_id 类型改 TEXT（与 vault_folders.id 一致）
  - 注释说明「UUID v4 字符串，不再自增——支持 git 同步」

- [x] **1.2 db.rs 类型 i64 → String**
  - VaultCipher.id / VaultFolder.id 字段改 String
  - VaultCipherInput / VaultCipherUpdate 的 id 字段改 String
  - 所有 `load_vault_cipher(id: i64)` / `insert_vault_cipher(input)` / `update_vault_cipher(id, ...)` / `delete_vault_cipher(id)` 签名改 String
  - `row.get::<_, i64>(0)` 改 `row.get::<_, String>(0)`
  - **insert 不再返回 last_insert_rowid**——改成调用方先 `Uuid::new_v4().to_string()` 生成再 INSERT

- [x] **1.3 v38→v39 schema 迁移**
  - init_schema 加 v39 分支：
    1. `CREATE TABLE vault_ciphers_new (... id TEXT PRIMARY KEY ...)` + `vault_folders_new`
    2. `INSERT INTO vault_ciphers_new SELECT hex(randomblob(16)) AS id, ... FROM vault_ciphers`（为每行生成 UUID）
    3. 用临时表 `vault_id_map(old_rowid TEXT, new_uuid TEXT)` 暂存映射
    4. 修复 folder_id 引用
    5. DROP 旧表 + RENAME 新表
    6. `PRAGMA user_version = 39`
  - **测试**：迁移测试覆盖（旧 i64 id → 新 UUID 字符串）

- [x] **1.4 vault crate types/storage 改 String**
  - `Cipher.id: i64` → `String`
  - `CipherInput.id` → 新增（创建时调用方生成 UUID）
  - 所有 storage::cipher / storage::folder 函数签名
  - 单测里所有 fixture 的 `id: 1` 改 `id: Uuid::new_v4().to_string()`

- [x] **1.5 desktop vault_commands Tauri 命令签名**
  - `vault_autotype(cipher_id: i64)` → `: String`
  - `vault_copy_password(cipher_id: i64)` → `: String`
  - `vault_copy_username(cipher_id: i64)` → `: String`
  - `vault_save_cipher(id: Option<i64>, ...)` → `: Option<String>`
  - `vault_delete_cipher(id: i64)` → `: String`
  - `vault_create_cipher` 返回值 i64 → String
  - 所有命令的 invoke 参数名保持 camelCase（cipherId）

- [x] **1.6 前端 CipherDto.id 类型 number → string**
  - VaultPicker/index.tsx CipherDto.id: number → string
  - Settings/Vault/*.tsx 同步改
  - invoke 调用不变（JS 自动序列化 string）
  - **follow-up（2026-07-21）**：v44 合并到 main 后发现 1.6 漏改了几个 state/prop 类型，导致 `tsc -b` 报 8 个错误（`CipherList.editing/activeId` 仍是 `number|null`、`CipherEditor.cipherId/folderId` 仍是 `number|null`、`VaultPicker.revealedPasswords` 仍是 `Record<number,>`、`SyncPanel` 未用 `Row` import）。已在 commit `94d85a16` 补齐——这是 Task 1.6 的遗漏，类型层面修复，无运行时行为变化。

- [x] **1.7 create_cipher 时生成 UUID**
  - `vault_commands::vault_create_cipher`：调用 `Uuid::new_v4().to_string()` 作为新 cipher id
  - 或在 vault crate storage 层做（更干净）

- [x] **1.8 验证**
  - `cargo build -p octopus-infra -p octopus-vault -p octopus-desktop --features 'embedded cloud vault'` 0 error 0 warning
  - `cargo test -p octopus-vault --lib` 全过
  - `cargo test -p octopus-infra --lib` 全过
  - `cargo test -p octopus-desktop` 全过（vault_commands 测试覆盖）
  - tsc 0 error

---

## Task 2: 文件存储模块（crates/vault/src/sync/store.rs）

**目标**：实现 `~/.octopus/.vault/` 下文件读写——meta.json / outline.json / ciphers/<桶>/<uuid>.json / folders/<uuid>.json。

**Files:**
- `crates/vault/src/sync/mod.rs`
- `crates/vault/src/sync/store.rs`
- `crates/vault/src/sync/outline.rs`
- `crates/vault/src/sync/error.rs`
- `crates/vault/Cargo.toml`（加 uuid 依赖）
- `crates/vault/src/lib.rs`（re-export sync 模块）

### Steps

- [x] **2.1 新建 sync 模块骨架**
  - `crates/vault/src/sync/mod.rs`：模块声明 + re-export
  - `crates/vault/src/sync/error.rs`：`SyncError` enum（Anyhow 风格）
    - `GitNotInstalled` / `NetworkUnreachable` / `SshPermissionDenied` / `SshHostKeyUnverified`
    - `RepoNotInitialized` / `RepoCorrupted` / `OutlineDamaged`
    - `ConflictNeedsManual` / `MasterPasswordMismatch`（security_stamp 不一致）
  - `crates/vault/src/lib.rs`：`pub mod sync;`

- [x] **2.2 数据结构（outline.rs）**
  - `OutlineEntry { sha: String, updated_at: String }`
  - `Outline { version: u32, vault_version: u64, ciphers: HashMap<String, OutlineEntry>, folders: HashMap<String, OutlineEntry> }`
  - `merge_outlines(local, remote) -> Outline`（spec §4.6 算法）
  - 单测：merge 双方新增 / 同 uuid 取最新 / vault_version 取 max

- [x] **2.3 文件路径辅助（store.rs）**
  - `vault_root() -> PathBuf`：返回 `~/.octopus/.vault/`
  - `meta_path() -> PathBuf`：`vault_root/meta.json`
  - `outline_path() -> PathBuf`：`vault_root/outline.json`
  - `cipher_file_path(uuid: &str) -> PathBuf`：`vault_root/ciphers/<uuid[0..2]>/<uuid>.json`
  - `folder_file_path(uuid: &str) -> PathBuf`：`vault_root/folders/<uuid[0..2]>/<uuid>.json`（folder 也分桶）
  - `shard_dir(uuid: &str) -> String`：取前 2 hex（`uuid.chars().filter(|c| c.is_ascii_hexdigit()).take(2).collect()`）

- [x] **2.4 meta.json 读写（store.rs）**
  - `MetaFile { version: u32, kdf_type, kdf_salt, kdf_iterations, kdf_memory_kib, kdf_parallelism, protected_user_vault_key, app_key_sync_enc, security_stamp, equivalent_domains }`
  - `read_meta_file() -> Result<MetaFile>`
  - `write_meta_file(meta: &VaultMeta) -> Result<()>`（从 vault_meta 表结构转换）
  - JSON 序列化（serde_json）

- [x] **2.5 cipher 文件读写（store.rs）**
  - `CipherFile { version: u32, id: String, encrypted: CipherEncStrings, plaintext_meta: CipherPlaintextMeta }`
  - `CipherEncStrings { name, notes, data, fields, password_history }`——全部 `v1:` 前缀密文
  - `CipherPlaintextMeta { folder_id, favorite, atype, reprompt, deleted_at, created_at, updated_at }`
  - `read_cipher_file(uuid: &str) -> Result<CipherFile>`
  - `write_cipher_file(cipher_row: &VaultCipher) -> Result<()>`（从 SQLite 行转换）
  - **加密层复用**：store.rs 接收已加密的 VaultCipher 行（storage::cipher.rs 已经在 SQLite 层加密），不重新加密

- [x] **2.6 folder 文件读写（store.rs）**
  - 同 cipher，结构更简单（只有 name 加密）

- [x] **2.7 outline.json 读写（store.rs）**
  - `read_outline_file() -> Result<Outline>`
  - `write_outline_file(outline: &Outline) -> Result<()>`
  - 单测：round-trip

- [x] **2.8 全量导出/导入（store.rs）**
  - `export_all_to_files(meta: &VaultMeta, ciphers: &[VaultCipher], folders: &[VaultFolder]) -> Result<()>`
    - 清空 `~/.octopus/.vault/ciphers/` 和 `folders/`
    - 写 meta.json / outline.json / 所有 cipher / folder 文件
    - outline 的 sha 字段：对 cipher 文件内容算 sha256（不是 git blob sha——git_blob_sha 由 git 命令拿）
  - `import_all_from_files() -> Result<(VaultMeta, Vec<VaultCipher>, Vec<VaultFolder>)>`
    - 读所有文件，返回内存结构供上层 upsert 到 SQLite

- [x] **2.9 验证**
  - `cargo build -p octopus-vault` 0 error
  - `cargo test -p octopus-vault --lib sync::` 全过（store / outline round-trip + merge）

---

## Task 3: git 命令 wrapper（crates/vault/src/sync/git.rs）

**目标**：封装 shell out `git` 命令，提供类型安全 API。

**Files:**
- `crates/vault/src/sync/git.rs`

### Steps

- [x] **3.1 check_git_available()**
  - `Command::new("git").arg("--version").output()`
  - 成功返 `true`，失败返 `false`
  - 启动时调用，无 git 则 sync 模块返 SyncError::GitNotInstalled

- [x] **3.2 git_init / git_remote_add / git_remote_list**
  - `git_init(path: &Path) -> Result<()>`
  - `git_remote_add(path: &Path, name: &str, url: &str) -> Result<()>`
  - `git_remote_list(path: &Path) -> Result<Vec<(String, String)>>`（name, url）
  - 所有命令在 path 下执行（`.current_dir(path)`）

- [x] **3.3 git_fetch / git_merge_ff / git_rebase**
  - `git_fetch_all(path: &Path) -> Result<()>`：`git fetch --all --prune`
  - `git_merge_ff(path: &Path, ref_name: &str) -> Result<bool>`：`git merge --ff-only <ref>`，返 `Ok(true)` 成功 ff，`Ok(false)` 不能 ff（需 rebase）
  - `git_rebase(path: &Path, ref_name: &str) -> Result<()>`：`git rebase <ref>`，失败时 stderr 含 conflict 信息

- [x] **3.4 git_add / git_commit / git_push**
  - `git_add_all(path: &Path) -> Result<()>`
  - `git_commit(path: &Path, msg: &str) -> Result<bool>`：返 `Ok(true)` 成功 commit，`Ok(false)` nothing to commit（stderr 含 "nothing to commit"）
  - `git_push(path: &Path, remote: &str, ref_name: &str) -> Result<()>`：失败时 stderr 含 SSH 错误信息

- [x] **3.5 git_clone**
  - `git_clone(url: &str, path: &Path) -> Result<()>`：B 机首次同步

- [x] **3.6 git_status / git_ls_remote**
  - `git_status_has_changes(path: &Path) -> Result<bool>`：`git status --porcelain` 非空
  - `git_ls_remote(url: &str) -> Result<bool>`：`git ls-remote --heads <url>`——测试连接用，成功返 true

- [x] **3.7 git_rebase_abort / git_merge_abort**
  - `git_rebase_abort(path: &Path) -> Result<()>`：崩溃恢复
  - `git_merge_abort(path: &Path) -> Result<()>`：崩溃恢复

- [x] **3.8 git_current_branch / git_checkout**
  - `git_current_branch(path: &Path) -> Result<String>`
  - `git_checkout(path: &Path, branch: &str) -> Result<()>`

- [x] **3.9 错误处理**
  - 把 git stderr 透传到 SyncError，分类：
    - 含 "Host key verification failed" → `SshHostKeyUnverified`
    - 含 "Permission denied (publickey)" → `SshPermissionDenied`
    - 含 "Could not resolve host" / "Connection timed out" → `NetworkUnreachable`
    - 含 "CONFLICT" → `ConflictNeedsManual`
    - 其他 → `GitError(stderr)`

- [x] **3.10 验证**
  - `cargo test -p octopus-vault --lib sync::git` 全过
  - 单测用 `tempfile::tempdir()` 创建临时 repo，覆盖每个函数

---

## Task 4: 同步引擎（crates/vault/src/sync/engine.rs）

**目标**：编排 pull → merge → 文件系统 ↔ SQLite 双向同步 → commit → push。

**Files:**
- `crates/vault/src/sync/engine.rs`
- `crates/vault/src/sync/mod.rs`（SyncState 锁）

### Steps

- [x] **4.1 SyncState 进程内锁**
  - `SyncState` struct：`Arc<Mutex<bool>>`
  - `try_lock() -> Option<SyncGuard>`：失败返 None（同步进行中）
  - `SyncGuard` RAII，drop 时解锁
  - 全局单例 `OnceLock<SyncState>`

- [x] **4.2 sync_status() -> SyncStatus**
  - enum `SyncStatus { Disabled, NotInitialized, Configured { remote_url: String, last_sync: Option<String> }, Syncing, Error(String) }`
  - 检测顺序：
    1. git 不可用 → Disabled
    2. `~/.octopus/.vault/` 不存在 → NotInitialized
    3. 存在但 `.git/` 不存在 → NotInitialized
    4. 存在 → 读 git remote 配置，返 Configured

- [x] **4.3 test_connection(remote_url: &str) -> Result<()>**
  - 调 `git::git_ls_remote(url)`
  - 成功返 Ok，失败返具体 SyncError

- [x] **4.4 enable_sync(remote_url: &str, gitee_url: Option<&str>) -> Result<()>**
  - 检测 `~/.octopus/.vault/` 状态：
    - 不存在 + 远程空 → push_initial 流程（§4.5）
    - 不存在 + 远程有数据 → clone_initial 流程（§4.6）
    - 存在 → 报错「已初始化，请先 disable」
  - 配置 remote：`git remote add origin <url>`，如有 gitee_url 再 add

- [x] **4.5 push_initial()**
  - `git init ~/.octopus/.vault`
  - 从 SQLite 导出全部到文件（store::export_all_to_files）
  - `git add -A && git commit -m "init vault"`
  - `git remote add origin <url>`
  - `git push -u origin main`

- [x] **4.6 clone_initial(remote_url)**
  - `git clone <url> ~/.octopus/.vault`
  - 从文件导入全部到 SQLite（store::import_all_from_files）
  - 用户必须先输 master_password 解锁（前端流程）

- [x] **4.7 sync_now() -> Result<SyncReport>**
  - 编排 spec §4.2 pull_merge_push 流程：
    1. try_lock SyncState（失败返「同步进行中」）
    2. 检查 git 可用
    3. `git fetch --all --prune`
    4. `git merge --ff-only origin/main`
       - 成功 ff → 远程有更新，继续读文件回 SQLite
       - 不能 ff → 走 rebase 路径（§4.8）
    5. **pull 阶段**：读 outline.json，对比本地 outline，找出差异 cipher/folder，读文件 upsert SQLite
    6. **push 阶段**：读 SQLite，找出本地变化，写文件，更新 outline（vault_version++）
    7. `git add -A && git commit -m "sync"`（无变化跳过）
    8. `git push origin main`
    9. 如有 gitee remote：`git push gitee main`
    10. 返回 `SyncReport { pulled: N, pushed: M, conflicts: 0 }`

- [x] **4.8 rebase 兜底**
  - `git_rebase("origin/main")`
  - outline.json 冲突 → 调 `merge_outlines` 解决，`git add outline.json && git rebase --continue`
  - 其他文件冲突 → 报 SyncError::ConflictNeedsManual（理论不可能——UUID 隔离）

- [x] **4.9 disable_sync()**
  - 删除 `~/.octopus/.vault/`（保留 SQLite 数据）
  - 提示「同步已禁用，本地数据保留」

- [x] **4.10 崩溃恢复**
  - sync_now 入口检查 git 状态：
    - `.git/MERGE_HEAD` 存在 → `git merge --abort`
    - `.git/rebase-merge` 或 `.git/rebase-apply` 存在 → `git rebase --abort`
  - 然后继续正常流程

- [x] **4.11 验证**
  - `cargo test -p octopus-vault --lib sync::engine` 全过
  - 集成测试：用两个 tempdir 模拟 A/B 机，覆盖 push_initial / clone_initial / 双向同步 / 冲突 rebase

---

## Task 5: 配置 UI（前端 + Tauri 命令）

**目标**：用户在设置页配置同步 + 触发同步 + 看状态。

**Files:**
- `crates/desktop/src/vault_sync_commands.rs`
- `crates/desktop/src/main.rs`
- `crates/desktop/frontend/src/pages/Settings/Vault/SyncPanel.tsx`
- `crates/desktop/frontend/src/pages/Settings/Vault/VaultPanel.tsx`（嵌入 SyncPanel）
- `crates/desktop/frontend/src/locales/{zh-CN,en}.yaml`

### Steps

- [x] **5.1 Tauri 命令（vault_sync_commands.rs）**
  - `vault_sync_status() -> Result<SyncStatusDto, String>`
  - `vault_sync_test_connection(remote_url: String) -> Result<(), String>`
  - `vault_sync_enable(remote_url: String, gitee_url: Option<String>) -> Result<(), String>`
  - `vault_sync_now() -> Result<SyncReportDto, String>`
  - `vault_sync_disable() -> Result<(), String>`
  - `vault_is_git_available() -> bool`（启动时检测用）
  - main.rs 注册所有命令（vault feature gate）

- [x] **5.2 前端 SyncPanel.tsx**
  - Props: `{ showToast: (msg: string) => void }`
  - 状态机（spec §7.2）：
    - mount 时调 `vault_sync_status`
    - 根据状态渲染不同 UI（未启用 / 配置中 / 已连接 / 同步中 / 错误）
  - 「启用同步」按钮 → 展开 remote URL 输入 + 测试连接 + 确认
  - 「立即同步」按钮 → 调 `vault_sync_now`，显示 SyncReport toast
  - 「禁用同步」按钮 → 二次确认 → 调 `vault_sync_disable`

- [x] **5.3 VaultPanel 嵌入 SyncPanel**
  - 在 VaultPanel 顶部加 SyncPanel（feature gate + git 可用）
  - 已初始化但未解锁也显示 SyncPanel（让用户能配置同步）

- [x] **5.4 i18n**
  - settings.vault.sync.{title, status, enable, disable, syncNow, testConnection, remoteUrl, giteeUrl, lastSync, syncing, success, noGit, sshHint}
  - 中英文翻译

- [x] **5.5 验证**
  - `cargo build -p octopus-desktop --features 'embedded cloud vault'` 0 error 0 warning
  - tsc 0 error
  - `cargo test -p octopus-desktop` 全过
  - bun run test 全过
  - **手动 e2e**：
    - A 机：启用同步（需先在 GitHub 建 private repo + 配 SSH key）→ 测试连接 → 立即同步
    - B 机（或同机另一个用户）：启用同步 → 选「clone」分支 → 输主密码 → 看到 cipher 出现

---

## Task 6: 文档 + 测试

**目标**：把实施过程沉淀到文档，补全测试。

**Files:**
- `docs/architecture.md`
- `docs/superpowers/specs/2026-07-21-vault-git-sync-design.md`（同步实施差异）
- `docs/superpowers/plans/2026-07-21-vault-git-sync.md`（本文件，标记 task 完成）

### Steps

- [x] **6.1 architecture.md**
  - vault 段补「Git 同步」子段：存储结构 / 同步流程 / SSH 认证 / 256 桶分片 / Phase 1 限制

- [x] **6.2 spec 文档同步**
  - 把 spec 里「设计阶段」标记改成「已实现」
  - 实施过程的偏差（如果有）回写 spec

- [x] **6.3 plan 标记完成**
  - 所有 task checkbox `[ ]` → `[x]`
  - 末尾加「实施总结」段：实际 commit 列表 + 关键决策变化 + 测试结果

- [x] **6.4 测试覆盖**
  - T1：UUID 迁移测试
  - T2：store round-trip + outline merge
  - T3：git wrapper（tempdir 临时 repo）
  - T4：集成测试（双 tempdir 模拟 A/B 机）
  - T5：e2e 测试清单（A→B push/pull / SSH 配置 / 错误处理）

- [x] **6.5 最终验证**
  - `cargo test --workspace`（除 real_model 测试）全过
  - `cargo build --release -p octopus-desktop --features 'embedded cloud vault'` 0 error 0 warning
  - tsc 0 error
  - bun run test 全过
  - 手动 e2e 完整流程

---

## 执行顺序与依赖

```
T1 (UUID 改造) ──→ T2 (文件存储) ──→ T3 (git wrapper) ──→ T4 (同步引擎) ──→ T5 (UI) ──→ T6 (文档)
                  ↑                    ↑                    ↑
                  └─ T2 依赖 T1 的 UUID id                   │
                                       └─ T4 依赖 T2+T3 ────┘
                                                                              ↓
                    ┌─────────────────────────────────────────────────────────────────────────┐
                    ↓                ↓                        ↓                              ↓
              T7 (私有库检测)    T8 (HTTPS→SSH)        T9 (非交互 prompt)          T11 (outline 稳定)
              依赖 T4 入口        依赖 T4 入口 +          依赖所有 git 命令             依赖 T4 push_to_files
                                  T3 git wrapper
                                                              ↓
                                                        T10 (空远程首次推送)
                                                        依赖 T4 sync_now + T9 git_command
```

T1 必须先做（其他都依赖 UUID 字符串 id）。T2/T3 可并行。T4 依赖 T2+T3。T5 依赖 T4。

增补任务依赖：
- **T7**：T4 `add_remote` / `clone_from` 入口（守卫挂在入口前）
- **T8**：T4 入口 + T3 git wrapper（用 ssh -T 和 git remote set-url）
- **T9**：所有 git 命令路径（git_command helper 接入 4 个底层入口）+ T8 引入的 `verify_ssh_key_for_host`
- **T10**：T4 sync_now + T9（错误识别依赖 classify_git_error）
- **T11**：T4 push_to_files + T2 store（Outline struct 定义在 outline.rs，export 在 store.rs）

## 预估工程量

| Task | 预估行数 | 预估时间 |
|---|---|---|
| T1 UUID 改造 | 300 行（含迁移）| 1-2 小时 |
| T2 文件存储 | 400 行 | 2-3 小时 |
| T3 git wrapper | 300 行 | 1-2 小时 |
| T4 同步引擎 | 400 行 | 3-4 小时 |
| T5 UI | 300 行 | 2-3 小时 |
| T6 文档测试 | 300 行 | 1-2 小时 |
| T7 私有库检测 | 600 行（含测试）| 2-3 小时 |
| T8 HTTPS→SSH 自动改写 | 400 行（含 sync_now 兜底）| 2-3 小时 |
| T9 非交互 prompt 防护 | 100 行（git_command helper + CredentialsRequired）| 0.5-1 小时 |
| T10 空远程首次推送 | 80 行（MergeFfResult enum + sync_now 分流）| 0.5-1 小时 |
| T11 outline 稳定性 + 报告数 | 150 行（BTreeMap + count_outline_changes）| 0.5-1 小时 |
| T12 md5 增量同步 + 目录重构 | 800 行（fingerprint + incremental_export + .sync/vault/ + 字段重命名 + 集成测试）| 3-4 小时 |
| **总计** | **~4130 行** | **16-25 小时** |

（spec 估的 1300 行偏少——加上 schema 迁移 + 测试 + UI + 私有库检测 + 同步健壮性套件 + md5 协议实际接近 4130 行）

## 风险提示

- **T1 风险最高**：cipher id 类型从 i64 改 String 涉及面广（infra + vault + desktop + 前端），漏改一处编译失败。建议改完后跑全量测试
- **T4 同步引擎**复杂度最高：文件系统 ↔ SQLite 双向同步要处理多种边界（新增/修改/删除/冲突）。建议先写集成测试覆盖典型场景
- **T5 e2e 测试**需要真实的 GitHub repo + SSH key——CI 环境难做，主要靠手动测试
- **T7 私有库检测**依赖外网（GitHub/Gitee API + ls-remote）——CI 不稳定，主要靠 `#[ignore]` 集成测试 + 手动验证；rate limit（60/h/IP）虽低但用户加 remote 频率低足够

---

## 实施记录

T1-T5 全部完成（2026-07-21）。T6 文档同步 + 测试收尾。

### Commit 列表

| Task | Commit | 内容 |
|---|---|---|
| T1 | `7d74c6bd` | cipher/folder id 从 i64 改 UUID 字符串（v43→v44）|
| T2 | `8568eec0` | 文件存储模块（sync/store.rs + outline.rs + error.rs）|
| T3 | `44e91ac8` | git 命令 wrapper（sync/git.rs，shell out 系统 git）|
| T4 | `bc65273b` | 同步引擎（sync/engine.rs，fetch→merge→pull/push→commit→push）|
| T5 | `f4e81ef3` | Git 同步 UI（vault_sync_commands.rs + SyncPanel.tsx）|

### 关键决策变化

1. **v43→v44（不是 v38→v39）**：plan 原写 v38→v39，但 user_version 已经到 v43（main 上其他功能推进），实际是 v43→v44。
2. **删除冗余 `if v >= 17` 分支**：原 init_schema 有两个 v>=17 分支（v40/v42 升级 + v44 迁移），第二个冗余（已被 v44 分支覆盖），删除避免重复执行。
3. **UUID 迁移用 `lower(hex(randomblob(16)))`**：不是标准 v4 UUID（无版本位），但全局唯一足够（真正 v4 UUID 在 create_cipher 时用 `Uuid::new_v4()` 生成）。
4. **测试隔离用 thread_local 而非 env var**：`octopus_config_home()` 是 `Lazy<PathBuf>`（首次调用后固定），env var 重定向不生效。改用 `thread_local TEST_VAULT_ROOT` override（与 `set_test_db` 同模式）。
5. **`git_commit` 返 bool**：`nothing to commit` 不是错误——让上层跳过无变化的 commit。
6. **`git_merge_ff` 返 bool**：成功 ff / 不能 ff（需 rebase），让上层走兜底路径。
7. **push_to_files 全量写**：简化实现（cipher < 1000 时毫秒级），不做增量文件 diff。

### 测试基线

最终基线（T1-T12 全部完成后）：

- vault: **257 pass**（含 fingerprint 7 + incremental_export 5 + 集成测试 3 + outline 序列化等）
- desktop: **387 pass**
- 前端: tsc + vite build 0 error
- cargo build: 0 error 0 warning
- 真实网络集成测试（`#[ignore]`）：5 个

历史基线演进：T1-T6 完成 200 → T7 私有库检测 230 → T8 HTTPS→SSH 238 → T9 非交互 prompt 240 → T10 空远程 241 → T11 outline 稳定 247 → T12 md5 增量 + 目录重构 + 集成测试 257。

---

## Task 7: 私有库检测（2026-07-21 增补）

**目标**：`add_remote` / `clone_from` 入口拦截公有库——AES-256-GCM 加密虽强，但密文泄露给攻击者做离线爆破仍是失败。详见 [spec §4.8](../specs/2026-07-21-vault-git-sync-design.md#48-私有库检测守卫2026-07-21-增补)。

**Files:**
- `crates/vault/Cargo.toml`
- `crates/vault/src/sync/git.rs`
- `crates/vault/src/sync/privacy.rs`（新建）
- `crates/vault/src/sync/error.rs`
- `crates/vault/src/sync/engine.rs`
- `crates/vault/src/sync/mod.rs`
- `crates/desktop/frontend/src/pages/Settings/Vault/SyncPanel.tsx`
- `crates/desktop/frontend/src/locales/{en,zh-CN}.yaml`

### Steps

- [x] **7.1 加 ureq 依赖**
  - `crates/vault/Cargo.toml`：`ureq = { version = "2", features = ["json", "tls"] }`（同步 HTTP 客户端，与 sync 模块阻塞 shell-out 风格一致）

- [x] **7.2 ls-remote 带超时**
  - `crates/vault/src/sync/git.rs` 新增 `git_ls_remote_with_timeout(url, timeout_secs)` + `LsRemoteResult` struct
  - 实现：`spawn` + `try_wait` 轮询 + 超时 `child.kill()`（macOS 无 `timeout` 命令）
  - 关键：设 `GIT_TERMINAL_PROMPT=0` + `GIT_ASKPASS=""` + `SSH_ASKPASS=""`——私有 HTTPS 库遇 401/404 立即失败而非卡死等输入

- [x] **7.3 URL 解析 + 检测引擎**
  - `crates/vault/src/sync/privacy.rs`（新建，~500 行含测试）
  - `GitRemoteUrl::parse` 支持 5 种格式：HTTPS / HTTPS+userinfo / SSH scp-like（正则）/ SSH explicit / file
  - `check_privacy` 分流：github.com → GitHub API、gitee.com → Gitee API、其他 HTTPS → ls-remote 嗅探、SSH → SshUnverifiable、file → Err(LocalPathRejected)
  - `PrivacyVerdict` enum：`Public` / `Private` / `Ambiguous(String)` / `SshUnverifiable` / `NetworkError(String)`
  - HTTP via `ureq`，User-Agent = `octopus-vault-sync`（GitHub API 强制要求）

- [x] **7.4 SyncError 扩展**
  - `crates/vault/src/sync/error.rs` 加 `PublicRepoRejected(url)` + `LocalPathRejected`
  - Display：含用户可读建议（"请把仓库改为 Private"、"请使用 GitHub/Gitee URL"）

- [x] **7.5 engine 接入**
  - `crates/vault/src/sync/engine.rs` 加 `ensure_private_repo(url)` 守卫
  - `add_remote` 入口、`clone_initial` 入口（`git_clone` 之前）都调守卫
  - `PublicRepoRejected` 硬阻断；其他 verdict 记日志放行
  - `crates/vault/src/sync/mod.rs` 注册 `pub mod privacy`

- [x] **7.6 前端 UX**
  - SyncPanel.tsx：「添加 remote」按钮 busy 时显 spinner；clone 按钮 busy 时显 `checkingPrivacy`
  - 两处表单下加常驻 `privacyHint` 文案
  - 失败时直接展示 `SyncError.to_string()`（已有路径）

- [x] **7.7 i18n**
  - `locales/en.yaml` + `locales/zh-CN.yaml` 加 `privacyHint` + `checkingPrivacy`

- [x] **7.8 测试覆盖**
  - URL 解析：HTTPS / HTTPS+userinfo / SSH scp-like / SSH explicit / file / 相对路径 / 自建 host（17 个测试）
  - 检测分流：file → 拒绝、SSH → SshUnverifiable、未知 scheme → 拒绝（4 个）
  - ls-remote 解读：success+refs、success+0refs、terminal prompts、DNS fail、超时（5 个）
  - engine：ensure_private 对 local path / SSH / 未知 scheme 的处理（3 个）
  - error Display：PublicRepoRejected 含 URL、LocalPathRejected 含提示（2 个）
  - `#[ignore]` 真实网络集成（3 个）：GitHub/Gitee 公有库检测、GitHub 不存在库 404 歧义

### 验证命令

```bash
cargo build --release -p octopus-vault -p octopus-desktop
cargo test -p octopus-vault --lib
cargo test -p octopus-desktop
cd crates/desktop/frontend && npx tsc --noEmit && npx vite build
# 集成测试（手动，需联网）
cargo test -p octopus-vault --lib sync::privacy::tests::integration_ -- --ignored
```

### 关键设计决策

1. **未认证 API 只能"确认公有"**：GitHub/Gitee 私有库未认证查询返 404（与"不存在"无法区分），Phase 1 不带 PAT，所以 404 归 Ambiguous 放行
2. **ls-remote 而非 HTTP HEAD**：自建 GitLab/Gitea 等也兼容（git 协议层面统一）；API 路径仅覆盖 github.com/gitee.com
3. **超时用代码层 spawn + try_wait 轮询**：macOS 无 `timeout` 命令，且 `mpsc`+`thread` 方案超时后无法 kill 子进程会留僵尸
4. **`GIT_TERMINAL_PROMPT=0`**：私有 HTTPS 库 ls-remote 会被拦住要用户名，必须设 0 让 git 立即失败而非卡死
5. **网络错误放行不阻断**：检测失败归 Ambiguous/NetworkError 都放行——用户加 remote 频率低，宁可少阻断也不卡 UI
6. **本地路径直接拒绝**：同步意义为 0，且暴露本地文件结构

### 实施记录

T7 全部完成（2026-07-21，commit `82f3c355`）。vault 测试 200 → 230（新增 30 个，含 3 个 `#[ignore]` 真实网络集成测试）。0 error 0 warning。

**实测验证**（spec 编写时跑的 `cargo test --ignored`）：
- `https://github.com/octocat/Hello-World.git` → API 200 + `private:false` → Public，被拒 ✅
- `https://gitee.com/mirrors/kubernetes.git` → API 200 + `private:false` → Public，被拒 ✅
- `https://github.com/octocat/nonexistent-xyz.git` → API 404 → Ambiguous，放行 ✅

---

## Task 8: HTTPS → SSH 自动改写（2026-07-21 增补）

**目标**：用户从浏览器复制的 GitHub/Gitee URL 默认 HTTPS，但 GitHub 自 2021-08 起禁用 HTTPS 密码认证仅支持 PAT。用户已踩坑：`Password authentication is not supported`。octopus 自动把 HTTPS URL 改写成 SSH URL，让 `~/.ssh/` 私钥接管认证。详见 [spec §4.9](../specs/2026-07-21-vault-git-sync-design.md#49-httpsssh-自动改写2026-07-21-增补)。

**Files:**
- `crates/vault/src/sync/privacy.rs`（`try_convert_https_to_ssh`）
- `crates/vault/src/sync/git.rs`（`verify_ssh_key_for_host`）
- `crates/vault/src/sync/engine.rs`（`maybe_rewrite_to_ssh` + add_remote/clone_initial 接入）

### Steps

- [x] **8.1 URL 转换函数**
  - `privacy::try_convert_https_to_ssh(url) -> Option<String>`
  - 仅 github.com / gitee.com 的 HTTPS URL 转 SSH（scp-like）；其他返 None
  - 支持 `https://user:token@...`（丢 userinfo）

- [x] **8.2 SSH key 预检**
  - `git::verify_ssh_key_for_host(host) -> Result<bool, SyncError>`
  - `ssh -T -o ConnectTimeout=5 -o StrictHostKeyChecking=accept-new -o BatchMode=yes git@<host>`
  - 识别 GitHub（"successfully authenticated"）+ Gitee（"Welcome to Gitee"）的成功标志

- [x] **8.3 engine 接入（add_remote / clone_initial）**
  - `engine::maybe_rewrite_to_ssh(url) -> Result<String, SyncError>`：组合 URL 转换 + 预检
  - `add_remote` / `clone_initial` 入口（私有库守卫之后）调用
  - SSH key 不可用 → 保留 HTTPS（不阻断，后续 push 失败由 toast 暴露）

- [x] **8.4 sync_now 兜底改写**
  - `git::git_remote_set_url(path, name, url)`：shell out `git remote set-url`
  - `engine::ensure_remotes_use_ssh_when_possible(root)`：sync_now 入口遍历 remote 改写
  - 解决场景：用户在自动改写功能加上之前已 add HTTPS URL；或 SSH key 后装

- [x] **8.5 测试覆盖**
  - URL 转换 7 个单测（github/gitee/userinfo/ssh 输入/自建 host/file/空路径）
  - engine rewrite 1 个单测（非 github/gitee URL 不改写）
  - `git_remote_set_url_updates_url` 1 个单测
  - 1 个 `#[ignore]` 真实 SSH key 验证集成测试（GitHub）

### 关键设计决策

1. **白名单仅 github.com / gitee.com**：自建 GitLab/Gitea/GHE 不在列——SSH 端口可能被封 / 改端口 / 未启用
2. **SSH key 不可用不阻断**：保留 HTTPS，让 push 错误（如 "Password authentication is not supported"）通过 toast 暴露给用户
3. **存储 SSH URL 而非 HTTPS**：用户在 SyncPanel 看到的与 .git/config 一致，避免认知混淆
4. **BatchMode=yes**：SSH 永不交互，避免卡密码 prompt
5. **StrictHostKeyChecking=accept-new**：首次连接自动接受 host key，省去用户先手动 `ssh -T` 一次
6. **sync_now 兜底覆盖老 remote**：add_remote 只对**新加的** remote 生效，sync_now 入口遍历所有 remote 兜底改写——避免用户在功能加上前已 add 的 HTTPS remote 继续卡住

### 实施记录

T8 全部完成（2026-07-21）。vault 测试 230 → 239（新增 9 个：7 个 URL 转换 + 1 个 engine rewrite + 1 个 set-url + 1 个 `#[ignore]` SSH key 验证）。0 error 0 warning。

**实测验证**（本机已配 SSH key）：
- `ssh -T git@github.com` → "Hi tryternity! You've successfully authenticated..." ✅
- `integration_verify_ssh_key_for_github` → Ok(true) ✅

**踩坑修正**：初版只覆盖 add_remote/clone_initial 入口，用户实测发现老 HTTPS remote 仍卡住——补 8.4 sync_now 兜底。

---

## Task 9: 非交互 prompt 防护（2026-07-21 增补）

**目标**：octopus 在 Tauri 后端进程跑 git，stdin 脱离终端——任何凭据 prompt（用户名/密码）都让 UI 无限转圈，用户无法交互输入。详见 [spec §4.10](../specs/2026-07-21-vault-git-sync-design.md#410-非交互-prompt-防护2026-07-21-增补)。

**Files:**
- `crates/vault/src/sync/git.rs`（`git_command` helper + 接入 4 个底层入口）
- `crates/vault/src/sync/error.rs`（`CredentialsRequired` 变体 + classify 识别）

### Steps

- [x] **9.1 git_command helper**
  - 统一构造 Command：`GIT_TERMINAL_PROMPT=0` + `GIT_ASKPASS=` + `SSH_ASKPASS=` + stdin `/dev/null`
  - 三层防御，任一生效都能避免卡死

- [x] **9.2 接入 4 个底层入口**
  - `run_git` / `run_git_allow_codes` / `git_ls_remote` / `git_ls_remote_with_timeout` 全部用 `git_command`
  - 覆盖 fetch / push / clone / merge / commit / add / remote 等所有 git 命令

- [x] **9.3 SyncError::CredentialsRequired**
  - 新增变体，Display 含用户可读建议（配 SSH key 或换 SSH/PAT URL）
  - classify_git_error 识别 5 个 stderr 关键字：`terminal prompts disabled` / `could not read username/password` / `authentication failed` / `password authentication is not supported` / `invalid username or token`

- [x] **9.4 测试**
  - `classify_credentials_required` 单测覆盖 GitHub HTTPS 失败的两种典型 stderr

### 关键设计决策

1. **不实现 UI 凭据输入**：octopus 设计原则不接触凭证，SSH key 一次配置永久有效；UI 凭据输入要处理密码存储 / Keychain 集成，工程量大且体验差
2. **三层防御冗余**：环境变量 + stdin null 双保险，避免任一机制失效时卡死
3. **stdin /dev/null 必需**：仅靠 `GIT_TERMINAL_PROMPT=0` 不够，某些 git 子命令或 askpass 可能绕过——stdin /dev/null 是最终兜底
4. **错误分类引导用户**：失败不静默，toast 明确说"请配 SSH key"——让用户知道下一步该做什么

### 实施记录

T9 全部完成（2026-07-21）。vault 测试 239 → 240（新增 1 个 classify_credentials_required）。0 error 0 warning。

**触发场景**：用户实测报告「点同步后控制台输出 `Username for 'https://github.com':` 然后无限转圈」——根本原因是 sync_now 走 fetch/push 时 git 试图从 TTY 读用户名，但 Tauri 后端无 TTY。

---

## Task 10: 空远程仓库首次推送（2026-07-21 增补）

**目标**：用户在 GitHub/Gitee 新建空仓库（不勾 README）后首次点同步，原流程的 `git merge --ff-only origin/main` 因 `origin/main` 不存在报错。详见 [spec §4.11](../specs/2026-07-21-vault-git-sync-design.md#411-空远程仓库首次推送2026-07-21-增补)。

**Files:**
- `crates/vault/src/sync/git.rs`（`MergeFfResult` enum + `git_merge_ff` 签名变更）
- `crates/vault/src/sync/engine.rs`（sync_now 分流 + 首次 push -u）

### Steps

- [x] **10.1 MergeFfResult enum**
  - 替换 `git_merge_ff` 返回类型从 `Result<bool, _>` → `Result<MergeFfResult, _>`
  - 3 个变体：`FastForwarded` / `CannotFastForward` / `NoUpstream`
  - NoUpstream 判关键字：`not something we can merge` / `invalid upstream` / `not a valid ref` / `unknown revision`

- [x] **10.2 sync_now 分流**
  - `NoUpstream` → 设 `is_first_push = true`，跳过 merge/rebase
  - `FastForwarded` → 继续 pull/push 正常流程
  - `CannotFastForward` → rebase 兜底（原逻辑）

- [x] **10.3 首次 push 用 -u**
  - `is_first_push = true` 时调 `git_push_set_upstream`（`git push -u origin main`）
  - 设完 upstream 后后续 sync_now 走正常 `git_push`

- [x] **10.4 测试**
  - `git_merge_ff_returns_no_upstream_when_branch_missing`：本地 init + commit + merge 不存在的 origin/main → 断言 NoUpstream

### 关键设计决策

1. **错误信号即状态信号**：让 git 自然报错后从 stderr 判断状态——比 fetch 前 ls-remote 预检测简单（少一次网络往返）
2. **enum 而非 bool**：bool 只能区分 ff vs 不能 ff，无法表达 NoUpstream 三态——必须 enum
3. **首次 push 用 -u**：符合 git 习惯，后续 sync_now 走正常 push
4. **不预检测**：直接试 merge 是最自然的状态探测——fetch 后 origin/main ref 已在本地（如果存在）

### 实施记录

T10 全部完成（2026-07-21）。vault 测试 240 → 241 pass（新增 1 个 NoUpstream 测试）。0 error 0 warning。

**触发场景**：用户实测报告「同步错误：git 错误：fatal: invalid upstream 'origin/main'。我只是在远程建立了一个空仓库，还没有任何分支，需要支持这种场景」。

---

## Task 11: outline 序列化稳定性 + SyncReport 真实变更数（2026-07-21 增补）

**目标**：修两个叠加 bug——(A) outline.json 因 HashMap 顺序随机导致字节级变化，git 误判为变化产生空 commit；(B) push_to_files 返总数而非变更数，SyncReport 误导用户。详见 [spec INV-S16/17](../specs/2026-07-21-vault-git-sync-design.md#5-不变量)。

**Files:**
- `crates/vault/src/sync/outline.rs`（HashMap → BTreeMap）
- `crates/vault/src/sync/store.rs`（export_all_to_files entries 同步改 BTreeMap）
- `crates/vault/src/sync/engine.rs`（push_to_files 对比 outline + count_outline_changes）

### Steps

- [x] **11.1 outline BTreeMap**
  - `Outline.ciphers` / `Outline.folders`：`HashMap<String, OutlineEntry>` → `BTreeMap<...>`
  - `Default::default()` + 所有测试同步
  - store.rs `export_all_to_files` 内部 entries 也改 BTreeMap，删 unused HashMap import

- [x] **11.2 push_to_files 对比变更**
  - 读旧 outline（`store::read_outline_file().unwrap_or_default()`）
  - export 后拿新 outline
  - `count_outline_changes(old, new) -> usize` 对比 cipher + folder 的 sha 变化（新增/修改/删除各算 1）

- [x] **11.3 测试**
  - `outline_serialization_is_deterministic`：同输入两次序列化字节一致 + aaa 在 zzz 前（BTreeMap 字典序）
  - `count_outline_changes_*` 5 个场景（zero / new / modified / deleted / mixed）

### 关键设计决策

1. **BTreeMap 而非手动 sort keys**：BTreeMap 是天然有序容器，零运行时开销，避免每次写盘前手动排序
2. **对比新旧 outline 算变更数**：而非「文件 mtime 变化」——mtime 在 git add -A 后意义不大，sha 才是权威
3. **删除也算变更**：cipher 被软删（SQLite deleted_at）→ export 时不写文件 → outline 没 entry → 旧有新无 = 1 变更

### 实施记录

T11 全部完成（2026-07-21，commit `2ac4c028`）。vault 测试 241 → 247 pass（新增 6 个：1 个 outline 序列化稳定 + 5 个 count_outline_changes）。0 error 0 warning。

**触发场景**：用户实测报告「每次同步都是'同步完成：拉取 0 条，推送 4 条'，应该连推送都没有啊，因为本地和 remote 是已经同步过的」。git log 3 个连续 sync commit diff 只有 outline.json 的 HashMap key 顺序变化。

---

## Task 12: md5 增量同步协议（2026-07-21 增补）

**目标**：`push_to_files` 从「全量重写 ciphers/ 目录」改为「md5 diff + 只写变化的文件」——解决 git history 噪音 + 性能浪费。详见 [spec §4.12](../specs/2026-07-21-vault-git-sync-design.md#412-md5-增量同步协议2026-07-21-增补)。

**Files:**
- `crates/vault/Cargo.toml`（加 md-5 依赖）
- `crates/vault/src/sync/fingerprint.rs`（新建——md5 计算）
- `crates/infra/src/db.sql` + `db.rs`（schema v44→v45：sync_md5 字段 + VaultCipher/VaultFolder struct + INSERT/UPDATE）
- `crates/vault/src/storage/cipher.rs` + `folder.rs`（写命令填 sync_md5）
- `crates/vault/src/sync/store.rs`（incremental_export 新函数）
- `crates/vault/src/sync/engine.rs`（push_to_files 改用 incremental_export）

### Steps

- [x] **12.1 fingerprint 模块**
  - `cipher_md5(&VaultCipher) -> String` + `folder_md5(&VaultFolder) -> String`
  - `cipher_md5_from_input(id, &VaultCipherInput)` + `folder_md5_from_fields(id, name, sort_order)`
  - 字段固定顺序 `|` 分隔，不含 created_at/updated_at

- [x] **12.2 schema v44→v45 迁移**
  - vault_ciphers / vault_folders 加 sync_md5 TEXT 字段
  - ALTER TABLE 检查列存在（开发期中间 binary 可能跳版本号）
  - 不回填——首次 sync 当作「需写文件」处理（旧数据 sync_md5=NULL）
  - 全新库 db.sql 已含字段，直接设 v45

- [x] **12.3 写命令填 sync_md5**
  - storage::create_cipher / save_cipher：构造 VaultCipherInput 后算 md5 填入
  - storage::soft_delete / restore：DB 操作后读完整 row 重算 md5（deleted_at 变了）
  - storage::create_folder / rename_folder：算 md5 传入 db 函数
  - pull 从文件读 cipher 写 SQLite：也算 md5 填入

- [x] **12.4 incremental_export**
  - 读旧 outline + 对比 sync_md5 → 只写变化文件 + 删 SQLite 无的
  - 返 (new_outline, changed_count)
  - outline.sha 字段值改用 SQLite sync_md5（不是文件字节 sha256）
  - 保留 export_all_to_files 给 push_initial（首次启用同步）

- [x] **12.5 push_to_files 改用 incremental_export**
  - 删原 count_outline_changes 函数 + 5 个测试（被 incremental_export 取代）
  - push_to_files 直接返 incremental_export 的 changed_count

- [x] **12.6 测试**
  - fingerprint 7 个测试（确定性 / 时间戳不变 / 内容变 / None 字段 / folder / md5 格式）
  - incremental_export 4 个测试（0 变更 / 只写变化 / 删文件 / outline 用 sync_md5）

- [x] **12.7 .sync/vault/ 目录结构重构**（2026-07-22 增补）
  - 用户反馈：「目录结构不对，没按要求 vault 子目录，还是放在根目录下面」
  - `store::sync_root() = ~/.octopus/.sync/`（git repo 根）
  - `store::vault_dir() = sync_root/vault/`（vault 数据子目录）
  - `vault_root()` 保留为 `vault_dir()` 别名（向后兼容旧调用方）
  - engine.rs 所有 git 命令（init/fetch/push/commit/remote/clone）改用 sync_root
  - 文件操作（meta/outline/ciphers/folders）走 vault_dir
  - VaultRootGuard 测试 helper 改用 .sync 路径

- [x] **12.8 outline 字段重命名 + vault_version 修复**（2026-07-22 增补）
  - 用户反馈：「sha 改为 md5」「updated_at 改为毫秒数好比较」「vault_version 每次都 +1」
  - `OutlineEntry.sha` → `md5`（字段名与值一致，不做旧文件兼容）
  - `OutlineEntry.updated_at`（ISO 字符串）→ `updated_ms`（i64 Unix 毫秒）
  - `merge_outlines` 用 updated_ms 数值比较替代 ISO 字符串比较
  - `iso_to_unix_ms()` helper（civil_to_days 公式，无 chrono 依赖）
  - `incremental_export` vault_version 只在 changed > 0 时 +1

- [x] **12.9 push 逻辑简化**（2026-07-22 增补）
  - 用户反馈：「第一次同步，就报已是最新无需同步」
  - 根因：之前用 `committed`（这次 sync 是否新 commit）判断 push——首次同步时
    enable_sync 已 commit init vault，sync_now 没新 commit 就跳过 push
  - 用户指出：「如果 git 有本地变更没推送，就可以 push。甚至直接 push，
    push 零变化也是 ok 的」
  - 修复：无条件 push——git push 幂等，零变化返 Everything up-to-date
  - 删掉中间方案 `git_needs_push` 函数（rev-list ahead/behind 预判——多余）

- [x] **12.10 集成测试**（2026-07-22 增补）
  - 用户反馈：「这些问题你写测试就应该可以发现，需要多写测试」
  - `enable_sync_creates_git_in_sync_root_not_vault_dir`——验证 .git 位置
  - `enable_sync_writes_vault_data_in_vault_subdir`——验证 vault 数据在子目录
  - `outline_json_uses_md5_and_updated_ms_field_names`——验证字段名
  - `incremental_export_vault_version_only_increments_on_change`——验证版本不递增
  - IntegrationGuard：内存 DB + tempdir + 预置 vault_meta

### 关键设计决策

1. **md5 不是安全 hash**：纯 diff 工具，碰撞无实际风险（cipher 数 < 10万）
2. **不含时间戳**：created_at/updated_at 跨设备必然不同，含了会导致永久 diff
3. **密文字段安全**：cipher 只在创建机器加密一次，sync 搬运密文——跨设备一致（详见 spec §2.4）
4. **不回填 md5**：避免在 infra 层引入 md5 依赖，让首次 sync 自然填上
5. **outline 字段名最终改为 md5/updated_ms**（之前曾考虑保留 sha 字段名做兼容，用户明确说不做兼容）
6. **push 无条件执行**：git push 幂等，不需要预判是否领先——用户原话「直接 push，push 零变化也是 ok 的」

### 实施记录

T12 全部完成（2026-07-22，commit `3c71c83c`）。vault 测试 247 → 257 pass。0 error 0 warning。

**触发场景汇总**（4 个用户实测反馈）：
1. 「目录结构不对，没按要求 vault 子目录」→ 12.7 目录重构
2. 「sha 改 md5，updated_at 改毫秒数」+ 「vault_version 每次 +1」→ 12.8 字段重命名 + 版本修复
3. 「第一次同步报已是最新」→ 12.9 push 逻辑简化
4. 「这些问题测试应该可以发现」→ 12.10 补集成测试

**架构演进伏笔**：fingerprint.rs + incremental_export 的设计为未来扩展 hotword/prompts 同步打基础——同一种 md5 diff 模式可复用（目前不抽象 trait，YAGNI）。

---

## Task 13: 热词同步 + sync crate 抽离（2026-07-22 增补）

**目标**：扩展 `.sync/` 目录支持热词同步——这是 `.sync/` 目录扩展的第一个新数据类型。前置改造：把通用 sync 代码抽到独立 `crates/sync/` crate；热词 id 从 i64 改 UUID 字符串（与 cipher 一致）。

**背景**：vault git 同步已完成 Phase 1（T1-T12）。现在要扩展 `.sync/` 目录，让热词也能跨设备同步。热词 `words_text` 已 normalize（拼音首字母排序 + 去重）——跨设备字节一致，天然适合 md5 diff。

**Spec 补充**：详见 [spec §4.13 热词同步协议](../specs/2026-07-21-vault-git-sync-design.md#413-热词同步协议2026-07-22-增补)（T10 同步写入）

### 关键设计决策（已确认）

1. **热词同步主键**：改 `hotword_sets.id` 为 TEXT UUID（与 cipher 完全一致，不是新增 sync_uuid 字段）
2. **明文同步**：热词不加密（当前 SQLite 也是明文，热词不含密码等高敏感信息）
3. **sync 代码放新建 `crates/sync/` crate**（不放在 vault crate）
4. **热词 md5 算在 sync crate**（保持 infra 无 md-5 依赖——与 vault cipher 的 sync_md5 在 vault crate 算同一思路）
5. **热词数据结构（HotwordSet struct）留 infra crate**，热词 sync 逻辑作为 `octopus_sync::hotword` 模块
6. **热词文件格式 HotwordSetFile 放 sync crate**（依赖 infra，import `HotwordSet`）
7. **跨 crate 依赖**：infra ← sync ← vault ← desktop；infra ← desktop（desktop 同时依赖 sync + infra）

### 目标目录结构

```
~/.octopus/.sync/             ← git repo 根（sync_root，已实现）
├── .git/
├── vault/                    ← 已实现（T1-T12）
│   ├── meta.json
│   ├── outline.json
│   ├── ciphers/<2hex>/<uuid>.json
│   └── folders/<2hex>/<uuid>.json
└── hotword/                  ← 本次新增
    ├── outline.json          ← {uuid → {md5, updated_ms}}
    └── sets/<2hex>/<uuid>.json
```

### 三个 Phase + 10 个子 Task

| 子 Task | 目标 | 主要文件 |
|---|---|---|
| 13.1 (Phase A) | 新建 `crates/sync/`，抽离通用 sync 代码 | `crates/sync/*`（新建）+ workspace Cargo.toml |
| 13.2 (Phase A) | vault crate 适配（删搬走文件 + re-export + 测试） | `crates/vault/src/sync/*` |
| 13.3 (Phase B) | schema v45→v46 迁移（id 改 UUID + sync_md5 字段） | `crates/infra/src/db.{rs,sql}` |
| 13.4 (Phase B) | 热词 CRUD 函数签名 i64→String（~15 个） | `crates/infra/src/db.rs` |
| 13.5 (Phase B) | 11 个 Tauri 命令 + 前端 HotwordPanel.tsx | `crates/desktop/src/hotword_commands.rs` + 前端 |
| 13.6 (Phase C) | 热词 fingerprint + store（md5 + HotwordSetFile + export/import） | `crates/sync/src/hotword.rs`（新建） |
| 13.7 (Phase C) | 热词 sync engine（pull/push files）+ 集成主 sync_now | `crates/sync/src/hotword.rs` + `crates/vault/src/sync/engine.rs` |
| 13.8 (Phase C) | 热词写命令填 sync_md5 | `crates/desktop/src/hotword_commands.rs` |
| 13.9 | 测试（md5 + 增量 export + 迁移 + 集成） | 各 crate `#[cfg(test)]` |
| 13.10 | 文档同步（spec + architecture.md） | `docs/` |

### 执行顺序

```
Phase A (sync crate 抽离，无功能变化)
  13.1 → 13.2

Phase B (热词 id 改 UUID，与 sync 无关的纯类型迁移)
  13.3 → 13.4 → 13.5

Phase C (热词 sync 实现，依赖 A+B)
  13.6 → 13.7 → 13.8

测试 + 文档贯穿：13.9 各 Task 内嵌 + 13.10 收尾
```

**Phase A 与 Phase B 可并行**（A 改 sync crate 搬运，B 改 infra 热词类型，互不依赖），但建议先 A 后 B——A 是纯机械搬运风险低，B 涉及 schema 迁移 + 前端类型改动影响面大，A 先把 sync crate 基础打好。

---

### 13.1 Phase A-T1：新建 crates/sync/，抽离通用 sync 代码

**目标**：把 vault::sync 中与具体业务数据（cipher/folder）无关的通用 sync 代码抽到独立 `crates/sync/`（octopus-sync）。

**Files:**
- `Cargo.toml`（workspace 根）—— members 加 `crates/sync`
- `crates/sync/Cargo.toml`（新建）
- `crates/sync/src/lib.rs`（新建）
- `crates/sync/src/git.rs`（从 vault 搬来）
- `crates/sync/src/outline.rs`（从 vault 搬来）
- `crates/sync/src/error.rs`（从 vault 搬来）
- `crates/sync/src/privacy.rs`（从 vault 搬来）
- `crates/sync/src/store.rs`（新建——只含通用路径/工具函数）

**Steps:**

- [x] **13.1.1 新建 crate 骨架**
  - `Cargo.toml` members 加 `"crates/sync"`
  - `crates/sync/Cargo.toml`：
    - `[package]` name = "octopus-sync"，edition/workspace 继承
    - `[dependencies]`：`octopus-infra = { path = "../infra" }` + md-5 + sha2 + ureq + serde + serde_json + anyhow + log + regex + parking_lot + base64
    - `[dev-dependencies]`：`tempfile = "3"`
  - `crates/sync/src/lib.rs`：`pub mod error; pub mod git; pub mod outline; pub mod privacy; pub mod store;` + re-export

- [x] **13.1.2 搬 git.rs / outline.rs / error.rs / privacy.rs**
  - 4 个文件整体从 `crates/vault/src/sync/` 复制到 `crates/sync/src/`
  - 内部 `use crate::sync::error::...` → `use crate::error::...`（去掉 sync:: 前缀）
  - 内部 `use crate::sync::git::...` → `use crate::git::...`
  - 这 4 个文件**零 octopus_vault 依赖**（调研已确认），搬过去只改 crate 内路径
  - privacy.rs 测试中 `crate::sync::git::verify_ssh_key_for_host` → `crate::git::verify_ssh_key_for_host`

- [x] **13.1.3 抽 store.rs 通用部分到 sync crate**
  - 新建 `crates/sync/src/store.rs`，只含以下通用项（业务相关的 vault_dir/cipher_file_path 等留 vault）：
    - `sync_root() -> PathBuf`（依赖 `octopus_infra::octopus_config_home`）
    - `shard_dir(uuid: &str) -> String`
    - `sha256_hex(content: &str) -> String`
    - `md5_hex(bytes: &[u8]) -> String`（从 fingerprint.rs 搬来，改为 **pub**）
    - `iso_to_unix_ms(s: &str) -> i64`（outline merge 用，通用工具）
    - `TEST_SYNC_ROOT` thread_local + `set_test_sync_root` + `clear_test_sync_root`（从 vault 的 `TEST_VAULT_ROOT` / `set_test_vault_root` / `clear_test_vault_root` 改名搬来）
  - **关键**：`sync_root()` 改名后所有 vault 引用方（store.rs/engine.rs）改成调 sync crate 的 `sync_root`

- [x] **13.1.4 验证**
  - `cargo build -p octopus-sync` 0 error 0 warning
  - `cargo test -p octopus-sync --lib` 全过（git/outline/error/privacy 测试随文件搬来）

---

### 13.2 Phase A-T2：vault crate 适配

**目标**：vault crate 删除已搬走的文件，改用 `octopus_sync::` 引用通用代码。

**Files:**
- `crates/vault/Cargo.toml`——加 `octopus-sync = { path = "../sync" }`
- `crates/vault/src/sync/mod.rs`
- `crates/vault/src/sync/git.rs`——删除（已搬 sync crate）
- `crates/vault/src/sync/outline.rs`——删除
- `crates/vault/src/sync/error.rs`——删除
- `crates/vault/src/sync/privacy.rs`——删除
- `crates/vault/src/sync/store.rs`——删通用部分，保留 vault 业务部分
- `crates/vault/src/sync/fingerprint.rs`——删 md5_hex（已搬 sync crate），改用 `octopus_sync::store::md5_hex`
- `crates/vault/src/sync/engine.rs`——更新引用路径

**Steps:**

- [x] **13.2.1 加 sync 依赖**
  - `crates/vault/Cargo.toml` `[dependencies]` 加 `octopus-sync = { path = "../sync" }`
  - 移除 vault 独占的 md-5 依赖（md5_hex 已搬 sync crate，vault 通过 sync crate 间接依赖）—— **验证**：sha2 保留（vault store 还有 sha256 用途？检查后再定，可能也搬走）

- [x] **13.2.2 删除已搬走的文件**
  - 删 `crates/vault/src/sync/git.rs` / `outline.rs` / `error.rs` / `privacy.rs`
  - mod.rs 删除对应 `pub mod` 声明

- [x] **13.2.3 store.rs 保留 vault 业务部分**
  - 删除 `sync_root` / `shard_dir` / `sha256_hex` / `TEST_VAULT_ROOT` / `set_test_vault_root` / `clear_test_vault_root` / `iso_to_unix_ms`（已搬 sync crate）
  - 保留 `vault_dir` / `vault_root` / `meta_path` / `outline_path` / `cipher_file_path` / `folder_file_path` / `MetaFile` / `CipherFile` / `FolderFile` / `export_all_to_files` / `incremental_export` / `import_*` 等 vault 业务函数
  - `vault_dir()` 内部调 `octopus_sync::store::sync_root().join("vault")`
  - `cipher_file_path` / `folder_file_path` 调 `octopus_sync::store::shard_dir`
  - 测试 helper `VaultRootGuard` 改用 `octopus_sync::store::set_test_sync_root` / `clear_test_sync_root`

- [x] **13.2.4 fingerprint.rs 改用 sync crate 的 md5_hex**
  - 删除 private `md5_hex` 函数
  - `cipher_md5` / `folder_md5` / `cipher_md5_from_input` / `folder_md5_from_fields` 改用 `octopus_sync::store::md5_hex`
  - md5_hex 测试（`md5_hex_returns_32_chars_lowercase`）随函数搬走，已在 sync crate 覆盖

- [x] **13.2.5 engine.rs 更新引用**
  - 所有 `store::sync_root()` → `octopus_sync::store::sync_root()`
  - `git::*` → `octopus_sync::git::*`
  - `error::SyncError` / `classify_git_error` → `octopus_sync::error::*`
  - `privacy::*` → `octopus_sync::privacy::*`
  - `outline::Outline` → `octopus_sync::outline::Outline`
  - 测试中 `store::set_test_vault_root` → `octopus_sync::store::set_test_sync_root`

- [x] **13.2.6 mod.rs re-export**
  - 删搬走的 re-export，保留 vault 业务项
  - 新增 re-export（方便外部用 `octopus_vault::sync::SyncError` 等）：`pub use octopus_sync::{error::SyncError, git, outline::Outline};`

- [x] **13.2.7 desktop crate 适配**
  - `crates/desktop/Cargo.toml` 加 `octopus-sync = { path = "../sync" }`（vault_sync_commands 可能直接用 sync 类型）
  - `crates/desktop/src/vault_sync_commands.rs` 引用路径更新（如有）

- [x] **13.2.8 验证（Phase A 收尾，无功能变化）**
  - `cargo build --workspace` 0 error 0 warning
  - `cargo test -p octopus-sync --lib` 全过
  - `cargo test -p octopus-vault --lib` 全过（257 pass 基线不变）
  - `cargo test -p octopus-desktop` 全过（387 pass 基线不变）

---

### 13.3 Phase B-T3：schema v45→v46 迁移

**目标**：`hotword_sets.id` 从 INTEGER AUTOINCREMENT 改 TEXT UUID；加 `sync_md5` 字段。

**Files:**
- `crates/infra/src/db.sql`
- `crates/infra/src/db.rs`（init_schema 迁移分支 + HotwordSet struct 字段类型）

**Steps:**

- [x] **13.3.1 db.sql schema 改 TEXT**
  - `hotword_sets.id`：`INTEGER PRIMARY KEY AUTOINCREMENT` → `TEXT PRIMARY KEY`
  - 加 `sync_md5 TEXT` 字段（在 updated_at 后）
  - 默认「通用」版本 INSERT 语句：`INSERT OR IGNORE INTO hotword_sets(id, name, enabled, words_text, sync_md5) VALUES('<固定-uuid>', '通用', 1, '', NULL)`——固定 UUID（如 `"00000000-0000-0000-0000-000000000001"`）保证跨设备一致（「通用」是默认集，两台机器都该有同一个 id）

- [x] **13.3.2 v45→v46 迁移逻辑**
  - init_schema 加 `if v == 45` 分支：
    1. `CREATE TABLE hotword_sets_new (... id TEXT PRIMARY KEY ..., sync_md5 TEXT)`
    2. `INSERT INTO hotword_sets_new SELECT lower(hex(randomblob(16))) AS id, name, enabled, words_text, created_at, updated_at, NULL FROM hotword_sets`（为每行生成 UUID，sync_md5 留 NULL）
    3. `DROP TABLE hotword_sets; RENAME TABLE hotword_sets_new TO hotword_sets;`
    4. `PRAGMA user_version = 46`
  - **测试**：迁移测试覆盖（旧 i64 id → 新 UUID 字符串 + sync_md5=NULL）

- [x] **13.3.3 全新库 v=46 早返**
  - init_schema 最新早返分支 `if v >= 46 { return Ok(()) }`
  - 全新库 INIT_SQL 已含新 schema，设 v46

- [x] **13.3.4 验证**
  - `cargo test -p octopus-infra --lib` 全过（含新迁移测试）

---

### 13.4 Phase B-T4：热词 CRUD 函数签名 i64→String

**目标**：~15 个 DB 函数的 id 参数从 i64 改 String。

**Files:**
- `crates/infra/src/db.rs`（HotwordSet struct + 15 个 CRUD 函数 + row_to_hotword_set + HOTWORD_SET_COLS）

**Steps:**

- [x] **13.4.1 HotwordSet struct + row mapper**
  - `HotwordSet.id: i64` → `String`
  - `HOTWORD_SET_COLS` 加 `sync_md5`：`"id, name, enabled, words_text, created_at, updated_at, sync_md5"`
  - `row_to_hotword_set`：`id: row.get(0)?`（自动 String）+ 加 `sync_md5: row.get(6)?`
  - struct 加 `pub sync_md5: Option<String>` 字段

- [x] **13.4.2 CRUD 函数签名改 String**
  - 影响的函数（来自调研）：
    - `get_hotword_set(id: i64)` → `(id: &str)`
    - `insert_hotword_set(name)` → 改为 `insert_hotword_set(id: &str, name: &str)`（调用方生成 UUID）—— **不再返回 last_insert_rowid**
    - `rename_hotword_set(id: i64, name)` → `(id: &str, name)`
    - `toggle_hotword_set(id: i64, enabled)` → `(id: &str, enabled)`
    - `set_hotword_set_words(id: i64, words_text)` → `(id: &str, words_text)`
    - `add_word_to_set(id: i64, word)` → `(id: &str, word)`
    - `add_words_to_set(id: i64, words)` → `(id: &str, words)`
    - `remove_word_from_set(id: i64, word)` → `(id: &str, word)`
    - `delete_hotword_set(id: i64)` → `(id: &str)`
    - 各 `_at` 内层函数同步改
  - `list_hotword_sets` 的 `ORDER BY id ASC` 改 `ORDER BY name ASC`（UUID 字符串排序无意义，按 name 排对用户友好）

- [x] **13.4.3 验证**
  - `cargo build -p octopus-infra` 0 error
  - `cargo test -p octopus-infra --lib` 全过（hotword 测试 fixture 改 String id）

---

### 13.5 Phase B-T5：Tauri 命令 + 前端适配

**目标**：11 个 Tauri 命令 + 前端 HotwordPanel.tsx 的 id 类型 i64→String。

**Files:**
- `crates/desktop/src/hotword_commands.rs`
- `crates/desktop/frontend/src/pages/Settings/HotwordPanel.tsx`

**Steps:**

- [x] **13.5.1 Tauri 命令签名**
  - 来自调研的 11 个命令，id 参数 i64 → String：
    - `create_hotword_set(name: String)` → 返回 `Result<String, String>`（生成 UUID）+ 内部调 `Uuid::new_v4().to_string()` 传入 `insert_hotword_set`
    - `rename_hotword_set(id: String, name: String)`
    - `delete_hotword_set(id: String)`
    - `toggle_hotword_set(id: String, enabled: bool)`
    - `add_word_to_set(id: String, word: String)`
    - `remove_word_from_set(id: String, word: String)`
    - `add_words_to_set(id: String, words: Vec<String>)`
    - `import_hotwords`：`target_set_id: Option<i64>` → `Option<String>`，返回 `Result<String, String>`
    - `export_hotwords(set_id: i64)` → `set_id: String`

- [x] **13.5.2 前端 HotwordPanel.tsx 类型**
  - `interface HotwordSet { id: number → string }`
  - `selectedId: number | null` → `string | null`
  - `renaming: number | null` → `string | null`
  - 所有 `invoke<number>('create_hotword_set')` → `invoke<string>`
  - callback 参数 `(id: number, ...)` → `(id: string, ...)`
  - invoke 参数 `{ id, ... }` 不变（JS 自动序列化）

- [x] **13.5.3 验证**
  - `cargo build -p octopus-desktop` 0 error 0 warning
  - `cargo test -p octopus-desktop` 全过
  - tsc + vite build 0 error

---

### 13.6 Phase C-T6：热词 fingerprint + store

**目标**：实现热词 md5 指纹 + 文件存储格式（HotwordSetFile）+ 增量 export/import。

**Files:**
- `crates/sync/src/hotword.rs`（新建）
- `crates/sync/src/lib.rs`（加 `pub mod hotword`）

**Steps:**

- [x] **13.6.1 新建 hotword 模块**
  - `crates/sync/src/lib.rs` 加 `pub mod hotword;`
  - `crates/sync/src/hotword.rs`：
    - doc comment 说明热词同步协议（明文 + md5 增量 + `.sync/hotword/` 目录）

- [x] **13.6.2 热词 fingerprint**
  - `hotword_set_md5(h: &HotwordSet) -> String`：拼接 `name | enabled | words_text`（不含 id / created_at / updated_at / sync_md5）
  - `hotword_set_md5_from_fields(name, enabled, words_text) -> String`：写命令填 md5 用（避免重复读 row）
  - 用 `crate::store::md5_hex`
  - **words_text 已 normalize**（拼音首字母排序 + 去重），跨设备字节一致

- [x] **13.6.3 hotword_dir + 文件路径**
  - `hotword_dir() -> PathBuf`：`sync_root().join("hotword")`
  - `hotword_outline_path() -> PathBuf`：`hotword_dir().join("outline.json")`
  - `hotword_set_file_path(uuid: &str) -> PathBuf`：`hotword_dir().join("sets").join(shard_dir(uuid)).join(format!("{}.json", uuid))`
  - 用 `crate::store::{sync_root, shard_dir}`

- [x] **13.6.4 HotwordSetFile struct**
  - ```rust
    pub struct HotwordSetFile {
        pub version: u32,          // = 1
        pub id: String,            // UUID
        pub name: String,          // 明文（热词不加密）
        pub enabled: bool,
        pub words_text: String,    // 已 normalize
        pub created_at: String,
        pub updated_at: String,
    }
    ```
  - `from_hotword_set(h: &HotwordSet) -> Self`
  - `read_hotword_set_file(uuid) -> Result<HotwordSetFile>`
  - `write_hotword_set_file(h: &HotwordSetFile) -> Result<()>`
  - `remove_hotword_set_file(uuid) -> Result<()>`

- [x] **13.6.5 增量 export/import**
  - `incremental_export_hotwords(sets: &[HotwordSet]) -> Result<(Outline, usize)>`：
    - 读旧 outline（`hotword_outline_path`）
    - 对每行：对比 sync_md5 → 跳过/重写/新增
    - SQLite 无 outline 有 → 删文件 + 删 entry
    - 返回 (new_outline, changed_count)
    - outline entry.md5 = sync_md5（与 vault 一致）
  - `export_all_hotwords(sets: &[HotwordSet]) -> Result<Outline>`：首次启用同步用（全量写）
  - `import_hotwords_from_files() -> Result<Vec<HotwordSetFile>>`：pull 用（读所有文件）

- [x] **13.6.6 outline 读写**
  - `read_hotword_outline() -> Result<Outline>`
  - `write_hotword_outline(o: &Outline) -> Result<()>`
  - 复用 `crate::outline::{Outline, OutlineEntry}`（vault 的 outline 结构通用——version/vault_version/ciphers/folders；热词用 ciphers 字段存 hotword set entries，或新增泛型）
  - **决策点**：Outline 的 `ciphers` / `folders` 字段名是 vault 语义。热词复用是否重命名字段？
    - 方案 A：热词 outline 用 `ciphers` 字段存 hotword sets（字段名误导，但复用结构）
    - 方案 B：Outline 泛型化 / 加 `entries: BTreeMap<String, OutlineEntry>` 通用字段
    - **推荐方案 A**（YAGNI，字段名内部细节，outline.json 内容是 `{version, vault_version, ciphers: {uuid: {md5, updated_ms}}}`——热词 outline.json 的 `ciphers` 实际存 hotword sets，语义偏移但功能正确，加注释说明）
  - 热词 outline 的 `vault_version` 字段语义改为「hotword_version」（累计变更计数），字段名不改（复用结构）

- [x] **13.6.7 验证**
  - `cargo build -p octopus-sync` 0 error
  - `cargo test -p octopus-sync --lib hotword` 全过

---

### 13.7 Phase C-T7：热词 sync engine + 集成主 sync_now

**目标**：热词 pull_from_files / push_to_files + 集成到 vault sync_now 流程。

**Files:**
- `crates/sync/src/hotword.rs`（engine 部分）
- `crates/vault/src/sync/engine.rs`（sync_now 集成）

**Steps:**

- [x] **13.7.1 热词 pull_from_files**
  - 读 `~/.octopus/.sync/hotword/outline.json`（merge 后的）
  - 对比 SQLite 现有 hotword_sets：找出新增/修改/删除
  - upsert SQLite（用 infra 的 CRUD 函数）
  - 返回 pulled count

- [x] **13.7.2 热词 push_to_files**
  - 读 SQLite 全部 hotword_sets（含 sync_md5）
  - 调 `incremental_export_hotwords`
  - 写新 outline
  - 返回 pushed count

- [x] **13.7.3 集成主 sync_now**
  - `crates/vault/src/sync/engine.rs` sync_now 流程：
    - pull 阶段（merge ff 后）：除 vault pull 外，加 hotword pull_from_files
    - push 阶段：除 vault incremental_export 外，加 hotword push_to_files
  - SyncReport 加 `hotwords_pulled: usize` + `hotwords_pushed: usize` 字段
  - **注意**：热词 upsert SQLite 需要 `octopus_infra::db` 的 upsert 能力——infra 已有 insert/update，可能需要加 `upsert_hotword_set`（ON CONFLICT(id) DO UPDATE）

- [x] **13.7.4 enable_sync / clone_initial 适配**
  - `enable_sync`（push_initial）：除 export_all_to_files 外，加 `export_all_hotwords`
  - `clone_initial`（import_all_from_files）：除 vault import 外，加 `import_hotwords_from_files` + upsert SQLite

- [x] **13.7.5 验证**
  - `cargo test -p octopus-vault --lib sync::engine` 全过
  - 集成测试：双 tempdir A/B 机热词同步（A 创建热词集 → sync → B clone → 看到热词集）

---

### 13.8 Phase C-T8：热词写命令填 sync_md5

**目标**：所有写热词的命令在写 SQLite 前算 md5 填入 sync_md5 字段。

**Files:**
- `crates/desktop/src/hotword_commands.rs`
- `crates/infra/src/db.rs`（DB 函数签名加 sync_md5 参数）

**Steps:**

- [x] **13.8.1 DB 函数加 sync_md5 参数**
  - insert/update 类函数加 `sync_md5: &str` 参数：
    - `insert_hotword_set(id, name, sync_md5)`
    - `rename_hotword_set(id, name, sync_md5)`——name 变了 md5 变
    - `toggle_hotword_set(id, enabled, sync_md5)`——enabled 变了 md5 变
    - `set_hotword_set_words(id, words_text, sync_md5)`
    - `add_word_to_set` / `add_words_to_set` / `remove_word_from_set`——words_text 变了，内部算 md5（或返回新 md5 让调用方填）
  - **决策**：md5 在 desktop 命令层算（调 `octopus_sync::hotword::hotword_set_md5_from_fields`），传入 DB 函数——保持 infra 不依赖 sync crate

- [x] **13.8.2 desktop 命令层算 md5**
  - 每个 Tauri 写命令（create/rename/toggle/add_word/remove_word/set_words/import）：
    - 操作前算新 md5（用命令参数 + 预期新状态）
    - 或操作后读完整 row 算 md5 再 update（更简单，多一次 DB 读但逻辑清晰）
  - **推荐**：操作后读 row 算 md5 再 update（add_word 等操作 words_text 在 DB 内 normalize，命令层不知道结果，读出来算最准）

- [x] **13.8.3 验证**
  - `cargo test -p octopus-desktop` 全过
  - 手动 e2e：创建热词集 → 查 SQLite sync_md5 非 NULL

---

### 13.9 测试（贯穿各 Task）

**目标**：覆盖 md5 计算 + 增量 export + 迁移 + 集成。

**测试清单：**

- [x] **13.9.1 fingerprint 测试**（13.6 内）
  - `hotword_set_md5_is_deterministic`
  - `hotword_set_md5_ignores_timestamps`
  - `hotword_set_md5_changes_on_content_change`（name/enabled/words_text 各变一次）
  - `hotword_set_md5_normalizes_equivalent_words`（"b a" 和 "a b" normalize 后 md5 相同）

- [x] **13.9.2 增量 export 测试**（13.6 内）
  - `incremental_export_zero_changes`（sync_md5 一致不写文件）
  - `incremental_export_writes_only_changed`（改 name → 只重写该文件）
  - `incremental_export_deletes_missing`（SQLite 删了 → 删文件）
  - `incremental_export_outline_uses_sync_md5`

- [x] **13.9.3 迁移测试**（13.3 内）
  - `migrate_v45_to_v46_hotword_id_to_uuid`（旧 i64 id → 新 UUID + sync_md5=NULL）
  - `migrate_preserves_words_text`

- [x] **13.9.4 集成测试**（13.7 内）
  - `hotword_sync_a_to_b`（A 机创建 → sync → B 机 clone → 看到热词集 + words_text 一致）
  - `hotword_sync_bidirectional`（A 改 name + B 加词 → 双向 sync → 两边都有最新）
  - `hotword_sync_delete_propagates`（A 删热词集 → sync → B 也删了）

---

### 13.10 文档同步

**目标**：spec 加 §4.13 热词同步协议；architecture.md 更新 crate 结构。

**Files:**
- `docs/superpowers/specs/2026-07-21-vault-git-sync-design.md`——加 §4.13
- `docs/superpowers/plans/2026-07-21-vault-git-sync.md`（本文件）——标记完成 + 实施记录
- `docs/architecture.md`——crate 结构 + 热词 sync 段

**Steps:**

- [x] **13.10.1 spec §4.13 热词同步协议**
  - 数据结构（HotwordSetFile + outline 复用）
  - md5 指纹拼接格式
  - 明文同步理由（热词不含高敏感信息）
  - 增量 export 流程（复用 §4.12 模式）
  - 与 vault sync 的集成点（sync_now 同时处理 vault + hotword）

- [x] **13.10.2 architecture.md**
  - workspace crate 列表加 octopus-sync
  - 依赖关系图更新（infra ← sync ← vault ← desktop）
  - 热词 sync 段（目录结构 + 流程）

- [x] **13.10.3 plan 实施记录**
  - 各 Task checkbox 标记完成
  - 实施过程的偏差回写
  - 测试基线更新（vault 257 → ?，sync 新 crate ? pass，desktop 387 → ?）

---

### 预估工程量

| 子 Task | 预估行数 | 预估时间 |
|---|---|---|
| 13.1 sync crate 抽离 | 100 行新建 + 搬运 ~1900 行 | 1-2 小时 |
| 13.2 vault 适配 | 200 行改动（引用路径 + 删文件）| 1-2 小时 |
| 13.3 schema v45→v46 | 100 行（迁移 + 测试）| 0.5-1 小时 |
| 13.4 CRUD 签名 | 150 行（15 函数 × ~10 行）| 1 小时 |
| 13.5 Tauri + 前端 | 200 行（11 命令 + HotwordPanel）| 1-2 小时 |
| 13.6 hotword fingerprint + store | 400 行（md5 + HotwordSetFile + export/import）| 2-3 小时 |
| 13.7 hotword engine + 集成 | 300 行（pull/push + sync_now 集成）| 2-3 小时 |
| 13.8 写命令填 md5 | 100 行 | 0.5-1 小时 |
| 13.9 测试 | 400 行 | 2-3 小时 |
| 13.10 文档 | 200 行 | 1 小时 |
| **总计** | **~2150 行** | **10-16 小时** |

### 风险提示

- **13.1/13.2 crate 抽离风险中**：大量引用路径改动，漏改一处编译失败。改完必须跑全量 workspace build + 全量测试
- **13.3 迁移风险低**：hotword_sets 与其他表无外键，迁移独立
- **13.6/13.7 outline 复用决策**：热词复用 vault 的 Outline struct（ciphers 字段名误导）——如果实施时发现混淆，再考虑泛型化
- **13.8 md5 计算时机**：操作后读 row 算 md5 多一次 DB 读，但保证准确（words_text 在 DB 内 normalize）

---

## Task 13 实施记录

Task 13 全部完成（2026-07-22）。Phase A（sync crate 抽离）+ Phase B（热词 id 改 UUID）+ Phase C（热词 sync 实现）。

### Commit 列表

| 子 Task | Commit | 内容 |
|---|---|---|
| 13.1-13.2 | `2348bd22` | Phase A：抽离 octopus-sync crate（git/outline/error/privacy + store 工具） |
| 13.3-13.5 | `66979c86` | Phase B：热词 id 改 TEXT UUID + sync_md5 字段（schema v46 + CRUD + Tauri + 前端）+ T12 遗留修复 |
| 13.6-13.8 | `a924bffe` | Phase C：热词 git 同步实现（fingerprint + store + engine + 写命令填 md5） |
| 13.9 | `896d26df` | 集成测试（A→B 同步 + 双向 + 删除传播 + v46 迁移） |
| 13.9 补强 | `7eb298a5` | 补强测试覆盖（desktop 7 + vault 集成 4 + 边界 5）+ 修 pull name 冲突 bug |
| 后续 | `8b56119e` | fix: pull_from_files 加 security_stamp 校验（INV-S9）+ Git 同步 Tab 挪到系统设置 |
| 后续 | `41616f26` | feat: 修改主密码入口（ChangePasswordModal + VaultPanel 按钮） |
| 后续 | `e18c7471` | feat: stamp 冲突双向解决（resolve_with_remote / resolve_with_local） |
| 后续 | `00b10c62` | feat: 自动同步（每小时，经 scheduler 调度，Phase 2） |

### 关键决策变化与实施发现

1. **跨 crate 测试隔离不能用 `#[cfg(test)]`**：sync crate 的 `set_test_sync_root` / `sync_root()` thread_local 检查最初用 `#[cfg(test)]` gate——但下游 crate（vault）测试编译时，sync crate 是非 test 模式，cfg(test) 不生效，thread_local 检查被跳过，测试读到真实 `~/.octopus/.sync/`。修复：改为 `#[doc(hidden)] pub`（始终编译，与 infra `set_test_db` 同模式）。

2. **v46 迁移段位置 bug**：初版把 v46 迁移段放在 `v >= 17 && v <= 43` 完整迁移分支**之前**，导致 v17-v43 老库被 v46 段（`if v >= 17`）拦截，跳过 vault UUID 迁移 + seed 加载。修复：v46 段移到 `v >= 17 && v <= 43` 分支**之后**（fall-through 点）。

3. **vault sync_md5 ALTER 容错**：v46 段的 vault_ciphers/folders sync_md5 兜底 ALTER，在纯热词测试库（无 vault 表）会失败。修复：ALTER 前先检查表存在（`pragma_table_info` 返回行数）。

4. **T12 遗留 3 问题**（干净 tree 上 infra 测试编译就失败）：
   - `VaultCipherInput` fixture 缺 `sync_md5` 字段（E0063）
   - 6 处 `assert_eq!(v, 44)` 应随版本升级更新（T12 升 v45 时漏改，又升 v46 时再漏——现统一改 46）
   - `agent_id` unused warning

5. **热词「通用」默认集用固定 UUID**：`00000000-0000-0000-0000-000000000001`——跨设备一致，两台机器的「通用」集 sync 时 id 相同不冲突。迁移时旧「通用」集（INTEGER id）也映射到此固定 UUID。

6. **热词 md5 在 desktop 命令层算（不在 infra）**：保持 infra 无 md5 依赖（与 vault cipher sync_md5 在 vault crate 算同一思路）。desktop `refill_sync_md5` helper 读完整 row 算 md5 再 update——因为 words_text 在 DB 内 normalize（拼音首字母排序 + 去重），命令层传入的原始词序与 DB 存的不同。

7. **热词 sync 失败不阻断 vault**：sync_now 的热词 pull/push 用 `match { Ok(n) => n, Err(e) => { log::warn; 0 } }`——热词同步出错只记日志，不让整个 sync_now 失败（vault 数据更关键）。

8. **security_stamp 守卫缺失致主密码失效**（2026-07-22 用户实测触发）：`pull_from_files` 无条件用 `.sync/vault/meta.json` 覆盖 DB 的 vault_meta，没有 security_stamp 校验。开发期间 dummy meta.json（stamp-1）经 sync 覆盖了真实 vault_meta（真实 UUID stamp），导致主密码验证失败。**根因**：spec INV-S9 写了不变量但代码没实现。修复：pull_from_files 读 meta.json 前对比 stamp，不一致返 `MasterPasswordMismatch` 拒绝覆盖。从 git 历史恢复了真实 vault_meta。教训：**spec 写的不变量必须有测试守护 + 代码验证，不能只写不实现**。

9. **Git 同步 UI 从 vault 挪到系统设置**（2026-07-22）：SyncPanel 从 VaultPanel PillTabs（受 vault 三态机保护，需解锁）挪到 GeneralPanel 的第 4 个子 Tab（一般/快捷键/语音/Git 同步）。原因：git 操作不碰密文（enable_sync/clone/sync_now 只管文件系统 + git），热词同步更不需要解锁——不应强制用户先输主密码才能管同步。

10. **修改主密码入口**（2026-07-22）：后端 `vault_change_password` 已实现但前端无入口。在 VaultPanel 顶部栏加 KeyRound 按钮 + ChangePasswordModal 弹窗（旧/新/确认 + 强度条）。改密码只重写 vault_meta 的 3 个包装密文 + 刷新 stamp，user_vault_key 不变 → 现有 cipher 密文不受影响。

### 测试基线

最终基线（Task 13 + 后续修复后）：

- sync: **96 pass**（git 14 + outline 7 + error 9 + privacy 25 + store 9 + hotword 26 + 4 ignored）
- infra: **158 pass**（含 v46 迁移测试 + hotword upsert/默认 UUID 测试）
- vault: **199 pass** + 1 ignored（含 stamp 守卫 2 测试 + 热词集成 4 测试）
- desktop: **394 pass**（含 hotword_commands 7 测试）
- 前端: tsc + vite build 0 error
- cargo build: 0 error 0 warning

历史基线演进：T12 完成 vault 257 → Task 13 抽离后 vault 193 + sync 70 → hotword +17 → 集成 +4 → sync 91 → 补强测试 sync 96 + vault 199 + desktop 394。

### e2e 验证（2026-07-22 通过）

- A 机创建热词版本 → sync → B 机 clone → 看到热词集 + words_text 一致 ✅
- A 改 name + B 加词 → 双向 sync → 两边都有最新 ✅
- A 删热词集 → sync → B 也删了 ✅
- 真实 GitHub private repo + SSH key 端到端 ✅
- stamp 冲突解决（以远程为准 / 以本地为准）双向 e2e ✅
