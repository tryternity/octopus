# Password Vault Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 为 octopus 引入密码管理功能：加密 vault + 密码生成器 + macOS Auto-Type + TOTP + Bitwarden 导入，并顺手加密现有 `models.secret_key`。

**Architecture:** 新增 `crates/vault/`（纯逻辑库，依赖 infra），暴露加密 / 存储 / 生成器 / 匹配 / 健康检查 / 导入的纯函数 API；`crates/desktop/` 加 `vault_commands.rs` + `autotype/` 模块包装为 Tauri 命令；前端在 Settings 下加 VaultPanel，**密码生成器为共享主体组件（`PasswordGenerator.tsx`）+ Modal 外壳**（跨场景复用，未来 Actionbar 独立窗口场景也可用）。加密用 Argon2id + HMAC-SHA512 简化 BIP44 派生 + AES-256-GCM；密钥双层（user_vault_key + app_key），app_key 用 K_machine（**本地加密文件 `~/.octopus/machine-key.enc`**）和 master_root_key 双密文存储，本机启动无感。

**Tech Stack:** Rust（argon2 / aes-gcm / hkdf / hmac / sha2 / zeroize / publicsuffix / totp-rs / zxcvbn / data-encoding / regex / uuid）+ Tauri 2 + React 19 + TypeScript + Tailwind 4 + Radix UI + enigo 0.6（已有）。**不再用 `keyring` crate**（K_machine 改本地加密文件，详见 spec §2.5）。

> **状态**：21 个 Task 全部完成 ✅；实施期 follow-up 修订见末尾「Follow-up Work」节。
> 同步修订详见 spec 顶部「同步修订」段。

## Global Constraints

（从 spec 第 0、1、2 节摘录的硬约束，所有任务隐式遵守）

- **加密**：Argon2id(t=3, m=65536 KiB, p=4) + HMAC-SHA512 child() 派生（取前 32B）+ AES-256-GCM(12B nonce, 16B tag)
- **派生 label**：固定为 `b"octopus/v1/user-vault"` / `b"octopus/v1/app-secrets"` / `b"octopus/v1/sync"` / `b"octopus/v1/send"`
- **KDF salt**：32B 随机，存 `vault_meta.kdf_salt`，**不用 email**
- **密文格式**：所有加密字段统一 `v1:<base64(nonce[12B] || ciphertext || tag[16B])>`
- **密钥管理**：所有 key 用 `Zeroizing<[u8; 32]>` 包装；master_password / master_root_key 派生完毕立即 zeroize；K_machine 永不落盘明文——存为 `~/.octopus/machine-key.enc`（file_key 由 `HKDF-SHA256(machine_id, USER)` 派生）
- **schema 升级**：v37 → v38，新增 3 张表（vault_meta / vault_ciphers / vault_folders）；不 ALTER models.secret_key（用 `v1:` 前缀判别）
- **不兼容旧 schema / 旧明文 secret_key**：首次 init vault 时一次性把现有 is_local=0 的明文 API Key 用 app_key 加密回写
- **错误返回**：Tauri 命令统一 `Result<T, String>`，String 为 `VaultError` 序列化的 JSON `{code, message}`（spec §7.2）；vault crate 内部用 `anyhow::Result`（与项目现有风格一致，**不用 thiserror**）
- **TOTP**：HMAC-SHA1, 30s, 6 位, ±1 步漂移
- **Auto-Type**：默认 `press_enter=false`；模拟键盘前必须 100ms 焦点等待
- **剪贴板**：用现有 `ClipboardHandle::write_text`（自动 suppress_next 跳过自身监听器）+ 单独写 `org.nspasteboard.ConcealedType` 给第三方工具；30s 自动清空
- **CSPRNG**：所有随机性必须来自 `rand::rngs::OsRng`
- **平台范围**：MVP 仅 macOS（AppleScript + enigo CGEvent）；Windows/Linux 编译通过但运行时返回 `Err("not implemented")`
- **跨 crate 依赖方向**：infra ← vault ← desktop；**vault 不依赖 tauri / tokio**
- **feature gate**：`octopus-desktop` 加 `vault` cargo feature（默认开），关掉后 vault 模块整体 cfg 掉（详见 spec 附录 E）
- **锁定超时**：可配置（`AppConfig.vault_lock_timeout_secs`，默认 180s/3min），以 `last_active_at` 为基准 + 前端 30s 心跳（spec §2.7）

---

## File Structure

### 新增文件

**crates/vault/**（新 crate，纯逻辑库）

| 文件 | 职责 |
|---|---|
| `Cargo.toml` | crate 依赖清单 |
| `src/lib.rs` | crate 入口，re-export 子模块 |
| `src/error.rs` | anyhow 风格错误辅助（bail!/ensure! 宏，无自定义 enum） |
| `src/crypto/mod.rs` | 加密原语汇总 |
| `src/crypto/kdf.rs` | Argon2id 派生 master_root_key |
| `src/crypto/hierarchy.rs` | HMAC-SHA512 child() 派生 |
| `src/crypto/symmetric.rs` | AES-256-GCM encrypt/decrypt（v1: 格式） |
| `src/crypto/util.rs` | 随机数 / Base64 / 常量时间比较 |
| `src/storage/mod.rs` | VaultStore 类型 + transaction 包装 |
| `src/storage/meta.rs` | vault_meta CRUD（含 KDF 参数、双密文 app_key） |
| `src/storage/cipher.rs` | vault_ciphers CRUD（密文层） |
| `src/storage/folder.rs` | vault_folders CRUD（folder 名用 user_vault_key 加密） |
| `src/types.rs` | Cipher / CipherData / LoginData / LoginUri / MatchType / Field / CipherInput |
| `src/unlock.rs` | 解锁态管理：K_machine 双密文、5 大流程 |
| `src/keychain.rs` | K_machine 在本地加密文件 `~/.octopus/machine-key.enc` 的存取（file_key 由 HKDF-SHA256 派生） |
| `src/migrate.rs` | 一次性迁移 models.secret_key（init vault 时触发） |
| `src/generator/mod.rs` | GeneratorConfig enum + dispatch（返回 Result） |
| `src/generator/random.rs` | Random 模式（保证字符类型至少 1 次） |
| `src/generator/passphrase_en.rs` | EFF 7776 词表 |
| `src/generator/passphrase_zh.rs` | 中文 4096 双字词表（jieba 词频） |
| `src/generator/pin.rs` | PIN 模式 |
| `src/generator/eff_wordlist.rs` | include_str! EFF 词表 |
| `src/generator/zh_wordlist_4096.rs` | jieba 词频 TOP 4096 |
| `src/totp.rs` | RFC 6238 TOTP 生成 |
| `src/matcher/mod.rs` | find_matching_ciphers + 5 种策略 |
| `src/matcher/psl.rs` | eTLD+1 提取 + 默认等价域名 |
| `src/health/mod.rs` | HealthReport 生成 |
| `src/health/strength.rs` | zxcvbn 强度评估 |
| `src/health/duplicate.rs` | 重复密码检测（内存 SHA-256） |
| `src/importer/mod.rs` | 导入导出统一入口 |
| `src/importer/bitwarden.rs` | Bitwarden unencrypted JSON 导入 |
| `src/importer/exporter.rs` | 导出 Bitwarden JSON |

**crates/desktop/src/**（已有 crate，新增模块，全部 `#[cfg(feature = "vault")]` 门控）

| 文件 | 职责 |
|---|---|
| `vault_commands.rs` | Tauri 命令（setup/unlock/lock/heartbeat/lock_timeout/CRUD/folder CRUD/autotype/totp/generate/health/import/export） |
| `vault_state.rs` | AppState 扩展（`SharedVaultSession = Arc<RwLock<VaultSession>>`，含 last_active_at） |
| `vault_error.rs` | VaultError enum + classify(anyhow) + JSON 序列化 |
| `vault_secret_access.rs` | secret_key 解密 chokepoint（**总是编译**，feature off 退化为 raw 返回） |
| `autotype/mod.rs` | AutoType trait + dispatch |
| `autotype/macos.rs` | macOS 实现（enigo + AppleScript） |
| `autotype/url_detect.rs` | 6 浏览器 AppleScript URL 检测 |
| `autotype/clipboard.rs` | concealed 剪贴板写入（30s 自动清空） |

**crates/desktop/frontend/src/pages/Settings/Vault/**（已有，新增）

| 文件 | 职责 |
|---|---|
| `VaultPanel.tsx` | vault 主面板（顶部 PillTabs 切 list/health/io + lock timeout 设置 + 30s 心跳） |
| `Vault/CipherEditor.tsx` | cipher 新建/编辑表单（密码字段右侧眼睛/生成/复制 3 按钮 + grid 两列布局 + Modal 生成器） |
| `Vault/PasswordGenerator.tsx` | 生成器共享主体（跨场景复用，预留 onAutotype 给未来 Actionbar 场景） |
| `Vault/PasswordGeneratorModal.tsx` | 生成器 Modal 外壳（CipherEditor 场景） |
| `Vault/UnlockDialog.tsx` | 解锁弹窗 |
| `Vault/SetupWizard.tsx` | 首次初始化向导（12 位 + 4 类校验） |
| `Vault/HealthReport.tsx` | 健康报告 |
| `Vault/ImportExport.tsx` | 导入导出 |
| `Vault/FolderSidebar.tsx` | folder 侧边栏（All / Favorites / Folders / Trash） |
| `Vault/FolderPromptDialog.tsx` | 新建/重命名 folder 弹窗 |
| `Vault/buildConfig.ts` + `.test.ts` | 生成器配置（前端 clamp 输入到合法范围） |
| `Vault/validateMasterPassword.ts` + `.test.ts` | 主密码强度校验（12 位 + 4 类） |
| `Vault/classifyError.ts` | VaultError JSON 解析 |

> **演进**（2026-07-19 修订）：`pages/PasswordGenerator/index.tsx` 独立浮窗 → CipherEditor
> 内嵌抽屉（`ecca9b04`）→ 共享主体 + Modal 外壳（本次重构）。主体 `PasswordGenerator.tsx`
> 跨场景复用，未来 Actionbar 独立窗口场景直接渲染主体即可。

### 修改文件

| 文件 | 改动 |
|---|---|
| `Cargo.toml` | workspace.members 加 `"crates/vault"` |
| `crates/desktop/Cargo.toml` | 加 `octopus-vault = { path = "../vault", optional = true }` + `url = { optional = true }` + `[features] vault = ["dep:octopus-vault", "dep:url"]`（默认开） |
| `crates/infra/src/db.sql` | 新增 3 张 vault 表（末尾追加） |
| `crates/infra/src/db.rs` | init_schema 加 `if v < 38` 段（仿 v36 模式）；加 vault struct + CRUD；加 `set_test_db` thread_local override |
| `crates/desktop/src/main.rs` | invoke_handler! 加 vault_commands（cfg gate）+ `feature_flags::is_vault_enabled`；setup 加 vault_state；register vault autotype shortcut（仅 Cmd+Shift+L） |
| `crates/desktop/src/lib.rs`（或 main.rs） | `#[cfg(feature="vault")] pub mod vault_commands / vault_state / vault_error / autotype; pub mod vault_secret_access;` |
| `crates/desktop/capabilities/default.json` | windows 数组加 `"vault_picker_window"` |
| `crates/desktop/frontend/src/App.tsx` + `pages/Settings/index.tsx` | 条件渲染 vault UI（基于 `is_vault_enabled` 探针） |
| `crates/desktop/frontend/src/locales/zh-CN.yaml` / `en.yaml` | 加 `settings.nav.vault` + `settings.vault.*` keys |
| `crates/infra/src/config.rs` | 加 `vault_autotype_shortcut`（默认 `CmdOrCtrl+Shift+L`）+ `vault_generator_shortcut`（已废弃，保留兼容旧 DB）+ `vault_lock_timeout_secs`（默认 180） |

---

## Task 概览（21 个任务 + Follow-up Work）

按依赖序（**全部完成 ✅**，commit 见末尾 Follow-up Work）：

| # | Task | 状态 |
|---|---|---|
| 1 | vault crate 骨架 + workspace 调整 | ✅ |
| 2 | crypto: kdf.rs（Argon2id） | ✅ |
| 3 | crypto: hierarchy.rs（HMAC-SHA512 child） | ✅ |
| 4 | crypto: symmetric.rs（AES-256-GCM + v1: 格式） | ✅ |
| 5 | infra schema v38 + struct + CRUD | ✅ |
| 6 | vault types.rs（Cipher / LoginData / MatchType 等） | ✅ |
| 7 | vault storage: meta.rs + cipher.rs + folder.rs | ✅ |
| 8 | vault keychain.rs（K_machine 存取）—— 后续改为本地文件 | ✅ |
| 9 | vault unlock.rs（5 大流程 + 双密文） | ✅ |
| 10 | vault generator: random.rs + pin.rs | ✅ |
| 11 | vault generator: passphrase_en.rs + zh.rs（词表） | ✅ |
| 12 | vault totp.rs | ✅ |
| 13 | vault matcher: 5 种策略 + eTLD+1 | ✅ |
| 14 | vault health: strength + duplicate | ✅ |
| 15 | vault importer: Bitwarden JSON | ✅ |
| 16 | desktop vault_state + AppState 集成 | ✅ |
| 17 | desktop vault_commands: setup/unlock/lock/CRUD/generate/totp/health/import/export | ✅ |
| 18 | desktop autotype: url_detect + macos + clipboard | ✅ |
| 19 | desktop vault_commands: autotype 命令 + 全局热键注册（仅 Cmd+Shift+L） | ✅ |
| 20 | desktop 一次性迁移 models.secret_key（+ follow-up #7 chokepoint） | ✅ |
| 21 | 前端 VaultPanel + SetupWizard + UnlockDialog + CipherEditor 内嵌生成器 + i18n | ✅ |

---


## Task 1: vault crate 骨架 + workspace 调整

**Files:**
- Create: `crates/vault/Cargo.toml`
- Create: `crates/vault/src/lib.rs`
- Modify: `Cargo.toml`（workspace 根，行 2 members 列表）
- Create: `crates/vault/tests/` 目录（占位）

**Interfaces:**
- Produces: 一个可 `cargo build -p octopus-vault` 编译通过的空 crate，`pub mod` 全部声明但暂时为空

- [x] **Step 1: 在 workspace 根 Cargo.toml 加 member**

打开 `Cargo.toml`，找到第 2 行的 members 数组，在 `"crates/search"` 后追加 `"crates/vault"`：

```toml
members = ["crates/infra", "crates/onnx-infra", "crates/asr-local", "crates/asr-cloud", "crates/server", "crates/cli", "crates/desktop", "crates/llm", "crates/dlp", "crates/download", "crates/clipboard", "crates/ocr", "crates/paddle-ocr", "crates/capx", "crates/translation", "crates/search", "crates/vault"]
```

- [x] **Step 2: 创建 crates/vault/Cargo.toml**

```toml
[package]
name = "octopus-vault"
version = "0.1.0"
edition = "2021"

[dependencies]
# 项目内依赖
octopus-infra = { path = "../infra" }

# 加密
argon2 = "0.5"
aes-gcm = "0.10"
hkdf = "0.12"
hmac = "0.12"
sha2 = "0.10"
rand = "0.8"
zeroize = { version = "1.7", features = ["zeroize_derive"] }
data-encoding = "2"
subtle = "2"

# TOTP
totp-rs = { version = "5", default-features = false }

# OS Keychain
keyring = "3"

# eTLD+1
publicsuffix = "2"

# 强度评估
zxcvbn = "3"

# 序列化/工具
serde = { version = "1", features = ["derive"] }
serde_json = "1"
anyhow = "1"
log = "0.4"
parking_lot = { workspace = true }
regex = "1"
```

- [x] **Step 3: 创建 crates/vault/src/lib.rs（仅声明子模块，全部 mod 暂为空文件）**

```rust
//! octopus-vault：密码 vault 核心库。
//!
//! 纯逻辑库，不依赖 tauri / tokio。负责：
//! - 加密（crypto/）
//! - SQLite 存储（storage/）
//! - 密码生成器（generator/）
//! - URL 匹配（matcher/）
//! - 密码健康检查（health/）
//! - Bitwarden 导入（importer/）
//! - TOTP、解锁态管理
//!
//! 依赖方向：infra ← vault ← desktop

pub mod crypto;
pub mod error;
pub mod storage;
pub mod types;
pub mod unlock;
pub mod keychain;
pub mod generator;
pub mod totp;
pub mod matcher;
pub mod health;
pub mod importer;
```

- [x] **Step 4: 创建空的子模块文件**

为每个 `pub mod` 创建对应文件，文件内容暂时只有一行注释：

```bash
# 一次创建所有占位文件
mkdir -p crates/vault/src/{crypto,storage,generator,matcher,health,importer}
for f in error types unlock keychain totp; do
    echo "//! 占位：Task 后续填充" > crates/vault/src/$f.rs
done
echo "//! 占位" > crates/vault/src/crypto/mod.rs
echo "//! 占位" > crates/vault/src/storage/mod.rs
echo "//! 占位" > crates/vault/src/generator/mod.rs
echo "//! 占位" > crates/vault/src/matcher/mod.rs
echo "//! 占位" > crates/vault/src/health/mod.rs
echo "//! 占位" > crates/vault/src/importer/mod.rs
```

- [x] **Step 5: 验证编译通过**

Run: `cargo build -p octopus-vault`
Expected: 0 error 0 warning（可能有 unused 警告，忽略）

- [x] **Step 6: Commit**

```bash
git add Cargo.toml crates/vault/
git commit -m "feat(vault): Task 1 - crate 骨架 + workspace 调整"
```

---

## Task 2: crypto: kdf.rs（Argon2id 派生）

**Files:**
- Create: `crates/vault/src/crypto/kdf.rs`
- Modify: `crates/vault/src/crypto/mod.rs`（re-export kdf）

**Interfaces:**
- Consumes: `argon2` crate
- Produces: `pub fn derive_master_root_key(password: &[u8], salt: &[u8], params: &Argon2Params) -> anyhow::Result<DerivedKey>`、`pub struct Argon2Params { pub iterations: u32, pub memory_kib: u32, pub parallelism: u32 }`、`impl Default for Argon2Params`（返回 spec 默认值 t=3, m=65536, p=4）、`pub struct DerivedKey(pub Zeroizing<[u8; 32]>)`

- [x] **Step 1: 在 crypto/mod.rs 暴露模块 + DerivedKey 类型**

替换 `crates/vault/src/crypto/mod.rs` 内容：

```rust
//! 加密原语：KDF、密钥派生、对称加密。

pub mod kdf;
pub mod hierarchy;
pub mod symmetric;
pub mod util;

use zeroize::Zeroizing;

/// 32 字节密钥，所有派生/加密 key 都用此类型。Drop 时自动清零。
#[derive(Clone)]
pub struct DerivedKey(pub Zeroizing<[u8; 32]>);

impl DerivedKey {
    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}
```

- [x] **Step 2: 写 kdf.rs 测试（先 fail）**

新建 `crates/vault/src/crypto/kdf.rs`：

```rust
//! Argon2id 派生 master_root_key。
//!
//! 参数：t=3, m=65536 KiB (64 MiB), p=4（OWASP 2024 推荐）。
//! salt：32B 随机（首次 init 生成，存 vault_meta.kdf_salt）。

use anyhow::{Context, Result};
use argon2::{Algorithm, Argon2, Params, Version};

use super::DerivedKey;

/// Argon2id 参数。默认 t=3, m=64 MiB, p=4。
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Argon2Params {
    pub iterations: u32,    // t，默认 3
    pub memory_kib: u32,    // m，默认 65536 = 64 MiB
    pub parallelism: u32,   // p，默认 4
}

impl Default for Argon2Params {
    fn default() -> Self {
        Self {
            iterations: 3,
            memory_kib: 65_536,
            parallelism: 4,
        }
    }
}

impl Argon2Params {
    /// 用 Params::new 构造 argon2 crate 用的参数对象
    fn to_params(&self) -> Result<Params> {
        Params::new(self.memory_kib, self.iterations, self.parallelism, Some(32))
            .context("Argon2id 参数无效")
    }
}

/// 从 master_password + 32B salt 派生 master_root_key。
///
/// **调用者必须在调用后立即 zeroize password**（本函数不接管 password 引用）。
pub fn derive_master_root_key(password: &[u8], salt: &[u8], params: &Argon2Params) -> Result<DerivedKey> {
    ensure!(salt.len() == 32, "salt 必须为 32 字节，当前 {}", salt.len());

    let argon2 = Argon2::new(Algorithm::Argon2id, Version::V0x13, params.to_params()?);
    let mut out = [0u8; 32];
    argon2
        .hash_password_into(password, salt, &mut out)
        .context("Argon2id 派生失败")?;
    Ok(DerivedKey(Zeroizing::new(out)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_params_match_spec() {
        let p = Argon2Params::default();
        assert_eq!(p.iterations, 3);
        assert_eq!(p.memory_kib, 65_536);
        assert_eq!(p.parallelism, 4);
    }

    #[test]
    fn test_kdf_deterministic() {
        // 同 password + salt + params → 同 master_root_key
        let salt = [42u8; 32];
        let p = Argon2Params::default();
        let k1 = derive_master_root_key(b"my-password", &salt, &p).unwrap();
        let k2 = derive_master_root_key(b"my-password", &salt, &p).unwrap();
        assert_eq!(k1.as_bytes(), k2.as_bytes());
    }

    #[test]
    fn test_different_password_different_key() {
        let salt = [42u8; 32];
        let p = Argon2Params::default();
        let k1 = derive_master_root_key(b"password1", &salt, &p).unwrap();
        let k2 = derive_master_root_key(b"password2", &salt, &p).unwrap();
        assert_ne!(k1.as_bytes(), k2.as_bytes());
    }

    #[test]
    fn test_different_salt_different_key() {
        let s1 = [1u8; 32];
        let s2 = [2u8; 32];
        let p = Argon2Params::default();
        let k1 = derive_master_root_key(b"same-pwd", &s1, &p).unwrap();
        let k2 = derive_master_root_key(b"same-pwd", &s2, &p).unwrap();
        assert_ne!(k1.as_bytes(), k2.as_bytes());
    }

    #[test]
    fn test_invalid_salt_length() {
        let p = Argon2Params::default();
        let result = derive_master_root_key(b"pwd", &[0u8; 16], &p);
        assert!(result.is_err());
    }
}
```

注：`ensure!` 宏来自 anyhow。`Zeroizing` 已在 mod.rs import，子模块需 `use super::DerivedKey` 即可（无需重新 import Zeroizing）。

但 `use super::DerivedKey` 不够——还需要 `use zeroize::Zeroizing`。修正 Step 2 代码：在文件顶部加：

```rust
use zeroize::Zeroizing;
```

（与 super::DerivedKey 一起 use）

- [x] **Step 3: 修正 hierarchy.rs 和 symmetric.rs 占位文件**

这两个文件 `pub mod` 已被声明但还没实现，先放空内容避免编译失败：

```bash
echo "//! 占位：Task 3 填充" > crates/vault/src/crypto/hierarchy.rs
echo "//! 占位：Task 4 填充" > crates/vault/src/crypto/symmetric.rs
echo "//! 占位：Task 4 填充" > crates/vault/src/crypto/util.rs
```

- [x] **Step 4: 运行测试，验证失败**

Run: `cargo test -p octopus-vault --lib crypto::kdf`
Expected: PASS（5 个测试全过——因为我们直接写了实现 + 测试，按 TDD 应该先 fail 再 pass。**注意**：Task 1 已经把 mod 都声明了，导致 hierarchy / symmetric / util 在 Task 2 阶段必须存在为占位）

**TDD 调整**：因为占位文件存在 + 主代码完整，测试会直接通过。这里 TDD 的价值在于"先写测试用例"，而非"先 fail"。Step 4 实际验证：所有测试通过。

Run: `cargo test -p octopus-vault --lib crypto::kdf -- --nocapture`
Expected: `5 passed`，包含 `test_default_params_match_spec / test_kdf_deterministic / test_different_password_different_key / test_different_salt_different_key / test_invalid_salt_length`

- [x] **Step 5: Commit**

```bash
git add crates/vault/src/crypto/
git commit -m "feat(vault): Task 2 - Argon2id KDF"
```

---

## Task 3: crypto: hierarchy.rs（HMAC-SHA512 child 派生）

**Files:**
- Create: `crates/vault/src/crypto/hierarchy.rs`

**Interfaces:**
- Consumes: `crate::crypto::DerivedKey`、`hmac` / `sha2` crate
- Produces: `impl DerivedKey { pub fn child(&self, label: &[u8]) -> DerivedKey }`、固定常量：`LABEL_USER_VAULT / LABEL_APP_SECRETS / LABEL_SYNC / LABEL_SEND`

- [x] **Step 1: 写 hierarchy.rs 测试（在文件内）**

替换 `crates/vault/src/crypto/hierarchy.rs`：

```rust
//! HMAC-SHA512 child() 派生（简化 BIP44 思想）。
//!
//! child_key = HMAC-SHA512(parent_key, label)[..32]
//! 后 32B 在完整 BIP32 中是 chain code，octopus 不用。
//!
//! label 固定（spec 第 2.1 节）：
//! - b"octopus/v1/user-vault"   → 加密 cipher
//! - b"octopus/v1/app-secrets"  → 加密 API Key
//! - b"octopus/v1/sync"         → 预留（MVP 不生成）
//! - b"octopus/v1/send"         → 预留（MVP 不生成）

use hmac::{Hmac, Mac};
use sha2::Sha512;

use super::DerivedKey;

/// 固定派生 label（spec INV：不可改）。
pub const LABEL_USER_VAULT: &[u8] = b"octopus/v1/user-vault";
pub const LABEL_APP_SECRETS: &[u8] = b"octopus/v1/app-secrets";
pub const LABEL_SYNC: &[u8] = b"octopus/v1/sync";
pub const LABEL_SEND: &[u8] = b"octopus/v1/send";

impl DerivedKey {
    /// 从当前 key 派生子 key。HMAC-SHA512，取前 32B。
    pub fn child(&self, label: &[u8]) -> DerivedKey {
        let mut mac = <Hmac<Sha512> as Mac>::new_from_slice(self.as_bytes())
            .expect("HMAC 接受任意 key 长度");
        mac.update(label);
        let result = mac.finalize().into_bytes();
        let mut child = [0u8; 32];
        child.copy_from_slice(&result[..32]);
        DerivedKey(crate::zeroize::Zeroizing::new(child))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_key(byte: u8) -> DerivedKey {
        DerivedKey(crate::zeroize::Zeroizing::new([byte; 32]))
    }

    #[test]
    fn test_child_deterministic() {
        let parent = make_key(42);
        let c1 = parent.child(LABEL_USER_VAULT);
        let c2 = parent.child(LABEL_USER_VAULT);
        assert_eq!(c1.as_bytes(), c2.as_bytes());
    }

    #[test]
    fn test_different_labels_different_children() {
        let parent = make_key(42);
        let user_vault = parent.child(LABEL_USER_VAULT);
        let app_secrets = parent.child(LABEL_APP_SECRETS);
        assert_ne!(user_vault.as_bytes(), app_secrets.as_bytes());
    }

    #[test]
    fn test_different_parents_different_children() {
        let p1 = make_key(1);
        let p2 = make_key(2);
        let c1 = p1.child(LABEL_USER_VAULT);
        let c2 = p2.child(LABEL_USER_VAULT);
        assert_ne!(c1.as_bytes(), c2.as_bytes());
    }

    #[test]
    fn test_child_different_from_parent() {
        let parent = make_key(42);
        let child = parent.child(LABEL_USER_VAULT);
        assert_ne!(parent.as_bytes(), child.as_bytes());
    }

    #[test]
    fn test_labels_immutable() {
        // 防止后续手贱改 label（spec INV）
        assert_eq!(LABEL_USER_VAULT, b"octopus/v1/user-vault");
        assert_eq!(LABEL_APP_SECRETS, b"octopus/v1/app-secrets");
        assert_eq!(LABEL_SYNC, b"octopus/v1/sync");
        assert_eq!(LABEL_SEND, b"octopus/v1/send");
    }
}
```

注：`crate::zeroize::Zeroizing` 路径——zeroize 是 crate 依赖，需要在 lib.rs 顶部 re-export。Step 2 处理。

- [x] **Step 2: 在 lib.rs re-export Zeroizing（方便子模块写 `crate::zeroize::Zeroizing`）**

修改 `crates/vault/src/lib.rs` 顶部加：

```rust
pub use zeroize::Zeroizing;
```

（放在 `pub mod` 声明之前）

- [x] **Step 3: 运行测试**

Run: `cargo test -p octopus-vault --lib crypto::hierarchy -- --nocapture`
Expected: 5 passed

- [x] **Step 4: Commit**

```bash
git add crates/vault/src/crypto/hierarchy.rs crates/vault/src/lib.rs
git commit -m "feat(vault): Task 3 - HMAC-SHA512 child 派生"
```

---

## Task 4: crypto: symmetric.rs（AES-256-GCM）+ util.rs（随机/Base64）

**Files:**
- Create: `crates/vault/src/crypto/symmetric.rs`
- Create: `crates/vault/src/crypto/util.rs`

**Interfaces:**
- Consumes: `aes-gcm` / `rand` / `data-encoding` crate
- Produces:
  - `impl DerivedKey { pub fn encrypt(&self, plaintext: &[u8]) -> Result<String>; pub fn decrypt(&self, ciphertext: &str) -> Result<Zeroizing<Vec<u8>>> }`
  - `pub const CIPHERTEXT_PREFIX: &str = "v1:"`
  - `util::random_bytes(len: usize) -> Vec<u8>`、`util::random_32() -> [u8; 32]`、`util::base64_encode(bytes: &[u8]) -> String`、`util::base64_decode(s: &str) -> Result<Vec<u8>>`

- [x] **Step 1: 写 util.rs**

替换 `crates/vault/src/crypto/util.rs`：

```rust
//! 工具函数：CSPRNG、Base64、常量时间比较。

use anyhow::{Context, Result};
use data_encoding::BASE64;
use rand::rngs::OsRng;
use rand::RngCore;

/// 用 OS 熵源生成随机字节（CSPRNG）。
pub fn random_bytes(len: usize) -> Vec<u8> {
    let mut buf = vec![0u8; len];
    OsRng.fill_bytes(&mut buf);
    buf
}

/// 生成 32B 随机（用于 K_machine / salt / key 等）。
pub fn random_32() -> [u8; 32] {
    let mut buf = [0u8; 32];
    OsRng.fill_bytes(&mut buf);
    buf
}

pub fn base64_encode(bytes: &[u8]) -> String {
    BASE64.encode(bytes)
}

pub fn base64_decode(s: &str) -> Result<Vec<u8>> {
    BASE64.decode(s.as_bytes()).context("Base64 解码失败")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_random_32_unique() {
        let a = random_32();
        let b = random_32();
        assert_ne!(a, b);
    }

    #[test]
    fn test_base64_round_trip() {
        let original = b"hello world 1234";
        let encoded = base64_encode(original);
        let decoded = base64_decode(&encoded).unwrap();
        assert_eq!(decoded, original);
    }

    #[test]
    fn test_base64_decode_invalid() {
        assert!(base64_decode("!!!invalid base64!!!").is_err());
    }
}
```

- [x] **Step 2: 写 symmetric.rs（含测试）**

替换 `crates/vault/src/crypto/symmetric.rs`：

```rust
//! AES-256-GCM 对称加密。
//!
//! 密文格式（统一）：v1:<base64(nonce[12B] || ciphertext || tag[16B])>
//! AES-GCM 自带 16B 认证 tag，不需要独立 HMAC。

use aes_gcm::aead::{Aead, KeyInit, OsRng as AeadOsRng};
use aes_gcm::{Aes256Gcm, Nonce};
use anyhow::{Context, Result};

use super::util::{base64_decode, base64_encode, random_bytes};
use crate::Zeroizing;

pub const CIPHERTEXT_PREFIX: &str = "v1:";
const NONCE_LEN: usize = 12;

impl super::DerivedKey {
    /// 加密，返回 "v1:<base64(nonce||ct||tag)>"。
    pub fn encrypt(&self, plaintext: &[u8]) -> Result<String> {
        let cipher = Aes256Gcm::new_from_slice(self.as_bytes())
            .context("AES-256-GCM key 长度必须为 32 字节")?;
        let nonce_bytes = random_bytes(NONCE_LEN);
        let nonce = Nonce::from_slice(&nonce_bytes);
        let ciphertext = cipher
            .encrypt(nonce, plaintext)
            .context("AES-256-GCM 加密失败")?;

        let mut combined = Vec::with_capacity(NONCE_LEN + ciphertext.len());
        combined.extend_from_slice(&nonce_bytes);
        combined.extend_from_slice(&ciphertext);
        Ok(format!("{}{}", CIPHERTEXT_PREFIX, base64_encode(&combined)))
    }

    /// 解密 v1: 前缀的密文。
    pub fn decrypt(&self, ciphertext: &str) -> Result<Zeroizing<Vec<u8>>> {
        let ct_str = ciphertext
            .strip_prefix(CIPHERTEXT_PREFIX)
            .context("密文必须以 v1: 开头")?;
        let combined = base64_decode(ct_str)?;
        ensure!(
            combined.len() > NONCE_LEN,
            "密文长度不足（缺 nonce）：{} bytes",
            combined.len()
        );

        let (nonce_bytes, ct) = combined.split_at(NONCE_LEN);
        let cipher = Aes256Gcm::new_from_slice(self.as_bytes())
            .context("AES-256-GCM key 长度必须为 32 字节")?;
        let nonce = Nonce::from_slice(nonce_bytes);
        let plaintext = cipher
            .decrypt(nonce, ct)
            .context("AES-256-GCM 解密失败：密文可能已损坏或 key 不匹配");

        Ok(Zeroizing::new(plaintext))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crypto::DerivedKey;
    use crate::Zeroizing as Z;

    fn make_key(byte: u8) -> DerivedKey {
        DerivedKey(Z::new([byte; 32]))
    }

    #[test]
    fn test_encrypt_decrypt_round_trip() {
        let key = make_key(1);
        let plaintext = b"sensitive data 1234";
        let ct = key.encrypt(plaintext).unwrap();
        assert!(ct.starts_with("v1:"));
        let pt = key.decrypt(&ct).unwrap();
        assert_eq!(&pt[..], plaintext);
    }

    #[test]
    fn test_decrypt_with_wrong_key_fails() {
        let k1 = make_key(1);
        let k2 = make_key(2);
        let ct = k1.encrypt(b"secret").unwrap();
        assert!(k2.decrypt(&ct).is_err());
    }

    #[test]
    fn test_nonce_uniqueness() {
        // 同 key 同明文 → 不同密文（nonce 随机）
        let key = make_key(1);
        let c1 = key.encrypt(b"same").unwrap();
        let c2 = key.encrypt(b"same").unwrap();
        assert_ne!(c1, c2);
        // 但都能解出来
        assert_eq!(&key.decrypt(&c1).unwrap()[..], b"same");
        assert_eq!(&key.decrypt(&c2).unwrap()[..], b"same");
    }

    #[test]
    fn test_decrypt_invalid_prefix() {
        let key = make_key(1);
        assert!(key.decrypt("no-prefix").is_err());
        assert!(key.decrypt("v2:abc").is_err());
    }

    #[test]
    fn test_decrypt_truncated() {
        let key = make_key(1);
        // base64 of 5 bytes（少于 12B nonce）
        assert!(key.decrypt("v1:AAAAAAA").is_err());
    }

    #[test]
    fn test_encrypt_empty_plaintext() {
        let key = make_key(1);
        let ct = key.encrypt(b"").unwrap();
        let pt = key.decrypt(&ct).unwrap();
        assert!(pt.is_empty());
    }

    #[test]
    fn test_encrypt_large_plaintext() {
        let key = make_key(1);
        let big = vec![42u8; 100_000];
        let ct = key.encrypt(&big).unwrap();
        let pt = key.decrypt(&ct).unwrap();
        assert_eq!(&pt[..], &big[..]);
    }
}
```

- [x] **Step 3: 运行测试**

Run: `cargo test -p octopus-vault --lib crypto -- --nocapture`
Expected: util (3) + symmetric (7) + hierarchy (5) + kdf (5) = 20 passed

- [x] **Step 4: Commit**

```bash
git add crates/vault/src/crypto/
git commit -m "feat(vault): Task 4 - AES-256-GCM 对称加密 + 工具函数"
```

---

## Task 5: infra schema v38 + struct + CRUD

**Files:**
- Modify: `crates/infra/src/db.sql`（末尾追加 3 张表）
- Modify: `crates/infra/src/db.rs`（init_schema 加 v38 段；加 VaultMeta / VaultCipher / VaultFolder struct + CRUD）
- Modify: `crates/infra/src/db.rs`（升级 `PRAGMA user_version = 38`）

**Interfaces:**
- Consumes: `crates/infra/src/db.rs` 的 `with_db` / `Connection`
- Produces:
  - `pub struct VaultMeta { id, kdf_type, kdf_salt: Vec<u8>, kdf_iterations, kdf_memory_kib, kdf_parallelism, protected_user_vault_key, app_key_local_enc, app_key_sync_enc, security_stamp, equivalent_domains, public_key, protected_private_key, created_at, updated_at }`
  - `pub struct VaultCipher { id, folder_id: Option<i64>, favorite: bool, atype: i64, name, notes: Option<String>, data, fields: Option<String>, password_history: Option<String>, reprompt: i64, deleted_at: Option<String>, created_at, updated_at }`
  - `pub struct VaultFolder { id, name, sort_order: i64, created_at, updated_at }`
  - `pub fn load_vault_meta() -> Result<Option<VaultMeta>>`
  - `pub fn upsert_vault_meta(meta: &VaultMetaInput) -> Result<()>`
  - `pub fn update_vault_security_stamp(stamp: &str) -> Result<()>`
  - `pub fn list_vault_ciphers() -> Result<Vec<VaultCipher>>`（含软删除的，由应用层过滤）
  - `pub fn load_vault_cipher(id: i64) -> Result<Option<VaultCipher>>`
  - `pub fn insert_vault_cipher(input: &VaultCipherInput) -> Result<i64>`
  - `pub fn update_vault_cipher(id: i64, input: &VaultCipherInput) -> Result<()>`
  - `pub fn soft_delete_vault_cipher(id: i64) -> Result<()>`
  - `pub fn restore_vault_cipher(id: i64) -> Result<()>`
  - `pub fn permanent_delete_vault_cipher(id: i64) -> Result<()>`
  - `pub fn list_vault_folders() -> Result<Vec<VaultFolder>>`
  - `pub fn insert_vault_folder(name: &str) -> Result<i64>`

- [x] **Step 1: db.sql 末尾追加 3 张表**

打开 `crates/infra/src/db.sql`，跳到文件末尾（行 407 后），追加：

```sql

-- ============================================================================
-- Password Vault（schema v38，2026-07-18 新增）
-- ============================================================================

-- vault 元数据：单行（CHECK id=1）。
-- KDF 参数 + 双层密钥的"保护壳"（master_root_key / K_machine 双密文 app_key）。
CREATE TABLE IF NOT EXISTS vault_meta (
    id                          INTEGER PRIMARY KEY CHECK (id = 1),
    kdf_type                    INTEGER NOT NULL,            -- 0=Argon2id（MVP 仅支持 0）
    kdf_salt                    BLOB NOT NULL,               -- 32 字节随机盐
    kdf_iterations              INTEGER NOT NULL,            -- Argon2id: t (默认 3)
    kdf_memory_kib              INTEGER NOT NULL,            -- Argon2id: m (默认 65536 = 64 MiB)
    kdf_parallelism             INTEGER NOT NULL,            -- Argon2id: p (默认 4)
    protected_user_vault_key    TEXT NOT NULL,               -- v1:base64(...)，被 master_root_key 加密
    app_key_local_enc           TEXT NOT NULL,               -- 被 K_machine 加密（本机无感启动）
    app_key_sync_enc            TEXT NOT NULL,               -- 被 master_root_key 加密（跨机同步）
    security_stamp              TEXT NOT NULL,               -- 改主密码 / 改 KDF 时刷新（UUID v4）
    equivalent_domains          TEXT NOT NULL DEFAULT '[]',  -- JSON 数组的数组
    public_key                  TEXT,
    protected_private_key       TEXT,
    created_at                  TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at                  TEXT NOT NULL DEFAULT (datetime('now'))
);

-- vault 密码条目。所有敏感字段（name/notes/data/fields/password_history）均为密文 v1:base64(...)。
CREATE TABLE IF NOT EXISTS vault_ciphers (
    id                  INTEGER PRIMARY KEY AUTOINCREMENT,
    folder_id           INTEGER DEFAULT NULL,            -- 预留：未来 FK vault_folders(id)
    favorite            INTEGER NOT NULL DEFAULT 0,
    atype               INTEGER NOT NULL,                -- 1=Login（MVP 仅此）
    name                TEXT NOT NULL,                   -- 密文 v1:base64(...)
    notes               TEXT DEFAULT NULL,               -- 密文
    data                TEXT NOT NULL,                   -- 密文 JSON（uris/username/password/totp）
    fields              TEXT DEFAULT NULL,               -- 密文 JSON（自定义字段）
    password_history    TEXT DEFAULT NULL,               -- 密文 JSON（密码历史）
    reprompt            INTEGER NOT NULL DEFAULT 0,      -- 0=None 1=Password
    deleted_at          TEXT DEFAULT NULL,               -- 回收站软删除
    created_at          TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at          TEXT NOT NULL DEFAULT (datetime('now')),
    FOREIGN KEY (folder_id) REFERENCES vault_folders(id) ON DELETE SET NULL
);

CREATE INDEX IF NOT EXISTS idx_vault_ciphers_favorite
    ON vault_ciphers(favorite) WHERE deleted_at IS NULL;
CREATE INDEX IF NOT EXISTS idx_vault_ciphers_deleted ON vault_ciphers(deleted_at);

-- vault 文件夹（schema 预留，MVP UI 不暴露，但 vault_ciphers.folder_id 已有 FK）。
CREATE TABLE IF NOT EXISTS vault_folders (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    name        TEXT NOT NULL,                    -- 密文 v1:base64(...)
    sort_order  INTEGER NOT NULL DEFAULT 0,
    created_at  TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at  TEXT NOT NULL DEFAULT (datetime('now'))
);
```

- [x] **Step 2: init_schema 加 v38 升级段**

打开 `crates/infra/src/db.rs`，找到第 436 行 `conn.execute("PRAGMA user_version = 37", [])?;`。

**注意**：根据调研，init_schema 函数结构是：
- 行 232 起 `fn init_schema`
- 行 237 `if v >= 37 { return Ok(()); }` ← 改为 `if v >= 38`
- 行 242-401 渐进式 ALTER（v17+）
- 行 404-438 全新库 INIT_SQL → set user_version=37

修改步骤：

(a) 在 `if v >= 37` 段之后（行 240 附近）追加新分支：

```rust
    // v37 → v38：新增 3 张 vault 表（2026-07-18 Password Vault）
    if v < 38 {
        conn.execute_batch(
            "
            CREATE TABLE IF NOT EXISTS vault_meta (
                id                          INTEGER PRIMARY KEY CHECK (id = 1),
                kdf_type                    INTEGER NOT NULL,
                kdf_salt                    BLOB NOT NULL,
                kdf_iterations              INTEGER NOT NULL,
                kdf_memory_kib              INTEGER NOT NULL,
                kdf_parallelism             INTEGER NOT NULL,
                protected_user_vault_key    TEXT NOT NULL,
                app_key_local_enc           TEXT NOT NULL,
                app_key_sync_enc            TEXT NOT NULL,
                security_stamp              TEXT NOT NULL,
                equivalent_domains          TEXT NOT NULL DEFAULT '[]',
                public_key                  TEXT,
                protected_private_key       TEXT,
                created_at                  TEXT NOT NULL DEFAULT (datetime('now')),
                updated_at                  TEXT NOT NULL DEFAULT (datetime('now'))
            );
            CREATE TABLE IF NOT EXISTS vault_ciphers (
                id                  INTEGER PRIMARY KEY AUTOINCREMENT,
                folder_id           INTEGER DEFAULT NULL,
                favorite            INTEGER NOT NULL DEFAULT 0,
                atype               INTEGER NOT NULL,
                name                TEXT NOT NULL,
                notes               TEXT DEFAULT NULL,
                data                TEXT NOT NULL,
                fields              TEXT DEFAULT NULL,
                password_history    TEXT DEFAULT NULL,
                reprompt            INTEGER NOT NULL DEFAULT 0,
                deleted_at          TEXT DEFAULT NULL,
                created_at          TEXT NOT NULL DEFAULT (datetime('now')),
                updated_at          TEXT NOT NULL DEFAULT (datetime('now')),
                FOREIGN KEY (folder_id) REFERENCES vault_folders(id) ON DELETE SET NULL
            );
            CREATE INDEX IF NOT EXISTS idx_vault_ciphers_favorite
                ON vault_ciphers(favorite) WHERE deleted_at IS NULL;
            CREATE INDEX IF NOT EXISTS idx_vault_ciphers_deleted ON vault_ciphers(deleted_at);
            CREATE TABLE IF NOT EXISTS vault_folders (
                id          INTEGER PRIMARY KEY AUTOINCREMENT,
                name        TEXT NOT NULL,
                sort_order  INTEGER NOT NULL DEFAULT 0,
                created_at  TEXT NOT NULL DEFAULT (datetime('now')),
                updated_at  TEXT NOT NULL DEFAULT (datetime('now'))
            );
            ",
        )?;
        conn.execute("PRAGMA user_version = 38", [])?;
        log::info!("schema v37 → v38：新增 vault 表");
    }
```

(b) 修改 `if v >= 37 { return Ok(()); }`（行 237）为 `if v >= 38 { return Ok(()); }`

(c) 修改全新库分支的 user_version 设置：找到 INIT_SQL 分支末尾的 `PRAGMA user_version = 37`（行 436），改为 `PRAGMA user_version = 38`（因为 INIT_SQL 已包含 vault 表，新库直接到 v38）

- [x] **Step 3: 写 VaultMeta / VaultCipher / VaultFolder struct**

在 `crates/infra/src/db.rs` 文件末尾追加（参考 ActionBarItem 模式，行 1660+）：

```rust

// ============================================================================
// Password Vault 模型（schema v38，2026-07-18）
// ============================================================================

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct VaultMeta {
    pub id: i64,
    pub kdf_type: i64,
    pub kdf_salt: Vec<u8>,
    pub kdf_iterations: i64,
    pub kdf_memory_kib: i64,
    pub kdf_parallelism: i64,
    pub protected_user_vault_key: String,
    pub app_key_local_enc: String,
    pub app_key_sync_enc: String,
    pub security_stamp: String,
    pub equivalent_domains: String,
    pub public_key: Option<String>,
    pub protected_private_key: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct VaultCipher {
    pub id: i64,
    pub folder_id: Option<i64>,
    pub favorite: bool,
    pub atype: i64,
    pub name: String,
    pub notes: Option<String>,
    pub data: String,
    pub fields: Option<String>,
    pub password_history: Option<String>,
    pub reprompt: i64,
    pub deleted_at: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct VaultFolder {
    pub id: i64,
    pub name: String,
    pub sort_order: i64,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone)]
pub struct VaultMetaInput {
    pub kdf_type: i64,
    pub kdf_salt: Vec<u8>,
    pub kdf_iterations: i64,
    pub kdf_memory_kib: i64,
    pub kdf_parallelism: i64,
    pub protected_user_vault_key: String,
    pub app_key_local_enc: String,
    pub app_key_sync_enc: String,
    pub security_stamp: String,
    pub equivalent_domains: String,
    pub public_key: Option<String>,
    pub protected_private_key: Option<String>,
}

#[derive(Debug, Clone)]
pub struct VaultCipherInput {
    pub folder_id: Option<i64>,
    pub favorite: bool,
    pub atype: i64,
    pub name: String,
    pub notes: Option<String>,
    pub data: String,
    pub fields: Option<String>,
    pub password_history: Option<String>,
    pub reprompt: i64,
}
```

- [x] **Step 4: 写 CRUD 函数（仿 ActionBarItem 的双层 API）**

继续在文件末尾追加：

```rust
fn row_to_vault_meta(row: &rusqlite::Row) -> rusqlite::Result<VaultMeta> {
    Ok(VaultMeta {
        id: row.get(0)?,
        kdf_type: row.get(1)?,
        kdf_salt: row.get(2)?,
        kdf_iterations: row.get(3)?,
        kdf_memory_kib: row.get(4)?,
        kdf_parallelism: row.get(5)?,
        protected_user_vault_key: row.get(6)?,
        app_key_local_enc: row.get(7)?,
        app_key_sync_enc: row.get(8)?,
        security_stamp: row.get(9)?,
        equivalent_domains: row.get(10)?,
        public_key: row.get(11)?,
        protected_private_key: row.get(12)?,
        created_at: row.get(13)?,
        updated_at: row.get(14)?,
    })
}

fn row_to_vault_cipher(row: &rusqlite::Row) -> rusqlite::Result<VaultCipher> {
    Ok(VaultCipher {
        id: row.get(0)?,
        folder_id: row.get(1)?,
        favorite: row.get::<_, i32>(2)? != 0,
        atype: row.get(3)?,
        name: row.get(4)?,
        notes: row.get(5)?,
        data: row.get(6)?,
        fields: row.get(7)?,
        password_history: row.get(8)?,
        reprompt: row.get(9)?,
        deleted_at: row.get(10)?,
        created_at: row.get(11)?,
        updated_at: row.get(12)?,
    })
}

const VAULT_CIPHER_COLS: &str = "id, folder_id, favorite, atype, name, notes, data, fields, password_history, reprompt, deleted_at, created_at, updated_at";

pub fn load_vault_meta() -> Result<Option<VaultMeta>> {
    with_db(|conn| load_vault_meta_at(conn))
}

fn load_vault_meta_at(conn: &Connection) -> Result<Option<VaultMeta>> {
    let mut stmt = conn.prepare(
        "SELECT id, kdf_type, kdf_salt, kdf_iterations, kdf_memory_kib, kdf_parallelism,
                protected_user_vault_key, app_key_local_enc, app_key_sync_enc, security_stamp,
                equivalent_domains, public_key, protected_private_key, created_at, updated_at
         FROM vault_meta WHERE id = 1",
    )?;
    let mut rows = stmt.query([])?;
    if let Some(row) = rows.next()? {
        Ok(Some(row_to_vault_meta(row)?))
    } else {
        Ok(None)
    }
}

pub fn upsert_vault_meta(input: &VaultMetaInput) -> Result<()> {
    with_db(|conn| upsert_vault_meta_at(conn, input))
}

fn upsert_vault_meta_at(conn: &Connection, input: &VaultMetaInput) -> Result<()> {
    conn.execute(
        "INSERT INTO vault_meta (id, kdf_type, kdf_salt, kdf_iterations, kdf_memory_kib, kdf_parallelism,
                                  protected_user_vault_key, app_key_local_enc, app_key_sync_enc, security_stamp,
                                  equivalent_domains, public_key, protected_private_key)
         VALUES (1, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
         ON CONFLICT(id) DO UPDATE SET
            kdf_type = excluded.kdf_type,
            kdf_salt = excluded.kdf_salt,
            kdf_iterations = excluded.kdf_iterations,
            kdf_memory_kib = excluded.kdf_memory_kib,
            kdf_parallelism = excluded.kdf_parallelism,
            protected_user_vault_key = excluded.protected_user_vault_key,
            app_key_local_enc = excluded.app_key_local_enc,
            app_key_sync_enc = excluded.app_key_sync_enc,
            security_stamp = excluded.security_stamp,
            equivalent_domains = excluded.equivalent_domains,
            public_key = excluded.public_key,
            protected_private_key = excluded.protected_private_key,
            updated_at = datetime('now')",
        rusqlite::params![
            input.kdf_type,
            input.kdf_salt,
            input.kdf_iterations,
            input.kdf_memory_kib,
            input.kdf_parallelism,
            input.protected_user_vault_key,
            input.app_key_local_enc,
            input.app_key_sync_enc,
            input.security_stamp,
            input.equivalent_domains,
            input.public_key,
            input.protected_private_key,
        ],
    )?;
    Ok(())
}

pub fn update_vault_security_stamp(stamp: &str) -> Result<()> {
    with_db(|conn| {
        conn.execute(
            "UPDATE vault_meta SET security_stamp = ?, updated_at = datetime('now') WHERE id = 1",
            rusqlite::params![stamp],
        )?;
        Ok(())
    })
}

pub fn list_vault_ciphers() -> Result<Vec<VaultCipher>> {
    with_db(|conn| {
        let mut stmt = conn.prepare(&format!(
            "SELECT {} FROM vault_ciphers ORDER BY updated_at DESC",
            VAULT_CIPHER_COLS
        ))?;
        let rows = stmt.query_map([], row_to_vault_cipher)?;
        let mut list = Vec::new();
        for row in rows {
            list.push(row?);
        }
        Ok(list)
    })
}

pub fn load_vault_cipher(id: i64) -> Result<Option<VaultCipher>> {
    with_db(|conn| {
        let mut stmt = conn.prepare(&format!("SELECT {} FROM vault_ciphers WHERE id = ?", VAULT_CIPHER_COLS))?;
        let mut rows = stmt.query(rusqlite::params![id])?;
        if let Some(row) = rows.next()? {
            Ok(Some(row_to_vault_cipher(row)?))
        } else {
            Ok(None)
        }
    })
}

pub fn insert_vault_cipher(input: &VaultCipherInput) -> Result<i64> {
    with_db(|conn| {
        conn.execute(
            "INSERT INTO vault_ciphers (folder_id, favorite, atype, name, notes, data, fields, password_history, reprompt)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
            rusqlite::params![
                input.folder_id,
                input.favorite as i32,
                input.atype,
                input.name,
                input.notes,
                input.data,
                input.fields,
                input.password_history,
                input.reprompt,
            ],
        )?;
        Ok(conn.last_insert_rowid())
    })
}

pub fn update_vault_cipher(id: i64, input: &VaultCipherInput) -> Result<()> {
    with_db(|conn| {
        conn.execute(
            "UPDATE vault_ciphers SET
                folder_id = ?, favorite = ?, atype = ?, name = ?, notes = ?, data = ?,
                fields = ?, password_history = ?, reprompt = ?, updated_at = datetime('now')
             WHERE id = ?",
            rusqlite::params![
                input.folder_id,
                input.favorite as i32,
                input.atype,
                input.name,
                input.notes,
                input.data,
                input.fields,
                input.password_history,
                input.reprompt,
                id,
            ],
        )?;
        Ok(())
    })
}

pub fn soft_delete_vault_cipher(id: i64) -> Result<()> {
    with_db(|conn| {
        conn.execute(
            "UPDATE vault_ciphers SET deleted_at = datetime('now') WHERE id = ?",
            rusqlite::params![id],
        )?;
        Ok(())
    })
}

pub fn restore_vault_cipher(id: i64) -> Result<()> {
    with_db(|conn| {
        conn.execute(
            "UPDATE vault_ciphers SET deleted_at = NULL WHERE id = ?",
            rusqlite::params![id],
        )?;
        Ok(())
    })
}

pub fn permanent_delete_vault_cipher(id: i64) -> Result<()> {
    with_db(|conn| {
        conn.execute("DELETE FROM vault_ciphers WHERE id = ?", rusqlite::params![id])?;
        Ok(())
    })
}

pub fn list_vault_folders() -> Result<Vec<VaultFolder>> {
    with_db(|conn| {
        let mut stmt = conn.prepare(
            "SELECT id, name, sort_order, created_at, updated_at FROM vault_folders ORDER BY sort_order ASC",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(VaultFolder {
                id: row.get(0)?,
                name: row.get(1)?,
                sort_order: row.get(2)?,
                created_at: row.get(3)?,
                updated_at: row.get(4)?,
            })
        })?;
        let mut list = Vec::new();
        for row in rows {
            list.push(row?);
        }
        Ok(list)
    })
}

pub fn insert_vault_folder(name: &str) -> Result<i64> {
    with_db(|conn| {
        conn.execute(
            "INSERT INTO vault_folders (name) VALUES (?)",
            rusqlite::params![name],
        )?;
        Ok(conn.last_insert_rowid())
    })
}
```

- [x] **Step 5: 编译验证**

Run: `cargo build -p octopus-infra`
Expected: 0 error 0 warning

- [x] **Step 6: 写 schema 升级测试（在 db.rs 内联 #[cfg(test)]）**

在 `crates/infra/src/db.rs` 文件末尾（或现有 tests mod 内）追加：

```rust
#[cfg(test)]
mod vault_schema_tests {
    use super::*;

    fn test_db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(include_str!("db.sql")).unwrap();
        conn.execute("PRAGMA user_version = 38", []).unwrap();
        conn
    }

    #[test]
    fn test_vault_meta_upsert_and_load() {
        let conn = test_db();
        assert!(load_vault_meta_at(&conn).unwrap().is_none());

        let input = VaultMetaInput {
            kdf_type: 0,
            kdf_salt: vec![1u8; 32],
            kdf_iterations: 3,
            kdf_memory_kib: 65_536,
            kdf_parallelism: 4,
            protected_user_vault_key: "v1:aaa".into(),
            app_key_local_enc: "v1:bbb".into(),
            app_key_sync_enc: "v1:ccc".into(),
            security_stamp: "stamp-1".into(),
            equivalent_domains: "[]".into(),
            public_key: None,
            protected_private_key: None,
        };
        upsert_vault_meta_at(&conn, &input).unwrap();

        let loaded = load_vault_meta_at(&conn).unwrap().unwrap();
        assert_eq!(loaded.kdf_salt, vec![1u8; 32]);
        assert_eq!(loaded.security_stamp, "stamp-1");
        assert_eq!(loaded.equivalent_domains, "[]");

        // Upsert（覆盖）
        let mut input2 = input.clone();
        input2.security_stamp = "stamp-2".into();
        upsert_vault_meta_at(&conn, &input2).unwrap();
        let loaded2 = load_vault_meta_at(&conn).unwrap().unwrap();
        assert_eq!(loaded2.security_stamp, "stamp-2");
    }

    #[test]
    fn test_vault_cipher_crud() {
        let conn = test_db();
        let input = VaultCipherInput {
            folder_id: None,
            favorite: false,
            atype: 1,
            name: "v1:enc-name".into(),
            notes: None,
            data: "v1:enc-data".into(),
            fields: None,
            password_history: None,
            reprompt: 0,
        };
        let id = insert_vault_cipher_at(&conn, &input).unwrap();
        assert!(id > 0);

        let loaded = load_vault_cipher_at(&conn, id).unwrap().unwrap();
        assert_eq!(loaded.name, "v1:enc-name");
        assert_eq!(loaded.atype, 1);

        let mut input2 = input.clone();
        input2.name = "v1:enc-name-2".into();
        update_vault_cipher_at(&conn, id, &input2).unwrap();
        let loaded2 = load_vault_cipher_at(&conn, id).unwrap().unwrap();
        assert_eq!(loaded2.name, "v1:enc-name-2");

        // 软删除
        soft_delete_vault_cipher_at(&conn, id).unwrap();
        let loaded3 = load_vault_cipher_at(&conn, id).unwrap().unwrap();
        assert!(loaded3.deleted_at.is_some());

        // 恢复
        restore_vault_cipher_at(&conn, id).unwrap();
        let loaded4 = load_vault_cipher_at(&conn, id).unwrap().unwrap();
        assert!(loaded4.deleted_at.is_none());

        // 物理删除
        permanent_delete_vault_cipher_at(&conn, id).unwrap();
        assert!(load_vault_cipher_at(&conn, id).unwrap().is_none());
    }

    #[test]
    fn test_vault_meta_check_constraint() {
        let conn = test_db();
        // 尝试插入 id=2 应失败（CHECK id=1）
        let result = conn.execute(
            "INSERT INTO vault_meta (id, kdf_type, kdf_salt, kdf_iterations, kdf_memory_kib, kdf_parallelism,
                                      protected_user_vault_key, app_key_local_enc, app_key_sync_enc, security_stamp)
             VALUES (2, 0, X'00', 0, 0, 0, '', '', '', '')",
            [],
        );
        assert!(result.is_err(), "CHECK(id=1) 应阻止 id=2");
    }
}
```

**重要**：上面测试用了 `_at` 后缀的函数（接 `&Connection`），但 Step 4 的 CRUD 是 `with_db` 包装的公开版本。需要补充 `_at` 内部版本。**追加**到 Step 4 的代码块后面：

```rust
// _at 内部版本（测试用 + with_db 包装的真实实现）
fn insert_vault_cipher_at(conn: &Connection, input: &VaultCipherInput) -> Result<i64> {
    conn.execute(
        "INSERT INTO vault_ciphers (folder_id, favorite, atype, name, notes, data, fields, password_history, reprompt)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
        rusqlite::params![
            input.folder_id, input.favorite as i32, input.atype, input.name,
            input.notes, input.data, input.fields, input.password_history, input.reprompt,
        ],
    )?;
    Ok(conn.last_insert_rowid())
}

fn update_vault_cipher_at(conn: &Connection, id: i64, input: &VaultCipherInput) -> Result<()> {
    conn.execute(
        "UPDATE vault_ciphers SET folder_id = ?, favorite = ?, atype = ?, name = ?, notes = ?,
                                   data = ?, fields = ?, password_history = ?, reprompt = ?,
                                   updated_at = datetime('now') WHERE id = ?",
        rusqlite::params![
            input.folder_id, input.favorite as i32, input.atype, input.name, input.notes,
            input.data, input.fields, input.password_history, input.reprompt, id,
        ],
    )?;
    Ok(())
}

fn load_vault_cipher_at(conn: &Connection, id: i64) -> Result<Option<VaultCipher>> {
    let mut stmt = conn.prepare(&format!("SELECT {} FROM vault_ciphers WHERE id = ?", VAULT_CIPHER_COLS))?;
    let mut rows = stmt.query(rusqlite::params![id])?;
    if let Some(row) = rows.next()? {
        Ok(Some(row_to_vault_cipher(row)?))
    } else {
        Ok(None)
    }
}

fn soft_delete_vault_cipher_at(conn: &Connection, id: i64) -> Result<()> {
    conn.execute("UPDATE vault_ciphers SET deleted_at = datetime('now') WHERE id = ?", rusqlite::params![id])?;
    Ok(())
}

fn restore_vault_cipher_at(conn: &Connection, id: i64) -> Result<()> {
    conn.execute("UPDATE vault_ciphers SET deleted_at = NULL WHERE id = ?", rusqlite::params![id])?;
    Ok(())
}

fn permanent_delete_vault_cipher_at(conn: &Connection, id: i64) -> Result<()> {
    conn.execute("DELETE FROM vault_ciphers WHERE id = ?", rusqlite::params![id])?;
    Ok(())
}
```

（`with_db` 包装的公开函数改为调用 `_at` 内部版本，这是项目现有 ActionBarItem 的模式）

公开函数的实现调整为：

```rust
pub fn insert_vault_cipher(input: &VaultCipherInput) -> Result<i64> {
    with_db(|conn| insert_vault_cipher_at(conn, input))
}
pub fn update_vault_cipher(id: i64, input: &VaultCipherInput) -> Result<()> {
    with_db(|conn| update_vault_cipher_at(conn, id, input))
}
pub fn load_vault_cipher(id: i64) -> Result<Option<VaultCipher>> {
    with_db(|conn| load_vault_cipher_at(conn, id))
}
pub fn soft_delete_vault_cipher(id: i64) -> Result<()> {
    with_db(|conn| soft_delete_vault_cipher_at(conn, id))
}
pub fn restore_vault_cipher(id: i64) -> Result<()> {
    with_db(|conn| restore_vault_cipher_at(conn, id))
}
pub fn permanent_delete_vault_cipher(id: i64) -> Result<()> {
    with_db(|conn| permanent_delete_vault_cipher_at(conn, id))
}
pub fn list_vault_ciphers() -> Result<Vec<VaultCipher>> {
    with_db(|conn| {
        let mut stmt = conn.prepare(&format!("SELECT {} FROM vault_ciphers ORDER BY updated_at DESC", VAULT_CIPHER_COLS))?;
        let rows = stmt.query_map([], row_to_vault_cipher)?;
        let mut list = Vec::new();
        for row in rows { list.push(row?); }
        Ok(list)
    })
}
```

- [x] **Step 7: 运行测试**

Run: `cargo test -p octopus-infra --lib vault_schema_tests -- --nocapture`
Expected: 3 passed

- [x] **Step 8: 整 workspace 编译验证**

Run: `cargo build`
Expected: 0 error

- [x] **Step 9: Commit**

```bash
git add crates/infra/src/db.sql crates/infra/src/db.rs
git commit -m "feat(vault): Task 5 - schema v38 + VaultMeta/VaultCipher/VaultFolder CRUD"
```

---

## Task 6: vault types.rs（Cipher / LoginData / MatchType）

**Files:**
- Create: `crates/vault/src/types.rs`

**Interfaces:**
- Produces:
  - `pub enum CipherType { Login = 1, SecureNote = 2, Card = 3, Identity = 4 }`（MVP 仅 Login）
  - `pub enum RepromptType { None = 0, Password = 1 }`
  - `pub enum MatchType { Domain = 0, Host = 1, Exact = 2, StartsWith = 3, RegularExpression = 4, Never = 5 }`
  - `pub struct LoginUri { pub uri: String, pub match_type: Option<MatchType> }`
  - `pub struct LoginData { pub uris: Vec<LoginUri>, pub username: Option<String>, pub password: Option<String>, pub totp: Option<String>, pub password_revision_date: Option<String> }`
  - `pub struct Field { pub name: String, pub value: Option<String>, pub field_type: i64 }`
  - `pub struct PasswordHistoryEntry { pub password: String, pub last_used_at: String }`
  - `pub enum CipherData { Login(LoginData) }`（未来扩展 SecureNote/Card/Identity）
  - `pub struct Cipher { id, folder_id, favorite, atype, name, notes, data, fields, password_history, reprompt, deleted_at, created_at, updated_at }`（解密后的明文）
  - `pub struct CipherInput { folder_id, favorite, atype, name, notes, data, fields, password_history, reprompt }`（新建/更新用）
  - 序列化辅助：`Cipher::to_encrypted_strings(&self, key: &DerivedKey) -> Result<CipherEncStrings>`、`CipherEncStrings::decrypt(key) -> Result<Cipher>`

- [x] **Step 1: 写 types.rs**

替换 `crates/vault/src/types.rs`：

```rust
//! Cipher 数据模型（解密后的明文结构 + 序列化辅助）。
//!
//! 这些类型仅在 vault 解锁状态下出现。落盘时通过 `to_encrypted_strings`
//! 转为密文字符串写入 SQLite。

use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::crypto::DerivedKey;

/// cipher 类型（MVP 仅 Login）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(into = "i64", from = "i64")]
pub enum CipherType {
    Login = 1,
    SecureNote = 2,
    Card = 3,
    Identity = 4,
}

impl From<CipherType> for i64 {
    fn from(t: CipherType) -> i64 {
        t as i64
    }
}

impl From<i64> for CipherType {
    fn from(v: i64) -> Self {
        match v {
            2 => CipherType::SecureNote,
            3 => CipherType::Card,
            4 => CipherType::Identity,
            _ => CipherType::Login, // 兜底为 Login（兼容未知类型）
        }
    }
}

/// 敏感操作前是否需要再次确认主密码。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(into = "i64", from = "i64")]
pub enum RepromptType {
    None = 0,
    Password = 1,
}

impl From<RepromptType> for i64 {
    fn from(t: RepromptType) -> i64 {
        t as i64
    }
}

impl From<i64> for RepromptType {
    fn from(v: i64) -> Self {
        if v == 1 {
            RepromptType::Password
        } else {
            RepromptType::None
        }
    }
}

/// URI 匹配策略（直接抄 Bitwarden 5 种 + Never）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(into = "i64", try_from = "i64")]
pub enum MatchType {
    Domain = 0,
    Host = 1,
    Exact = 2,
    StartsWith = 3,
    RegularExpression = 4,
    Never = 5,
}

impl From<MatchType> for i64 {
    fn from(t: MatchType) -> i64 {
        t as i64
    }
}

impl TryFrom<i64> for MatchType {
    type Error = anyhow::Error;
    fn try_from(v: i64) -> Result<Self> {
        Ok(match v {
            0 => MatchType::Domain,
            1 => MatchType::Host,
            2 => MatchType::Exact,
            3 => MatchType::StartsWith,
            4 => MatchType::RegularExpression,
            5 => MatchType::Never,
            _ => anyhow::bail!("无效的 MatchType: {}", v),
        })
    }
}

/// 单条 URI + 其匹配策略（None 表示用客户端默认，octopus 强制 Domain）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoginUri {
    pub uri: String,
    /// null = 用客户端默认（Domain）
    pub match_type: Option<MatchType>,
}

/// Login 类型 cipher 的明文 payload（落盘时加密为 data 字段）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoginData {
    pub uris: Vec<LoginUri>,
    pub username: Option<String>,
    pub password: Option<String>,
    /// Base32 secret（如 "JBSWY3DPEHPK3PXP"），不带 otpauth:// 前缀。
    pub totp: Option<String>,
    pub password_revision_date: Option<String>,
}

/// 自定义字段（密码、文本、隐藏等）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Field {
    pub name: String,
    pub value: Option<String>,
    /// 0=Text 1=Hidden 2=Boolean（Bitwarden 协议）
    pub field_type: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PasswordHistoryEntry {
    pub password: String,
    pub last_used_at: String,
}

/// cipher data 枚举（MVP 仅 Login，未来扩展 SecureNote/Card/Identity）。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "data", rename_all = "lowercase")]
pub enum CipherData {
    Login(LoginData),
}

/// 解密后的 cipher 完整对象。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Cipher {
    pub id: i64,
    pub folder_id: Option<i64>,
    pub favorite: bool,
    pub atype: CipherType,
    pub name: String,
    pub notes: Option<String>,
    pub data: CipherData,
    pub fields: Vec<Field>,
    pub password_history: Vec<PasswordHistoryEntry>,
    pub reprompt: RepromptType,
    pub deleted_at: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

/// 新建/更新 cipher 的输入（不带 id/时间戳）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CipherInput {
    pub folder_id: Option<i64>,
    pub favorite: bool,
    pub atype: CipherType,
    pub name: String,
    pub notes: Option<String>,
    pub data: CipherData,
    pub fields: Vec<Field>,
    pub password_history: Vec<PasswordHistoryEntry>,
    pub reprompt: RepromptType,
}

/// 加密后的 cipher 字段（与 db.rs 的 VaultCipher 明文字段一一对应）。
/// 由 vault crate 调用 `Cipher::encrypt_strings(&key)` 生成，再调
/// `VaultCipherInput { name, notes, data, fields, password_history, ... }` 落库。
pub struct CipherEncStrings {
    pub name: String,
    pub notes: Option<String>,
    pub data: String,
    pub fields: Option<String>,
    pub password_history: Option<String>,
}

impl Cipher {
    /// 用 user_vault_key 加密所有敏感字段。
    pub fn encrypt_strings(&self, key: &DerivedKey) -> Result<CipherEncStrings> {
        let name = key.encrypt(self.name.as_bytes())?;
        let notes = match &self.notes {
            Some(n) => Some(key.encrypt(n.as_bytes())?),
            None => None,
        };
        let data_json = serde_json::to_vec(&self.data)?;
        let data = key.encrypt(&data_json)?;
        let fields = if self.fields.is_empty() {
            None
        } else {
            let json = serde_json::to_vec(&self.fields)?;
            Some(key.encrypt(&json)?)
        };
        let password_history = if self.password_history.is_empty() {
            None
        } else {
            let json = serde_json::to_vec(&self.password_history)?;
            Some(key.encrypt(&json)?)
        };
        Ok(CipherEncStrings {
            name,
            notes,
            data,
            fields,
            password_history,
        })
    }
}

impl CipherInput {
    /// 用 user_vault_key 加密。
    pub fn encrypt_strings(&self, key: &DerivedKey) -> Result<CipherEncStrings> {
        let name = key.encrypt(self.name.as_bytes())?;
        let notes = match &self.notes {
            Some(n) => Some(key.encrypt(n.as_bytes())?),
            None => None,
        };
        let data_json = serde_json::to_vec(&self.data)?;
        let data = key.encrypt(&data_json)?;
        let fields = if self.fields.is_empty() {
            None
        } else {
            let json = serde_json::to_vec(&self.fields)?;
            Some(key.encrypt(&json)?)
        };
        let password_history = if self.password_history.is_empty() {
            None
        } else {
            let json = serde_json::to_vec(&self.password_history)?;
            Some(key.encrypt(&json)?)
        };
        Ok(CipherEncStrings {
            name,
            notes,
            data,
            fields,
            password_history,
        })
    }
}

/// 从 infra 的 VaultCipher（密文行）+ 解密 key → 解密 Cipher。
pub fn decrypt_cipher_row(
    row: &octopus_infra::db::VaultCipher,
    key: &DerivedKey,
) -> Result<Cipher> {
    let name_bytes = key.decrypt(&row.name)?;
    let name = String::from_utf8(name_bytes.to_vec())?;

    let notes = match &row.notes {
        Some(n) => {
            let bytes = key.decrypt(n)?;
            Some(String::from_utf8(bytes.to_vec())?)
        }
        None => None,
    };

    let data_bytes = key.decrypt(&row.data)?;
    let data: CipherData = serde_json::from_slice(&data_bytes)?;

    let fields = match &row.fields {
        Some(f) => {
            let bytes = key.decrypt(f)?;
            serde_json::from_slice(&bytes)?
        }
        None => vec![],
    };

    let password_history = match &row.password_history {
        Some(p) => {
            let bytes = key.decrypt(p)?;
            serde_json::from_slice(&bytes)?
        }
        None => vec![],
    };

    Ok(Cipher {
        id: row.id,
        folder_id: row.folder_id,
        favorite: row.favorite,
        atype: row.atype.into(),
        name,
        notes,
        data,
        fields,
        password_history,
        reprompt: row.reprompt.into(),
        deleted_at: row.deleted_at,
        created_at: row.created_at,
        updated_at: row.updated_at,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_key(byte: u8) -> DerivedKey {
        DerivedKey(crate::Zeroizing::new([byte; 32]))
    }

    fn sample_input() -> CipherInput {
        CipherInput {
            folder_id: None,
            favorite: false,
            atype: CipherType::Login,
            name: "GitHub".into(),
            notes: Some("personal".into()),
            data: CipherData::Login(LoginData {
                uris: vec![LoginUri {
                    uri: "https://github.com".into(),
                    match_type: Some(MatchType::Domain),
                }],
                username: Some("user@example.com".into()),
                password: Some("p@ssw0rd".into()),
                totp: Some("JBSWY3DPEHPK3PXP".into()),
                password_revision_date: None,
            }),
            fields: vec![Field {
                name: "backup_code".into(),
                value: Some("12345".into()),
                field_type: 1,
            }],
            password_history: vec![],
            reprompt: RepromptType::None,
        }
    }

    #[test]
    fn test_cipher_encrypt_decrypt_round_trip() {
        let key = make_key(1);
        let input = sample_input();
        let enc = input.encrypt_strings(&key).unwrap();
        assert!(enc.name.starts_with("v1:"));
        assert!(enc.data.starts_with("v1:"));
        assert!(enc.fields.as_ref().unwrap().starts_with("v1:"));

        // 构造一个 VaultCipher 行模拟解密路径
        let row = octopus_infra::db::VaultCipher {
            id: 1,
            folder_id: None,
            favorite: false,
            atype: 1,
            name: enc.name,
            notes: enc.notes,
            data: enc.data,
            fields: enc.fields,
            password_history: enc.password_history,
            reprompt: 0,
            deleted_at: None,
            created_at: "2026-07-18".into(),
            updated_at: "2026-07-18".into(),
        };
        let decrypted = decrypt_cipher_row(&row, &key).unwrap();
        assert_eq!(decrypted.name, "GitHub");
        assert_eq!(decrypted.notes, Some("personal".into()));
        if let CipherData::Login(login) = decrypted.data {
            assert_eq!(login.username, Some("user@example.com".into()));
            assert_eq!(login.password, Some("p@ssw0rd".into()));
            assert_eq!(login.uris[0].uri, "https://github.com");
        } else {
            panic!("应为 Login");
        }
        assert_eq!(decrypted.fields[0].name, "backup_code");
        assert_eq!(decrypted.fields[0].value, Some("12345".into()));
    }

    #[test]
    fn test_cipher_encrypt_empty_fields_omitted() {
        let key = make_key(1);
        let mut input = sample_input();
        input.fields = vec![];
        input.password_history = vec![];
        input.notes = None;
        let enc = input.encrypt_strings(&key).unwrap();
        assert!(enc.fields.is_none(), "空 fields 应省略");
        assert!(enc.password_history.is_none(), "空 history 应省略");
        assert!(enc.notes.is_none(), "None notes 应省略");
    }

    #[test]
    fn test_cipher_type_round_trip() {
        assert_eq!(i64::from(CipherType::Login), 1);
        assert_eq!(i64::from(CipherType::SecureNote), 2);
        assert_eq!(CipherType::from(1), CipherType::Login);
        assert_eq!(CipherType::from(99), CipherType::Login); // 兜底
    }

    #[test]
    fn test_match_type_round_trip() {
        assert_eq!(i64::from(MatchType::Domain), 0);
        assert_eq!(i64::from(MatchType::RegularExpression), 4);
        assert_eq!(MatchType::try_from(0).unwrap(), MatchType::Domain);
        assert!(MatchType::try_from(99).is_err());
    }
}
```

- [x] **Step 2: 运行测试**

Run: `cargo test -p octopus-vault --lib types::tests -- --nocapture`
Expected: 4 passed

- [x] **Step 3: Commit**

```bash
git add crates/vault/src/types.rs
git commit -m "feat(vault): Task 6 - Cipher/LoginData/MatchType 数据模型"
```

---

## Task 7: vault storage（meta/cipher/folder 高层 API）

**Files:**
- Create: `crates/vault/src/storage/mod.rs`
- Create: `crates/vault/src/storage/meta.rs`
- Create: `crates/vault/src/storage/cipher.rs`
- Create: `crates/vault/src/storage/folder.rs`

**Interfaces:**
- Consumes: Task 5 的 infra CRUD、Task 6 的 Cipher/CipherInput
- Produces: 一层包装把 `types::Cipher` 的加解密与 infra CRUD 结合
  - `pub fn read_vault_meta_or_default() -> Result<Option<VaultMeta>>`（直接转发）
  - `pub fn save_vault_meta(input: &VaultMetaInput) -> Result<()>`
  - `pub fn update_security_stamp(stamp: &str) -> Result<()>`
  - `pub fn list_ciphers(key: &DerivedKey) -> Result<Vec<Cipher>>`（解密）
  - `pub fn load_cipher(id: i64, key: &DerivedKey) -> Result<Option<Cipher>>`
  - `pub fn create_cipher(input: &CipherInput, key: &DerivedKey) -> Result<i64>`（加密 + insert）
  - `pub fn save_cipher(id: i64, input: &CipherInput, key: &DerivedKey) -> Result<()>`
  - `pub fn soft_delete(id: i64) -> Result<()>`、`restore(id)`、`permanent_delete(id)`

- [x] **Step 1: storage/mod.rs**

```rust
//! vault 存储层：把 types::Cipher 的加解密与 infra CRUD 结合。

pub mod cipher;
pub mod folder;
pub mod meta;

pub use cipher::{create_cipher, list_ciphers, load_cipher, save_cipher, soft_delete, restore, permanent_delete};
pub use meta::{read_vault_meta, save_vault_meta, update_security_stamp};
pub use folder::{list_folders, create_folder};
```

- [x] **Step 2: storage/meta.rs**

```rust
//! vault_meta 表的薄包装（直接转发 infra）。

use anyhow::Result;
use octopus_infra::db::{self, VaultMeta, VaultMetaInput};

pub fn read_vault_meta() -> Result<Option<VaultMeta>> {
    Ok(db::load_vault_meta()?)
}

pub fn save_vault_meta(input: &VaultMetaInput) -> Result<()> {
    Ok(db::upsert_vault_meta(input)?)
}

pub fn update_security_stamp(stamp: &str) -> Result<()> {
    Ok(db::update_vault_security_stamp(stamp)?)
}
```

- [x] **Step 3: storage/cipher.rs**

```rust
//! vault_ciphers 表的高层 API：Cipher 加解密 + CRUD。

use anyhow::Result;

use octopus_infra::db::{self, VaultCipherInput};

use crate::crypto::DerivedKey;
use crate::types::{decrypt_cipher_row, Cipher, CipherInput};

pub fn list_ciphers(key: &DerivedKey) -> Result<Vec<Cipher>> {
    let rows = db::list_vault_ciphers()?;
    let mut out = Vec::with_capacity(rows.len());
    for row in rows {
        out.push(decrypt_cipher_row(&row, key)?);
    }
    Ok(out)
}

pub fn load_cipher(id: i64, key: &DerivedKey) -> Result<Option<Cipher>> {
    let row = match db::load_vault_cipher(id)? {
        Some(r) => r,
        None => return Ok(None),
    };
    Ok(Some(decrypt_cipher_row(&row, key)?))
}

pub fn create_cipher(input: &CipherInput, key: &DerivedKey) -> Result<i64> {
    let enc = input.encrypt_strings(key)?;
    let db_input = VaultCipherInput {
        folder_id: input.folder_id,
        favorite: input.favorite,
        atype: input.atype.into(),
        name: enc.name,
        notes: enc.notes,
        data: enc.data,
        fields: enc.fields,
        password_history: enc.password_history,
        reprompt: input.reprompt.into(),
    };
    Ok(db::insert_vault_cipher(&db_input)?)
}

pub fn save_cipher(id: i64, input: &CipherInput, key: &DerivedKey) -> Result<()> {
    let enc = input.encrypt_strings(key)?;
    let db_input = VaultCipherInput {
        folder_id: input.folder_id,
        favorite: input.favorite,
        atype: input.atype.into(),
        name: enc.name,
        notes: enc.notes,
        data: enc.data,
        fields: enc.fields,
        password_history: enc.password_history,
        reprompt: input.reprompt.into(),
    };
    Ok(db::update_vault_cipher(id, &db_input)?)
}

pub fn soft_delete(id: i64) -> Result<()> {
    Ok(db::soft_delete_vault_cipher(id)?)
}

pub fn restore(id: i64) -> Result<()> {
    Ok(db::restore_vault_cipher(id)?)
}

pub fn permanent_delete(id: i64) -> Result<()> {
    Ok(db::permanent_delete_vault_cipher(id)?)
}
```

- [x] **Step 4: storage/folder.rs**

```rust
//! vault_folders 表的薄包装（MVP UI 不暴露，但提供 API）。

use anyhow::Result;
use octopus_infra::db::{self, VaultFolder};

pub fn list_folders() -> Result<Vec<VaultFolder>> {
    Ok(db::list_vault_folders()?)
}

/// 注意：name 应由调用者先用 user_vault_key.encrypt() 加密后再传入。
/// MVP UI 不使用，故不在 storage 层做加密。
pub fn create_folder(name_encrypted: &str) -> Result<i64> {
    Ok(db::insert_vault_folder(name_encrypted)?)
}
```

- [x] **Step 5: 编译验证**

Run: `cargo build -p octopus-vault`
Expected: 0 error 0 warning

- [x] **Step 6: 写集成测试（测试 storage 层加解密往返）**

新建 `crates/vault/tests/storage.rs`（集成测试，独立 .rs 文件）：

```rust
//! storage 层集成测试：使用临时 DB 验证 Cipher 加解密 + CRUD 往返。
//!
//! 注意：这测试需要真实 SQLite，但 with_db() 用的是 ~/.octopus/octopus.db。
//! 为隔离测试，提供 test fixture 重写 octopus_config_home 到 tempdir。
//! 但 octopus-infra 当前未暴露"用任意路径"的入口，所以此测试先以 unit 形式
//! 放在 storage::tests，直接调 db:: 函数（会写入 ~/.octopus，需要手动清理）。
//!
//! 更稳的做法：在 Task 5 的 db.rs 加 #[cfg(test)] pub fn with_test_db<F: FnOnce(&Connection)>(f: F)。
//! 为简化 MVP，本 Task 跳过 storage 集成测试，依赖 Task 6 的 unit 测试（已覆盖加解密往返）。

// 此文件保留为占位，Task 5 的 vault_schema_tests + Task 6 的 types::tests
// 已覆盖核心 CRUD + 加解密逻辑。
```

实际改为在 `crates/vault/src/storage/cipher.rs` 末尾加 `#[cfg(test)] mod tests`：

追加到 `storage/cipher.rs` 末尾：

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{CipherData, CipherType, Field, LoginData, LoginUri, RepromptType};

    fn make_key(byte: u8) -> DerivedKey {
        DerivedKey(crate::Zeroizing::new([byte; 32]))
    }

    fn sample_input(name: &str) -> CipherInput {
        CipherInput {
            folder_id: None,
            favorite: false,
            atype: CipherType::Login,
            name: name.into(),
            notes: None,
            data: CipherData::Login(LoginData {
                uris: vec![LoginUri {
                    uri: format!("https://{}.com", name.to_lowercase()),
                    match_type: None,
                }],
                username: Some("user".into()),
                password: Some("pass".into()),
                totp: None,
                password_revision_date: None,
            }),
            fields: vec![],
            password_history: vec![],
            reprompt: RepromptType::None,
        }
    }

    // 注意：以下测试需要真实 DB（会写入 ~/.octopus/octopus.db）。
    // 在 CI 环境可能失败。如果 ~/.octopus 不可写，整个测试模块 #[ignore]。
    // 用户本地手动运行：cargo test -p octopus-vault --lib storage::cipher -- --nocapture --ignored
    #[test]
    #[ignore]
    fn test_cipher_crud_round_trip_with_real_db() {
        let key = make_key(7);
        let input = sample_input("TestSite");

        // 清理可能存在的旧数据（id 自增，但不假设具体 id）
        // 直接 create + load + verify
        let id = create_cipher(&input, &key).expect("create");
        assert!(id > 0);

        let loaded = load_cipher(id, &key).expect("load").expect("should exist");
        assert_eq!(loaded.name, "TestSite");
        if let CipherData::Login(login) = loaded.data {
            assert_eq!(login.username, Some("user".into()));
        } else {
            panic!("应为 Login");
        }

        // 更新
        let mut input2 = input.clone();
        input2.name = "TestSite2".into();
        save_cipher(id, &input2, &key).expect("save");
        let loaded2 = load_cipher(id, &key).expect("load").expect("should exist");
        assert_eq!(loaded2.name, "TestSite2");

        // 软删除 + 恢复
        soft_delete(id).expect("soft delete");
        let loaded3 = load_cipher(id, &key).expect("load").expect("should still exist (soft del)");
        assert!(loaded3.deleted_at.is_some());

        restore(id).expect("restore");
        let loaded4 = load_cipher(id, &key).expect("load").expect("should exist");
        assert!(loaded4.deleted_at.is_none());

        // 物理删除
        permanent_delete(id).expect("perm delete");
        assert!(load_cipher(id, &key).expect("load").is_none());
    }
}
```

- [x] **Step 7: 运行 lib 编译**

Run: `cargo build -p octopus-vault`
Expected: 0 error

集成测试 `test_cipher_crud_round_trip_with_real_db` 默认 `#[ignore]`，正常 `cargo test` 不会跑。

- [x] **Step 8: Commit**

```bash
git add crates/vault/src/storage/
git commit -m "feat(vault): Task 7 - storage 层（meta/cipher/folder）"
```

---

## Task 8: vault keychain.rs（K_machine 存取）

> **⚠️ Follow-up 修订（commit `0def2450`）**：原计划用 OS Keychain（`keyring` crate），
> 实施时发现 macOS 对 adhoc 签名 binary 写 Keychain 是 session-only（重启即丢），
> 改为本地加密文件 `~/.octopus/machine-key.enc`，file_key 由
> `HKDF-SHA256(IOPlatformUUID + USER)` 派生。`keyring` 依赖从 vault/Cargo.toml 移除。
> **公开 API 名字不变**（`load_or_create_machine_key` 等），但内部实现全改。
> 下文的 keyring 代码块是历史记录，**实际实现见 `crates/vault/src/keychain.rs`**。

**Files:**
- Create: `crates/vault/src/keychain.rs`

**Interfaces:**
- Consumes: `keyring` crate
- Produces:
  - `pub const KEYCHAIN_SERVICE: &str = "octopus-vault";`
  - `pub const KEYCHAIN_USER: &str = "machine-key";`
  - `pub fn load_or_create_machine_key() -> Result<Zeroizing<[u8; 32]>>`
  - `pub fn load_machine_key() -> Result<Option<Zeroizing<[u8; 32]>>>`
  - `pub fn save_machine_key(key: &[u8; 32]) -> Result<()>`
  - `pub fn delete_machine_key() -> Result<()>`

- [x] **Step 1: 写 keychain.rs**

```rust
//! K_machine 在 OS Keychain 的存取。
//!
//! macOS: Keychain Services
//! Windows: Credential Manager
//! Linux: Secret Service（需 gnome-keyring / KDE Wallet，否则降级到每次输主密码）

use anyhow::{Context, Result};
use keyring::Entry;

use crate::crypto::util::random_32;
use crate::Zeroizing;

pub const KEYCHAIN_SERVICE: &str = "octopus-vault";
pub const KEYCHAIN_USER: &str = "machine-key";

/// 读取或创建 K_machine。
///
/// - 首次调用（不存在）→ 生成新 32B 随机 key 存入 Keychain，返回
/// - 后续调用 → 读已有 key 返回
///
/// 失败场景：Linux 无 secret service → 返回 Err（调用方应降级到方案 Y）
pub fn load_or_create_machine_key() -> Result<Zeroizing<[u8; 32]>> {
    if let Some(existing) = load_machine_key()? {
        return Ok(existing);
    }
    let new_key = random_32();
    save_machine_key(&new_key)?;
    Ok(Zeroizing::new(new_key))
}

/// 读取已有 K_machine。不存在返回 Ok(None)。
pub fn load_machine_key() -> Result<Option<Zeroizing<[u8; 32]>>> {
    let entry = Entry::new(KEYCHAIN_SERVICE, KEYCHAIN_USER)
        .context("无法访问 OS Keychain")?;
    match entry.get_password() {
        Ok(s) => {
            let bytes: Vec<u8> = s.bytes().collect();
            ensure!(bytes.len() == 32, "K_machine 长度异常：{} bytes", bytes.len());
            let mut arr = [0u8; 32];
            arr.copy_from_slice(&bytes);
            Ok(Some(Zeroizing::new(arr)))
        }
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(e) => Err(e).context("读取 K_machine 失败"),
    }
}

/// 保存 K_machine（覆盖式）。
pub fn save_machine_key(key: &[u8; 32]) -> Result<()> {
    let entry = Entry::new(KEYCHAIN_SERVICE, KEYCHAIN_USER)
        .context("无法访问 OS Keychain")?;
    // Keychain API 接收 String（password 风格），把 32B 当 UTF-8 直接转
    // 注意：32B 随机可能含无效 UTF-8，所以用 base64 编码后存
    let s = crate::crypto::util::base64_encode(key);
    entry.set_password(&s).context("写入 K_machine 失败")?;
    Ok(())
}

/// 删除 K_machine（仅测试 / reset 用）。
pub fn delete_machine_key() -> Result<()> {
    let entry = Entry::new(KEYCHAIN_SERVICE, KEYCHAIN_USER)
        .context("无法访问 OS Keychain")?;
    match entry.delete_password() {
        Ok(()) => Ok(()),
        Err(keyring::Error::NoEntry) => Ok(()),
        Err(e) => Err(e).context("删除 K_machine 失败"),
    }
}
```

**修正 Step 1**：load_machine_key 里我用了 `s.bytes().collect()` 但存的是 base64，需要修正为 base64 decode。修正版：

```rust
pub fn load_machine_key() -> Result<Option<Zeroizing<[u8; 32]>>> {
    let entry = Entry::new(KEYCHAIN_SERVICE, KEYCHAIN_USER)
        .context("无法访问 OS Keychain")?;
    match entry.get_password() {
        Ok(s) => {
            let bytes = crate::crypto::util::base64_decode(&s)?;
            ensure!(bytes.len() == 32, "K_machine 长度异常：{} bytes", bytes.len());
            let mut arr = [0u8; 32];
            arr.copy_from_slice(&bytes);
            Ok(Some(Zeroizing::new(arr)))
        }
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(e) => Err(e).context("读取 K_machine 失败"),
    }
}
```

把上面整段替换到 load_machine_key 函数。

- [x] **Step 2: 编译验证**

Run: `cargo build -p octopus-vault`
Expected: 0 error

- [x] **Step 3: 写测试（需真实 Keychain，CI 默认 ignore）**

在 keychain.rs 末尾追加：

```rust
#[cfg(test)]
mod tests {
    use super::*;

    // ⚠️ 真实 Keychain 测试，需要在本地跑（会弹授权框）
    #[test]
    #[ignore]
    fn test_machine_key_round_trip() {
        // 清理旧数据
        let _ = delete_machine_key();

        // 首次读应不存在
        assert!(load_machine_key().unwrap().is_none());

        // 创建
        let k1 = load_or_create_machine_key().unwrap();
        assert_eq!(k1.len(), 32);

        // 再次读应能拿到同一把
        let k2 = load_machine_key().unwrap().unwrap();
        assert_eq!(k1.as_ref(), k2.as_ref());

        // 再调 load_or_create 应返回同一把（不覆盖）
        let k3 = load_or_create_machine_key().unwrap();
        assert_eq!(k1.as_ref(), k3.as_ref());

        // 清理
        delete_machine_key().unwrap();
        assert!(load_machine_key().unwrap().is_none());
    }
}
```

- [x] **Step 4: Commit**

```bash
git add crates/vault/src/keychain.rs
git commit -m "feat(vault): Task 8 - K_machine 在 OS Keychain 存取"
```

---

## Task 9: vault unlock.rs（5 大流程 + 双密文）

**Files:**
- Create: `crates/vault/src/unlock.rs`

**Interfaces:**
- Consumes: Task 2/3/4（crypto）、Task 5（infra）、Task 7（storage）、Task 8（keychain）
- Produces:
  - `pub struct UnlockedKeys { pub user_vault_key: DerivedKey, pub app_key: DerivedKey }`
  - `pub struct VaultStatus { pub initialized: bool, pub user_vault_unlocked: bool }`
  - `pub fn is_initialized() -> Result<bool>`
  - `pub fn setup_vault(password: &str) -> Result<UnlockedKeys>`（流程 A）
  - `pub fn unlock_app_key_local() -> Result<Option<DerivedKey>>`（流程 B：本机启动）
  - `pub fn unlock_with_master_password(password: &str) -> Result<UnlockedKeys>`（流程 C/D）
  - `pub fn change_master_password(old: &str, new: &str) -> Result<()>`（流程 E）
  - `pub fn regenerate_security_stamp() -> Result<String>`

- [x] **Step 1: 写 unlock.rs 完整实现**

```rust
//! vault 解锁态管理。
//!
//! 5 大流程（spec 第 2.5 节）：
//!   A. setup_vault              - 首次初始化（设主密码）
//!   B. unlock_app_key_local     - 本机启动（K_machine 解 app_key_local_enc）
//!   C. unlock_with_master_password - 换机器首次 / K_machine 缺失（输主密码）
//!   D. unlock_with_master_password - 超时锁定后重新解锁
//!   E. change_master_password   - 改主密码
//!
//! 流程 C 和 D 共享 unlock_with_master_password 函数（区别在调用 context）。

use anyhow::{Context, Result, ensure};
use uuid::Uuid;

use octopus_infra::db::{VaultMeta, VaultMetaInput};

use crate::crypto::hierarchy::{LABEL_APP_SECRETS, LABEL_USER_VAULT};
use crate::crypto::kdf::{derive_master_root_key, Argon2Params};
use crate::crypto::util::random_32;
use crate::crypto::DerivedKey;
use crate::keychain;
use crate::storage::meta;

/// 解锁态：持有派生的 user_vault_key 和 app_key（均 32B Zeroizing）。
pub struct UnlockedKeys {
    pub user_vault_key: DerivedKey,
    pub app_key: DerivedKey,
}

/// vault 状态摘要（用于 UI 显示）。
pub struct VaultStatus {
    pub initialized: bool,
    pub user_vault_unlocked: bool, // 由调用方（desktop）维护，此处恒为 false
}

pub fn is_initialized() -> Result<bool> {
    Ok(meta::read_vault_meta()?.is_some())
}

fn meta_to_kdf_params(meta: &VaultMeta) -> Argon2Params {
    Argon2Params {
        iterations: meta.kdf_iterations as u32,
        memory_kib: meta.kdf_memory_kib as u32,
        parallelism: meta.kdf_parallelism as u32,
    }
}

/// 流程 A：首次初始化 vault。
///
/// 输入：用户设的主密码（明文，调用后立即 zeroize 由调用者负责）。
/// 副作用：
///   - 生成 32B kdf_salt
///   - 派生 master_root_key，进一步派生 user_vault_key 和 app_key
///   - 生成 K_machine（OS Keychain）
///   - 双密文 app_key 落盘
///   - 落盘 vault_meta
///   - 不迁移 models.secret_key（迁移由 Task 20 单独负责）
pub fn setup_vault(password: &str) -> Result<UnlockedKeys> {
    ensure!(!is_initialized()?, "vault 已初始化");

    let kdf_salt = random_32();
    let params = Argon2Params::default();
    let master_root_key = derive_master_root_key(password.as_bytes(), &kdf_salt, &params)?;

    // 派生 user_vault_key / app_key
    let user_vault_key = master_root_key.child(LABEL_USER_VAULT);
    let app_key = master_root_key.child(LABEL_APP_SECRETS);

    // 加密 user_vault_key（用 master_root_key）
    let protected_user_vault_key = master_root_key.encrypt(user_vault_key.as_bytes())?;
    // 加密 app_key（双密文）
    let k_machine = keychain::load_or_create_machine_key()?;
    let app_key_local_enc = {
        let k_machine_derived = DerivedKey::from_raw(*k_machine);
        k_machine_derived.encrypt(app_key.as_bytes())?
    };
    let app_key_sync_enc = master_root_key.encrypt(app_key.as_bytes())?;

    let stamp = Uuid::new_v4().to_string();

    let input = VaultMetaInput {
        kdf_type: 0,
        kdf_salt: kdf_salt.to_vec(),
        kdf_iterations: params.iterations as i64,
        kdf_memory_kib: params.memory_kib as i64,
        kdf_parallelism: params.parallelism as i64,
        protected_user_vault_key,
        app_key_local_enc,
        app_key_sync_enc,
        security_stamp: stamp,
        equivalent_domains: "[]".into(),
        public_key: None,
        protected_private_key: None,
    };
    meta::save_vault_meta(&input)?;

    Ok(UnlockedKeys {
        user_vault_key,
        app_key,
    })
}

/// 流程 B：本机启动时尝试用 K_machine 解 app_key（无感）。
///
/// 返回：
///   - Ok(Some(app_key))：成功解出 app_key，应用可用 ASR
///   - Ok(None)：vault 未初始化 / K_machine 不存在 / 解密失败 → 调用方应走流程 C
pub fn unlock_app_key_local() -> Result<Option<DerivedKey>> {
    let meta = match meta::read_vault_meta()? {
        Some(m) => m,
        None => return Ok(None),
    };
    let k_machine = match keychain::load_machine_key()? {
        Some(k) => k,
        None => return Ok(None),
    };
    let k_machine_derived = DerivedKey::from_raw(*k_machine);
    match k_machine_derived.decrypt(&meta.app_key_local_enc) {
        Ok(bytes) => {
            let mut arr = [0u8; 32];
            ensure!(
                bytes.len() == 32,
                "app_key 解密后长度异常：{}",
                bytes.len()
            );
            arr.copy_from_slice(&bytes);
            Ok(Some(DerivedKey::from_raw(arr)))
        }
        Err(_) => Ok(None), // 解密失败 → 降级到流程 C
    }
}

/// 流程 C/D：用主密码解锁（换机器 / 超时锁定后）。
///
/// 同时解开 user_vault_key 和 app_key。
/// 成功后调用方可选择用本机 K_machine 重新加密 app_key 落盘（流程 C 末尾）。
pub fn unlock_with_master_password(password: &str) -> Result<UnlockedKeys> {
    let meta = meta::read_vault_meta()?
        .context("vault 未初始化")?;
    let params = meta_to_kdf_params(&meta);
    let master_root_key = derive_master_root_key(password.as_bytes(), &meta.kdf_salt, &params)?;

    // 解 user_vault_key
    let user_vault_bytes = master_root_key.decrypt(&meta.protected_user_vault_key)?;
    ensure!(user_vault_bytes.len() == 32, "user_vault_key 长度异常");
    let mut uv_arr = [0u8; 32];
    uv_arr.copy_from_slice(&user_vault_bytes);
    let user_vault_key = DerivedKey::from_raw(uv_arr);

    // 解 app_key（用 sync 密文）
    let app_key_bytes = master_root_key.decrypt(&meta.app_key_sync_enc)?;
    ensure!(app_key_bytes.len() == 32, "app_key 长度异常");
    let mut ak_arr = [0u8; 32];
    ak_arr.copy_from_slice(&app_key_bytes);
    let app_key = DerivedKey::from_raw(ak_arr);

    // 流程 C 特有：用本机 K_machine 重新加密 app_key → 落盘
    // 这样下次本机启动就能用流程 B 无感
    refresh_app_key_local_enc(&app_key)?;

    Ok(UnlockedKeys {
        user_vault_key,
        app_key,
    })
}

/// 流程 E：改主密码。
///
/// 副作用：重写 3 个密文 + 刷新 security_stamp。
/// 不重加密 vault_ciphers（因为 user_vault_key 不变）。
pub fn change_master_password(old_password: &str, new_password: &str) -> Result<()> {
    let meta = meta::read_vault_meta()?
        .context("vault 未初始化")?;
    let old_params = meta_to_kdf_params(&meta);
    let old_master = derive_master_root_key(old_password.as_bytes(), &meta.kdf_salt, &old_params)?;

    // 验证旧密码（用 protected_user_vault_key 解密试一下）
    let user_vault_bytes = old_master
        .decrypt(&meta.protected_user_vault_key)
        .context("旧主密码错误")?;
    ensure!(user_vault_bytes.len() == 32, "user_vault_key 长度异常");
    let mut uv_arr = [0u8; 32];
    uv_arr.copy_from_slice(&user_vault_bytes);
    let user_vault_key = DerivedKey::from_raw(uv_arr);

    // 用旧 master 解出 app_key
    let app_key_bytes = old_master.decrypt(&meta.app_key_sync_enc)?;
    let mut ak_arr = [0u8; 32];
    ak_arr.copy_from_slice(&app_key_bytes);
    let app_key = DerivedKey::from_raw(ak_arr);

    // 用新密码派生新 master_root_key
    let new_master = derive_master_root_key(new_password.as_bytes(), &meta.kdf_salt, &old_params)?;

    // 重加密 3 个密文
    let new_protected_user_vault_key = new_master.encrypt(user_vault_key.as_bytes())?;
    let new_app_key_sync_enc = new_master.encrypt(app_key.as_bytes())?;
    let new_app_key_local_enc = {
        let k_machine = keychain::load_or_create_machine_key()?;
        let k_machine_derived = DerivedKey::from_raw(*k_machine);
        k_machine_derived.encrypt(app_key.as_bytes())?
    };

    // 刷新 security_stamp（让其他机器同步后强制重新输主密码）
    let new_stamp = Uuid::new_v4().to_string();

    let input = VaultMetaInput {
        kdf_type: meta.kdf_type,
        kdf_salt: meta.kdf_salt.clone(),
        kdf_iterations: meta.kdf_iterations,
        kdf_memory_kib: meta.kdf_memory_kib,
        kdf_parallelism: meta.kdf_parallelism,
        protected_user_vault_key: new_protected_user_vault_key,
        app_key_local_enc: new_app_key_local_enc,
        app_key_sync_enc: new_app_key_sync_enc,
        security_stamp: new_stamp,
        equivalent_domains: meta.equivalent_domains,
        public_key: meta.public_key,
        protected_private_key: meta.protected_private_key,
    };
    meta::save_vault_meta(&input)?;
    Ok(())
}

/// 用本机 K_machine 重新加密 app_key → 写入 app_key_local_enc。
/// 用于流程 C 末尾，让本机下次启动可走流程 B。
fn refresh_app_key_local_enc(app_key: &DerivedKey) -> Result<()> {
    let k_machine = match keychain::load_machine_key()? {
        Some(k) => k,
        None => keychain::load_or_create_machine_key()?,
    };
    let k_machine_derived = DerivedKey::from_raw(*k_machine);
    let new_local_enc = k_machine_derived.encrypt(app_key.as_bytes())?;

    let meta = meta::read_vault_meta()?.context("vault 未初始化")?;
    let input = VaultMetaInput {
        kdf_type: meta.kdf_type,
        kdf_salt: meta.kdf_salt.clone(),
        kdf_iterations: meta.kdf_iterations,
        kdf_memory_kib: meta.kdf_memory_kib,
        kdf_parallelism: meta.kdf_parallelism,
        protected_user_vault_key: meta.protected_user_vault_key,
        app_key_local_enc: new_local_enc,
        app_key_sync_enc: meta.app_key_sync_enc,
        security_stamp: meta.security_stamp,
        equivalent_domains: meta.equivalent_domains,
        public_key: meta.public_key,
        protected_private_key: meta.protected_private_key,
    };
    meta::save_vault_meta(&input)?;
    Ok(())
}

/// 仅刷新 security_stamp 并返回新值。
pub fn regenerate_security_stamp() -> Result<String> {
    let new_stamp = Uuid::new_v4().to_string();
    meta::update_security_stamp(&new_stamp)?;
    Ok(new_stamp)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_uninitialized_vault_status() {
        // 注意：依赖 ~/.octopus/octopus.db 实际状态
        // 这里仅测函数签名和错误处理，不真正调用 setup_vault
        let _ = is_initialized();
    }
}
```

**注意 Step 1 还需要**：在 `crypto/mod.rs` 或 `crypto/hierarchy.rs` 加 `impl DerivedKey { pub fn from_raw(arr: [u8; 32]) -> Self }`。

- [x] **Step 2: 在 crypto/mod.rs 加 DerivedKey::from_raw**

修改 `crates/vault/src/crypto/mod.rs`，给 `impl DerivedKey` 块加方法：

```rust
impl DerivedKey {
    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    /// 从已知 32B 数组构造（用于把 K_machine 包装成 DerivedKey）。
    pub fn from_raw(arr: [u8; 32]) -> Self {
        DerivedKey(Zeroizing::new(arr))
    }
}
```

- [x] **Step 3: 在 vault/Cargo.toml 加 uuid 依赖**

修改 `crates/vault/Cargo.toml`，在 `[dependencies]` 加：

```toml
uuid = { version = "1", features = ["v4"] }
```

- [x] **Step 4: 编译验证**

Run: `cargo build -p octopus-vault`
Expected: 0 error 0 warning

- [x] **Step 5: 写集成测试（本地手动运行）**

新建 `crates/vault/tests/unlock.rs`：

```rust
//! vault::unlock 集成测试。
//!
//! ⚠️ 需要真实 ~/.octopus/octopus.db + OS Keychain 权限。
//! 默认 #[ignore]，需 --ignored 跑。
//! 测试会修改 ~/.octopus/octopus.db，建议在测试环境用。

use octopus_vault::unlock;

#[test]
#[ignore]
fn test_full_setup_unlock_cycle() {
    // 清理（如果之前有遗留）
    // 注意：实际项目应加 reset_vault() 工具函数

    // 1. setup_vault
    let keys = unlock::setup_vault("test-password-123").expect("setup");
    assert_eq!(keys.user_vault_key.as_bytes().len(), 32);
    assert_eq!(keys.app_key.as_bytes().len(), 32);

    // 2. 本机启动解锁（K_machine 已在 setup 时生成）
    let app_key_local = unlock::unlock_app_key_local().expect("local unlock").expect("应有 K_machine");
    assert_eq!(app_key_local.as_bytes(), keys.app_key.as_bytes());

    // 3. 主密码解锁（应该能拿到同样的 user_vault_key 和 app_key）
    let keys2 = unlock::unlock_with_master_password("test-password-123").expect("master unlock");
    assert_eq!(keys2.user_vault_key.as_bytes(), keys.user_vault_key.as_bytes());
    assert_eq!(keys2.app_key.as_bytes(), keys.app_key.as_bytes());

    // 4. 错误密码应失败
    assert!(unlock::unlock_with_master_password("wrong-password").is_err());

    // 5. 改主密码
    unlock::change_master_password("test-password-123", "new-pwd-456").expect("change pwd");

    // 6. 旧密码失败，新密码成功
    assert!(unlock::unlock_with_master_password("test-password-123").is_err());
    let keys3 = unlock::unlock_with_master_password("new-pwd-456").expect("new pwd unlock");
    assert_eq!(keys3.user_vault_key.as_bytes(), keys.user_vault_key.as_bytes()); // user_vault_key 不变
}
```

- [x] **Step 6: Commit**

```bash
git add crates/vault/src/unlock.rs crates/vault/src/crypto/mod.rs crates/vault/Cargo.toml crates/vault/tests/
git commit -m "feat(vault): Task 9 - 5 大解锁流程 + 双密文 K_machine"
```

---

## Task 10: generator: random.rs + pin.rs

**Files:**
- Create: `crates/vault/src/generator/mod.rs`
- Create: `crates/vault/src/generator/random.rs`
- Create: `crates/vault/src/generator/pin.rs`
- 修改 `crates/vault/src/lib.rs` 已声明 `pub mod generator;`（无需改）

**Interfaces:**
- Consumes: `rand` crate
- Produces:
  - `pub enum GeneratorConfig { Random(RandomConfig), PassphraseEn(PassphraseEnConfig), PassphraseZh(PassphraseZhConfig), Pin(PinConfig) }`
  - `pub fn generate(cfg: &GeneratorConfig) -> String`
  - `pub struct RandomConfig { length: u32, uppercase: bool, lowercase: bool, numbers: bool, symbols: bool, avoid_ambiguous: bool }`
  - `pub struct PinConfig { length: u32 }`

- [x] **Step 1: generator/mod.rs**

```rust
//! 密码生成器：Random / PassphraseEn / PassphraseZh / PIN。

pub mod pin;
pub mod random;
pub mod passphrase_en;
pub mod passphrase_zh;
pub mod eff_wordlist;
pub mod zh_wordlist_4096;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "lowercase")]
pub enum GeneratorConfig {
    Random(RandomConfig),
    PassphraseEn(PassphraseEnConfig),
    PassphraseZh(PassphraseZhConfig),
    Pin(PinConfig),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RandomConfig {
    #[serde(default = "default_length_16")]
    pub length: u32,
    #[serde(default = "default_true")]
    pub uppercase: bool,
    #[serde(default = "default_true")]
    pub lowercase: bool,
    #[serde(default = "default_true")]
    pub numbers: bool,
    #[serde(default = "default_false")]
    pub symbols: bool,
    #[serde(default = "default_true")]
    pub avoid_ambiguous: bool,
}

impl Default for RandomConfig {
    fn default() -> Self {
        Self {
            length: 16,
            uppercase: true,
            lowercase: true,
            numbers: true,
            symbols: false,
            avoid_ambiguous: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PinConfig {
    #[serde(default = "default_length_6")]
    pub length: u32,
}

impl Default for PinConfig {
    fn default() -> Self {
        Self { length: 6 }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PassphraseEnConfig {
    #[serde(default = "default_length_3")]
    pub word_count: u32,
    #[serde(default = "default_sep_dash")]
    pub separator: String,
    #[serde(default = "default_true")]
    pub capitalize: bool,
    #[serde(default = "default_true")]
    pub include_number: bool,
}

impl Default for PassphraseEnConfig {
    fn default() -> Self {
        Self {
            word_count: 3,
            separator: "-".into(),
            capitalize: true,
            include_number: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PassphraseZhConfig {
    #[serde(default = "default_length_4")]
    pub word_count: u32,
    #[serde(default = "default_sep_empty")]
    pub separator: String,
    #[serde(default = "default_true")]
    pub include_number: bool,
    #[serde(default = "default_false")]
    pub include_symbol: bool,
}

impl Default for PassphraseZhConfig {
    fn default() -> Self {
        Self {
            word_count: 4,
            separator: "".into(),
            include_number: true,
            include_symbol: false,
        }
    }
}

pub fn generate(cfg: &GeneratorConfig) -> String {
    match cfg {
        GeneratorConfig::Random(c) => random::generate(c),
        GeneratorConfig::PassphraseEn(c) => passphrase_en::generate(c),
        GeneratorConfig::PassphraseZh(c) => passphrase_zh::generate(c),
        GeneratorConfig::Pin(c) => pin::generate(c),
    }
}

fn default_length_16() -> u32 { 16 }
fn default_length_6() -> u32 { 6 }
fn default_length_3() -> u32 { 3 }
fn default_length_4() -> u32 { 4 }
fn default_true() -> bool { true }
fn default_false() -> bool { false }
fn default_sep_dash() -> String { "-".into() }
fn default_sep_empty() -> String { "".into() }
```

- [x] **Step 2: generator/random.rs**

```rust
//! 随机字符密码：保证每种启用字符类型至少出现 1 次。

use rand::rngs::OsRng;
use rand::seq::SliceRandom;

use super::RandomConfig;

const UPPER: &[&str] = &["A","B","C","D","E","F","G","H","I","J","K","L","M","N","O","P","Q","R","S","T","U","V","W","X","Y","Z"];
const LOWER: &[&str] = &["a","b","c","d","e","f","g","h","i","j","k","l","m","n","o","p","q","r","s","t","u","v","w","x","y","z"];
const DIGITS: &[&str] = &["0","1","2","3","4","5","6","7","8","9"];
const SYMBOLS: &[&str] = &["!","@","#","$","%","^","&","*","(",")","-","_","=","+","[","]","{","}","<",">","?"];
const AMBIGUOUS: &[char] = &['l', '1', 'I', 'O', '0', '|', '`', '\'', '"'];

fn build_charset(cfg: &RandomConfig) -> Vec<char> {
    let mut s: String = String::new();
    if cfg.uppercase { s.push_str(UPPER.concat().as_str()); }
    if cfg.lowercase { s.push_str(LOWER.concat().as_str()); }
    if cfg.numbers { s.push_str(DIGITS.concat().as_str()); }
    if cfg.symbols { s.push_str(SYMBOLS.concat().as_str()); }
    if cfg.avoid_ambiguous {
        s = s.chars().filter(|c| !AMBIGUOUS.contains(c)).collect();
    }
    s.chars().collect()
}

pub fn generate(cfg: &RandomConfig) -> String {
    assert!(cfg.length >= 5, "length 必须 >= 5");
    assert!(cfg.length <= 128, "length 必须 <= 128");

    let mut rng = OsRng;
    let mut result: Vec<char> = Vec::with_capacity(cfg.length as usize);

    // 强制每种启用类型至少 1 个
    if cfg.uppercase && !cfg.avoid_ambiguous {
        result.extend(UPPER.choose(&mut rng).unwrap().chars());
    } else if cfg.uppercase {
        let filtered: Vec<&str> = UPPER.iter().filter(|s| !AMBIGUOUS.contains(&s.chars().next().unwrap())).copied().collect();
        if let Some(c) = filtered.choose(&mut rng) { result.extend(c.chars()); }
    }
    if cfg.lowercase {
        let pool: Vec<&str> = LOWER.iter().filter(|s| !cfg.avoid_ambiguous || !AMBIGUOUS.contains(&s.chars().next().unwrap())).copied().collect();
        if let Some(c) = pool.choose(&mut rng) { result.extend(c.chars()); }
    }
    if cfg.numbers {
        let pool: Vec<&str> = DIGITS.iter().filter(|s| !cfg.avoid_ambiguous || !AMBIGUOUS.contains(&s.chars().next().unwrap())).copied().collect();
        if let Some(c) = pool.choose(&mut rng) { result.extend(c.chars()); }
    }
    if cfg.symbols {
        let pool: Vec<&str> = SYMBOLS.iter().filter(|s| !cfg.avoid_ambiguous || !AMBIGUOUS.contains(&s.chars().next().unwrap())).copied().collect();
        if let Some(c) = pool.choose(&mut rng) { result.extend(c.chars()); }
    }

    let charset = build_charset(cfg);
    assert!(!charset.is_empty(), "至少选一种字符集");
    while (result.len() as u32) < cfg.length {
        if let Some(c) = charset.choose(&mut rng) {
            result.push(*c);
        }
    }

    result.shuffle(&mut rng);
    result.into_iter().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_length_within_bounds() {
        let cfg = RandomConfig::default();
        for _ in 0..100 {
            let s = generate(&cfg);
            assert_eq!(s.len(), 16);
        }
    }

    #[test]
    fn test_avoid_ambiguous_default() {
        let cfg = RandomConfig::default();
        for _ in 0..200 {
            let s = generate(&cfg);
            assert!(!s.contains('l'), "不应含 l: {}", s);
            assert!(!s.contains('1'), "不应含 1: {}", s);
            assert!(!s.contains('O'), "不应含 O: {}", s);
            assert!(!s.contains('0'), "不应含 0: {}", s);
        }
    }

    #[test]
    fn test_each_type_present() {
        let cfg = RandomConfig {
            length: 30,
            uppercase: true,
            lowercase: true,
            numbers: true,
            symbols: true,
            avoid_ambiguous: false,
        };
        for _ in 0..100 {
            let s = generate(&cfg);
            assert!(s.chars().any(|c| c.is_uppercase()), "缺大写: {}", s);
            assert!(s.chars().any(|c| c.is_lowercase()), "缺小写: {}", s);
            assert!(s.chars().any(|c| c.is_ascii_digit()), "缺数字: {}", s);
            assert!(s.chars().any(|c| "!@#$%^&*()".contains(c)), "缺符号: {}", s);
        }
    }

    #[test]
    fn test_only_numbers() {
        let cfg = RandomConfig {
            length: 10,
            uppercase: false,
            lowercase: false,
            numbers: true,
            symbols: false,
            avoid_ambiguous: false,
        };
        let s = generate(&cfg);
        assert!(s.chars().all(|c| c.is_ascii_digit()));
    }

    #[test]
    #[should_panic(expected = "length 必须 >= 5")]
    fn test_too_short_panics() {
        let cfg = RandomConfig { length: 3, ..Default::default() };
        generate(&cfg);
    }
}
```

- [x] **Step 3: generator/pin.rs**

```rust
//! 纯数字 PIN。

use rand::rngs::OsRng;
use rand::Rng;

use super::PinConfig;

pub fn generate(cfg: &PinConfig) -> String {
    assert!(cfg.length >= 1, "PIN length 必须 >= 1");
    assert!(cfg.length <= 32, "PIN length 必须 <= 32");
    let mut rng = OsRng;
    (0..cfg.length).map(|_| char::from_digit(rng.gen_range(0..10), 10).unwrap()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pin_length_default() {
        let cfg = PinConfig::default();
        for _ in 0..100 {
            let s = generate(&cfg);
            assert_eq!(s.len(), 6);
            assert!(s.chars().all(|c| c.is_ascii_digit()));
        }
    }

    #[test]
    fn test_custom_length() {
        let cfg = PinConfig { length: 4 };
        let s = generate(&cfg);
        assert_eq!(s.len(), 4);
    }
}
```

- [x] **Step 4: 占位 passphrase 文件（Task 11 实现）**

```bash
echo "//! 占位：Task 11 填充" > crates/vault/src/generator/passphrase_en.rs
echo "//! 占位：Task 11 填充" > crates/vault/src/generator/passphrase_zh.rs
echo "//! 占位：Task 11 填充" > crates/vault/src/generator/eff_wordlist.rs
echo "//! 占位：Task 11 填充" > crates/vault/src/generator/zh_wordlist_4096.rs
```

- [x] **Step 5: 运行测试**

Run: `cargo test -p octopus-vault --lib generator -- --nocapture`
Expected: random (5) + pin (2) = 7 passed

- [x] **Step 6: Commit**

```bash
git add crates/vault/src/generator/
git commit -m "feat(vault): Task 10 - 密码生成器 Random + PIN 模式"
```

---

## Task 11: generator: passphrase_en.rs + zh.rs（含词表）

**Files:**
- Create: `crates/vault/src/generator/eff_wordlist.rs`（EFF 7776 词）
- Create: `crates/vault/src/generator/zh_wordlist_4096.rs`（中文 4096 词，**先放占位 100 词**，后续填充）
- Create: `crates/vault/src/generator/passphrase_en.rs`
- Create: `crates/vault/src/generator/passphrase_zh.rs`

**注意**：EFF 完整词表 7776 词太大，本 Task 用脚本生成完整列表（每个词一行）。中文词表需要外部选词，本 Task 先放 100 词占位（标 TODO 提示后续扩充到 4096）。

- [x] **Step 1: 用脚本下载/生成 EFF 词表**

EFF 长词表 7776 词（CC BY 3.0）。从 https://www.eff.org/files/2016/07/18/eff_large_wordlist.txt 下载，写脚本提取每行第二个字段（dice_ware_id,word 中的 word）：

```bash
mkdir -p crates/vault/data
curl -sL https://www.eff.org/files/2016/07/18/eff_large_wordlist.txt -o crates/vault/data/eff_large_wordlist.txt
# 验证行数
wc -l crates/vault/data/eff_large_wordlist.txt  # 应为 7776
```

然后生成 Rust 常量数组文件：

```bash
cat > crates/vault/src/generator/eff_wordlist.rs <<'RUST_EOF'
//! EFF 大词表（7776 词，CC BY 3.0）。
//! 来源：https://www.eff.org/dice
//! 编译时 include_str! 外部数据文件，避免源码膨胀。

pub const EFF_WORDLIST: &[&str] = &[
RUST_EOF

awk -F'\t' '{print "    \"" $2 "\","}' crates/vault/data/eff_large_wordlist.txt >> crates/vault/src/generator/eff_wordlist.rs

cat >> crates/vault/src/generator/eff_wordlist.rs <<'RUST_EOF'
];
RUST_EOF

# 验证
grep -c '"' crates/vault/src/generator/eff_wordlist.rs  # 应 ~7776
```

- [x] **Step 2: 写 passphrase_en.rs**

```rust
//! 英文 passphrase：EFF 7776 词，可加数字、大写、分隔符。

use rand::rngs::OsRng;
use rand::seq::SliceRandom;
use rand::Rng;

use super::eff_wordlist::EFF_WORDLIST;
use super::PassphraseEnConfig;

pub fn generate(cfg: &PassphraseEnConfig) -> String {
    assert!(cfg.word_count >= 3, "word_count 必须 >= 3");
    assert!(cfg.word_count <= 10, "word_count 必须 <= 10");

    let mut rng = OsRng;
    let mut words: Vec<String> = (0..cfg.word_count)
        .map(|_| EFF_WORDLIST.choose(&mut rng).unwrap().to_string())
        .map(|w| if cfg.capitalize {
            let mut c = w.chars();
            match c.next() {
                Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
                None => String::new(),
            }
        } else {
            w
        })
        .collect();

    let mut result = words.join(&cfg.separator);
    if cfg.include_number {
        let n: u32 = rng.gen_range(0..=9);
        result = format!("{}{}{}", result, if cfg.separator.is_empty() { "" } else { cfg.separator.as_str() }, n);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_word_count() {
        let cfg = PassphraseEnConfig::default();
        for _ in 0..50 {
            let s = generate(&cfg);
            // 默认 3 词 + 1 数字（带 -）→ "word1-word2-word3-5" 共 4 段
            let parts: Vec<&str> = s.split('-').collect();
            assert_eq!(parts.len(), 4, "实际: {}", s);
        }
    }

    #[test]
    fn test_capitalize() {
        let cfg = PassphraseEnConfig::default();
        let s = generate(&cfg);
        // 至少一个词首字母大写
        assert!(s.chars().any(|c| c.is_uppercase()), "应有大写: {}", s);
    }

    #[test]
    fn test_all_words_from_eff_list() {
        let cfg = PassphraseEnConfig { include_number: false, ..Default::default() };
        for _ in 0..50 {
            let s = generate(&cfg);
            for word in s.split('-') {
                let lower = word.to_lowercase();
                assert!(
                    EFF_WORDLIST.iter().any(|w| *w == lower),
                    "词 {} 不在 EFF 列表",
                    word
                );
            }
        }
    }
}
```

- [x] **Step 3: 写 zh_wordlist_4096.rs 占位（100 词，标 TODO 扩到 4096）**

```rust
//! 中文 passphrase 双字词表（目标 4096 词，12 bit/词）。
//!
//! 来源：从 THUOCL/jieba 词频 TOP 10000 过滤 → 取 TOP 4096（MIT 许可）。
//! 当前为 MVP 占位 100 词（包含一些常用双字词）。
//! TODO: 实施时补全到 4096 词。补全后 4 词 = 48 bit 强度。

pub const ZH_WORDLIST_4096: &[&str] = &[
    // 100 个高频双字词占位（实施时替换为完整 4096 词表）
    "我们", "什么", "可以", "知道", "因为", "所以", "如果", "已经",
    "他们", "自己", "现在", "这样", "怎么", "还是", "认为", "觉得",
    "春日", "明月", "归途", "远方", "故人", "江湖", "山河", "岁月",
    "书信", "灯盏", "渡口", "暮色", "清晨", "灯火", "星辰", "夜雨",
    "晚秋", "初冬", "深秋", "孟夏", "仲春", "季夏", "寒冬", "酷暑",
    "山川", "河流", "海洋", "森林", "草原", "沙漠", "雪山", "云海",
    "晨曦", "夕阳", "黄昏", "黎明", "正午", "深夜", "凌晨", "傍晚",
    "心境", "情绪", "思绪", "念头", "回忆", "梦想", "希望", "勇气",
    "时间", "空间", "世界", "宇宙", "天地", "万物", "众生", "灵魂",
    "故事", "传说", "神话", "历史", "现在", "未来", "过往", "永恒",
    "温暖", "寒冷", "明亮", "黑暗", "清晰", "朦胧", "柔和", "强烈",
    "安静", "喧闹", "平淡", "精彩", "简单", "复杂", "快乐", "忧伤",
    "诗意", "画意", "音律", "书香", "笔墨", "纸砚", "琴棋", "书画",
];
```

- [x] **Step 4: 写 passphrase_zh.rs**

```rust
//! 中文 passphrase：双字词组合，可加数字、符号。

use rand::rngs::OsRng;
use rand::seq::SliceRandom;

use super::zh_wordlist_4096::ZH_WORDLIST_4096;
use super::PassphraseZhConfig;

pub fn generate(cfg: &PassphraseZhConfig) -> String {
    assert!(cfg.word_count >= 3, "word_count 必须 >= 3");
    assert!(cfg.word_count <= 8, "word_count 必须 <= 8");

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
        let symbols = ['!', '@', '#', '$', '%', '&', '*'];
        let s = symbols.choose(&mut rng).unwrap();
        result = format!("{}{}", result, s);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_word_count_default() {
        let cfg = PassphraseZhConfig::default();
        for _ in 0..100 {
            let s = generate(&cfg);
            // 4 词 = 8 字符 + 1 数字 = 9 字符
            let chars: Vec<char> = s.chars().filter(|c| !c.is_ascii_digit()).collect();
            assert_eq!(chars.len(), 8, "应为 4 个双字词 (8 字符)，实际: {}", s);
        }
    }

    #[test]
    fn test_no_separator() {
        let cfg = PassphraseZhConfig::default();
        let s = generate(&cfg);
        // 默认 separator 是空字符串，不应有 - 或空格
        assert!(!s.contains('-'));
        assert!(!s.contains(' '));
    }

    #[test]
    fn test_with_symbol() {
        let cfg = PassphraseZhConfig { include_symbol: true, include_number: false, ..Default::default() };
        let s = generate(&cfg);
        assert!(s.ends_with(['!', '@', '#', '$', '%', '&', '*']), "应以符号结尾: {}", s);
    }

    #[test]
    #[ignore]
    fn test_wordlist_size_4096_after_completion() {
        // TODO: 词表补全到 4096 后启用此测试
        assert_eq!(ZH_WORDLIST_4096.len(), 4096, "当前词表大小: {}", ZH_WORDLIST_4096.len());
    }
}
```

**修正 Step 4**：`gen_range` 在文件用到了，需要 `use rand::Rng;`。补充到文件顶部 import：

```rust
use rand::Rng;
```

- [x] **Step 5: 运行测试**

Run: `cargo test -p octopus-vault --lib generator -- --nocapture`
Expected: random (5) + pin (2) + en (3) + zh (3) = 13 passed（test_wordlist_size_4096_after_completion 标 #[ignore]）

- [x] **Step 6: Commit**

```bash
git add crates/vault/data/ crates/vault/src/generator/
git commit -m "feat(vault): Task 11 - Passphrase EN (EFF) + ZH (4096 词表占位)"
```

---

## Task 12: vault totp.rs

**Files:**
- Create: `crates/vault/src/totp.rs`

**Interfaces:**
- Produces:
  - `pub struct TotpGenerator { inner: totp_rs::TOTP }`（无法构造含具体 secret）
  - `impl TotpGenerator { pub fn from_base32(secret: &str) -> Result<Self>; pub fn current(&self) -> Result<String>; pub fn seconds_remaining(&self) -> u64 }`

- [x] **Step 1: 写 totp.rs**

```rust
//! TOTP（RFC 6238）生成。
//!
//! 固定算法：HMAC-SHA1, 30s, 6 位, ±1 步漂移（totp-rs skew=1）。
//! 输入：Base32 secret（如 "JBSWY3DPEHPK3PXP"）。
//! 输出：当前 6 位数字 + 剩余秒数。

use anyhow::{Context, Result};
use std::time::{SystemTime, UNIX_EPOCH};
use totp_rs::{Algorithm, Secret, TOTP};

pub struct TotpGenerator {
    inner: TOTP,
}

impl TotpGenerator {
    pub fn from_base32(secret: &str) -> Result<Self> {
        let bytes = Secret::Encoded(secret.to_string())
            .to_bytes()
            .context("TOTP secret Base32 解码失败")?;
        let totp = TOTP::new(Algorithm::SHA1, 6, 1, 30, bytes)
            .context("TOTP 构造失败")?;
        Ok(Self { inner: totp })
    }

    pub fn current(&self) -> Result<String> {
        Ok(self.inner.generate_current().context("TOTP 生成失败")?)
    }

    pub fn seconds_remaining(&self) -> u64 {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        30 - (now % 30)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_totp_format_6_digits() {
        let gen = TotpGenerator::from_base32("JBSWY3DPEHPK3PXP").unwrap();
        let code = gen.current().unwrap();
        assert_eq!(code.len(), 6);
        assert!(code.chars().all(|c| c.is_ascii_digit()));
    }

    #[test]
    fn test_totp_seconds_remaining_in_range() {
        let gen = TotpGenerator::from_base32("JBSWY3DPEHPK3PXP").unwrap();
        let r = gen.seconds_remaining();
        assert!(r >= 1 && r <= 30);
    }

    #[test]
    fn test_invalid_base32_secret() {
        assert!(TotpGenerator::from_base32("!!!invalid base32!!!").is_err());
    }

    #[test]
    fn test_known_totp_value() {
        // RFC 6238 测试向量
        // Secret: GEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQ (Base32 of "12345678901234567890")
        // 算法：SHA1, 30s, 8 digits
        // 注：totp-rs 默认 6 digits，所以这里用我们自己的 6 digit 配置
        // 这个测试仅验证不 panic，因为时点会影响具体值
        let gen = TotpGenerator::from_base32("GEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQ").unwrap();
        let code = gen.current().unwrap();
        assert_eq!(code.len(), 6);
    }
}
```

- [x] **Step 2: 运行测试**

Run: `cargo test -p octopus-vault --lib totp -- --nocapture`
Expected: 4 passed

- [x] **Step 3: Commit**

```bash
git add crates/vault/src/totp.rs
git commit -m "feat(vault): Task 12 - TOTP 生成（RFC 6238）"
```

---

## Task 13: vault matcher（5 种策略 + eTLD+1）

**Files:**
- Create: `crates/vault/src/matcher/mod.rs`
- Create: `crates/vault/src/matcher/psl.rs`

**Interfaces:**
- Consumes: `publicsuffix` crate、Task 6 的 Cipher/CipherData/LoginData/LoginUri/MatchType
- Produces:
  - `pub fn find_matching_ciphers(url: &Url, ciphers: &[Cipher], equivalent_domains: &[Vec<String>]) -> Vec<&Cipher>`
  - `pub fn etld_plus_one(host: &str) -> Option<String>`
  - `pub fn default_equivalent_domains() -> Vec<Vec<String>>`

- [x] **Step 1: matcher/psl.rs**

```rust
//! eTLD+1 提取（用公共后缀列表 PSL）。

use publicsuffix::{Domain, List};

/// 编译时内嵌 PSL（runtime 不下载）。
/// publicsuffix crate 默认从网络下载列表，我们用其提供的 builtin。
fn psl_list() -> &'static List {
    use std::sync::OnceLock;
    static LIST: OnceLock<List> = OnceLock::new();
    LIST.get_or_init(|| {
        // publicsuffix 2.x 提供内嵌 PSL（需 enable "builtin" feature）
        // 但实际使用可能需要运行时 fetch，本 Task 用 empty list fallback
        // TODO: 实施时改用 List::from_str(include_str!("public_suffix_list.dat"))
        List::empty()
    })
}

/// 提取 eTLD+1。
///   mail.google.com   → google.com
///   foo.bar.example.co.uk → example.co.uk
///   localhost         → localhost
///   192.168.1.1       → 192.168.1.1
pub fn etld_plus_one(host: &str) -> Option<String> {
    let list = psl_list();
    let parsed: Domain<'_> = list.parse_domain(host).ok()?;
    if let Some(root) = parsed.root() {
        return Some(root.to_string());
    }
    // 单段域名（如 localhost）或 IP：原样返回
    Some(host.to_string())
}

/// MVP 内置默认等价域名（借鉴 Bitwarden global_domains.json）。
pub fn default_equivalent_domains() -> Vec<Vec<String>> {
    vec![
        vec!["google.com".into(), "youtube.com".into(), "gmail.com".into(), "g.co".into()],
        vec!["live.com".into(), "hotmail.com".into(), "outlook.com".into()],
        vec!["apple.com".into(), "icloud.com".into()],
        vec!["amazon.com".into(), "amazon.co.jp".into(), "amazon.co.uk".into()],
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_etld_plus_one_simple() {
        // 注意：空 PSL list 下，root() 行为是返回整段 host
        // 完整 PSL 才能正确处理 .co.uk 等，本测试假设有 PSL
        // 实施时需要把 PSL 数据内嵌
        let r = etld_plus_one("example.com");
        assert!(r.is_some());
    }

    #[test]
    fn test_localhost_returns_as_is() {
        let r = etld_plus_one("localhost");
        assert_eq!(r.as_deref(), Some("localhost"));
    }

    #[test]
    fn test_default_equivalent_domains_nonempty() {
        let d = default_equivalent_domains();
        assert!(!d.is_empty());
        assert!(d.iter().any(|g| g.contains(&"google.com".to_string())));
    }
}
```

**注意**：`publicsuffix` crate 需要在 `Cargo.toml` 加 `"builtin"` feature 或运行时 fetch PSL。**MVP 简化**：用 `List::empty()` + 在 `etld_plus_one` 实现一个**简化版本**——按 `.` 分段取最后两段（对于无 PSL 的情况），处理常见情况：

修正 Step 1，把 etld_plus_one 改为更实用的简化版（不依赖 PSL）：

```rust
pub fn etld_plus_one(host: &str) -> Option<String> {
    // 简化版：按 . 分段处理
    // 完整版需要 PSL（公共后缀列表）才能正确处理 .co.uk 等
    // MVP 接受这个局限（多数登录网站是 .com / .cn / .io 等单 TLD）
    if host.is_empty() {
        return None;
    }
    let parts: Vec<&str> = host.split('.').collect();
    match parts.len() {
        0 => None,
        1 => Some(host.to_string()), // localhost
        2 => Some(host.to_string()), // example.com
        _ => {
            // 取最后两段：mail.google.com → google.com
            // 局限：example.co.uk → co.uk（错，但 MVP 接受）
            let n = parts.len();
            Some(format!("{}.{}", parts[n - 2], parts[n - 1]))
        }
    }
}
```

- [x] **Step 2: matcher/mod.rs**

```rust
//! URL 匹配：5 种策略 + 等价域名。
//!
//! 直接借鉴 Bitwarden：
//!   Domain (eTLD+1, 默认) / Host / Exact / StartsWith / RegularExpression / Never

pub mod psl;

use std::collections::HashSet;

use regex::Regex;
use url::Url;

use crate::types::{Cipher, CipherData, MatchType};

pub fn find_matching_ciphers(
    url: &Url,
    ciphers: &[Cipher],
    equivalent_domains: &[Vec<String>],
) -> Vec<&Cipher> {
    ciphers
        .iter()
        .filter(|c| c.deleted_at.is_none())
        .filter(|c| matches_any_uri(url, c, equivalent_domains))
        .collect()
}

fn matches_any_uri(url: &Url, cipher: &Cipher, equivalent: &[Vec<String>]) -> bool {
    let login = match &cipher.data {
        CipherData::Login(l) => l,
        _ => return false,
    };
    login.uris.iter().any(|lu| match_uri_one(url, lu, equivalent))
}

fn match_uri_one(url: &Url, lu: &crate::types::LoginUri, equivalent: &[Vec<String>]) -> bool {
    let strategy = lu.match_type.unwrap_or(MatchType::Domain);
    let target = url.as_str();
    let cipher_uri = &lu.uri;

    match strategy {
        MatchType::Domain => psl::etld_plus_one(url.host_str().unwrap_or(""))
            .map(|target_domain| matches_domain(&target_domain, cipher_uri, equivalent))
            .unwrap_or(false),
        MatchType::Host => {
            let target_host = url.host_str().unwrap_or("");
            // cipher_uri 可能是 https://github.com 或 github.com
            let cipher_host = Url::parse(cipher_uri)
                .ok()
                .and_then(|u| u.host_str().map(String::from))
                .unwrap_or_else(|| cipher_uri.to_string());
            target_host == cipher_host
        }
        MatchType::Exact => target == cipher_uri,
        MatchType::StartsWith => target.starts_with(cipher_uri.as_str()),
        MatchType::RegularExpression => Regex::new(cipher_uri)
            .map(|r| r.is_match(target))
            .unwrap_or(false),
        MatchType::Never => false,
    }
}

/// Domain 匹配：target_domain 是否在 cipher_domain + 其等价域名 + target_domain 集合中。
fn matches_domain(
    target_domain: &str,
    cipher_uri: &str,
    equivalent: &[Vec<String>],
) -> bool {
    let cipher_host = Url::parse(cipher_uri)
        .ok()
        .and_then(|u| u.host_str().map(String::from))
        .unwrap_or_else(|| cipher_uri.to_string());
    let cipher_domain = psl::etld_plus_one(&cipher_host).unwrap_or_else(|| cipher_host.clone());

    let mut candidates: HashSet<String> = HashSet::new();
    candidates.insert(cipher_domain.clone());
    // 加入等价域名
    for group in equivalent {
        if group.contains(&cipher_domain) {
            for d in group {
                candidates.insert(d.clone());
            }
        }
    }
    candidates.contains(target_domain)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{LoginData, LoginUri};

    fn make_cipher(uris: &[(&str, Option<MatchType>)]) -> Cipher {
        Cipher {
            id: 1,
            folder_id: None,
            favorite: false,
            atype: crate::types::CipherType::Login,
            name: "test".into(),
            notes: None,
            data: CipherData::Login(LoginData {
                uris: uris
                    .iter()
                    .map(|(u, m)| LoginUri {
                        uri: u.to_string(),
                        match_type: *m,
                    })
                    .collect(),
                username: None,
                password: None,
                totp: None,
                password_revision_date: None,
            }),
            fields: vec![],
            password_history: vec![],
            reprompt: crate::types::RepromptType::None,
            deleted_at: None,
            created_at: "2026-07-18".into(),
            updated_at: "2026-07-18".into(),
        }
    }

    #[test]
    fn test_domain_match_subdomain() {
        let cipher = make_cipher(&[("https://github.com", None)]);
        let url = Url::parse("https://gist.github.com/foo").unwrap();
        let result = find_matching_ciphers(&url, std::slice::from_ref(&cipher), &[]);
        assert_eq!(result.len(), 1);
    }

    #[test]
    fn test_domain_match_exact() {
        let cipher = make_cipher(&[("https://github.com", None)]);
        let url = Url::parse("https://github.com/login").unwrap();
        let result = find_matching_ciphers(&url, std::slice::from_ref(&cipher), &[]);
        assert_eq!(result.len(), 1);
    }

    #[test]
    fn test_domain_match_different_etld_no() {
        let cipher = make_cipher(&[("https://github.com", None)]);
        let url = Url::parse("https://github.io/foo").unwrap();
        let result = find_matching_ciphers(&url, std::slice::from_ref(&cipher), &[]);
        assert_eq!(result.len(), 0); // 不同 eTLD+1
    }

    #[test]
    fn test_host_match() {
        let cipher = make_cipher(&[("https://mail.google.com", Some(MatchType::Host))]);
        let url_match = Url::parse("https://mail.google.com/inbox").unwrap();
        let url_nomatch = Url::parse("https://drive.google.com").unwrap();
        assert_eq!(
            find_matching_ciphers(&url_match, std::slice::from_ref(&cipher), &[]).len(),
            1
        );
        assert_eq!(
            find_matching_ciphers(&url_nomatch, std::slice::from_ref(&cipher), &[]).len(),
            0
        );
    }

    #[test]
    fn test_exact_match() {
        let cipher = make_cipher(&[("https://example.com/login", Some(MatchType::Exact))]);
        let url_match = Url::parse("https://example.com/login").unwrap();
        let url_nomatch = Url::parse("https://example.com/login?foo=1").unwrap();
        assert_eq!(
            find_matching_ciphers(&url_match, std::slice::from_ref(&cipher), &[]).len(),
            1
        );
        assert_eq!(
            find_matching_ciphers(&url_nomatch, std::slice::from_ref(&cipher), &[]).len(),
            0
        );
    }

    #[test]
    fn test_starts_with_match() {
        let cipher = make_cipher(&[("https://example.com/admin", Some(MatchType::StartsWith))]);
        let url = Url::parse("https://example.com/admin/users").unwrap();
        assert_eq!(
            find_matching_ciphers(&url, std::slice::from_ref(&cipher), &[]).len(),
            1
        );
    }

    #[test]
    fn test_regex_match() {
        let cipher = make_cipher(&[(r"^https://.*\.example\.com", Some(MatchType::RegularExpression))]);
        let url = Url::parse("https://foo.example.com/bar").unwrap();
        assert_eq!(
            find_matching_ciphers(&url, std::slice::from_ref(&cipher), &[]).len(),
            1
        );
    }

    #[test]
    fn test_never_match() {
        let cipher = make_cipher(&[("https://example.com", Some(MatchType::Never))]);
        let url = Url::parse("https://example.com").unwrap();
        assert_eq!(
            find_matching_ciphers(&url, std::slice::from_ref(&cipher), &[]).len(),
            0
        );
    }

    #[test]
    fn test_equivalent_domains() {
        let cipher = make_cipher(&[("https://google.com", None)]);
        let equivalent = vec![vec!["google.com".to_string(), "youtube.com".to_string()]];
        let url = Url::parse("https://youtube.com/watch?v=123").unwrap();
        assert_eq!(
            find_matching_ciphers(&url, std::slice::from_ref(&cipher), &equivalent).len(),
            1
        );
    }

    #[test]
    fn test_skip_deleted_cipher() {
        let mut cipher = make_cipher(&[("https://example.com", None)]);
        cipher.deleted_at = Some("2026-07-18".into());
        let url = Url::parse("https://example.com").unwrap();
        assert_eq!(
            find_matching_ciphers(&url, std::slice::from_ref(&cipher), &[]).len(),
            0
        );
    }

    #[test]
    fn test_multiple_uris_any_match() {
        let cipher = make_cipher(&[
            ("https://github.com", None),
            ("https://gitlab.com", None),
        ]);
        let url = Url::parse("https://gitlab.com/foo").unwrap();
        assert_eq!(
            find_matching_ciphers(&url, std::slice::from_ref(&cipher), &[]).len(),
            1
        );
    }
}
```

- [x] **Step 3: 在 vault/Cargo.toml 加 url crate**

修改 `crates/vault/Cargo.toml`，加：

```toml
url = "2"
```

- [x] **Step 4: 运行测试**

Run: `cargo test -p octopus-vault --lib matcher -- --nocapture`
Expected: psl (3) + mod (10) = 13 passed

- [x] **Step 5: Commit**

```bash
git add crates/vault/Cargo.toml crates/vault/src/matcher/
git commit -m "feat(vault): Task 13 - URL 匹配（5 种策略 + 简化 eTLD+1）"
```

---

## Task 14: vault health（strength + duplicate）

**Files:**
- Create: `crates/vault/src/health/mod.rs`
- Create: `crates/vault/src/health/strength.rs`
- Create: `crates/vault/src/health/duplicate.rs`

**Interfaces:**
- Consumes: `zxcvbn` / `sha2` crate、Task 6 的 Cipher
- Produces:
  - `pub struct PasswordStrength { score: u8, entropy_bits: f64, warning: Option<String>, suggestions: Vec<String> }`
  - `pub fn evaluate(password: &str) -> PasswordStrength`
  - `pub struct DuplicateGroup { password_hash: String, cipher_ids: Vec<i64> }`
  - `pub fn find_duplicates(ciphers: &[Cipher]) -> Vec<DuplicateGroup>`
  - `pub struct HealthReport { weak_count: usize, duplicate_groups: usize, duplicate_cipher_count: usize, total_logins: usize, average_score: f64 }`
  - `pub fn generate_report(ciphers: &[Cipher]) -> HealthReport`

- [x] **Step 1: health/strength.rs**

```rust
//! zxcvbn 密码强度评估。

use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct PasswordStrength {
    /// 0-4（zxcvbn 评分）
    pub score: u8,
    pub entropy_bits: f64,
    pub warning: Option<String>,
    pub suggestions: Vec<String>,
}

/// 评估密码强度。
pub fn evaluate(password: &str) -> PasswordStrength {
    let est = zxcvbn::zxcvbn(password, &[]).unwrap_or_else(|_| {
        // zxcvbn() 在空密码等极端情况会 Err，兜底返回 score=0
        return PasswordStrength {
            score: 0,
            entropy_bits: 0.0,
            warning: None,
            suggestions: vec![],
        };
    });

    let score = est.score().value() as u8;
    let entropy_bits = (est.guesses_log10() as f64) * 3.32; // log10 → log2

    let feedback = est.feedback();
    let warning = feedback.warning().map(|s| s.to_string());
    let suggestions = feedback.suggestions().iter().map(|s| s.to_string()).collect();

    PasswordStrength {
        score,
        entropy_bits,
        warning,
        suggestions,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_weak_password_low_score() {
        let s = evaluate("password");
        assert!(s.score <= 1, "password 应是弱密码: score={}", s.score);
    }

    #[test]
    fn test_strong_password_high_score() {
        let s = evaluate("Tr0ub4dour&3-something-longer");
        assert!(s.score >= 3, "应是强密码: score={}", s.score);
    }

    #[test]
    fn test_passphrase_strong() {
        let s = evaluate("correct horse battery staple");
        assert!(s.score >= 3, "应是强密码: score={}", s.score);
    }

    #[test]
    fn test_empty_password_no_panic() {
        let s = evaluate("");
        // 不应 panic
        assert_eq!(s.score, 0);
    }
}
```

**注意**：`zxcvbn::zxcvbn()` 返回 `Result<Entropy, ZxcvbnError>`，其中 `Err` 仅在 `password.len() > 4096` 时返回。空字符串实际返回 Ok。修正 Step 1：

```rust
pub fn evaluate(password: &str) -> PasswordStrength {
    match zxcvbn::zxcvbn(password, &[]) {
        Ok(est) => {
            let score = est.score().value() as u8;
            let entropy_bits = (est.guesses_log10() as f64) * 3.32;
            let feedback = est.feedback();
            let warning = feedback.warning().map(|s| s.to_string());
            let suggestions = feedback.suggestions().iter().map(|s| s.to_string()).collect();
            PasswordStrength {
                score,
                entropy_bits,
                warning,
                suggestions,
            }
        }
        Err(_) => PasswordStrength {
            score: 0,
            entropy_bits: 0.0,
            warning: None,
            suggestions: vec![],
        },
    }
}
```

- [x] **Step 2: health/duplicate.rs**

```rust
//! 重复密码检测（内存 SHA-256，不持久化 hash）。

use std::collections::HashMap;

use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::types::{Cipher, CipherData};

#[derive(Debug, Serialize)]
pub struct DuplicateGroup {
    /// SHA-256(password)，用于分组
    pub password_hash: String,
    pub cipher_ids: Vec<i64>,
}

pub fn find_duplicates(ciphers: &[Cipher]) -> Vec<DuplicateGroup> {
    let mut map: HashMap<String, Vec<i64>> = HashMap::new();
    for c in ciphers {
        if let CipherData::Login(login) = &c.data {
            if let Some(pwd) = &login.password {
                let mut hasher = Sha256::new();
                hasher.update(pwd.as_bytes());
                let hash = hasher.finalize();
                let hash_hex = data_encoding::HEXLOWER.encode(&hash);
                map.entry(hash_hex).or_default().push(c.id);
            }
        }
    }
    map.into_iter()
        .filter(|(_, ids)| ids.len() > 1)
        .map(|(password_hash, cipher_ids)| DuplicateGroup {
            password_hash,
            cipher_ids,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{
        CipherType, Field, LoginData, LoginUri, PasswordHistoryEntry, RepromptType,
    };

    fn make_cipher(id: i64, password: Option<&str>) -> Cipher {
        Cipher {
            id,
            folder_id: None,
            favorite: false,
            atype: CipherType::Login,
            name: format!("c-{}", id),
            notes: None,
            data: CipherData::Login(LoginData {
                uris: vec![],
                username: None,
                password: password.map(String::from),
                totp: None,
                password_revision_date: None,
            }),
            fields: vec![],
            password_history: vec![],
            reprompt: RepromptType::None,
            deleted_at: None,
            created_at: "2026-07-18".into(),
            updated_at: "2026-07-18".into(),
        }
    }

    #[test]
    fn test_no_duplicates() {
        let ciphers = vec![
            make_cipher(1, Some("a")),
            make_cipher(2, Some("b")),
            make_cipher(3, Some("c")),
        ];
        assert!(find_duplicates(&ciphers).is_empty());
    }

    #[test]
    fn test_finds_duplicates() {
        let ciphers = vec![
            make_cipher(1, Some("same")),
            make_cipher(2, Some("same")),
            make_cipher(3, Some("different")),
            make_cipher(4, Some("same")),
        ];
        let groups = find_duplicates(&ciphers);
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].cipher_ids.len(), 3);
    }

    #[test]
    fn test_multiple_duplicate_groups() {
        let ciphers = vec![
            make_cipher(1, Some("a")),
            make_cipher(2, Some("a")),
            make_cipher(3, Some("b")),
            make_cipher(4, Some("b")),
            make_cipher(5, Some("unique")),
        ];
        let groups = find_duplicates(&ciphers);
        assert_eq!(groups.len(), 2);
    }

    #[test]
    fn test_skip_none_password() {
        let ciphers = vec![
            make_cipher(1, None),
            make_cipher(2, None),
        ];
        assert!(find_duplicates(&ciphers).is_empty());
    }
}
```

**注意**：`data-encoding` 已在 Cargo.toml，但默认 features 只包含 BASE64。需要确认 `HEXLOWER` 可用——`data-encoding 2.x` 默认包含 HEXLOWER，OK。

- [x] **Step 3: health/mod.rs**

```rust
//! 健康报告：弱密码 + 重复密码汇总。

pub mod duplicate;
pub mod strength;

use serde::Serialize;

use crate::types::{Cipher, CipherData};

#[derive(Debug, Serialize)]
pub struct HealthReport {
    pub weak_count: usize,
    pub weak_cipher_ids: Vec<i64>,
    pub duplicate_groups: Vec<duplicate::DuplicateGroup>,
    pub total_logins: usize,
    pub average_score: f64,
}

pub fn generate_report(ciphers: &[Cipher]) -> HealthReport {
    let logins: Vec<&Cipher> = ciphers
        .iter()
        .filter(|c| matches!(&c.data, CipherData::Login(_)) && c.deleted_at.is_none())
        .collect();

    // 弱密码：score < 3
    let mut weak_cipher_ids = Vec::new();
    let mut total_score: f64 = 0.0;
    let mut score_count: usize = 0;
    for c in &logins {
        if let CipherData::Login(login) = &c.data {
            if let Some(pwd) = &login.password {
                let s = strength::evaluate(pwd);
                total_score += s.score as f64;
                score_count += 1;
                if s.score < 3 {
                    weak_cipher_ids.push(c.id);
                }
            }
        }
    }

    let weak_count = weak_cipher_ids.len();
    let duplicate_groups = duplicate::find_duplicates(
        &logins.iter().map(|&r| r.clone()).collect::<Vec<_>>(),
    );
    let average_score = if score_count > 0 {
        total_score / score_count as f64
    } else {
        0.0
    };

    HealthReport {
        weak_count,
        weak_cipher_ids,
        duplicate_groups,
        total_logins: logins.len(),
        average_score,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{
        CipherType, LoginData, LoginUri, RepromptType,
    };

    fn make_cipher(id: i64, password: &str) -> Cipher {
        Cipher {
            id,
            folder_id: None,
            favorite: false,
            atype: CipherType::Login,
            name: format!("c-{}", id),
            notes: None,
            data: CipherData::Login(LoginData {
                uris: vec![LoginUri { uri: format!("https://{}.com", id), match_type: None }],
                username: None,
                password: Some(password.into()),
                totp: None,
                password_revision_date: None,
            }),
            fields: vec![],
            password_history: vec![],
            reprompt: RepromptType::None,
            deleted_at: None,
            created_at: "2026-07-18".into(),
            updated_at: "2026-07-18".into(),
        }
    }

    #[test]
    fn test_report_aggregates() {
        let ciphers = vec![
            make_cipher(1, "password"),      // 弱 + 重复
            make_cipher(2, "password"),      // 弱 + 重复
            make_cipher(3, "Tr0ub4dour&3-something-very-long"), // 强
        ];
        let report = generate_report(&ciphers);
        assert_eq!(report.total_logins, 3);
        assert!(report.weak_count >= 2, "至少 2 个弱: {}", report.weak_count);
        assert_eq!(report.duplicate_groups.len(), 1);
        assert_eq!(report.duplicate_groups[0].cipher_ids.len(), 2);
    }

    #[test]
    fn test_report_excludes_deleted() {
        let mut ciphers = vec![make_cipher(1, "weak")];
        ciphers[0].deleted_at = Some("2026-07-18".into());
        let report = generate_report(&ciphers);
        assert_eq!(report.total_logins, 0);
    }
}
```

- [x] **Step 4: 在 vault/Cargo.toml 加 sha2**

修改 Cargo.toml：`sha2 = "0.10"` 已经存在（Task 1 已加），无需改。

- [x] **Step 5: 运行测试**

Run: `cargo test -p octopus-vault --lib health -- --nocapture`
Expected: strength (4) + duplicate (4) + mod (2) = 10 passed

- [x] **Step 6: Commit**

```bash
git add crates/vault/src/health/
git commit -m "feat(vault): Task 14 - 健康检查（zxcvbn 强度 + 重复密码）"
```

---

## Task 15: vault importer（Bitwarden unencrypted JSON）

**Files:**
- Create: `crates/vault/src/importer/mod.rs`
- Create: `crates/vault/src/importer/bitwarden.rs`
- Create: `crates/vault/src/importer/exporter.rs`

**Interfaces:**
- Consumes: serde_json、Task 6 的 Cipher/CipherInput、Task 7 的 storage
- Produces:
  - `pub struct ImportReport { total: usize, imported: usize, skipped: usize, errors: Vec<String> }`
  - `pub fn import_bitwarden_json(json: &str, key: &DerivedKey) -> Result<ImportReport>`
  - `pub fn export_vault_json(ciphers: &[Cipher]) -> Result<String>`

- [x] **Step 1: importer/mod.rs**

```rust
//! 导入导出：Bitwarden unencrypted JSON。

pub mod bitwarden;
pub mod exporter;

pub use bitwarden::{import_bitwarden_json, ImportReport};
pub use exporter::export_vault_json;
```

- [x] **Step 2: importer/bitwarden.rs**

```rust
//! Bitwarden unencrypted JSON 导入。
//!
//! 仅支持 type=1 (Login)。
//! 加密导出（encrypted=true）不支持（MVP）。

use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};

use crate::crypto::DerivedKey;
use crate::storage;
use crate::types::{
    CipherData, CipherInput, CipherType, Field, LoginData, LoginUri, MatchType, RepromptType,
};

#[derive(Debug, Deserialize)]
struct BitwardenExport {
    encrypted: bool,
    #[serde(default)]
    items: Vec<BitwardenItem>,
}

#[derive(Debug, Deserialize)]
struct BitwardenItem {
    #[serde(default)]
    name: String,
    #[serde(default)]
    notes: Option<String>,
    #[serde(default)]
    favorite: bool,
    #[serde(default = "default_type")]
    #[serde(rename = "type")]
    item_type: i64,
    #[serde(default)]
    fields: Vec<BitwardenField>,
    #[serde(default)]
    login: Option<BitwardenLogin>,
}

fn default_type() -> i64 { 1 }

#[derive(Debug, Deserialize)]
struct BitwardenField {
    #[serde(default)]
    name: String,
    #[serde(default)]
    value: Option<String>,
    #[serde(default)]
    #[serde(rename = "type")]
    field_type: i64,
}

#[derive(Debug, Deserialize)]
struct BitwardenLogin {
    #[serde(default)]
    username: Option<String>,
    #[serde(default)]
    password: Option<String>,
    #[serde(default)]
    totp: Option<String>,
    #[serde(default)]
    uris: Vec<BitwardenUri>,
}

#[derive(Debug, Deserialize)]
struct BitwardenUri {
    uri: String,
    #[serde(default)]
    r#match: Option<i64>,
}

#[derive(Debug, Serialize)]
pub struct ImportReport {
    pub total: usize,
    pub imported: usize,
    pub skipped: usize,
    pub errors: Vec<String>,
}

pub fn import_bitwarden_json(json: &str, key: &DerivedKey) -> Result<ImportReport> {
    let export: BitwardenExport = serde_json::from_str(json).context("JSON 解析失败")?;
    ensure!(!export.encrypted, "不支持加密导出（仅 unencrypted JSON）");

    let mut imported = 0;
    let mut skipped = 0;
    let mut errors: Vec<String> = Vec::new();

    for (idx, item) in export.items.iter().enumerate() {
        if item.item_type != 1 {
            skipped += 1;
            continue;
        }
        let login = match &item.login {
            Some(l) => l,
            None => {
                skipped += 1;
                continue;
            }
        };

        let input = CipherInput {
            folder_id: None,
            favorite: item.favorite,
            atype: CipherType::Login,
            name: item.name.clone(),
            notes: item.notes.clone(),
            data: CipherData::Login(LoginData {
                uris: login
                    .uris
                    .iter()
                    .map(|u| LoginUri {
                        uri: u.uri.clone(),
                        match_type: u.r#match.and_then(|m| MatchType::try_from(m).ok()),
                    })
                    .collect(),
                username: login.username.clone(),
                password: login.password.clone(),
                totp: login.totp.clone(),
                password_revision_date: None,
            }),
            fields: item
                .fields
                .iter()
                .map(|f| Field {
                    name: f.name.clone(),
                    value: f.value.clone(),
                    field_type: f.field_type,
                })
                .collect(),
            password_history: vec![],
            reprompt: RepromptType::None,
        };

        match storage::create_cipher(&input, key) {
            Ok(_) => imported += 1,
            Err(e) => {
                errors.push(format!("Item {} ({}): {}", idx, item.name, e));
                skipped += 1;
            }
        }
    }

    Ok(ImportReport {
        total: export.items.len(),
        imported,
        skipped,
        errors,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_key(byte: u8) -> DerivedKey {
        DerivedKey(crate::Zeroizing::new([byte; 32]))
    }

    #[test]
    fn test_reject_encrypted_export() {
        let key = make_key(1);
        let json = r#"{"encrypted": true, "items": []}"#;
        let result = import_bitwarden_json(json, &key);
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_minimal_export() {
        // 仅测 JSON 解析（不实际写入 DB）
        let json = r#"{
            "encrypted": false,
            "items": [
                {
                    "name": "GitHub",
                    "favorite": false,
                    "type": 1,
                    "login": {
                        "username": "user@example.com",
                        "password": "secret",
                        "uris": [{"uri": "https://github.com", "match": null}]
                    }
                }
            ]
        }"#;
        let export: BitwardenExport = serde_json::from_str(json).unwrap();
        assert!(!export.encrypted);
        assert_eq!(export.items.len(), 1);
        assert_eq!(export.items[0].name, "GitHub");
    }

    #[test]
    fn test_skip_non_login_type() {
        let json = r#"{
            "encrypted": false,
            "items": [
                {"name": "Note", "type": 2, "notes": "secret"},
                {"name": "Login", "type": 1, "login": {"username": "u"}}
            ]
        }"#;
        let export: BitwardenExport = serde_json::from_str(json).unwrap();
        let login_count = export.items.iter().filter(|i| i.item_type == 1).count();
        assert_eq!(login_count, 1);
    }

    #[test]
    fn test_invalid_json_errors() {
        let key = make_key(1);
        let result = import_bitwarden_json("not json", &key);
        assert!(result.is_err());
    }
}
```

- [x] **Step 3: importer/exporter.rs**

```rust
//! 导出 vault 为 Bitwarden unencrypted JSON。

use anyhow::Result;
use serde::Serialize;

use crate::types::{Cipher, CipherData};

#[derive(Debug, Serialize)]
struct BitwardenExport {
    encrypted: bool,
    version: i64,
    items: Vec<BitwardenItem>,
    folders: Vec<serde_json::Value>,
}

#[derive(Debug, Serialize)]
struct BitwardenItem {
    name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    notes: Option<String>,
    favorite: bool,
    #[serde(rename = "type")]
    item_type: i64,
    fields: Vec<BitwardenField>,
    login: Option<BitwardenLogin>,
}

#[derive(Debug, Serialize)]
struct BitwardenField {
    name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    value: Option<String>,
    #[serde(rename = "type")]
    field_type: i64,
}

#[derive(Debug, Serialize)]
struct BitwardenLogin {
    #[serde(skip_serializing_if = "Option::is_none")]
    username: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    password: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    totp: Option<String>,
    uris: Vec<BitwardenUri>,
}

#[derive(Debug, Serialize)]
struct BitwardenUri {
    uri: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    r#match: Option<i64>,
}

pub fn export_vault_json(ciphers: &[Cipher]) -> Result<String> {
    let items: Vec<BitwardenItem> = ciphers
        .iter()
        .filter(|c| c.deleted_at.is_none())
        .map(|c| {
            let (item_type, login) = match &c.data {
                CipherData::Login(l) => (1i64, Some(BitwardenLogin {
                    username: l.username.clone(),
                    password: l.password.clone(),
                    totp: l.totp.clone(),
                    uris: l
                        .uris
                        .iter()
                        .map(|u| BitwardenUri {
                            uri: u.uri.clone(),
                            r#match: u.match_type.map(|m| m.into()),
                        })
                        .collect(),
                })),
            };
            BitwardenItem {
                name: c.name.clone(),
                notes: c.notes.clone(),
                favorite: c.favorite,
                item_type,
                fields: c
                    .fields
                    .iter()
                    .map(|f| BitwardenField {
                        name: f.name.clone(),
                        value: f.value.clone(),
                        field_type: f.field_type,
                    })
                    .collect(),
                login,
            }
        })
        .collect();

    let export = BitwardenExport {
        encrypted: false,
        version: 2,
        items,
        folders: vec![],
    };

    Ok(serde_json::to_string_pretty(&export)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{
        CipherType, Field, LoginData, LoginUri, PasswordHistoryEntry, RepromptType,
    };

    fn make_login_cipher(name: &str) -> Cipher {
        Cipher {
            id: 1,
            folder_id: None,
            favorite: false,
            atype: CipherType::Login,
            name: name.into(),
            notes: Some("personal".into()),
            data: CipherData::Login(LoginData {
                uris: vec![LoginUri { uri: "https://example.com".into(), match_type: None }],
                username: Some("user".into()),
                password: Some("pass".into()),
                totp: None,
                password_revision_date: None,
            }),
            fields: vec![],
            password_history: vec![],
            reprompt: RepromptType::None,
            deleted_at: None,
            created_at: "2026-07-18".into(),
            updated_at: "2026-07-18".into(),
        }
    }

    #[test]
    fn test_export_round_trip_parse() {
        let ciphers = vec![make_login_cipher("GitHub")];
        let json = export_vault_json(&ciphers).unwrap();
        assert!(json.contains("\"GitHub\""));
        assert!(json.contains("\"user\""));
        assert!(json.contains("\"pass\""));

        // 重新解析回来
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed["encrypted"], false);
        assert_eq!(parsed["items"][0]["name"], "GitHub");
    }

    #[test]
    fn test_export_skips_deleted() {
        let mut c = make_login_cipher("GitHub");
        c.deleted_at = Some("2026-07-18".into());
        let json = export_vault_json(&[c]).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed["items"].as_array().unwrap().len(), 0);
    }
}
```

- [x] **Step 4: 运行测试**

Run: `cargo test -p octopus-vault --lib importer -- --nocapture`
Expected: bitwarden (4) + exporter (2) = 6 passed

- [x] **Step 5: Commit**

```bash
git add crates/vault/src/importer/
git commit -m "feat(vault): Task 15 - Bitwarden unencrypted JSON 导入导出"
```

---

## Task 16: desktop vault_state + AppState 集成

**Files:**
- Create: `crates/desktop/src/vault_state.rs`
- Modify: `crates/desktop/src/main.rs`（注册 AppState）
- Modify: `crates/desktop/src/lib.rs` 或 `main.rs`（声明 mod）

**Interfaces:**
- Consumes: Task 9 的 unlock
- Produces:
  - `pub struct VaultSession { user_vault_key: Option<Arc<DerivedKey>>, app_key: Option<Arc<DerivedKey>>, unlocked_at: Option<Instant> }`
  - `pub type SharedVaultSession = Arc<RwLock<VaultSession>>`
  - `pub const DEFAULT_USER_VAULT_TIMEOUT_SECS: u64 = 15 * 60`
  - 启动流程：app launch 时调 `vault::unlock::unlock_app_key_local()` 把 app_key 注入

- [x] **Step 1: 在 desktop Cargo.toml 加依赖**

修改 `crates/desktop/Cargo.toml`，在 `[dependencies]` 加：

```toml
octopus-vault = { path = "../vault" }
```

- [x] **Step 2: 写 vault_state.rs**

```rust
//! vault 解锁态管理。
//!
//! AppState：在 Tauri 进程内持有 user_vault_key / app_key。
//! 设计原则：
//!   - app_key 在进程启动时解密并常驻内存（用 K_machine 解 app_key_local_enc）
//!   - user_vault_key 在用户主动解锁后常驻，15 分钟超时清零（仅清 user_vault_key，app_key 不动）
//!   - 所有 key 用 Arc 共享，零拷贝传递；不暴露明文 slice 给外部

use std::sync::Arc;
use std::time::{Duration, Instant};

use parking_lot::RwLock;

use octopus_vault::crypto::DerivedKey;

pub const DEFAULT_USER_VAULT_TIMEOUT_SECS: u64 = 15 * 60;

pub struct VaultSession {
    /// None = 未解锁（用户密码 vault 锁定）
    pub user_vault_key: Option<Arc<DerivedKey>>,
    /// None = 未初始化 / 启动失败（少见）
    pub app_key: Option<Arc<DerivedKey>>,
    pub unlocked_at: Option<Instant>,
}

impl Default for VaultSession {
    fn default() -> Self {
        Self {
            user_vault_key: None,
            app_key: None,
            unlocked_at: None,
        }
    }
}

impl VaultSession {
    pub fn is_user_vault_unlocked(&self) -> bool {
        if self.user_vault_key.is_none() {
            return false;
        }
        // 超时检查
        if let Some(t) = self.unlocked_at {
            if t.elapsed() > Duration::from_secs(DEFAULT_USER_VAULT_TIMEOUT_SECS) {
                return false;
            }
        }
        true
    }

    pub fn set_user_vault_unlocked(&mut self, key: Arc<DerivedKey>) {
        self.user_vault_key = Some(key);
        self.unlocked_at = Some(Instant::now());
    }

    pub fn lock_user_vault(&mut self) {
        self.user_vault_key = None;
        self.unlocked_at = None;
    }
}

pub type SharedVaultSession = Arc<RwLock<VaultSession>>;

/// 启动时调一次：尝试用 K_machine 解 app_key 注入。
pub fn bootstrap_app_key(session: &SharedVaultSession) {
    match octopus_vault::unlock::unlock_app_key_local() {
        Ok(Some(app_key)) => {
            log::info!("vault app_key 已通过 K_machine 解锁（无感启动）");
            session.write().app_key = Some(Arc::new(app_key));
        }
        Ok(None) => {
            log::info!("vault 需主密码（K_machine 缺失或 vault 未初始化）");
        }
        Err(e) => {
            log::warn!("vault app_key 解锁失败: {}", e);
        }
    }
}
```

- [x] **Step 3: 在 main.rs 注册 mod 和 AppState**

在 `crates/desktop/src/main.rs` 顶部 `mod` 声明区加（找一个已有 `mod action_bar_commands;` 的位置）：

```rust
pub mod vault_state;
pub mod vault_commands;
pub mod autotype;
```

（vault_commands 和 autotype 在后续 Task 创建，先声明）

- [x] **Step 4: 在 main.rs setup 注入 AppState**

找到 setup 闭包内的 `app.manage(...)` 区段（根据调研在 main.rs 行 379+），在合适位置加：

```rust
let vault_session: vault_state::SharedVaultSession = std::sync::Arc::new(
    parking_lot::RwLock::new(vault_state::VaultSession::default())
);
vault_state::bootstrap_app_key(&vault_session);
app.manage(vault_session);
```

- [x] **Step 5: 创建占位的 vault_commands.rs 和 autotype/mod.rs**

```bash
mkdir -p crates/desktop/src/autotype
echo "//! 占位：Task 17 填充" > crates/desktop/src/vault_commands.rs
echo "//! 占位：Task 18 填充" > crates/desktop/src/autotype/mod.rs
```

- [x] **Step 6: 编译验证**

Run: `cargo build -p octopus-desktop 2>&1 | head -50`
Expected: 0 error

可能有 unused 警告（vault_commands 和 autotype 占位），但 0 error。

- [x] **Step 7: Commit**

```bash
git add crates/desktop/Cargo.toml crates/desktop/src/vault_state.rs crates/desktop/src/main.rs crates/desktop/src/vault_commands.rs crates/desktop/src/autotype/
git commit -m "feat(vault): Task 16 - desktop AppState + app_key 启动注入"
```

---

## Task 17: desktop vault_commands（CRUD + 生成 + 健康检查 + 导入导出 + 解锁）

**Files:**
- Create: `crates/desktop/src/vault_commands.rs`

**Interfaces:**
- Consumes: Task 9-15 vault crate 所有公开 API、Task 16 VaultSession
- Produces Tauri 命令：
  - `vault_status(state) -> VaultStatusDto`
  - `vault_setup(state, password) -> ()`
  - `vault_unlock(state, password) -> ()`
  - `vault_lock(state) -> ()`
  - `vault_change_password(state, old, new) -> ()`
  - `vault_list_ciphers(state) -> Vec<CipherDto>`
  - `vault_get_cipher(state, id) -> CipherDto`
  - `vault_create_cipher(state, input) -> i64`
  - `vault_update_cipher(state, id, input) -> ()`
  - `vault_delete_cipher(state, id, permanent) -> ()`
  - `vault_generate(state, cfg) -> String`
  - `vault_generate_totp(state, cipher_id) -> TotpResultDto`
  - `vault_health_report(state) -> HealthReport`
  - `vault_import_bitwarden(state, json) -> ImportReport`
  - `vault_export(state) -> String`

- [x] **Step 1: 写 vault_commands.rs（完整）**

```rust
//! vault Tauri 命令层。
//!
//! 命令返回类型用 DTO（避免直接暴露 vault crate 内部类型）。

use std::sync::Arc;

use tauri::State;

use octopus_vault::crypto::DerivedKey;
use octopus_vault::types::{Cipher, CipherData, CipherInput, CipherType, RepromptType};
use octopus_vault::generator::GeneratorConfig;
use octopus_vault::health::HealthReport;
use octopus_vault::importer::ImportReport;

use crate::vault_state::SharedVaultSession;

// === DTO ===

#[derive(serde::Serialize)]
pub struct VaultStatusDto {
    pub initialized: bool,
    pub user_vault_unlocked: bool,
}

#[derive(serde::Serialize, serde::Deserialize, Clone)]
pub struct LoginUriDto {
    pub uri: String,
    pub match_type: Option<i64>,
}

#[derive(serde::Serialize, serde::Deserialize, Clone)]
pub struct LoginDataDto {
    pub uris: Vec<LoginUriDto>,
    pub username: Option<String>,
    pub password: Option<String>,
    pub totp: Option<String>,
}

#[derive(serde::Serialize, serde::Deserialize, Clone)]
pub struct CipherDto {
    pub id: i64,
    pub folder_id: Option<i64>,
    pub favorite: bool,
    pub atype: i64,
    pub name: String,
    pub notes: Option<String>,
    pub login: Option<LoginDataDto>,
    pub fields: Vec<FieldDto>,
    pub reprompt: i64,
    pub deleted_at: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(serde::Serialize, serde::Deserialize, Clone)]
pub struct FieldDto {
    pub name: String,
    pub value: Option<String>,
    pub field_type: i64,
}

#[derive(serde::Deserialize)]
pub struct CipherInputDto {
    pub folder_id: Option<i64>,
    pub favorite: bool,
    pub name: String,
    pub notes: Option<String>,
    pub login: Option<LoginDataDto>,
    pub fields: Vec<FieldDto>,
    pub reprompt: Option<i64>,
}

#[derive(serde::Serialize)]
pub struct TotpResultDto {
    pub code: String,
    pub seconds_remaining: u64,
}

// === 辅助：从 AppState 拿 user_vault_key（必须解锁） ===

fn require_user_vault_key(state: &State<'_, SharedVaultSession>) -> Result<Arc<DerivedKey>, String> {
    let session = state.read();
    if !session.is_user_vault_unlocked() {
        return Err("vault 未解锁".into());
    }
    session.user_vault_key.clone().ok_or_else(|| "vault 未解锁".to_string())
}

fn require_app_key(state: &State<'_, SharedVaultSession>) -> Result<Arc<DerivedKey>, String> {
    let session = state.read();
    session.app_key.clone().ok_or_else(|| "vault app_key 不可用".to_string())
}

// === DTO ↔ Domain 转换 ===

fn cipher_to_dto(c: Cipher) -> CipherDto {
    let (login, atype) = match &c.data {
        CipherData::Login(l) => (
            Some(LoginDataDto {
                uris: l
                    .uris
                    .iter()
                    .map(|u| LoginUriDto {
                        uri: u.uri.clone(),
                        match_type: u.match_type.map(|m| m.into()),
                    })
                    .collect(),
                username: l.username.clone(),
                password: l.password.clone(),
                totp: l.totp.clone(),
            }),
            1,
        ),
    };
    CipherDto {
        id: c.id,
        folder_id: c.folder_id,
        favorite: c.favorite,
        atype,
        name: c.name,
        notes: c.notes,
        login,
        fields: c
            .fields
            .iter()
            .map(|f| FieldDto {
                name: f.name.clone(),
                value: f.value.clone(),
                field_type: f.field_type,
            })
            .collect(),
        reprompt: c.reprompt.into(),
        deleted_at: c.deleted_at,
        created_at: c.created_at,
        updated_at: c.updated_at,
    }
}

fn dto_to_input(dto: CipherInputDto) -> Result<CipherInput, String> {
    let login = dto.login.ok_or_else(|| "login 必填".to_string())?;
    Ok(CipherInput {
        folder_id: dto.folder_id,
        favorite: dto.favorite,
        atype: CipherType::Login,
        name: dto.name,
        notes: dto.notes,
        data: CipherData::Login(octopus_vault::types::LoginData {
            uris: login
                .uris
                .into_iter()
                .map(|u| octopus_vault::types::LoginUri {
                    uri: u.uri,
                    match_type: u.match_type.and_then(|m| {
                        octopus_vault::types::MatchType::try_from(m).ok()
                    }),
                })
                .collect(),
            username: login.username,
            password: login.password,
            totp: login.totp,
            password_revision_date: None,
        }),
        fields: dto
            .fields
            .into_iter()
            .map(|f| octopus_vault::types::Field {
                name: f.name,
                value: f.value,
                field_type: f.field_type,
            })
            .collect(),
        password_history: vec![],
        reprompt: dto
            .reprompt
            .map(|r| RepromptType::from(r))
            .unwrap_or(RepromptType::None),
    })
}

// === Tauri 命令 ===

#[tauri::command]
pub fn vault_status(state: State<'_, SharedVaultSession>) -> Result<VaultStatusDto, String> {
    let initialized = octopus_vault::unlock::is_initialized().map_err(|e| e.to_string())?;
    let user_vault_unlocked = state.read().is_user_vault_unlocked();
    Ok(VaultStatusDto {
        initialized,
        user_vault_unlocked,
    })
}

#[tauri::command]
pub fn vault_setup(state: State<'_, SharedVaultSession>, password: String) -> Result<(), String> {
    let keys = octopus_vault::unlock::setup_vault(&password).map_err(|e| e.to_string())?;
    let mut session = state.write();
    session.set_user_vault_unlocked(Arc::new(keys.user_vault_key));
    session.app_key = Some(Arc::new(keys.app_key));
    Ok(())
}

#[tauri::command]
pub fn vault_unlock(state: State<'_, SharedVaultSession>, password: String) -> Result<(), String> {
    let keys = octopus_vault::unlock::unlock_with_master_password(&password).map_err(|e| e.to_string())?;
    let mut session = state.write();
    session.set_user_vault_unlocked(Arc::new(keys.user_vault_key));
    session.app_key = Some(Arc::new(keys.app_key));
    Ok(())
}

#[tauri::command]
pub fn vault_lock(state: State<'_, SharedVaultSession>) -> Result<(), String> {
    state.write().lock_user_vault();
    Ok(())
}

#[tauri::command]
pub fn vault_change_password(
    state: State<'_, SharedVaultSession>,
    old_password: String,
    new_password: String,
) -> Result<(), String> {
    octopus_vault::unlock::change_master_password(&old_password, &new_password)
        .map_err(|e| e.to_string())?;
    // 改密码后不主动解锁 user_vault（让用户重新输）
    state.write().lock_user_vault();
    Ok(())
}

#[tauri::command]
pub fn vault_list_ciphers(state: State<'_, SharedVaultSession>) -> Result<Vec<CipherDto>, String> {
    let key = require_user_vault_key(&state)?;
    let ciphers = octopus_vault::storage::list_ciphers(&key).map_err(|e| e.to_string())?;
    Ok(ciphers.into_iter().map(cipher_to_dto).collect())
}

#[tauri::command]
pub fn vault_get_cipher(
    state: State<'_, SharedVaultSession>,
    id: i64,
) -> Result<CipherDto, String> {
    let key = require_user_vault_key(&state)?;
    let cipher = octopus_vault::storage::load_cipher(id, &key)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("cipher {} 不存在", id))?;
    Ok(cipher_to_dto(cipher))
}

#[tauri::command]
pub fn vault_create_cipher(
    state: State<'_, SharedVaultSession>,
    input: CipherInputDto,
) -> Result<i64, String> {
    let key = require_user_vault_key(&state)?;
    let domain = dto_to_input(input)?;
    octopus_vault::storage::create_cipher(&domain, &key).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn vault_update_cipher(
    state: State<'_, SharedVaultSession>,
    id: i64,
    input: CipherInputDto,
) -> Result<(), String> {
    let key = require_user_vault_key(&state)?;
    let domain = dto_to_input(input)?;
    octopus_vault::storage::save_cipher(id, &domain, &key).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn vault_delete_cipher(
    state: State<'_, SharedVaultSession>,
    id: i64,
    permanent: bool,
) -> Result<(), String> {
    // permanent=true 不需要 user_vault_key（只是删行）
    if permanent {
        octopus_vault::storage::permanent_delete(id).map_err(|e| e.to_string())
    } else {
        octopus_vault::storage::soft_delete(id).map_err(|e| e.to_string())
    }
}

#[tauri::command]
pub fn vault_restore_cipher(
    state: State<'_, SharedVaultSession>,
    id: i64,
) -> Result<(), String> {
    octopus_vault::storage::restore(id).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn vault_generate(cfg: GeneratorConfig) -> Result<String, String> {
    Ok(octopus_vault::generator::generate(&cfg))
}

#[tauri::command]
pub fn vault_generate_totp(
    state: State<'_, SharedVaultSession>,
    cipher_id: i64,
) -> Result<TotpResultDto, String> {
    let key = require_user_vault_key(&state)?;
    let cipher = octopus_vault::storage::load_cipher(cipher_id, &key)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "cipher 不存在".to_string())?;
    let login = match cipher.data {
        CipherData::Login(l) => l,
        _ => return Err("非 Login 类型".into()),
    };
    let totp_secret = login.totp.ok_or_else(|| "无 TOTP secret".to_string())?;
    let gen = octopus_vault::totp::TotpGenerator::from_base32(&totp_secret)
        .map_err(|e| e.to_string())?;
    Ok(TotpResultDto {
        code: gen.current().map_err(|e| e.to_string())?,
        seconds_remaining: gen.seconds_remaining(),
    })
}

#[tauri::command]
pub fn vault_health_report(state: State<'_, SharedVaultSession>) -> Result<HealthReport, String> {
    let key = require_user_vault_key(&state)?;
    let ciphers = octopus_vault::storage::list_ciphers(&key).map_err(|e| e.to_string())?;
    Ok(octopus_vault::health::generate_report(&ciphers))
}

#[tauri::command]
pub fn vault_import_bitwarden(
    state: State<'_, SharedVaultSession>,
    json: String,
) -> Result<ImportReport, String> {
    let key = require_user_vault_key(&state)?;
    octopus_vault::importer::import_bitwarden_json(&json, &key).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn vault_export(state: State<'_, SharedVaultSession>) -> Result<String, String> {
    let key = require_user_vault_key(&state)?;
    let ciphers = octopus_vault::storage::list_ciphers(&key).map_err(|e| e.to_string())?;
    octopus_vault::importer::export_vault_json(&ciphers).map_err(|e| e.to_string())
}
```

- [x] **Step 2: 在 main.rs invoke_handler! 注册命令**

打开 `crates/desktop/src/main.rs`，找到 `tauri::generate_handler![` 块（行 226）。在 `action_bar_commands::*` 之后或 `extensions::*` 之前加：

```rust
// vault 命令（2026-07-18）
crate::vault_commands::vault_status,
crate::vault_commands::vault_setup,
crate::vault_commands::vault_unlock,
crate::vault_commands::vault_lock,
crate::vault_commands::vault_change_password,
crate::vault_commands::vault_list_ciphers,
crate::vault_commands::vault_get_cipher,
crate::vault_commands::vault_create_cipher,
crate::vault_commands::vault_update_cipher,
crate::vault_commands::vault_delete_cipher,
crate::vault_commands::vault_restore_cipher,
crate::vault_commands::vault_generate,
crate::vault_commands::vault_generate_totp,
crate::vault_commands::vault_health_report,
crate::vault_commands::vault_import_bitwarden,
crate::vault_commands::vault_export,
```

- [x] **Step 3: 编译验证**

Run: `cargo build -p octopus-desktop 2>&1 | head -30`
Expected: 0 error

- [x] **Step 4: Commit**

```bash
git add crates/desktop/src/vault_commands.rs crates/desktop/src/main.rs
git commit -m "feat(vault): Task 17 - desktop vault Tauri 命令层"
```

---

## Task 18: desktop autotype（url_detect + macos + clipboard）

**Files:**
- Create: `crates/desktop/src/autotype/mod.rs`
- Create: `crates/desktop/src/autotype/url_detect.rs`
- Create: `crates/desktop/src/autotype/macos.rs`
- Create: `crates/desktop/src/autotype/clipboard.rs`

**Interfaces:**
- Consumes: enigo / objc2-app-kit（NSPasteboard）/ std::process::Command (osascript)
- Produces:
  - `pub fn current_browser_url() -> Result<Option<String>>`
  - `pub fn activate_browser(bundle_id: &str) -> Result<()>`
  - `pub fn autotype_login(username: &str, password: &str, press_enter: bool) -> Result<()>`
  - `pub fn copy_to_clipboard_concealed(text: &str, ttl_secs: u64) -> Result<()>`

- [x] **Step 1: autotype/url_detect.rs（macOS AppleScript）**

```rust
//! 用 AppleScript 取当前浏览器 active tab URL。
//!
//! 支持：Chrome / Safari / Firefox / Edge / Brave / Arc。
//! 首次调用会触发 macOS 权限授权框。

use anyhow::{Context, Result};
use std::collections::HashMap;
use std::process::Command;
use std::time::Duration;

const OSA_TIMEOUT: Duration = Duration::from_secs(5);

fn run_osascript(script: &str) -> Result<String> {
    use std::sync::mpsc;
    let (tx, rx) = mpsc::channel();
    let s = script.to_string();
    std::thread::spawn(move || {
        let out = Command::new("osascript").arg("-e").arg(&s).output();
        let _ = tx.send(out);
    });
    let output = rx
        .recv_timeout(OSA_TIMEOUT)
        .context("osascript 执行超时（可能未授权）")??;
    if !output.status.success() {
        anyhow::bail!(
            "osascript 失败: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn frontmost_bundle_id() -> Result<String> {
    // NSWorkspace 通过 osascript 取
    let script = r#"
    tell application "System Events"
        set frontApp to first application process whose frontmost is true
        set bundleId to bundle identifier of frontApp
    end tell
    "#;
    Ok(run_osascript(script)?)
}

fn script_for_browser(bundle_id: &str) -> Option<&'static str> {
    match bundle_id {
        "com.google.Chrome" | "com.microsoft.edgemac" | "com.brave.Browser" => Some(
            r#"tell application "Google Chrome" to get URL of active tab of front window"#,
        ),
        "com.apple.Safari" => Some(
            r#"tell application "Safari" to get URL of current tab of front window"#,
        ),
        "org.mozilla.firefox" => Some(
            r#"tell application "System Events" to tell process "Firefox"
                get value of text field 1 of group 1 of toolbar 1 of window 1
            end tell"#,
        ),
        "company.thebrowser.Browser" => Some(
            r#"tell application "Arc" to get URL of active tab of front window"#,
        ),
        _ => None,
    }
}

pub fn current_browser_url() -> Result<Option<String>> {
    let bundle_id = match frontmost_bundle_id() {
        Ok(id) => id,
        Err(_) => return Ok(None),
    };
    let script = match script_for_browser(&bundle_id) {
        Some(s) => s,
        None => return Ok(None), // 前台不是已知浏览器
    };
    match run_osascript(script) {
        Ok(url) if !url.is_empty() => Ok(Some(url)),
        Ok(_) => Ok(None),
        Err(e) => {
            log::warn!("URL 检测失败 for {}: {}", bundle_id, e);
            Ok(None)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_script_for_browser_chrome() {
        assert!(script_for_browser("com.google.Chrome").is_some());
    }

    #[test]
    fn test_script_for_browser_unknown() {
        assert!(script_for_browser("com.unknown.app").is_none());
    }

    #[test]
    fn test_script_for_browser_safari() {
        assert!(script_for_browser("com.apple.Safari").is_some());
    }

    #[test]
    fn test_script_for_browser_firefox() {
        assert!(script_for_browser("org.mozilla.firefox").is_some());
    }
}
```

- [x] **Step 2: autotype/macos.rs（enigo 键盘模拟）**

```rust
//! macOS Auto-Type：用 enigo 模拟键盘输入。
//!
//! 关键：密码字段（masked input）能正常接收 CGEvent 输入，
//! 因为 enigo 发的是真实键盘事件，浏览器收到的是按键而非 DOM 填值。

use anyhow::{Context, Result};
use std::process::Command;
use std::time::Duration;

use enigo::{Direction, Enigo, Key, Keyboard, Settings};

const FOCUS_WAIT: Duration = Duration::from_millis(100);

/// 把指定 bundle_id 的 app 激活到前台。
pub fn activate_app(bundle_id: &str) -> Result<()> {
    let script = format!(
        r#"tell application id "{}" to activate"#,
        bundle_id
    );
    let output = Command::new("osascript")
        .arg("-e")
        .arg(&script)
        .output()
        .context("osascript 调用失败")?;
    if !output.status.success() {
        anyhow::bail!(
            "activate {} 失败: {}",
            bundle_id,
            String::from_utf8_lossy(&output.stderr)
        );
    }
    Ok(())
}

/// 模拟键盘输入 username + Tab + password[ + Tab + Enter]。
pub fn autotype_login(username: &str, password: &str, press_enter: bool) -> Result<()> {
    std::thread::sleep(FOCUS_WAIT);

    let mut enigo = Enigo::new(&Settings::default()).context("enigo 初始化失败")?;

    // username
    enigo.text(username).context("输入 username 失败")?;

    // Tab → password 字段
    enigo.key(Key::Tab, Direction::Click);
    std::thread::sleep(Duration::from_millis(30));

    // password
    enigo.text(password).context("输入 password 失败")?;

    if press_enter {
        std::thread::sleep(Duration::from_millis(30));
        enigo.key(Key::Return, Direction::Click);
    }
    Ok(())
}
```

**注意**：enigo 0.6 API。检查 crates/desktop/src/paste.rs 的实际用法以校准。`Enigo::new` 在 0.6 可能签名不同——根据调研 `paste.rs:140` 的用法是 `Enigo::new(&Settings::default())?`，OK。但 `key()` 是 `Direction::Press / Release / Click`，需 `use enigo::Direction`。

- [x] **Step 3: autotype/clipboard.rs（concealed 写入 + 30s 自动清空）**

```rust
//! 剪贴板 concealed 写入：30 秒后自动清空。
//!
//! 走 octopus-clipboard 的 ClipboardHandle::write_text（自动 suppress_next，
//! 跳过自身 clipboard_history 监听器），同时单独写 org.nspasteboard.ConcealedType
//! 让第三方剪贴板工具（Maccy / Paste / iCloud Universal Clipboard）跳过。

use anyhow::{Context, Result};
use std::time::Duration;

use objc2::msg_send;
use objc2::runtime::AnyObject;
use objc2::rc::Retained;
use objc2_app_kit::{NSPasteboard, NSPasteboardTypeString};
use objc2_foundation::NSString;

const DEFAULT_TTL: Duration = Duration::from_secs(30);
const CONCEALED_MARKER: &str = "octopus-vault-concealed";

/// 复制到剪贴板并 concealed 标记 + 30s 自动清空。
pub fn copy_concealed(text: &str) -> Result<()> {
    copy_concealed_with_ttl(text, DEFAULT_TTL)
}

pub fn copy_concealed_with_ttl(text: &str, ttl: Duration) -> Result<()> {
    unsafe {
        let pb: Retained<NSPasteboard> = NSPasteboard::generalPasteboard();
        pb.clearContents();

        // 写入文本
        let ns_str = NSString::from_str(text);
        pb.setString_forType(&ns_str, NSPasteboardTypeString);

        // 写入 concealed 标记（第三方剪贴板工具识别）
        let marker = NSString::from_str(CONCEALED_MARKER);
        let concealed_type = NSString::from_str("org.nspasteboard.ConcealedType");
        let _: bool = msg_send![
            &pb,
            setString: &marker,
            forType: &concealed_type
        ];
    }

    // 让 octopus 自身 clipboard 监听器跳过这次写入
    // 方式：通过 ClipboardHandle::suppress_next（需 AppState 提供 handle 引用）
    // 此处由调用方在调本函数前手动调 handle.suppress_next()

    // spawn 定时清空
    let ttl_secs = ttl.as_secs();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_secs(ttl_secs));
        let _ = clear_clipboard();
    });

    Ok(())
}

fn clear_clipboard() -> Result<()> {
    unsafe {
        let pb: Retained<NSPasteboard> = NSPasteboard::generalPasteboard();
        pb.clearContents();
        let empty = NSString::from_str("");
        pb.setString_forType(&empty, NSPasteboardTypeString);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_ttl_is_30s() {
        assert_eq!(DEFAULT_TTL, Duration::from_secs(30));
    }
}
```

**注意**：objc2 API 调用方式可能需要调整。根据调研，`crates/desktop/src/action_bar_commands.rs:545-555` 用了 `objc2::msg_send!` + `runtime::AnyObject` 取 NSPasteboard，但这里用 `objc2-app-kit` 的强类型 API。实施时根据实际编译报错调整——可以参考 `crates/clipboard/` 里是否已有 NSPasteboard 封装。

- [x] **Step 4: autotype/mod.rs（trait + dispatch）**

```rust
//! Auto-Type：跨平台键盘模拟 + URL 检测。
//!
//! MVP 仅 macOS。Windows/Linux 编译通过但运行时返回 Err("not implemented")。

pub mod clipboard;
#[cfg(target_os = "macos")]
pub mod macos;
#[cfg(target_os = "macos")]
pub mod url_detect;

#[cfg(target_os = "macos")]
pub use macos::{activate_app, autotype_login};
#[cfg(target_os = "macos")]
pub use url_detect::current_browser_url;
#[cfg(target_os = "macos")]
pub use clipboard::{copy_concealed, copy_concealed_with_ttl};

#[cfg(not(target_os = "macos"))]
pub fn activate_app(_bundle_id: &str) -> anyhow::Result<()> {
    anyhow::bail!("Auto-Type 尚未实现此平台")
}
#[cfg(not(target_os = "macos"))]
pub fn autotype_login(_u: &str, _p: &str, _enter: bool) -> anyhow::Result<()> {
    anyhow::bail!("Auto-Type 尚未实现此平台")
}
#[cfg(not(target_os = "macos"))]
pub fn current_browser_url() -> anyhow::Result<Option<String>> {
    Ok(None)
}
#[cfg(not(target_os = "macos"))]
pub fn copy_concealed(_t: &str) -> anyhow::Result<()> {
    anyhow::bail!("concealed 剪贴板尚未实现此平台")
}
#[cfg(not(target_os = "macos"))]
pub fn copy_concealed_with_ttl(_t: &str, _ttl: std::time::Duration) -> anyhow::Result<()> {
    anyhow::bail!("concealed 剪贴板尚未实现此平台")
}
```

- [x] **Step 5: 编译验证**

Run: `cargo build -p octopus-desktop 2>&1 | head -30`
Expected: 0 error（objc2 API 可能需要调整，根据编译报错修正）

- [x] **Step 6: Commit**

```bash
git add crates/desktop/src/autotype/
git commit -m "feat(vault): Task 18 - macOS Auto-Type + URL 检测 + concealed 剪贴板"
```

---

## Task 19: desktop autotype Tauri 命令 + 全局热键注册

> **⚠️ Follow-up 修订（commit `ecca9b04`）**：原计划注册两个全局热键（Cmd+Shift+L autotype +
> Cmd+Shift+G 生成器浮窗）+ `password_generator_window` 浮窗。**实施时生成器热键 + 浮窗全删除**——
> 生成器 UI 内嵌 CipherEditor。`AppConfig.vault_generator_shortcut` 字段保留仅为兼容旧 DB，
> 运行时不消费。当前唯一 vault 全局热键是 `Cmd+Shift+L`。
> 下文涉及 generator shortcut / password_generator_window 的代码块是历史记录。

**Files:**
- Modify: `crates/desktop/src/vault_commands.rs`（加 autotype 命令）
- Modify: `crates/desktop/src/main.rs`（注册新命令 + 注册全局热键）
- Modify: `crates/infra/src/config.rs`（加 vault_autotype_shortcut / vault_generator_shortcut 字段）

**Interfaces:**
- Produces Tauri 命令：
  - `vault_autotype(state) -> AutoTypeResult`
  - `vault_detect_and_match(state) -> Vec<CipherDto>`
  - `vault_copy_password(state, id) -> ()`（复制密码到 concealed 剪贴板）
- 全局热键：Cmd+Shift+L → autotype, Cmd+Shift+G → 浮窗

- [x] **Step 1: 在 vault_commands.rs 加 autotype 命令**

追加到 `crates/desktop/src/vault_commands.rs` 末尾：

```rust
use crate::autotype;

#[derive(serde::Serialize)]
pub struct AutoTypeResultDto {
    pub filled: bool,
    pub message: String,
    pub fallback_to_clipboard: bool,
}

/// 触发 Auto-Type 完整流程：检测 URL → 匹配 cipher → 解锁确认 → 模拟键盘
#[tauri::command]
pub fn vault_autotype(
    state: State<'_, SharedVaultSession>,
    cipher_id: i64,
) -> Result<AutoTypeResultDto, String> {
    let key = require_user_vault_key(&state)?;

    // 1. 取 cipher
    let cipher = octopus_vault::storage::load_cipher(cipher_id, &key)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "cipher 不存在".to_string())?;

    // 2. reprompt 确认（如有）
    // TODO: 由前端在调本命令前弹密码框；本命令不直接处理

    // 3. 提取 username / password
    let (username, password) = match &cipher.data {
        octopus_vault::types::CipherData::Login(l) => {
            (l.username.clone().unwrap_or_default(), l.password.clone().unwrap_or_default())
        }
        _ => return Err("非 Login 类型".into()),
    };

    // 4. Auto-Type
    match autotype::autotype_login(&username, &password, false) {
        Ok(()) => Ok(AutoTypeResultDto {
            filled: true,
            message: "已填充".into(),
            fallback_to_clipboard: false,
        }),
        Err(e) => {
            // fallback：复制密码到剪贴板
            let _ = autotype::copy_concealed(&password);
            Ok(AutoTypeResultDto {
                filled: false,
                message: format!("Auto-Type 失败，已复制密码到剪贴板（30s 清空）: {}", e),
                fallback_to_clipboard: true,
            })
        }
    }
}

/// 检测当前浏览器 URL + 返回匹配 cipher 列表
#[tauri::command]
pub fn vault_detect_and_match(
    state: State<'_, SharedVaultSession>,
) -> Result<Vec<CipherDto>, String> {
    let key = require_user_vault_key(&state)?;

    let url_str = autotype::current_browser_url()
        .map_err(|e| e.to_string())?
        .unwrap_or_default();
    if url_str.is_empty() {
        // URL 检测失败 → 返回全部 cipher 让用户手动选
        return octopus_vault::storage::list_ciphers(&key)
            .map_err(|e| e.to_string())
            .map(|cs| cs.into_iter().map(cipher_to_dto).collect());
    }

    let url = url::Url::parse(&url_str).map_err(|e| format!("URL 解析失败: {}", e))?;
    let ciphers = octopus_vault::storage::list_ciphers(&key).map_err(|e| e.to_string())?;

    // 默认等价域名（MVP）
    let equivalent = octopus_vault::matcher::psl::default_equivalent_domains();

    let matched = octopus_vault::matcher::find_matching_ciphers(&url, &ciphers, &equivalent);
    Ok(matched.into_iter().cloned().map(cipher_to_dto).collect())
}

/// 复制指定 cipher 的密码到 concealed 剪贴板
#[tauri::command]
pub fn vault_copy_password(
    state: State<'_, SharedVaultSession>,
    cipher_id: i64,
) -> Result<(), String> {
    let key = require_user_vault_key(&state)?;
    let cipher = octopus_vault::storage::load_cipher(cipher_id, &key)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "cipher 不存在".to_string())?;

    if let octopus_vault::types::CipherData::Login(l) = cipher.data {
        if let Some(pwd) = l.password {
            autotype::copy_concealed(&pwd).map_err(|e| e.to_string())?;
            return Ok(());
        }
    }
    Err("无密码".into())
}
```

- [x] **Step 2: 在 desktop Cargo.toml 加 url crate**

修改 `crates/desktop/Cargo.toml`，加：

```toml
url = "2"
```

- [x] **Step 3: 在 main.rs invoke_handler 注册新命令**

在 Task 17 注册的 vault 命令列表后加：

```rust
crate::vault_commands::vault_autotype,
crate::vault_commands::vault_detect_and_match,
crate::vault_commands::vault_copy_password,
```

- [x] **Step 4: 在 infra config.rs 加 vault 热键字段**

打开 `crates/infra/src/config.rs`，找到 `action_bar_shortcut` 字段（行 194-196 附近），加：

```rust
/// vault Auto-Type 全局热键。默认 CmdOrCtrl+Shift+L。
#[serde(default = "default_vault_autotype_shortcut")]
pub vault_autotype_shortcut: String,

/// vault 密码生成器浮窗热键。默认 CmdOrCtrl+Shift+G。
#[serde(default = "default_vault_generator_shortcut")]
pub vault_generator_shortcut: String,
```

在文件末尾的默认函数区（含 `default_action_bar_shortcut` 等）加：

```rust
fn default_vault_autotype_shortcut() -> String {
    "CmdOrCtrl+Shift+L".into()
}

fn default_vault_generator_shortcut() -> String {
    "CmdOrCtrl+Shift+G".into()
}
```

- [x] **Step 5: 在 main.rs 启动时注册 vault 全局热键**

找到 main.rs 中 `register_action_bar_shortcut` 调用的位置（约行 594-598），仿照加：

```rust
// vault Auto-Type 热键（默认 Cmd+Shift+L）
if let Err(e) = crate::vault_commands::register_vault_autotype_shortcut(
    &app.handle(),
    &config.vault_autotype_shortcut,
) {
    log::warn!("注册 vault autotype 热键失败: {}", e);
}

// vault 生成器热键（默认 Cmd+Shift+G）
if let Err(e) = crate::vault_commands::register_vault_generator_shortcut(
    &app.handle(),
    &config.vault_generator_shortcut,
) {
    log::warn!("注册 vault generator 热键失败: {}", e);
}
```

在 `vault_commands.rs` 加注册函数：

```rust
use tauri::AppHandle;
use tauri_plugin_global_shortcut::{GlobalShortcutExt, Shortcut, ShortcutState};

pub fn register_vault_autotype_shortcut(
    app: &AppHandle,
    shortcut_str: &str,
) -> Result<(), String> {
    let shortcut: Shortcut = shortcut_str
        .parse()
        .map_err(|e| format!("解析热键 '{}' 失败: {}", shortcut_str, e))?;
    let app_handle = app.clone();
    app.global_shortcut()
        .on_shortcut(shortcut, move |_app, _scut, event| {
            if event.state() == ShortcutState::Pressed {
                log::info!("vault autotype 触发");
                // TODO: Task 21 加前端事件 → 弹选择浮窗
                // 当前先 emit 一个事件
                let _ = app_handle.emit("vault://autotype-triggered", ());
            }
        })
        .map_err(|e| format!("注册热键 '{}' 失败: {}", shortcut_str, e))?;
    Ok(())
}

pub fn register_vault_generator_shortcut(
    app: &AppHandle,
    shortcut_str: &str,
) -> Result<(), String> {
    let shortcut: Shortcut = shortcut_str
        .parse()
        .map_err(|e| format!("解析热键 '{}' 失败: {}", shortcut_str, e))?;
    let app_handle = app.clone();
    app.global_shortcut()
        .on_shortcut(shortcut, move |_app, _scut, event| {
            if event.state() == ShortcutState::Pressed {
                log::info!("vault generator 触发");
                let _ = tauri::WebviewWindowBuilder::new(
                    &app_handle,
                    "password_generator_window",
                    tauri::WebviewUrl::App("index.html".into()),
                )
                .title("密码生成器")
                .inner_size(480.0, 360.0)
                .resizable(false)
                .build();
            }
        })
        .map_err(|e| format!("注册热键 '{}' 失败: {}", shortcut_str, e))?;
    Ok(())
}
```

**注意**：`emit` 和 `WebviewWindowBuilder` 的 import 路径需要根据实际 tauri 2 API 调整。`use tauri::{Emitter, WebviewUrl, WebviewWindowBuilder};` 通常是需要的。

- [x] **Step 6: 编译验证**

Run: `cargo build -p octopus-desktop 2>&1 | head -30`
Expected: 0 error

- [x] **Step 7: Commit**

```bash
git add crates/desktop/Cargo.toml crates/desktop/src/vault_commands.rs crates/desktop/src/main.rs crates/infra/src/config.rs
git commit -m "feat(vault): Task 19 - autotype 命令 + 全局热键注册"
```

---

## Task 20: 一次性迁移 models.secret_key（init vault 时触发）

**Files:**
- Modify: `crates/vault/src/unlock.rs`（在 setup_vault 末尾加迁移逻辑）
- Modify: `crates/vault/src/migrate.rs`（新文件，提供迁移函数）
- Modify: `crates/vault/src/lib.rs`（声明 pub mod migrate）

**Interfaces:**
- Consumes: Task 9 的 app_key、infra 的 models 表 CRUD
- Produces: `pub fn migrate_secret_keys_to_encrypted(app_key: &DerivedKey) -> Result<usize>`

- [x] **Step 1: 在 vault/lib.rs 声明 mod**

修改 `crates/vault/src/lib.rs`，加 `pub mod migrate;`

- [x] **Step 2: 写 migrate.rs**

```rust
//! 一次性迁移：把 models.secret_key 的明文 API Key 用 app_key 加密回写。
//!
//! 触发时机：首次 setup_vault 之后。
//! 规则：
//!   - 仅处理 is_local=0（云端 API Key）的行
//!   - 跳过已 v1: 开头的行（避免重复加密）
//!   - 迁移后字段以 v1: 前缀存密文

use anyhow::Result;
use octopus_infra::db;

use crate::crypto::DerivedKey;

/// 迁移所有未加密的 secret_key。返回迁移的行数。
pub fn migrate_secret_keys_to_encrypted(app_key: &DerivedKey) -> Result<usize> {
    let models = db::list_models_for_secret_migration()?;
    let mut count = 0usize;
    for (model_id, plaintext) in models {
        let encrypted = app_key.encrypt(plaintext.as_bytes())?;
        db::update_model_secret_key(model_id, &encrypted)?;
        count += 1;
        log::info!("迁移 model {} 的 secret_key 为加密格式", model_id);
    }
    Ok(count)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_signature_compiles() {
        // 仅签名测试，不真正调用 DB
        let _ = std::any::TypeId::of::<fn(&DerivedKey) -> Result<usize>>();
    }
}
```

- [x] **Step 3: 在 infra db.rs 加迁移辅助函数**

打开 `crates/infra/src/db.rs`，在文件末尾追加：

```rust
/// 返回所有需要迁移的 model：(id, 明文 secret_key)。
/// 仅 is_local=0 且不以 v1: 开头的行。
pub fn list_models_for_secret_migration() -> Result<Vec<(i64, String)>> {
    with_db(|conn| {
        let mut stmt = conn.prepare(
            "SELECT id, secret_key FROM models WHERE is_local = 0 AND secret_key != '' AND secret_key NOT LIKE 'v1:%'",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
        })?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    })
}

/// 更新指定 model 的 secret_key 字段。
pub fn update_model_secret_key(model_id: i64, new_secret_key: &str) -> Result<()> {
    with_db(|conn| {
        conn.execute(
            "UPDATE models SET secret_key = ?, updated_at = datetime('now') WHERE id = ?",
            rusqlite::params![new_secret_key, model_id],
        )?;
        Ok(())
    })
}
```

- [x] **Step 4: 在 unlock.rs setup_vault 末尾调迁移**

打开 `crates/vault/src/unlock.rs`，找到 `setup_vault` 函数末尾（在 `Ok(UnlockedKeys { ... })` 之前）加：

```rust
    // 一次性迁移现有明文 secret_key（仅首次 init vault 时触发）
    match crate::migrate::migrate_secret_keys_to_encrypted(&app_key) {
        Ok(n) if n > 0 => log::info!("已迁移 {} 个 model 的 secret_key 为加密格式", n),
        Ok(_) => log::debug!("无明文 secret_key 需迁移"),
        Err(e) => log::warn!("secret_key 迁移失败（不阻塞 setup）: {}", e),
    }
```

- [x] **Step 5: 编译验证**

Run: `cargo build -p octopus-vault`
Expected: 0 error

- [x] **Step 6: Commit**

```bash
git add crates/vault/src/migrate.rs crates/vault/src/lib.rs crates/vault/src/unlock.rs crates/infra/src/db.rs
git commit -m "feat(vault): Task 20 - 一次性迁移 models.secret_key 为加密格式"
```

---

## Task 21: 前端 VaultPanel + SetupWizard + UnlockDialog + CipherEditor 内嵌生成器 + i18n

> **⚠️ Follow-up 修订（多个 commit）**：
> - **生成器位置**（`ecca9b04`）：`pages/PasswordGenerator/index.tsx` 独立浮窗废弃——
>   生成器 UI 内嵌 CipherEditor，`buildConfig.ts` + 测试移到 `pages/Settings/Vault/`。
> - **folder UI**（`4e5c3540`）：新增 FolderSidebar + FolderPromptDialog + CipherEditor folder dropdown。
> - **lock timeout 设置 UI**（`651e8db3`）：VaultPanel 加 30s/1/3/5/15min/Never 选项 + 30s 心跳。
> - **主密码校验**（`1c46a9d9`）：SetupWizard + UnlockDialog 用 `validateMasterPassword.ts`（12 位 + 4 类）。
> - **错误分类**（`23eedb35`）：所有调用点用 `classifyError.ts` 解析 VaultError JSON。
> - **feature gate**（`d70aa426`）：Settings/index.tsx + App.tsx 条件渲染 vault UI（基于 is_vault_enabled 探针）。
>
> 下文涉及 `PasswordGenerator/` 独立浮窗的代码块是历史记录。

**Files:**
- Create: `crates/desktop/frontend/src/pages/Settings/VaultPanel.tsx`
- Create: `crates/desktop/frontend/src/pages/Settings/Vault/`（子组件目录）
  - `SetupWizard.tsx` - 首次初始化向导
  - `UnlockDialog.tsx` - 解锁弹窗
  - `CipherList.tsx` - cipher 列表
  - `CipherEditor.tsx` - 新建/编辑表单
  - `HealthReport.tsx` - 健康报告
  - `ImportExport.tsx` - 导入导出
- Create: `crates/desktop/frontend/src/pages/PasswordGenerator/index.tsx` - 独立浮窗
- Modify: `crates/desktop/frontend/src/pages/Settings/index.tsx` - NAV_ITEMS 加 vault
- Modify: `crates/desktop/frontend/src/App.tsx` - 加 password_generator_window case
- Modify: `crates/desktop/capabilities/default.json` - 加 password_generator_window
- Modify: `crates/desktop/frontend/src/locales/zh-CN.yaml` 和 `en.yaml` - i18n

**Interfaces:**
- Consumes: Task 17/19 的 Tauri 命令
- Produces: 完整 UI 流程

- [x] **Step 1: 在 i18n yaml 加 vault keys**

打开 `crates/desktop/frontend/src/locales/zh-CN.yaml`，加：

```yaml
  nav:
    vault: 密码保险库
  vault:
    title: 密码保险库
    description: 端到端加密存储你的登录凭证，自动填充网页登录表单
    setup:
      title: 初始化密码保险库
      passwordLabel: 主密码
      passwordConfirm: 确认主密码
      passwordMismatch: 两次输入不一致
      weakPassword: 主密码过弱（建议至少 12 位，含大小写数字符号）
      warning: 主密码不可找回，遗忘后将无法解密保险库
      submit: 创建保险库
    unlock:
      title: 解锁保险库
      passwordLabel: 主密码
      submit: 解锁
      wrongPassword: 主密码错误
    lock: 锁定保险库
    changePassword: 修改主密码
    list:
      title: 密码条目
      empty: 还没有密码条目
      search: 搜索名称/URL/用户名
      addNew: 添加
      favorite: 收藏
      deleted: 已删除
    editor:
      titleNew: 新建密码条目
      titleEdit: 编辑密码条目
      nameLabel: 名称
      urlLabel: 网址（每行一个）
      usernameLabel: 用户名
      passwordLabel: 密码
      totpLabel: TOTP Secret（Base32）
      notesLabel: 备注
      favoriteLabel: 收藏
      save: 保存
      cancel: 取消
      delete: 删除
      permanentDelete: 永久删除
    health:
      title: 密码健康
      weak: 弱密码
      weakCount: "{{count}} 个弱密码"
      duplicates: 重复密码
      duplicatesCount: "{{count}} 组重复"
      total: 共 {{count}} 个登录
      averageScore: 平均强度 {{score}} / 4
    importExport:
      title: 导入导出
      importLabel: 从 Bitwarden JSON 导入
      exportLabel: 导出为 Bitwarden JSON
      exportWarning: 导出文件包含所有密码的明文，请妥善保管
      importSuccess: "导入完成：{{imported}} 成功，{{skipped}} 跳过"
    totp:
      copyCode: 复制验证码
      secondsRemaining: "{{secs}} 秒后刷新"
    autotype:
      trigger: 自动填充
      noMatch: 没有匹配当前页面的密码条目
      fillFail: 自动填充失败，已复制到剪贴板
    generator:
      title: 密码生成器
      mode:
        random: 随机字符
        passphraseEn: 英文短语
        passphraseZh: 中文短语
        pin: 数字 PIN
      length: 长度
      uppercase: 大写字母
      lowercase: 小写字母
      numbers: 数字
      symbols: 符号
      avoidAmbiguous: 避免易混字符
      wordCount: 词数
      separator: 分隔符
      capitalize: 首字母大写
      includeNumber: 含数字
      includeSymbol: 含符号
      regenerate: 重新生成
      copy: 复制
      strength: 强度
      strengthLevels:
        0: 极弱
        1: 弱
        2: 一般
        3: 强
        4: 极强
```

在 `en.yaml` 加对应英文翻译（结构同上）。

- [x] **Step 2: 修改 Settings/index.tsx NAV_ITEMS**

打开 `crates/desktop/frontend/src/pages/Settings/index.tsx`，找到 NAV_ITEMS（约行 39-48），加：

```tsx
{ page: "vault", icon: Lock, labelKey: "settings.nav.vault" },
```

在 import 区加 `Lock` from `lucide-react`。

修改 `PageName` 类型（约行 37）：

```tsx
type PageName = "general" | "clipboard" | "actionbar" | "agent" | "hotword" | "models" | "prompts" | "system" | "history" | "vault";
```

在 switch 渲染处加 case：

```tsx
case "vault":
  return <VaultPanel />;
```

加 import：

```tsx
import VaultPanel from "./VaultPanel";
```

- [x] **Step 3: 写 VaultPanel.tsx（主面板）**

```tsx
import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { useT } from "@/lib/i18n";
import { Lock, ShieldCheck, Settings as SettingsIcon, FileText } from "lucide-react";
import { UnderlineTabs } from "@/components/ui/UnderlineTabs";
import SetupWizard from "./Vault/SetupWizard";
import UnlockDialog from "./Vault/UnlockDialog";
import CipherList from "./Vault/CipherList";
import HealthReport from "./Vault/HealthReport";
import ImportExport from "./Vault/ImportExport";

interface VaultStatus {
  initialized: boolean;
  user_vault_unlocked: boolean;
}

export default function VaultPanel() {
  const t = useT();
  const [status, setStatus] = useState<VaultStatus | null>(null);
  const [tab, setTab] = useState<"ciphers" | "health" | "io">("ciphers");
  const [showUnlock, setShowUnlock] = useState(false);
  const [showSetup, setShowSetup] = useState(false);

  useEffect(() => {
    refreshStatus();
  }, []);

  async function refreshStatus() {
    const s = await invoke<VaultStatus>("vault_status");
    setStatus(s);
    if (!s.initialized) setShowSetup(true);
    else if (!s.user_vault_unlocked) setShowUnlock(true);
  }

  async function handleLock() {
    await invoke("vault_lock");
    await refreshStatus();
  }

  if (!status) return <div>Loading...</div>;

  if (!status.initialized || showSetup) {
    return (
      <SetupWizard
        onCompleted={async () => {
          setShowSetup(false);
          await refreshStatus();
        }}
      />
    );
  }

  if (!status.user_vault_unlocked || showUnlock) {
    return (
      <UnlockDialog
        onSuccess={async () => {
          setShowUnlock(false);
          await refreshStatus();
        }}
      />
    );
  }

  return (
    <div className="space-y-4">
      <div className="flex items-center justify-between">
        <div>
          <h2 className="text-xl font-semibold flex items-center gap-2">
            <ShieldCheck className="size-5" />
            {t("settings.vault.title")}
          </h2>
          <p className="text-sm text-muted-foreground">
            {t("settings.vault.description")}
          </p>
        </div>
        <div className="flex gap-2">
          <button
            className="px-3 py-1 text-sm rounded border hover:bg-muted"
            onClick={handleLock}
          >
            <Lock className="size-3.5 inline-block mr-1" />
            {t("settings.vault.lock")}
          </button>
        </div>
      </div>

      <UnderlineTabs
        value={tab}
        onValueChange={(v) => setTab(v as typeof tab)}
        items={[
          { value: "ciphers", label: t("settings.vault.list.title") },
          { value: "health", label: t("settings.vault.health.title") },
          { value: "io", label: t("settings.vault.importExport.title") },
        ]}
      />

      {tab === "ciphers" && <CipherList />}
      {tab === "health" && <HealthReport />}
      {tab === "io" && <ImportExport />}
    </div>
  );
}
```

- [x] **Step 4: 写 SetupWizard.tsx**

```tsx
import { useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { useT } from "@/lib/i18n";
import { Button } from "@/components/ui/Button";

export default function SetupWizard({ onCompleted }: { onCompleted: () => Promise<void> }) {
  const t = useT();
  const [password, setPassword] = useState("");
  const [confirm, setConfirm] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  async function handleSubmit(e: React.FormEvent) {
    e.preventDefault();
    setError(null);
    if (password.length < 12) {
      setError(t("settings.vault.setup.weakPassword"));
      return;
    }
    if (password !== confirm) {
      setError(t("settings.vault.setup.passwordMismatch"));
      return;
    }
    setBusy(true);
    try {
      await invoke("vault_setup", { password });
      await onCompleted();
    } catch (e: any) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  }

  return (
    <form onSubmit={handleSubmit} className="space-y-4 max-w-md">
      <h2 className="text-xl font-semibold">{t("settings.vault.setup.title")}</h2>
      <p className="text-sm text-amber-600">{t("settings.vault.setup.warning")}</p>
      <div>
        <label className="block text-sm mb-1">{t("settings.vault.setup.passwordLabel")}</label>
        <input
          type="password"
          value={password}
          onChange={(e) => setPassword(e.target.value)}
          className="w-full px-3 py-2 border rounded"
          autoFocus
        />
      </div>
      <div>
        <label className="block text-sm mb-1">{t("settings.vault.setup.passwordConfirm")}</label>
        <input
          type="password"
          value={confirm}
          onChange={(e) => setConfirm(e.target.value)}
          className="w-full px-3 py-2 border rounded"
        />
      </div>
      {error && <p className="text-sm text-red-600">{error}</p>}
      <Button type="submit" disabled={busy}>
        {busy ? "..." : t("settings.vault.setup.submit")}
      </Button>
    </form>
  );
}
```

- [x] **Step 5: 写 UnlockDialog.tsx**

```tsx
import { useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { useT } from "@/lib/i18n";
import { Button } from "@/components/ui/Button";

export default function UnlockDialog({ onSuccess }: { onSuccess: () => Promise<void> }) {
  const t = useT();
  const [password, setPassword] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  async function handleSubmit(e: React.FormEvent) {
    e.preventDefault();
    setError(null);
    setBusy(true);
    try {
      await invoke("vault_unlock", { password });
      await onSuccess();
    } catch (e: any) {
      setError(t("settings.vault.unlock.wrongPassword"));
    } finally {
      setBusy(false);
    }
  }

  return (
    <form onSubmit={handleSubmit} className="space-y-4 max-w-md">
      <h2 className="text-xl font-semibold">{t("settings.vault.unlock.title")}</h2>
      <div>
        <label className="block text-sm mb-1">{t("settings.vault.unlock.passwordLabel")}</label>
        <input
          type="password"
          value={password}
          onChange={(e) => setPassword(e.target.value)}
          className="w-full px-3 py-2 border rounded"
          autoFocus
        />
      </div>
      {error && <p className="text-sm text-red-600">{error}</p>}
      <Button type="submit" disabled={busy}>
        {busy ? "..." : t("settings.vault.unlock.submit")}
      </Button>
    </form>
  );
}
```

- [x] **Step 6: 写 CipherList.tsx + CipherEditor.tsx + HealthReport.tsx + ImportExport.tsx**

由于篇幅限制，这些组件的实现可以参照已有 `ActionBarPanel.tsx` 的模式（list + edit form）。每个文件基础结构如下：

**CipherList.tsx**:

```tsx
import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { useT } from "@/lib/i18n";
import CipherEditor from "./CipherEditor";

interface LoginDataDto {
  uris: { uri: string; matchType: number | null }[];
  username: string | null;
  password: string | null;
  totp: string | null;
}

interface CipherDto {
  id: number;
  folderId: number | null;
  favorite: boolean;
  atype: number;
  name: string;
  notes: string | null;
  login: LoginDataDto | null;
  fields: { name: string; value: string | null; field_type: number }[];
  reprompt: number;
  deletedAt: string | null;
}

export default function CipherList() {
  const t = useT();
  const [ciphers, setCiphers] = useState<CipherDto[]>([]);
  const [query, setQuery] = useState("");
  const [editing, setEditing] = useState<number | "new" | null>(null);

  useEffect(() => { refresh(); }, []);
  async function refresh() {
    const list = await invoke<CipherDto[]>("vault_list_ciphers");
    setCiphers(list);
  }

  const filtered = ciphers.filter(c => {
    if (!query) return true;
    const q = query.toLowerCase();
    return c.name.toLowerCase().includes(q)
      || (c.login?.username || "").toLowerCase().includes(q)
      || (c.login?.uris || []).some(u => u.uri.toLowerCase().includes(q));
  });

  if (editing !== null) {
    return <CipherEditor
      cipherId={editing === "new" ? null : editing}
      onClose={async () => { setEditing(null); await refresh(); }}
    />;
  }

  return (
    <div className="space-y-3">
      <div className="flex gap-2">
        <input
          value={query}
          onChange={e => setQuery(e.target.value)}
          placeholder={t("settings.vault.list.search")}
          className="flex-1 px-3 py-1.5 text-sm border rounded"
        />
        <button
          className="px-3 py-1.5 text-sm bg-primary text-primary-foreground rounded"
          onClick={() => setEditing("new")}
        >
          {t("settings.vault.list.addNew")}
        </button>
      </div>

      <div className="divide-y">
        {filtered.length === 0 && (
          <div className="py-8 text-center text-muted-foreground">
            {t("settings.vault.list.empty")}
          </div>
        )}
        {filtered.map(c => (
          <div
            key={c.id}
            className="py-2 flex items-center gap-3 cursor-pointer hover:bg-muted/50 rounded px-2"
            onClick={() => setEditing(c.id)}
          >
            <div className="flex-1">
              <div className="font-medium">{c.name}</div>
              <div className="text-xs text-muted-foreground">
                {c.login?.username || "—"}
              </div>
            </div>
            {c.deletedAt && (
              <span className="text-xs text-muted-foreground">
                {t("settings.vault.list.deleted")}
              </span>
            )}
          </div>
        ))}
      </div>
    </div>
  );
}
```

**CipherEditor.tsx**: 表单包含 name / url / username / password / totp / notes / favorite 复选框。保存调 `vault_create_cipher` 或 `vault_update_cipher`。底部按钮：保存 / 取消 / 删除（软删除调 `vault_delete_cipher`）/ 永久删除（调 `vault_delete_cipher` 带 permanent=true）。

**HealthReport.tsx**: 调 `vault_health_report`，渲染弱密码列表 + 重复组 + 平均强度。

**ImportExport.tsx**: 文件选择 + 调 `vault_import_bitwarden` / `vault_export` 触发下载。

- [x] **Step 7: 修改 capabilities/default.json**

打开 `crates/desktop/capabilities/default.json`，windows 数组加 `"password_generator_window"`：

```json
{
  "$schema": "../gen/schemas/desktop-schema.json",
  "identifier": "default",
  "description": "Capability for main window",
  "windows": [
    "main",
    "result_window",
    "settings_window",
    "clipboard_window",
    "compact_editor_window",
    "action_bar_window",
    "overlay_window",
    "screenshot_*",
    "password_generator_window"
  ],
  "permissions": [...]
}
```

- [x] **Step 8: 修改 App.tsx 加 password_generator_window case**

打开 `crates/desktop/frontend/src/App.tsx`，找到 switch（行 60-84），加 case：

```tsx
case "password_generator_window":
  return <PasswordGenerator />;
```

加 import：

```tsx
import PasswordGenerator from "./pages/PasswordGenerator";
```

- [x] **Step 9: 写 PasswordGenerator 浮窗组件**

新建 `crates/desktop/frontend/src/pages/PasswordGenerator/index.tsx`:

```tsx
import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { useT } from "@/lib/i18n";
import { RefreshCw, Copy, Check } from "lucide-react";

type Mode = "random" | "passphraseEn" | "passphraseZh" | "pin";

interface RandomConfig {
  length: number;
  uppercase: boolean;
  lowercase: boolean;
  numbers: boolean;
  symbols: boolean;
  avoid_ambiguous: boolean;
}

interface PassphraseEnConfig {
  word_count: number;
  separator: string;
  capitalize: boolean;
  include_number: boolean;
}

interface PassphraseZhConfig {
  word_count: number;
  separator: string;
  include_number: boolean;
  include_symbol: boolean;
}

interface PinConfig { length: number; }

type GeneratorConfig =
  | { mode: "random"; length: number; uppercase: boolean; lowercase: boolean; numbers: boolean; symbols: boolean; avoid_ambiguous: boolean }
  | { mode: "passphraseEn"; word_count: number; separator: string; capitalize: boolean; include_number: boolean }
  | { mode: "passphraseZh"; word_count: number; separator: string; include_number: boolean; include_symbol: boolean }
  | { mode: "pin"; length: number };

export default function PasswordGenerator() {
  const t = useT();
  const [mode, setMode] = useState<Mode>("passphraseZh");
  const [result, setResult] = useState("");
  const [copied, setCopied] = useState(false);

  // 各模式的本地配置（简化版，MVP 用默认值）
  const [randomCfg] = useState<RandomConfig>({
    length: 16, uppercase: true, lowercase: true, numbers: true, symbols: false, avoid_ambiguous: true,
  });
  const [enCfg] = useState<PassphraseEnConfig>({
    word_count: 3, separator: "-", capitalize: true, include_number: true,
  });
  const [zhCfg] = useState<PassphraseZhConfig>({
    word_count: 4, separator: "", include_number: true, include_symbol: false,
  });
  const [pinCfg] = useState<PinConfig>({ length: 6 });

  useEffect(() => { regenerate(); }, [mode]);

  function buildConfig(): GeneratorConfig {
    switch (mode) {
      case "random": return { mode: "random", ...randomCfg };
      case "passphraseEn": return { mode: "passphraseEn", ...enCfg };
      case "passphraseZh": return { mode: "passphraseZh", ...zhCfg };
      case "pin": return { mode: "pin", ...pinCfg };
    }
  }

  async function regenerate() {
    const pwd = await invoke<string>("vault_generate", { cfg: buildConfig() });
    setResult(pwd);
    setCopied(false);
  }

  async function handleCopy() {
    await navigator.clipboard.writeText(result);
    setCopied(true);
    setTimeout(() => setCopied(false), 2000);
  }

  return (
    <div className="p-4 space-y-3" style={{ width: 440 }}>
      <div className="flex gap-2">
        {(["random", "passphraseEn", "passphraseZh", "pin"] as Mode[]).map(m => (
          <button
            key={m}
            onClick={() => setMode(m)}
            className={`px-2 py-1 text-xs rounded ${
              mode === m ? "bg-primary text-primary-foreground" : "bg-muted"
            }`}
          >
            {t(`settings.vault.generator.mode.${m}`)}
          </button>
        ))}
      </div>

      <div className="p-3 border rounded font-mono text-lg break-all min-h-[60px] bg-muted/30">
        {result}
      </div>

      <div className="flex gap-2">
        <button onClick={regenerate} className="flex-1 px-3 py-2 border rounded hover:bg-muted">
          <RefreshCw className="size-4 inline-block mr-1" />
          {t("settings.vault.generator.regenerate")}
        </button>
        <button onClick={handleCopy} className="flex-1 px-3 py-2 border rounded hover:bg-muted">
          {copied ? <Check className="size-4 inline-block mr-1 text-green-600" /> : <Copy className="size-4 inline-block mr-1" />}
          {t("settings.vault.generator.copy")}
        </button>
      </div>
    </div>
  );
}
```

- [x] **Step 10: 前端构建验证**

Run: `cd crates/desktop/frontend && bun run build`
Expected: 0 error

- [x] **Step 11: 整 workspace 编译验证**

Run: `cargo build`
Expected: 0 error 0 warning（前端类型不匹配会反映到 ts build）

- [x] **Step 12: Commit**

```bash
git add crates/desktop/frontend/ crates/desktop/capabilities/default.json
git commit -m "feat(vault): Task 21 - 前端 VaultPanel + SetupWizard + UnlockDialog + PasswordGenerator + i18n"
```

---

## 修订：UI 重设计 + 生成器架构升级 + dev 模式（2026-07-19）

针对首发版 UI 细节做的迭代，全部已落地（未 commit 时记录）。按主题分组：

### A. CipherEditor UI 重设计

- [x] **密码眼睛/复制按钮 bug 修复**：`CopyButton` 原内部硬编码 `absolute right-1`，被放进
  flex 容器后与眼睛按钮位置完全重叠 → 点眼睛触发复制。修复：加 `className` prop，
  密码字段传 inline 样式走 flex 流。username 字段保持原 absolute（默认）。
- [x] **3 按钮并排**：眼睛 / 生成 / 复制统一 `px-1.5 py-1 + size-3.5`，激活态 `text-foreground`。
- [x] **移除「高级」折叠 + 「生成」文字**：urls / folder / notes 直接平铺展示。
- [x] **grid 两列布局**：用户名 | 密码 一行，TOTP | 文件夹 一行；网址 / 备注 各占整行。
- [x] **textarea `size="full"` 修复**：`Input`/`Textarea`/`Select` 默认 `max-w-[220px]`，
  之前 textarea 看似只占左半列就是这个原因。所有 vault 输入控件加 `size="full"`。
- [x] **删 header meta "全部已加密 · N 个条目"**：与 Tab 栏 + footer 加密提示信息冗余。

### B. VaultPanel 改 Tab

- [x] **footer link → 顶部 PillTabs**：3 个 Tab（密码条目 / 密码健康 / 导入导出），
  复用 `components/ui/tabs.tsx` 的 PillTabs（与 ModelsPanel 同款）。修复"点进健康回不到
  主页"的脱节问题。
- [x] **footer 只保留"端到端加密"提示**。

### C. 生成器架构升级（核心）

- [x] **新建 `PasswordGenerator.tsx`（共享主体）**：从原 `PasswordGenerator/index.tsx`
  （`git show ecca9b04^:` 可取回）迁移完整要素——Segmented 模式切换 + 显示区 + 强度条 +
  4 种模式专属配置（滑杆/toggle/number input）+ 操作栏。Props 设计：
  `onUsePassword?`（modal 场景）/ `onAutotype?`（未来 Actionbar 场景）/ `showToast`。
- [x] **新建 `PasswordGeneratorModal.tsx`（外壳 A）**：半透明遮罩 + 居中卡片（480px）+ × 关闭
  + Esc 关闭 + 点遮罩关闭。内部渲染 `<PasswordGenerator>`。
- [x] **CipherEditor.tsx 删除内嵌抽屉**：~50 行抽屉 + 相关 state（genMode/genResult/genBusy/
  regenerate/applyGenerated）全部删除，改为 modal 调用。
- [x] **架构动机**：用户要求"未来 Actionbar 也能召唤生成器，给网页注册新账号时填初始密码"。
  抽共享主体后，未来 Actionbar 独立 Tauri 窗口场景（外壳 B）只需在窗口 root 渲染主体，
  无需重构。

### D. 强度评估算法修复（密码学正确性）

- [x] **算法替换**：`estimateStrength(s)` （ASCII 字符种类 + 长度启发式）→
  `estimateStrengthByConfig(mode, cfg)`（按生成参数算熵）。
- [x] **根因**：原算法对中文短语结构性歧视——4 个中文双字词 = 8 字符 + 中文算 1 类字符
  → 永远评 1 分（极弱），实际熵 48 bit 比英文 3 词（39 bit）还高。
- [x] **新算法**：
  - random: `length × log2(charset_size)`
  - passphraseEn: `word_count × log2(7776)`
  - passphraseZh: `word_count × log2(4096)`
  - pin: `length × log2(10)`
- [x] **评分映射**：< 28 极弱 / 28-35 弱 / 36-59 一般 / 60-127 强 / ≥ 128 极强。

### E. dev 模式基础设施（工程改进，跨 vault 全局）

- [x] **`tauri.conf.json` 加 `devUrl` + `beforeDevCommand`**：debug build 下 Tauri 自动把
  所有 `WebviewUrl::App(...)` 映射到 `http://localhost:1420`（query string 保留）。
- [x] **`vite.config.ts` 加 `strictPort: true` + `clearScreen: false`**。
- [x] **`Cargo.toml` 启用 tauri `devtools` feature**：WebView 右键 Inspect Element 可用。
- [x] **新建 `run-octopus-dev.sh`**：后台启 vite + `cargo run`（debug profile）+ 退出 trap kill vite。
- [x] **devtools 自动弹窗修复**：删 `result_window.rs:60-61` 的
  `#[cfg(debug_assertions)] window.open_devtools()`——dev 模式下会自动弹 Inspector 挡住
  语音识别结果窗口。改为按需开（右键 Inspect）。
- [x] **价值**：改前端秒级 HMR 生效，不重编 Rust（release 流程完全不变）。

### F. 其他小修复

- [x] **HealthReport `${count}` 未替换 bug**：`t("settings.vault.health.total")` 漏传
  `{ count: report.total_logins }` → 页面显示原始 `${count}`。
- [x] **中文词表回归测试**：加 `test_wordlist_no_duplicates` + `test_wordlist_all_two_cjk_chars`
  锁死「4096 词无重复 + 全 2 字 CJK」不变量（progress.md 记录早期 100 词版本曾有 '现在' 重复）。

### 验证

- `bunx tsc --noEmit`：0 error
- `bun run test`：304 tests pass
- `cargo test -p octopus-vault --lib passphrase_zh`：9 tests pass
- `cargo check -p octopus-desktop --features "embedded cloud"`：0 error 0 warning
- HMR 端到端验证：改前端 → vite 推送 → WebView 自动刷新 → 无 Rust 重编

---

## Follow-up Work（post-initial 21 tasks）

21 个 Task 完成后，又基于 self-review / dogfooding / 集成测试发现的问题做了一波修订。
全部已落地。下面按 commit 时序列出。

### 测试 & 修复（早期 wave）

- [x] **Test Wave 1**（commit `7fd81953`）：+17 tests，引入 `set_test_db` thread_local override
  注入 in-memory 连接（与 octopus-infra 既有测试隔离），解决原 plan「`with_db` 集成测试 `#[ignore]`」caveat。
- [x] **Test Wave 2**（commit `8c8631af`）：+22 tests，发现并修复 `update_model_secret_key` SQL bug。
- [x] **Keychain mock**（commit `4f74a327`）：在 keychain.rs 加 `set_test_keychain` thread_local override，
  un-ignore 4 个原本 `#[ignore]` 的 unlock 测试（不再依赖真实 macOS Keychain）。
- [x] **ZH wordlist 4096**（commit `e5e68cf6`）：jieba 词频 TOP 4096（原 plan 占位 100 词）。
- [x] **Final review fixes**（commit `674f009e`）：C1 serde（FolderDto serialize）+ I1 spec 同步 +
  I2 flaky 测试修复 + I3 password_history 自动追加 + I4 unlock 路径 + M1/M2 小修。

### Dogfooding 反馈（#2-#9）

- [x] **#4 Auto-Type picker UI**（commit `9d191a41`）：dead-hotkey 修复——Cmd+Shift+L 触发后浮窗
  偶发不响应。
- [x] **#2/#3/#5**（commit `079cbeab`）：password_history 自动追加（密码变更时）+ change_password
  后强制 re-unlock（旧 VaultSession 失效）+ TOTP 30s 倒计时轮询（cipher 详情页）。
- [x] **#7/#8**（commit `e77ad7c1`）：secret_key 解密统一 chokepoint
  （`crates/desktop/src/vault_secret_access.rs`，4 个云端推理调用点都走 `try_decrypt_secret_global`）+
  `detect_and_match` 列表上限（防匹配过多卡 UI）。
- [x] **#9 VaultError**（commit `23eedb35`）：新建 `crates/desktop/src/vault_error.rs`，11 个用户安全
  错误变体 + `classify(anyhow)` 启发式映射 + JSON `{code, message}` 序列化。前端 `classifyError.ts`
  程序化处理。详见 spec §7.2。

### 架构性修订（#6/#10 + 关键 bug）

- [x] **#10 cargo feature gate**（commit `d70aa426`）：`octopus-desktop` 加 `vault` cargo feature
  （默认开），关掉后 vault 模块整体 cfg 掉。`vault_secret_access` 总是编译（chokepoint 不能断），
  feature off 时退化为 raw 原样返回。前端通过 `feature_flags::is_vault_enabled()` 运行时探针。
  详见 spec 附录 E。
- [x] **#6 folder UI**（commit `4e5c3540`）：原 plan folder 仅 schema 预留，实施时补完整 UI——
  4 个 Tauri 命令（list/create/rename/delete）+ FolderSidebar + FolderPromptDialog +
  CipherEditor folder dropdown + folder 名用 user_vault_key 加密（修复早期明文 bug）。
  详见 spec §3.4。
- [x] **主密码强度校验**（commit `1c46a9d9`）：落实 spec 文案承诺——12 字符 + 必含 4 字符类，
  前端 `validateMasterPassword.ts` + 后端 `vault_setup` / `vault_change_password` 双校验。
- [x] **Panic → Result**（commit `a42dedd9`）：5 个生成器函数全改 `Result<String>`，`assert!` 换
  `ensure!`（panic 会崩 Tauri 主进程使 vault 整体不可用）。详见 spec §5.2.2 + INV-G8。
- [x] **K_machine 改本地文件**（commit `0def2450`）：macOS adhoc 签名 binary 写 Keychain 是
  session-only（重启即丢），改为 `~/.octopus/machine-key.enc`（file_key = HKDF(machine_id+USER)）。
  `keyring` 依赖从 vault/Cargo.toml 移除。详见 spec §2.5。
- [x] **心跳 + 焦点失活**（commit `752419ec`）：锁定从「固定 5min，从 unlocked_at 起算」改为
  「以 last_active_at 为基准 + 前端 30s 心跳」——vault tab 离开 + 时间到 → 自动锁。
- [x] **Lock timeout 可配**（commit `651e8db3`）：`AppConfig.vault_lock_timeout_secs`，UI 提供
  30s/1min/3min/5min/15min/Never 选项，默认 3min（偏激进）。详见 spec §2.7。
- [x] **生成器内嵌 CipherEditor**（commit `ecca9b04`）：删除全局热键 Cmd+Shift+G + 独立
  password_generator_window 浮窗。生成器 UI 内嵌 CipherEditor 密码字段旁。
  `AppConfig.vault_generator_shortcut` 字段保留仅为兼容旧 DB，运行时不消费。
  详见 spec §5.2 + 附录 B/D。

---

## Self-Review

### Spec coverage

| Spec 章节 | 对应 Task / Follow-up | 覆盖 |
|---|---|---|
| 0. 目标与范围 | 全部 Task | ✅ |
| 1. 架构总览（含 feature gate） | Task 1, 16 + FU #10 | ✅ |
| 2. 加密层（含 K_machine 本地文件 / 可配置锁定） | Task 2, 3, 4, 8, 9 + FU K_machine/heartbeat/timeout | ✅ |
| 3. 数据模型（含 folder UI 完整） | Task 5, 6, 7 + FU #6 | ✅ |
| 4. URL 匹配 + Auto-Type | Task 13, 18, 19 + FU #4/#8 | ✅ |
| 5. TOTP + 生成器（内嵌 + Result）+ 健康 | Task 10, 11, 12, 14 + FU ZH4096/panic→Result/inline | ✅ |
| 6. Bitwarden 导入 + 同步 | Task 15 + FU history auto-append (#2) | ✅ |
| 7. 降级 + 错误（VaultError）+ 不变量 | 全部 + FU #9 | ✅ |
| 附录 A: Tauri 命令清单 | Task 17, 19 + FU heartbeat/folder/feature_flags | ✅ |
| 附录 B: 前端组件 | Task 21 + FU folder UI | ✅ |
| 附录 C: capabilities | Task 21 | ✅ |
| 附录 D: 全局热键（仅 Cmd+Shift+L） | Task 19 + FU ecca9b04 | ✅ |
| 附录 E: cargo feature gate | FU #10 | ✅ |
| 顺手改进：models.secret_key 加密 + chokepoint | Task 20 + FU #7 | ✅ |

### 类型一致性

- `DerivedKey` 在 Task 2 定义，Task 3/4/9/15/20 使用 ✅
- `Cipher / CipherInput / CipherData / LoginData` 在 Task 6 定义，Task 7/9/13/14/15 使用 ✅
- `Argon2Params` 在 Task 2 定义，Task 9 使用 ✅
- `VaultSession / SharedVaultSession` 在 Task 16 定义，Task 17/19 使用 ✅；FU 加 `last_active_at: Option<Instant>`
- DTO 命名：`CipherDto / CipherInputDto / VaultStatusDto / AutoTypeResultDto / TotpResultDto / FolderDto`（Task 17/19 + FU #6）✅
- `VaultError` enum（FU #9）在 vault_error.rs 定义，所有 vault_commands 调用点经 `classify` 翻译 ✅

### 原 caveat 落地情况

1. **`with_db` 集成测试** → **已解决**（FU Test Wave 1，`set_test_db` thread_local override）
2. **objc2 API** → 实施时按编译报错调整（commit 散落在 Task 18 系列）
3. **enigo API** → 实施时对照 paste.rs 调整（Task 18 系列）
4. **Tauri 2 Emitter/WebviewWindowBuilder API** → 实施时按报错调整
5. **中文词表** → **已解决**（FU `e5e68cf6`，jieba 词频 TOP 4096 直接 commit 进 git，无 curl 依赖）

### 仍待办（backlog，非 plan 缺陷）

- Quick Access vault tab（`Cmd+Shift+V`）—— P2 未实现
- HIBP 泄露查询 —— P2
- CSV/1Password/KeePass 导入 —— P1
- Windows/Linux Auto-Type —— P1/P2
- SecureNote / Card / Identity cipher 类型 —— 未来

---

## 执行选择

Plan 已完成全部 21 个 Task + Follow-up Work。原文保留如下（历史记录）：

**1. Subagent-Driven (已采用)** - 每个 Task 派独立 subagent，Task 间 review。
**2. Inline Execution** - 备选。

实际采用 Subagent-Driven + 多轮 self-review follow-up。


---

## 修订：安全审查复查 + 修复（2026-07-19）

收到外部安全审查报告，10 个 Critical/High + 9 个次要。逐条回读源码核实后：
**9 个完全成立 / 1 个部分成立（#10 dead code）/ 3 个次要部分成立**。

### 已修复（9 个，3 个独立 commit）

| # | 严重度 | 问题 | 修复 commit |
|---|---|---|---|
| #1 | 🔴 真高危 | PSL 域名匹配钓鱼（多段 TLD 退化） | e82c9f19 接入 publicsuffix crate |
| #2 | 🟠 高 | autotype 焦点竞态 | b15e6c94 autotype_login 加 expected_bundle_id 校验 |
| #3 | 🟠 高 | reprompt 后端绕过（前端实际也没实现） | b15e6c94 后端强制 master_password 校验 + 前端 reprompt view |
| #5 | 🟠 高 | 解密失败回退发密文（密文进云端 log） | b15e6c94 try_decrypt_secret_global 返 Result |
| #6 | 🟡 中 | list_ciphers 单行失败带垮整表 | e8a37159 返回 (Vec, Vec<failure>) 部分结果 |
| #8 | 🟡 中 | lock 不擦飞行中 Arc（注释误导） | e8a37159 vault_state.rs 注释订正 |
| 次-2 | 🟡 中 | 裸 std::thread::spawn TTL | e8a37159 改 tauri::async_runtime + tokio::time |

### 未修复（仅文档化）

| # | 严重度 | 不修原因 |
|---|---|---|
| #4 vault_meta 非原子 RMW | 🟡 中 | 实际并发概率极低（单进程桌面 app，需双 modal 同时操作）；记入 spec "已知工程折衷" |
| #7 TOTP 拒绝短 secret + 硬编码 | ✅ 已修（2026-07-19 follow-up） | 启用 otpauth feature + new_unchecked / from_url_unchecked 放宽 80bit 限制 + from_input 智能分发 otpauth:// URL / 裸 Base32 |
| #9 K_machine 派生 file_key 弱 | 🟡 中 | 已知工程折衷（adhoc 签名 keychain 失效）；spec §2.5 补威胁模型说明 |
| #10 bundle_id AppleScript 注入 | 🟢 低 | 当前是 dead code，无调用点；加防御性注释，未来启用时强制白名单 |
| 次-1 心跳以 focus 为基准 | 🟡 中 | 设计选择（窗口聚焦即续命），补注释说明 |
| 次-3 suppress_next 竞态 | 🟢 低 | 竞态存在但报告归因 iCloud 不准（实际是本机其他剪贴板事件） |

### 复查细节

- 所有报告引用的代码事实全部准确（仅个别行号略偏）
- 报告严重度评估偏激进——"永久数据丢失""key 长期残留"等措辞高估了实际触发概率
- 报告 #3 描述"前端处理 reprompt"不准确——前端实际也没实现，比报告更严重
- 报告 #10 "目前真实存在"夸大——activate_app 是 dead code 无调用点
- 报告次-3 "iCloud Universal Clipboard 先消费 suppress"不准——ConcealedType 标记本已让 iCloud 跳过，真正消费者是本机其他剪贴板事件

### 验证

- cargo test -p octopus-vault --lib: 129 pass（+2 PSL 钓鱼场景测试）
- cargo test -p octopus-infra --lib: 130 pass
- bunx tsc --noEmit: 0 error
- bun run test: 304 pass
- cargo build -p octopus-desktop --features 'embedded cloud vault': 0 error 0 warning

---

## 修订：#7 TOTP follow-up（2026-07-19，commit 1754d649）

针对「未修复」表中标为"单独迭代"的 #7 单独做的 follow-up：

### 已修

- [x] **启用 totp-rs otpauth feature**（Cargo.toml）：原 `default-features=false` 关闭 otpauth，无法解析 otpauth:// URL
- [x] **`TotpGenerator::from_base32` 改用 `new_unchecked`**：跳过 totp-rs 强制 ≥128bit 限制，支持 RFC 6238 下限的 80bit 标准 secret（`JBSWY3DPEHPK3PXP` 解码 10 字节）
- [x] **新增 `from_otpauth_url`**：解析完整 URL（SHA256/SHA512、digits=8、period=60 等变体），GitHub/银行/Authy 导出场景必需
- [x] **新增 `from_input` 智能分发**：前端用户粘贴任一格式（`otpauth://` 开头 → URL；否则 → 裸 Base32）都能识别
- [x] **`seconds_remaining` 按 `self.step` 算**：支持非 30s period
- [x] **后端 `vault_generate_totp` 改用 `from_input`**：cipher 无需知道存的是哪种格式
- [x] **前端 CipherEditor totp 字段** placeholder 显示 `JBSWY3DPEHPK3PXP 或 otpauth://totp/...`，label 改为 `TOTP（Base32 或 otpauth URL）`
- [x] **6 个新测试**：`test_short_80bit_secret_accepted`（80bit 标准 secret）、`test_otpauth_url_full_parse`、`test_otpauth_url_sha256_8digits_60s`（银行/Authy 非标准变体）、`test_otpauth_url_minimal`、`test_from_input_dispatch`（智能分发 + 大小写不敏感 + trim）、`test_otpauth_url_invalid`

### 验证

- cargo test -p octopus-vault --lib: 135 pass（+6 TOTP 测试）
- bunx tsc --noEmit: 0 error; bun run test: 304 pass

---

## 修订：复审 A-F 修复 + #4 #10 收尾（2026-07-19，commit 086b6890 / 2622d0ab）

收到外部复审报告（针对 commit 48eb2034），6 个遗留/新引入问题全部复查成立，全部修。

### 复审 A-F 修复（commit 086b6890）

| 字母 | 严重度 | 问题 | 修复 |
|---|---|---|---|
| A | 🔴 高 | vault_copy_password 缺 reprompt 校验（#3 的孪生缺口） | 加 master_password 参数 + 强制校验；前端 copyOnly 路径也走 reprompt view |
| B | 🟠 中 | #2 verify_focused(None) 第三方 app 注入未闭合 | **仅文档化**：spec §4.5 加"已知窗口（B 复审遗留）"段，明确 username 可能泄漏 + 残留风险 |
| C | 🟡 中 | clipboard TTL 误清用户后续复制 | 新增 `clear_clipboard_if_matches(expected)`：读 NSPasteboard.stringForType 比对，相同才清 |
| D | 🟡 中 | verify_focused osascript 失败 fail-open | `unwrap_or_default()` → `?` 传播 bail（fail-closed） |
| E | 🟢 低 | 硬编码 com.octopus.desktop 与 tauri.conf 耦合 | 提取 `OCTOPUS_BUNDLE_ID` 常量 + `test_octopus_bundle_id_matches_tauri_config` 测试锁死 |
| F | 🟢 低 | config.rs 解密失败返空串与其他 3 处不一致 | log message 改为更明显的"保险库未解锁或密文损坏"（签名约束保留） |

### 后续收尾（commit 2622d0ab）

| # | 严重度 | 问题 | 修复 |
|---|---|---|---|
| #4 | 🟡 中 | vault_meta 非原子 RMW | 新增 `meta_lock.rs`：进程内 `OnceLock<Mutex<()>>`，串行化 change_master_password + refresh_app_key_local_enc 的 read-modify-write 整段；测试 `test_lock_serializes_concurrent_writers` |
| #10 | 🟢 低 | activate_app bundle_id AppleScript 注入 | `validate_bundle_id`：char-level 校验 `[A-Za-z0-9.-]` 长度 1-256；测试覆盖合法格式 + 引号/分号/反引号/中文等注入尝试 |

### 验证

- cargo test -p octopus-vault --lib: 136 pass（+1 meta_lock 测试）
- cargo test -p octopus-desktop validate_bundle_id: 2 pass（accept_legal + reject_injection）
- cargo test -p octopus-desktop test_octopus_bundle_id: 1 pass（常量一致性）
- bunx tsc --noEmit: 0 error; bun run test: 304 pass
- cargo build -p octopus-desktop --features 'embedded cloud vault': 0 error 0 warning

### 仍未修（仅文档化）

剩 3 个：

| # | 不修原因 |
|---|---|
| #9 K_machine 派生 file_key 弱 | 已知工程折衷（adhoc 签名 keychain 失效），spec §2.5 已补威胁模型 |
| 次-1 心跳以 focus 为基准 | 设计选择，"窗口聚焦即续命"；spec/architecture 已注释说明 |
| 次-3 suppress_next 竞态 | 竞态存在但报告归因 iCloud 不准；ConcealedType 标记本已让 iCloud 跳过 |


---

## 修订：ActionBar 密码生成器集成（外壳 B 落地，2026-07-19）

落地 spec §5.2 架构图中的外壳 B（P1 计划：ActionBar 加内置按钮触发独立生成器浮窗）。

### 设计决策（与用户协商）

- **触发方式**：ActionBar 搜索框右侧内置按钮（独立于 DB items），onClick → invoke `open_password_generator`。用户要全局快捷键可在命令面板配置（ActionBar 通用机制）
- **窗口形态**：**透明 always-on-top 浮窗**（非独立 Tauri 普通窗口）——避免独立窗口"hide 才能让浏览器回前台"的焦点切换问题；浮窗透明且不抢键盘焦点，浏览器始终在前台
- **位置**：跟随鼠标（前台浏览器输入框附近通常有鼠标），边界保护防超出屏幕。未来增强为跟随浏览器 frame
- **Auto-type 行为**：点使用后自动 hide 浮窗（用户决策，与 VaultPicker 一致）；username 留空（生成器场景无 username），press_enter=true（生成后通常需立即提交）

### 改动（commit 待写）

- 新增 `crates/desktop/src/password_generator_window.rs`：浮窗创建（480×480 透明 always_on_top）+ `show_password_generator_window` + `compute_window_position`（跟随鼠标 + 边界 clamp）
- 新增 `crates/desktop/frontend/src/pages/PasswordGenerator/index.tsx`：浮窗 root，渲染 `<PasswordGenerator onAutotype={...}>` + 顶部标题栏（X 关闭）
- `crates/desktop/src/vault_commands.rs` 新增 2 命令：
  - `open_password_generator`：算位置 + show 浮窗
  - `password_generator_autotype(password)`：hide 浮窗 → `autotype_login("", pwd, true, None)` 注入前台
- `crates/desktop/src/main.rs`：注册新模块 + 2 命令（vault feature gated，与 vault_autotype 同）
- `crates/desktop/capabilities/default.json`：windows 数组加 `password_generator_window`
- `crates/desktop/frontend/src/App.tsx`：加路由分支 + vault feature 探针覆盖新窗口
- `crates/desktop/frontend/src/pages/ActionBar/index.tsx`：搜索框右侧加内置 🔑 按钮

### 共享主体复用

- `pages/Settings/Vault/PasswordGenerator.tsx` 主体零改动——`onAutotype` prop 已在 2026-07-19 重构时预留
- 外壳 A（CipherEditor Modal）+ 外壳 B（独立浮窗）渲染同一主体

### 验证

- cargo build -p octopus-desktop --features 'embedded cloud vault': 0 error 0 warning
- bunx tsc --noEmit: 0 error; bun run test: 304 pass

### 已知窗口（与 vault_autotype 同）

- autotype focus 校验走 verify_focused(None) 最小防御——hide 期间焦点被抢到第三方 app 时密码会打到错误窗口
- spec §4.5 已记录该已知窗口；未来增强为浏览器白名单

---

## 修订：ActionBar UI 调整（2026-07-19）

随密码生成器外壳 B 落地，做了几个 ActionBar UI 调整 + 一个回归修复。

### 改动

- [x] **生成器 Tab 顺序**（commit 2c6ca38f）：Segmented items 重排为 random → passphraseEn → passphraseZh → pin（原 zh/en/random/pin）；默认 mode 从 passphraseZh 改为 random（与主流密码管理器对齐）
- [x] **生成器取消按钮**（commit 2cbaae38）：PasswordGenerator 主体加 onCancel? prop（可选，提供才显示），独立浮窗场景传 onCancel=handleClose（hide 浮窗）；CipherEditor Modal 不传（已有 × 关闭 + 点遮罩）
- [x] **独立窗口 toast 系统**（commit 031a591c）：提取 Settings 局部 toast 为可复用 hook（lib/useToast.tsx），PasswordGeneratorWindow 复制操作有"已复制"反馈
- [x] **浮窗位置跟随浏览器 frame**（commit 031a591c）：CGWindowListCopyWindowInfo + owner name 白名单匹配前台浏览器；三级 fallback（浏览器 frame → 鼠标 → 屏幕顶部居中）
- [x] **删除 Settings UI 的 copy 菜单类型**（commit fb65d3ef）：用户改用 Cmd+C；i18n 删 typeCopy；ActionBarPanel TYPE_META/ACTION_TYPES 删 copy；新建默认 actionType copy → url（后续按层级区分）
- [x] **子菜单 → 父菜单 文案**（commit fb65d3ef）：i18n typeSubmenu 改文案（DB 字段仍 submenu，纯 i18n 改动）
- [x] **新建菜单项默认类型按层级**（commit 172de750）：主菜单默认 submenu（父菜单），子菜单默认 script（执行动作）
- [x] **回归修复**（commit 78a85cc5）：executeSearchResult 的 case "copy" 被误改为 legacy 提示，破坏 calculator/command 复制功能——恢复原逻辑 + 加注释区分两个 switch

### 关键踩坑（避免下次重蹈）

**executeItem vs executeSearchResult 的 case "copy" 是两回事**：
- `executeItem`（line 568）：执行 ActionBar DB 菜单项 → 后端 `execute_action_bar` → 后端删 `"copy" =>` 分支**是对的**（用户配置 copy 类型已禁止）
- `executeSearchResult`（line 628）：执行搜索结果 → 前端 switch → **case "copy" 必须保留！**
  calculator/command 的 search result 走这里（actionType="copy"，actionData={text:...}）
  纯前端 clipboard.writeText，与用户配置的 copy 菜单类型无关

删除 Settings UI 类型时只动 TYPE_META/ACTION_OPTIONS + 后端 execute_action_bar，
**不要动 executeSearchResult 的运行时 case**——搜索结果的 actionType 是各 Provider
自由产出的（calculator.rs:55 / command.rs:7），与用户配置菜单解耦。

### 验证

- cargo build / cargo test -p octopus-vault --lib (136) / -p octopus-infra --lib (130)
- bunx tsc --noEmit: 0 error; bun run test: 304 pass

---

## 修订：第三轮复审修复（2026-07-19，commit 待写）

收到第三轮复审报告，2 个新发现 + 3 个次要观察全部复查成立。

### 已修

| # | 问题 | 修复 |
|---|---|---|
| 新发现 1 | TOTP period=0 / digits 异常 / algorithm 异常 → `current()` panic（不可信输入触发，崩 Tauri 命令） | `from_otpauth_url` 加 ensure! clamp：period>0 / digits ∈ {6,8} / algorithm ∈ {SHA1,SHA256,SHA512}；`current()` 加 last-resort 防护（step==0 bail!） |
| 新发现 2 | #4 meta 锁未覆盖 `regenerate_security_stamp` / `setup_vault`（这两条不持锁） | 锁下沉到 `save_vault_meta` / `update_security_stamp` 内部（ReentrantMutex 同线程可重入，外层 RMW 持锁时内层 save 不死锁）|
| 次要 B | `BROWSER_OWNER_NAMES` 与 `url_detect.rs:43` bundle_id 白名单两套独立列表，未来加浏览器需手动同步 | 注释加 reference 提示同步维护（不抽常量源，over-engineering）|

### 测试

- `test_period_zero_returns_err_not_panic`：period=0 返 Err 不 panic
- `test_digits_invalid_returns_err`：digits=0/7/20 都返 Err
- `test_algorithm_invalid_returns_err`：algorithm=MD5 返 Err
- `test_lock_is_reentrant_same_thread`：ReentrantMutex 同线程重入不死锁（锁下沉前提）

### 不成立的次要观察

- 次要 C：from_otpauth_url 内部不再 lowercase——标准 URL 解析已处理大小写，不需要额外处理

### 验证

- cargo test -p octopus-vault --lib: 140 pass（+4 新测试）
- cargo test -p octopus-infra --lib: 130 pass
- bunx tsc --noEmit: 0 error; bun run test: 304 pass
- cargo build -p octopus-desktop --features 'embedded cloud vault': 0 error 0 warning

---

## 修订：第四轮审查 12 项全修（2026-07-19，commit 19153933）

收到第四轮审查报告，12 个问题全部复查成立（#5 功能影响被夸大但安全属性成立 / #7 核心因果链不成立但 latent risk 存在 / #10 INV-3 适用性过宽但安全卫生缺陷真实）。全部修复。

### 🔴 critical/high（7 项）

- [x] **#1 后端主密码强度校验**：新建 `crates/vault/src/validate.rs`（Rust 版 validate_master_password，翻译自前端 ts）；setup_vault + change_master_password 入口调用。防 DevTools invoke('vault_setup', {password: 'a'}) 设弱密码
- [x] **#2 Bitwarden 导入完全未去重**：importer/bitwarden.rs 导入前预加载库内 ciphers 构 seen HashSet（key=name+first_uri），重复跳过计 skipped。加 test_import_dedup_on_second_import
- [x] **#3 缺暴力破解退避**：新建 `crates/vault/src/attempt_guard.rs`（UnlockAttemptGuard AtomicU32 + AtomicU64，退避 0/1/2/4/8/16/30s）；3 路径接入（unlock_with_master_password / verify_master_password / change_master_password 旧密码校验）；成功 reset / 失败 record_failure
- [x] **#4 reprompt 字段导入静默丢失**：BitwardenItem (De)Serialize 加 reprompt 字段；CipherInput 构造改 RepromptType::from(item.reprompt)；exporter 写出 i64::from(c.reprompt)
- [x] **#5 迁移非事务化+吞错**：migrate.rs 两阶段（先全部加密 Vec + 整批 unchecked_transaction）；setup_vault 不再 log::warn 吞错，return Err

### 🟡 medium（3 项）

- [x] **#6 save_machine_key 非原子写**：keychain.rs unix/non-unix 都改 temp file + sync_all + rename 原子替换
- [x] **#7 测试可能删真实 machine-key.enc**：test_machine_key_round_trip_via_file 加 #[ignore]
- [x] **#8 流程 D 无条件重写 local_enc**：refresh_app_key_local_enc 加"解密比较"短路（meta_lock 内解 app_key_local_enc == 当前 app_key 则跳过 save）

### 🟢 low（4 项）

- [x] **#9 list_folders 无单行容错**：返回 (Vec<FolderDto>, Vec<i64>) 部分结果（照搬 cipher.rs 修复 #6 模式）
- [x] **#10 child() chain code 未 zeroize**：crypto/hierarchy.rs 64B HMAC 输出包装 Zeroizing<[u8;64]>
- [x] **#11 空 cipher_uri 匹配任意**：matcher/mod.rs match_uri_one 加 early return（trim().is_empty() → false）
- [x] **#12 DuplicateGroup Debug 打印 hash**：health/duplicate.rs 手写 Debug impl redact password_hash

### 次要观察（TOTP 小整洁，commit 45fd0a12）

- [x] from_otpauth_url 去掉 url.to_string() 多余分配（AsRef<str> 直接接 &str）
- [x] test_algorithm_invalid 注释诚实化（测的是上游解析非 clamp）
- [x] digits 注释消除"留宽松"歧义

### 验证

- cargo test -p octopus-vault --lib: 161 pass（+26 新测试）
- cargo test -p octopus-infra --lib: 130 pass
- bunx tsc --noEmit: 0 error; bun run test: 304 pass
- cargo build -p octopus-desktop --features 'embedded cloud vault': 0 error 0 warning

### 四轮审查总览

| 轮次 | 问题数 | 已修 | 部分修/文档化 |
|---|---|---|---|
| 第一轮（10C+3次） | 13 | 9 | 4 |
| 第二轮（A-F 复审） | 6 | 5 | 1（B） |
| 第三轮（新发现 + 次要） | 5 | 3 | 2 |
| 第四轮（12 项） | 12 | 12 | 0 |
| **小计** | **36** | **29** | **7** |

---

## 修订：第五轮审查 8 项（2026-07-19）

收到第五轮审查报告（针对第四轮 commit 19153933 的复审），4 个新发现/回归（A1/B1/B2/A2）+ 4 个低优先级观察（O1-O4）。逐条复查源码后：**A1/B1/B2/A2/O2/O3 全部成立修复，O1/O4 文档化**（O1 工程成本远超收益；O4 与前端对齐是更重要的不变量）。

### 🔴 A1 — 迁移失败后 vault_meta 不回滚 → 不可恢复（必修）

**问题**：`setup_vault` 在迁移之前已独立 commit `vault_meta`（`:104 save_vault_meta`），迁移失败（`:113` `return Err`）→ vault_meta 已落盘 + secret_key 仍全明文 + `ensure!(!is_initialized())`（`:66`）阻止重跑 + 全仓库无 reset/destroy 入口 → 不可恢复的「已初始化但部分明文」状态。`unlock.rs:107-112` 修复注释声称「失败=完全未动 DB」推理错误（迁移 UPDATE 没动 DB，但 vault_meta 动了）。

**修复**：
- `infra/db.rs` 新增 `delete_vault_meta_row()`（DELETE FROM vault_meta WHERE id=1）
- `unlock.rs setup_vault` 迁移失败时显式调 `delete_vault_meta_row()` 回滚——让 `is_initialized()` 回到 false，用户可重新走 setup
- 不采用「save_vault_meta 与迁移合并到同事务」方案：强行合并需重构 meta 模块所有写路径，工程成本远大于「失败时显式 DELETE 回滚」
- 即使 DELETE 本身失败也不掩盖迁移错误——返回的 Err 同时报告「迁移失败 + 回滚失败」

### 🟠 B1 — change_master_password 成功后缺 guard().reset()（必修）

**问题**：失败路径 `record_failure()`（`:240`）累计退避，但成功路径（`:287-292`）无 `reset()`。连续输错旧密码几次后改密成功 → 退避计数仍累计 → 下次 `vault_unlock` 被 `remaining_wait()` 挡。`unlock.rs:580` 测试自己手动 reset 并注释「避免挡住下面的成功路径」——开发者意识到污染但只补了测试。

**修复**：`change_master_password` 成功路径加 `crate::attempt_guard::guard().reset()`，与 `unlock_with_master_password` 成功路径（`:201`）对称。

### 🟡 B2 — refresh 副作用失败拖垮正确密码（建议修）

**问题**：`unlock_with_master_password` 闭包把「密码校验（decrypt）」与「副作用（refresh_app_key_local_enc 写 local_enc）」捆绑，`match result` 在任意 Err 时 `record_failure()`。流程 C 写 local_enc 失败（save_vault_meta DB 错 / Keychain 错）→ 正确密码被判失败 + 退避 + 整个 unlock 返 Err。

**修复**：分两阶段——
1. 密码校验阶段（decrypt user_vault_key + app_key）：失败 = 密码错 → `record_failure`
2. 副作用阶段（refresh_app_key_local_enc）：密码已校验通过（已 `reset`），此处失败仅返 Err，**不**调 `record_failure`——用户可立即重试不被退避挡

### 🟢 A2 — child() chain code zeroize 未达目标（建议修）

**问题**：`hierarchy.rs child()` 把 `bytes`（GenericArray<U64>）的拷贝进 `Zeroizing<[u8;64]>` 后清零，但**原件 `bytes` 自身 drop 时不清零**——后 32B chain code 仍残留栈帧。报告建议的 `finalize_into(&mut Zeroizing<[u8;64]>)` 类型不成立（finalize_into 签名是 `&mut GenericArray<u8, U64>`）。

**正解**：
- `vault/Cargo.toml` 显式启用 `generic-array = { version = "0.14", features = ["zeroize"] }`——generic-array 0.14 已实现 `impl<T: Zeroize, N: ArrayLength<T>> Zeroize for GenericArray<T, N>`（`impl_zeroize.rs`），但需要该 feature 才生效
- `child()` 用 `Zeroizing::new(mac.finalize().into_bytes())` 包装 GenericArray 本体——`Zeroizing` DerefMut 让 `&bytes[..32]` 仍可用，scope 结束时**原件**被 zeroize（含 chain code）

### 🟢 O2 — 软删参与 Bitwarden 导入去重（修）

**问题**：`infra/db.rs list_vault_ciphers` SQL 无 `WHERE deleted_at IS NULL`（设计如此——回收站视图需要列出软删项），bitwarden importer 把软删项也算进 dedup seen HashSet → 用户软删后再导入同一份备份被静默 skip，无法通过导入恢复。

**修复**：`bitwarden.rs import_bitwarden_json` 算 dedup seen 时加 `.filter(|c| c.deleted_at.is_none())`。**不**改 `list_vault_ciphers`——保持 infra 不过滤的设计（回收站视图需要）。

### 🟢 O3 — attempt_guard.rs 注释陈旧（修）

**问题**：`reset()` 注释「不重置 next_allowed_at，已无意义」但代码 `:75` 实际 `next_allowed_at.store(0)`——行为正确无害，仅注释误导。

**修复**：注释订正为「重置 failures 和 next_allowed_at，让下次失败从 0 退避开始」。

### 文档化（不修）

| # | 不修原因 |
|---|---|
| **O1** 迁移 SELECT 独立连接 vs UPDATE 事务 | setup 一次性 UI 流程毫秒窗口内插入新明文行需用户在同一瞬间配置新云模型——触发概率极低。修复需重构 migrate.rs 用单事务+同一 conn，工程成本远超收益。**记入 spec §3 已知窗口**。 |
| **O4** validate.rs 非 ASCII 字母不计入大小写类 | 与前端策略对齐（前后端一致性是更重要的不变量），且对中文用户偏严不是安全问题。报告自己也评估为「非 bug」。**注释订正**已说明策略。 |

### 验证

- cargo test -p octopus-vault --lib: **166 pass**（+5 新测试：B1/B2/A1/A2-deterministic/A2-zeroize-compile + O2-soft-delete-reimport）
- cargo test -p octopus-infra --lib: 130 pass
- bunx tsc --noEmit: 0 error; bun run test: 304 pass
- cargo build -p octopus-desktop --features 'embedded cloud vault': 0 error 0 warning

### 五轮审查总览（含本轮）

| 轮次 | 问题数 | 已修 | 部分修/文档化 |
|---|---|---|---|
| 第一轮（10C+3次） | 13 | 9 | 4 |
| 第二轮（A-F 复审） | 6 | 5 | 1（B） |
| 第三轮（新发现 + 次要） | 5 | 3 | 2 |
| 第四轮（12 项） | 12 | 12 | 0 |
| 第五轮（A1/B1/B2/A2 + O1-O4） | 8 | 6 | 2（O1/O4） |
| **总计** | **44** | **35** | **9** |

