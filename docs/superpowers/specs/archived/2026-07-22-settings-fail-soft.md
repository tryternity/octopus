# 设置页 fail-soft 健壮性改造

> **日期**：2026-07-22
> **状态**：✅ 已实现（cargo check 0 error；tsc 0 error。models 表 SQL 错误不再拖垮整个设置页）

---

## 0. 问题

`get_config` 命令把 6 个独立数据源（app_config / models×3 / prompts / microphones）塞进一个无容错的 `?` 传播链。models 表 SQL 错误（如 schema 变更期间 `is_local` 列不存在）→ 整个 `get_config` 返回 Err → 前端 `configResp` 永远 null → 除 clipboard/system 外的**所有**设置页面卡在 loading。

**错误传播链**：
```
models 表 SQL 错误
  → get_config 返回 Err（line 34/39/42/47 的 ? 传播）
  → index.tsx configResp 保持 null（line 78 catch 只 toast）
  → !configResp 分支（line 168）挡住 models/prompts/actionbar/agent/vault/hotword/settings 全部页面
  → 只有 clipboard（自己 invoke query_clipboard_history）和 system（自己 invoke subscribe_system_status）不受影响
```

**根因**：独立的页面被不相关的数据源错误拖垮。

> **注**：原文描述的 `is_local` 列错误是触发场景之一（已被 source_type 迁移 builtin-models spec §4 解决）。但 fail-soft 的价值是泛化的——任何未来 schema 变更期间或 DB 损坏导致的 models/prompts 表 SQL 错误，都不会再拖垮整个设置页。

---

## 1. 不变量

- **INV-F1（数据源隔离）**：`get_config` 内每个数据源查询失败只影响该字段（返空数组），不传播到其他字段或整个命令
- **INV-F2（app_config 致命）**：`load_config()`（app_config 表）失败仍返回 Err——没有配置整个设置页没意义
- **INV-F3（页面独立）**：不依赖 `configResp` 的页面（models/prompts/actionbar/agent/vault）不被 `!configResp` loading 阻塞

---

## 2. 改动

### 2.1 后端：`get_config` 各数据源独立容错

**文件**：`crates/desktop/src/settings_commands.rs`

| 数据源 | 改前 | 改后 |
|---|---|---|
| `load_config()` (app_config) | `?` 传播 | **保持 `?`**（致命） |
| `list_engines_from_db()` (models asr) | `?` 传播 | `unwrap_or_else(warn + vec![])` |
| `list_llm_models()` (models llm) | `?` 传播 | `unwrap_or_else(warn + vec![])` |
| `list_ocr_models()` (models ocr) | `?` 传播 | `unwrap_or_else(warn + vec![])` |
| `list_prompts()` (prompts) | `?` 传播 | `unwrap_or_else(warn + vec![])` |
| `list_microphones()` (cpal) | 已有容错 | 不变 |
| `load_active_prompt_id()` | `.unwrap_or(1)` | 不变 |

### 2.2 前端：`!configResp` 降级路由

**文件**：`crates/desktop/frontend/src/pages/Settings/index.tsx`

把 models/prompts/actionbar/agent/vault 移到 `!configResp` 判断**之前**（它们各自 invoke 独立命令，不依赖 configResp）。只有 settings(GeneralPanel) 和 hotword 留在 `!configResp` 后面。

改前路由顺序：
```
clipboard → system → !configResp? loading → settings → models → prompts → actionbar → agent → hotword → vault
```

改后路由顺序：
```
clipboard → system → models → prompts → actionbar → agent → vault → !configResp? loading → settings → hotword
```

---

## 3. 改造后行为

| 场景 | 改前 | 改后 |
|---|---|---|
| models 表 SQL 错误 | 所有页面（除 clipboard/system）卡 loading | models 页面空列表 + log warn；其他页面正常 |
| prompts 表 SQL 错误 | 所有页面卡 loading | prompts 页面空列表 + log warn；其他页面正常 |
| app_config 表错误 | 所有页面卡 loading | **不变**——仍卡 loading（致命错误） |
| 正常 | 所有页面正常 | 不变 |

---

## 4. 不在范围

- 不拆 `get_config` 为多个命令（改动太大，且当前 fail-soft 已解决核心问题）
- 不改 models 表 schema（另一个 session 在做）
- 不改 PromptsPanel/ModelsTab 的浪费式 get_config 调用（后续优化，低优先级）
