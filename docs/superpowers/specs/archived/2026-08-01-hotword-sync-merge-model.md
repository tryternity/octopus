# 热词同步升级到 merge 模型

> **日期**：2026-08-01
> **状态**：✅ 已实现
> **背景**：`docs/pr/0801.md` 第 1 条——「热词的更新失败，添加了很多热词，但同步的时候会被仓库里面的数据覆盖」
> **前置 spec**：[2026-07-25-hotword-sync-overwrite-bug](archived/2026-07-25-hotword-sync-overwrite-bug.md)（已归档，其修复已被 revert，本次取代）

---

## 1. 问题复述

用户在热词集里加词，手动/定时触发 git 同步后，新加的词消失——被仓库里的旧数据覆盖。

这是 [2026-07-25 spec](archived/2026-07-25-hotword-sync-overwrite-bug.md) 记录的同一个 bug 的**回归**：那次修复（`sync_now` 在 `UpToDate` 时跳过 pull）被 `10e56330`（2026-07-27）revert，而 revert 的论证只对 vault 成立。

## 2. 根因（回归链）

### 2.1 热词 pull 无方向感知

`pull_hotwords_from_files`（`crates/sync/src/hotword.rs`）的判定逻辑：

```rust
let needs_update = !db_ids.contains(uuid)
    || hotword_md5_mismatch_v2(uuid, &entry.md5, &db_sets);
// hotword_md5_mismatch_v2 = (db.sync_md5 != outline.md5) —— 只比 md5，不判方向
```

`needs_update == true` 时，读文件 + `upsert_hotword_set`（`ON CONFLICT(id) DO UPDATE SET words_text=excluded.words_text, ...` 全字段覆盖）。

### 2.2 触发链（单设备）

1. 上次 sync：热词集「通用」（含词A）已 commit + push，仓库 outline md5 = `md5(A)`，文件 = "A"
2. 本地加词 B → `refill_sync_md5` 把 DB `sync_md5` 更新为 `md5(A+B)`
3. `sync_now` 触发：
   - `git merge --ff-only` → 远端无新 commit → `MergeFfResult::UpToDate`
   - **热词 pull 无条件执行**（`skip_pull` 仅 `NoUpstream` 为 true）
   - pull 读旧 outline（md5(A)）对比 DB（md5(A+B)）→ mismatch → 读旧文件（"A"）→ `upsert_hotword_set` 覆盖 DB → **词 B 丢失**
   - push 把 DB（现在又变回 "A"）导出到文件 → commit + push → 词 B 永久从远端消失

### 2.3 回归为什么发生

| 时间 | commit | 行为 |
|---|---|---|
| 2026-07-25 | `52cb99d6` | 正确修复：`UpToDate` 时 `skip_pull = true`（vault + hotword 都跳过 pull） |
| 2026-07-27 | `10e56330` | **revert**：`skip_pull` 改回只在 `NoUpstream` 为 true。论证：「pull 内部已有 md5 比对，不会无脑覆盖」|
| 2026-07-27 后 | `ad415b3c` | vault 迁移到真正的 3-way merge `merge_vault`（按 `updated_at` 判方向） |

**revert 的论证只对 vault 成立**：vault 后来有了 `merge_vault`，pull 不再被生产路径调用（2026-07-29 作为死代码删除）。但热词的 `pull_hotwords_from_files` **从未迁移**，仍是无方向覆盖。`engine.rs:723-724` 的注释当时坦承「热词子系统独立维护 pull/push 两步，不受 vault merge 影响——本机 vault merge 模型尚未推广到 hotword」。

`merge_vault` 内部确实有 md5 比对保护（不无脑覆盖），但这保护**只在 vault 路径生效**——热词走的是独立的 `pull_hotwords_from_files`，没有这层保护。

## 3. 修复方案：热词升级到 merge 模型

### 3.1 核心：新增 `merge_hotwords`（对称于 `merge_vault`）

`crates/sync/src/hotword.rs` 新增 `merge_hotwords() -> Result<HotwordMergeReport>`，拷贝 `merge_vault` 的 3-way merge 模板，去掉 vault 特有的 stamp/meta 校验（热词无加密、无 meta、无 folder）：

```
对 outline 每条 entry（uuid, md5, updated_ms）：
  - DB 无 → pull（读文件 upsert，回填 sync_md5）
  - DB 有：
    - remote_updated_ms > local_updated_ms（iso_to_unix_ms(db.updated_at)）→ pull 覆盖 DB
    - local > remote → push 覆盖文件
    - 相等 → md5 比对，不等则冲突 DB 赢（push 到文件）

DB 有 + outline 无 → push 写文件
末尾：export_all_hotwords 从 DB 重建 outline（DB 是单一真相源）
```

### 3.2 `HotwordMergeReport`（独立于 vault `MergeReport`）

sync crate 不能依赖 vault crate（依赖方向是 vault → sync），故 sync crate 内定义轻量 `HotwordMergeReport { pulled, pushed, conflicts, skipped }`。`sync_now` 调用方映射进 `SyncReport` 的 `hotwords_pulled` / `hotwords_pushed` 字段（前端 message 显示语义不变）。

### 3.3 `sync_now` 切换

```rust
// 改前（两步，pull 无方向）：
let hotwords_pulled = if skip_pull { 0 } else { pull_hotwords_from_files()? };
let hotwords_pushed = push_hotwords_to_files()?;

// 改后（单步 merge，对称于 merge_vault）：
let hotwords_merged = if skip_pull {
    // NoUpstream（首次推送）：远程无内容可 merge，直接 push
    HotwordMergeReport { pushed: push_hotwords_to_files()?, .. }
} else {
    merge_hotwords()?
};
```

`skip_pull` 语义不变（仅 `NoUpstream`）——现在它对热词也安全：merge 模型有方向判断，`UpToDate` 时跑 merge 不会覆盖（remote_updated 不会大于 local_updated）。

### 3.4 保留的函数

- `push_hotwords_to_files`：仍被 `skip_pull`（NoUpstream）分支 + `enable_sync`（首次启用同步）路径使用 → **保留**
- `pull_hotwords_from_files`：生产路径不再调用，**保留**作为首次 clone 场景 reference + 测试覆盖其「无方向」设计契约（`pull_function_direction_blind_by_design` 测试，防未来误改）

## 4. 测试

TDD 先写 4 个 merge 测试（`crates/sync/src/hotword.rs`）：

| 测试 | 场景 | 期望 |
|---|---|---|
| `merge_pulls_remote_newer_set` | outline updated_ms 较新 + DB 旧 | DB 被远程覆盖 |
| `merge_keeps_local_newer_set_not_overwritten` | **核心回归**：outline 旧 + DB 新加词 | DB 保留新词，文件被更新 |
| `merge_pushes_db_only_set` | DB 有 + outline 无 | 文件被写出，outline 重建 |
| `merge_db_wins_on_equal_timestamp_md5_conflict` | updated_ms 相等 + md5 不等 | DB 赢（文件被 DB 覆盖） |

时间戳策略：DB 的 `updated_at = datetime('now')` ≈ 当前毫秒；构造「远程新」用远未来 `updated_ms`（如 `9999999999999`），「远程旧」用 `updated_ms: 1`。

既有 `pull_overwrites_local_new_data_when_outline_stale_documented_bug` 改名为 `pull_function_direction_blind_by_design`，注释从「文档化 bug」改为「文档化设计契约」（pull 的无方向特性是设计，非 bug；bug 在于 sync_now 曾依赖它做双向同步）。

## 5. 已知问题（不在本次修复）

- [x] **热词 set 级删除复活（跨设备）**：~~`delete_hotword_set` 是硬删（`DELETE FROM hotword_sets + hotword_words`，无 set 级 `is_deleted`）。A 删集 → push 删文件 → B merge 不删 DB → B push 又写回 → A merge 复活。~~ **已解决**（2026-08-02，schema v58，[spec](2026-08-02-hotword-set-soft-delete.md)）：set 级 `is_deleted` 存删除时刻 epoch 秒（0=活跃，>0=删除时刻）+ `UNIQUE(name, is_deleted)` 复合约束，tombstone 经 merge 传播。`merge_set_soft_delete_propagates` 测试覆盖。**注**：词级软删（`hotword_words.is_deleted`）早已解决（commit `96560238`）。
- [ ] **多设备同时改 last-write-wins**：用户已确认可接受，不做冲突合并。

## 6. 代码位置速查（2026-08-01 状态；set 级软删 tombstone 见 [2026-08-02 spec](2026-08-02-hotword-set-soft-delete.md)）

| 位置 | 作用 |
|---|---|
| `crates/sync/src/hotword.rs::merge_hotwords` | merge 主体（3-way，对称 merge_vault） |
| `crates/sync/src/hotword.rs::HotwordMergeReport` | merge 结果报告 |
| `crates/sync/src/hotword.rs::pull_hotwords_from_files` | 旧 pull（保留，无方向，仅首次 clone + 测试用） |
| `crates/sync/src/hotword.rs::push_hotwords_to_files` | push（NoUpstream + enable_sync 用） |
| `crates/vault/src/sync/engine.rs::sync_now` | 调用 merge_hotwords（line ~745） |
| `crates/vault/src/sync/engine.rs::merge_vault` | vault merge（对称参考，line ~1021） |
| `crates/infra/src/db/hotword.rs::delete_hotword_set` | set 级软删（v58，is_deleted=时间戳） |
| `crates/infra/src/db/hotword.rs::list_all_hotword_sets` | 含 tombstone（sync export 用，v58 新增） |
