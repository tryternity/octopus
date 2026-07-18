# 密码管理功能调研报告

> **日期**：2026-07-18
> **分支**：`research_password_vault`
> **目标**：为 octopus 引入「密码生成与保存 + 自动填充」功能（特别是 actionbar 打开网站时自动匹配并填充登录凭证），调研主流密码管理器的实现方案，给出技术选型与路线建议。
> **调研对象**：vaultwarden、bitwarden/clients、bitwarden/server、gopass、rbw、passbolt、nodewarden、1Password、Bitwarden。

---

## 0. TL;DR（执行摘要）

### 核心结论
1. **octopus 当前完全没有密钥存储机制**——`models.secret_key` 明文存 SQLite，没有任何加密依赖（无 keyring/aes/argon2/stronghold）。引入密码功能需要**从零搭建加密层**。
2. **actionbar 打开网站走「系统默认浏览器」**（`open`/`xdg-open`/`cmd start`），不是内嵌 WebView。这意味着"自动填充网页表单"有三条可选路径：**①浏览器扩展 ②桌面 Auto-Type（聚焦浏览器窗口后模拟键盘）③改为内嵌 Tauri WebView 窗口**。这是个**关键架构决策点**，建议作为下一阶段 brainstorming 的主题。
3. **加密方案推荐走「rbw / Bitwarden 客户端」心智模型**：主密码 → Argon2id 派生 master key → 解开 user symmetric key → 用 AES-256-GCM 加密每个 cipher 字段。**Tauri 主进程本身充当 agent**（内存缓存解锁态），不需要 spawn 独立 daemon。
4. **不建议照搬任何一整套现有方案**：
   - passbolt（PGP + 团队协作）——过度设计，octopus 是单用户工具
   - gopass（文件即数据库 + GPG）——推翻现有 SQLite 体系，得不偿失
   - 完整 Bitwarden 兼容协议——服务端 / 多设备同步 / 账号体系，octopus 单机用不上
5. **MVP 三件套**：①Argon2id 加密 vault（密文存 SQLite）②密码生成器 ③Quick Access 浮窗 + Auto-Type 填充（沿用 actionbar 热键机制）。

### 已识别的关键决策点（需要 brainstorming）
| # | 决策 | 影响 |
|---|---|---|
| D1 | **填充路径**：浏览器扩展 / 桌面 Auto-Type / 内嵌 WebView | 决定整个工程的形态与复杂度 |
| D2 | **是否兼容 Bitwarden 协议**（让用户复用已有 vault） | 决定加密层的实现复杂度 |
| D3 | **是否支持云同步 / 多设备** | 决定是否要做账号体系、服务端 |
| D4 | **API Key（`models.secret_key`）是否一并纳入加密** | 安全改进范围 |

---

## 1. octopus 现状（决定建议方向）

### 1.1 actionbar 是什么

actionbar 是 **Alfred/Raycast 风格的全局命令面板**：
- 热键（默认 `Cmd+Shift+Space`）唤起浮窗，浮窗 label = `"action_bar_window"`（`crates/desktop/src/action_bar_window.rs:6`）
- 选中文本时在鼠标旁弹出，无选中时居中
- 承载菜单动作 / 搜索 / AI / 脚本 / URL 跳转等

**核心后端文件**（`crates/desktop/src/`）：
- `action_bar_commands.rs`（93KB / 2146 行）——所有 Tauri 命令、动作执行、URL 打开
- `action_bar_window.rs`（7.7KB）——浮窗窗口生命周期
- `action_hotkey.rs`（9.6KB）——菜单项全局快捷键

**核心前端目录**（`crates/desktop/frontend/src/pages/ActionBar/`）：`index.tsx`（1127 行）、`SearchPanel.tsx`、`urlDetect.ts` 等。

### 1.2 打开网站的完整链路（关键发现）

**结论：走系统默认浏览器，非内嵌 WebView**。共 3 套打开 URL 的 API，最终全部走 OS 命令：

| 路径 | 实现位置 | 调用方 |
|---|---|---|
| A. 菜单项 `action_type="url"` | `action_bar_commands.rs:1606-1636` | `execute_action_bar` → spawn `open`/`xdg-open`/`cmd start` |
| B. 搜索结果 quicklink | `search_commands.rs:91` `open_url` | macOS-only（缺跨平台分支，待清理） |
| C. Tauri plugin-opener | `main.rs:205` 注册 | `@tauri-apps/plugin-opener` `openUrl` |

路径 A 的安全设计（已有）：URL 模板渲染时做 **scheme 白名单**，只放行 `http/https`，防止 `smb://` `file:///` `vnc://` 等危险 scheme 触发系统操作。

### 1.3 网站/URL 数据存储（关键发现）

**没有独立的"网站表"**。actionbar 的"网站"全部存在通用菜单表 `action_bar_items` 里：

```sql
-- crates/infra/src/db.sql:253-274
CREATE TABLE IF NOT EXISTS action_bar_items (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    parent_id INTEGER DEFAULT NULL,
    title TEXT NOT NULL,
    icon TEXT NOT NULL DEFAULT '',
    action_type TEXT NOT NULL,           -- 'submenu'|'ai'|'url'|'script'|'agent'|'copy'|...
    action_data TEXT NOT NULL DEFAULT '', -- url 类型: URL 模板 https://.../?q={text}
    trigger_keyword TEXT NOT NULL DEFAULT '',
    global_shortcut TEXT NOT NULL DEFAULT '',
    accepts TEXT NOT NULL DEFAULT 'text', -- 'text'|'file'|'any'
    ...
);
```

URL 类型种子（`db.sql:277-293`）：`网页`（空 actionData，用选中文本作 URL）、`Google`、`百度`、`Bing`（搜索模板）。

### 1.4 现有数据库 schema（v37）

schema 真相源：`crates/infra/src/db.sql`（无历史迁移链，改 schema 改 db.sql + 升 `user_version`，当前 v37）。表清单：

| 表 | 用途 |
|---|---|
| `models` | ASR/LLM/OCR/Translate 模型注册，**含 `secret_key` 明文存 API key** |
| `prompts` | 润色提示词 |
| `app_config` | key-value 应用配置（已从 yaml 迁过来，`db.rs:508`） |
| `clipboard_history` + `clipboard_history_fts` | 剪贴板历史 + FTS5 |
| `image_data` | 图片 BLOB |
| `action_bar_items` | ActionBar 菜单/URL/动作 |
| `script_runs` | 脚本执行记录 |
| `hotwords` / `hotword_sets` / `hotword_hits` | 热词 |
| `agent_adapters` / `agent_tasks` | 用户自定义 agent |
| `launcher_index` / `search_frequency` | 启动器与搜索频次 |

**凭证/密码类表：完全不存在**。搜索 `password`/`credential`/`login` 关键词在 Rust/TS 源码中零命中。

### 1.5 配置与密钥存储（关键缺口）

- **配置**：`~/.octopus/octopus.db`（SQLite 单文件，WAL 模式），YAML 已废弃（仅作迁移源）。
- **API Key 存储**：`models.secret_key`（`db.sql:15`）明文存，注释明确「`is_local=0` 时是 API Key」。
- **密钥存储机制：完全没有**。搜索 `keyring` / `keychain` / `secrecy` / `Secret<` / `aes-gcm` / `argon2` / `stronghold` 在所有 Cargo.toml 与 .rs 源码中**零命中**。
- 应用日志 `~/.octopus/logs/action-bar.log` 可能含选中敏感文本，但脚本临时文件已用 0o600 权限保护（`action_bar_commands.rs:1351`）。

### 1.6 平台原生 API 使用经验

octopus 已大量使用 `objc2::msg_send!`（`action_bar_commands.rs:546-554` 读 NSPasteboard、`action_bar_window.rs:56-65` 读 NSWindow）。**扩展到 macOS Keychain Services（`SecItemAdd` 等）路径成熟**。

### 1.7 前端架构

- **栈**：React 19.2 + Vite 8 + TypeScript 6 + Tailwind CSS 4 + Radix UI（shadcn 风格）+ lucide-react + CodeMirror 6 + Tauri 2.11
- **包管理**：Bun
- **路由**：按 Tauri 窗口 label 分发（`App.tsx:60-84`）
- **i18n**：已有 `settings.actionBar.typeUrl` 等标签可仿照

---

## 2. 各产品调研结果

### 2.1 vaultwarden（Rust 服务端，本地 `/Users/wudarui/workspace/agent/vaultwarden`）

**定位**：Bitwarden 兼容的 Rust 服务端实现，与 Bitwarden 客户端 2025.12.0 兼容。

#### 数据模型（核心实体）

**Cipher（密码条目，最关键）** — `src/db/models/cipher.rs:35-62`：
```
uuid, user_uuid, organization_uuid,
atype: i32,                       -- 1=Login 2=SecureNote 3=Card 4=Identity 5=SshKey
name: String,                     -- 密文（Base64）
notes: Option<String>,            -- 密文
fields: Option<String>,           -- JSON 数组，每项 name/value 均加密
data: String,                     -- 类型相关 payload（uris/username/password/totp/passkey），全加密
key: Option<String>,              -- 单条目级密钥（被 user key 加密），2024.2.0+
password_history: Option<String>, -- JSON 数组，密码历史（加密）
deleted_at, reprompt              -- reprompt: 0=None 1=Password
```

**关键洞察**：除 `uuid/created_at/user_uuid/atype/deleted_at/reprompt` 等元数据外，**所有敏感字段（name/notes/fields/data/password_history/key）都是客户端加密好的密文，服务端只存原样**。

**User** — `src/db/models/user.rs:29-72`：
- `password_hash` / `salt` / `password_iterations` — master password 的 PBKDF2 验证参数
- `akey` — **被 master key 加密的对称主密钥**（Base64）
- `private_key` / `public_key` — RSA 密钥对（私钥用 user key 加密）
- `client_kdf_type`（0=PBKDF2, 1=Argon2id）/ `client_kdf_iter`（默认 600_000）/ `client_kdf_memory` / `client_kdf_parallelism`
- `security_stamp` — 任何密钥变更都刷新，使所有 JWT 立即失效
- `equivalent_domains` / `excluded_globals` — URI 等价域配置

其他：`Folder`（加密 name）、`Collection`（加密 name + 多对多权限）、`Organization`（RSA 密钥对）、`Device`、`Attachment`（带 `akey` 独立密钥）、`TwoFactor`、`AuthRequest`（跨设备登录审批）、`Send`。

#### 加密方案（最重要）

**vaultwarden 服务端是零知识（zero-knowledge）的**——只看到密文，从不加密/解密 vault 内容。

证据：
- **没有任何 AES/HKDF crate**，加密相关只有 `ring`（PBKDF2/HMAC/SHA256）、`subtle`（常量时间比较）、`argon2`（仅 ADMIN_TOKEN PHC 校验，不参与用户密钥派生）、`totp-lite`、`webauthn-rs`、`yubico_ng`
- `update_cipher_from_data`（`ciphers.rs:395-576`）对 `name/notes/fields/data` 只 `to_string()` 后直接落库，从不解密
- 服务端校验仅限：长度、JSON 结构、兼容性修补

**服务端唯一参与的密码学**：验证客户端传来的 password_hash（`User::set_password` `user.rs:192-215`）。注意是 **double-hash**（对客户端已经 PBKDF2 过的 hash 再做一次 PBKDF2，salt=64B 随机，iter 默认 600_000），防止 hash 本身被泄露后能直接登录。

#### 客户端加密流程（依据 Bitwarden 协议反推，注释多次引用 upstream）

1. **注册 / 改密码**：客户端用 `master password + email(lowercase,trim,作 salt) + client_kdf_iter` 经 PBKDF2（默认 600k 轮）或 **Argon2id**（m=64MiB,t=3,p=4 推荐值）派生 master key
2. master key 经 HKDF-Expand 派生 `enc key`（32B）+ `mac key`（32B），即 stretched key
3. 用 stretched key 经 **AES-256-CBC + HMAC-SHA256**（Encrypt-then-MAC）加密新生成的 user key → 写入 `User.akey`
4. 同时生成 RSA-2048 密钥对，私钥用 user key 加密 → `User.private_key`
5. 客户端把 `{password_hash, protected_user_key, kdf_params, public_key, encrypted_private_key}` POST 到 `/accounts/register`
6. **登录**：prelogin 拿 KDF 参数 → 客户端算 master key + password hash → `POST /connect/token` → 服务端 double-hash 比对 → 返回 `akey`（加密的 user key）、KDF 参数、JWT → 客户端用 master key 解 `akey` 拿到 user key
7. **每个 cipher 字段**：用 user key（或 cipher key）做 AES-CBC + HMAC 加密

#### API 设计

身份与密钥交换（`src/api/identity.rs:44-57`）：
- `POST /accounts/prelogin` — 返回 KDF 参数
- `POST /connect/token` — 核心，`grant_type` 支持 5 种：`password` / `refresh_token` / `client_credentials`（API key，绕过 2FA）/ `authorization_code`（SSO）/ `send_access`
- JWT 用 **RS256**，claims 含 `security_stamp` 让密码重置能立即让其它 token 失效

`/sync` 一次性返回 `profile, folders, collections, policies, ciphers, domains, sends, userDecryption`，用 `CipherSyncData`（`ciphers.rs:139`）做 N+1 优化。

#### URI 匹配与 autofill

**vaultwarden 服务端不做 URI 匹配**，匹配完全在客户端。服务端只做：
1. **存储 `match` 字段**：每个 login URI 在 `Cipher.data` JSON 里是 `{uri, match}`，取值：0=Domain（默认）/ 1=Host / 2=Exact / 3=StartsWith / 4=RegularExpression / null=客户端默认
2. **下发默认匹配策略**：`OrgPolicyType::UriMatchDefaults = 16`
3. **等价域名**：`User.equivalent_domains` + 全局等价域名表 `src/static/global_domains.json`

#### 2FA（TOTP 实现最简）

支持：TOTP / Email / Duo / YubiKey / WebAuthn / RecoveryCode。TOTP 实现（`src/api/core/two_factor/authenticator.rs:115-181`）：
- 库：`totp-lite`，`totp_custom::<Sha1>(30, 6, secret, time)` — HMAC-SHA1, 30s, 6 位
- secret：20 字节随机，Base32 编码
- 时间漂移：默认 ±1 步（前后 30s），可由 `AUTHENTICATOR_DISABLE_TIME_DRIFT=true` 关闭
- 防重放：记录 `last_used` time_step

#### Send（临时密码分享）

`src/db/models/send.rs` + `src/api/core/sends.rs`。访问 ID = `BASE64URL_NOPAD(UUID v4)` 不暴露内部主键。访问密码 PBKDF2(100_000) 校验。自动过期/删除用 `job_scheduler_ng` 定时任务（`sends.rs:63-69`）。

#### 给 octopus 复用的关键设计模式

1. **零知识设计**：服务端只验证 hash（double-hash 防 hash 泄露），其余字段全部透明存密文
2. **双层密钥**：master key（KDF 派生，从不离开客户端）保护 user key（随机生成的对称密钥）；user key 保护 vault 数据。**改密码只需重新加密 user key**，不用重加密整个 vault
3. **cipher 级密钥**（`Cipher.key`，2024.2.0+）：每个 cipher 独立密钥，便于分享/部分授权
4. **prelogin 返回 KDF 参数**：避免客户端在登录前就知道派生参数
5. **security_stamp**：敏感变更刷新 stamp，所有 token 立即失效
6. **TOTP**：`totp-lite` + SHA1 + 30s + 6 位 + ±1 步漂移 + last_used 防重放
7. **URI 匹配**：服务端只存 `match` int + 等价域名列表，匹配逻辑放客户端

---

### 2.2 Bitwarden 官方客户端（bitwarden/clients）

**仓库**：Nx monorepo（Angular + TypeScript + RxJS），存放除移动端外所有客户端。**正在大规模迁移到 Rust SDK**（`@bitwarden/sdk-internal`），TS 层逐渐变成薄封装。

#### 浏览器扩展 autofill 流程（最相关）

源码：`apps/browser/src/autofill/`，结构：
```
apps/browser/src/autofill/
├── background/         # service worker
├── content/            # 注入页面的 content scripts
├── services/
│   ├── autofill.service.ts                    # 主逻辑（填充脚本生成）
│   ├── collect-autofill-content.service.ts    # 扫描页面表单元素
│   ├── insert-autofill-content.service.ts     # 把值写入 DOM
│   └── autofill-constants.ts                  # 字段名启发式列表
├── overlay/            # 内联自动填充菜单（iframe UI）
├── fido2/              # Passkey（WebAuthn）虚拟认证器
└── notification/       # "保存密码" 提示条
```

核心流程：
1. **content script 注入**（`trigger-autofill-script-injection.ts`）
2. **表单扫描**（`CollectAutofillContentService`）：遍历 DOM，识别 username/password/card/identity 字段，构造 `AutofillPageDetails`，字段识别用启发式（`autofill-constants.ts` 的 `UsernameFieldNames`/`PasswordFieldNames`）+ 新的 **targeting rules**（服务端下发的精确字段规则）
3. **cipher 匹配**：用当前 tab URL 在 vault 中查找
4. **生成填充脚本**（`AutofillService.generateFillScript*`）
5. **写入 DOM**（`InsertAutofillContentService`）：`el.value = ...` 并触发 `input`/`change` 事件（绕过 React/Vue 受控组件）
6. **内联菜单**（`AutofillOverlayContentService`）：输入框旁渲染 iframe

#### 桌面客户端 autofill（auto-type）—— ⭐ octopus 最该抄的部分

源码：`apps/desktop/src/autofill/`，结构：
```
├── main/                          # Electron 主进程
│   ├── main-desktop-autotype.service.ts        # 全局快捷键 + 调用 NAPI
│   └── main-ssh-agent.service.ts
├── services/
│   ├── desktop-autotype.service.ts             # 业务逻辑（cipher 匹配）
│   └── desktop-autotype-policy.service.ts
└── models/
    └── main-autotype-keyboard-shortcut.ts      # 默认 Ctrl+Alt+B
```

**实现机制**（`MainDesktopAutotypeService`）：
1. `globalShortcut.register(shortcut, callback)` 注册全局热键，**默认 `Ctrl+Alt+B`**
2. 触发时调用 `autotype.getForegroundWindowTitle()`（来自 `@bitwarden/desktop-napi`，**Rust 原生 Node 模块**）获取当前前台窗口标题
3. `matchCiphersToWindowTitle(windowTitle)` 匹配：约定 cipher URI 以 `apptitle://` 前缀开头，剩余部分作为子串匹配（大小写不敏感）
4. 取第一个匹配 cipher 的 username/password，通过 IPC 发回主进程
5. **`doAutotype`** 构造 `username + "\t" + password`，转 charCode 数组，调用 Rust NAPI **`autotype.typeInput(inputArray, keyboardShortcut)`** —— **OS 级键盘事件模拟**

**关键限制**：目前只支持 **Windows + Premium**，不使用 macOS Accessibility API / AppleScript / Windows UI Automation API，走的是更底层的键盘事件注入（类似 SendInput）。

#### 客户端加密流程

密钥层级：
```
Master Password (用户输入) + email(lowercase,trim,作 salt)
        ▼  KDF: PBKDF2-SHA256 (默认 600k) 或 Argon2id
   Master Key
        ▼  AES-CBC 解密 "Protected Symmetric Key"
   User Key (AES-256-CBC + HMAC-SHA256, 512-bit stretched)
        ▼  加密/解密每个 cipher 字段
   Vault 数据
```

关键类：
- `KeyService` — `libs/key-management/src/key.service.ts`
- `KdfConfigService` — `libs/key-management/src/kdf-config.service.ts`
- 底层 Rust 加密：`@bitwarden/sdk-internal`（npm 包，源在 `bitwarden/sdk-internal` 仓库）

**KDF 参数**：
- PBKDF2-SHA256：默认 **600,000 轮**，范围 `[600_000, 2_000_000]`，pre-login 最低 5,000（防降级）
- **Argon2id**：默认 `iterations=6, memory=32 MiB, parallelism=4`，范围 iters `[2,10]` / mem `[16,1024]` MiB / parallelism `[1,16]`
- **hashMasterKey(password, key)**：PBKDF2 **1 轮**（服务端鉴权用，不是加密用）

#### URL/域名匹配策略（5 种，octopus 直接可抄）

源码：`libs/common/src/models/domain/domain-service.ts` + `libs/common/src/vault/models/view/login-uri.view.ts`

```ts
export const UriMatchStrategy = {
  Domain: 0, Host: 1, StartsWith: 2, Exact: 3, RegularExpression: 4, Never: 5,
};
// 默认 = Domain（即 eTLD+1）
```

`matchesUri(targetUri, equivalentDomains, defaultUriMatch?)`：
- **Domain**：用 `Utils.getDomain()` 抽 eTLD+1（如 `mail.google.com` → `google.com`），判断 cipher domain 是否在 `matchDomains`（等价域名 + targetDomain）集合中
- **Host**：比较 hostname（含端口）。`https://mail.google.com` 只匹配 `mail.google.com`
- **Exact**：完全相等
- **StartsWith**：前缀匹配
- **RegularExpression**：`new RegExp(uri, "i").test(target)`
- **Never**：永不匹配

**等价域名**（Equivalent Domains）：`equivalentDomains: string[][]`（如 `[[ "google.com", "youtube.com" ]]`）扩大匹配范围。组织策略 `PolicyType.UriMatchDefaults` 可强制默认匹配策略。

**octopus 借鉴要点**：5 种策略代码量极小可直接照搬；关键依赖是可靠的 `getDomain(url)`（eTLD+1 提取），需用 **公共后缀列表（Public Suffix List）**。

#### 密码生成器

- **长度**：5–128 字符，**默认 16**
- **字符集**：大小写 + 数字 + 特殊字符 `!@#$%^&*`，可独立开关，可避免歧义字符 `l1IO0`
- **16 字符默认熵 ≈ 95 bits**
- **Passphrase 模式**：基于 **EFF word list**（`libs/common/src/platform/misc/wordlist.ts` 的 `EFFLongWordList`），最少 3 词，可加数字、大写、分隔符

#### Bitwarden Send 加密设计

1. 每个 Send 生成 **128-bit 随机 secret key**
2. 用 **HKDF-SHA256** 从 secret key 派生 **512-bit 加密密钥**（256 enc + 256 MAC）
3. **AES-256-CBC + HMAC** 加密内容
4. 加密内容上传服务端，**128-bit key 放 URL `#` fragment**（fragment 不发服务器）
5. 接收方本地解密

特性：限时（1h~30d，默认 7d）+ 一次性访问 + 可选密码保护 + 接收方**无需 Bitwarden 账户**。

#### Secrets Manager（新产品）

与个人 vault **完全独立**的产品线，面向 DevOps。核心概念：Project（RBAC 分组）、Secret（key/value/note）、Machine Account（非人类身份）、Access Token（机器凭证）。**不能访问个人 vault**（产品边界明确）。

#### bitwarden/server 架构

- C# / .NET (ASP.NET Core) + T-SQL / SQL Server
- 多服务：**Api**（Vault API）/ **Identity**（OIDC 认证）/ **Notifications**（SignalR 推送）/ **Admin** / **Events** / **Billing**
- 数据访问 Repository 模式 + Dapper / EF Core

**与 vaultwarden 兼容性**：vaultwarden 只实现客户端 API 子集。最小兼容集：Identity (`/connect/token`)、Accounts、**Sync**、Ciphers CRUD、Folders/Collections/Sends、Notifications、2FA、Org Keys。

---

### 2.3 1Password 与 Bitwarden 产品功能对比

#### 1Password 的差异化护城河

**Secret Key 机制**（最重要的差异化）：
- 128-bit 强度，34 字符（形如 `A3-XXXXXX-XXXXXX-...`），账户级唯一
- 在创建账户的**那台设备本地生成，永不上传服务端**
- 加密流程：Secret Key + 账户密码 → HKDF 派生 salt → salt + 账户密码 → **PBKDF2-HMAC-SHA256 (650,000 次)** → 256-bit 主密钥 → AES-256-GCM 加密 vault
- **价值**：抵御服务端泄露（攻击者拿到密文也缺 Secret Key）+ 抵御弱主密码（Secret Key 把等效熵拉到 128-bit）+ 参与服务端认证（SRP 协议，服务端无法仅凭密码验证身份）

**Travel Mode**：过境时物理移除未标记 "safe for travel" 的 vault 本地副本，过境后恢复。

**Watchtower 功能**：
- 泄露密码：集成 HIBP，**k-anonymity 协议**（仅发 SHA-1 前 5 字符前缀）
- 弱密码 / 重复密码（本地）
- 不安全网站（HTTP、过时 TLS）
- 过期项目（信用卡、护照）
- 2FA 可用但未启用

#### Bitwarden 的差异化护城河

**免费无限设备**——最关键的产品竞争力。

**Bitwarden Send**：限时（1h-30d，默认 7d）+ 一次性访问 + 可选密码保护 + 接收方无需账户。

**Secrets Manager**：面向 DevOps 的机器密钥管理。

**Passwordless.dev**：给开发者的无密码认证 SDK。

#### 功能矩阵

| 功能 | 类别 | 1Password | Bitwarden |
|---|---|---|---|
| 密码生成器 | **必备** | ✅ | ✅ |
| 加密本地存储 | **必备** | ✅ AES-256-GCM | ✅ AES-256-CBC + HMAC |
| 主密码 + KDF | **必备** | ✅ PBKDF2 650k + Secret Key | ✅ PBKDF2 600k / Argon2id |
| 跨设备同步 | **必备** | ✅ | ✅（免费也支持） |
| 浏览器扩展 + autofill | **必备** | ✅ | ✅ |
| 桌面 Auto-Type | **增强** | ✅ | ✅（仅 Windows + Premium） |
| 全局 Quick Access 浮窗 | **增强** | ✅ | ⚠️ 部分 |
| TOTP | **增强** | ✅ | ✅（Premium） |
| 文件附件 | **增强** | ✅ | ✅（5GB） |
| 共享 Vault | **增强** | ✅ | ✅ |
| Passkey | **增强** | ✅ provider | ✅ |
| Watchtower / Health | **增强/差异化** | ✅ | ✅（Premium） |
| Travel Mode | **差异化** | ✅ 独有 | ❌ |
| Secret Key | **差异化** | ✅ 独有 | ❌ |
| Bitwarden Send | **差异化** | ❌ | ✅ 独有 |
| Secrets Manager | **差异化** | ✅ | ✅ |
| 完全免费无限设备 | **差异化** | ❌ | ✅ |

---

### 2.4 gopass 与 rbw

#### gopass（pass 系代表，去中心化）

**架构**：文件系统即数据库（每文件一 secret，目录即分组），文件格式 = 第 1 行密码 + 后续 YAML metadata（`username:`/`url:`/`notes:`/`totp:` 等）。

**加密**：GPG（gpg-agent）非对称加密，多 recipient 支持团队共享；或 age（现代替代，实验性）。

**核心功能**：
- **密码生成**：`cryptic`（随机字符）/ `memorable`（可发音）/ `xkcd`（字典词组）/ `external`
- **TOTP**：`gopass otp`，兼容 Google Authenticator 的 `otpauth://` 格式
- **同步**：`git push/pull`（密文进 git）
- **团队共享**：**mounts/substores**（子目录挂载到独立 git + 独立 recipient）+ **teams**（每团队独立 store）+ `rekey`（recipient 变更后重新加密）

**API/集成**：
- **JSON API（`gopass jsonapi`）**：浏览器扩展用，基于 **native messaging**（stdin/stdout 长度前缀 JSON）
- **无 HTTP server**
- 浏览器扩展 `gopassbridge`（Chrome/Firefox），按需启动 native host 进程

#### rbw（Bitwarden 系代表，daemon 模式）

**定位**：Bitwarden 服务端的**非官方 Rust CLI 客户端**，与 pass 生态**毫无血缘关系**。

**架构**：client + daemon（ssh-agent 心智模型）：
```
rbw (一次性 CLI 进程)
   ↕ Unix domain socket
rbw-agent (常驻后台，内存持已解锁 vault key)
   ↕ HTTPS
Bitwarden 服务器
```
- agent 按需自动 spawn
- 解锁时调 **pinentry** 弹主密码框
- agent 内存里保留**已解密 vault key**（不是主密码），所有命令瞬时响应
- `rbw stop-agent` / `rbw purge`

**加密**：完全复用 Bitwarden 协议（prelogin → KDF → stretched key → AES-CBC-256 + HMAC）。

**为什么用 Rust 重写**：常驻 agent（官方 CLI 无状态）、启动快（单二进制，无 Node 冷启动）、单文件部署、内存安全。

#### 两类设计哲学对比

| 维度 | pass 系（gopass） | Bitwarden 系（rbw） |
|---|---|---|
| 存储模型 | 文件系统即数据库 | 单一加密 vault（云同步） |
| 加密粒度 | 每个文件独立 | 整 vault 一把对称 key |
| 加密算法 | GPG / age（可插拔） | AES-CBC-256 + PBKDF2/Argon2id |
| 同步 | git（密文进 git） | 客户端-服务器 HTTPS |
| 多端 | 各设备 git clone + GPG key | 服务端推送 |
| 共享 | 多 recipient / mounts | 服务器侧组织/集合 |
| 离线 | 天然离线优先 | 需先同步缓存 |
| 解锁缓存 | gpg-agent 进程外 | 内置 daemon |
| 适合人群 | 程序员、Unix 用户 | 普通用户、跨平台 |

---

### 2.5 passbolt 与 nodewarden

#### passbolt（团队协作型）

**技术栈**：PHP 8 + CakePHP + MySQL/PostgreSQL + **OpenPGP（GnuPG）**。

**加密**（与 Bitwarden 根本不同）：
- 每个用户本地生成 **OpenPGP keypair**，**公钥上传服务器，私钥本地保存**
- 每个 resource 有独立对称密钥加密密码明文
- 分享：客户端用接收方公钥重新加密对称密钥，生成新的 `Secret` 记录（`PUT /share/resource/{id}`），服务器只是中转
- v5（2024-2026）新增 metadata 加密：name/username/URI/description 也加密

**API**：RESTful + JSON，路由表 `config/routes.php`。核心：`/auth/login`（GPG challenge-response）、`/users`、`/gpgkeys`（公钥目录）、`/resources`、`/secrets/resource/{id}`、`/share/resource/{acoForeignKey}`、`/permissions`、`/setup/start/{userId}/{tokenId}`。

**浏览器扩展**（`passbolt_browser_extension`）：
- 按 **W3C autofill 标准**匹配 `name`/`id`/`autocomplete` token（`username`、`current-password`）
- URL 匹配 + 等效域名
- 通过 iframe（Quickaccess）注入，**仅填充、不自动提交**
- 已知坑：React/受控组件需 dispatch `input`/`change` 事件

**与 Bitwarden 本质差异**：

| 维度 | passbolt | Bitwarden |
|---|---|---|
| 定位 | **团队协作优先** | 个人优先，组织附加 |
| 加密 | **OpenPGP 非对称**（每人 keypair） | 对称 AES + 公钥仅包装 user key |
| 分享模型 | O(接收者数) 次公钥加密 | 加入组织/集合时解开 collection key |
| 服务端依赖 | **依赖 gpg 二进制** | 纯代码 |
| 元数据保护 | v5 才加密 | 原生支持 |

#### nodewarden（⚠️ 修正：不是 Node.js，是 Cloudflare Workers）

**关键纠正**：`package.json` 明确写「Minimal Bitwarden-compatible server running on Cloudflare Workers」。`wrangler.toml` 绑定 **D1（托管 SQLite）/ R2 或 KV（对象存储）/ Durable Objects（实时推送）**。仓库名虽叫 nodewarden，**与 Node.js 运行时无关**。

**技术栈**：
- 运行时：Cloudflare Workers（V8 isolate）
- 语言：TypeScript（ESM）
- 数据库：**D1**（托管 SQLite），schema 见 `migrations/0001_init.sql`
- 对象存储：**R2**（默认，需绑卡）或 **KV**（免绑卡，单文件 25MiB）
- 实时同步：**Durable Objects**（`NotificationsHub`、`BackupTransferRunner`）—— 比 vaultwarden 的 WebSocket 更现代
- 认证：JWT + WebAuthn/Passkey + TOTP + YubiKey OTP
- 加密：纯 TS 实现 Bitwarden 协议，用 `@noble/hashes`
- 前端：**原创 Web Vault**（Preact + TanStack Query + Tailwind + wouter），打包为 **PWA，可离线使用**

**关键 schema 字段**（`migrations/0001_init.sql`）：
- `users`：`key`（加密的对称主密钥）/ `private_key` / `public_key`
- `devices`：`encrypted_user_key` / `encrypted_public_key` / `encrypted_private_key` + `push_uuid/push_token` —— **多设备信任机制**
- `webauthn_credentials`：`encrypted_user_key` / `supports_prf` —— **Passkey + PRF** 无密码解锁
- `auth_requests`：跨设备登录审批
- `ciphers`：标准 Bitwarden 字段（type/data/key/reprompt/archived_at/deleted_at）

**fill-assist 端点**（`src/handlers/fill-assist.ts`）：实现 Bitwarden 2024-2025 推出的 **fill-assist 协议**（`POST /fill-assist`、JSON Schema + manifest + Digital Asset Link 检查）。当前返回空表单清单（合规占位实现），是 Bitwarden 对抗 iOS/Android 系统 autofill 框架限制的新协议。

**与 vaultwarden 对比**：

| 维度 | vaultwarden | nodewarden |
|---|---|---|
| 语言 | Rust 单二进制 | TypeScript Cloudflare Worker |
| 部署 | Docker/二进制 | **Serverless，免费额度即可跑** |
| 数据库 | MariaDB/PostgreSQL/SQLite | **D1（托管，无运维）** |
| Web Vault | 复用 Bitwarden 官方 | **原创 Preact PWA，可离线** |
| 实时同步 | WebSocket 自建 | **Durable Objects 托管** |
| 附件存储 | 本地/S3 | R2 或 KV |
| 组织/集合 | 支持 | **未实现**（定位单/多用户） |
| 多用户 | 注册开关 | 邀请码注册 |

---

## 3. 关键技术点深挖（octopus 实现时会用到）

### 3.1 加密方案对比（给 octopus 选型）

| 方案 | 算法 | 密钥层级 | 优势 | 劣势 |
|---|---|---|---|---|
| **A. 借鉴 Bitwarden 协议（自实现）** | AES-256-CBC + HMAC + PBKDF2/Argon2id | master key → user key → cipher key | 标准化、易理解、可参考成熟实现 | 需写不少 crypto 代码 |
| **B. Tauri Stronghold** | IOTA Stronghold | 主密码派生 | 现成 Tauri 插件 | 锁定 Stronghold 体系，迁移困难 |
| **C. OS Keychain** | 系统管理 | OS 管理 | 用户无感、最省事 | 信任完全交给 OS，跨平台不一致 |
| **D. 1Password Secret Key 模式** | AES-256-GCM + PBKDF2 + Secret Key | Secret Key + 主密码双因子 | 防服务端泄露 | octopus 单机无服务端，价值有限 |
| **E. passbolt PGP 模型** | OpenPGP 非对称 | 每用户 keypair | 团队协作强 | octopus 单用户，过度设计 |

**推荐：方案 A（借鉴 Bitwarden 协议自实现）**，理由：
1. 与 octopus 现有 Rust 栈契合（`aes`、`hkdf`、`sha2`、`argon2` crate 成熟）
2. 模型清晰，可在 desktop crate 内自包含
3. 不依赖外部服务，单机可用
4. 未来若要兼容 Bitwarden 生态，迁移成本最低

### 3.2 自动填充路径对比（⭐ 关键决策点）

| 路径 | 实现复杂度 | 跨平台性 | 用户体验 | 与 octopus 现状契合度 |
|---|---|---|---|---|
| **A. 浏览器扩展** | 高（独立工程，MV3、content script） | ✅ Chrome/Firefox/Safari/Edge | 最好（自动检测表单） | 低（需另起项目） |
| **B. 桌面 Auto-Type** | 中（macOS CGEvent + Windows SendInput + Linux ydotool） | ✅ | 中（需先聚焦浏览器，模拟键盘） | **高**（octopus 已有全局热键 + objc2 经验） |
| **C. 内嵌 Tauri WebView** | 中（改 actionbar 的 url action） | ✅ | 中（应用内体验） | 中（需重构打开网站方式） |
| **D. 仅复制到剪贴板** | 低 | ✅ | 低（手动粘贴） | 高 |

**推荐：B + D 组合 MVP，A 作为后续工程**。理由：
- B（Auto-Type）是 Bitwarden/1Password 桌面端的标准做法，octopus 已有全部基础设施（全局热键、objc2、窗口管理）
- D（剪贴板）作为 fallback，简单可靠
- A（浏览器扩展）工程量大，建议桌面稳定后另起

**关键技术挑战**（B 路径）：
- macOS：CGEvent 键盘模拟（octopus 已用过），但需要处理输入框聚焦、密码字段、HTTPS 表单
- Windows：SendInput API
- Linux：xdotool（X11）/ ydotool（Wayland）
- 当前 URL 获取：octopus 已有 `app_context/mod.rs:160-170` 识别 Browser kind，但**未抓地址栏 URL**，需新增（osascript 读 Safari/Chrome active tab，或 Accessibility API）

### 3.3 URL/域名匹配策略（直接抄 Bitwarden）

5 种策略 + eTLD+1 提取 + 等价域名表。Rust 侧依赖：**`psl` 或 `publicsuffix` crate**（公共后缀列表）。

```rust
enum UriMatchStrategy {
    Domain,     // eTLD+1 (默认)
    Host,       // hostname[:port]
    Exact,
    StartsWith,
    RegularExpression,
    Never,
}

fn match_uri(cipher_uri: &str, target_url: &str, equivalent: &[Vec<String>]) -> bool { ... }
```

### 3.4 TOTP（RFC 6238，最简实现）

```rust
// crate: totp-rs 或自实现
// HMAC-SHA1, 30s, 6 位, ±1 步漂移, last_used 防重放
fn generate_totp(secret: &[u8], time: u64) -> String { ... }
```

### 3.5 密码生成器

- 随机字符：长度可调（默认 16）、字符集可配、可避免歧义字符
- Passphrase：EFF word list（最低 3 词），可加数字、大写、分隔符
- Rust crate：`rand`（CSPRNG）+ 内嵌 EFF 词表

---

## 4. 给 octopus 的路线建议

### 4.1 推荐架构

```
crates/
├── vault/                       # 新增：密码 vault 核心库（无项目内依赖，只依赖 infra）
│   ├── crypto/                  # Argon2id + AES-GCM + HKDF + TOTP
│   ├── storage/                 # SQLite 表 + 加密读写
│   ├── generator/               # 密码生成器
│   ├── matcher/                 # URL 匹配 + eTLD+1
│   └── unlock/                  # 解锁态管理（master key 内存缓存）
└── desktop/
    ├── src/
    │   ├── vault_commands.rs    # 新增：Tauri 命令
    │   └── autotype/            # 新增：跨平台键盘模拟
    │       ├── macos.rs         # CGEvent
    │       ├── windows.rs       # SendInput
    │       └── linux.rs         # xdotool/ydotool
    └── frontend/
        └── src/pages/
            ├── VaultPanel/      # vault 设置页（CRUD cipher）
            ├── PasswordGenerator/  # 密码生成器浮窗
            └── VaultUnlock/     # 解锁弹窗（输入主密码）
```

### 4.2 数据模型（新增 SQLite 表）

借鉴 vaultwarden/nodewarden，简化后：

```sql
-- vault 元数据（KDF 参数、加密的 user key、版本）
CREATE TABLE IF NOT EXISTS vault_meta (
    id INTEGER PRIMARY KEY,        -- 单行
    kdf_type INTEGER NOT NULL,     -- 0=PBKDF2 1=Argon2id
    kdf_iter INTEGER NOT NULL,
    kdf_memory INTEGER,            -- Argon2id
    kdf_parallelism INTEGER,       -- Argon2id
    email_salt TEXT NOT NULL,      -- KDF salt（替代 Bitwarden 的 email）
    protected_user_key TEXT NOT NULL,  -- 加密的 user key（Base64）
    public_key TEXT,               -- RSA 公钥（可选，未来分享用）
    protected_private_key TEXT,    -- 加密的 RSA 私钥
    security_stamp TEXT NOT NULL,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

-- 密码条目（核心）
CREATE TABLE IF NOT EXISTS vault_ciphers (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    atype INTEGER NOT NULL,        -- 1=Login 2=SecureNote 3=Card 4=Identity
    name TEXT NOT NULL,            -- 密文（Base64）
    notes TEXT,                    -- 密文
    fields TEXT,                   -- JSON，自定义字段（均加密）
    data TEXT NOT NULL,            -- JSON: {uris:[{uri, match}], username, password, totp}（均加密）
    password_history TEXT,         -- JSON 数组（加密）
    cipher_key TEXT,               -- 可选，cipher 级密钥
    favorite INTEGER NOT NULL DEFAULT 0,
    reprompt INTEGER NOT NULL DEFAULT 0,  -- 0=None 1=Password
    deleted_at TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

-- 文件夹（简化，可暂不做）
CREATE TABLE IF NOT EXISTS vault_folders (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    name TEXT NOT NULL,            -- 密文
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);
```

### 4.3 MVP 功能集（P0）

| # | 功能 | 实现要点 |
|---|---|---|
| 1 | **首次初始化 vault** | 用户设主密码 → Argon2id 派生 master key → 生成 user key → 用 master key 加密 user key 落库 |
| 2 | **解锁 / 锁定** | 输入主密码 → 派生 master key → 解 user key → 内存缓存（Tauri 进程内）；锁定时 zeroize |
| 3 | **CRUD cipher** | 加密所有敏感字段后落库；解密在内存进行 |
| 4 | **密码生成器** | 浮窗 UI（Cmd+Shift+G 等热键）+ 长度/字符集/passphrase 模式 |
| 5 | **URL 匹配 + Auto-Type** | 全局热键（默认 Cmd+Shift+L）→ 检测当前浏览器 URL（osascript / Accessibility）→ 匹配 cipher → 模拟键盘 `username\tTabpassword\tEnter` |
| 6 | **Quick Access 浮窗** | 沿用 actionbar 浮窗机制，加 `vault` tab，搜索 → 复制/填充 |

### 4.4 P1（增强）

- TOTP 生成与填充联动（填充密码后自动复制 TOTP）
- 多类型条目（信用卡、安全笔记）
- 弱密码 / 重复密码检测（本地）
- 自定义字段
- 密码历史

### 4.5 P2（差异化）

- Bitwarden Send 式临时分享（**本地版**：加密片段 + 访问密码，导出文件/二维码）
- HIBP k-anonymity 查询（仅发 SHA-1 前 5 字符）
- 浏览器扩展（独立工程）

### 4.6 P3（不建议 / 与 octopus 定位不符）

- **Secret Key**（1P 杀手锏）：octopus 单机无服务端，没有这个攻击面
- **云同步 / 多设备**：涉及账号体系、服务端、合规，偏离核心
- **Travel Mode**：单机工具用"隐藏 vault"即可
- **Secrets Manager / Passwordless.dev**：面向 DevOps 和开发者市场，与 ASR 无关
- **passbolt 路线**：PGP + 团队协作过度设计
- **gopass 文件即数据库**：推翻现有 SQLite 体系

### 4.7 顺手的安全改进（无论选哪条路线）

1. **`models.secret_key` 加密存储**：当前明文 API Key 也要纳入加密范围（schema 迁移：新增 `secret_key_nonce`/`secret_key_tag` 或独立 `encrypted_secrets` 表）
2. **`app_config` 表的 token/cookie** 同样纳入
3. **内存中的明文 key 用 `zeroize` crate 清零**
4. **主密码派生用 Argon2id**（不要裸 PBKDF2）

---

## 5. 关键参考链接

### 官方文档
- [Bitwarden Security Whitepaper](https://bitwarden.com/help/bitwarden-security-white-paper/)
- [Bitwarden KDF Algorithms](https://bitwarden.com/help/kdf-algorithms/)
- [Bitwarden URI Match Detection](https://bitwarden.com/help/uri-match-detection/)
- [Bitwarden Send Encryption](https://bitwarden.com/help/send-encryption/)
- [1Password Security Design – Secret Key](https://agilebits.github.io/security-design/deepKeys.html)
- [1Password Watchtower Privacy](https://support.1password.com/watchtower-privacy/)
- [1Password Auto-Type](https://support.1password.com/windows-auto-type/)

### 开发者文档
- [Bitwarden Web Clients Architecture](https://contributing.bitwarden.com/architecture/clients/)
- [Bitwarden Browser Autofill Deep Dive](https://contributing.bitwarden.com/architecture/deep-dives/autofill/)
- [Bitwarden Cryptography Guide](https://contributing.bitwarden.com/architecture/cryptography/crypto-guide)
- [Bitwarden Clients API (Mintlify)](https://bitwarden-clients.mintlify.app/)
- [Passbolt OpenPGP docs](https://www.passbolt.com/docs/hosting/openpgp/)
- [Passbolt Security White Paper v5.10](https://www.passbolt.com/docs/files/security_white_paper_-_passbolt_pro_edition_v5.10_-_(march_2026_-_rev10).pdf)

### 关键源码路径
**vaultwarden**（`/Users/wudarui/workspace/agent/vaultwarden`）：
- 数据模型：`src/db/models/{cipher,user,send,two_factor}.rs`
- 服务端加密（仅 PBKDF2 验证）：`src/crypto.rs`
- API 路由：`src/api/{identity,core/ciphers,core/sends}.rs`
- TOTP：`src/api/core/two_factor/authenticator.rs:115-181`

**bitwarden/clients**：
- 浏览器 autofill：`apps/browser/src/autofill/services/autofill.service.ts`
- 桌面 auto-type（NAPI）：`apps/desktop/src/autofill/main/main-desktop-autotype.service.ts`
- 桌面 cipher 匹配：`apps/desktop/src/autofill/services/desktop-autotype.service.ts`
- KeyService：`libs/key-management/src/key.service.ts`
- URL 匹配：`libs/common/src/vault/models/view/login-uri.view.ts`
- UriMatchStrategy：`libs/common/src/models/domain/domain-service.ts`
- 底层 Rust 加密：[`bitwarden/sdk-internal`](https://github.com/bitwarden/sdk-internal)

**bitwarden/server**：C#/.NET + SQL Server，多服务（Api/Identity/Notifications/Admin/Events）。

**passbolt**：
- API：`passbolt_api`（PHP CakePHP），`config/routes.php` + `src/Model/Table/`
- 扩展：`passbolt_browser_extension`，`src/all/contentScripts/`

**nodewarden**：
- schema：`migrations/0001_init.sql`
- 路由：`src/router*.ts`
- fill-assist：`src/handlers/fill-assist.ts`

### 第三方深度分析
- [How Bitwarden Encrypts and Decrypts Secrets (Miguel Grinberg)](https://blog.miguelgrinberg.com/post/how-bitwarden-encrypts-and-decrypts-secrets)
- [Bitwarden design flaw: server-side iterations (Palant.info)](https://palant.info/2023/01/23/bitwarden-design-flaw-server-side-iterations/)
- [跨平台 bitwarden-autotype Rust 参考实现](https://github.com/mcofficer/bitwarden-autotype)

---

## 6. octopus 现有代码复用点速查

| 需求 | 现有基础 | 缺口 |
|---|---|---|
| 触发方式（热键唤起浮窗） | `action_bar_window.rs` + 全局热键注册完整 | 直接复用，加 `vault` tab 或 `action_type` |
| 网站数据存储 | `action_bar_items.action_type='url'` 的 `action_data` | **新建专用 `vault_ciphers` 表**（域名/URL/用户名/密码/notes） |
| 打开网站登录页 | `execute_action_bar` url 分支 / `open_url` | 已就绪 |
| 获取当前浏览器 URL | `app_context/mod.rs:160-170` 仅识别 Browser kind，**未抓地址栏 URL** | **需新增**（osascript 读 Safari/Chrome active tab，或 AX） |
| 密钥加密存储 | **无** | **必须引入**（`aes`/`argon2`/`hkdf`/`zeroize` crate） |
| 凭证 UI | 设置页 `ActionBarPanel` 的 EditForm 模式可仿照 | 新建 `VaultPanel` |
| Tauri capability | `opener:default` 已开 | 加新命令需注册到 `main.rs:226 invoke_handler` |
| 平台原生 API | `objc2::msg_send!` 已大量用（NSPasteboard/NSWindow） | Keychain / CGEvent 直接照搬调用模式 |
| schema 升级流程 | 改 db.sql + 升 `init_schema` 的 `user_version`（当前 v37） | v38 加 vault 表 |

---

## 7. 下一步行动建议

按 superpowers 工作流推进：

1. **brainstorming（必须）**：聚焦 4 个决策点（D1-D4），特别是 D1（填充路径）会决定整个工程形态。建议至少讨论：
   - 用户最常使用场景：是"打开网站 → 自动登录"，还是"全局浮窗 → 搜索密码 → 复制"？
   - 是否要兼容 Bitwarden 协议让用户复用已有 vault？
   - 是否需要云同步？
   - macOS 上的浏览器 URL 获取方案：osascript（需用户授权） vs Accessibility API（需权限） vs 手动选择 cipher？

2. **写 spec**：`docs/superpowers/specs/2026-07-XX-password-vault-design.md`
   - 数据模型、加密流程、URL 匹配策略、Auto-Type 实现细节、解锁态缓存机制
   - 不变量（master key 永不出进程、cipher 字段必加密、Argon2id 最低参数）
   - 降级路径（主密码忘记 = vault 不可恢复，要有清晰的 emergency kit 概念）

3. **写 plan**：`docs/superpowers/plans/2026-07-XX-password-vault.md`
   - Task 1：新增 `crates/vault/` crate，crypto + storage + generator + matcher + unlock 模块
   - Task 2：schema v38，新增 `vault_meta` / `vault_ciphers` 表
   - Task 3：Tauri 命令（首次初始化、解锁、CRUD cipher、生成器）
   - Task 4：前端 VaultPanel + 解锁弹窗 + 生成器浮窗
   - Task 5：URL 匹配 + Auto-Type（macOS 优先）
   - Task 6：Quick Access 浮窗集成到 actionbar
   - Task 7：把 `models.secret_key` 纳入加密范围（顺手改进）

4. **实现 + review plan**：按 AGENTS.md 规定的验证纪律（编译 + grep 影响面 + 端到端链路 + 测试）。

---

**调研完成**。本报告作为后续 brainstorming + spec + plan 的事实基础。
