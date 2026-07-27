# casing 统一 Task 3: model 域 DTO — 设计规格（spec）

> **Status: ✅ 已实现**（2026-07-27，分支 `feat/record-followup`）。
>
> **本 spec 范围**：model 域所有命令返回 DTO 统一加 `#[serde(rename_all = "camelCase")]`，前端所有消费点（interface + 字段访问）同步改 camelCase。合并原 roadmap Task 3+4+8（model + runtime_config + translation DTO）——因为这些 struct 的 `source_type`/`is_enabled`/`is_available`/`is_streaming`/`is_thinking` 字段跨 struct 共享，前端同一页面消费，必须一次性同步改。
>
> **决策**：`ModelRowData`（前端内部归一 DTO，非 wire）也同步改 camelCase——保证前端全局一致。
>
> **关联文档**：
> - 全工程 casing roadmap：AGENTS.md「序列化 casing 规范」
> - Task 1：`docs/superpowers/specs/2026-07-27-record-task-event-enum.md`
> - Task 2：`docs/superpowers/specs/2026-07-27-casing-unification-record-dto.md`

---

## 实现注记（Implementation Notes）

### 2026-07-27 实施完成

| 检查项 | 结果 |
|---|---|
| A1 cargo build（desktop + translation） | 0 error 0 warning |
| A2 cargo test（desktop + translation） | desktop 422 + translation 20 全过 |
| A3 pnpm tsc --noEmit | 0 error |
| A4 前端 9 文件 snake_case 残留 | 0（Result/index.tsx 的 3 处 `set_polish_mode` 等是 Tauri 命令名字符串，非字段访问，正确不改） |
| A5 后端 11 struct 都有 rename_all | model_commands 9（含已有 6 + 新加 3）+ builtin_models 1 + runtime_config 4 + settings_commands 2 + translation 1 |
| A6 VerifiedCache/VerifiedEntry 未被误改 | 两者都只有 derive，无 rename_all ✅ |

**偏差**：
- 实施用 macOS BSD sed `[[:<:]]`/`[[:>:]]` word boundary 批量替换（`\b` 在 BSD sed 不支持，初次尝试失败后改用 BSD 语法成功）
- model_commands.rs 实际有 9 个 rename_all（spec 说「11 个 struct 加 rename_all」是跨文件总数；model_commands.rs 内 3 个新加 + 6 个已有 = 9）
- 前端 Result/index.tsx 的 3 处 `invoke("set_polish_mode", ...)` 是 Tauri 命令名（snake_case，Tauri 2 自动 camelCase 映射），非字段访问，正确不改

**ModelRowData 决策落地**：`is_ready`/`is_current`/`source_type` 全部改 camelCase（`isReady`/`isCurrent`/`sourceType`），所有 Tab 填充代码 + ModelRow 渲染访问点同步改。

---

## 0. 改动清单

### 0.1 后端（11 个 struct 加 `#[serde(rename_all = "camelCase")]`）

每个 struct 仅在 `#[derive(Serialize...)]` 下一行插入 `#[serde(rename_all = "camelCase")]`。**struct 内部字段名 + 所有构造/访问代码不动**（serde 自动转 wire 名）。

| # | 文件:行 | struct | 受影响 snake_case 字段 |
|---|---|---|---|
| 1 | `crates/desktop/src/model_commands.rs:25` | `DownloadableModel` | `is_available` / `is_enabled` / `source_type` |
| 2 | `crates/desktop/src/model_commands.rs:46` | `VerifyResult` | `broken_files` |
| 3 | `crates/desktop/src/model_commands.rs:82` | `ModelFile` | （无 snake_case 字段，加 rename_all 为未来安全） |
| 4 | `crates/desktop/src/builtin_models.rs:17` | `BuiltinModelInfo` | `is_streaming` |
| 5 | `crates/desktop/src/runtime_config.rs:133` | `ToolbarState` | `asr_engine` / `polish_mode` / `hide_toolbar` / `denoise_mode` / `polish_llm_valid` / `edit_shortcut` / `translate_mode` |
| 6 | `crates/desktop/src/runtime_config.rs:151` | `EngineOption` | `source_type` / `secret_key` / `is_streaming` / `is_thinking` |
| 7 | `crates/desktop/src/runtime_config.rs:168` | `LlmOption` | `source_type` / `secret_key` / `is_streaming` / `is_thinking` |
| 8 | `crates/desktop/src/runtime_config.rs:186` | `OcrOption` | `source_type` |
| 9 | `crates/desktop/src/settings_commands.rs:16` | `ConfigResponse` | `asr_engines` / `llm_models` / `ocr_models` / `active_prompt_id` |
| 10 | `crates/desktop/src/settings_commands.rs:604` | `PromptInfo` | `is_system` |
| 11 | `crates/translation/src/discovery.rs:4` | `TranslationModelInfo` | `size_mb`（跨 crate） |

### 0.2 后端不动（7 个已是 camelCase + 2 个 sidecar 缓存）

- 已是 camelCase（无需改）：`CloudModelInput` / `AsrCloudPreset` / `LlmProviderPreset` / `TranslateCloudModel` / `TestConnectionResult` / `ModelDetail` / `TranslateStatus`
- **❌ 不能动**：`VerifiedCache` / `VerifiedEntry`（`model_commands.rs:95/100`，sidecar 文件缓存 `.verified.json`，加 rename_all 破坏旧缓存兼容）

### 0.3 前端（interface 字段 + 访问点改 camelCase）

**必须改（wire 对齐，8 个文件）**：

| 文件 | interface | 字段访问点数 |
|---|---|---|
| `pages/Settings/Models/AsrTab.tsx` | `EngineOption` / `DownloadableModel` / `VerifyResult` | ~17 |
| `pages/Settings/Models/LlmTab.tsx` | `LlmOption` | ~9 |
| `pages/Settings/Models/OcrTab.tsx` | `DownloadableModel` / `OcrOption` | ~9 |
| `pages/Settings/Models/TranslateTab.tsx` | `DownloadableModel` | ~6 |
| `pages/Settings/Models/ModelRow.tsx` | `ModelRowData`（内部归一） | ~22 |
| `pages/Download/index.tsx` | `BuiltinModelInfo` | ~2 |
| `pages/Result/index.tsx` | `ToolbarState` | ~16 |
| `pages/Settings/index.tsx` | `ConfigResponse` | ~6 |
| `pages/Settings/PromptsPanel.tsx` | `Prompt` + `active_prompt_id` | ~4 |

**不动（已是 camelCase 或非 model 域）**：
- `TranslateCloudModel` / `TranslateStatus` / `CloudModelData` / `AsrPreset` / `LlmPreset`（已是 camelCase）
- `ActionBarItem`（非 model 域）
- `ModelFile` interface（字段全单词）

### 0.4 字段映射表（snake → camel，全局统一）

| snake_case | camelCase |
|---|---|
| `source_type` | `sourceType` |
| `is_available` | `isAvailable` |
| `is_enabled` | `isEnabled` |
| `is_streaming` | `isStreaming` |
| `is_thinking` | `isThinking` |
| `is_ready` | `isReady`（仅 ModelRowData 内部） |
| `is_current` | `isCurrent`（仅 ModelRowData 内部） |
| `is_system` | `isSystem` |
| `broken_files` | `brokenFiles` |
| `secret_key` | `secretKey` |
| `asr_engine` | `asrEngine` |
| `asr_engines` | `asrEngines` |
| `llm_models` | `llmModels` |
| `ocr_models` | `ocrModels` |
| `active_prompt_id` | `activePromptId` |
| `polish_mode` | `polishMode` |
| `polish_llm_valid` | `polishLlmValid` |
| `hide_toolbar` | `hideToolbar` |
| `denoise_mode` | `denoiseMode` |
| `edit_shortcut` | `editShortcut` |
| `translate_mode` | `translateMode` |
| `size_mb` | `sizeMb` |

---

## 1. 验收

| # | 检查项 | 通过标准 |
|---|---|---|
| **A1** | `cargo build -p octopus-desktop -p octopus-translation` | 0 error 0 warning |
| **A2** | `cargo test -p octopus-desktop -p octopus-translation` | 全套无回归 |
| **A3** | `pnpm tsc --noEmit` | 0 error |
| **A4** | grep 确认前端无残留 snake_case | `source_type`/`is_available`/`is_enabled`/`is_streaming`/`is_thinking`/`broken_files`/`secret_key` 等在 9 个前端文件内 0 残留 |
| **A5** | grep 确认后端 11 struct 都有 rename_all | 11 个 `#[serde(rename_all = "camelCase")]` |
| **A6** | grep 确认 VerifiedCache 未被误改 | `model_commands.rs:95/100` 无 rename_all |

## 2. 测试策略

纯字段重命名 + 加 serde 属性，无新逻辑，不写新单测。验收靠 build + tsc + 现有测试套件无回归。

## 3. 实施顺序（降低中间态断裂）

1. **后端先改**：11 个 struct 加 rename_all（一次性，build 验证 0 error）
2. **前端 wire interface 先改**：8 个文件的 interface 字段名改 camelCase（tsc 会报大量字段不存在错误，正常）
3. **前端字段访问点改**：按文件逐个改，每改完一个文件跑一次 tsc 确认该文件 0 error
4. **ModelRowData 最后改**：内部归一 DTO，改它要同步改所有 Tab 的填充代码——放最后避免中间态断裂
5. **最终回归**：build + test + tsc 全过

## 4. 风险

| 风险 | 缓解 |
|---|---|
| 前端字段访问点漏改 | A4 grep 确认 0 残留 |
| `VerifiedCache` 误加 rename_all | A6 grep 确认未改 |
| ModelRowData 改动断裂（22 处） | 实施顺序放最后，改完立即 tsc |
| `ConfigResponse` 嵌套 struct 字段上浮 | 后端 EngineOption 等加 rename_all 后，ConfigResponse 序列化时嵌套字段自动 camelCase——前端只需改顶层 key + 嵌套字段访问 |
