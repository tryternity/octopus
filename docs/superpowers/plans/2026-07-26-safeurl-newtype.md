# SafeUrl Newtype 实施计划

**日期**：2026-07-26
**关联 spec**：[safeurl-newtype-design](../specs/2026-07-26-safeurl-newtype-design.md)
**状态**：待实施

## 任务分解

### Task 1: sync crate 定义 SafeUrl + 改 redact_url

**文件**：`crates/sync/src/error.rs`

**变更点**：
1. 新增 `SafeUrl(String)` newtype 定义（private 字段 + Display + Serialize + as_redacted_str）。
2. 改 `redact_url(url: &str) -> String` → `redact_url(url: &str) -> SafeUrl`（唯一构造器）。
3. 改 `SyncError::PublicRepoRejected(String)` → `SyncError::PublicRepoRejected(SafeUrl)`，构造时调 redact_url。
4. 更新 Display impl——`SafeUrl` 已实现 Display，直接 `write!(f, "...", safe_url)`。
5. 更新现有契约测试（`redact_url_strips_userinfo` / `redact_url_never_leaks_pat`）——断言从 `String` 改为 `SafeUrl`，调 `as_redacted_str()` 比较。
6. 更新 `display_does_not_leak_pat_from_stderr` 测试——`PublicRepoRejected` 构造改为传 `SafeUrl`（或传原始 url 由构造器内部 redact——需新增 `SyncError::PublicRepoRejected::new(url: &str)` 构造方法）。

**验证命令**：
```bash
cargo build -p octopus-sync 2>&1 | tail -10  # 0 error 0 warning
cargo test -p octopus-sync 2>&1 | tail -10   # 105 pass（+2 新契约测试）
```

**注意事项**：
- `SyncError::PublicRepoRejected` 的构造点在 engine.rs（vault crate），不是 sync crate——构造时需要调 redact_url。考虑加 convenience 构造方法 `SyncError::public_repo_rejected(url: &str) -> Self`，内部调 redact_url，避免调用方忘记。

### Task 2: vault crate 改返回类型 + helper

**文件**：`crates/vault/src/sync/engine.rs`

**变更点**：
1. `SyncStatus.remotes: Vec<(String, String)>` → `Vec<(String, SafeUrl)>`。
2. `list_remotes() -> Result<Vec<(String, String)>, _>` → `Result<Vec<(String, SafeUrl)>, _>`。
3. `redact_remotes_for_outflow` 返回 `Vec<(String, SafeUrl)>`，内部调 `redact_url`（已是唯一构造器）。
4. 4 个函数的 log 点（add_remote / ensure_private_repo / maybe_rewrite_to_ssh / ensure_remotes_use_ssh_when_possible）——`let safe_url = redact_url(url)` 自动变 `SafeUrl` 类型，log 宏用 Display，**无需改 log 调用**。
5. `SyncError::PublicRepoRejected(url.to_string())` 改为 `SyncError::public_repo_rejected(url)`（convenience 构造）。
6. 更新 `redact_remotes_for_outflow_strips_pat` 测试——断言改用 `as_redacted_str()`。

**验证命令**：
```bash
cargo build -p octopus-vault 2>&1 | tail -10  # 0 error 0 warning
cargo test -p octopus-vault --lib 2>&1 | tail -5  # 251 pass
```

### Task 3: desktop crate 跟随

**文件**：`crates/desktop/src/vault_sync_commands.rs`

**变更点**：
1. `vault_sync_list_remotes() -> Result<Vec<(String, String)>, String>` → `Result<Vec<(String, SafeUrl)>, String>`（或保留 `(String, String)` 由 desktop 层调 redact——但这样 newtype 失去意义，应让 SafeUrl 流到 Tauri 序列化层）。
2. 确认 Tauri 序列化 `SafeUrl` 后是 redact 字符串（serde::Serialize impl 已在 newtype 定义）。

**验证命令**：
```bash
cargo build -p octopus-desktop 2>&1 | tail -10  # 0 error 0 warning
```

**注意事项**：
- Tauri command 返回值序列化走 serde——`SafeUrl` derive Serialize 后，前端拿到的就是 redact 字符串。
- 前端 `SyncPanel.tsx` 类型 `[string, string][]` 仍兼容（SafeUrl 序列化后是 string）。

### Task 4: 全量回归测试

**验证命令**：
```bash
cargo test -p octopus-sync 2>&1 | tail -5      # 105+ pass
cargo test -p octopus-vault 2>&1 | tail -10    # 251+ pass + 集成 unlock.rs
cargo build -p octopus-desktop 2>&1 | tail -5  # 0 warning
cd crates/desktop/frontend && npx tsc --noEmit  # 0 error
```

### Task 5: 文档同步

**文件**：
- `docs/superpowers/specs/2026-07-24-vault-security-hardening.md`——在 PAT 外溢章节末尾追加"第五十五轮：SafeUrl newtype 引入，编译期根治"。
- `docs/architecture.md`——在 vault sync 模块描述里加 SafeUrl 类型说明。

## 验收清单

- [ ] Task 1: sync crate SafeUrl 定义 + redact_url 改返回类型
- [ ] Task 2: vault crate SyncStatus/list_remotes/helper 改返回类型
- [ ] Task 3: desktop crate 跟随
- [ ] Task 4: 全量测试通过（sync 105+ / vault 251+ / desktop build 0 warning / tsc 0 error）
- [ ] Task 5: 文档同步

## 回滚预案

若实现中发现 `SafeUrl` 扩散成本过高（如需改 10+ 个非预期签名）：
1. 保留 sync crate 的 SafeUrl 定义（不删除——已投入）。
2. vault/desktop 改回用 `String` + `redact_remotes_for_outflow` helper（第五十三轮已实现的状态）。
3. 在 spec 的"降级路径"章节记录回滚决策。

## 风险点

- **SyncError::PublicRepoRejected 构造点**：如果调用方传的是已 redact 的 url，会 double-redact（无害但浪费）。需在 convenience 构造方法里明确"传原始 url"。
- **Serialize 顺序**：`SafeUrl` derive Serialize 后，JSON 输出是 `{"..."}`（对象）还是 `"..."`（字符串）？需验证——如果是对象，前端类型要改。**预期是字符串**（newtype pattern + Serialize 通常透传内部类型）。

## 后续（非本 plan 范围）

- OBS-CLONE-URL-STORES-PAT-IN-CONFIG（上游 PAT 拒收）：需独立 spec，与本 spec 互补。
- log 宏参数的编译期保证：需自定义 log 宏（如 `log_safe!`），成本高，暂不做。
