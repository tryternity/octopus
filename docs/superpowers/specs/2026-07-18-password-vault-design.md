# 密码管理功能设计（Password Vault）

> **日期**：2026-07-18
> **分支**：`research_password_vault`
> **状态**：已实现并同步到代码（含 follow-up 修订）
> **调研依据**：`docs/research/2026-07-18-password-vault-research.md`
> **目标读者**：后续实施者（plan/实现/review）
>
> **同步修订（2026-07-18 follow-up）**：实施过程中几处设计变更已在文中就地修正，
> 主要差异：① K_machine 从 OS Keychain 改为本地加密文件（macOS adhoc 签名 binary 写
> Keychain 是 session-only 不持久，详见 §2.5）；② 锁定超时从固定 15min 改为可配置（默认
> 3min）+ 心跳机制（§2.7）；③ 密码生成器从全局浮窗移入 CipherEditor 内嵌（§5.2 / 附录 B / 附录 D）；
> ④ vault crate 内部用 `anyhow`，Tauri 命令边界用 `VaultError` 用户安全错误（§7.2）；
> ⑤ `octopus-desktop` 加 `vault` cargo feature gate（附录 E）；⑥ folder 完整 UI（§3.4 / §5）；
> ⑦ ZH 词表来源改为 jieba 词频 TOP 4096（§5.2.4）。
> 详见 plan 文档「Follow-up Work」节。

---

## 0. 目标与范围

### 0.1 核心目标

为 octopus 引入「密码生成与保存 + 自动填充」功能，主要使用场景：

1. **全局热键 `Cmd+Shift+L` 触发 Auto-Type**（检测浏览器 URL → 匹配 cipher → 模拟键盘填充）
2. **cipher 编辑器内嵌密码生成器**（随机字符、英文 passphrase、中文 passphrase、PIN）—— 不再有独立浮窗
3. **API Key 加密存储**（顺手解决现有 `models.secret_key` 明文问题）
4. **（未来 P2）全局热键唤起 Quick Access 浮窗，搜索密码并复制到剪贴板**

### 0.2 MVP 范围

| 包含 | 不包含 |
|---|---|
| Auto-Type（macOS）+ 剪贴板 fallback | 浏览器扩展（P2） |
| 双层加密 vault（user_vault_key + app_key） | 云同步（P2，仅密钥层预留） |
| Argon2id + HMAC-SHA512 简化 BIP44 + AES-256-GCM | 完整 BIP32/44 协议 |
| Y+ 双密文存储（K_machine 本机 + master_key 同步） | Bitwarden 完整协议兼容 |
| Login cipher 类型 | SecureNote / Card / Identity（未来） |
| TOTP 生成（HMAC-SHA1） | HIBP 在线查询（P2） |
| 密码生成器（Random + EN/ZH Passphrase + PIN） | 生成器历史（用户决策不存） |
| 本地健康检查（弱密码 + 重复密码） | HIBP 泄露查询（P2） |
| Bitwarden unencrypted JSON 导入 | CSV/1Password/KeePass 导入（P1） |
| 手动导出 JSON 作为备份 | HIBP 泄露查询（P2） |
| 文件夹管理（侧边栏 + CRUD + 加密文件夹名） | 云同步（P2，仅密钥层预留） |
| AppleScript URL 检测（Chrome/Safari/Firefox/Edge/Brave/Arc） | Windows/Linux Auto-Type（P1/P2） |
| 可配置锁定超时（30s / 1min / 3min / 5min / 15min / Never，默认 3min） | 固定 15min 超时（已废弃） |
| Cargo feature gate（`--no-default-features` 可关掉 vault） | |
| VaultError 用户安全错误（11 个分类 + JSON 序列化） | |

### 0.3 非目标

- **不做多用户/团队协作**（octopus 是单用户桌面工具）
- **不做云服务端**（除非未来加可选 self-hosted sync provider）
- **不兼容旧 schema**（只一个开发者，强制迁移）
- **不实现 passkey 提供方**（超出 MVP）
- **不实现 1Password Secret Key 机制**（octopus 单机无服务端，价值有限）

---

## 1. 架构总览

### 1.1 Crate 结构

```
crates/
├── infra/                     (已有，新增 vault schema 表)
│   └── src/db.sql             新增 vault_meta / vault_ciphers / vault_folders（v38）
│
├── vault/                     (新增 crate，纯逻辑库，依赖 infra)
│   └── src/
│       ├── lib.rs
│       ├── crypto/            加密原语
│       │   ├── mod.rs
│       │   ├── kdf.rs         Argon2id
│       │   ├── symmetric.rs   AES-256-GCM + DerivedKey
│       │   ├── hierarchy.rs   HMAC-SHA512 child() 派生（简化 BIP44）
│       │   └── util.rs        随机 / Base64 / 常量时间比较
│       ├── storage/           SQLite 读写
│       │   ├── mod.rs
│       │   ├── meta.rs        vault_meta CRUD
│       │   ├── cipher.rs      vault_ciphers CRUD（密文层）
│       │   └── folder.rs      vault_folders CRUD（folder 名用 user_vault_key 加密）
│       ├── generator/         密码生成器（所有 generate 返回 Result<String>）
│       │   ├── mod.rs         GeneratorConfig + dispatch（返回 Result）
│       │   ├── random.rs
│       │   ├── passphrase_en.rs  EFF 7776 词表
│       │   ├── passphrase_zh.rs  jieba 词频 TOP 4096 双字词
│       │   ├── pin.rs
│       │   ├── eff_wordlist.rs
│       │   └── zh_wordlist_4096.rs
│       ├── matcher/           URL 匹配
│       │   ├── mod.rs
│       │   └── psl.rs         eTLD+1（publicsuffix crate）
│       ├── importer/          Bitwarden 导入
│       │   ├── bitwarden.rs
│       │   └── exporter.rs
│       ├── health/            本地密码健康检查
│       │   ├── mod.rs
│       │   ├── strength.rs    zxcvbn 强度
│       │   └── duplicate.rs   重复检测
│       ├── totp.rs            TOTP 生成
│       ├── unlock.rs          解锁态管理（5 大流程 + 双密文）
│       ├── keychain.rs        K_machine 本地加密文件（HKDF-SHA256 派生 file_key）
│       ├── migrate.rs         一次性迁移 models.secret_key
│       ├── types.rs           Cipher / LoginData / MatchType 等
│       └── error.rs           anyhow 风格辅助（无自定义 enum）
│
└── desktop/                   (已有，新增命令与平台集成；全部 #[cfg(feature = "vault")] 门控)
    ├── src/
    │   ├── vault_commands.rs      Tauri 命令（含 folder CRUD + heartbeat + lock_timeout）
    │   ├── vault_state.rs         AppState（SharedVaultSession = Arc<RwLock<VaultSession>>）
    │   ├── vault_error.rs         VaultError enum + classify(anyhow) + JSON 序列化
    │   ├── vault_secret_access.rs secret_key 解密 chokepoint（feature off 时退化为 raw 原样返回）
    │   └── autotype/
    │       ├── mod.rs             trait AutoType
    │       ├── macos.rs           enigo + AppleScript
    │       ├── url_detect.rs      AppleScript 取浏览器 URL
    │       └── clipboard.rs       concealed 剪贴板写入
    └── frontend/src/pages/
        ├── Settings/Vault/        vault UI 子目录
        │   ├── VaultPanel.tsx     主面板（含 folder 侧边栏 + lock timeout 设置）
        │   ├── CipherList.tsx
        │   ├── CipherEditor.tsx   含内嵌生成器按钮 + folder dropdown
        │   ├── UnlockDialog.tsx
        │   ├── SetupWizard.tsx
        │   ├── HealthReport.tsx
        │   ├── ImportExport.tsx
        │   ├── FolderSidebar.tsx  folder 侧边栏
        │   ├── FolderPromptDialog.tsx
        │   ├── buildConfig.ts     生成器配置（前端 clamp 输入到合法范围）
        │   └── validateMasterPassword.ts  12 字符 + 4 字符类校验
        └── (无独立 PasswordGenerator 浮窗)
```

> **全局热键变更**：原计划的 `Cmd+Shift+G` 全局浮窗 + `password_generator_window`
> 已废弃——生成器 UI 内嵌到 CipherEditor。`AppConfig.vault_generator_shortcut`
> 字段保留仅为兼容旧 DB，运行时不消费。当前唯一 vault 全局热键是 `Cmd+Shift+L`
> （Auto-Type picker）。

### 1.2 依赖图

```
infra        (无项目内依赖)
   ↑
   │
vault        (新增，依赖 infra)
   ↑
   │
desktop      (依赖 vault，通过 `vault` cargo feature 门控)
```

其他 crate（asr/llm/cli/server/dlp/...）**完全不变**。`vault` feature off 时
`octopus-desktop` 仍可编译，仅丢失 vault UI + Auto-Type + 命令（详见附录 E）。

### 1.3 Workspace 调整

```toml
# Cargo.toml (workspace)
[workspace]
members = [
    "crates/infra", "crates/onnx-infra", "crates/asr-local", "crates/asr-cloud",
    "crates/server", "crates/cli", "crates/desktop", "crates/llm", "crates/dlp",
    "crates/download", "crates/clipboard", "crates/ocr", "crates/paddle-ocr",
    "crates/capx", "crates/translation", "crates/search",
    "crates/vault",   # ← 新增
]
```

### 1.4 vault crate 依赖

```toml
# crates/vault/Cargo.toml
[dependencies]
octopus-infra = { path = "../infra" }

# 加密
argon2 = { version = "0.5", features = ["std"] }
aes-gcm = { version = "0.10", features = ["std"] }
hkdf = "0.12"             # machine-key.enc 的 file_key 派生
hmac = "0.12"
sha2 = "0.10"
rand = "0.8"
zeroize = { version = "1.7", features = ["zeroize_derive"] }
data-encoding = "2"
subtle = "2"

# TOTP
totp-rs = { version = "5", default-features = false }

# eTLD+1
publicsuffix = "2"

# URL 解析（matcher 用）
url = "2"

# 强度评估
zxcvbn = "3"

# 序列化/工具
serde = { version = "1", features = ["derive"] }
serde_json = "1"
anyhow = "1"
log = "0.4"
parking_lot = { workspace = true }
regex = "1"               # RegularExpression match type
uuid = { version = "1", features = ["v4"] }   # security_stamp 生成

[dev-dependencies]
rusqlite = { version = "0.31", features = ["bundled"] }   # set_test_db 注入 in-memory 连接
```

**不依赖**：`tokio`（纯同步）、`tauri`、`reqwest`（无网络）、`keyring`（已废弃——K_machine 改为本地加密文件，详见 §2.5）、`thiserror`（vault crate 内部全用 anyhow）。

> **`keyring` 移除背景**：macOS 对 adhoc 签名 binary（octopus 开发版）写入的 Keychain 项是 session-only，
> 进程退出即丢，"本机无感启动" 完全失效。改用本地加密文件后跨平台行为一致（Linux/Windows 同样可工作）。

### 1.5 desktop crate 依赖调整

```toml
# crates/desktop/Cargo.toml
[dependencies]
# ... 已有依赖不变 ...

# Vault（新增，optional + feature gate）
octopus-vault = { path = "../vault", optional = true }
url = { version = "2", optional = true }   # matcher 用

# enigo（已有，用于 Auto-Type 跨平台键盘模拟）
enigo = "0.6"

[features]
default = ["embedded", "vault"]
vault = ["dep:octopus-vault", "dep:url"]
```

**feature 语义**（详见附录 E）：
- `vault` on（默认）：完整 vault UI + 命令 + Auto-Type + 全局热键 `Cmd+Shift+L`。
- `vault` off：`cargo build --no-default-features --features embedded`，vault 模块全部 cfg 掉，binary 更小、无密钥访问、无密码管理 UI。
- `vault_secret_access.rs` 总是编译（云端推理热路径的 chokepoint），feature off 时 `try_decrypt_secret_global` 退化为 raw 原样返回。

---

## 2. 加密层

### 2.1 密钥派生树（HMAC-SHA512，简化 BIP44）

```
master_password (用户脑中，永不出用户)
       │
       │ Argon2id(password, random_salt[32B], t=3, m=64MiB, p=4)
       │ 输出 32B master_root_key
       ▼
master_root_key: Zeroizing<[u8; 32]>  (派生后立即 zeroize)
       │
       │ HMAC-SHA512(parent, label)，取前 32B
       ▼
   ├── child("octopus/v1/user-vault")  → user_vault_key  (加密 cipher)
   ├── child("octopus/v1/app-secrets") → app_key         (加密 API Key)
   ├── child("octopus/v1/sync")        → sync_key        (预留，MVP 不生成)
   └── child("octopus/v1/send")        → send_key        (预留，MVP 不生成)
```

### 2.2 派生函数

```rust
pub struct DerivedKey(Zeroizing<[u8; 32]>);

impl DerivedKey {
    /// HMAC-SHA512(parent, label)，取前 32B 作为子 key
    pub fn child(&self, label: &[u8]) -> DerivedKey {
        let mut mac = Hmac::<Sha512>::new_from_slice(&self.0.0).unwrap();
        mac.update(label);
        let result = mac.finalize().into_bytes();
        let mut child = [0u8; 32];
        child.copy_from_slice(&result[..32]);
        DerivedKey(Zeroizing::new(child))
    }

    /// AES-256-GCM 加密，返回 "v1:<base64(nonce||ciphertext||tag)>"
    pub fn encrypt(&self, plaintext: &[u8]) -> Result<String>;

    /// AES-256-GCM 解密（输入须以 v1: 开头）
    pub fn decrypt(&self, ciphertext: &str) -> Result<Zeroizing<Vec<u8>>>;
}
```

**为什么不保留 stretched key 的 MAC 部分**：AES-256-GCM 自带认证（16B tag），不需要独立 HMAC。原 Bitwarden 的 enc+mac 二元组是为 AES-CBC+HMAC 准备的，octopus 不需要兼容。

**为什么用 HMAC-SHA512 而不是 HKDF-Expand**：HMAC 树形派生是简化 BIP44 思想，未来加新功能（sync/send/folders）只需新增 label，零架构改动。HMAC 与 HKDF 内部都是 HMAC，安全等价。

### 2.3 KDF 参数

| 参数 | 默认值 | 范围 |
|---|---|---|
| `kdf_type` | 0 (Argon2id) | MVP 仅 0 |
| `kdf_iterations` | 3 (t) | 1-10 |
| `kdf_memory_kib` | 65536 (64 MiB) | 16384-262144 |
| `kdf_parallelism` | 4 (p) | 1-16 |
| `kdf_salt` | 32B 随机 | 固定 |

**Argon2id t=3, m=64MiB** 是 OWASP 2024 推荐值，比 Bitwarden 默认 m=32MiB 稍激进，安全性更高。每次解锁耗时取决于硬件（M1 Mac 约 0.3-0.5s，旧 Intel 机器可能 1-2s），均在可接受范围。

参数存 `vault_meta` 表，未来调整强度无需改 schema。

### 2.4 落盘的密文数据

| 字段 | 加密内容 | 用什么 key 加密 |
|---|---|---|
| `vault_ciphers.name` | cipher 名 | user_vault_key |
| `vault_ciphers.notes` | 备注 | user_vault_key |
| `vault_ciphers.data` | JSON `{uris, username, password, totp}` 全字段 | user_vault_key |
| `vault_ciphers.fields` | JSON 自定义字段 | user_vault_key |
| `vault_ciphers.password_history` | JSON 密码历史 | user_vault_key |
| `vault_folders.name` | folder 名（follow-up：folder UI 完整后改为加密） | user_vault_key |
| `vault_meta.protected_user_vault_key` | 加密的 user_vault_key | master_root_key |
| `vault_meta.app_key_local_enc` | 加密的 app_key（本机无感启动） | **K_machine**（本地文件） |
| `vault_meta.app_key_sync_enc` | 加密的 app_key（跨机同步） | **master_root_key** |
| `~/.octopus/machine-key.enc` | K_machine 明文（32B） | **file_key**（HKDF(machine_id + USER)） |
| `models.secret_key`（重构后） | API Key 密文 | app_key |

**密文格式（统一）**：`v1:<base64(nonce[12B] || ciphertext || tag[16B])>`。
`machine-key.enc` 同格式，但解密用 `file_key`（不是 vault 派生 key）。

### 2.5 K_machine 存储（本地加密文件）

K_machine 用于加密 `app_key_local_enc`（本机无感启动时解开）。早期设计把它存进
OS Keychain（macOS Keychain / Windows Credential Manager / Linux Secret Service），
**实施时改为本地加密文件**，原因：

- **macOS adhoc 签名问题**：octopus-desktop 开发版是 adhoc 签名，macOS 把此类 binary 写入
  Keychain 的项标记为 **session-only**——重启进程后立刻丢失（"本机无感启动"完全失效，
  每次重启都丢 K_machine → 解不开 `app_key_local_enc` → 强制弹主密码）。
- **跨平台一致**：Linux 无 secret service / Windows Credential Manager 走 `keyring` crate
  各家实现行为不一，本地文件方案三端完全一致。

**文件位置**：`~/.octopus/machine-key.enc`（0600 权限 on Unix）。

**file_key 派生**：
```
file_key = HKDF-SHA256(
    salt = b"octopus-vault-machine-key-file/v1",
    ikm  = machine_id || USER,        // machine_id 见下，USER 是用户名
    info = b"file-key",
    len  = 32 字节
)
```

**machine_id 来源**（跨平台）：
- macOS：`ioreg` 读 `IOPlatformUUID`（重启稳定，每机唯一）
- Linux：`/etc/machine-id` 或 `/var/lib/dbus/machine-id`
- Windows：`reg query ... MachineGuid`

**威胁模型不变**：
- 只拿到 DB（没拿到本机文件）→ 解不开 `app_key_local_enc`（仍需主密码走流程 C）
- 同机 + 同用户 → 同 `file_key` → 能解密（重启、kill -9 后仍可用）
- 换机或换用户 → 不同 `file_key` → 解不开（迁移需主密码）

**已知工程折衷**（2026-07-19 安全审查 #9 补遗，spec 原未覆盖）：
**本方案不防「同机同用户恶意/root 进程」**——`IOPlatformUUID`（ioreg）和 `$USER`
都是任何本机同用户进程公开可读的，文件 0600 仅防其他用户、不防同用户/root。
任意同用户进程（恶意脚本、被劫持插件）读 `machine-key.enc` + `ioreg` 拿 UUID +
读 `$USER` → HKDF 复算 `file_key` → 解出 `K_machine` → 解 `app_key_local_enc`
→ 解密所有 `models.secret_key`。原先期望的"另一应用想使用钥匙串"系统弹窗保护
完全失效。

缓解方向（**P2 单独迭代**）：
- UI 明示"本机加密仅防拆盘/备份，不防本机恶意进程"
- 优先恢复 OS Keychain 路径（正式签名后）
- 考虑要求主密码派生参与 `file_key`（牺牲无感启动）

**公开 API 不变**：`load_or_create_machine_key` / `load_machine_key` /
`save_machine_key` / `delete_machine_key`，调用方（unlock.rs / tests）无需改动。
测试用 `set_test_keychain` thread_local override 注入 in-memory 替身。

### 2.6 三大流程

#### 流程 A：首次初始化（用户设置 vault）

```
1. 用户在 VaultPanel 输入 master_password（两次确认，前端校验 12 位 + 4 字符类）
2. generate random 32B kdf_salt → vault_meta.kdf_salt
3. derive_master_key(password, kdf_salt, params) → master_root_key
4. zeroize master_password（&str 由调用方作用域管理，见 INV-1）
5. 派生 user_vault_key = master_root_key.child("octopus/v1/user-vault")
   派生 app_key = master_root_key.child("octopus/v1/app-secrets")
6. 随机生成 K_machine (32B) → 用 file_key 加密 → 落盘 machine-key.enc（首次）
7. 加密 user_vault_key → protected_user_vault_key（用 master_root_key）
   加密 app_key → app_key_local_enc（用 K_machine）
   加密 app_key → app_key_sync_enc（用 master_root_key）
8. zeroize master_root_key
9. 生成 security_stamp (UUID v4)
10. 落盘 vault_meta（单行）
11. 一次性迁移现有 models.secret_key（migrate.rs）：遍历 is_local=0 且不以 v1: 开头的行
    → 用 app_key 加密 → 回写 models.secret_key
12. AppState 持有 user_vault_key + app_key
```

#### 流程 B：本机启动（用户无感）

```
1. AppState::new()：
   - 读 vault_meta 表
   - 如不存在 → 标记 vault 未初始化，前端展示 Setup 页
   - 如存在：
     a. 从 machine-key.enc 读 K_machine（file_key 解密；失败则降级到流程 C）
     b. 用 K_machine 解 app_key_local_enc → app_key
     c. app_key 存入 AppState（SharedVaultSession = Arc<RwLock<VaultSession>>）
     d. user_vault_key 暂未解（用户密码 vault 锁定）
2. 启动期间所有 ASR 调用通过 vault_secret_access::try_decrypt_secret_global
   解 models.secret_key → API key 在内存（follow-up #7 统一 chokepoint）
3. 用户访问 vault UI 或触发 Auto-Type 时才需要解锁 user_vault_key（流程 D）
```

#### 流程 C：换机器首次启动 / K_machine 缺失

```
1. machine-key.enc 不存在或解 app_key_local_enc 失败
2. 弹主密码输入框
3. derive_master_key → master_root_key
4. 解 app_key_sync_enc → app_key
5. 解 protected_user_vault_key → user_vault_key
6. 用本机新生成的 K_machine 重新加密 app_key → app_key_local_enc，落盘
   + 用 file_key 加密 K_machine → 落盘 machine-key.enc
7. AppState 同时持有 user_vault_key 和 app_key
```

#### 流程 D：解锁用户 vault（超时锁定后）

```
1. 用户点 vault UI 或 Auto-Type 触发，user_vault_key 不在内存
2. 弹主密码输入框
3. derive_master_key → master_root_key
4. 解 protected_user_vault_key → user_vault_key
5. AppState 持有 user_vault_key + 初始化 unlocked_at + last_active_at
6. 超时由 is_user_vault_unlocked(timeout_secs) 在每次访问时按需检查
   （timeout 来自 AppConfig.vault_lock_timeout_secs，默认 180s/3min）
7. （app_key 不受影响，仍可用，云端 ASR 不中断）
```

#### 流程 E：改主密码

```
1. 用户输入旧密码 → derive master_root_key_old
2. 解 protected_user_vault_key → user_vault_key（验证旧密码正确）
3. 用户输入新密码（前端同样校验 12 位 + 4 字符类）→ derive master_root_key_new
4. 用 master_root_key_new 重新加密 user_vault_key → 新 protected_user_vault_key
5. 用 master_root_key_new 重新加密 app_key → 新 app_key_sync_enc
6. 用 K_machine 重新加密 app_key → 新 app_key_local_enc（K_machine 不变）
7. 刷新 security_stamp（让其他机器同步后强制走流程 C）
8. 落盘
9. follow-up：前端需重新弹解锁框（因 VaultSession 仍持有旧 user_vault_key）
```

**关键不变量**：改主密码不需要重加密 vault_ciphers（因为 user_vault_key 不变），只重加密 3 个元数据密文（`protected_user_vault_key` + `app_key_sync_enc` + `app_key_local_enc`）+ 刷新 `security_stamp`。

### 2.7 锁定超时（可配置 + 心跳）

锁定行为在 follow-up 阶段从「固定 15min / 从 unlocked_at 起算」改为「可配置 / 从
last_active_at 起算 / 心跳维护」。设计理由：

- **焦点失活语义**：用户离开 vault tab（切走 / 关窗）后不应继续保留解锁态——
  从 `last_active_at` 起算比从 `unlocked_at` 起算更安全。
- **可配置**：默认 3min 偏激进（"用过就走"），但提供 Never（0）给常驻场景。

**配置字段**（`AppConfig`，详见 §3.7）：
```
vault_lock_timeout_secs: u64
  默认 180（3min），UI 提供 30s / 1min / 3min / 5min / 15min / Never 选项
  0 = 永不锁定（UI 显示警告）
```

**VaultSession 字段**：
```rust
pub struct VaultSession {
    pub user_vault_key: Option<DerivedKey>,
    pub app_key: Option<DerivedKey>,
    pub unlocked_at: Option<Instant>,        // 解锁时刻（仅记录，不参与超时判断）
    pub last_active_at: Option<Instant>,     // 最近一次心跳时刻（超时基准）
}

pub fn is_user_vault_unlocked(&mut self, timeout_secs: u64) -> bool;
```

**心跳协议**：
- 前端 VaultPanel mount 后，每 30s 调一次 `vault_heartbeat` Tauri 命令（刷新 `last_active_at`）。
- VaultPanel unmount（切 tab / 关窗）→ 心跳停止 → `now - last_active_at > timeout_secs` → 自动锁。
- 每次 `vault_unlock` 也会初始化 `last_active_at = now`。

**Tauri 命令**：`vault_heartbeat` / `vault_get_lock_timeout` / `vault_set_lock_timeout`。

### 2.8 不变量（加密层）

| # | 不变量 |
|---|---|
| INV-1 | master_password 通过 Argon2id 派生后，derived master_root_key / user_vault_key / app_key 用 Zeroizing 保护。master_password 本身作为 &str 由调用方作用域管理；Rust 字符串无法在 safe code 中可靠 zeroize（&str 是借用的，String 的 bytes 也可能被复制），故按业界惯例（rbw、bitwarden CLI 等）不做强 zeroize。 |
| INV-2 | master_root_key 在派生子 key 后立即 zeroize（子 key 才是常驻） |
| INV-3 | 所有 key 用 `Zeroizing<[u8; 32]>` 包装 |
| INV-4 | DB 中所有 vault 字段必须是 `v1:` 前缀的密文 |
| INV-5 | K_machine 永不落盘明文——存为 `~/.octopus/machine-key.enc`（AES-256-GCM 加密，file_key 由 `HKDF-SHA256(machine_id, USER)` 派生）。仅 K_machine 的密文落盘，明文 K_machine 只在内存。 |
| INV-6 | 任何密文写入必须经 vault crate 的 `DerivedKey::encrypt()`（machine-key.enc 用 file_key 同样的 encrypt 协议） |
| INV-7 | 改主密码不重加密 vault_ciphers（user_vault_key 不变） |
| INV-8 | TOTP：HMAC-SHA1, 30s, 6 位, ±1 步漂移 |
| INV-9 | 锁定超时以 `last_active_at` 为基准（非 `unlocked_at`）；前端心跳维护 `last_active_at`，vault tab 离开 + 配置时间到 → 自动锁。`timeout_secs=0` 表示永不锁定（UI 警告）。 |
| INV-10 | 主密码强度：≥ 12 字符 + 必含 4 字符类（大写/小写/数字/符号），前端 + 后端双校验。 |

---

## 3. 数据模型

### 3.1 Schema 升级（v37 → v38）

```sql
-- user_version 直接升到 38
-- 新增 3 张 vault 表
-- 不 ALTER models.secret_key（用 "v1:" 前缀判别）
-- 不做旧 schema 兼容（只一个开发者）
```

### 3.2 `vault_meta` 表（单行）

```sql
CREATE TABLE IF NOT EXISTS vault_meta (
    id                          INTEGER PRIMARY KEY CHECK (id = 1),

    -- KDF 参数（首次设置后不变，跨机同步自带）
    kdf_type                    INTEGER NOT NULL,    -- 0=Argon2id（MVP 仅支持 0）
    kdf_salt                    BLOB NOT NULL,       -- 32 字节随机盐
    kdf_iterations              INTEGER NOT NULL,    -- Argon2id: t (默认 3)
    kdf_memory_kib              INTEGER NOT NULL,    -- Argon2id: m (默认 65536 = 64 MiB)
    kdf_parallelism             INTEGER NOT NULL,    -- Argon2id: p (默认 4)

    -- 双层密钥的"保护壳"
    protected_user_vault_key    TEXT NOT NULL,       -- v1:base64(...)，被 master_root_key 加密
    app_key_local_enc           TEXT NOT NULL,       -- 被 K_machine 加密（本机无感启动）
    app_key_sync_enc            TEXT NOT NULL,       -- 被 master_root_key 加密（跨机同步）

    -- 失效控制
    security_stamp              TEXT NOT NULL,       -- 改主密码 / 改 KDF 时刷新 (UUID v4)

    -- 等价域名（URL 匹配用）
    equivalent_domains          TEXT NOT NULL DEFAULT '[]',  -- JSON 数组的数组

    -- 可选 RSA 公私钥对（未来组织/分享用，MVP 不填）
    public_key                  TEXT,
    protected_private_key       TEXT,

    created_at                  TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at                  TEXT NOT NULL DEFAULT (datetime('now'))
);
```

### 3.3 `vault_ciphers` 表

```sql
CREATE TABLE IF NOT EXISTS vault_ciphers (
    id                  INTEGER PRIMARY KEY AUTOINCREMENT,
    folder_id           INTEGER DEFAULT NULL,        -- 预留：未来 FK vault_folders(id)
    favorite            INTEGER NOT NULL DEFAULT 0,
    atype               INTEGER NOT NULL,            -- 1=Login（MVP 仅此）
    name                TEXT NOT NULL,               -- 密文 v1:base64(...)
    notes               TEXT DEFAULT NULL,           -- 密文
    data                TEXT NOT NULL,               -- 密文 JSON（见 3.5）
    fields              TEXT DEFAULT NULL,           -- 密文 JSON
    password_history    TEXT DEFAULT NULL,           -- 密文 JSON
    reprompt            INTEGER NOT NULL DEFAULT 0,  -- 0=None 1=Password
    deleted_at          TEXT DEFAULT NULL,
    created_at          TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at          TEXT NOT NULL DEFAULT (datetime('now')),
    FOREIGN KEY (folder_id) REFERENCES vault_folders(id) ON DELETE SET NULL
);

CREATE INDEX IF NOT EXISTS idx_vault_ciphers_favorite
    ON vault_ciphers(favorite) WHERE deleted_at IS NULL;
CREATE INDEX IF NOT EXISTS idx_vault_ciphers_deleted ON vault_ciphers(deleted_at);
```

### 3.4 `vault_folders` 表（folder UI 已完整实现）

```sql
CREATE TABLE IF NOT EXISTS vault_folders (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    name        TEXT NOT NULL,        -- 密文 v1:base64(...)，用 user_vault_key 加密
    sort_order  INTEGER NOT NULL DEFAULT 0,
    created_at  TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at  TEXT NOT NULL DEFAULT (datetime('now'))
);
```

**folder 完整功能**（follow-up #6 已落地）：
- 4 个 Tauri 命令：`vault_list_folders` / `vault_create_folder` / `vault_rename_folder` / `vault_delete_folder`
- VaultPanel 侧边栏：`All` / `Favorites` / `<Folders list>` / `Trash`
- CipherEditor 有 folder dropdown
- **folder 名用 user_vault_key 加密**（follow-up bug 修复：早期版本曾用明文）
- 删除 folder → 该 folder 下的 ciphers 回到根目录（FK `ON DELETE SET NULL`）

**理由**：vault_ciphers.folder_id 已有 FK 指向此表。folder 现已是 MVP 一部分，不再是「schema 预留」。

### 3.5 `data` 字段（加密前的 JSON 结构，atype=1）

```json
{
  "uris": [
    { "uri": "https://github.com", "match": 0 },
    { "uri": "https://gist.github.com", "match": null }
  ],
  "username": "user@example.com",
  "password": "p@ssw0rd!",
  "totp": "JBSWY3DPEHPK3PXP",
  "password_revision_date": "2026-07-18T10:00:00Z"
}
```

`match` 取值（直接抄 Bitwarden 协议）：
- `0` = Domain（eTLD+1，默认）
- `1` = Host
- `2` = Exact
- `3` = StartsWith
- `4` = RegularExpression
- `null` = 用客户端默认（octopus 强制 Domain）

### 3.6 `models.secret_key` 字段重构

**不 ALTER 字段**，原地改语义：
- `is_local=1`：仍是下载清单 manifest JSON（不变，**不**走解密）
- `is_local=0`：改为密文格式 `v1:base64(...)`，由 app_key 加密
- 判别：以 `v1:` 开头 → 新密文（用 app_key 解密）；否则视为待迁移数据（首次 init vault 时一次性加密回写）
- **统一 chokepoint**（follow-up #7）：所有消费端（云端 ASR / LLM polish / 云端翻译 / Settings）经
  `vault_secret_access::try_decrypt_secret_global(raw) -> String` 解密；vault 未启用 / 未初始化 / raw 不以
  `v1:` 开头时原样返回（向后兼容）。

**不兼容旧明文读取**：用户只一个开发者，且 vault 是新功能，开发完成后必然启用。

### 3.7 AppConfig 新增字段

| 字段 | 类型 | 默认 | 说明 |
|---|---|---|---|
| `vault_autotype_shortcut` | String | `"CmdOrCtrl+Shift+L"` | Auto-Type picker 全局热键 |
| `vault_generator_shortcut` | String | `"CmdOrCtrl+Shift+G"` | **已废弃**（保留仅为兼容旧 DB，运行时不消费——生成器已内嵌） |
| `vault_lock_timeout_secs` | u64 | `180`（3min） | 锁定超时；UI 选项 30s/1min/3min/5min/15min/Never；`0` = 永不锁定 |

### 3.8 不变量（数据层）

| # | 不变量 |
|---|---|
| INV-D1 | vault_meta 表永远只有 1 行（CHECK 约束） |
| INV-D2 | vault_ciphers 的 name/data/notes/fields/password_history 必须以 `v1:` 开头 |
| INV-D3 | atype=1 的 data 必须能反序列化为 LoginData |
| INV-D4 | models.secret_key 以 `v1:` 开头表示已加密（app_key），否则视为待迁移数据 |
| INV-D5 | 软删除的 cipher 不参与 Auto-Type 匹配 |
| INV-D6 | vault_folders.name 必须以 `v1:` 开头（user_vault_key 加密） |
| INV-D7 | 删除 folder 时其下 ciphers 回到根目录（FK ON DELETE SET NULL），不连带删除 cipher |

---

## 4. URL 匹配 + Auto-Type

### 4.1 触发流程

```
0. 用户已聚焦浏览器（如 Chrome），停留在 github.com/login
   ↓ 用户按全局热键（默认 Cmd+Shift+L）
1. desktop/autotype 触发器收到热键事件
   （沿用 action_bar_window.rs 的热键注册机制）
   ↓
2. 检查 vault 解锁状态（AppState）
   - user_vault_key 在内存 → 跳到 3
   - 不在 → 弹解锁窗口 → 输主密码
   ↓
3. AppleScript 取当前浏览器 URL
   ↓
4. 内存中匹配 cipher（vault::matcher::find_matching_ciphers）
   - 0 个：显示"无匹配 cipher" 浮窗 + 全 cipher 列表
   - 1 个：跳到 5
   - N 个：弹选择浮窗，按最近使用排序 → 跳到 5
   ↓
5. （如 cipher.reprompt=1）弹主密码确认框，验证后继续
   （如 cipher.reprompt=0）直接跳到 6
   ↓
6. 执行 Auto-Type
   6a. AppleScript 把浏览器带到前台
   6b. enigo 模拟键盘：username + Tab + password
       默认不按 Enter（避免误触发提交，可配置）
   6c. （若有 TOTP）生成 6 位码，写入剪贴板（30s 后清空）
   ↓
7. 收尾：关闭浮窗，更新 cipher.updated_at（记录最近使用）
```

### 4.2 URL 检测（macOS AppleScript）

```rust
// crates/desktop/src/autotype/url_detect.rs

pub fn current_browser_url() -> Result<Option<Url>> {
    let frontmost = frontmost_app_bundle_id()?;  // NSWorkspace
    let browser = match BROWSER_MAP.get(&frontmost) {
        Some(b) => b,
        None => return Ok(None),
    };
    let url_str = run_applescript(browser.script())?;
    Url::parse(&url_str).map(Some).map_err(Into::into)
}

static BROWSER_MAP: Lazy<HashMap<&str, BrowserScript>> = Lazy::new(|| {
    hashmap! {
        "com.google.Chrome"            => BrowserScript::chrome(),
        "com.apple.Safari"             => BrowserScript::safari(),
        "org.mozilla.firefox"          => BrowserScript::firefox(),
        "com.microsoft.edgemac"        => BrowserScript::chrome(),  // Chromium 内核
        "com.brave.Browser"            => BrowserScript::chrome(),
        "company.thebrowser.Browser"   => BrowserScript::arc(),
    }
});
```

**AppleScript 模板**：

```applescript
-- Chrome / Edge / Brave（都支持 same API）
tell application "Google Chrome"
    get URL of active tab of front window
end tell

-- Safari
tell application "Safari"
    get URL of current tab of front window
end tell

-- Firefox（无 AppleScript dictionary，用 System Events）
tell application "System Events"
    tell process "Firefox"
        get value of text field 1 of group 1 of toolbar 1 of window 1
    end tell
end tell

-- Arc
tell application "Arc"
    get URL of active tab of front window
end tell
```

**首次调用权限引导**：用户首次设置 vault 时主动触发一次"测试取 URL"，让 macOS 弹"octopus 想要控制 Google Chrome"授权框，避免真正用 Auto-Type 时才发现没权限。

**降级路径**：所有脚本失败时返回 `Ok(None)`，触发器跳到「手动选择 cipher」浮窗。

### 4.3 URL 匹配算法（5 种策略）

**eTLD+1 提取**（2026-07-19 安全修复 #1）：用 `publicsuffix` crate + 内嵌 Mozilla PSL
（`crates/vault/data/public_suffix_list.dat`，~330KB），正确处理多段 TLD
（`.co.uk / .com.cn / .co.jp`）；IP 字面量精确匹配（`parse::<IpAddr>` 判断）；
PSL 查不到时返回 host 本身（fail-closed）。

> **踩坑警示**：首发版用「按 `.` 分段取最后两段」简化算法，导致钓鱼漏洞——
> `barclays.co.uk` 与 `evil-attacker.co.uk` 都退化为 `co.uk` 互相匹配，
> autotype 会把银行密码注入钓鱼站。Cargo.toml 早就有 `publicsuffix = "2"` 但代码
> 故意不用。修复后增加 `test_phishing_protection_multilevel_tld` 测试锁死不变量。

```rust
// crates/vault/src/matcher/mod.rs

pub fn find_matching_ciphers(
    url: &Url,
    ciphers: &[Cipher],
    equivalent_domains: &[Vec<String>],
) -> Vec<&Cipher> {
    ciphers.iter()
        .filter(|c| c.deleted_at.is_none())
        .filter(|c| matches_any_uri(url, c, equivalent_domains))
        .collect()
}

fn match_uri_one(url: &Url, lu: &LoginUri, equivalent: &[Vec<String>]) -> bool {
    let strategy = lu.match_type.unwrap_or(MatchType::Domain);
    match strategy {
        MatchType::Domain => matches_domain(url, &lu.uri, equivalent),
        MatchType::Host => host_of(url) == host_of(&lu.uri),
        MatchType::Exact => url.as_str() == lu.uri,
        MatchType::StartsWith => url.as_str().starts_with(&lu.uri),
        MatchType::RegularExpression => Regex::new(&lu.uri).map(|r| r.is_match(url.as_str())).unwrap_or(false),
        MatchType::Never => false,
    }
}

/// Domain 匹配：用 publicsuffix crate 提取 eTLD+1，比较
fn matches_domain(url: &Url, cipher_uri: &str, equivalent: &[Vec<String>]) -> bool {
    let target_domain = etld_plus_one(url.host_str()?)?;
    let cipher_domain = etld_plus_one(Url::parse(cipher_uri).ok()?.host_str()?)?;

    let mut candidates: HashSet<String> = HashSet::new();
    candidates.insert(cipher_domain.clone());
    for group in equivalent {
        if group.contains(&cipher_domain) {
            candidates.extend(group.iter().cloned());
        }
    }
    candidates.contains(&target_domain)
}

/// eTLD+1：mail.google.com → google.com
pub fn etld_plus_one(host: &str) -> Option<String> {
    let list = publicsuffix::List::empty();  // 编译时内嵌 PSL
    let domain = list.parse_domain(host).ok()?;
    domain.root().map(|s| s.to_string())
        .or_else(|| Some(host.to_string()))  // localhost 等非 PSL 域名
}
```

### 4.4 等价域名（Equivalent Domains）

`vault_meta.equivalent_domains` 字段（JSON）：

```json
[
  ["google.com", "youtube.com", "gmail.com"],
  ["live.com", "hotmail.com", "outlook.com"],
  ["apple.com", "icloud.com"]
]
```

**MVP 默认值**（内置，借鉴 Bitwarden global_domains.json）：

```rust
static DEFAULT_EQUIVALENT_DOMAINS: &[&[&str]] = &[
    &["google.com", "youtube.com", "gmail.com", "g.co"],
    &["live.com", "hotmail.com", "outlook.com"],
    &["apple.com", "icloud.com"],
    &["amazon.com", "amazon.co.jp", "amazon.co.uk"],
];
```

MVP 不暴露编辑 UI，未来加设置页即可。

### 4.5 Auto-Type 实现（enigo 跨平台）

**焦点安全**（2026-07-19 修复 #2）：`autotype_login` 加 `expected_bundle_id: Option<&str>`
参数——注入 username 前 + password 前各调一次 `verify_focused`：
- `Some(id)`：严格白名单，前台必须 == 指定 bundle id
- `None`：最小防御，仅校验前台不是 octopus 自身（`com.octopus.desktop`），
  防 VaultPicker 未 hide 时密码打到 octopus 窗口

调用方 `vault_autotype` 命令当前传 `None`——前端 VaultPicker `handlePick` 已改为
**先 hide 浮窗再 invoke**（首发版反了，hide 在 invoke 之后）。

```rust
// crates/desktop/src/autotype/macos.rs（签名 4 参数，原 3 参数已废弃）
pub fn autotype_login(
    username: &str,
    password: &str,
    press_enter: bool,
    expected_bundle_id: Option<&str>,  // 焦点校验（修复 #2）
) -> Result<()>
```
```

**密码字段（masked input）**：enigo 在 macOS 用 CGEvent 输入，浏览器收到的是真实按键事件，能正常进 password 框。比浏览器扩展的 DOM 填充更可靠（不会被 React 受控组件拒绝）。

**Enter 键配置**：
- 默认 `press_enter=false`（避免误触发提交）
- cipher 可单独配置（未来加字段）
- MVP 全局配置即可（VaultPanel 设置项）

### 4.6 剪贴板路径（fallback / TOTP）

```rust
// crates/desktop/src/autotype/clipboard.rs

/// 复制到剪贴板，并标记为 concealed（30s 后自动清空）
pub fn copy_to_clipboard_concealed(text: &str, ttl_seconds: u64) -> Result<()> {
    // 1. 写入 NSPasteboard
    let pb = unsafe { NSPasteboard::generalPasteboard() };
    pb.clearContents();
    pb.setString_forType(NSString::from_str(text), NSPasteboardTypeString);

    // 2. 标记为 concealed（iCloud Universal Clipboard、Maccy、Paste 等会跳过）
    unsafe {
        let _ = pb.setString_forType(
            NSString::from_str("octopus-vault-concealed"),
            c"org.nspasteboard.ConcealedType",
        );
    }

    // 3. 通知 octopus 自己的 clipboard_history 监听器跳过这次写入
    CLIPBOARD_SKIP_NEXT.store(true, Ordering::SeqCst);

    // 4. spawn 定时清空
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_secs(ttl_seconds)).await;
        clear_clipboard_if_unchanged(text);
    });

    Ok(())
}
```

**与 octopus 自身 `clipboard_history` 的协调**（关键）：
- octopus 已有剪贴板监听器（`crates/clipboard/`），会把所有写入记到 DB
- **必须让监听器识别 concealed 标记并跳过**——否则密码就进了 FTS 索引被全文搜出
- 实现：监听器在 pasteboard changeCount 变化时检查 `org.nspasteboard.ConcealedType`，存在则不入库

### 4.7 不变量（URL 匹配 + Auto-Type）

| # | 不变量 |
|---|---|
| INV-A1 | URL 检测失败时必须 fallback 到手动选择，不能 silently fail |
| INV-A2 | Auto-Type 前必须确认目标窗口仍是浏览器（用户可能切走）—— 2026-07-19 修复 #2 落地：`verify_focused` 在注入前后各校验一次 |
| INV-A3 | 模拟键盘前必须有 100ms+ 焦点等待 |
| INV-A4 | 剪贴板复制必须有 30s 自动清空 + concealed 标记 |
| INV-A5 | octopus 自身 clipboard 监听器必须跳过 concealed 内容 |
| INV-A6 | 默认不按 Enter（避免误触发提交） |
| INV-A7 | 弹主密码确认框（reprompt=1 的 cipher）验证不通过则中止 —— 2026-07-19 修复 #3：后端 `vault_autotype` 强制校验 `master_password`，不可绕过 |
| INV-A8 | eTLD+1 必须用 Mozilla PSL（公共后缀列表），不能用「分段取末两段」简化算法——2026-07-19 修复 #1：钓鱼漏洞（`barclays.co.uk` vs `evil-attacker.co.uk`）；IP 字面量精确匹配 |

### 4.8 跨平台扩展计划

| 平台 | URL 检测 | Auto-Type | MVP |
|---|---|---|---|
| macOS | AppleScript（6 浏览器） | enigo（CGEvent） | ✅ |
| Windows | UIAutomation（P1） | enigo（SendInput） | ❌ P1 |
| Linux | xdotool getactivewindow + DBus（P2） | enigo（X11） | ❌ P2 |

---

## 5. TOTP + 密码生成器 + 健康检查

### 5.1 TOTP（RFC 6238）

```rust
// crates/vault/src/totp.rs
use totp_rs::{Algorithm, TOTP, Secret};

pub struct TotpGenerator { inner: TOTP }

impl TotpGenerator {
    pub fn from_base32(secret: &str) -> Result<Self> {
        let bytes = Secret::Encoded(secret.to_string()).to_bytes()?;
        let totp = TOTP::new(Algorithm::SHA1, 6, 1, 30, bytes)?;
        Ok(Self { inner: totp })
    }

    pub fn current(&self) -> String {
        self.inner.generate_current().unwrap()
    }

    pub fn seconds_remaining(&self) -> u64 {
        30 - (SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs() % 30)
    }
}
```

**算法固定**：HMAC-SHA1, 30s, 6 位, ±1 步漂移（totp-rs 默认 skew=1）。

**输入格式**：cipher 的 `data.totp` 存 Base32 secret（如 `JBSWY3DPEHPK3PXP`），不存完整 `otpauth://` URL。导入 Bitwarden JSON 时提取 secret 部分。

**调用时机**：
- Auto-Type 完密码后：生成 6 位码 → 复制到剪贴板（30s 清空）→ toast 提示
- VaultPanel cipher 详情：实时显示 6 位码 + 倒计时圆环 + 复制按钮

### 5.2 密码生成器（跨场景复用主体 + Modal/独立窗口外壳）

**架构**（2026-07-19 重构，commit 见 plan 修订段）：生成器拆为**共享主体组件** + **场景外壳**两层：

```
┌─────────────────────────────────────────┐
│ <PasswordGenerator> 共享主体             │ ← 纯内容：Segmented 模式 + 显示区 + 强度条
│   props: onUsePassword?(pwd)            │   + 模式专属配置 + 操作栏
│         onAutotype?(pwd)  future        │   操作按钮按 props 动态显示
└─────────────────────────────────────────┘
        ▲                            ▲
        │                            │
┌───────┴────────┐          ┌────────┴────────┐
│ 外壳 A：Modal   │          │ 外壳 B：独立窗口  │ future
│ CipherEditor   │          │ Actionbar 触发   │
│ ✅ 已落地       │          │ P2 待办          │
└────────────────┘          └─────────────────┘
```

**已落地（外壳 A）**：`CipherEditor` 密码字段右侧 🔑 按钮 → 弹 `<PasswordGeneratorModal>` 半透明遮罩 + 居中卡片。点「使用此密码」写回 password 字段并关闭。

**未来扩展（外壳 B）**：`PasswordGenerator` 已预留 `onAutotype` prop——未来 Actionbar 加按钮触发独立 Tauri 窗口（参考 `compact_editor_window.rs` 模板）时，窗口 root 直接渲染主体组件，`onAutotype` 触发 `vault_autotype_password` 命令（与现有 vault autotype 机制复用）。无需重构主体。

**架构演进史**（避免下次重构踩同样坑）：
- 初版：`password_generator_window` 独立 Tauri 窗口 + `Cmd+Shift+G` 全局热键 + `pages/PasswordGenerator/`
- 2026-07-18：删除独立窗口，改为 CipherEditor 内嵌抽屉（`ecca9b04`）——为简化首发版
- 2026-07-19：再重构为「共享主体 + Modal/独立窗口外壳」——主体可复用，外壳按场景换，未来 Actionbar 集成无需重复实现

前端配置代码 `buildConfig.ts` 在 `pages/Settings/Vault/`，输入由前端 clamp 到合法范围。

#### 5.2.1 配置（4 种模式）

```rust
// crates/vault/src/generator/mod.rs

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "camelCase")]   // 注意：camelCase（不是 lowercase）
pub enum GeneratorConfig {
    Random(RandomConfig),            // 随机字符（默认）
    PassphraseEn(PassphraseEnConfig), // 英文 passphrase
    PassphraseZh(PassphraseZhConfig), // 中文 passphrase
    Pin(PinConfig),                  // 纯数字 PIN
}

pub struct RandomConfig {
    pub length: u32,           // 默认 16，范围 5-128（ensure!）
    pub uppercase: bool,       // 默认 true
    pub lowercase: bool,       // 默认 true
    pub numbers: bool,         // 默认 true
    pub symbols: bool,         // 默认 false
    pub avoid_ambiguous: bool, // 默认 true
}

pub struct PassphraseEnConfig {
    pub word_count: u32,       // 默认 3，范围 3-10（ensure!）
    pub separator: String,     // 默认 "-"
    pub capitalize: bool,      // 默认 true
    pub include_number: bool,  // 默认 true
}

pub struct PassphraseZhConfig {
    pub word_count: u32,       // 默认 4，范围 3-8（ensure!）
    pub separator: String,     // 默认 ""（中文无需分隔）
    pub include_number: bool,  // 默认 true
    pub include_symbol: bool,  // 默认 false
}

pub struct PinConfig {
    pub length: u32,  // 默认 6，范围 1-32（ensure!）
}
```

**默认模式**：根据用户 locale 选——`zh-CN` 默认 `PassphraseZh`，其他默认 `Random`。

#### 5.2.2 函数签名（全部返回 `Result<String>`，永不 panic）

follow-up #D 修订：所有 5 个生成函数（`generate` dispatch + 4 个具体模式）签名统一为
`fn generate(cfg: &XxxConfig) -> anyhow::Result<String>`。**用 `ensure!` 替代 `assert!`**
做输入校验——`assert!` 在 release 仍是 panic，会导致 Tauri 主进程崩溃（vault 整体不可用）。
前端 `buildConfig.ts` 同步 clamp 输入到合法范围。

```rust
// crates/vault/src/generator/mod.rs
pub fn generate(cfg: &GeneratorConfig) -> Result<String> { /* dispatch */ }

// crates/vault/src/generator/random.rs
pub fn generate(cfg: &RandomConfig) -> Result<String> {
    ensure!(cfg.length >= 5, "随机密码长度必须 ≥ 5（当前 {}）", cfg.length);
    ensure!(cfg.length <= 128, "随机密码长度必须 ≤ 128（当前 {}）", cfg.length);
    // ... build charset + balanced fill + shuffle ...
}

// passphrase_en.rs: ensure!(word_count in 3..=10)
// passphrase_zh.rs: ensure!(word_count in 3..=8)
// pin.rs:           ensure!(length in 1..=32)
```

#### 5.2.3 Random 模式（保证字符类型至少 1 次）

```rust
fn generate_random_balanced(cfg: &RandomConfig) -> Result<String> {
    use rand::rngs::OsRng;
    use rand::seq::SliceRandom;

    let mut result: Vec<char> = Vec::with_capacity(cfg.length as usize);
    let mut rng = OsRng;

    if cfg.uppercase { result.push(UPPER.choose(&mut rng).copied().unwrap()); }
    if cfg.lowercase { result.push(LOWER.choose(&mut rng).copied().unwrap()); }
    if cfg.numbers   { result.push(DIGITS.choose(&mut rng).copied().unwrap()); }
    if cfg.symbols   { result.push(SYMBOLS.choose(&mut rng).copied().unwrap()); }

    let charset = build_charset(cfg);
    while (result.len() as u32) < cfg.length {
        result.push(charset.chars().choose(&mut rng).unwrap());
    }

    result.shuffle(&mut rng);
    result.into_iter().collect()
}
```

#### 5.2.3 Passphrase EN（EFF 词表）

EFF 大词表 7776 词，3 词 ≈ 39 bit，6 词 ≈ 78 bit。

```rust
include!("eff_wordlist.rs");  // 编译时内嵌

pub fn generate_en(cfg: &PassphraseEnConfig) -> String {
    let mut rng = OsRng;
    let words: Vec<String> = (0..cfg.word_count)
        .map(|_| EFF_WORDLIST.choose(&mut rng).unwrap().to_string())
        .map(|w| if cfg.capitalize { capitalize_first(&w) } else { w })
        .collect();
    let mut result = words.join(&cfg.separator);
    if cfg.include_number {
        let n: u32 = OsRng.gen_range(0..=9);
        result = format!("{}{}", result, n);
    }
    result
}
```

#### 5.2.4 Passphrase ZH（4096 双字词）

**词表来源**（follow-up #H 修订）：
- 从 **jieba-rs 词典 `src/data/dict.txt`**（Apache-2.0）取，按 jieba 内置词频降序
- 过滤：专名（pos: `nr`/`ns`/`nt`/`nz`/`nrt`/`nrfg`）、单字、≥3 字词、不雅/敏感词、生僻字、易混字、数字/外文词
- 取剩余 TOP 4096（按词频降序）
- 抽样人工 review 5%（约 200 词）

> 原计划用 THUOCL，实施时改用 jieba——jieba-rs 是 Rust 生态原生词典，许可证更宽松
> （Apache-2.0 vs THUOCL MIT），且 jieba 词频已经过中文 NLP 社区长期验证。

**4096 = 2^12**，4 词 = 48 bit，5 词 = 60 bit。

```rust
include!("zh_wordlist_4096.rs");

pub fn generate(cfg: &PassphraseZhConfig) -> Result<String> {
    use rand::rngs::OsRng;
    use rand::seq::SliceRandom;

    ensure!(cfg.word_count >= 3, "中文 passphrase 词数必须 ≥ 3（当前 {}）", cfg.word_count);
    ensure!(cfg.word_count <= 8, "中文 passphrase 词数必须 ≤ 8（当前 {}）", cfg.word_count);

    let mut rng = OsRng;
    let words: Vec<&str> = (0..cfg.word_count)
        .map(|_| ZH_WORDLIST_4096.choose(&mut rng).unwrap())
        .collect();
    let mut result = words.join(&cfg.separator);

    if cfg.include_number {
        let n: u32 = OsRng.gen_range(0..=9);
        result = format!("{}{}", result, n);
    }
    if cfg.include_symbol {
        let s = ['!', '@', '#', '$', '%', '&', '*'].choose(&mut rng).unwrap();
        result = format!("{}{}", result, s);
    }
    Ok(result)
}
```

**强度对照表**：

| word_count | 熵 (bit) | 示例 |
|---|---|---|
| 3 | 36 + 数字 = 39 | `明月归途9` |
| 4 (默认) | 48 + 数字 = 51 | `明月归途春日9` |
| 5 | 60 + 数字 = 63 | `明月归途春日远方9` |
| 6 | 72 + 数字 = 75 | `明月归途春日远方故人9` |

#### 5.2.5 历史不存储

**用户决策**：完全不存历史。生成器 UI 只保留：
- 当前生成的密码（大字号显示）
- 配置区（模式 + 各参数）
- "重新生成" / "复制" / "填充到当前 cipher 字段" 按钮
- 强度指示器（实时）

用户偶尔需要回看？通过 cipher 的 password_history（user_vault_key 加密）。

#### 5.2.6 生成器内部强度评估（前端独立算法）

生成器 modal 内的强度条用**前端独立算法**（`estimateStrengthByConfig`），不走后端
`vault_evaluate_password`（zxcvbn）。原因：生成器知道自己用什么 mode + config，按参数
算熵比从字符串猜准确得多。

```
random       length × log2(charset_size)
             charset = uppercase(26) + lowercase(26) + numbers(10) + symbols(~32)
passphraseEn word_count × log2(7776)   // EFF 大词表
passphraseZh word_count × log2(4096)   // jieba 双字词词频 TOP
pin          length × log2(10)
```

评分映射（0-4）：< 28 bit 极弱 / 28-35 弱 / 36-59 一般 / 60-127 强 / ≥ 128 极强。

**写回 cipher 后**：`CipherEditor` 的 `PasswordStrengthBar` 会再走后端 `vault_evaluate_password`
做精确评估——双层评估，生成时即时反馈 + 持久化前精确测量。

> **踩坑警示**：曾经用「字符种类 + 字符长度」启发式算法（ASCII 思维），对中文短语结构性
> 歧视——把中文当 1 类字符 + 4 个双字词只算 8 字符 → 永远评 1 分（极弱），实际熵 48 bit
> 比英文 3 词（39 bit）还高。修复见 commit（plan 修订段记录）。

### 5.3 密码健康检查（本地）

#### 5.3.1 强度评估（zxcvbn）

```rust
// crates/vault/src/health/strength.rs

pub struct PasswordStrength {
    pub score: u8,            // 0-4（zxcvbn 评分）
    pub entropy_bits: f64,
    pub crack_time: String,
    pub warning: Option<&'static str>,
    pub suggestions: Vec<&'static str>,
}

pub fn evaluate(password: &str) -> PasswordStrength {
    let est = zxcvbn::zxcvbn(password, &[]).unwrap();
    PasswordStrength {
        score: est.score().value(),
        entropy_bits: est.guesses_log10() as f64 * 3.32,
        crack_time: format!("{}", est.crack_times().offline_slow_hashing_1e4_per_second()),
        warning: est.feedback().warning().map(|s| s),
        suggestions: est.feedback().suggestions().iter().map(|s| *s).collect(),
    }
}
```

评分：0=极弱 / 1=弱 / 2=一般 / 3=强 / 4=极强。MVP 阈值：score < 3 算弱。

#### 5.3.2 重复密码检测

```rust
pub fn find_duplicates(ciphers: &[Cipher]) -> Vec<DuplicateGroup> {
    let mut map: HashMap<String, Vec<i64>> = HashMap::new();
    for c in ciphers {
        if let CipherData::Login(login) = &c.data {
            if let Some(pwd) = &login.password {
                let hash = sha256_hex(pwd);  // 内存计算，不持久化
                map.entry(hash).or_default().push(c.id);
            }
        }
    }
    map.into_iter()
        .filter(|(_, ids)| ids.len() > 1)
        .map(|(hash, ids)| DuplicateGroup { password_hash: hash, cipher_ids: ids })
        .collect()
}
```

**关键**：SHA-256 hash 不持久化到 DB（避免泄露密码指纹），每次扫描内存计算。

#### 5.3.3 健康报告

```rust
pub struct HealthReport {
    pub weak_count: usize,
    pub duplicate_groups: usize,
    pub duplicate_cipher_count: usize,
    pub total_logins: usize,
    pub average_score: f64,
}

pub fn generate_report(ciphers: &[Cipher]) -> HealthReport { ... }
```

UI 展示：弱密码列表、重复密码组、平均强度评分。点击任一 cipher 跳到编辑页，可一键用生成器换密码。

#### 5.3.4 不做 HIBP 查询（MVP）

理由：需要联网（octopus 单机为主）+ 用户对"密码哈希上传"敏感 + 实现成本高。留 P2。

### 5.4 不变量（生成器 + 健康检查）

| # | 不变量 |
|---|---|
| INV-G1 | 密码生成器所有随机性必须来自 `OsRng`（CSPRNG） |
| INV-G2 | Random 模式必须保证每种启用字符类型至少出现 1 次 |
| INV-G3 | Passphrase EN 必须用 EFF 大词表（7776 词） |
| INV-G4 | Passphrase ZH 必须用 jieba 词频 TOP 4096 双字词 |
| INV-G5 | 不存生成器历史 |
| INV-G6 | TOTP 算法固定 HMAC-SHA1, 30s, 6 位, ±1 步漂移 |
| INV-G7 | 健康检查的 SHA-256 hash 不持久化到 DB |
| INV-G8 | **生成器所有 5 个公开函数返回 `Result<String>`，永不 panic**（输入校验用 `ensure!` 而非 `assert!`）—— panic 会崩 Tauri 主进程，使整个 vault 不可用 |

---

## 6. Bitwarden 导入 + 同步

### 6.1 导入格式

MVP 仅支持 **Bitwarden unencrypted JSON**：

```json
{
  "encrypted": false,
  "version": 2,
  "items": [
    {
      "id": "uuid-string",
      "name": "GitHub",
      "notes": "personal account",
      "favorite": false,
      "type": 1,
      "fields": [{ "name": "backup code", "value": "12345", "type": 0 }],
      "login": {
        "username": "user@example.com",
        "password": "p@ssw0rd",
        "totp": "JBSWY3DPEHPK3PXP",
        "uris": [{ "uri": "https://github.com", "match": null }]
      }
    }
  ]
}
```

```rust
// crates/vault/src/importer/bitwarden.rs

pub fn import_bitwarden_json(
    json: &str,
    user_vault_key: &DerivedKey,
    store: &mut VaultStore,
) -> Result<ImportReport> {
    let export: BitwardenExport = serde_json::from_str(json)?;
    if export.encrypted {
        return Err(VaultError::UnsupportedImportFormat);
    }
    let mut imported = 0;
    let mut skipped = 0;
    for item in export.items {
        if item.type != 1 { skipped += 1; continue; }  // 仅 Login
        let cipher = convert_bitwarden_item(item, user_vault_key)?;
        store.insert_cipher(cipher)?;
        imported += 1;
    }
    Ok(ImportReport { imported, skipped, total: export.items.len() })
}
```

**去重**：按 name + 第一条 uri 去重（Bitwarden 的 id 在 octopus 无意义）。

### 6.2 导出

```rust
pub fn export_vault_json(ciphers: &[Cipher]) -> Result<String> {
    let items = ciphers.iter()
        .filter(|c| c.deleted_at.is_none())
        .map(convert_to_bitwarden)
        .collect();
    Ok(serde_json::to_string_pretty(&BitwardenExport {
        encrypted: false,
        version: 2,
        items,
        folders: vec![],
    })?)
}
```

**UI 提示**："导出文件包含所有密码的明文，请妥善保管。"

### 6.3 同步策略

#### MVP 不做云同步

理由：用户单机使用为主 + 云同步工程量大 + spec D3 决策已确认（双层 + Y+ 方案）只为"未来支持同步"做了**密钥层**准备，不实现传输层。

#### 手动导出/导入作为同步替代

```
机器 A：VaultPanel → 导出 → backup.json（明文，用户保管）
机器 B：初始化 vault（设新主密码）→ 导入 backup.json
       → 所有 cipher 解密后用 B 的 user_vault_key 重新加密入库
```

注意：手动同步下 A 和 B 用**不同的** master_password（各自 vault_meta.kdf_salt 不同），但都能解开 backup.json。

#### 未来云同步（P2/P3，预留接口）

```rust
pub trait VaultSync {
    fn push(&self, encrypted_db: &[u8]) -> Result<SyncMetadata>;
    fn pull(&self) -> Result<Vec<u8>>;
    fn resolve_conflict(&self, local: &[u8], remote: &[u8]) -> Result<Vec<u8>>;
}

// 未来实现：
// - SyncProviderWebDAV
// - SyncProviderS3
// - SyncProviderSelfHosted (octopus-server 加 endpoint)
```

### 6.4 不变量（导入同步）

| # | 不变量 |
|---|---|
| INV-I1 | Bitwarden 导入仅支持 unencrypted JSON（MVP） |
| INV-I2 | 导入失败时已成功的 cipher 不回滚（中断容忍） |
| INV-I3 | 导入按 name + 第一条 uri 去重 |
| INV-I4 | 导出明文 JSON 必须明确警告用户 |
| INV-I5 | 手动同步不引入任何网络请求（纯本地文件读写） |

---

## 7. 降级路径 + 错误处理

### 7.1 失败场景与降级

| # | 场景 | 降级 | UX |
|---|---|---|---|
| F1 | vault 未初始化 | VaultPanel 展示 Setup 页 | 引导向导 |
| F2 | machine-key.enc 不可读（权限 / 文件丢失 / file_key 不匹配） | 弹主密码输入，走流程 C（用 app_key_sync_enc 解） | 警告 + 弹密码框 |
| F3 | K_machine 解 app_key_local_enc 失败（DB 损坏/被篡改） | 弹主密码输入，用 app_key_sync_enc 解 | "启动数据异常，请输主密码" |
| F4 | 用户输错主密码 | 计数 + 指数退避（1s/2s/4s/8s/16s/30s） | 错误提示 + 倒计时 |
| F5 | 主密码彻底遗忘 | 无降级——vault 不可恢复 | 警告 + 引导"导出 emergency kit" |
| F6 | vault 锁定超时（默认 3min，可配 30s~15min/Never） | user_vault_key zeroize | 自动锁定 toast |
| F7 | URL 检测失败 | 弹手动选择 cipher 浮窗 | "无法检测当前页面，请手动选择" |
| F8 | URL 匹配 0 个 cipher | 浮窗显示"无匹配"+ 全 cipher 列表 | 让用户搜索选择 |
| F9 | URL 匹配 N 个 cipher | 弹选择浮窗，按最近使用排序（上限 follow-up #8） | 用户点选 |
| F10 | Auto-Type 时焦点丢失 | 中止操作 + toast 警告 | "目标窗口已切换，已取消填充" |
| F11 | Auto-Type 失败（enigo 报错/权限拒绝） | fallback 到复制密码到剪贴板（30s 清空） | "键盘模拟失败，已复制到剪贴板" |
| F12 | 剪贴板写入失败 | toast 报错 | "复制失败" |
| F13 | TOTP secret 格式错误 | cipher 详情页红色错误标记 | "TOTP 格式无效" |
| F14 | 导入 JSON 格式错误 | 报错 + 行号 | "第 12 行解析失败" |
| F15 | 导入 cipher 数过多（>1000） | 进度条 + 后台异步 | 防 UI 卡死 |
| F16 | DB 写入失败（磁盘满/权限） | 回滚 + toast | "保存失败" |
| F17 | 改主密码时输错旧密码 | 拒绝 + 计数（同 F4） | 错误提示 |
| F18 | schema 升级失败（v37→v38） | 回滚 user_version + 报错 | "数据库升级失败" |
| F19 | 主密码强度不达标（< 12 字符或缺字符类） | 前端实时校验阻止提交 + 后端再校验 | 红字提示具体原因 |
| F20 | vault_meta 表损坏/不存在 | 引导重新初始化或导入备份 | Setup 页 |
| F21 | vault feature 编译关掉 | 前端 `is_vault_enabled()` 返回 false → 整段 vault UI 隐藏；`try_decrypt_secret_global` 退化为 `Ok(raw)` 返回（旧 DB 无 v1: 加密格式） | 无 vault UI |
| F22 | vault 启用但解密失败（app_key 不可用 / 密文损坏） | `try_decrypt_secret_global` 返 `Err`（2026-07-19 修复 #5）；4 个调用点 fail-soft：云端推理跳过 + 提示"请先解锁保险库"，**不再把密文当 Bearer 发到云端** | 提示 + 跳过本次推理 |

### 7.2 错误类型（双轨制）

错误处理分两层：

**vault crate 内部**：继续用 `anyhow::Result`（与 octopus 其他 crate 风格一致，**不用 thiserror**）。
错误链通过 anyhow context 串起来，含文件路径 / SQL 片段 / 加密细节——**仅供日志**。

**Tauri 命令边界**（`crates/desktop/src/vault_error.rs`，follow-up #9）：用 `VaultError` enum
翻译为用户安全消息。命令签名仍是 `Result<T, String>`，但 String 是 JSON `{code, message}`：

```rust
// crates/desktop/src/vault_error.rs
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VaultError {
    NotInitialized,              // vault 尚未初始化
    Locked,                      // 已初始化但未解锁
    InvalidMasterPassword,       // 主密码错误
    RepromptRequired,            // reprompt 保护的高敏感 cipher 操作未提供 master_password（2026-07-19 #3）
    CipherNotFound(i64),         // 指定 id 不存在
    InvalidInput(String),        // 通用用户输入错误
    TotpInvalidSecret,           // TOTP secret 非 Base32
    ImportFailed(String),        // 导入失败（仅用户安全上下文）
    KeychainUnavailable,         // 系统密钥串 / machine-key 文件不可用
    ClipboardError,              // 剪贴板写入失败
    AutoTypeFailed,              // Auto-Type 失败（已 fallback）
    InternalError,               // 兜底——**绝不**包含内部细节
}

impl VaultError {
    pub fn user_message(&self) -> &'static str;   // 用户安全消息（无路径/SQL/算法细节）
    pub fn code(&self) -> &'static str;            // 稳定错误码（snake_case）
}

/// 启发式分类 anyhow 错误链 → VaultError（无法分类的兜底 InternalError）
pub fn classify(err: &anyhow::Error) -> VaultError;

/// 序列化为 JSON `{"code": "...", "message": "..."}` 给前端
pub fn serialize(err: &VaultError) -> String;
```

**前端契约**（`pages/Settings/Vault/classifyError.ts`）：先 JSON parse（新格式），
失败则退回旧的字符串匹配（向后兼容）。前端按 `code` 程序化处理（如 `locked` → 弹解锁框）。

### 7.3 主密码错误防护

```rust
pub struct UnlockAttemptGuard {
    failures: AtomicU32,
    next_allowed_at: AtomicU64,
}

impl UnlockAttemptGuard {
    pub fn record_failure(&self) -> Duration {
        let n = self.failures.fetch_add(1, Ordering::SeqCst) + 1;
        let delay_secs = match n {
            1 => 0, 2 => 1, 3 => 2, 4 => 4, 5 => 8, 6 => 16, _ => 30,
        };
        self.next_allowed_at.store(now_unix() + delay_secs as u64, Ordering::SeqCst);
        Duration::from_secs(delay_secs as u64)
    }
    // ... check_allowed, reset
}
```

不引入"锁定 N 小时"机制——单机工具，指数退避足够。

### 7.4 主密码强度校验（前后端双校验）

`pages/Settings/Vault/validateMasterPassword.ts`：

```typescript
export const MIN_MASTER_PASSWORD_LENGTH = 12;

export interface MasterPasswordValidation {
  ok: boolean;
  reason?: string;     // 具体原因（"少于 12 字符" / "缺少大写字母" 等）
}

export function validateMasterPassword(pwd: string): MasterPasswordValidation {
  if (pwd.length < MIN_MASTER_PASSWORD_LENGTH) return { ok: false, reason: "..." };
  const hasUpper = /[A-Z]/.test(pwd);
  const hasLower = /[a-z]/.test(pwd);
  const hasDigit = /[0-9]/.test(pwd);
  const hasSymbol = /[^A-Za-z0-9]/.test(pwd);
  const classes = [hasUpper, hasLower, hasDigit, hasSymbol].filter(Boolean).length;
  if (classes < 4) return { ok: false, reason: "..." };
  return { ok: true };
}
```

后端 `vault_setup` / `vault_change_password` 同样校验，防前端绕过。

### 7.5 不变量汇总（全局）

见 2.8 / 3.8 / 4.7 / 5.4 / 6.4 各节。新增 INV-9 / INV-10（§2.8）、INV-D6 / INV-D7（§3.8）、
INV-G8（§5.4）。

### 7.6 关键测试场景

```rust
#[test]
fn test_kdf_round_trip() { /* Argon2id 同输入 → 同输出 */ }

#[test]
fn test_child_key_deterministic() { /* 同 master_root_key + 同 label → 同子 key */ }

#[test]
fn test_child_key_different_labels() { /* 不同 label 必产生不同 key */ }

#[test]
fn test_aes_gcm_round_trip() { /* encrypt → decrypt 还原 */ }

#[test]
fn test_decrypt_with_wrong_key_fails() { /* 错 key 必失败 */ }

#[test]
fn test_nonce_uniqueness() { /* 同 key 同明文 → 不同密文 */ }

#[test]
fn test_domain_match() {
    // github.com 匹配 github.com 和 gist.github.com
    // 不匹配 github.io（不同 eTLD+1）
}

#[test]
fn test_etld_plus_one() {
    // mail.google.com → google.com
    // foo.bar.example.co.uk → example.co.uk
    // localhost → localhost
}

#[test]
fn test_equivalent_domains() {
    // google.com 的 cipher 应匹配 youtube.com（等价域名）
}

#[test]
fn test_password_strength_zxcvbn() {
    // "password" → score 0
    // "Tr0ub4dour&3" → score 2
    // "correcthorsebatterystaple" → score 4
}

#[test]
fn test_passphrase_zh_entropy() {
    // 4 词中文 passphrase 熵 ≈ 48 bit
}

#[test]
fn test_totp_format() {
    // 输出 6 位数字字符串
}
```

### 7.7 实施风险评估

| 风险 | 概率 | 影响 | 缓解 |
|---|---|---|---|
| AppleScript URL 检测在 Chrome 上失效 | 中 | 中 | 测多版本；fallback 手动选 |
| enigo 在 macOS 输入密码字段异常 | 低 | 高 | 提前 e2e 测试 |
| Argon2id m=64MiB 在低端机器慢 | 低 | 中 | 参数可调 |
| machine-key.enc 文件被删 / 拷贝到他机 | 中 | 低 | file_key 与 machine_id + USER 绑定，他机解不开；本机重丢走流程 C |
| 用户主密码遗忘 | 中 | 极高 | 引导 emergency kit；无技术解决 |
| octopus clipboard 监听器漏接 concealed | 中 | 高 | 集成测试覆盖 |
| Bitwarden JSON 格式变动 | 低 | 低 | 严格 schema 校验 |
| schema v38 升级中途失败 | 低 | 中 | 单事务 + user_version 最后一步升 |
| panic 在生成器 / 命令路径使 vault 崩 | 中 | 高 | follow-up #D：生成器全 `Result`；Tauri 命令 catch_unwind 兜底 |

---

## 附录 A：Tauri 命令清单

```rust
// crates/desktop/src/vault_commands.rs

#[tauri::command]
pub fn vault_status(state: State<AppState>) -> Result<VaultStatus, String>;
// → { initialized: bool, locked: bool, cipher_count: usize }

#[tauri::command]
pub fn vault_setup(state: State<AppState>, password: String) -> Result<(), String>;
// 首次初始化：设主密码 + 迁移 models.secret_key

#[tauri::command]
pub fn vault_unlock(state: State<AppState>, password: String) -> Result<(), String>;
// 解锁 user_vault_key

#[tauri::command]
pub fn vault_lock(state: State<AppState>) -> Result<(), String>;
// 主动锁定

#[tauri::command]
pub fn vault_change_password(
    state: State<AppState>,
    old_password: String,
    new_password: String,
) -> Result<(), String>;

#[tauri::command]
pub fn vault_list_ciphers(state: State<AppState>) -> Result<Vec<CipherDto>, String>;
// 返回解密后的 cipher 列表（仅元数据，不含 password 明文）

#[tauri::command]
pub fn vault_get_cipher(state: State<AppState>, id: i64) -> Result<CipherDto, String>;
// 返回完整 cipher（含 password）

#[tauri::command]
pub fn vault_create_cipher(state: State<AppState>, input: CipherInput) -> Result<i64, String>;

#[tauri::command]
pub fn vault_update_cipher(state: State<AppState>, id: i64, input: CipherInput) -> Result<(), String>;

#[tauri::command]
pub fn vault_delete_cipher(state: State<AppState>, id: i64, permanent: bool) -> Result<(), String>;
// permanent=false → 软删除（回收站）；permanent=true → 物理 DELETE

#[tauri::command]
pub fn vault_generate(state: State<AppState>, cfg: GeneratorConfig) -> Result<String, String>;

#[tauri::command]
pub fn vault_generate_totp(state: State<AppState>, cipher_id: i64) -> Result<TotpResult, String>;
// { code: "123456", seconds_remaining: 18 }

#[tauri::command]
pub fn vault_autotype(state: State<AppState>, cipher_id: i64) -> Result<AutoTypeResult, String>;
// 触发 Auto-Type 完整流程

#[tauri::command]
pub fn vault_autotype_detect_and_match(state: State<AppState>) -> Result<Vec<CipherDto>, String>;
// AppleScript 检测 URL + 返回匹配 cipher 列表

#[tauri::command]
pub fn vault_health_report(state: State<AppState>) -> Result<HealthReport, String>;

#[tauri::command]
pub fn vault_import_bitwarden(state: State<AppState>, json: String) -> Result<ImportReport, String>;

#[tauri::command]
pub fn vault_export(state: State<AppState>) -> Result<String, String>;
// 返回明文 JSON 字符串，前端触发下载

#[tauri::command]
pub fn vault_test_url_detect() -> Result<Option<String>, String>;
// 测试用：触发 macOS 权限授权框

// === follow-up 新增命令 ===

#[tauri::command]
pub fn vault_heartbeat(state: State<AppState>) -> Result<(), String>;
// 前端保险库 tab 每 30s 调一次，刷新 last_active_at 防超时锁

#[tauri::command]
pub fn vault_get_lock_timeout(config: State<SharedRuntimeConfig>) -> u64;
// 读 AppConfig.vault_lock_timeout_secs

#[tauri::command]
pub fn vault_set_lock_timeout(config: State<SharedRuntimeConfig>, secs: u64) -> Result<(), String>;
// 写 AppConfig.vault_lock_timeout_secs（0 = 永不锁定）

#[tauri::command]
pub fn vault_list_folders(state: State<AppState>) -> Result<Vec<FolderDto>, String>;
#[tauri::command]
pub fn vault_create_folder(state: State<AppState>, name: String) -> Result<i64, String>;
#[tauri::command]
pub fn vault_rename_folder(state: State<AppState>, id: i64, name: String) -> Result<(), String>;
#[tauri::command]
pub fn vault_delete_folder(state: State<AppState>, id: i64) -> Result<(), String>;
// folder name 经 user_vault_key 加密；删除时 cipher FK ON DELETE SET NULL 回到根目录

#[tauri::command]
pub fn vault_detect_and_match(state: State<AppState>) -> Result<Vec<CipherDto>, String>;
// vault_autotype_detect_and_match 的实现名（含 fallback 上限 follow-up #8）

#[tauri::command]
pub fn feature_flags::is_vault_enabled() -> bool;
// 前端运行时探针：feature 关掉时整段 vault UI 不渲染
```

> **错误格式**：所有命令的 `Err(String)` 中 String 是 `VaultError` 序列化的 JSON
> `{"code": "...", "message": "..."}`（详见 §7.2）。前端 `classifyError.ts` 解析。

## 附录 B：前端组件清单

```
crates/desktop/frontend/src/pages/Settings/Vault/
├── VaultPanel.tsx              # 主面板（顶部 PillTabs 切 list/health/io + lock timeout 设置）
├── CipherList.tsx              # cipher 列表
├── CipherEditor.tsx            # 新建/编辑 cipher 表单（密码字段右侧眼睛/生成/复制 3 按钮 + grid 两列布局）
├── PasswordGenerator.tsx       # 生成器共享主体（Segmented 模式 + 显示 + 强度 + 配置 + 操作栏，跨场景复用）
├── PasswordGeneratorModal.tsx  # 生成器 Modal 外壳（外壳 A，CipherEditor 场景）
├── UnlockDialog.tsx            # 解锁弹窗
├── SetupWizard.tsx             # 首次初始化向导（含 12 位 + 4 类校验）
├── HealthReport.tsx            # 健康报告
├── ImportExport.tsx            # 导入导出
├── FolderSidebar.tsx           # folder 侧边栏（All / Favorites / Folders / Trash）
├── FolderPromptDialog.tsx      # 新建/重命名 folder 弹窗
├── buildConfig.ts              # 生成器配置（前端 clamp 输入到合法范围）
├── buildConfig.test.ts
├── validateMasterPassword.ts   # 主密码强度校验（12 位 + 4 类）
├── validateMasterPassword.test.ts
├── folderTypes.ts
└── classifyError.ts            # VaultError JSON 解析（向后兼容旧字符串）
```

> **架构演进**：`pages/PasswordGenerator/` 独立浮窗（首发版）→ CipherEditor 内嵌抽屉
> （`ecca9b04`）→ 共享主体 + Modal 外壳（2026-07-19 重构，主体复用，未来 Actionbar 独立
> 窗口场景也能用）。
> **`pages/QuickAccess/` vault tab 仍未实现**（P2 待办）。

## 附录 C：能力配置（capabilities/default.json）

新增窗口 label：
- `vault_picker_window` — Auto-Type picker 浮窗（URL 检测 + 匹配 cipher 列表）

```json
{
  "permissions": ["core:default", "opener:default", "..."],
  "windows": ["main", "settings_window", "action_bar_window", "...", "vault_picker_window"]
}
```

> 原计划的 `password_generator_window` + `vault_setup_window` 已废弃——生成器当前走
> CipherEditor Modal（主体组件 `PasswordGenerator.tsx` 可复用），setup 走 VaultPanel
> 内联向导（不弹独立窗口）。**未来扩展**：Actionbar 触发的独立生成器窗口场景可能恢复
> `password_generator_window`，渲染同一个 `<PasswordGenerator>` 主体（详见 §5.2 架构图）。

## 附录 D：全局热键

| 热键 | 功能 | 备注 |
|---|---|---|
| `Cmd+Shift+L` | 触发 Auto-Type（检测 URL → 匹配 → 填充） | 默认 `AppConfig.vault_autotype_shortcut` |

热键注册沿用 `action_bar_window.rs` 的 `register_action_bar_shortcut` 机制，vault feature off 时整段跳过。

> **已废弃热键**：`Cmd+Shift+G`（生成器独立浮窗，已改为 CipherEditor Modal——主体组件
> `PasswordGenerator.tsx` 复用，未来 Actionbar 场景恢复独立窗口时不再需要全局热键）、
> `Cmd+Shift+V`（Quick Access vault tab，未实现）。`AppConfig.vault_generator_shortcut`
> 字段保留仅为兼容旧 DB，运行时不消费。

## 附录 E：Cargo feature gate（follow-up #10）

`crates/desktop/Cargo.toml`：

```toml
[features]
default = ["embedded", "vault"]
vault = ["dep:octopus-vault", "dep:url"]
```

**feature on（默认）**：完整 vault UI + 命令 + Auto-Type + 全局热键 `Cmd+Shift+L`。
**feature off**（`cargo build --no-default-features --features embedded`）：
- `vault_state` / `vault_commands` / `autotype` / `vault_error` 模块整体 `#[cfg(feature = "vault")]` 掉
- `octopus-vault` 依赖切掉（更小 binary、无密钥访问、无密码管理 UI）
- `AppState` 不注入 `SharedVaultSession`
- 全局热键不注册
- `vault_secret_access.rs` **总是编译**——它是云端推理热路径（AliyunEngine /
  LLM polish / 云端翻译）的统一 chokepoint。feature off 时 `try_decrypt_secret_global`
  退化为 raw 原样返回（没 vault 即不可能有 `v1:` 加密格式），4 个调用点无需 cfg gate。
- 前端通过 `feature_flags::is_vault_enabled() -> bool` Tauri 命令运行时探针
  （`Settings/index.tsx` + `App.tsx` 条件渲染 vault UI）。

设计理由：让 CI / 定制构建 / 未来拆分都能干净地关掉 vault 而不影响云端推理。

---

## 参考实现

- **vaultwarden**（本地 `/Users/wudarui/workspace/agent/vaultwarden`）：服务端零知识设计、TOTP 实现
- **bitwarden/clients**：浏览器 autofill、桌面 auto-type、加密分层、URL 匹配算法
- **1Password**：Secret Key 机制（参考但 MVP 不实现）、Quick Access 交互
- **rbw**：daemon 缓存解锁态心智模型（octopus 用 Tauri 主进程替代）
- **gopass**：EFF 词表思路
- **MetaMask V3 Keystore**：随机 32B salt、AES-GCM 简化
- **jieba-rs**：中文词频词典（Apache-2.0），ZH passphrase 词表来源

完整调研见 `docs/research/2026-07-18-password-vault-research.md`。

---

**设计完成并已同步实现**。下一步：plan 文档「Follow-up Work」节记录实施期变更。
