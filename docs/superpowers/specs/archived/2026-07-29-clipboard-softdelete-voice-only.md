# 剪贴板软删策略重构：仅 voice 软删 + 回收站 100 条上限

**日期**：2026-07-29
**类型**：行为变更（删除语义反转）
**分支**：`daily_feature_0729`

## 背景与动机

当前软删策略（v47，2026-07-22 引入）：**图片物理删，其他（text/voice/file/ocr）软删进回收站**。

问题：非语音类型（尤其是 text）软删后堆积在回收站，用户实际几乎不还原文本——回收站沦为"第二垃圾场"，徒增 DB 体积与认知负担。语音转录文本是唯一有还原价值的类型（用户可能误删刚识别的段落），值得保留回收站入口。

## 用户决策（2026-07-29）

1. **软删范围收窄为仅 `voice`**：text / image / file / ocr 删除时一律物理删（DELETE）。
2. **voice 回收站 ≤ 100 条上限**（实时不变量）：任何入口删除 voice 后，若回收站内 voice 超过 100 条，把**最老的**（`created_at` ASC）voice 物理删到恰好 100 条。

## 设计

### 不变量

> **INV-1**：`clipboard_history` 中 `item_type='voice' AND is_deleted=1` 的行数恒 ≤ 100（任意删除操作完成后）。

### 删除分流（替换原"image 物理 / 其他软删"）

| 入口 | voice | text/ocr/file/image |
|---|---|---|
| `delete_item` / `delete_items`（单/批量默认删） | 软删 + enforce 100 上限 | 物理删（image 另做 blob 清理） |
| `clear_history` / `clear_history_by_filter`（清空历史） | 软删 + enforce 100 上限 | 物理删 |
| `permanent_delete_*` / 回收站永久删 | 物理删 | 物理删（不变） |
| 自动清理 `cleanup::run_cleanup` | 按 age/count 物理删（不变，属于容量管理，不进回收站） | 物理删（策略对齐） |

**注**：`permanent_delete_*` 和回收站永久删本就是物理 DELETE，不受影响。`cleanup` 是容量管理（按天/数量自动删），逻辑独立，本次保持其内部"物理删"语义但把 text/image 分流的注释更新为"全部物理删"。

### 核心实现

新增 helper：

```rust
/// 判断 id 的 item_type 是否为 voice（查不到返回 false）。
fn is_voice_item(conn: &Connection, id: i64) -> bool

/// 软删 voice 单条后，若回收站 voice 超 100 条，物理删最老的至恰好 100。
/// 返回被物理删的条数（用于 FTS 重建判定）。
fn enforce_voice_trash_limit(conn: &Connection, max_trash: u32) -> Result<usize>
```

`delete_item` / `delete_items`：
```rust
pub fn delete_item(conn, id) -> Result<()> {
    if is_voice_item(conn, id) {
        soft_delete(conn, id)?;
        enforce_voice_trash_limit(conn, VOICE_TRASH_MAX)?; // 100
    } else {
        permanent_delete_item(conn, id)?; // text/ocr/file/image 全物理删
    }
    Ok(())
}
```

**关键改变**：原 `is_image_item` 分流（image 物理 / 其他软删）→ 新 `is_voice_item` 分流（voice 软删 / 其他物理删）。语义完全反转。

`clear_history` / `clear_history_by_filter`：
```rust
// 1. 非 voice 全部物理删（含 image blob 清理）
DELETE FROM clipboard_history WHERE item_type != 'voice' AND {fav/where}
// 2. voice 软删
UPDATE clipboard_history SET is_deleted = 1 WHERE item_type = 'voice' AND {fav/where}
// 3. enforce 100 上限
enforce_voice_trash_limit(conn, 100)?
```

### 常量

```rust
/// voice 软删回收站上限（用户决策 2026-07-29）。
const VOICE_TRASH_MAX: u32 = 100;
```

定义在 `store.rs`，`enforce_voice_trash_limit` 默认用此值，测试可传不同值。

### FTS5 重建

- voice 在 `cleanup::run_cleanup` 中被物理删（容量管理）→ 触发 FTS 重建（既有逻辑不变）。
- `enforce_voice_trash_limit` 物理删 voice → **不在 store 层重建 FTS**（store 层不直接管 FTS，FTS trigger 由 DB 层 `DELETE` 自动维护一致性，`cleanup` 层做 rebuild 是冗余保险）。
- 验证：删除后 `clipboard_history_fts` 不应有指向已删行的 entry（trigger 保证）。

### `is_deleted` 列对非 voice 类型

策略反转后，非 voice 类型删除即物理删，理论上 `is_deleted=1` 只可能出现在 voice 行。schema 注释（`db.sql:74-75`）需更新说明。

### 删除回收站自动清理（`purge_trash` / `trash_purge` 任务）

**用户追加决策（2026-07-29）**：删除剪贴板回收站自动清理功能。

- **删除原因**：voice 软删的 100 条上限已由 `enforce_voice_trash_limit` 在删除入口实时保证（INV-1），回收站自动清理（TTL 3 天 + 容量 500 条）属于多余——现在回收站只有 voice，且其容量已被实时限制。
- **删除内容**：
  - `store.rs::purge_trash` 函数（TTL 超期 + 容量超限双条件物理删）
  - `main.rs` scheduler 的 `trash_purge` 定时任务注册
  - `scheduler/lib.rs` doc 示例改用 `clipboard_cleanup`
- **保留**：`run_cleanup`（按天数/数量清理活跃区——独立功能，与回收站无关）。

### 回收站概念对用户不可见（移除回收站 UI + 相关命令/函数）

**用户追加决策（2026-07-29）**：回收站不再暴露给用户。

- **设计意图**：voice 的逻辑删除（软删）主要用于**热词挖掘**（INV-C1：`list_recent_text` 不过滤 `is_deleted`，软删内容仍是热词来源）及后续优化语音识别准确性，并非给用户当"可还原的删除"用。容量已由 `enforce_voice_trash_limit` 实时保证（≤100），无需暴露回收站概念增加认知负担。
- **删除内容**：
  - **前端**：`ClipboardPanel.tsx` 移除 trash tab（`FILTER_GROUPS` 状态组里删 trash 项）+ 移除 `isTrash` 分支（还原/永久删/清空回收站按钮、空态文案分支）+ 删 `RotateCcw` import（`Trash2` 保留，常规删除仍用）。
  - **i18n**：删 `clipboardPanel` 下 8 个 trash 文案 key（`filterTrash`/`restore`/`permanentDelete`/`emptyTrash`/`trashEmpty`/`restored`/`permanentlyDeleted`/`trashEmptied`），zh-CN + en 各一处。
  - **后端命令**：删 `restore_clipboard_item` / `restore_clipboard_items` / `permanent_delete_clipboard_item` / `permanent_delete_clipboard_items` / `empty_clipboard_trash`（前端无引用后全部成死代码），从 `main.rs` invoke_handler 注册表移除。
  - **store 函数**：删 `restore_item` / `restore_items` / `empty_trash` / `permanent_delete_items`（无内部复用）。**保留 `permanent_delete_item`**（单数）——被 `delete_item` / `delete_items` 内部复用（非 voice 类型的物理删实现）。
- **浮窗侧**（`pages/Clipboard/`）：无需改动——本就无 trash tab、无 trash 命令调用。

## 不在本次范围

- 收藏（`is_favorite`）保护规则不变：收藏项任何删除入口都跳过。
- vault 模块的软删完全独立，不受影响（vault 回收站保留）。

## 测试计划（TDD）

新增到 `store.rs` 的 `#[cfg(test)] mod tests`：

1. **`test_delete_voice_soft_deletes`**：删一条 voice → 行还在，`is_deleted=1`。
2. **`test_delete_text_physical`**：删一条 text → 行不存在（物理删）。
3. **`test_delete_image_still_physical_with_blob_cleanup`**：删 image → 行删 + blob 清理（回归：image 仍物理删）。
4. **`test_voice_trash_limit_enforced_on_delete`**：回收站已有 100 条 voice，再软删 1 条 → 最老 1 条被物理删，回收站恰好 100 条（INV-1）。
5. **`test_voice_trash_limit_below_threshold_noop`**：回收站 < 100 条 → enforce 不删任何行。
6. **`test_clear_history_only_soft_deletes_voice`**：清空历史（含 text+voice+image）→ voice `is_deleted=1`，text/image 行不存在。
7. **`test_clear_history_voice_trash_limit`**：清空历史时若 voice 进回收站后超 100 → enforce 物理删最老。

## 验证命令

```bash
cargo test -p octopus-clipboard --lib
cargo build -p octopus-desktop
```
