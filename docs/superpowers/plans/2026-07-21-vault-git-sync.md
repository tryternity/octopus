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

## Task 1: cipher/folder id 改 UUID 字符串（v38→v39 前置改造）

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
```

T1 必须先做（其他都依赖 UUID 字符串 id）。T2/T3 可并行。T4 依赖 T2+T3。T5 依赖 T4。

## 预估工程量

| Task | 预估行数 | 预估时间 |
|---|---|---|
| T1 UUID 改造 | 300 行（含迁移）| 1-2 小时 |
| T2 文件存储 | 400 行 | 2-3 小时 |
| T3 git wrapper | 300 行 | 1-2 小时 |
| T4 同步引擎 | 400 行 | 3-4 小时 |
| T5 UI | 300 行 | 2-3 小时 |
| T6 文档测试 | 300 行 | 1-2 小时 |
| **总计** | **~2000 行** | **10-16 小时** |

（spec 估的 1300 行偏少——加上 schema 迁移 + 测试 + UI 实际接近 2000 行）

## 风险提示

- **T1 风险最高**：cipher id 类型从 i64 改 String 涉及面广（infra + vault + desktop + 前端），漏改一处编译失败。建议改完后跑全量测试
- **T4 同步引擎**复杂度最高：文件系统 ↔ SQLite 双向同步要处理多种边界（新增/修改/删除/冲突）。建议先写集成测试覆盖典型场景
- **T5 e2e 测试**需要真实的 GitHub repo + SSH key——CI 环境难做，主要靠手动测试

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

- vault: **200 pass**（166 base + 34 sync：error 6 + outline 6 + store 9 + git 10 + engine 3）
- desktop: **381 pass**
- 前端: **304 pass**
- cargo build + tsc: 0 error 0 warning
