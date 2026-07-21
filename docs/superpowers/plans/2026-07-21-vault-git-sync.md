# Vault Git 同步实施计划（Phase 1）

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 为 octopus 密码箱增加多设备同步——用 git repo（GitHub/Gitee private repo）作为后端，SSH key 认证（系统已配），每 cipher 单独加密文件 + 256 桶分片 + outline.json 增量索引。

**Architecture:** 复用现有 vault 加密层（user_vault_key + AES-256-GCM，零改动）；新增 `crates/vault/src/sync/` 模块（git 命令 wrapper + 文件存储 + 同步引擎）；新增 `crates/desktop/src/vault_sync_commands.rs`（Tauri 命令）；新增前端 `Settings/Vault/SyncPanel.tsx`（同步配置 UI）。前置改造：cipher/folder id 从 i64 改 UUID 字符串（v38→v39 schema 迁移）。

**Tech Stack:** Rust（uuid / serde / anyhow——已用）+ shell out 系统 `git` 命令（无新依赖）+ Tauri 2 + React 19 + TypeScript + Tailwind 4。

**Spec:** [2026-07-21-vault-git-sync-design.md](../specs/2026-07-21-vault-git-sync-design.md)

> **状态**：Phase 1 实施中。

## Global Constraints

（从 spec §1-§5 摘录的硬约束，所有任务隐式遵守）

- **加密层零改动**：复用现有 `user_vault_key`（派生自 master_password），密文格式 `v1:<base64(nonce||ct||tag)>` 与 SQLite 完全一致
- **git 实现**：shell out 系统 `git` 命令（不嵌入 libgit2）；无 git 则同步功能禁用
- **认证**：Phase 1 只支持 SSH key（用户系统已配），octopus 完全不接触凭证
- **cipher id**：UUID v4 字符串（前置改造 v38→v39），不再用 i64 自增
- **分片**：`ciphers/<uuid 前 2 hex>/<full-uuid>.json`，256 桶
- **outline.json**：`{version, vault_version, ciphers: {uuid: {sha, updated_at}}, folders: {...}}`——增量同步索引
- **同步触发**：手动（Phase 1）；Phase 2 才加自动
- **冲突处理**：UUID 隔离 + `git merge --ff-only` + rebase 兜底
- **多 remote**：支持 GitHub + Gitee 双 remote（用户自配）
- **commit message**：统一 `sync` 或 `init vault`，不暴露操作细节
- **跨 crate 依赖方向**：infra ← vault ← desktop；sync 模块在 vault crate 内
- **错误返回**：Tauri 命令统一 `Result<T, String>`（与现有 vault_commands 一致）；vault crate 内部用 `anyhow::Result`
- **feature gate**：sync 模块在 vault feature 下（继承现有 vault feature gate）
- **平台范围**：macOS / Linux 优先；Windows 测试覆盖（shell out git 跨平台一致）

---

## File Structure

### 新增文件

**crates/vault/src/sync/**（新模块）

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
| `crates/infra/src/db.rs` | i64 → String 类型；v38→v39 schema 迁移逻辑 |
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
| **总计** | **~3330 行** | **13-21 小时** |

（spec 估的 1300 行偏少——加上 schema 迁移 + 测试 + UI + 私有库检测 + 同步健壮性套件实际接近 3330 行）

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

- vault: **253 pass**（含 fingerprint 7 + incremental_export 4 + outline 序列化等）
- desktop: **387 pass**
- 前端: tsc + vite build 0 error
- cargo build: 0 error 0 warning
- 真实网络集成测试（`#[ignore]`）：5 个

历史基线演进：T1-T6 完成 200 → T7 私有库检测 230 → T8 HTTPS→SSH 238 → T9 非交互 prompt 240 → T10 空远程 241 → T11 outline 稳定 247 → T12 md5 增量 253。

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

### 关键设计决策

1. **md5 不是安全 hash**：纯 diff 工具，碰撞无实际风险（cipher 数 < 10万）
2. **不含时间戳**：created_at/updated_at 跨设备必然不同，含了会导致永久 diff
3. **密文字段安全**：cipher 只在创建机器加密一次，sync 搬运密文——跨设备一致（详见 spec §2.4）
4. **不回填 md5**：避免在 infra 层引入 md5 依赖，让首次 sync 自然填上
5. **outline.sha 字段名不变**：值从「文件字节 sha」变成「逻辑内容 md5」——向后兼容（旧 outline 首次 sync 会被覆盖）

### 实施记录

T12 全部完成（2026-07-21）。vault 测试 247 → 253 pass（新增 11 个：7 fingerprint + 4 incremental_export；删除 5 个 count_outline_changes）。0 error 0 warning。

**架构演进伏笔**：fingerprint.rs + incremental_export 的设计为未来扩展 hotword/prompts 同步打基础——同一种 md5 diff 模式可复用（目前不抽象 trait，YAGNI）。
