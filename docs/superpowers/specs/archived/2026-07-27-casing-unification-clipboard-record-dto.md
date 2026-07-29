# casing 统一 Task 7: clipboard + RecordingMeta — 设计规格（spec）

> **Status: ✅ 已实现**（2026-07-27，代码已完成；spec 状态 2026-07-28 补标）。
>
> **本 spec 范围**：clipboard crate（MetaInfo/FileEntry/ClipboardItem）+ RecordingMeta 加 `#[serde(rename_all = "camelCase")]`，前端同步改。需清库（MetaInfo 持久化到 DB）。
>
> **关联文档**：AGENTS.md「序列化 casing 规范」+ Task 1-6 已完成。

---

## 实现注记（Implementation Notes）

### 2026-07-27 实施完成

| struct | 文件 | rename_all | 备注 |
|---|---|---|---|
| MetaInfo | `crates/clipboard/src/model.rs:40` | ✅ camelCase | 持久化到 DB meta_info 列，按全新库策略不兼容老数据 |
| FileEntry | `crates/clipboard/src/model.rs:70` | ✅ camelCase | 字段级 `#[serde(rename = "type")]` 仍优先（file_type → "type"） |
| ClipboardItem | `crates/clipboard/src/model.rs:78` | ✅ camelCase | DB 行映射 + 命令返回 DTO 双重身份 |
| RecordingMeta | `crates/record/src/store.rs:19` | ✅ camelCase | 翻转 Task 4.1 的「故意不加」决策 |

**ItemType enum 不动**（行 4 `rename_all = "snake_case"` 是变体名 text/voice/ocr/image/file，持久化到 DB item_type 列，改了破坏数据）。

---

## 0. 改动清单

### 后端

| struct | 文件:行 | snake_case 字段 | 备注 |
|---|---|---|---|
| `MetaInfo` | `crates/clipboard/src/model.rs:40` | `duration_ms`/`char_count`/`asr_mode`/`polish_model` | **持久化到 DB**（meta_info 列 JSON）——加 rename_all 后需清库 |
| `FileEntry` | `crates/clipboard/src/model.rs:69` | `file_type`（有 `#[serde(rename = "type")]`） | 字段级 rename 优先于 rename_all，加后 `file_type` 仍输出 `"type"`（实测验证） |
| `ClipboardItem` | `crates/clipboard/src/model.rs:76` | `item_type`/`ref_data`/`meta_info`/`is_favorite`/`is_rich`/`created_at`/`has_thumbnail`/`deleted_at` | DB 行映射 + 命令返回 DTO 双重身份 |
| `RecordingMeta` | `crates/record/src/store.rs:19` | `file_path`/`duration_ms`/`has_system_audio`/`has_microphone`/`audio_tracks`/`source_type`/`file_size`/`has_thumbnail`/`is_favorite`/`created_at`/`deleted_at` | **翻转 P1 决策**——之前故意不加，现统一加 camelCase，更新 struct 注释 |

**ItemType**（enum，行 5）已有 `rename_all = "snake_case"`——这是 enum 变体名（text/voice/ocr/image/file），**不动**（变体名是数据值不是字段名，且持久化到 DB item_type 列）。

### 前端

| 文件 | 改动 |
|---|---|
| `types/clipboard.ts` | MetaInfo + ClipboardItem interface 字段 |
| `pages/Settings/ClipboardPanel.tsx` | 字段访问 |
| `pages/Settings/RecordingPanel.tsx` | RecordingMeta interface（audio_tracks 已在 Task 4.1 fix 时改 snake_case，现在翻转回 camelCase）+ 其他字段 |

### 清库

加 rename_all 后，老 DB 数据（MetaInfo snake_case JSON）读会丢字段。用户确认清库。**清库步骤**：删除 `~/.octopus/octopus.db`（或仅清 clipboard_items 表）。

## 1. 验收

| # | 检查项 | 通过标准 |
|---|---|---|
| A1 | cargo build | 0 error 0 warning |
| A2 | cargo test | 全套无回归 |
| A3 | pnpm build（含 tsc -b） | 0 error |
| A4 | grep 前端 clipboard + recording snake_case 残留 | 0 |
| A5 | 后端 4 struct 都有 rename_all | 4 |
| A6 | RecordingMeta struct 注释已更新（移除「故意不加」改为「统一 camelCase」） | 是 |

## 2. 关键技术点

- **FileEntry 的 `#[serde(rename = "type")]`**：字段级 rename 优先于 struct 级 rename_all（实测验证）。加 rename_all 不破坏 `file_type → "type"` 映射。
- **ItemType enum 不动**：它的 `rename_all = "snake_case"` 是变体名（数据值），不是字段名。且持久化到 DB（item_type 列存 "text"/"voice" 等），改了破坏老数据。
- **RecordingMeta 翻转 P1**：之前 Task 4.1 blocker 后加「故意不加 rename_all」注释，现在全工程统一 camelCase，翻转决策——加 rename_all + 更新注释 + 前端 audio_tracks 翻回 audioTracks。
