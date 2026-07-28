# DTO 转换层精简——casing 统一后的架构简化

- **Status:** 🔜 待实现（2026-07-28）
- **Date:** 2026-07-28
- **关联：** AGENTS.md「序列化 casing 规范」+ casing 统一 Task 1-10 已完成

## 背景

casing 统一 Task 1-10 完成后，所有返回前端的 struct 都加了 `#[serde(rename_all = "camelCase")]`。这暴露了一个架构冗余：原本「内部 snake_case struct → DTO camelCase 转换 → 前端」的三层结构，在内部 struct 也 camelCase 后，DTO 转换层变成纯开销。

## 调研结论（2026-07-28 全面调查）

### DTO 分类

| 类别 | 数量 | 例子 | 能否删 |
|---|---|---|---|
| **纯冗余 DTO**（字段 1:1 镜像内部 struct，无转换） | ~14 | VaultStatusDto / TotpResultDto / VerifyResult / RecordStatus / MergeResult / ProcessStats 等 | ✅ 删 |
| **加密格式 DTO**（镜像内部 struct + 内部 struct 持久化为加密 JSON） | 3 | LoginDataDto / LoginUriDto / FieldDto | ⚠️ Phase 2（需 alias 处理老数据） |
| **真转换 DTO**（enum 展平/字段过滤/字段重命名/计算字段） | ~12 | CipherDto / CipherInputDto / DownloadableModel / TimeSeries / ModelMemory 等 | ❌ 不删（有架构价值） |

### CipherDto 不能删的真相

`CipherDto` 不是纯 casing 镜像——它做了 3 件真转换：
1. `CipherData::Login(LoginData)` tagged enum → `Option<LoginDataDto>` + `atype: i64`（展平）
2. `CipherType`/`RepromptType` enum → `i64`
3. 丢 `password_history` 字段

即使 casing 统一，这个转换层仍有价值（wire shape 简化）。删除需前端协同改成 tagged enum，属 Phase 3（本次不做）。

## 设计

### Phase 1：删除 14 个纯冗余 DTO（安全，无格式风险）

**策略**：给被镜像的内部 struct 加 `rename_all = "camelCase"`（如果没有的话），命令直接返回内部 struct。

| 删除的 DTO | 文件 | 替代（直接返回的内部 struct 或 inline） |
|---|---|---|
| `VaultStatusDto` | vault_commands.rs | inline struct（2 字段，is_initialized + is_unlocked） |
| `TotpResultDto` | vault_commands.rs | inline struct（code + seconds_remaining） |
| `AutoTypeResultDto` | vault_commands.rs | inline struct（filled + message + fallback_to_clipboard） |
| `PasswordStrengthDto` | vault_commands.rs | `PasswordStrength` 加 rename_all 后直接返回 |
| `VerifyResult` | model_commands.rs | inline struct（ok + message） |
| `TestConnectionResult` | model_commands.rs | inline struct（ok + message） |
| `ModelDetail` | model_commands.rs | inline struct（聚合 source + secret_key 等） |
| `RecordStatus` | record_commands.rs | inline struct（state + elapsed_secs） |
| `MergeResult` | record_commands.rs | inline struct（new_id + file_path） |
| `ProcessStats` | system_status_commands.rs | inline struct（cpu + memory 等） |
| `SystemStats` | system_status_commands.rs | inline struct |
| `TranslateStatus` | translation_commands.rs | inline struct（strategy + engine_name + available） |
| `PromptInfo` | settings_commands.rs | `PromptRecord` 加 rename_all 后直接返回 |
| `LlmProviderPreset` | model_commands.rs | `LlmProviderPresetRow` 加 rename_all 后直接返回 |

**内部 struct 需加 rename_all**（Phase 1 前置）：
- `octopus_vault::health::strength::PasswordStrength`
- `octopus_infra::db::PromptRecord`
- `octopus_infra::db::LlmProviderPresetRow`

### Phase 2：vault 加密格式 DTO 删除（需 alias 处理老数据）

**风险**：LoginData/Field/PasswordHistoryEntry 序列化为 JSON 后用 user_vault_key 加密存 DB。改 casing 破坏老加密数据。

**策略**：用 `#[serde(alias = "...")]` 让 camelCase 字段同时接受老 snake_case JSON。

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LoginData {
    pub uris: Vec<LoginUri>,
    pub username: Option<String>,
    pub password: Option<String>,
    pub totp: Option<String>,
    #[serde(alias = "password_revision_date")]
    pub password_revision_date: Option<String>,  // 同时接受 passwordRevisionDate 和 password_revision_date
}
```

这样老加密数据（snake_case）和新数据（camelCase）都能正确反序列化。无需迁移脚本。

**删除的 DTO**：
- `LoginUriDto` → `LoginUri` 加 rename_all + alias
- `LoginDataDto` → `LoginData` 加 rename_all + alias
- `FieldDto` → `Field` 加 rename_all + alias

**保留**：`CipherDto` / `CipherInputDto`（真转换层，Phase 3 才动）

## 验收标准

| # | 检查项 | 通过标准 |
|---|---|---|
| A1 | Phase 1 删除 | 14 个纯冗余 DTO 删除 + 命令直接返回内部 struct |
| A2 | Phase 2 删除 | LoginDataDto/LoginUriDto/FieldDto 删除 + LoginData/Field 加 alias + rename_all |
| A3 | 老加密数据兼容 | 老加密 cipher 解密后字段不丢（alias 兜底） |
| A4 | cargo build + test | 0 error 0 warning + 全套测试 pass |
| A5 | pnpm build | 0 error（前端 interface 可能需同步） |
| A6 | grep 残留 | 被删的 DTO 名 0 残留 |

## 不动

- `CipherDto` / `CipherInputDto`（真转换层，Phase 3）
- `DownloadableModel` / `TranslateCloudModel` / `ModelFile` / `TimeSeries` / `ModelMemory` / `OcrTextBlock`（真转换）
- 所有输入 DTO（`CloudModelInput` / `RecordConfig` 等）
- 所有事件 payload（`RecordTaskEvent` / `OpenTabPayload` 等）
