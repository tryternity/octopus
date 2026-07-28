# casing 统一 Task 10: infra DB 映射 — 评估 + 实施

- **Status:** ✅ 已完成（2026-07-28）
- **Date:** 2026-07-28
- **关联：** AGENTS.md「序列化 casing 规范」+ Task 1-9 已完成

## 背景

Task 10 原计划改 infra crate 的所有 DB 直映射 struct（ModelEntry/LlmModelInfo/OcrModelInfo/PromptRecord/VaultMeta/VaultCipher 等）。按「全新库评估」策略（不考虑老数据兼容），调查后发现实际范围远小于预期。

## 评估结论（2026-07-28 全面调查）

infra/db.rs 有 ~20 个 Serialize struct 无 `rename_all`。逐个评估：

| struct | 是否返回前端 | 结论 |
|---|---|---|
| `TranscriptionRecord` | ✅ `get_history` 命令直接返回 | **需改**（唯一） |
| `LlmModelInfo` | ❌ runtime_config.rs 转换成 `LlmOption` 后返回 | 不改（不影响前端） |
| `OcrModelInfo` | ❌ runtime_config.rs 转换成 `OcrOption` 后返回 | 不改 |
| `ModelEntry` / `ModelRow` / `LocalAsrModelRow` | ❌ 纯内部，用 rusqlite row.get() 不经 serde 序列化给前端 | 不改 |
| `VaultMeta` / `VaultCipher` / `VaultFolder` | ❌ vault_commands 通过 CipherDto/FolderDto 转换后返回 | 不改 |
| `PromptRecord` | ❌ 不直接返回前端 | 不改 |
| `LauncherRow` / `FreqRow` / `AsrEngineRow` 等 | ❌ 纯内部 DB 映射 | 不改 |
| `AsrSection` / `AsrConfig` / `CompatibleLlmConfig` | ❌ 配置内部结构，不经 Tauri 边界 | 不改 |

**关键判别原则**：DB 直映射 struct 如果用 `rusqlite row.get()` 读取（按列名/列序，不经 serde），加 `rename_all` 不影响 DB 读写——只影响 Tauri 序列化路径。所以**只有经 Tauri 命令直接返回前端的**才需要改。

## 实施记录

### 2026-07-28 TranscriptionRecord 改造

| 改动 | 文件 |
|---|---|
| 加 `#[serde(rename_all = "camelCase")]` | `crates/infra/src/db.rs:2887` |
| HistoryPanel interface + 消费点改 camelCase | `crates/desktop/frontend/src/pages/Settings/HistoryPanel.tsx` |

字段映射：`created_at` → `createdAt` / `polish_status` → `polishStatus` / `duration_ms` → `durationMs`

### 验证
- cargo build infra + desktop: 0 error 0 warning
- cargo test infra: 154 pass
- pnpm build: 0 error

## 结论

**casing 统一 Task 1-10 全部完成**。全工程 Tauri 命令返回 DTO + 事件 payload + 返回前端的 DB 直映射 struct 统一为 camelCase。不动的只剩三类（协议层 / 外部格式 / vault sync 持久化）。
