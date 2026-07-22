# Clipboard Soft Delete & Trash Implementation Plan

> 本 plan 已全部实现完成 ✅。

**Goal:** 剪贴板文本类条目（text/voice/ocr/file）删除时进回收站（软删 UPDATE deleted_at），图片仍物理删。回收站支持还原 / 永久删 / 清空。热词挖掘（list_recent_text）不受影响（INV-C1）。

**关联 spec:** `docs/superpowers/specs/2026-07-22-clipboard-soft-delete.md`

> **状态**：全部完成 ✅
> - cargo check: 0 error 0 warning
> - tsc: 0 error
> - cargo test -p octopus-clipboard: 17 pass
> - cargo test -p octopus-infra: 158 pass

## 不变量

- **INV-C1（热词来源不断）**：`list_recent_text` 故意不过滤 `deleted_at`。软删行还在→仍被挖掘；永久删（DELETE）后行不在→挖不到。
- **INV-C2（FTS 自动保留）**：FTS5 trigger `clip_fts_ad` 仅绑 AFTER DELETE，软删用 UPDATE 不触发→索引保留。
- **INV-C3（图片物理删）**：image_data 靠 clipboard_history 引用计数，软删行还在→refcount 不归零→blob 泄漏。
- **INV-C4（回收站隔离）**：deleted_at IS NOT NULL 的行只在 filter="trash" 时返回。

---

## 修改文件（11 个）

| 文件 | 改动 |
|---|---|
| `crates/infra/src/db.sql` | clipboard_history 加 `deleted_at TEXT DEFAULT NULL` 列 + 索引 `idx_clip_deleted` |
| `crates/infra/src/db.rs` | init_schema 加 v46→v47 迁移；list_recent_text 加 INV-C1 注释；全新库直设 v47；4 个测试 assert v46→v47 |
| `crates/clipboard/src/store.rs` | SELECT_COLS / FTS JOIN / row_to_item 加 deleted_at；build_where 加 INV-C4 + "trash"；新增 6 函数 + is_image_item；删除分流 |
| `crates/clipboard/src/model.rs` | ClipboardItem struct 加 `deleted_at: Option<String>` |
| `crates/clipboard/src/cleanup.rs` | delete_by_age/count 返回 (soft, phys) 元组；图片物理删/文本软删分流；FTS rebuild 条件改 |
| `crates/desktop/src/clipboard_commands.rs` | 新增 5 命令 |
| `crates/desktop/src/main.rs` | invoke_handler! 注册 5 个新命令 |
| `crates/desktop/frontend/src/types/clipboard.ts` | ClipboardItem 加 `deleted_at?: string \| null` |
| `crates/desktop/frontend/src/pages/Settings/ClipboardPanel.tsx` | FILTER_GROUPS 加 trash tab；行内还原+永久删按钮；全部清空；空状态 |
| `crates/desktop/frontend/src/locales/en.yaml` | clipboardPanel 段 +8 key |
| `crates/desktop/frontend/src/locales/zh-CN.yaml` | clipboardPanel 段 +8 key |

---

## Task 概览（9 组，全部完成 ✅）

| # | Task 组 | 状态 |
|---|---|---|
| 1 | DB schema（db.sql + db.rs 迁移 + INV-C1 注释） | ✅ |
| 2 | store.rs（CRUD + 删除分流 + build_where） | ✅ |
| 3 | clipboard_commands.rs + main.rs（5 命令） | ✅ |
| 4 | 前端 ClipboardPanel.tsx + types/clipboard.ts | ✅ |
| 5 | i18n（en + zh-CN） | ✅ |
| 6 | cleanup.rs（自动清理分流） | ✅ |
| 7 | 验证（cargo check / tsc / test） | ✅ |
| 8 | infra cpu.rs + octopus-scheduler crate（TTL 调度） | ✅ |
| 9 | purge_expired_trash + desktop setup 注册任务 | ✅ |

---

## 不变量测试覆盖

| 不变量 | 保证方式 |
|---|---|
| INV-C1 | `list_recent_text` SQL 内联注释 + doc 注释；无 deleted_at 过滤子句 |
| INV-C2 | `cleanup.rs` FTS rebuild 条件 `physical_deleted > 0`；FTS trigger 仅 AFTER DELETE |
| INV-C3 | `store.rs` 所有删除入口经 `is_image_item` 分流；图片分支调 `permanent_delete_item` |
| INV-C4 | `build_where` 非 trash 分支追加 `AND deleted_at IS NULL`；trash 分支反向 |

## Follow-up（可选，P2）

- 回收站 TTL（软删 N 天后自动永久删）
- 浮窗暴露回收站入口
- 跨设备同步软删态
