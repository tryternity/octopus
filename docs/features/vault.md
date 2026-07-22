# 密码保险箱（Password Vault）

> 本地优先的密码管理功能。AES-256-GCM 加密存储，主密码派生密钥，不依赖 OS Keychain（macOS adhoc 签名限制）。含 Auto-Type 自动填充、URL 匹配（防钓鱼）、TOTP、密码生成器、健康检查、Bitwarden 导入导出、Git 跨设备同步。`feature = "vault"` 编译开关控制，默认启用。

源文件：`crates/vault/`（纯逻辑库）+ `crates/desktop/src/vault_commands.rs`（Tauri 命令层）+ `crates/desktop/src/autotype/`（键盘模拟）+ `crates/desktop/frontend/src/pages/Settings/Vault/`（管理 UI）+ `pages/VaultPicker/`（Auto-Type 浮窗）。

---

## 1. 模块结构

| 模块 | 职责 |
|------|------|
| `crypto` | 加密层：密钥层级（`hierarchy.rs`）、Argon2id KDF（`kdf.rs`）、AES-256-GCM 对称加解密（`symmetric.rs`） |
| `storage` | DB CRUD：cipher（`cipher.rs`）、folder（`folder.rs`）、vault_meta（`meta.rs`）；含软删/回收站 |
| `types` | 数据结构：`Cipher`/`CipherInput`/`LoginData`/`LoginUri`/`MatchType`/`RepromptType` |
| `matcher` | URL 匹配：5 种策略（Domain/Host/Exact/StartsWith/RegularExpression）+ eTLD+1（内嵌 Mozilla PSL）+ 等价域名 |
| `generator` | 密码生成器：random / passphrase-en / passphrase-zh / pin 四种模式 |
| `health` | 健康检查：密码强度（zxcvbn）、重复密码检测 |
| `importer` | Bitwarden JSON 导入（unencrypted）+ 导出 |
| `totp` | TOTP 6 位验证码生成（RFC 6238） |
| `unlock` | 解锁流程：主密码校验 → K_machine 派生 → app_key 解密 |
| `sync` | Git 同步：md5 增量 diff + 文件序列化 + pull/push engine |
| `attempt_guard` | 主密码错误退避（指数退避 1s/2s/4s/8s/16s/30s） |
| `validate` | 输入校验（bundle_id 白名单、主密码强度） |

---

## 2. 数据结构

### Cipher（密码条目）

```rust
struct Cipher {
    id: String,              // UUID v4（v44 起，支持 git 同步跨设备无冲突）
    folder_id: Option<String>,
    favorite: bool,
    name: String,            // 加密存储
    notes: Option<String>,   // 加密存储
    data: CipherData,        // 目前仅 Login 变体，加密存储
    fields: Vec<Field>,      // 加密存储
    password_history: Vec<String>,
    reprompt: RepromptType,  // None=不需要 / Password=使用时再验主密码
    deleted_at: Option<String>, // 软删时间戳（回收站）
    created_at: String,
    updated_at: String,
}

enum CipherData {
    Login(LoginData),        // 未来扩展 SecureNote / Card / Identity
}

struct LoginData {
    uris: Vec<LoginUri>,     // 匹配策略 per-uri
    username: Option<String>,
    password: Option<String>,
    totp: Option<String>,    // Base32 secret 或 otpauth URL
}
```

### DB 表（schema v38 起）

| 表 | 说明 |
|---|---|
| `vault_meta` | 单行：加密状态标记、K_machine 密文、app_key_local_enc、app_key_sync_enc、security_stamp、PBKDF2 参数 |
| `vault_ciphers` | 密码条目：id(UUID) + folder_id + name(密文) + data(密文 JSON) + reprompt + deleted_at + sync_md5 |
| `vault_folders` | 文件夹：id(UUID) + name(密文) + sync_md5 |

---

## 3. 加密层

### 密钥层级

```
主密码 ──Argon2id──→ K_master（不落盘）
                          │
K_machine（本地文件密文）──┴── HKDF ──→ app_key
                                          │
                              ┌───────────┴───────────┐
                          app_key_local_enc      app_key_sync_enc
                          （解密 K_machine 用）    （K_master 直接加密 app_key，
                           主密码改了就废         git 同步跨设备解 app_key 用）
```

- **K_machine**：首次 setup 时 `OsRng` 生成 32 字节随机数，用 `app_key` 加密后存 `vault_meta.app_key_local_enc`（本地文件方式，非 OS Keychain——macOS adhoc 签名 binary 写 Keychain 是 session-only 不持久）
- **app_key**：实际加密 cipher 数据的对称密钥（AES-256-GCM）
- **改主密码**：用旧主密码解出 app_key → 用新主密码重新加密 app_key → cipher 数据不变

### 加解密

- 密文格式：`v1:<base64(nonce[12B] || ciphertext || tag[16B])>`
- 每个字段独立加密（name / notes / data / fields / password_history 各一个密文）
- `DerivedKey`（`Zeroizing<[u8; 32]>`）用后自动清零

---

## 4. 解锁 / 锁定

| 操作 | 行为 |
|---|---|
| **Setup**（首次） | 主密码 → Argon2id → K_master → 生成 K_machine → 加密 app_key → 写 vault_meta |
| **Unlock** | 主密码 → Argon2id → K_master → 解 app_key_local_enc 拿 app_key → 存内存 `SharedVaultSession` |
| **Lock** | 清零 `SharedVaultSession` 里的 user_vault_key |
| **自动锁定** | 可配超时（30s/1min/3min/5min/15min/Never，默认 3min）。心跳机制：前端每 30s 调 `vault_heartbeat` 刷新 `last_active_at`，超时自动锁 |
| **错误退避** | 主密码错误次数 → 指数退避（1s→2s→4s→8s→16s→30s），正确密码 reset |

---

## 5. Auto-Type（自动填充）

### 触发流程

1. 全局热键 `Cmd+Shift+L`（可配）
2. 热键 callback **先抓浏览器 URL**（此时浏览器还前台）→ 存 `SharedPickerUrlCache`
3. 弹出 VaultPicker 浮窗（320×360 固定 + transparent 圆角）
4. 前端调 `vault_detect_and_match` → 读缓存 URL → eTLD+1 匹配 cipher
5. 用户选 cipher + 模式 → `vault_autotype` → **后端 hide 浮窗** → 浏览器回前台 → 键盘模拟注入

### URL 匹配（防钓鱼）

- **eTLD+1**：用内嵌 Mozilla public suffix list（`crates/vault/data/public_suffix_list.dat`），非「split-on-dot take last two」——`barclays.co.uk` 不会匹配 `evil-attacker.co.uk`
- **5 种策略**（per-uri `match_type`）：Domain（默认，eTLD+1）/ Host / Exact / StartsWith / RegularExpression / Never
- **等价域名**：`default_equivalent_domains()`（如 google.com ↔ youtube.com）
- **URL 检测失败 → 返回空列表**（2026-07-21 安全加固）：不返回 fallback 最近 20 条，防钓鱼误选。用户可通过搜索框主动搜索（`vault_search_ciphers`）

### 填充模式

| 模式 | 行为 | 适用场景 |
|---|---|---|
| `UsernamePassword` | 填用户名 + 密码 | 标准 login 表单 |
| `PasswordOnly`（默认） | 只填密码 | webmail SPA（Tab 不可靠 / iframe 密码框） |
| `UsernameOnly` | 只填用户名 | 分步登录 |

### 安全约束（INV-A 系列）

| 不变量 | 说明 |
|---|---|
| INV-A7 | reprompt=1 的 cipher，autotype/复制密码时后端强制再校验主密码（DevTools 不可绕过） |
| INV-A11 | 热键 callback 抓 URL 必须在 show VaultPicker 之前（否则浮窗抢前台，URL 检测失败） |
| INV-A12 | URL 检测失败时返回空列表（不 fallback，防钓鱼） |
| INV-A13 | 默认 PasswordOnly 模式（webmail SPA Tab 不可靠） |
| INV-A14 | hide VaultPicker 由后端做（前端 hide + invoke 有 race condition） |

---

## 6. 回收站

- 文本 cipher 删除走软删（`UPDATE deleted_at`），可还原 / 永久删
- 回收站 tab 仅在 Settings → Vault → CipherList（VaultPicker 浮窗无回收站）
- 操作：还原（`vault_restore_cipher`）/ 永久删（`vault_delete_cipher permanent=true`）/ 全部清空（`vault_empty_trash`）

---

## 7. 密码生成器

| 模式 | 说明 |
|---|---|
| random | 随机字符（可配长度、大小写/数字/符号） |
| passphrase-en | 英文单词 passphrase（可配单词数、分隔符、首字母大写、含数字） |
| passphrase-zh | 中文词语 passphrase（4096 词表） |
| pin | 纯数字 PIN |

两个入口：CipherEditor 密码字段旁 🔑（填入表单）+ ActionBar 独立浮窗（生成后直接 Auto-Type 到前台浏览器）。

---

## 8. 健康检查

- **密码强度**：zxcvbn 评分（0-4）+ 熵估算，后端 `vault_evaluate_password` 命令（前端不打包 zxcvbn）
- **重复密码**：检测多条 cipher 使用相同密码
- **报告**：`vault_health_report` 返回 weak/duplicate 列表

---

## 9. 导入导出

- **导入**：Bitwarden unencrypted JSON（`encrypted: false`）。加密导出不支持。dedup 逻辑：软删后再导入同一份会重新入库
- **导出**：Bitwarden unencrypted JSON 格式（`encrypted: false`），解密全部 cipher 后序列化

---

## 10. Git 跨设备同步

详见 `docs/superpowers/specs/2026-07-21-vault-git-sync-design.md`。核心机制：

- 用 git repo（GitHub/Gitee private repo）同步，shell out 系统 git，SSH key 认证
- `~/.octopus/.sync/vault/` 目录：meta.json + outline.json（md5 增量索引）+ `ciphers/<2hex>/<uuid>.json`（256 桶分片）
- md5 内容指纹（`sync_md5` 字段）做增量 diff——只 push 变化的文件
- 跨设备密钥一致性：app_key_sync_enc 用主密码直接加密 app_key，任何设备只要知道主密码就能解
- security_stamp 守卫：pull 时对比 stamp，不一致拒绝覆盖（防主密码改了但没同步）

UI 入口：系统设置 → Git 同步 tab（不依赖 vault 解锁）。

**自动同步**（Phase 2）：`octopus-scheduler` 的 `vault_sync` 任务（interval=3600s = 1 小时），CPU 空闲时自动调 `sync_now()` 同步 vault + 热词。结果存 `.sync/last_auto_sync.json`（SyncPanel 展示上次同步时间/结果，不弹 toast）。

---

## 11. Tauri 命令清单

| 命令 | 说明 |
|---|---|
| `vault_status` / `vault_setup` / `vault_unlock` / `vault_lock` / `vault_heartbeat` | 生命周期 |
| `vault_list_ciphers` / `vault_get_cipher` / `vault_create_cipher` / `vault_update_cipher` | CRUD |
| `vault_delete_cipher(id, permanent)` / `vault_restore_cipher` / `vault_empty_trash` | 删除/回收站 |
| `vault_detect_and_match` / `vault_search_ciphers` / `vault_get_cached_url` | URL 匹配/搜索 |
| `vault_autotype` / `vault_copy_password` / `vault_copy_username` | Auto-Type/复制 |
| `vault_generate` / `vault_evaluate_password` / `vault_generate_totp` / `vault_health_report` | 工具 |
| `vault_list_folders` / `vault_create_folder` / `vault_rename_folder` / `vault_delete_folder` | 文件夹 |
| `vault_import_bitwarden` / `vault_export` | 导入导出 |
| `vault_change_password` | 改主密码 |
| `vault_get_lock_timeout` / `vault_set_lock_timeout` | 锁定超时 |
| `open_password_generator` / `password_generator_autotype` | 密码生成器浮窗 |

---

## 12. 前端页面

| 组件 | 说明 |
|---|---|
| `Settings/Vault/VaultPanel` | 主管理面板：解锁/初始化 + CipherList + 文件夹侧栏 |
| `Settings/Vault/CipherList` | 列表 + 搜索 + tab（所有/收藏/类型/回收站）+ 卡片网格 |
| `Settings/Vault/CipherEditor` | 编辑/新建表单：name/url/username/password/totp/notes/favorite/folder/reprompt |
| `Settings/Vault/FolderSidebar` | 左侧导航：所有条目/收藏/folders/回收站 |
| `VaultPicker` | Auto-Type 浮窗：URL 匹配列表 + 搜索 + 三段式 cipher 行 + 内联新建 |
| `PasswordGenerator` | 密码生成器主体（Modal / 独立浮窗两个外壳共用） |
| `UnlockDialog` / `SetupWizard` | 解锁/初始化弹窗 |
