# vault_commands.rs 拆分 plan（desktop crate 大文件重构 #3）

> **对应 spec**: `docs/superpowers/specs/2026-07-29-vault-commands-split.md`
> **分支**: `daily_refactor_vault`
> **状态**: ✅ 已完成（2026-07-29）
> **原则**: 纯代码搬家 + glob re-export。与 action_bar_commands 同模式。

## 阶段 0：目录化

### Task 0.1 — vault_commands.rs → vault_commands/mod.rs
- `mkdir -p crates/desktop/src/vault_commands && git mv ... vault_commands/mod.rs`
- 验证：build + test（vault feature）

---

## 阶段 1：子模块提取（按行数从小到大）

### Task 1.1 — window.rs（~125 行）
- `register_vault_autotype_shortcut` / `open_password_generator` / `password_generator_autotype`

### Task 1.2 — session.rs（~115 行）
- `VaultStatus` struct + `vault_status` / `vault_setup` / `vault_unlock` / `vault_lock` / `vault_heartbeat` / `vault_get_lock_timeout` / `vault_set_lock_timeout` / `vault_change_password`

### Task 1.3 — generate.rs（~125 行）
- `TotpResult` struct + `vault_generate` / `vault_evaluate_password` / `vault_generate_totp` / `vault_health_report` / `vault_import_bitwarden` / `vault_export`

### Task 1.4 — autotype.rs（~470 行）
- `AutoTypeResult` struct + `vault_autotype` / `vault_search_ciphers` / `vault_detect_and_match` / `vault_get_cached_url` / `vault_copy_password` / `vault_copy_username`

### Task 1.5 — cipher.rs（~520 行，含测试）
- `AutoTypeMode` enum + folder CRUD（list/create/rename/delete）+ cipher CRUD（list/get/create/update/delete/restore/empty_trash）
- 测试：DTO 转换 + secret key + update_cipher history（~400 行）

---

## 阶段 2：收尾

### Task 2.1 — 文档同步 + 全量验证
- spec status → ✅ + architecture.md
- 全量验证：embedded,cloud,vault / remote-ws / remote-grpc + test

---

## 验证 checklist
- [x] `cargo build -p octopus-desktop --features embedded,cloud,vault` — 0 error 0 warning
- [x] `cargo test -p octopus-desktop` — 441 passed, 0 failed, 1 ignored
- [x] git diff 确认：只搬函数 + re-export（逻辑零改动）

## 回滚
每个 Task 独立 commit。失败 `git reset --hard HEAD~1`。

---

## 完成记录（2026-07-29）

6 个 Task 全部完成（含 Task 0.1 目录化 + Task 1.1–1.5 子模块 + Task 2.1 文档同步）。

最终目录结构 + 偏差 + 验证结果详见 spec 末尾「实施记录」。

| Task | 子模块 | 行数 | commit |
|---|---|---|---|
| 0.1 | 目录化 | — | `1946ef2e` |
| 1.1 | window.rs | 139 | `7ab3f0fe` |
| 1.2 | session.rs | 128 | `8d6cf996` |
| 1.3 | generate.rs | 136 | `1bdd1cae` |
| 1.4 | autotype.rs | 376 | `dfd1a936` |
| 1.5 | cipher.rs（含测试） | 933 | `2d207258` |
| — | mod.rs（留） | 220 | — |
| **合计** | | **1932** | |
