# 密码箱 Git 同步设计（Vault Git Sync）

> **日期**：2026-07-21
> **分支**：`research_password_vault`
> **状态**：Phase 1 已实现（T1-T10 完成，含 §4.8 私有库检测守卫 + §4.9 HTTPS→SSH 自动改写 + §4.10 非交互 prompt 防护 + §4.11 空远程仓库首次推送；待 e2e 测试）
> **前置依赖**：[2026-07-18-password-vault-design.md](./2026-07-18-password-vault-design.md) 已落地
> **目标读者**：后续实施者（plan/实现/review）
>
> **调研依据**：本会话多轮 brainstorming + 三个并行 Explore agent 调研（vault 加密层 / GitHub API / Bitwarden 同步协议）。决策见 §1.4 路线对比。

---

## 0. 目标与范围

### 0.1 核心目标

为 octopus 密码箱增加**多设备同步**能力——用户在 A 设备改密码，B 设备能拉到最新。同步后端用 **git repo**（GitHub/Gitee private repo），octopus 完全不接触凭证（SSH key + macOS keychain 由 git/ssh 全自动处理）。

主要场景：
1. 用户在 GitHub/Gitee 建 private repo `vault`
2. 配 SSH key（开发者几乎必然已配）
3. octopus 在 `~/.octopus/.vault/` 初始化 git repo + remote add
4. 改密码后点「同步」按钮 → 自动 pull + commit + push
5. 换电脑 → 安装 octopus → 输主密码 → 配 remote → 同步 → 全部 cipher 出现

### 0.2 Phase 1 范围

| 包含 | 不包含 |
|---|---|
| 直接 git repo 同步（shell out 系统 git） | GitHub Contents API（已评估，方案 B 胜出） |
| GitHub + Gitee 双 remote 支持（用户自配） | PAT 认证（Phase 2，先用 SSH key） |
| 每 cipher 单独加密文件 + 256 桶分片 | 单文件整体加密（增量同步差，已弃） |
| outline.json 增量索引（sha256 去重） | git2-rs 嵌入式（Send/Sync 问题） |
| 手动同步按钮 | 自动同步（Phase 2） |
| cipher id 改 UUID 字符串（前置改造） | Bitwarden 协议兼容（已弃，加密格式不通） |
| macOS / Linux 支持 | Windows 测试（shell out git 跨平台一致） |
| 远程已存在 repo 时 clone + 解锁即用 | 远程 repo 自动创建（用户手动建）|
| **私有库检测守卫**（§4.8，add_remote/clone 拒绝公有库） | PAT 认证后的精确私有库识别（Phase 2 加 PAT 才能区分 404 歧义）|

### 0.3 非目标

- **不做服务端**（用 GitHub/Gitee 现成 git 服务）
- **不做团队/多用户协作**（单用户多设备）
- **不做 Bitwarden 协议兼容**（评估过，加密格式不兼容，工程量 3000-5000 行）
- **不做实时推送**（手动触发 Phase 1，Phase 2 才加自动）
- **不做附件同步**（vault MVP 无附件功能）
- **不做密码历史远程同步**（password_history 字段已含本地历史，足够）

---

## 1. 路线评估与决策

### 1.1 候选路线

经过多轮 brainstorming + 3 个并行 Explore agent 调研，评估了 3 条同步路线：

| 路线 | 工程量 | 维护成本 | 决策 |
|---|---|---|---|
| **A. Bitwarden 协议**（vaultwarden 服务端） | 3000-5000 行（大重构） | 高（被协议演进绑架） | ❌ 弃 |
| **B. GitHub/Gitee git repo**（shell out 系统 git） | 1300 行（小增量） | 低（无服务端运维） | ✅ 选 |
| **C. 自研 server + REST** | 800-1500 行（中等） | 中（要部署服务端） | ❌ 弃 |

### 1.2 路线 A（Bitwarden）弃用理由

- **加密格式不兼容**：octopus 用 AES-256-GCM `v1:` 前缀；Bitwarden 用 AES-256-CBC + HMAC-SHA256 `<encType>.<iv>|<ct>|<mac>` 格式
- **KDF salt 不兼容**：octopus 32B 随机；Bitwarden 用邮箱小写
- **cipher 主键不兼容**：octopus i64 自增；Bitwarden UUID 字符串
- → 必须重写加密层 + 数据迁移，**不能增量**

### 1.3 路线 B（git repo）选定理由

- **加密层已就绪**：octopus vault 的 `app_key_sync_enc` 字段（注释明确「跨机器同步用」）+ `protected_user_vault_key` 都是用 `master_root_key` 加密的，新机器输同一主密码即可解开
- **git 完美适配小文件**：vault < 1MB 远低于任何限制
- **客户端已加密**：服务端看到的全是密文（zero-knowledge 模型）
- **零运维**：用户用自己的 GitHub/Gitee 账号
- **天然支持 GitHub + Gitee**：用户配多个 remote（`git push origin main && git push gitee main`）
- **天然增量同步**：git pack protocol 只传变化的 blob
- **天然版本历史**：`git log` 可回溯误删
- **天然冲突处理**：UUID 文件名隔离 + git merge

### 1.4 方案演进（Contents API → 直接 git）

最初考虑用 GitHub Contents API（PUT/GET 单文件）+ outline.json 索引。但用户提议「直接在 `~/.octopus/.vault/` 建 git repo」后，发现这条路线更优：

| 维度 | Contents API | 直接 git repo |
|---|---|---|
| 冲突处理 | 自己实现 SHA 乐观锁 + 409 重试 | git merge --ff-only / rebase |
| 增量同步 | outline.json + 每 cipher 单文件 | git pack protocol（更高效）|
| 版本历史 | 自己维护 | git log（免费）|
| GitHub + Gitee | 写两个 trait impl | 配多个 remote（一行 git 命令）|
| 认证 | PAT（octopus 自己存）| SSH key（git/ssh 自动，octopus 不接触）|
| 工程量 | 1800 行 | 1300 行 |

**直接 git repo 全面胜出**——选这条路。

### 1.5 SSH vs PAT（认证方式）

Phase 1 选 **SSH key（系统已配）**：

| 维度 | SSH | PAT |
|---|---|---|
| 用户群体匹配 | 开发者基本配过 SSH key | 需专门生成 token |
| octopus 接触凭证 | **完全不接触**（git/ssh/keychain 全自动）| 自己存 DB/keychain |
| 过期管理 | 不过期 | 默认 30-90 天过期 |
| 实现复杂度 | remote URL 输入框 + git 命令 | PAT 输入 + 存储 + URL 构造 + 过期检测 |

**PAT 留 Phase 2**——如果用户反馈 SSH 配置麻烦再补。

### 1.6 分片方案（256 桶）

每 cipher 单独存一个加密文件。按 uuid 前 2 个 hex 字符分桶：

```
ciphers/
├── a1/                         ← 256 个桶（uuid 前 2 hex）
│   ├── <full-uuid1>.json
│   └── <full-uuid2>.json
├── b2/
└── ...
```

**为什么单级 256 桶**：
- git 自己的 `.git/objects/` 也是单级 256 桶（前 2 hex），自证可行
- 10000 cipher 平均 40/桶，git ls-tree 毫秒级
- 调试直观（桶名 = uuid 前缀）
- 极端场景（> 50000 cipher）才加二级桶 + rehash（向后兼容）

---

## 2. 加密与密钥（复用现有，零改动）

### 2.1 密钥层级（与 [vault spec §2.1](./2026-07-18-password-vault-design.md) 一致）

```
master_password (用户输入)
    │
    │  Argon2id(t=3, m=64MiB, p=4, salt=32B)
    ▼
master_root_key (32B)
    │
    ├──► user_vault_key (32B)  ← 加密所有 cipher
    └──► app_key (32B)         ← 加密 API key
```

**关键属性**：只要 master_password + KDF 参数一致，三把 key 在任意机器都能确定性派生。

### 2.2 同步用密文（已在 vault_meta 表，零改动）

```sql
-- crates/infra/src/db.sql vault_meta 表已有字段
protected_user_vault_key  TEXT NOT NULL,  -- master_root_key 加密 user_vault_key
app_key_sync_enc          TEXT NOT NULL,  -- master_root_key 加密 app_key（同步用）
kdf_salt                  BLOB NOT NULL,  -- 32B 随机 salt
kdf_iterations            INTEGER NOT NULL,
kdf_memory_kib            INTEGER NOT NULL,
kdf_parallelism           INTEGER NOT NULL,
security_stamp            TEXT NOT NULL,
```

**新机器同步后恢复流程**：
1. octopus 拉到 `meta.json`（含上述字段）
2. 用户输 master_password
3. 用 KDF 参数派生 master_root_key
4. 用 master_root_key 解 `protected_user_vault_key` → user_vault_key
5. 用 user_vault_key 解所有 cipher 密文

### 2.3 不参与同步的数据

| 数据 | 不同步原因 |
|---|---|
| `app_key_local_enc`（K_machine 加密） | K_machine 本机随机，换机失效 |
| K_machine 本身 | 本机 OS Keychain，每机器独立 |
| `models.secret_key` 加密的 API key | 通过 `app_key_sync_enc` 同步 app_key 后本地解密 |

---

## 3. 数据模型变更

### 3.1 cipher id 从 i64 改 UUID 字符串（前置改造，T1）

**当前**：
```sql
-- crates/infra/src/db.sql
CREATE TABLE vault_ciphers (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    ...
);
```

**改后**：
```sql
CREATE TABLE vault_ciphers (
    id TEXT PRIMARY KEY,              -- UUID v4 字符串（不再自增）
    ...
);
```

**变更影响**：
- `vault_folders` 的 FK `folder_id → vault_ciphers.id` 跟着改类型（folder 也要 UUID 化——见 §3.2）
- 所有 `i64` 类型的 cipher_id 参数改 `String`
- 所有 `load_cipher(id)` / `vault_autotype(cipher_id)` / `vault_copy_password(cipher_id)` 等 Tauri 命令签名改 String
- 前端 CipherDto.id 类型 `number` → `string`
- DB 迁移：现有 cipher 生成 UUID（一次性 v37 schema 升级）

**为什么必须改**：
- 同步场景下两台机器的 SQLite AUTOINCREMENT 必然冲突（A 机生成 id=1，B 机也生成 id=1）
- UUID 全局唯一，无冲突

### 3.2 folder 也改 UUID（保持 FK 一致）

```sql
CREATE TABLE vault_folders (
    id TEXT PRIMARY KEY,              -- UUID 字符串
    name TEXT NOT NULL,               -- 加密
    ...
);
```

### 3.3 schema 版本升级 v43 → v44

> **修正**：spec 原写 v38→v39，但实施时 user_version 已经到 v43（main 上其他功能推进），实际是 v43→v44。详见 [plan §关键决策变化](../plans/2026-07-21-vault-git-sync.md#关键决策变化)。

v44 迁移：
1. 新建 `vault_ciphers_new` 和 `vault_folders_new`（id TEXT PRIMARY KEY）
2. 旧表数据 → 新表（为每行生成 UUID，写一份 `old_id_to_uuid` 映射暂存）
3. 修复 `vault_ciphers.folder_id` 引用（按映射翻译）
4. DROP 旧表，RENAME 新表
5. `PRAGMA user_version = 44`

### 3.4 vault 文件存储格式（新增模块，T2）

`~/.octopus/.vault/` 是 git repo，结构：

```
~/.octopus/.vault/
├── .git/                               ← git 元数据
├── meta.json                           ← vault_meta 同步字段
├── outline.json                        ← 增量索引
└── ciphers/
    ├── a1/                             ← uuid 前 2 hex 分桶
    │   ├── <full-uuid1>.json
    │   └── <full-uuid2>.json
    └── b2/
```

#### 3.4.1 meta.json

```json
{
  "version": 1,
  "kdf_type": 0,
  "kdf_salt": "<base64 32B>",
  "kdf_iterations": 3,
  "kdf_memory_kib": 65536,
  "kdf_parallelism": 4,
  "protected_user_vault_key": "v1:<base64 encrypted>",
  "app_key_sync_enc": "v1:<base64 encrypted>",
  "security_stamp": "<uuid>",
  "equivalent_domains": "[]"
}
```

**与 SQLite vault_meta 表字段一一对应**——同步时直接镜像。

#### 3.4.2 ciphers/<uuid>.json（单 cipher 加密文件）

```json
{
  "version": 1,
  "id": "a1b2c3d4-...",
  "encrypted": {
    "name": "v1:<base64 encrypted>",
    "notes": "v1:<base64 encrypted>",
    "data": "v1:<base64 encrypted>",
    "fields": "v1:<base64 encrypted>",
    "password_history": "v1:<base64 encrypted>"
  },
  "plaintext_meta": {
    "folder_id": "<uuid> 或 null",
    "favorite": false,
    "atype": 1,
    "reprompt": 0,
    "deleted_at": null,
    "created_at": "2026-07-21T...",
    "updated_at": "2026-07-21T..."
  }
}
```

**为什么 encrypted/plaintext_meta 分开**：
- encrypted 字段全部是 user_vault_key 加密的密文（与 SQLite 中格式完全一致）
- plaintext_meta 是非敏感元数据（明文存储节省同步时的加解密开销）
- 远程看到 plaintext_meta 时只能知道：cipher 类型（Login）/ 是否收藏 / 创建更新时间 / 是否软删——这些 git history 本来就暴露，可接受

#### 3.4.3 outline.json（增量索引）

```json
{
  "version": 1,
  "vault_version": 42,
  "ciphers": {
    "a1b2c3d4-...": {
      "sha": "<git blob sha1>",
      "updated_at": "2026-07-21T..."
    },
    "e5f6g7h8-...": {
      "sha": "<git blob sha1>",
      "updated_at": "2026-07-21T..."
    }
  },
  "folders": {
    "i9j0k1l2-...": {
      "sha": "<git blob sha1>",
      "updated_at": "2026-07-21T..."
    }
  }
}
```

**作用**：客户端拉 sync 时，先 GET outline.json，对比本地 outline，按 sha 差异决定哪些 cipher 文件需要下载——**避免 git fetch 全部历史**（虽然 git pack 已增量，但 outline 让客户端能精确控制同步粒度）。

**vault_version**：monotonic 递增整数，每次本地改动 +1。用于检测「远程版本比本地旧」（防止 push 旧数据覆盖）。

#### 3.4.4 folders/<uuid>.json（folder 加密文件）

```json
{
  "version": 1,
  "id": "i9j0k1l2-...",
  "encrypted": {
    "name": "v1:<base64 encrypted>"
  },
  "plaintext_meta": {
    "created_at": "...",
    "updated_at": "..."
  }
}
```

---

## 4. 同步引擎

### 4.1 同步触发

**Phase 1：手动触发**——用户在设置页点「同步」按钮。

触发后的流程（`vault_sync_now` 命令）：

```
1. 检查 ~/.octopus/.vault/ 是否已初始化（git repo + remote 配置）
2. 检查 git --version（无 git 则报错）
3. 加锁（防并发同步——见 §4.5）
4. 执行 sync_flow()：
   a. git fetch --all
   b. 检查远程是否有 meta.json
      - 无（首次同步到空 repo）→ 走 push_initial 流程（§4.3）
      - 有 → 走 pull_merge_push 流程（§4.2）
5. 解锁
```

### 4.2 pull_merge_push 流程（常规同步）

```
1. cd ~/.octopus/.vault
2. git fetch --all --prune             ← 拉所有 remote 的最新 refs
3. git checkout main（如在别的分支，记录后切回）
4. git merge --ff-only origin/main
   - 成功（fast-forward）→ 远程有新内容，本地已更新到最新
   - 失败（本地有 commit 但远程也有新 commit）→ 走 rebase 路径（§4.4）
5. 把文件系统状态读回 SQLite：
   - 读 meta.json → upsert vault_meta
   - 读 outline.json → 拿 {uuid: sha} 映射
   - 对比本地 outline，找出新增/修改/删除的 cipher
   - 对每个变化：读 ciphers/<桶>/<uuid>.json → upsert/delete SQLite
   - 更新本地 outline.json
6. 把 SQLite 本地变化写回文件系统：
   - 对每个 SQLite 中变化但文件系统未变化的 cipher：写文件
   - 删除 SQLite 已删但文件系统还在的 cipher 文件
   - 更新 outline.json（含 vault_version++）
7. git add -A
8. git commit -m "sync"               ← 无变化时 git 会报 nothing to commit，跳过
9. git push origin main
10. 如果配了 gitee：git push gitee main
```

### 4.3 push_initial 流程（首次推送）

用户在 A 机首次启用同步：
```
1. git init
2. 写 meta.json + outline.json + ciphers/*（从 SQLite 导出全部）
3. git add -A
4. git commit -m "init vault"
5. git remote add origin <user-provided-url>
6. git push -u origin main
```

用户在 B 机首次同步（远程已有数据）：
```
1. cd ~/.octopus
2. git clone <remote-url> .vault
3. 解锁 vault（用户输主密码）→ 派生 user_vault_key
4. 读 meta.json → upsert vault_meta
5. 读 outline.json + 所有 ciphers/* → upsert SQLite
```

### 4.4 rebase 兜底（极少发生）

`git merge --ff-only` 失败说明本地和远程都有新 commit。由于 cipher 文件名是 UUID（全局唯一），**实际冲突几乎不可能**——只有 outline.json 可能冲突。

处理：
```
1. git rebase origin/main
2. 如果 outline.json 冲突：
   - 读冲突双方的 outline
   - 按 uuid 取最新 sha 合并（merge logic 见 §4.6）
   - git add outline.json && git rebase --continue
3. 如果其他文件冲突（理论不可能）：
   - 报错让用户手动介入（打开终端）
```

### 4.5 并发同步锁

`SyncState`（进程内 `Arc<Mutex<()>>`）：
- 同步进行中再次触发同步 → 立即返 Err("同步正在进行中")
- 防止用户连点同步按钮 / Tauri 命令并发

**不做跨进程锁**——单实例 app 已有 `tauri_plugin_single_instance` 保证只有一个 octopus 进程。

### 4.6 outline.json merge 算法

```rust
fn merge_outlines(local: Outline, remote: Outline) -> Outline {
    let mut merged = local.clone();
    for (uuid, remote_entry) in remote.ciphers {
        match merged.ciphers.get(uuid) {
            None => {
                // 本地无，远程有 → 新增
                merged.ciphers.insert(uuid.clone(), remote_entry);
            }
            Some(local_entry) => {
                // 双方都有 → 取 updated_at 更新的
                if remote_entry.updated_at > local_entry.updated_at {
                    merged.ciphers.insert(uuid.clone(), remote_entry);
                }
            }
        }
    }
    // vault_version 取 max
    merged.vault_version = local.vault_version.max(remote.vault_version);
    merged
}
```

**删除传播**：git 本身记录删除（`git rm` 后 commit）。pull 时如果远程删了某 cipher 文件，本地 checkout 后文件就没了——SQLite 同步时检测到 outline 有 uuid 但文件不存在 → 软删 SQLite 行（设 deleted_at）。

### 4.7 失败场景与降级

| 场景 | 失败处理 |
|---|---|
| 无 git 命令 | `git --version` 失败 → 同步功能禁用，UI 提示「请安装 git」|
| 无网络 | `git fetch` 失败 → 报错「网络不可达」，不阻塞本地操作 |
| SSH host key 未验证 | `git push` 失败 → 提示「请在终端跑 ssh -T git@github.com」|
| SSH key 未配置 | `git push` 失败（permission denied）→ 提示「请配置 SSH key」|
| 远程仓库不存在 | `git push` 失败 → 提示「请确认 remote URL 正确」|
| 远程仓库非空非 vault | `git pull` 拉到无关历史 → rebase 失败 → 提示用户手动介入 |
| outline.json 损坏 | JSON parse 失败 → 重建 outline（扫所有 cipher 文件）|
| cipher 文件解密失败 | user_vault_key 不匹配 → 跳过该 cipher + 记 failures 列表（与现有 list_ciphers 一致）|
| 同步中 app 崩溃 | git repo 状态可能不干净（merge in progress）→ 下次同步前 `git merge --abort` 清理 |
| 用户中途换主密码 | security_stamp 不一致 → 拒绝同步，提示「远程 vault 用了不同主密码」|
| add_remote/clone 输入公有库 | §4.8 私有库守卫硬阻断 → `PublicRepoRejected`，提示用户改 Private 或换 URL |
| add_remote/clone 输入本地路径 | §4.8 私有库守卫 → `LocalPathRejected`，提示用户用 GitHub/Gitee URL |
| GitHub API 限流（60/h/IP） | §4.8 检测返 `Ambiguous`（不阻断）→ 用户可继续或换用 SSH |
| 私有库检测网络错误 | §4.8 检测返 `NetworkError`（不阻断）→ 用户可继续或稍后重试 |

---

### 4.8 私有库检测守卫（2026-07-21 增补）

**动机**：vault 同步用 AES-256-GCM 加密，理论上密文推到公网也安全。但密文泄露给攻击者做离线爆破仍是失败——主密码弱时 KDF（Argon2id）也挡不住算力攻击。所以 `add_remote` / `clone_from` 入口必须拦截公有库。

**策略（Phase 1：未认证 API + ls-remote 兜底）**：

| URL 类型 | 检测方法 | 判定 |
|---|---|---|
| `file://` 或本地路径 | 直接拒绝 | 暴露本地路径无意义 |
| `github.com` / `gitee.com` HTTPS | HTTP API 查 `private` 字段 | 200 + `private:false` → Public（拒绝）；其他 → Ambiguous（放行） |
| 其他 host HTTPS | `git ls-remote --heads`（带 10s 超时） | exit 0 + refs → Public（拒绝）；其他 → Ambiguous（放行） |
| SSH (`git@host:...`) | 无法匿名嗅探 | SshUnverifiable（放行 + UI 强提示） |

**关键不变量**：检测到公有必拒；歧义/私有/网络错误一律放行（不阻断用户），因为不会泄露给公众。

**GitHub/Gitee 404 歧义**：未认证查询私有库返 404（与"不存在"无法区分，是有意设计避免信息泄漏）。所以 Phase 1 只能"确认公有"，不能"确认私有"。Phase 2 加 PAT 后能区分。

**ls-remote 关键实现细节**：
- macOS 无 `timeout` 命令——代码层 `spawn` + `try_wait` 轮询 + 超时 `child.kill()`
- 必须设环境变量 `GIT_TERMINAL_PROMPT=0`——私有 HTTPS 库遇 401/404 会被 git 拦住要用户名，设 0 后立即失败而非卡死等输入
- GitHub/Gitee HTTPS 公有库 ls-remote 直接 exit 0 + 完整 refs 列表（匿名可读）

**URL 解析（`privacy::GitRemoteUrl`）**：支持 5 种格式——
- `https://github.com/owner/repo.git`
- `https://user:token@github.com/owner/repo.git`（去 userinfo）
- `git@github.com:owner/repo.git`（scp-like，正则）
- `ssh://git@github.com/owner/repo.git`
- `file://` / `/abs/path` / `./rel/path` / `~/path`（→ File）

owner/repo 从 URL path 最后两段提取，去 `.git` 后缀 + trailing `/`。

**新 SyncError 变体**：
- `PublicRepoRejected(url)` → "拒绝添加公有库 {url}——密码箱必须使用私有库..."
- `LocalPathRejected` → "本地路径不能作为同步 remote..."

**UI 提示**：添加 remote 表单 + clone 表单下方常驻私有库检测说明（`privacyHint`），busy 状态文案 `checkingPrivacy`。检测失败时直接展示 `SyncError.to_string()`（已有路径，无新逻辑）。

**实测验证**（spec 编写时实测）：
- `https://github.com/octocat/Hello-World.git` → API 200 + `private:false` → Public，被拒
- `https://github.com/octocat/nonexistent-xyz.git` → API 404 → Ambiguous，放行
- `git@github.com:owner/repo.git` → SshUnverifiable，放行 + UI 提示
- GitHub API 未认证限流：60/h/IP（用户加 remote 频率低，足够）

---

### 4.9 HTTPS → SSH 自动改写（2026-07-21 增补）

**动机**：用户从浏览器复制的 GitHub/Gitee URL 默认是 HTTPS，但 GitHub 自 2021-08
起禁用 HTTPS 密码认证仅支持 PAT。用户已踩坑：`Password authentication is not
supported for Git operations`。用户机器通常已配 SSH key（开发者几乎必然），
所以 octopus 自动把 HTTPS URL 改写成 SSH URL，让 `~/.ssh/` 私钥接管认证。

**范围**：仅 `github.com` / `gitee.com`（主流平台双协议必可用，SSH 端口 22 必开放）。
自建 GitLab/Gitea / GitHub Enterprise 不在列（SSH 端口可能被封 / 改端口）。

**流程**（add_remote / clone_initial 入口，私有库守卫之后）：

```
HTTPS URL → try_convert_https_to_ssh()
  ├─ 非 github/gitee / 已是 SSH / 本地路径 → 返原 URL（不改写）
  └─ github/gitee HTTPS URL → 生成 SSH URL → verify_ssh_key_for_host(host)
       ├─ SSH key 可用 → 返 SSH URL（实际 add/clone 用 SSH）
       └─ SSH key 不可用 / ssh 命令失败 → 返原 HTTPS URL（保留，后续 push 错误由 toast 暴露）
```

**URL 转换规则**：

| HTTPS URL | SSH URL |
|---|---|
| `https://github.com/owner/repo` | `git@github.com:owner/repo.git` |
| `https://github.com/owner/repo.git` | `git@github.com:owner/repo.git` |
| `https://user:token@github.com/owner/repo` | `git@github.com:owner/repo.git`（丢 userinfo）|
| `https://gitee.com/owner/repo` | `git@gitee.com:owner/repo.git` |

**SSH key 预检**：`ssh -T -o ConnectTimeout=5 -o StrictHostKeyChecking=accept-new -o BatchMode=yes git@<host>`
- GitHub 返 exit 1 + stderr `Hi <user>! You've successfully authenticated...`（不允许 shell 故 exit 非 0）
- Gitee 返 exit 0 + 类似问候
- 失败返 exit 255 + `Permission denied (publickey)` / `Could not resolve hostname`

关键 SSH 选项：
- `-T`：禁用 TTY 分配（避免阻塞）
- `BatchMode=yes`：永不交互（避免卡密码 prompt）
- `StrictHostKeyChecking=accept-new`：首次连接自动接受 host key（省去用户先手动 `ssh -T` 一次）

**存储形式**：转 SSH 后直接存（`.git/config` 里是 SSH URL，Remote 列表显示也是 SSH）。
用户在 SyncPanel 看到的 URL 是 SSH 形式——与实际生效协议一致，避免认知混淆。
如果用户输 HTTPS 但看到 SSH，是预期的（自动转换的副作用）。

**失败不阻断**：SSH key 验证失败时不阻断用户——保留 HTTPS URL，让后续 push 失败的
错误（如 `Password authentication is not supported`）通过 toast 暴露，用户能据此
判断是配 SSH key 还是改 URL。

**新增 API**：
- `privacy::try_convert_https_to_ssh(url) -> Option<String>`：纯函数 URL 转换
- `git::verify_ssh_key_for_host(host) -> Result<bool, SyncError>`：shell out `ssh -T`
- `git::git_remote_set_url(path, name, url)`：shell out `git remote set-url`
- `engine::maybe_rewrite_to_ssh(url) -> Result<String, SyncError>`：组合转换 + 预检
- `engine::ensure_remotes_use_ssh_when_possible(root)`：sync_now 入口对已有 remote 兜底改写

**sync_now 兜底（避免盲区）**：`add_remote` 时改写只对**新加的** remote 生效——
但用户可能在自动改写功能加上**之前**就 `git remote add` 了 HTTPS URL（`.git/config`
里写死 HTTPS），或 SSH key 是后装的。sync_now 入口在 `cleanup_in_progress_ops`
之后、`fetch` 之前调 `ensure_remotes_use_ssh_when_possible`：遍历所有 remote，
对 github/gitee HTTPS URL 且本机 SSH key 可用的 → `git remote set-url` 改 SSH，
单个 remote 改写失败不影响其他 remote（只记日志）。

---

### 4.10 非交互 prompt 防护（2026-07-21 增补）

**动机**：octopus 在 Tauri 后端进程里跑 git，**stdin 脱离终端**——任何交互式
凭据 prompt（用户名/密码）都会让进程卡死，UI 上看到的是"无限转圈"，用户根本
无法输入。用户已踩坑：HTTPS GitHub 库 push 时 git 输出 `Username for 'https://github.com':`
然后无限等待。

**不变量（INV-S14）**：所有 git 命令都必须**非交互**——禁用 prompt + stdin /dev/null。

**实现**：`git_command(args)` helper 统一构造 Command，设 3 层防御：
1. `GIT_TERMINAL_PROMPT=0`——git 遇 HTTPS 凭据需求立即失败（不读 stdin）
2. `GIT_ASKPASS=` + `SSH_ASKPASS=`——禁用外部 askpass 程序（macOS Keychain 等）
3. stdin → `/dev/null`——双保险，即使前两个被忽略 git 也立即读到 EOF 失败

**统一接入**：`run_git` / `run_git_allow_codes` / `git_ls_remote` /
`git_ls_remote_with_timeout` 全部用 `git_command` 构造，**所有** git 命令都默认非交互。

**错误分类**：禁用 prompt 后 git 失败的 stderr 含特定字符串，`classify_git_error`
识别后归 `SyncError::CredentialsRequired`，前端 toast 显示"远程仓库需要认证但无法
交互输入——请配 SSH key（推荐）或使用 SSH/PAT URL"，引导用户改用 SSH key。
识别关键字：
- `terminal prompts disabled`
- `could not read username/password`
- `authentication failed`
- `password authentication is not supported`
- `invalid username or token`

**为什么不实现 UI 凭据输入**：
- 安全：octopus 设计原则是不接触凭证（spec §1.5），让 git/ssh/keychain 全自动
- 工程量：UI 凭据输入要处理密码存储 / Keychain 集成 / 多 remote 凭据管理，复杂度高
- 替代方案更优：SSH key 一次配置永久有效，PAT 也可用，HTTPS 用户名密码体验最差

---

### 4.11 空远程仓库首次推送（2026-07-21 增补）

**动机**：用户在 GitHub/Gitee 新建仓库时通常**不勾选**「Initialize with README」
——这是个空仓库，没有任何分支。用户首次点同步时，sync_now 流程的
`git merge --ff-only origin/main` 报 `merge: origin/main - not something we can merge`
（origin/main 不存在）。必须识别这种场景并跳过 merge/rebase，直接走首次 push。

**状态机**（`MergeFfResult` enum）：

| 状态 | 触发条件 | sync_now 行为 |
|---|---|---|
| `FastForwarded` | `git merge --ff-only` 成功 | 远程领先本地，已合并 → 继续 pull/push/commit |
| `CannotFastForward` | merge 失败 + stderr 不含 "upstream"/"merge" 关键字 | 本地远程分叉 → rebase 兜底 |
| `NoUpstream` | merge 失败 + stderr 含 `not something we can merge` / `invalid upstream` / `not a valid ref` / `unknown revision` | 远程空仓库 → **跳过 merge/rebase**，直接 commit + `git push -u origin main` |

**NoUpstream 关键字判断**：源自 git 实测 stderr——
- `merge: origin/main - not something we can merge`（macOS git 2.x 主流版本）
- 兼容 `invalid upstream` / `not a valid ref` / `unknown revision`（不同 git 版本/翻译变体）

**首次 push 用 `-u`**：`git push -u origin main` 同时设 upstream——后续 push 不需要 -u。
后续 sync_now 走 `FastForwarded` 或 `CannotFastForward` 正常路径。

**API 变化**：
- `git::git_merge_ff` 返回类型从 `Result<bool, _>` 改为 `Result<MergeFfResult, _>`
  （bool 只能区分 ff 成功 vs 不能 ff，无法表达 NoUpstream）
- `engine::sync_now` 按 `MergeFfResult` 分流，记录 `is_first_push` 标志位决定 push 用 -u 还是普通 push

**为什么不用预检测（fetch 前 ls-remote）**：让 git 自然报错后再判断更简单——
预检测要额外网络往返，且 fetch 之后 origin/main ref 已经在本地（如果存在），
直接试 merge 是最自然的判断方式。

---

## 5. 不变量

| # | 不变量 | 说明 |
|---|---|---|
| INV-S1 | cipher.id 必须是 UUID v4 字符串 | 跨设备无冲突 |
| INV-S2 | cipher 文件名必须等于 cipher.id | 文件名 ↔ 内容一一对应 |
| INV-S3 | cipher 文件路径必须按 uuid 前 2 hex 分桶 | `ciphers/<hex[0..2]>/<uuid>.json` |
| INV-S4 | cipher 文件 encrypted 字段必须以 `v1:` 开头（AES-256-GCM）| 与 SQLite 存储格式一致 |
| INV-S5 | meta.json 必须包含 KDF 派生所需全部参数 | salt + iterations + memory + parallelism |
| INV-S6 | 同步前后 vault_version 必须 +1（有变化时）| 防旧版本覆盖 |
| INV-S7 | git commit message 统一为 `sync` 或 `init vault` | 不暴露操作细节 |
| INV-S8 | 同步过程中必须持 SyncState 锁 | 防并发触发 |
| INV-S9 | 远程 security_stamp ≠ 本地时拒绝同步 | 防主密码不一致 |
| INV-S10 | cipher 文件加密用 user_vault_key（不是 app_key）| 与 SQLite 一致 |
| INV-S11 | add_remote / clone_from 入口必须拒绝公有库 | 见 §4.8——密文泄露给攻击者做离线爆破仍是失败 |
| INV-S12 | 本地路径（`file://` / `/abs/path`）禁止作为同步 remote | 同步意义为 0，且暴露本地文件结构 |
| INV-S13 | github.com / gitee.com HTTPS URL 在 SSH key 可用时应自动转 SSH | 见 §4.9——避免 GitHub HTTPS 密码认证已禁用的死局 |
| INV-S14 | 所有 git 命令必须非交互（禁用 prompt + stdin /dev/null） | 见 §4.10——Tauri 后端进程 stdin 脱离终端，交互 prompt 会让 UI 卡死 |
| INV-S15 | sync_now 必须识别空远程仓库（NoUpstream）并走首次 push -u | 见 §4.11——用户新建空 repo 后首次点同步不能失败 |

---

## 6. 模块结构

```
crates/vault/src/
├── sync/                           # 新增模块（T2-T7）
│   ├── mod.rs                      # 公共 API + SyncState
│   ├── git.rs                      # git 命令 wrapper（shell out）+ ls-remote 带超时
│   ├── store.rs                    # 文件存储（meta/outline/ciphers 读写）
│   ├── outline.rs                  # outline.json 数据结构 + merge 算法
│   ├── engine.rs                   # 同步引擎（pull_merge_push / push_initial / clone_initial + 私有库守卫）
│   ├── privacy.rs                  # 私有库检测（URL 解析 + GitHub/Gitee API + ls-remote 嗅探，T7）
│   └── error.rs                    # SyncError enum（含 PublicRepoRejected / LocalPathRejected）
├── storage/                        # 现有 SQLite 存储（零改动）
├── crypto/                         # 现有加密层（零改动）
└── ...

crates/desktop/src/
├── vault_sync_commands.rs          # 新增 Tauri 命令（vault_sync_now / vault_sync_status 等）
└── ...

crates/desktop/frontend/src/pages/Settings/Vault/
├── SyncPanel.tsx                   # 新增同步设置 UI（T5）
└── ...
```

---

## 7. UI 设计（T5）

### 7.1 设置页同步段

VaultPanel 顶部加一个「同步」段（feature gate: vault 启用 + git 检测到）：

```
┌─ 同步 ────────────────────────────────────┐
│ 状态：未启用                                │
│                                            │
│ [启用同步]                                 │
└────────────────────────────────────────────┘

点击「启用同步」后展开配置：

┌─ 同步 ────────────────────────────────────┐
│ 状态：✓ 已同步（上次：2026-07-21 12:34）   │
│                                            │
│ Remote URL:                                │
│ [git@github.com:user/vault.git]            │
│                                            │
│ 添加 Gitee mirror（可选）:                 │
│ [git@gitee.com:user/vault.git]            │
│                                            │
│ [测试连接] [立即同步] [禁用同步]          │
│                                            │
│ 提示：首次同步前请在终端运行              │
│       ssh -T git@github.com                │
│       验证 host key                        │
└────────────────────────────────────────────┘
```

### 7.2 同步状态机

```
未启用（disabled）→ [启用同步] → 配置中（configuring）→ [测试连接] → 
  连接成功（connected）→ [立即同步] → 同步中（syncing）→ 
    同步完成（synced）/ 同步失败（error）
```

### 7.3 错误提示

同步失败时 toast 显示具体原因（网络不可达 / SSH 失败 / 冲突需手动介入等），不阻塞 vault 其他功能。

私有库检测失败（add_remote / clone_from 入口）显示 `SyncError::to_string()`：
- **公有库拒绝** → "拒绝添加公有库 {url} 作为同步仓库——密码箱必须使用私有库..."
- **本地路径拒绝** → "本地路径不能作为同步 remote——请使用 GitHub/Gitee 私有库或自建 Git 服务的 URL"

「添加 remote」「clone」按钮 busy 时分别显示 spinner / "检测仓库可见性..."（检测可能 1-3s）。两处表单下方常驻 `privacyHint` 文案，让用户预先知道检测策略。

---

## 8. 实施分阶段（Phase 1 任务）

### T1: cipher/folder id 改 UUID（前置改造）

- `crates/infra/src/db.sql`: vault_ciphers / vault_folders 的 id 改 TEXT PRIMARY KEY
- `crates/infra/src/db.rs`: 所有 i64 类型的 cipher_id / folder_id 改 String
- `crates/vault/src/types.rs`: Cipher.id / CipherInput / CipherDto 改 String
- `crates/vault/src/storage/`: 所有 CRUD 函数签名更新
- `crates/desktop/src/vault_commands.rs`: 所有 Tauri 命令签名更新
- 前端 CipherDto.id 类型 `number` → `string`
- v43 → v44 schema 迁移：旧数据生成 UUID

**验证**：cargo test -p octopus-vault -p octopus-infra -p octopus-desktop 全过

### T2: vault 文件存储模块

- 新增 `crates/vault/src/sync/store.rs`
- 实现 `read_meta_file()` / `write_meta_file()`
- 实现 `read_cipher_file(uuid)` / `write_cipher_file(cipher)`
- 实现 `read_outline_file()` / `write_outline_file()`
- 实现 `read_folder_file(uuid)` / `write_folder_file(folder)`
- **加密层复用现有**：store.rs 调 storage:: 的 encrypt/decrypt 函数

**验证**：单元测试覆盖 round-trip（写 → 读 → 比对）

### T3: git 命令 wrapper

- 新增 `crates/vault/src/sync/git.rs`
- 实现 `check_git_available() -> bool`（shell out `git --version`）
- 实现 `git_init(path)` / `git_remote_add(path, name, url)` / `git_remote_list(path)`
- 实现 `git_fetch_all(path)` / `git_merge_ff(path, ref)` / `git_rebase(path, ref)`
- 实现 `git_add_all(path)` / `git_commit(path, msg)` / `git_push(path, remote, ref)`
- 实现 `git_clone(url, path)`
- 实现 `git_status_has_changes(path) -> bool`
- 所有函数 shell out `Command::new("git")`，错误处理把 stderr 透传给调用方

**验证**：单元测试覆盖每个函数（用 tempfile::tempdir() 创建临时 repo）

### T4: 同步引擎

- 新增 `crates/vault/src/sync/engine.rs`
- 实现 `sync_now() -> Result<SyncReport>`：编排 fetch → merge → 文件系统 ↔ SQLite 双向同步 → commit → push
- 实现 `enable_sync(remote_url) -> Result<()>`：首次配置（push_initial 或 clone_initial）
- 实现 `disable_sync() -> Result<()>`：删 `~/.octopus/.vault/`（保留 SQLite）
- 实现 `sync_status() -> SyncStatus`：返回当前状态
- `SyncState` 进程内锁

**验证**：集成测试覆盖 push_initial / clone_initial / 双向同步 / 冲突 rebase

### T5: 配置 UI

- 新增 `crates/desktop/frontend/src/pages/Settings/Vault/SyncPanel.tsx`
- VaultPanel 顶部加同步段（feature gate）
- 注册 Tauri 命令：`vault_sync_now` / `vault_sync_status` / `vault_sync_enable` / `vault_sync_disable` / `vault_sync_test_connection`
- i18n：中英文翻译

**验证**：手动 e2e（A 机 push → B 机 clone → 验证 cipher 同步）

### T6: 文档 + 测试

- architecture.md vault 段补同步说明
- spec 文档同步实际实现差异
- plan 文档记录实施过程
- 补单元测试 + 集成测试

### T7: 私有库检测守卫（2026-07-21 增补，详见 §4.8）

- 新增 `crates/vault/src/sync/privacy.rs`（URL 解析 + 检测引擎 + PrivacyVerdict enum）
- `crates/vault/src/sync/git.rs` 加 `git_ls_remote_with_timeout`（spawn + try_wait 轮询 + kill 超时）
- `crates/vault/src/sync/error.rs` 加 `PublicRepoRejected` / `LocalPathRejected`
- `crates/vault/src/sync/engine.rs` `add_remote` + `clone_initial` 入口加 `ensure_private_repo` 守卫
- 前端 SyncPanel：spinner + `privacyHint` + `checkingPrivacy` 文案
- 依赖：`ureq`（同步 HTTP 客户端，查 GitHub/Gitee API）

**验证**：30 个新单元测试 + 3 个 `#[ignore]` 真实网络集成测试（GitHub/Gitee 公有库检测、不存在库 404 歧义）

### T8: HTTPS → SSH 自动改写（2026-07-21 增补，详见 §4.9）

- `crates/vault/src/sync/privacy.rs` 加 `try_convert_https_to_ssh`（纯函数，仅 github/gitee）
- `crates/vault/src/sync/git.rs` 加 `verify_ssh_key_for_host`（shell out `ssh -T -o BatchMode=yes`）
- `crates/vault/src/sync/engine.rs` 加 `maybe_rewrite_to_ssh`，在 add_remote + clone_initial 入口调用
- 转换前先 `ssh -T` 预检——SSH key 不可用则保留 HTTPS（不阻断用户）

**验证**：7 个 URL 转换单元测试 + 1 个 SSH key 验证 `#[ignore]` 集成测试 + 1 个 rewrite 不改非 github/gitee URL 测试

---

## 9. 风险与已知限制

| 风险 | 严重度 | 缓解 |
|---|---|---|
| 用户没装 git | 中 | 启动时检测，无 git 则同步禁用 + UI 提示 |
| SSH key 配置门槛 | 中 | Phase 1 文档详尽引导；Phase 2 加 PAT 备选 |
| git history 暴露时间戳 | 低 | private repo 可接受；commit msg 统一 `sync` |
| `.git/` 目录膨胀 | 低 | git gc 自动清理；cipher 密文 < 1KB |
| 远程仓库被攻破 | 低 | 客户端已加密，攻击者只看到密文 |
| 多设备并发 push 冲突 | 低 | git rebase 兜底；UUID 隔离避免真冲突 |
| 大 vault 性能（> 10000 cipher）| 低 | 256 桶分片已覆盖；超 50000 cipher 才需二级桶 |
| Windows 路径差异 | 低 | shell out git 跨平台一致；测试覆盖 |
| 同步中崩溃留下脏 git 状态 | 中 | 下次同步前 `git merge --abort` 清理 |

---

## 10. 后续 Phase 2（不在本次范围）

- **自动同步**：vault 变化后 debounce 30s 自动 push + 启动时自动 pull
- **PAT 认证**：作为 SSH 备选，覆盖更多用户
- **冲突 UI**：rebase 失败时引导用户手动解决（极少发生）
- **二级分桶**：如果用户 cipher > 50000，加二级桶 rehash
- **附件同步**：vault 未来加附件功能时
- **多 remote 并发推送**：`git push origin main && git push gitee main` 并行化

---

## 附录 A：与现有 vault 加密的一致性

vault 文件存储的加密格式**与 SQLite 中完全一致**：

| 字段 | SQLite 中 | 文件存储中 | 加密 key |
|---|---|---|---|
| cipher.name | `v1:<base64>` | `v1:<base64>` | user_vault_key |
| cipher.notes | `v1:<base64>` 或 NULL | `v1:<base64>` 或省略 | user_vault_key |
| cipher.data | `v1:<base64>` | `v1:<base64>` | user_vault_key |
| cipher.fields | `v1:<base64>` 或 NULL | `v1:<base64>` 或省略 | user_vault_key |
| cipher.password_history | `v1:<base64>` 或 NULL | `v1:<base64>` 或省略 | user_vault_key |
| folder.name | `v1:<base64>` | `v1:<base64>` | user_vault_key |

**零加密格式改动**——同步模块只是把 SQLite 行搬运到 JSON 文件，加解密层完全复用。

---

## 附录 B：git 命令清单（shell out）

| 操作 | 命令 | 用途 |
|---|---|---|
| 检测可用性 | `git --version` | 启动时检测 |
| 初始化 | `git init` | 首次启用同步 |
| 配 remote | `git remote add <name> <url>` | 加 origin / gitee |
| 列 remote | `git remote -v` | 状态显示 |
| 拉取 | `git fetch --all --prune` | 同步前 |
| 合并 | `git merge --ff-only <ref>` | fast-forward 合并 |
| 变基 | `git rebase <ref>` | 兜底冲突 |
| 暂存 | `git add -A` | commit 前 |
| 提交 | `git commit -m "sync"` | 标准同步 commit |
| 推送 | `git push <remote> main` | 推到 origin / gitee |
| 克隆 | `git clone <url> <path>` | B 机首次同步 |
| 状态 | `git status --porcelain` | 检测有无变化 |
| 清理 | `git merge --abort` / `git rebase --abort` | 崩溃恢复 |

---

## 附录 C：典型用户流程

### C.1 首次启用同步（A 机）

```
1. 用户在 GitHub 创建 private repo: github.com/user/vault（空 repo）
2. 用户本地已配 SSH key（开发者默认）
3. octopus 设置 → 密码保险库 → 同步段 → 点「启用同步」
4. 输入 Remote URL: git@github.com:user/vault.git
5. 点「测试连接」：
   - octopus 跑 git ls-remote git@github.com:user/vault.git
   - 如果 host key 未验证 → 提示「请在终端跑 ssh -T git@github.com」
   - 成功 → 显示绿色 ✓
6. 点「立即同步」：
   - octopus: git init ~/.octopus/.vault
   - 从 SQLite 导出 meta.json + outline.json + ciphers/*
   - git add -A && git commit -m "init vault"
   - git remote add origin <url>
   - git push -u origin main
7. 同步状态变「✓ 已同步」
```

### C.2 B 机首次同步

```
1. 用户在 B 机装 octopus
2. 启动 octopus → 设置 → 密码保险库
3. 输入 master_password（与 A 机一致）→ setup_vault() 初始化空 vault
4. 同步段 → 点「启用同步」
5. 输入 Remote URL（同 A 机）
6. 点「立即同步」：
   - octopus 检测到 ~/.octopus/.vault/ 不存在但远程有数据
   - git clone <url> ~/.octopus/.vault
   - 读 meta.json → upsert vault_meta
   - 读 outline.json + ciphers/* → upsert SQLite
7. B 机 vault 显示全部 cipher
```

### C.3 日常双向同步

```
A 机改密码 → 点「同步」：
  - git fetch → git merge --ff-only → 文件系统读回 SQLite
  - SQLite 改动写文件系统 → git add -A → git commit -m "sync"
  - git push origin main

B 机点「同步」：
  - git fetch → git merge --ff-only（拿到 A 的最新）
  - 文件系统读回 SQLite → B 机看到新密码
  - 无本地改动 → 不 commit / push
```

---

## 参考文档

- [vault 原始设计 spec](./2026-07-18-password-vault-design.md)——加密层、密钥层级、数据模型基础
- [vault 实施计划](../plans/2026-07-18-password-vault.md)——含方案 A bookmarklet 评估（已弃）
- Tauri deep-link plugin 文档（未采用，但评估过）
- vaultwarden 源码 `vault/src/api/core/ciphers.rs`（Bitwarden 同步协议参考）
