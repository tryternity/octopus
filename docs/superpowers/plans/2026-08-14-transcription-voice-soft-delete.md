# 识别记录 voice 软删 实施计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan.

**Goal:** 设置页删 voice 走 clipboard voice-aware 分流（软删保语料），列表查询过滤 is_deleted。

**Spec:** `docs/superpowers/specs/2026-08-14-transcription-voice-soft-delete-design.md`

## Task 1: 列表查询加 is_deleted 过滤 + 删除路径改 clipboard::delete_items

**Files:**
- Modify: `crates/infra/src/db/transcription.rs`（3 处查询加 `AND is_deleted = 0`）
- Modify: `crates/desktop/src/core/db_queue.rs:134`（改 delete_items）
- Modify: `crates/desktop/src/commands/settings_commands.rs:583`（改 delete_items）

- [ ] **Step 1: infra transcription.rs — 3 处查询加 is_deleted = 0**

`list_transcriptions_at` 基础查询：
```
WHERE item_type = 'voice'
```
→
```
WHERE item_type = 'voice' AND is_deleted = 0
```

FTS5 search：
```
WHERE c.item_type = 'voice'
```
→
```
WHERE c.item_type = 'voice' AND c.is_deleted = 0
```

LIKE search：
```
WHERE c.item_type = 'voice' AND c.content LIKE ?1
```
→
```
WHERE c.item_type = 'voice' AND c.is_deleted = 0 AND c.content LIKE ?1
```

- [ ] **Step 2: desktop db_queue.rs — 改 delete_items**

```rust
// 旧：
octopus_infra::db::delete_transcriptions(&[id.to_string()])
// 新：
octopus_infra::db::with_db(|conn| {
    octopus_clipboard::store::delete_items(conn, &[id.to_string()])
})
```

- [ ] **Step 3: desktop settings_commands.rs — 改 delete_items**

```rust
// 旧：
let deleted = octopus_infra::db::delete_transcriptions(&ids).map_err(e2s)?;
// 新：
let deleted = octopus_infra::db::with_db(|conn| {
    octopus_clipboard::store::delete_items(conn, &ids)
}).map_err(e2s)?;
```

- [ ] **Step 4: 编译 + 测试**

```bash
cargo build -p octopus-desktop --features "cloud,embedded,vault" 2>&1 | tail -5
cargo test -p octopus-infra --lib 2>&1 | grep "test result" | tail -2
cargo test -p octopus-desktop --features "cloud,embedded,vault" 2>&1 | grep "test result" | tail -2
```

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "fix: 设置页删 voice 走 clipboard voice-aware 分流（软删保语料）+ 列表过滤 is_deleted"
```
