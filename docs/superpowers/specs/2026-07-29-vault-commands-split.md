# vault_commands.rs 拆分 spec（desktop crate 大文件重构 #3）

> **Status: ✅ 已实现**（2026-07-29，分支 `daily_refactor_vault`）

## 背景

`crates/desktop/src/vault_commands.rs` 1832 行（含 685 行测试，占 37%），是 desktop crate 当前第 2 大功能域文件。承载密码保险库的全部 Tauri 命令：会话管理、条目 CRUD、密码生成/评估、自动填写、导入导出。

前两个大文件拆分已完成：coordinator.rs（3085 行）+ action_bar_commands.rs（2441 行）。vault_commands.rs 是下一个。

## 现状结构分析

### 与 action_bar_commands 同模式

~50 个 `pub fn`（多数 `#[tauri::command]`，`#[cfg(feature = "vault")]` 门控）+ helper 函数 + struct/enum。跨文件引用密集：
- **invoke_handler.rs** 注册 33 个命令（`vault_commands::xxx`，全部 `#[cfg(feature = "vault")]` 门控）
- **12 处外部文件**通过 `crate::vault_commands::xxx` 引用

→ 同样用 **glob re-export**（`pub use submodule::*`）保持路径不变。

### vault feature gate（关键差异）

整个 `vault_commands` 模块在 main.rs 是 `#[cfg(feature = "vault")] mod vault_commands;`。拆分后子模块继承这个 gate——所有函数本身就是 `#[cfg(feature = "vault")]` 上下文，不需要额外处理。

### 职责聚类（5 组 → 5 子模块 + mod.rs）

| 子模块 | 行数（含测试） | 内容 |
|---|---|---|
| `mod.rs`（留） | ~40 | mod 声明 + glob re-export + 共享 helper（require_user_vault_key / require_app_key_from_session / cipher_to_dto / dto_to_input / merge_password_history + CipherDto / CipherInputDto struct） |
| `session.rs` | ~115 | vault_status / setup / unlock / lock / heartbeat / get/set_lock_timeout / change_password + VaultStatus struct |
| `cipher.rs` | ~520（含 ~400 行测试） | list/get/create/update/delete/restore/empty_trash ciphers + folder CRUD + AutoTypeMode enum |
| `generate.rs` | ~125 | vault_generate / evaluate_password / generate_totp / health_report / import_bitwarden / export + TotpResult struct |
| `autotype.rs` | ~470 | vault_autotype / search_ciphers / detect_and_match / get_cached_url / copy_password / copy_username + AutoTypeResult struct |
| `window.rs` | ~125 | register_vault_autotype_shortcut / open_password_generator / password_generator_autotype |

## 目标目录结构

```
crates/desktop/src/vault_commands/
├── mod.rs           # ~40 行：mod 声明 + glob re-export + 共享 helper/DTO struct
├── session.rs       # 会话管理（解锁/锁定/心跳/超时/改密）
├── cipher.rs        # 条目 CRUD + folder CRUD（含大量 DTO 转换测试）
├── generate.rs      # 密码生成/评估/TOTP/健康/导入导出
├── autotype.rs      # 自动填写/搜索/检测/复制
└── window.rs        # 快捷键注册 + 生成器浮窗
```

## 拆分约束（不变量）

### 1. glob re-export
mod.rs 顶部 `pub use submodule::*;`，`crate::vault_commands::xxx` 路径不变。

### 2. 共享 helper 留 mod.rs
`require_user_vault_key` / `require_app_key_from_session` / `cipher_to_dto` / `dto_to_input` / `merge_password_history` + `CipherDto` / `CipherInputDto` struct 被多个子模块用，留 mod.rs（子模块 `use super::{...}` 引用）。

### 3. 测试分布
685 行测试集中在 DTO 转换（cipher_to_dto / dto_to_input）+ secret key + update_cipher history → 多数搬 cipher.rs。

### 4. 逻辑完全不变
纯代码搬家。

## 风险
低。与 action_bar_commands 完全同模式，已验证 glob re-export 可行。

## 不做
- 不改函数逻辑
- 不改 Tauri 命令签名
- 不改 invoke_handler.rs / 外部引用路径

---

## 实施记录（2026-07-29）

### 最终目录结构

```
crates/desktop/src/vault_commands/
├── mod.rs       220 行：mod 声明 + glob re-export + 共享 helper/DTO
│                      （CipherDto / CipherInputDto / require_user_vault_key /
│                       require_app_key_from_session / cipher_to_dto / dto_to_input /
│                       PASSWORD_HISTORY_MAX / merge_password_history）
├── window.rs    139 行：register_vault_autotype_shortcut / open_password_generator /
│                       password_generator_autotype
├── session.rs   128 行：VaultStatus + 8 个会话命令（status/setup/unlock/lock/
│                       heartbeat/get+set_lock_timeout/change_password）
├── generate.rs  136 行：TotpResult + vault_generate/evaluate_password/generate_totp/
│                       health_report/import_bitwarden/export
├── autotype.rs  376 行：AutoTypeResult + VAULT_DETECT_MATCH_LIMIT + 6 个自动填写命令
│                       （autotype/search_ciphers/detect_and_match/get_cached_url/
│                        copy_password/copy_username）
└── cipher.rs    933 行（含 ~685 行测试）：AutoTypeMode + folder CRUD（4 命令）+
                       cipher CRUD（7 命令：list/get/create/update/delete/restore/
                       empty_trash）+ 全部 DTO 转换 / secret_key / history /
                       detect_match 测试
```

合计 1932 行（原 1832 行 + 拆分新增的子模块文件头注释 / use 导入 ~100 行）。

### 关键决策与偏差

1. **VAULT_DETECT_MATCH_LIMIT 归属 autotype.rs**：spec 原文未明确，实施时放 autotype.rs
   （`vault_detect_and_match` 的核心常量），mod.rs 通过 glob re-export 让
   `crate::vault_commands::VAULT_DETECT_MATCH_LIMIT` 路径不变，cipher.rs 测试通过该
   crate 路径引用。

2. **PASSWORD_HISTORY_MAX 留 mod.rs**：被共享 helper `merge_password_history` 使用，
   跟 helper 一起留 mod.rs（cipher.rs 测试通过 `crate::vault_commands::PASSWORD_HISTORY_MAX`
   引用）。

3. **AutoTypeMode 归属 cipher.rs**（按 plan），但 autotype.rs 的 `vault_autotype` 也用。
   autotype.rs 用 `use super::AutoTypeMode;`——经 mod.rs 的 `pub use cipher::*` glob
   re-export 生效，跨子模块引用透明。

4. **测试全搬 cipher.rs**：685 行测试（DTO 转换 / secret_key / history / detect_match）
   全部归 cipher.rs，因测试逻辑紧密围绕 cipher 域 helper/命令。测试中 `super::*` 指向
   cipher.rs 本身，故通过 `crate::vault_commands::{...}` 显式引用 mod.rs 的 helper
   （cipher_to_dto / dto_to_input / merge_password_history / require_app_key_from_session
   / CipherInputDto / PASSWORD_HISTORY_MAX / VAULT_DETECT_MATCH_LIMIT）。测试路径从
   `vault_commands::tests::*` 变为 `vault_commands::cipher::tests::*`（441 passed 不变）。

5. **crate::autotype 命名冲突**：autotype.rs 同时是 vault_commands 子模块名，又需要调用
   `crate::autotype::*`（键盘模拟）。不能 `use crate::autotype;`（与 `mod autotype;`
   E0255 重复定义），改用全路径调用 `crate::autotype::autotype_login_with_mode` 等。

### 验证结果

```
cargo build -p octopus-desktop --features embedded,cloud,vault  → 0 error 0 warning
cargo test  -p octopus-desktop                                   → 441 passed, 0 failed, 1 ignored
```

### 6 个 commit（每个 Task 独立）

- `1946ef2e refactor(desktop): vault_commands 目录化（Task 0.1）`
- `7ab3f0fe refactor(desktop): vault_commands window.rs 子模块提取（Task 1.1）`
- `8d6cf996 refactor(desktop): vault_commands session.rs 子模块提取（Task 1.2）`
- `1bdd1cae refactor(desktop): vault_commands generate.rs 子模块提取（Task 1.3）`
- `dfd1a936 refactor(desktop): vault_commands autotype.rs 子模块提取（Task 1.4）`
- `2d207258 refactor(desktop): vault_commands cipher.rs 子模块提取（Task 1.5）`

