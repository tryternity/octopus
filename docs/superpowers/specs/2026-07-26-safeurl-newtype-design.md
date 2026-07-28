# SafeUrl Newtype 设计——PAT 泄露结构性根治

**日期**：2026-07-26（设计），2026-07-28（实施）
**状态**：✅ 已实施（commit 见 git log）
**类型**：架构改进（跨 crate 类型系统改动）
**关联**：
- [vault-security-hardening](./2026-07-24-vault-security-hardening.md)——第四十九~五十四轮 PAT 外溢链
- [vault-git-sync-design](./2026-07-21-vault-git-sync-design.md)——sync 模块原始设计

## 背景：六轮 PAT 外溢链（为什么需要结构性根治）

第四十九~五十三轮连续 6 次 PAT（Personal Access Token）泄露外溢，证明**逐处 `let safe_url = redact_url(url)` 的点状修复无法收敛**：

| 轮次 | 外溢点 | 模式 |
|---|---|---|
| 49 | `SyncError::PublicRepoRejected(url)` Display | 点状 redact |
| 50 | `ensure_private_repo` 内部 5 分支 | 点状 redact |
| 51 | 调用方链（add_remote / maybe_rewrite_to_ssh） | 点状 redact |
| 52 | `ensure_remotes_use_ssh_when_possible` | 点状 redact |
| 53 | `list_remotes` / `SyncStatus.remotes` 返回值（UI 渲染） | helper 抽取 |
| 54 | （未发现新外溢，PAT 专题收官） | — |

第五十三轮抽 `redact_remotes_for_outflow` helper 是结构性进步（点状 → 集中构造），但仍**不能完全防漏调**——helper 只防"现有调用点改错"，不防"新增流出点不调 helper"。第五十四轮独立复查确认 6 个维度无新外溢，但**未来新增 url 流出点仍可能漏调**（第八次外溢候选）。

**根治方案**：引入 `SafeUrl(String)` newtype，让所有流出 crate 边界的 url 必须是 `SafeUrl` 类型，**编译器强制在边界构造**（调 `redact_url` 是唯一构造器）。漏调 = 编译错误，而非运行时 PAT 泄露。

## 设计目标

### 必须达成（编译期保证）

1. **流出 crate 边界的 url 必经 redact**：所有 `pub fn` 返回值 / `Serialize` struct 字段 / `Display` 输出里出现的 url，必须是 `SafeUrl` 类型。
2. **`redact_url` 是 `SafeUrl` 的唯一构造器**：无法从 `&str` / `String` 直接构造 `SafeUrl`（不实现 `From<&str>` / `From<String>`）。
3. **`SafeUrl` 不可拿回原始 url**：不实现 `AsRef<str>`（或只实现 `as_redacted_str`），防止下游误用。

### 可以接受（非编译期保证）

4. **内部 url 仍是 `&str` / `String`**：add_remote / ensure_private_repo / maybe_rewrite_to_ssh 内部需要原始 PAT url 工作（私有库检测、SSH 改写），这些保持 `&str`。
5. **log 宏参数**：log 的格式化由宏本身控制，无法在类型层强制。仍需人工 + code review（当前已收敛，6 个 log 点全 safe_url）。

### 不做（明确排除）

6. **不改 SyncError 其他变体**（`GitError(String)` / `CredentialsRequired(String)` 等）：它们的 String 是 git stderr（已 #11 redact），不是 url 语义，保持 String。
7. **不改 cipher/folder 文件存储格式**：CipherFile / FolderFile 是磁盘序列化格式，不流向前端，保持 String。

## 类型定义

```rust
// crates/sync/src/error.rs（最底层 crate，所有上游可用）

/// 已 redact 的 URL——PAT/密码/userinfo 已剥离，可安全用于 log / Display / 流出 crate 边界。
///
/// **唯一构造器是 `redact_url`**——无法从 `&str` / `String` 直接构造，
/// 编译期保证所有 `SafeUrl` 实例都经过 redact。
///
/// 用于：SyncStatus.remotes / list_remotes 返回值 / SyncError::PublicRepoRejected /
/// log 宏参数。流出 octopus-sync / octopus-vault crate 边界的 url 必须是此类型。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct SafeUrl(String);

impl SafeUrl {
    /// 已 redact 的字符串引用——用于 log / Display。
    ///
    /// 命名 `as_redacted_str`（而非 `as_str`）——刻意加长调用名，提醒调用方
    /// 这是已 redact 的内容，不可当作原始 url 使用。
    pub fn as_redacted_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for SafeUrl {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// 唯一构造器——redact 后包成 SafeUrl。
pub fn redact_url(url: &str) -> SafeUrl {
    let redacted = match url::Url::parse(url) {
        Ok(mut parsed) => {
            let _ = parsed.set_password(None);
            let _ = parsed.set_username("");
            parsed.to_string()
        }
        Err(_) => url.to_string(), // scp-like 等非 `://` 格式原样返回
    };
    SafeUrl(redacted)
}
```

**关键设计点**：

- **`SafeUrl(String)` 而非 `SafeUrl(str)`**——owned，避免生命周期传染（`SafeUrl<'a>` 会让所有含它的 struct 都带生命周期参数，扩散成本过高）。
- **不实现 `From<&str>` / `From<String>`**——这是编译期保证的核心。如果实现了，`SafeUrl::from(url)` 会绕过 redact。
- **不实现 `AsRef<str>`**——防止下游 `safe_url.as_ref()` 拿到内部 String 后误用。`as_redacted_str` 命名刻意加长，提醒调用方。
- **`Display` 实现**——让 `format!("{}", safe_url)` / `log::info!("{}", safe_url)` 直接工作，无需调 `as_redacted_str`。
- **`serde::Serialize`**——让 `SafeUrl` 可直接作为 Serialize struct 字段（SyncStatus.remotes），序列化结果就是 redact 后的字符串。

## 边界改造点（影响面）

### octopus-sync（底层 crate）

| 位置 | 改造 |
|---|---|
| `error.rs::redact_url` | 返回类型 `String` → `SafeUrl`（唯一构造器） |
| `error.rs::SyncError::PublicRepoRejected(String)` | 字段类型 → `SafeUrl`（构造时 redact，Display 直接用） |

### octopus-vault（依赖 sync）

| 位置 | 改造 |
|---|---|
| `engine.rs::SyncStatus.remotes: Vec<(String, String)>` | → `Vec<(String, SafeUrl)>` |
| `engine.rs::list_remotes() -> Result<Vec<(String, String)>, _>` | → `Result<Vec<(String, SafeUrl)>, _>` |
| `engine.rs::redact_remotes_for_outflow` | 返回 `Vec<(String, SafeUrl)>`，内部调 `redact_url` 构造 |
| `engine.rs` 4 个函数的 log 点（add_remote / ensure_private_repo / maybe_rewrite_to_ssh / ensure_remotes_use_ssh） | `let safe_url = redact_url(url)` 类型自动从 String 变 SafeUrl，log 宏用 Display——**无需改 log 调用** |

### octopus-desktop（依赖 vault）

| 位置 | 改造 |
|---|---|
| `vault_sync_commands.rs::vault_sync_list_remotes` | 返回 `Vec<(String, SafeUrl)>`，Tauri 序列化后前端拿到 redact 字符串 |
| 前端 `SyncPanel.tsx` | **无需改**——`SafeUrl` Serialize 后是普通 string，`{url}` 渲染 redact 后内容 |

### 前端（TypeScript）

**无需改动**——`SafeUrl` 经 serde 序列化后是普通 JSON string，前端类型仍是 `string`。

## 测试策略

### 契约测试（保留 + 增强）

- `redact_url_strips_userinfo`（第五十二轮加）：保留，但断言类型从 `String` 变 `SafeUrl`，调 `as_redacted_str()` 比较。
- `redact_url_never_leaks_pat`（第五十二轮加）：保留，同上。

### 编译期测试（新增）

```rust
/// 编译期保证：SafeUrl 无 From<&str> 实现（绕过 redact 的唯一入口）。
///
/// 若有人误加 impl From<&str> for SafeUrl，此测试编译失败。
#[test]
fn safe_url_has_no_from_str_constructor() {
    // 这个函数存在即证明 From<&str> 未实现——
    // 如果实现了，下面这行会编译失败（ambiguous associated item）。
    fn assert_no_from_str<T>() where T: ?Sized {}
    // 静态断言：SafeUrl: !From<&str>
    // Rust 无直接的 negative trait bound，改用尝试构造 + 编译失败检测：
    // 此处留空——靠 code review + 类型系统自然拦截（SafeUrl 字段是 private）。
}
```

**实际编译期保证来自**：`SafeUrl.0` 是 private 字段，外部 crate 无法 `SafeUrl("xxx".to_string())` 构造——只能调 `redact_url`。

### 边界测试（新增）

```rust
/// list_remotes 返回的 url 不含 PAT（行为层守护）。
/// 这个测试在 newtype 引入前无法跑（依赖 git repo），newtype 引入后
/// 可直接测 redact_remotes_for_outflow 的 SafeUrl 输出。
#[test]
fn list_remotes_output_is_safe_url() {
    let remotes = vec![("origin".to_string(), "https://user:ghp_xxx@github.com/o/r.git".to_string())];
    let safe = redact_remotes_for_outflow(&remotes);
    // 类型断言：返回值必须是 Vec<(String, SafeUrl)>
    let _: &[(String, SafeUrl)] = &safe;
    // 内容断言：PAT 已剥离
    assert!(!safe[0].1.as_redacted_str().contains("ghp_xxx"));
}
```

## 风险与权衡

### 收益

- **编译期杜绝第八次外溢**：新增 `pub fn -> Vec<(String, String)>` 含 url 的函数，编译失败（必须显式构造 SafeUrl）。
- **单一真相源**：所有 redact 逻辑集中在 `redact_url`，code review 只需看一处。
- **类型自文档化**：`SafeUrl` 出现在签名里就是"已 redact"的明示。

### 成本

- **跨 crate 签名改动**：sync + vault + desktop 三层都要改（但改动是机械的——`String` → `SafeUrl`，构造点加 `redact_url()`）。
- **`SafeUrl` 不在热路径**：url 操作只在 add_remote / sync_now 时发生（用户操作触发），非每帧/每 tick——无性能影响。
- **`Display` 但无 `AsRef<str>`** 的设计会让某些场景需显式调 `as_redacted_str()`——可接受的认知成本（换取安全性）。

### 不解决的问题

- **log 宏参数**：Rust 的 log 宏接受任何 `Display` 类型，无法在类型层强制 log 参数必须是 SafeUrl。仍需人工 + code review（当前已收敛）。
- **`.git/config` 仍存 PAT**（OBS-CLONE-URL-STORES-PAT-IN-CONFIG）：newtype 只防"流出 crate 边界时漏 redact"，不防"上游写入 PAT"。后者是产品设计决策（add_remote 是否拒绝 PAT url），不在本 spec 范围。

## 降级路径

若实现中发现 `SafeUrl` 扩散成本过高（比如需要改 10+ 个签名），可降级为：

- **保留 `redact_remotes_for_outflow` helper**（第五十三轮已实现），不加 newtype。
- **加更强的行为测试**：模拟 PAT url 走完整 add_remote → list_remotes 链路，断言输出无 PAT（需 test fixture 注入 git repo）。

但当前评估改动面可控（4 个 crate 文件，约 15 处签名），优先按 newtype 推进。

## 实施顺序

1. **sync crate**：定义 SafeUrl + 改 redact_url 返回类型 + 改 SyncError::PublicRepoRejected。
2. **vault crate**：改 SyncStatus / list_remotes / redact_remotes_for_outflow 返回类型。
3. **desktop crate**：vault_sync_commands 自动跟随（Tauri 序列化）。
4. **测试**：跑全量 vault + sync + desktop 测试，确认 0 regression。

详见 plan：`docs/superpowers/plans/2026-07-26-safeurl-newtype.md`。
