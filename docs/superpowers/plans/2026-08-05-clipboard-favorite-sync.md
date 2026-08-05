# 剪贴板收藏同步实施计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 剪贴板 favorite 跨设备同步——仅文本类（text/voice/ocr），AES-256-GCM 加密内容走 git sync。

**Architecture:** clipboard_history.id 从 INTEGER(毫秒戳) 改 TEXT(UUID v4) 作跨设备同步锚点；新增 clipboard_favorites 表（**4 字段极简**：`history_id` PK + `is_deleted` + `updated_at` + `sync_md5`，无独立 id / 无 FK——详见 spec §2.2 + §12.1）；`.sync/clipboard/` 目录走 hotword 同款 outline + 分片文件 pattern；clipboard.key（32B AES key，明文存 git 先防君子）加解密 sync 文件内容（**内联 `ClipboardKey`，非 vault `DerivedKey`——循环依赖，详见 spec §6.3**）；merge 逻辑套 hotword tombstone 单向优先 fix（052c67cc/7da97a01 同款）。

**Tech Stack:** Rust + SQLite + rusqlite + octopus-sync（git wrapper + outline + store）+ **内联 AES-256-GCM（`aes-gcm` crate，`ClipboardKey`——非 vault `DerivedKey`，因循环依赖，详见 spec §6.3）** + uuid crate

**Spec:** `docs/superpowers/specs/2026-08-05-clipboard-favorite-sync-design.md`

## Global Constraints

- **清表重建**：`clipboard_history` 老数据不迁移，升 schema 版本后清表重建（用户已确认）
- **仅文本类同步**：text/voice/ocr 收藏可同步；image/file 类型的 is_favorite 本机可用但 sync 时跳过
- **casing**：sync 文件 JSON 字段 camelCase（`isDeleted` / `updatedMs` / `encryptedPayload` 等），对齐 vault sync 2026-07-28 casing 统一
- **CURRENT_SCHEMA_VERSION**：58 → 59（clipboard_history id 类型变更 + favorites 表新增，破坏性，老库 `bail!` 提示清库）
- **cargo test**：每个 task 完成后跑相关 crate 的 `cargo test -p <crate> --lib`，全绿才进下一 task
- **uuid 依赖**：`uuid = { version = "1", features = ["v4"] }`（infra 已有，确认即可）
- **DerivedKey 构造**：~~`octopus_vault::crypto::DerivedKey::from_raw([u8; 32])`~~ → 实际改为内联 `ClipboardKey`（`crates/sync/src/clipboard.rs`，对称 vault `DerivedKey` 但因循环依赖不依赖 vault；详见 spec §6.3）：`ClipboardKey::from_raw([u8; 32])` → `.encrypt(&bytes)` / `.decrypt(&str)`
- **tombstone 单向优先不变量**：merge 阶段 1 的 `Some(db_f)` 分支，timestamp 比较前先检查 `remote_is_tombstone`，true 则 pull 不 push（对称 052c67cc/7da97a01）

---

## 文件结构

### 新建文件

| 文件 | 职责 |
|---|---|
| `crates/sync/src/clipboard.rs` | clipboard favorite sync（outline / 文件读写 / merge），对称 `crates/sync/src/hotword.rs` |
| `crates/infra/src/db/clipboard_favorite.rs` | `clipboard_favorites` 表 CRUD（对称 `db/hotword.rs`） |
| `crates/clipboard/src/favorite.rs` | favorite 业务逻辑（收藏 / 取消 / 列表查询） |

### 改动文件

| 文件 | 改动 |
|---|---|
| `crates/infra/resources/sql/schema.sql` | clipboard_history id TEXT + favorites 表 + FTS5 trigger 适配 |
| `crates/infra/src/db/mod.rs` | 模块声明加 clipboard_favorite + CURRENT_SCHEMA_VERSION 58→59 |
| `crates/infra/src/db/transcription.rs` | INSERT 用 UUID；ORDER BY created_at DESC；FTS5 rowid |
| `crates/clipboard/src/store.rs` | NewClipboardItem.id i64→String；insert_with_unique_id 改 UUID |
| `crates/clipboard/src/lib.rs` | pub mod favorite |
| `crates/sync/src/lib.rs` | pub mod clipboard |
| `crates/sync/Cargo.toml` | 加 uuid 依赖 |
| `crates/vault/src/sync/engine.rs` | sync_now 加 merge_clipboard_favorites 调用 |

---

## Task 1: schema 变更——clipboard_history id UUID + favorites 表 + FTS5 适配

> **实现合并**：Task 3（clipboard_favorites 表 CRUD）实际与本 Task 同 commit 实现——schema 与 CRUD 强耦合，分两次 commit 反复编译。Task 1+3 合并为单 commit。

**Files:**
- Modify: `crates/infra/resources/sql/schema.sql`
- Modify: `crates/infra/src/db/mod.rs`（CURRENT_SCHEMA_VERSION 58→59 + `pub mod clipboard_favorite`）

**Interfaces:**
- Produces: 新 schema（clipboard_history.id TEXT + clipboard_favorites 表 + FTS5 trigger 用 NEW.rowid）供后续所有 task 依赖

- [x] **Step 1: 改 schema.sql——clipboard_history.id INTEGER → TEXT**

`crates/infra/resources/sql/schema.sql` 第 66-82 行，`id` 列改类型 + 注释：

```sql
CREATE TABLE IF NOT EXISTS clipboard_history (
    id              TEXT PRIMARY KEY,           -- UUID v4（原 INTEGER 毫秒戳；2026-08-05 改 UUID 作跨设备 sync 锚点）
    item_type       TEXT    NOT NULL,          -- 'text' | 'voice' | 'ocr' | 'image' | 'file'
    -- ... 其余列不变
```

- [x] **Step 2: 改 FTS5 trigger——NEW.id → NEW.rowid**

schema.sql 第 109-120 行，3 个 trigger 的 `VALUES (..., new.id, ...)` 改为 `new.rowid`：

```sql
CREATE TRIGGER IF NOT EXISTS clip_fts_ai AFTER INSERT ON clipboard_history BEGIN
    INSERT INTO clipboard_history_fts(rowid, content) VALUES (new.rowid, new.content);
END;
CREATE TRIGGER IF NOT EXISTS clip_fts_ad AFTER DELETE ON clipboard_history BEGIN
    INSERT INTO clipboard_history_fts(clipboard_history_fts, rowid, content)
    VALUES ('delete', old.rowid, old.content);
END;
CREATE TRIGGER IF NOT EXISTS clip_fts_au AFTER UPDATE OF content ON clipboard_history BEGIN
    INSERT INTO clipboard_history_fts(clipboard_history_fts, rowid, content)
    VALUES ('delete', old.rowid, old.content);
    INSERT INTO clipboard_history_fts(rowid, content) VALUES (new.rowid, new.content);
END;
```

- [x] **Step 3: 加 clipboard_favorites 表**

schema.sql 在 clipboard_history 段之后加：

```sql
-- ── 剪贴板收藏（clipboard_favorites）──────────────────────────────────────────
-- 仅文本类（text/voice/ocr）的 favorite 才 sync；image/file 的 is_favorite 本机可用但不进此表。
CREATE TABLE IF NOT EXISTS clipboard_favorites (
    id              TEXT PRIMARY KEY,           -- UUID v4（同步锚点，跨设备稳定）
    history_id      TEXT NOT NULL,              -- 指向 clipboard_history.id（UUID，跨设备一致）
    is_deleted      INTEGER NOT NULL DEFAULT 0, -- 0=活跃，>0=删除时刻 epoch 秒（tombstone）
    created_at      TEXT NOT NULL,
    updated_at      TEXT NOT NULL,
    sync_md5        TEXT,
    UNIQUE(history_id, is_deleted),
    FOREIGN KEY (history_id) REFERENCES clipboard_history(id)
);
CREATE INDEX IF NOT EXISTS idx_clip_fav_active ON clipboard_favorites(is_deleted) WHERE is_deleted = 0;
```

- [x] **Step 4: 改 CURRENT_SCHEMA_VERSION 58 → 59**

`crates/infra/src/db/mod.rs` 第 441 行：

```rust
pub const CURRENT_SCHEMA_VERSION: u32 = 59;
```

- [x] **Step 5: 加 clipboard_favorite 模块声明**

`crates/infra/src/db/mod.rs` 第 6-13 行的 mod 块加：

```rust
mod clipboard_favorite;
```

并在公开导出处（`pub use` 或 `pub mod`）加 `pub mod clipboard_favorite;`（对齐 hotword 的导出方式）。

- [x] **Step 6: 编译验证**

```bash
cargo build -p octopus-infra 2>&1 | tail -5
```
Expected: 0 error（clipboard_favorite 模块此时还不存在，先建空文件）

- [x] **Step 7: Commit**

```bash
git add -A && git commit -m "feat(clipboard-sync): schema 变更——clipboard_history id UUID + favorites 表 + FTS5 适配

- clipboard_history.id INTEGER(毫秒戳) → TEXT(UUID v4)
- FTS5 trigger: NEW.id → NEW.rowid（id 变 TEXT 后不能作 FTS5 rowid）
- 新增 clipboard_favorites 表（4 字段极简：history_id PK + is_deleted + updated_at + sync_md5，无 FK）
- CURRENT_SCHEMA_VERSION 58 → 59（破坏性，老库 bail 提示清库）"
```

> **实现偏差**：实际 commit 把 schema（Task 1）+ favorites 表 CRUD（Task 3）合并为单 commit。favorites 表从原设计的 6 字段简化到 4 字段（无独立 id / 无 created_at / 无 UNIQUE / 无 FK）——详见 spec §2.2 + §12.1。

---

## Task 2: clipboard_history UUID 主键适配（store.rs + transcription.rs）

**Files:**
- Modify: `crates/clipboard/src/store.rs`（NewClipboardItem.id i64→String + insert_clipboard_item + insert_with_unique_id）
- Modify: `crates/infra/src/db/transcription.rs`（INSERT/ORDER BY/FTS5 适配）
- Modify: `crates/clipboard/src/watcher.rs`（传 UUID）

**Interfaces:**
- Consumes: Task 1 的新 schema
- Produces: `NewClipboardItem.id: String`（UUID v4），所有 clipboard_history INSERT 走 UUID；`insert_clipboard_item` 返回 `Result<String>`

- [x] **Step 1: 写失败测试——insert_clipboard_item 用 UUID**

`crates/clipboard/src/store.rs` test mod 里加测试：

```rust
#[test]
fn insert_clipboard_item_generates_uuid_id() {
    use crate::model::ItemType;
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    // 建最小 schema（clipboard_history 表 + FTS5 trigger）
    conn.execute_batch(include_str!("../../infra/resources/sql/schema.sql")).unwrap();
    // 此处先只跑 clipboard_history 段——如果 schema.sql 含其他表建不了，用单独 SQL

    let item = NewClipboardItem {
        id: uuid::Uuid::new_v4().to_string(),
        item_type: ItemType::Text,
        content: "hello".into(),
        ref_data: None,
        meta_info: None,
        created_at: "2026-08-05T10:00:00".into(),
        has_thumbnail: None,
        is_rich: false,
    };
    let id = insert_clipboard_item(&conn, &item).unwrap();
    assert!(uuid::Uuid::parse_str(&id).is_ok(), "id 应是合法 UUID v4");
}
```

- [x] **Step 2: 跑测试确认失败**

```bash
cargo test -p octopus-clipboard --lib insert_clipboard_item_generates_uuid_id 2>&1 | tail -10
```
Expected: FAIL（NewClipboardItem.id 还是 i64，类型不匹配）

- [x] **Step 3: 改 NewClipboardItem.id i64 → String**

`crates/clipboard/src/store.rs` 第 539-549 行：

```rust
pub struct NewClipboardItem {
    pub id: String,               // 原 i64（毫秒戳）→ String（UUID v4）
    pub item_type: ItemType,
    // ... 其余字段不变
}
```

- [x] **Step 4: 改 insert_clipboard_item 返回 String + SQL 参数**

`crates/clipboard/src/store.rs` 第 8-37 行，签名 + SQL：

```rust
pub fn insert_clipboard_item(conn: &Connection, item: &NewClipboardItem) -> Result<String> {
    // ... meta_json 等不变
    conn.execute(
        "INSERT INTO clipboard_history (id, item_type, ...) VALUES (?, ?, ...)",
        params![
            item.id,    // 现在是 String（UUID）
            // ... 其余不变
        ],
    )?;
    Ok(item.id.clone())   // 原 Ok(item.id)
}
```

三个 INSERT 分支（Text/Image/File）都改。

- [x] **Step 5: 改 insert_with_unique_id + insert_asr_item + insert_ocr_item**

`crates/clipboard/src/store.rs` 第 557-572 行 `insert_with_unique_id`：改为 UUID 生成：

```rust
fn insert_with_unique_id<F>(mut insert_fn: F) -> Result<String>
where F: FnMut(&str) -> rusqlite::Result<usize>,
{
    loop {
        let id = uuid::Uuid::new_v4().to_string();
        match insert_fn(&id) {
            Ok(_) => return Ok(id),
            Err(rusqlite::Error::SqliteFailure(err, _)) if err.code == rusqlite::ErrorCode::ConstraintViolation => {
                continue;  // UUID 冲突概率极低，直接重试
            }
            Err(e) => return Err(e.into()),
        }
    }
}
```

`insert_asr_item` / `insert_ocr_item` 的闭包参数从 `|id|` (i64) 改 `|id: &str|`，SQL `?` 参数对应改。

- [x] **Step 6: 改 transcription.rs ORDER BY + INSERT**

`crates/infra/src/db/transcription.rs`：
- 第 220、230、280 行 `ORDER BY id DESC` → `ORDER BY created_at DESC, id DESC`
- 第 11 行 `insert_transcription_at_id`：参数 `id: i64` → `id: &str`，SQL 参数对应改
- 第 318、355、361、387、409、426、442、449、482、568 行所有 INSERT 语句的 id 参数改 TEXT

- [x] **Step 7: 改 watcher.rs 调用点**

`crates/clipboard/src/watcher.rs` 第 111、169、209 行 `insert_clipboard_item` 调用，`id` 字段传 `uuid::Uuid::new_v4().to_string()`。

- [x] **Step 8: 跑测试**

```bash
cargo test -p octopus-clipboard --lib 2>&1 | tail -5
cargo test -p octopus-infra --lib transcription 2>&1 | tail -5
```
Expected: 全绿

- [x] **Step 9: Commit**

```bash
git add -A && git commit -m "feat(clipboard-sync): clipboard_history id INTEGER→TEXT(UUID v4) 适配

- NewClipboardItem.id: i64 → String（UUID v4）
- insert_clipboard_item 返回 String
- insert_with_unique_id 改 UUID 生成
- transcription.rs ORDER BY id → ORDER BY created_at DESC, id DESC
- watcher.rs 调用点传 UUID"
```

---

## Task 3: clipboard_favorites 表 CRUD（db/clipboard_favorite.rs）

**Files:**
- Create: `crates/infra/src/db/clipboard_favorite.rs`
- Modify: `crates/infra/src/db/mod.rs`（确认 pub 导出）

**Interfaces:**
- Consumes: Task 1 的 schema
- Produces: `ClipboardFavorite` struct + `insert_favorite` / `soft_delete_favorite` / `list_favorites` / `get_favorite` / `upsert_favorite_sync` / `list_all_favorites`（含 tombstone）

- [x] **Step 1: 写失败测试——insert + list + soft_delete**

`crates/infra/src/db/clipboard_favorite.rs` 文件底部 `#[cfg(test)] mod tests`：

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::open_init;

    fn setup() -> rusqlite::Connection {
        let conn = open_init().unwrap();
        conn
    }

    #[test]
    fn insert_and_list_favorite() {
        let conn = setup();
        // 先 insert 一条 clipboard_history 行（FK 依赖）
        conn.execute(
            "INSERT INTO clipboard_history (id, item_type, content, created_at) VALUES (?1, 'text', 'hello', '2026-08-05')",
            ["hist-uuid-1"],
        ).unwrap();
        insert_favorite_at(&conn, "fav-uuid-1", "hist-uuid-1", 0).unwrap();
        let favs = list_active_favorites_at(&conn).unwrap();
        assert_eq!(favs.len(), 1);
        assert_eq!(favs[0].id, "fav-uuid-1");
        assert_eq!(favs[0].history_id, "hist-uuid-1");
    }

    #[test]
    fn soft_delete_and_tombstone() {
        let conn = setup();
        conn.execute(
            "INSERT INTO clipboard_history (id, item_type, content, created_at) VALUES (?1, 'text', 'hello', '2026-08-05')",
            ["hist-uuid-2"],
        ).unwrap();
        insert_favorite_at(&conn, "fav-uuid-2", "hist-uuid-2", 0).unwrap();
        soft_delete_favorite_at(&conn, "fav-uuid-2", 1722835200).unwrap();
        // list_active 不含 tombstone
        let active = list_active_favorites_at(&conn).unwrap();
        assert!(active.iter().all(|f| f.id != "fav-uuid-2"));
        // list_all 含 tombstone
        let all = list_all_favorites_at(&conn).unwrap();
        assert!(all.iter().any(|f| f.id == "fav-uuid-2" && f.is_deleted > 0));
    }
}
```

- [x] **Step 2: 跑测试确认失败（模块不存在）**

```bash
cargo test -p octopus-infra --lib clipboard_favorite 2>&1 | tail -5
```
Expected: FAIL（模块 / 函数不存在）

- [x] **Step 3: 实现 ClipboardFavorite struct + CRUD**

`crates/infra/src/db/clipboard_favorite.rs`：

```rust
use anyhow::Result;
use rusqlite::{params, Connection};

#[derive(Debug, Clone)]
pub struct ClipboardFavorite {
    pub id: String,
    pub history_id: String,
    pub is_deleted: i64,
    pub created_at: String,
    pub updated_at: String,
    pub sync_md5: Option<String>,
}

const COLS: &str = "id, history_id, is_deleted, created_at, updated_at, sync_md5";

// ── _at 变体（接 &Connection，测试 + sync 用）──

pub(crate) fn insert_favorite_at(
    conn: &Connection,
    id: &str,
    history_id: &str,
    is_deleted: i64,
) -> Result<()> {
    let now = iso_now();
    conn.execute(
        "INSERT INTO clipboard_favorites (id, history_id, is_deleted, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?4)",
        params![id, history_id, is_deleted, now],
    )?;
    Ok(())
}

pub(crate) fn soft_delete_favorite_at(conn: &Connection, id: &str, epoch_secs: i64) -> Result<()> {
    conn.execute(
        "UPDATE clipboard_favorites SET is_deleted = ?1, updated_at = ?2 WHERE id = ?3",
        params![epoch_secs, iso_now(), id],
    )?;
    Ok(())
}

pub(crate) fn list_active_favorites_at(conn: &Connection) -> Result<Vec<ClipboardFavorite>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {COLS} FROM clipboard_favorites WHERE is_deleted = 0 ORDER BY created_at DESC"
    ))?;
    let rows = stmt.query_map([], |row| parse_favorite(row))?;
    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}

pub(crate) fn list_all_favorites_at(conn: &Connection) -> Result<Vec<ClipboardFavorite>> {
    // 含 tombstone——sync merge 需感知软删态
    let mut stmt = conn.prepare(&format!("SELECT {COLS} FROM clipboard_favorites"))?;
    let rows = stmt.query_map([], |row| parse_favorite(row))?;
    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}

fn parse_favorite(row: &rusqlite::Row) -> rusqlite::Result<ClipboardFavorite> {
    Ok(ClipboardFavorite {
        id: row.get(0)?,
        history_id: row.get(1)?,
        is_deleted: row.get(2)?,
        created_at: row.get(3)?,
        updated_at: row.get(4)?,
        sync_md5: row.get(5)?,
    })
}

fn iso_now() -> String {
    chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string()
}
```

- [x] **Step 4: 加 pub 包装（走 ensure_db / with_db）**

在 `clipboard_favorite.rs` 底部加 pub 函数（对齐 hotword.rs 的 `insert_hotword_set` / `list_all_hotword_sets` 模式）：

```rust
use crate::db::{ensure_db, with_db};

pub fn insert_favorite(id: &str, history_id: &str) -> Result<()> {
    ensure_db()?;
    with_db(|conn| insert_favorite_at(conn, id, history_id, 0))
}

pub fn soft_delete_favorite(id: &str, epoch_secs: i64) -> Result<()> {
    ensure_db()?;
    with_db(|conn| soft_delete_favorite_at(conn, id, epoch_secs))
}

pub fn list_active_favorites() -> Result<Vec<ClipboardFavorite>> {
    ensure_db()?;
    with_db(list_active_favorites_at)
}

pub fn list_all_favorites() -> Result<Vec<ClipboardFavorite>> {
    ensure_db()?;
    with_db(list_all_favorites_at)
}

pub fn load_favorite(id: &str) -> Result<Option<ClipboardFavorite>> {
    ensure_db()?;
    with_db(|conn| {
        let mut stmt = conn.prepare(&format!("SELECT {COLS} FROM clipboard_favorites WHERE id = ?1"))?;
        let mut rows = stmt.query(params![id])?;
        match rows.next()? {
            Some(row) => Ok(Some(parse_favorite(row)?)),
            None => Ok(None),
        }
    })
}

/// sync upsert（含 is_deleted + sync_md5）——对称 hotword upsert_hotword_set
pub fn upsert_favorite_sync(fav: &ClipboardFavorite) -> Result<()> {
    ensure_db()?;
    with_db(|conn| {
        let existing: Option<i64> = conn
            .query_row("SELECT 1 FROM clipboard_favorites WHERE id = ?1", params![fav.id], |r| r.get(0))
            .ok();
        if existing.is_some() {
            conn.execute(
                "UPDATE clipboard_favorites SET history_id=?1, is_deleted=?2, sync_md5=?3, updated_at=?4 WHERE id=?5",
                params![fav.history_id, fav.is_deleted, fav.sync_md5, fav.updated_at, fav.id],
            )?;
        } else {
            conn.execute(
                "INSERT INTO clipboard_favorites (id, history_id, is_deleted, created_at, updated_at, sync_md5)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![fav.id, fav.history_id, fav.is_deleted, fav.created_at, fav.updated_at, fav.sync_md5],
            )?;
        }
        Ok(())
    })
}
```

- [x] **Step 5: 跑测试确认通过**

```bash
cargo test -p octopus-infra --lib clipboard_favorite 2>&1 | tail -5
```
Expected: 全绿

- [x] **Step 6: Commit**

```bash
git add -A && git commit -m "feat(clipboard-sync): clipboard_favorites 表 CRUD

- ClipboardFavorite struct + _at 变体（测试用）+ pub 包装（ensure_db/with_db）
- insert / soft_delete / list_active / list_all / load / upsert_sync
- 对称 hotword.rs 的 CRUD pattern"
```

---

## Task 4: clipboard.key 加解密模块

> **实现合并**：Task 5（sync 文件读写）合并到本 Task——加密原语 + 文件格式 + outline 一起在 `crates/sync/src/clipboard.rs` 实现，不可分。最终 Task 4+5 同 commit。

**Files:**
- Create: `crates/clipboard/src/crypto.rs`（或 `crates/sync/src/clipboard_crypto.rs`）
- Modify: `crates/sync/Cargo.toml`（加 uuid 依赖 + octopus-vault 依赖）

**Interfaces:**
- Produces: `load_or_create_clipboard_key() -> Result<DerivedKey>` / `encrypt_payload(key, json) -> Result<String>` / `decrypt_payload(key, ciphertext) -> Result<String>`

- [x] **Step 1: 写失败测试——encrypt/decrypt round-trip**

`crates/sync/src/clipboard.rs`（新文件）test mod：

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encrypt_decrypt_roundtrip() {
        let key = DerivedKey::from_raw([42u8; 32]);
        let plaintext = r#"{"content":"hello"}"#;
        let encrypted = key.encrypt(plaintext.as_bytes()).unwrap();
        assert!(encrypted.starts_with("v1:"));
        let decrypted = key.decrypt(&encrypted).unwrap();
        assert_eq!(String::from_utf8_lossy(&decrypted), plaintext);
    }
}
```

- [x] **Step 2: 跑测试确认失败（模块不存在）**

- [x] **Step 3: 实现 clipboard.key 加解密**

`crates/sync/src/clipboard.rs`（初期只放 crypto + key 管理，merge 逻辑后续 task 加）：

```rust
//! 剪贴板收藏同步——`.sync/clipboard/` outline + 加密文件（对称 hotword.rs）。
use anyhow::{Context, Result};
use std::path::PathBuf;
use octopus_vault::crypto::DerivedKey;

/// clipboard.key 文件路径
fn clipboard_key_path() -> Result<PathBuf> {
    Ok(octopus_sync::store::sync_root().join("clipboard").join("clipboard.key"))
}

/// 加载或创建 clipboard.key（32B AES-256-GCM key）。
/// 不存在时生成随机 32B → hex 写文件 0600。
pub fn load_or_create_clipboard_key() -> Result<DerivedKey> {
    let path = clipboard_key_path()?;
    if let Ok(hex_str) = std::fs::read_to_string(&path) {
        let bytes = hex::decode(hex_str.trim()).context("clipboard.key hex 解码失败")?;
        let mut arr = [0u8; 32];
        arr.copy_from_slice(&bytes);
        return Ok(DerivedKey::from_raw(arr));
    }
    // 不存在 → 生成
    let key_bytes = octopus_vault::crypto::util::random_bytes(32);
    let hex_str = hex::encode(&key_bytes);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&path, &hex_str)?;
    set_readonly(&path)?;
    let mut arr = [0u8; 32];
    arr.copy_from_slice(&key_bytes);
    Ok(DerivedKey::from_raw(arr))
}

#[cfg(unix)]
fn set_readonly(path: &PathBuf) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    Ok(())
}
#[cfg(not(unix))]
fn set_readonly(_path: &PathBuf) -> Result<()> { Ok(()) }
```

- [x] **Step 4: 加 Cargo.toml 依赖**

`crates/sync/Cargo.toml`：
```toml
uuid = { version = "1", features = ["v4"] }
hex = "0.4"
octopus-vault = { path = "../vault" }
```

- [x] **Step 5: 跑测试**

```bash
cargo test -p octopus-sync --lib clipboard::tests::encrypt_decrypt 2>&1 | tail -5
```
Expected: 全绿

- [x] **Step 6: Commit**

---

## Task 5: sync 文件读写（outline + favorites/<2hex>/<uuid>.json）

**Files:**
- Modify: `crates/sync/src/clipboard.rs`

**Interfaces:**
- Produces: `ClipboardFavoriteFile` struct + `read_clipboard_outline` / `write_clipboard_outline` / `read_favorite_file` / `write_favorite_file` / `export_all_favorites` / `merge_clipboard_favorites`

- [x] **Step 1: 定义 sync 文件结构 + outline 结构**

```rust
use serde::{Serialize, Deserialize};
use std::collections::BTreeMap;
use octopus_sync::outline::OutlineEntry;

/// outline.json 结构
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ClipboardOutline {
    pub version: u32,
    pub favorites: BTreeMap<String, OutlineEntry>,
}

/// favorites/<2hex>/<uuid>.json 文件结构
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClipboardFavoriteFile {
    pub version: u32,
    pub id: String,
    pub is_deleted: i64,              // 0=活跃，>0=tombstone epoch
    pub encrypted_payload: String,    // v1: 加密的 FavoritePayload JSON
    pub created_at: String,
    pub updated_at: String,
}

/// encrypted_payload 解密后的内容
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FavoritePayload {
    pub history_row: HistoryRowJson,
    pub favorite_id: String,
    pub content_hash: String,
}

/// clipboard_history 行的 sync 序列化（脱 DB 行映射）
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryRowJson {
    pub id: String,
    pub item_type: String,
    pub content: String,
    pub ref_data: Option<String>,
    pub meta_info: Option<String>,
    pub is_rich: bool,
    pub created_at: String,
    pub segments: Option<String>,
}
```

- [x] **Step 2: 实现 outline + 文件读写函数**

```rust
fn clipboard_dir() -> Result<PathBuf> {
    Ok(octopus_sync::store::sync_root().join("clipboard"))
}
fn favorites_dir() -> Result<PathBuf> {
    Ok(clipboard_dir()?.join("favorites"))
}
fn outline_path() -> Result<PathBuf> {
    Ok(clipboard_dir()?.join("outline.json"))
}
fn favorite_file_path(uuid: &str) -> Result<PathBuf> {
    let dir = favorites_dir()?;
    let shard = octopus_sync::store::shard_subdir(uuid);  // <2hex> 桶
    Ok(dir.join(shard).join(format!("{uuid}.json")))
}

pub fn read_clipboard_outline() -> Result<ClipboardOutline> {
    let path = outline_path()?;
    if !path.exists() { return Ok(ClipboardOutline { version: 1, favorites: BTreeMap::new() }); }
    let json = std::fs::read_to_string(&path)?;
    Ok(serde_json::from_str(&json).unwrap_or_default())
}

pub fn write_clipboard_outline(outline: &ClipboardOutline) -> Result<()> {
    let path = outline_path()?;
    if let Some(p) = path.parent() { std::fs::create_dir_all(p)?; }
    let json = serde_json::to_string_pretty(outline)?;
    std::fs::write(path, json)?;
    Ok(())
}

pub fn read_favorite_file(uuid: &str) -> Result<ClipboardFavoriteFile> {
    let path = favorite_file_path(uuid)?;
    let json = std::fs::read_to_string(&path)?;
    Ok(serde_json::from_str(&json)?)
}

pub fn write_favorite_file(file: &ClipboardFavoriteFile) -> Result<()> {
    let path = favorite_file_path(&file.id)?;
    if let Some(p) = path.parent() { std::fs::create_dir_all(p)?; }
    let json = serde_json::to_string(file)?;
    std::fs::write(path, json)?;
    Ok(())
}

pub fn delete_favorite_file(uuid: &str) -> Result<()> {
    let path = favorite_file_path(uuid)?;
    let _ = std::fs::remove_file(path);
    Ok(())
}
```

- [x] **Step 3: 写测试——write + read round-trip**

- [x] **Step 4: 跑测试**

- [x] **Step 5: Commit**

---

## Task 6: export_all_favorites（DB → .sync 文件）

> **实现合并**：Task 6（export）+ Task 7（merge）+ Task 8（sync_now 接入）三 Task 同 commit 实现——merge 必然依赖 export（末尾调用 export_all_favorites 重建文件 + outline），sync_now 接入只 1 行代码改动，三 Task 一起编译一起测。

**Files:**
- Modify: `crates/sync/src/clipboard.rs`

**Interfaces:**
- Produces: `export_all_favorites() -> Result<ClipboardOutline>`

- [x] **Step 1: 实现 export**

```rust
pub fn export_all_favorites() -> Result<ClipboardOutline> {
    let key = load_or_create_clipboard_key()?;
    let favs = octopus_infra::db::clipboard_favorite::list_all_favorites()?;
    // 清空 favorites/ 目录后重建
    let fav_dir = favorites_dir()?;
    if fav_dir.is_dir() { let _ = std::fs::remove_dir_all(&fav_dir); }

    let mut outline = ClipboardOutline { version: 1, favorites: BTreeMap::new() };
    for fav in &favs {
        // JOIN clipboard_history 取行（需在 db 层加查询函数）
        let hist = octopus_infra::db::load_clipboard_history_row(&fav.history_id)?;
        let hist = match hist { Some(h) => h, None => continue }; // 孤行跳过

        let payload = FavoritePayload {
            history_row: HistoryRowJson::from_db(&hist),
            favorite_id: fav.id.clone(),
            content_hash: md5_hex(hist.content.as_bytes()),
        };
        let payload_json = serde_json::to_string(&payload)?;
        let encrypted = key.encrypt(payload_json.as_bytes())?;

        let file = ClipboardFavoriteFile {
            version: 1,
            id: fav.id.clone(),
            is_deleted: fav.is_deleted,
            encrypted_payload: encrypted,
            created_at: fav.created_at.clone(),
            updated_at: fav.updated_at.clone(),
        };
        write_favorite_file(&file)?;

        let md5 = favorite_md5(fav);
        outline.favorites.insert(fav.id.clone(), OutlineEntry {
            md5,
            updated_ms: octopus_sync::store::iso_to_unix_ms(&fav.updated_at),
        });
    }
    write_clipboard_outline(&outline)?;
    Ok(outline)
}
```

- [x] **Step 2: 写测试——export 后文件存在 + outline 正确**

- [x] **Step 3: 跑测试**

- [x] **Step 4: Commit**

---

## Task 7: merge_clipboard_favorites（双向同步 + tombstone 单向优先 fix）

**Files:**
- Modify: `crates/sync/src/clipboard.rs`

**Interfaces:**
- Consumes: Task 4-6 的所有函数
- Produces: `merge_clipboard_favorites() -> Result<ClipboardMergeReport>`（对称 `merge_hotwords`）

- [x] **Step 1: 实现 merge（套 hotword pattern + tombstone 单向优先）**

```rust
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClipboardMergeReport {
    pub pulled: usize,
    pub pushed: usize,
    pub conflicts: usize,
    pub skipped: usize,
}

pub fn merge_clipboard_favorites() -> Result<ClipboardMergeReport> {
    let key = load_or_create_clipboard_key()?;
    let remote_outline = read_clipboard_outline()?;
    let db_favs = octopus_infra::db::clipboard_favorite::list_all_favorites()?;
    let db_by_id: HashMap<&str, &ClipboardFavorite> = db_favs.iter().map(|f| (f.id.as_str(), f)).collect();
    let mut report = ClipboardMergeReport::default();

    for (uuid, entry) in &remote_outline.favorites {
        let remote_updated = entry.updated_ms;
        match db_by_id.get(uuid.as_str()) {
            None => {
                // DB 无 → pull
                match pull_favorite(uuid, &key) {
                    Ok(true) => report.pulled += 1,
                    Ok(false) => report.skipped += 1,
                    Err(e) => { log::warn!("[sync] clipboard fav {} pull 失败：{}", uuid, e); report.skipped += 1; }
                }
            }
            Some(db_f) => {
                let local_updated = octopus_sync::store::iso_to_unix_ms(&db_f.updated_at);
                // 🔴 tombstone 单向优先（对称 052c67cc/7da97a01 fix）
                let remote_is_tombstone = read_favorite_file(uuid)
                    .map(|f| f.is_deleted > 0)
                    .unwrap_or(false);
                if remote_is_tombstone {
                    match pull_favorite(uuid, &key) {
                        Ok(true) => report.pulled += 1,
                        Ok(false) => report.skipped += 1,
                        Err(e) => { log::warn!("[sync] clipboard fav {} tombstone pull：{}", uuid, e); report.skipped += 1; }
                    }
                } else if remote_updated > local_updated {
                    match pull_favorite(uuid, &key) { /* ... */ }
                } else if local_updated > remote_updated {
                    push_favorite(db_f, &key)?;
                    report.pushed += 1;
                } else {
                    // 时间戳相等 → md5 比对
                    let db_md5 = db_f.sync_md5.clone().unwrap_or_else(|| favorite_md5(db_f));
                    if db_md5 != entry.md5 { push_favorite(db_f, &key)?; report.pushed += 1; report.conflicts += 1; }
                }
            }
        }
    }
    // DB 有 + outline 无 → push
    for db_f in &db_favs {
        if !remote_outline.favorites.contains_key(&db_f.id) {
            push_favorite(db_f, &key)?;
            report.pushed += 1;
        }
    }
    // 重建 outline
    export_all_favorites()?;
    Ok(report)
}

fn pull_favorite(uuid: &str, key: &DerivedKey) -> Result<bool> {
    let file = read_favorite_file(uuid)?;
    // tombstone 传播
    if file.is_deleted > 0 {
        if let Some(existing) = octopus_infra::db::clipboard_favorite::load_favorite(uuid)? {
            // 已存在 → UPDATE is_deleted（如果还没被标记）
            if existing.is_deleted == 0 {
                octopus_infra::db::clipboard_favorite::soft_delete_favorite(uuid, file.is_deleted)?;
            }
        } else {
            // 不存在 → 但要 INSERT。tombstone 关联的 history 可能不在 DB——
            // 解密 payload 拿 history_row，先 UPSERT history 再 INSERT favorite tombstone
            let payload = decrypt_payload(&file.encrypted_payload, key)?;
            upsert_history_row(&payload.history_row)?;
            let fav = ClipboardFavorite {
                id: uuid.to_string(),
                history_id: payload.history_row.id.clone(),
                is_deleted: file.is_deleted,
                created_at: file.created_at.clone(),
                updated_at: file.updated_at.clone(),
                sync_md5: None,
            };
            octopus_infra::db::clipboard_favorite::upsert_favorite_sync(&fav)?;
        }
        return Ok(true);
    }
    // active → 解密 + UPSERT history + UPSERT favorite
    let payload = decrypt_payload(&file.encrypted_payload, key)?;
    upsert_history_row(&payload.history_row)?;
    let fav = ClipboardFavorite {
        id: payload.favorite_id.clone(),
        history_id: payload.history_row.id.clone(),
        is_deleted: 0,
        created_at: file.created_at.clone(),
        updated_at: file.updated_at.clone(),
        sync_md5: None,
    };
    octopus_infra::db::clipboard_favorite::upsert_favorite_sync(&fav)?;
    Ok(true)
}
```

- [x] **Step 2: 实现 push_favorite + decrypt_payload + upsert_history_row + favorite_md5 辅助**

```rust
fn push_favorite(fav: &ClipboardFavorite, key: &DerivedKey) -> Result<()> {
    let hist = octopus_infra::db::load_clipboard_history_row(&fav.history_id)?
        .context("push: favorite 关联 history 不存在")?;
    let payload = FavoritePayload {
        history_row: HistoryRowJson::from_db(&hist),
        favorite_id: fav.id.clone(),
        content_hash: md5_hex(hist.content.as_bytes()),
    };
    let json = serde_json::to_string(&payload)?;
    let encrypted = key.encrypt(json.as_bytes())?;
    let file = ClipboardFavoriteFile {
        version: 1, id: fav.id.clone(), is_deleted: fav.is_deleted,
        encrypted_payload: encrypted,
        created_at: fav.created_at.clone(), updated_at: fav.updated_at.clone(),
    };
    write_favorite_file(&file)
}

fn decrypt_payload(encrypted: &str, key: &DerivedKey) -> Result<FavoritePayload> {
    let bytes = key.decrypt(encrypted)?;
    Ok(serde_json::from_slice(&bytes)?)
}

fn upsert_history_row(row: &HistoryRowJson) -> Result<()> {
    octopus_infra::db::upsert_clipboard_history_sync(row)
}

fn favorite_md5(fav: &ClipboardFavorite) -> String {
    // 身份 md5——只含 id + history_id（状态变化靠 updated_at 比较）
    octopus_sync::store::md5_hex(format!("{}|{}", fav.id, fav.history_id).as_bytes())
}
```

- [x] **Step 3: 写测试——tombstone 单向优先（对称 hotword/vault fix）**

```rust
#[test]
fn merge_remote_tombstone_not_overwritten_by_local_active_newer() {
    // 远程 tombstone + 本地 active updated_at 更新 → 不覆盖
    // 对称 hotword merge_remote_tombstone_not_overwritten_by_local_active_newer
}
```

- [x] **Step 4: 写测试——正常 pull / push / 冲突**

- [x] **Step 5: 跑全部 sync 测试**

```bash
cargo test -p octopus-sync --lib 2>&1 | tail -5
```
Expected: 全绿

- [x] **Step 6: Commit**

---

## Task 8: sync_now 接入（engine.rs）

**Files:**
- Modify: `crates/vault/src/sync/engine.rs`（sync_now 里加 merge_clipboard_favorites 调用 + SyncReport 加 clipboard 字段）

**Interfaces:**
- Consumes: Task 7 的 merge_clipboard_favorites

- [x] **Step 1: 在 sync_now 的 hotword merge 后加 clipboard merge**

`crates/vault/src/sync/engine.rs` 第 776 行附近（hotword merge 之后）：

```rust
// clipboard favorite merge（对称 vault + hotword merge）
let clipboard_report = match octopus_sync::clipboard::merge_clipboard_favorites() {
    Ok(r) => r,
    Err(e) => {
        log::warn!("[sync] clipboard favorite merge 失败（不阻断 vault/hotword 同步）：{}", e);
        octopus_sync::clipboard::ClipboardMergeReport::default()
    }
};
```

- [x] **Step 2: 加 SyncReport 字段（clipboard_pulled / clipboard_pushed）**

对齐 hotwords_pulled / hotwords_pushed 模式。

- [x] **Step 3: 编译 + 跑 vault 测试确保无回归**

```bash
cargo test -p octopus-vault --lib 2>&1 | tail -5
```
Expected: 全绿

- [x] **Step 4: Commit**

---

## Task 9: favorite 业务逻辑 + 前端命令（收藏/取消/列表）

**Files:**
- Create: `crates/clipboard/src/favorite.rs`
- Modify: `crates/clipboard/src/lib.rs`（pub mod favorite）
- Modify: `crates/desktop/src/clipboard/clipboard_commands.rs`（Tauri 命令）
- Modify: 前端 TS interface（id number → string + favorite 相关类型）

**Interfaces:**
- Produces: `toggle_favorite(history_id)` / `list_favorites() -> Vec<FavoriteView>` + Tauri 命令 + 前端类型

- [x] **Step 1: 实现 favorite.rs 业务逻辑**

```rust
// crates/clipboard/src/favorite.rs
use anyhow::Result;
use octopus_infra::db::{self, clipboard_favorite::ClipboardFavorite};

pub fn toggle_favorite(history_id: &str) -> Result<bool> {
    // 查是否已收藏（active）
    let existing = db::clipboard_favorite::load_favorite_by_history(history_id)?;
    if let Some(fav) = existing {
        if fav.is_deleted == 0 {
            // 已收藏 → 取消
            let epoch = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs() as i64).unwrap_or(0);
            db::clipboard_favorite::soft_delete_favorite(&fav.id, epoch)?;
            db::set_clipboard_favorite(history_id, false)?;
            Ok(false)
        } else {
            // tombstone → 恢复（重新 active）
            db::clipboard_favorite::restore_favorite(&fav.id)?;
            db::set_clipboard_favorite(history_id, true)?;
            Ok(true)
        }
    } else {
        // 新收藏
        let fav_id = uuid::Uuid::new_v4().to_string();
        db::clipboard_favorite::insert_favorite(&fav_id, history_id)?;
        db::set_clipboard_favorite(history_id, true)?;
        Ok(true)
    }
}

pub fn list_favorites() -> Result<Vec<(ClipboardFavorite, ClipboardHistoryRow)>> {
    // JOIN clipboard_history
    db::clipboard_favorite::list_active_favorites_join_history()
}
```

- [x] **Step 2: 在 db 层加缺的查询函数**

- `load_favorite_by_history(history_id) -> Result<Option<ClipboardFavorite>>`
- `restore_favorite(id) -> Result<()>`（is_deleted=0）
- `set_clipboard_favorite(history_id, bool) -> Result<()>`（UPDATE clipboard_history.is_favorite）
- `load_clipboard_history_row(id) -> Result<Option<ClipboardHistoryRow>>`
- `upsert_clipboard_history_sync(row: &HistoryRowJson) -> Result<()>`
- `list_active_favorites_join_history() -> Result<Vec<(ClipboardFavorite, ClipboardHistoryRow)>>`

- [x] **Step 3: 加 Tauri 命令**

`crates/desktop/src/clipboard/clipboard_commands.rs`：

```rust
#[tauri::command]
pub async fn toggle_clipboard_favorite(history_id: String) -> Result<bool, String> {
    octopus_clipboard::favorite::toggle_favorite(&history_id).map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn list_clipboard_favorites() -> Result<Vec<FavoriteView>, String> {
    octopus_clipboard::favorite::list_favorites()
        .map_err(|e| e.to_string())?
        .into_iter().map(|(fav, hist)| Ok(FavoriteView::from(fav, hist)))
        .collect()
}
```

- [x] **Step 4: 前端 TS——id 类型 + favorite 相关类型**

`ClipboardHistoryItem.id`: `number` → `string`
新增 `FavoriteView` interface + favorite 列表组件

- [x] **Step 5: 跑全量测试**

```bash
cargo test -p octopus-infra -p octopus-clipboard -p octopus-sync -p octopus-vault --lib 2>&1 | tail -10
```

- [x] **Step 6: Commit**

---

## Task 10: 清库 + 全量编译 + e2e 验证

- [x] **Step 1: 删除老 DB（清表重建）**

```bash
rm ~/.octopus/octopus.db*
```

- [x] **Step 2: cargo build 全量**

```bash
cargo build -p octopus-infra -p octopus-clipboard -p octopus-sync -p octopus-vault 2>&1 | tail -10
```
Expected: 0 error 0 warning

- [x] **Step 3: 全量 cargo test**

```bash
cargo test -p octopus-infra -p octopus-clipboard -p octopus-sync -p octopus-vault --lib 2>&1 | tail -10
```
Expected: 全绿

- [x] **Step 4: tsc + vite build**

```bash
cd crates/desktop/frontend && npx tsc --noEmit && npm run build
```

- [x] **Step 5: Commit**

---

## Spec Coverage

> 实现期间 Task 1+3 / Task 4+5 / Task 6+7+8 分别合并为单 commit。下表「实际 commit」列指最终落库的 commit 分组。

| Spec Section | 计划 Task | 实际 commit |
|---|---|---|
| §2 Schema 变更（含 §2.2 favorites 表简化） | Task 1 + Task 3 | Task 1+3 合并 |
| §3 .sync/clipboard/ 目录布局 | Task 5 | Task 4+5 合并 |
| §3.1 clipboard.key | Task 4 | Task 4+5 合并 |
| §3.2 outline.json | Task 5 | Task 4+5 合并 |
| §3.3 favorites/<2hex>/<uuid>.json（含 tombstone 空 payload） | Task 5 | Task 4+5 合并 |
| §4 数据流（收藏/取消/push/pull/展示/cleanup） | Task 9 + Task 6 + Task 7 | Task 6+7+8 合并 + Task 9 |
| §5 merge 逻辑 + tombstone 单向优先 | Task 7 | Task 6+7+8 合并 |
| §6 clipboard.key 管理（含内联 ClipboardKey） | Task 4 | Task 4+5 合并 |
| §7 crate 架构 + sync_now 接入 | Task 8 | Task 6+7+8 合并 |
| §8 影响面追踪（id UUID + FTS5） | Task 1 + Task 2 | Task 1+3 + Task 2 |
| §9 测试策略 | Task 3/5/6/7 内嵌 | 同 |
| §10 不变量 | Task 7 测试守护 | 同 |
| §12 实现记录 | — | 实现后回写到 spec §1-§11 |

---

## 实际实现偏差（2026-08-05 实施记录）

Task 1-10 全部完成（commit 已 push）。实施期间相对本 plan 的偏差：

1. **favorites 表简化 6→4 字段**（Task 3）——spec §2.2 原设计 6 字段（id + history_id + is_deleted + created_at + updated_at + sync_md5 + UNIQUE + FK），实现期间简化到 4 字段：删除独立 `id`（history_id 直接作 PK）、删除 `created_at`（用 history 行的）、删除 `UNIQUE(history_id, is_deleted)`（PK 已唯一）、删除 `FOREIGN KEY`（cleanup 物理删 history 时 favorite tombstone 必须存活）。详见 spec §2.2 + §12.1。

2. **`ClipboardKey` 内联到 sync crate**（Task 4）——spec §6 原设计复用 `octopus_vault::crypto::symmetric::SymmetricKey`，但实际依赖方向是 `sync ← vault`（vault 已依赖 sync），反向依赖形成循环（`sync → vault → sync`）。改为在 `crates/sync/src/clipboard.rs` 内联 `ClipboardKey`（AES-256-GCM，byte-compatible with vault `DerivedKey`）。详见 spec §6.3 + §12.2。

3. **Tombstone 不写/不解密 payload**（Task 6/7）——spec §3.3 / §4.3 / §4.4 原设计 tombstone 也写完整 encrypted payload。实现期间发现：tombstone 内容无意义 + history 行可能已被 cleanup 物理删（JOIN 拿不到）。改为 export tombstone 写 `encrypted_payload=""`（空），pull tombstone 不解密、不还原 history（仅 INSERT/UPDATE favorite tombstone + 清残留 history.is_favorite）。详见 spec §3.3 + §4.4 + §12.4。

4. **Desktop cascade：`ClipboardItem.id` i64→String rippled to 16 处**（Task 9）——`NewClipboardItem.id` 从 i64 改 String 后，前端 TS interface `ClipboardHistoryItem.id` 从 `number` 改 `string`，rippe 到 16 处文件（粘贴 / 删除 / 收藏 / OCR 入口 / 复制 / 编辑 / 截图入库 / 滚动截图入库 等所有走 id 的命令）。

5. **v58→v59 迁移是 `bail!`（非自动迁移）**（Task 1）——`clipboard_history.id` 类型变更（INTEGER→TEXT）无法自动迁移（老数据是毫秒戳，UUID 主键语义不同）。`init_schema` 的 `58 =>` 分支直接 `bail!` 提示清库重建（用户已确认接受）。原 plan Task 1 Step 4 暗示常规升版本——实际是破坏性 bail。

6. **Recording pipeline 仍用毫秒戳字符串**（Task 2 推迟项）——`crates/desktop/src/core/db_queue.rs::DbCommand.id` 仍是 `i64`（毫秒戳），`insert_transcription_at_id(&id.to_string(), ...)` 把毫秒戳当字符串写入 clipboard_history.id。本次仅改非录音路径（text/image/file/ocr）用 `Uuid::new_v4()`——录音流程与 voice id 强耦合（毫秒戳作 session 标识），改 UUID 影响面大，推迟到后续 follow-up。详见 spec §12.8。

7. **Task 合并**（commit 粒度）——Task 1+3、Task 4+5、Task 6+7+8 分别合并为单 commit（共 3 个核心 commit + Task 2 + Task 9 + Task 10 验证）。理由：schema+CRUD、加密+文件格式、merge+export+sync_now 强耦合，分 commit 反复编译。

---

---

## 风险提示

1. **FTS5 rowid 适配**：clipboard_history.id 改 TEXT 后，FTS5 trigger 必须用 SQLite 内部 `rowid`（隐式整数自增），不能用 TEXT 主键。Task 1 Step 2 关键。
2. **uuid crate 依赖**：infra/clipboard/sync 三个 crate 都需要，确认各自 Cargo.toml 有声明。
3. **前端 id 类型 breaking**：`ClipboardHistoryItem.id` 从 number 改 string，前端所有用到 id 的地方都要适配（粘贴 / 删除 / 收藏 / OCR 入口等）。
4. **sync_now 失败不阻断**：clipboard merge 失败不应阻断 vault/hotword 同步（Task 8 try-catch 模式，对齐 hotword merge 的容错）。
5. **cleanup 历史清理 vs favorite**：确认 cleanup 的 `WHERE is_favorite=0` 条件覆盖了 sync 拉来的历史行（它们 is_favorite=1）。
