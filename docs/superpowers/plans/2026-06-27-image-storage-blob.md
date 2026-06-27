# 图片存储迁移：文件系统 → DB BLOB 实施计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development or superpowers:executing-plans.

**Goal:** 将剪贴板图片从文件系统迁移到 SQLite DB BLOB（WebP 无损 + 缩略图），消除文件不一致风险。

**Architecture:** 新增 `image_data` 表存 WebP BLOB。clipboard crate 的 image.rs 全面重写（文件 I/O → DB 操作）。watcher 编码流程改为 WebP。desktop 命令从 DB 读 BLOB。启动时一次性迁移旧文件。

**Tech Stack:** Rust + webp 0.3 + image 0.25 + rusqlite + Tauri

**Spec:** `docs/superpowers/specs/2026-06-27-image-storage-blob-design.md`

---

## 文件结构

| 文件 | 变更类型 | 责任 |
|---|---|---|
| `crates/infra/src/db.sql` | Modify | 新增 image_data 表 CREATE + DB v7 迁移 |
| `crates/infra/src/db.rs` | Modify | init_schema v6→v7 分支 |
| `crates/clipboard/src/image.rs` | **Rewrite** | 删文件 I/O，改为 DB BLOB 编码/读取/删除 |
| `crates/clipboard/src/store.rs` | Modify | 新增 image_data CRUD + delete_item/clear_history 引用计数 |
| `crates/clipboard/src/watcher.rs` | Modify | 图片编码流程改为 WebP → DB |
| `crates/clipboard/src/cleanup.rs` | Modify | 删除文件系统 blob 回收，改为 DB 清理 |
| `crates/desktop/src/clipboard_commands.rs` | Modify | save_image_item/ocr_image 从 DB 读 BLOB + 新增 get_image_thumb |
| `crates/desktop/src/main.rs` | Modify | 注册 get_image_thumb + 迁移调用 |
| `crates/desktop/frontend/src/pages/Clipboard/ClipboardItem.tsx` | Modify | 图片条目内联缩略图 |
| `crates/desktop/frontend/src/pages/Settings/ClipboardPanel.tsx` | Modify | 管理页图片条目缩略图 |

---

### Task 1: DB — image_data 表 + v7 迁移

**Files:**
- Modify: `crates/infra/src/db.sql`
- Modify: `crates/infra/src/db.rs`

- [ ] **Step 1: db.sql 新增 image_data 表**

在 clipboard_history 表块之后（FTS5 之前）添加：

```sql

-- ── 图片 BLOB 存储（image_data 表）─────────────────────────────────────────
-- 替代文件系统 clipboard_images/，WebP 无损 + 缩略图存 DB，引用计数回收。
CREATE TABLE IF NOT EXISTS image_data (
    hash       TEXT PRIMARY KEY,     -- SHA-256(PNG bytes)，去重键
    blob       BLOB NOT NULL,        -- WebP 100% 无损原图
    thumb      BLOB NOT NULL,        -- WebP 20% 缩略图（240×240 Lanczos resize）
    width      INTEGER NOT NULL,
    height     INTEGER NOT NULL,
    created_at TEXT NOT NULL
);
```

- [ ] **Step 2: db.rs init_schema 新增 v6→v7 迁移**

在 `init_schema` 函数的 `v == 5` 分支之后，添加：

```rust
    } else if v == 6 {
        // v6 → v7：image_data 表
        log::info!("DB migrating v6 → v7: adding image_data table...");
        conn.execute_batch(INIT_SQL).context("v6→v7: 建 image_data 表")?;
        conn.execute("PRAGMA user_version = 7", [])?;
        log::info!("DB migrated to v7: image_data");
    }
```

同时把 `v < 2` 分支的 `PRAGMA user_version = 6` 改为 `= 7`，以及 v2/v3/v4/v5 各分支末尾的 `= 6` 也改为 `= 7`（新用户直接到 v7，中间版本跳到 v7）。

- [ ] **Step 3: 验证编译**

```bash
cargo build -p octopus-infra 2>&1 | tail -3
```

- [ ] **Step 4: 手动验证迁移**

```bash
sqlite3 ~/.octopus/octopus.db "PRAGMA user_version;"
cargo run -p octopus-infra 2>/dev/null; # 或直接启动应用
sqlite3 ~/.octopus/octopus.db "PRAGMA user_version;"
sqlite3 ~/.octopus/octopus.db ".schema image_data"
```

Expected: user_version 从 6 → 7，image_data 表存在。

- [ ] **Step 5: Commit**

```bash
git add crates/infra/src/db.sql crates/infra/src/db.rs
git commit -m "feat(infra): image_data 表 + DB v7 迁移"
```

---

### Task 2: image.rs 全面重写

**Files:**
- Rewrite: `crates/clipboard/src/image.rs`

- [ ] **Step 1: 重写 image.rs**

```rust
//! 图片编码：RGBA → PNG → SHA-256 → WebP 无损 + 缩略图 → DB BLOB。
//! 替代旧文件系统方案，不再写 ~/.octopus/clipboard_images/。

use anyhow::{Context, Result};
use sha2::{Digest, Sha256};

/// RGBA 像素 → PNG bytes + SHA-256 hash。
/// hash 用于去重（同一张图只存一份 BLOB）。
pub fn encode_and_hash(rgba: &[u8], width: u32, height: u32) -> Result<(Vec<u8>, String)> {
    let img = ::image::RgbaImage::from_raw(width, height, rgba.to_vec())
        .context("Failed to create RgbaImage from raw pixels")?;
    let mut png_bytes = Vec::new();
    img.write_to(&mut std::io::Cursor::new(&mut png_bytes), ::image::ImageFormat::Png)
        .context("Failed to encode PNG")?;
    let hash = sha256_hex(&png_bytes);
    Ok((png_bytes, hash))
}

/// 编码结果：WebP 无损原图 + WebP 缩略图。
pub struct EncodedImage {
    pub webp_blob: Vec<u8>,
    pub thumb_blob: Vec<u8>,
}

/// PNG bytes → WebP 100% 无损 + 缩略图 WebP 20%（240×240 Lanczos）。
pub fn encode_to_webp(png_bytes: &[u8], width: u32, height: u32) -> Result<EncodedImage> {
    let img = ::image::load_from_memory_with_format(png_bytes, ::image::ImageFormat::Png)
        .context("Failed to decode PNG for WebP encoding")?;
    let rgba = img.to_rgba8();

    // 无损 WebP 原图
    let encoder = webp::Encoder::from_rgba(&rgba, rgba.width(), rgba.height());
    let webp_blob = encoder.encode_lossless();
    let webp_blob = webp_blob.to_vec();

    // 缩略图：resize 240×240 → WebP 20%
    let thumb_img = img.resize(240, 240, ::image::imageops::FilterType::Lanczos3);
    let thumb_rgba = thumb_img.to_rgba8();
    let thumb_encoder = webp::Encoder::from_rgba(&thumb_rgba, thumb_rgba.width(), thumb_rgba.height());
    let thumb_blob = thumb_encoder.encode(20.0);
    let thumb_blob = thumb_blob.to_vec();

    Ok(EncodedImage { webp_blob, thumb_blob })
}

pub fn sha256_hex(data: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(data);
    let result = hasher.finalize();
    let mut hex = String::with_capacity(64);
    for byte in result {
        hex.push_str(&format!("{:02x}", byte));
    }
    hex
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_encode_and_hash() {
        let rgba = vec![255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, 255, 255, 0, 255];
        let (png, hash) = encode_and_hash(&rgba, 2, 2).unwrap();
        assert!(!png.is_empty());
        assert_eq!(hash.len(), 64);
    }

    #[test]
    fn test_dedup_same_image() {
        let rgba = vec![255, 0, 0, 255, 0, 255, 0, 255];
        let (_, hash1) = encode_and_hash(&rgba, 2, 1).unwrap();
        let (_, hash2) = encode_and_hash(&rgba, 2, 1).unwrap();
        assert_eq!(hash1, hash2);
    }

    #[test]
    fn test_encode_to_webp() {
        let rgba = vec![255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, 255, 255, 0, 255];
        let (png, _) = encode_and_hash(&rgba, 2, 2).unwrap();
        let encoded = encode_to_webp(&png, 2, 2).unwrap();
        assert!(!encoded.webp_blob.is_empty());
        assert!(!encoded.thumb_blob.is_empty());
        // WebP 文件头：RIFF
        assert_eq!(&encoded.webp_blob[..4], b"RIFF");
        assert_eq!(&encoded.thumb_blob[..4], b"RIFF");
    }
}
```

- [ ] **Step 2: 验证编译 + 测试**

```bash
cargo test -p octopus-clipboard --lib image 2>&1 | tail -10
```

Expected: 3 tests pass.

- [ ] **Step 3: Commit**

```bash
git add crates/clipboard/src/image.rs
git commit -m "feat(clipboard): image.rs 重写为 WebP DB BLOB 编码"
```

---

### Task 3: store.rs — image_data CRUD + 删除引用计数

**Files:**
- Modify: `crates/clipboard/src/store.rs`

- [ ] **Step 1: 新增 image_data CRUD 函数**

在 `get_referenced_blob_hashes` 函数之后添加：

```rust
/// ── image_data 表 CRUD ──

/// 插入图片 BLOB（WebP 无损 + 缩略图）。
pub fn insert_image_data(
    conn: &Connection,
    hash: &str,
    webp_blob: &[u8],
    thumb_blob: &[u8],
    width: i64,
    height: i64,
) -> Result<()> {
    conn.execute(
        "INSERT OR REPLACE INTO image_data (hash, blob, thumb, width, height, created_at)
         VALUES (?, ?, ?, ?, ?, ?)",
        params![hash, webp_blob, thumb_blob, width, height, iso_now()],
    )?;
    Ok(())
}

/// 读取图片 WebP 无损 BLOB。
pub fn get_image_blob(conn: &Connection, hash: &str) -> Result<Option<Vec<u8>>> {
    let mut stmt = conn.prepare("SELECT blob FROM image_data WHERE hash = ?")?;
    let row = stmt.query_row(params![hash], |r| r.get::<_, Vec<u8>>(0));
    match row {
        Ok(blob) => Ok(Some(blob)),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(e) => Err(e.into()),
    }
}

/// 读取缩略图 WebP BLOB。
pub fn get_image_thumb(conn: &Connection, hash: &str) -> Result<Option<Vec<u8>>> {
    let mut stmt = conn.prepare("SELECT thumb FROM image_data WHERE hash = ?")?;
    let row = stmt.query_row(params![hash], |r| r.get::<_, Vec<u8>>(0));
    match row {
        Ok(blob) => Ok(Some(blob)),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(e) => Err(e.into()),
    }
}

/// 删除 image_data 中无引用的 BLOB（引用计数为 0）。
pub fn cleanup_unreferenced_images(conn: &Connection) -> Result<usize> {
    let deleted = conn.execute(
        "DELETE FROM image_data WHERE hash NOT IN (
            SELECT DISTINCT blob_hash FROM clipboard_history WHERE blob_hash IS NOT NULL
        )",
        [],
    )?;
    Ok(deleted)
}

/// 删除指定 hash 的 image_data（如果无其他条目引用）。
fn delete_image_if_unreferenced(conn: &Connection, hash: &str) {
    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM clipboard_history WHERE blob_hash = ?",
            params![hash],
            |r| r.get(0),
        )
        .unwrap_or(0);
    if count == 0 {
        let _ = conn.execute(
            "DELETE FROM image_data WHERE hash = ?",
            params![hash],
        );
    }
}
```

- [ ] **Step 2: 修改 delete_item 使用引用计数**

```rust
/// 删除单条。若被删的是图片且无其他条目引用同一 blob，顺带删除 image_data 行。
pub fn delete_item(conn: &Connection, id: i64) -> Result<()> {
    let blob_hash: Option<String> = conn
        .query_row(
            "SELECT blob_hash FROM clipboard_history WHERE id = ?",
            params![id],
            |r| r.get::<_, Option<String>>(0),
        )
        .ok()
        .flatten();

    conn.execute("DELETE FROM clipboard_history WHERE id = ?", params![id])?;
    track_deletes(conn, 1);

    if let Some(hash) = blob_hash {
        delete_image_if_unreferenced(conn, &hash);
    }

    Ok(())
}
```

- [ ] **Step 3: 修改 clear_history 使用 cleanup_unreferenced_images**

```rust
/// 清空历史（可选保留收藏）。删除后回收无引用的 image_data BLOB。
pub fn clear_history(conn: &Connection, keep_favorite: bool) -> Result<usize> {
    let rows = if keep_favorite {
        conn.execute("DELETE FROM clipboard_history WHERE is_favorite = 0", [])?
    } else {
        conn.execute("DELETE FROM clipboard_history", [])?
    };
    if rows > 0 {
        track_deletes(conn, rows as u32);
        cleanup_unreferenced_images(conn)?;
    }
    Ok(rows)
}
```

- [ ] **Step 4: 删除旧的 get_referenced_blob_hashes + 文件系统引用**

删除 `get_referenced_blob_hashes` 函数（不再需要——cleanup_unreferenced_images 用 SQL 子查询替代）。

删除 `delete_item` 和 `clear_history` 中对 `crate::image::delete_blob_files` / `crate::image::cleanup_orphaned_blobs` 的调用（如果存在）。

- [ ] **Step 5: 验证编译 + 测试**

```bash
cargo test -p octopus-clipboard 2>&1 | tail -8
```

Expected: all pass.

- [ ] **Step 6: Commit**

```bash
git add crates/clipboard/src/store.rs
git commit -m "feat(clipboard): image_data CRUD + 引用计数删除"
```

---

### Task 4: watcher.rs — 图片编码流程改为 WebP → DB

**Files:**
- Modify: `crates/clipboard/src/watcher.rs`

- [ ] **Step 1: 修改 image 分支**

将 image 分支（约 line 109-157）替换为：

```rust
        } else if handle.has(ContentFormat::Image) {
            // image 类型
            let img_data = handle.read_image()?;
            let (w, h) = img_data.get_size();

            // 超过 40MB 跳过
            let estimated_size = (w as usize) * (h as usize) * 4;
            if estimated_size > 40 * 1024 * 1024 {
                log::info!("Skipping large image ({}x{} ~{}MB)", w, h, estimated_size / 1024 / 1024);
                return Ok(());
            }

            let rgba_img = img_data.to_rgba8()
                .map_err(|e| anyhow::anyhow!("to_rgba8 failed: {}", e))?;
            let rgba = rgba_img.to_vec();
            let (png_bytes, hash) = image::encode_and_hash(&rgba, w, h)?;

            // 去重
            let existing = octopus_infra::db::with_db(|conn| {
                store::find_by_content_hash(conn, &hash)
            })?;

            if let Some(id) = existing {
                octopus_infra::db::with_db(|conn| store::touch_created_at(conn, id))?;
            } else {
                // 编码 WebP 无损 + 缩略图
                let encoded = image::encode_to_webp(&png_bytes, w, h)?;

                // 存 image_data BLOB
                octopus_infra::db::with_db(|conn| {
                    store::insert_image_data(conn, &hash, &encoded.webp_blob, &encoded.thumb_blob, w as i64, h as i64)
                })?;

                // 存 clipboard_history 条目
                octopus_infra::db::with_db(|conn| {
                    store::insert_clipboard_item(conn, &store::NewClipboardItem {
                        id: store::chrono_millis(),
                        item_type: ItemType::Image,
                        content: hash.clone(),
                        search_text: String::new(),
                        created_at: store::iso_now(),
                        blob_hash: Some(hash),
                        width: Some(w as i64),
                        height: Some(h as i64),
                        has_thumbnail: Some(1),
                        file_count: None,
                        is_rich: false,
                    })
                })?;
            }
```

- [ ] **Step 2: 删除不再使用的 import**

检查文件头，删除 `use crate::image;` 如果不再直接引用（现在通过 `image::encode_and_hash` 和 `image::encode_to_webp` 调用，仍需保留）。

- [ ] **Step 3: 验证编译**

```bash
cargo build -p octopus-clipboard 2>&1 | tail -5
```

- [ ] **Step 4: Commit**

```bash
git add crates/clipboard/src/watcher.rs
git commit -m "feat(clipboard): watcher 图片编码改为 WebP → DB BLOB"
```

---

### Task 5: cleanup.rs — 删除文件系统 blob 回收

**Files:**
- Modify: `crates/clipboard/src/cleanup.rs`

- [ ] **Step 1: 修改 run_cleanup**

将步骤 3（孤立 blob 回收）改为 DB 清理：

```rust
    // 3. 无引用 image_data BLOB 清理
    let reclaimed = crate::store::cleanup_unreferenced_images(conn)?;
```

删除原来调用 `crate::image::cleanup_orphaned_blobs` 和 `crate::store::get_referenced_blob_hashes` 的代码。

- [ ] **Step 2: 验证编译 + 测试**

```bash
cargo test -p octopus-clipboard 2>&1 | tail -5
```

- [ ] **Step 3: Commit**

```bash
git add crates/clipboard/src/cleanup.rs
git commit -m "refactor(clipboard): cleanup 改为 DB BLOB 引用计数清理"
```

---

### Task 6: desktop — save_image_item / ocr_image 从 DB 读 + get_image_thumb

**Files:**
- Modify: `crates/desktop/src/clipboard_commands.rs`
- Modify: `crates/desktop/src/main.rs`

- [ ] **Step 1: 修改 save_image_item 从 DB 读 WebP BLOB**

将 `save_image_item` 中读文件的部分：

```rust
    // 旧代码：
    let orig_path = octopus_clipboard::image::clipboard_images_dir()
        .join(format!("{}.png", blob_hash));
    let png_bytes = std::fs::read(&orig_path).map_err(|e| e.to_string())?;
```

替换为：

```rust
    // 新代码：从 DB 读 WebP 无损 BLOB
    let webp_blob = octopus_infra::db::with_db(|conn| {
        octopus_clipboard::store::get_image_blob(conn, &blob_hash)
    })
    .map_err(|e| e.to_string())?
    .ok_or("图片数据不存在")?;
```

然后修改保存逻辑——WebP 格式直接写 BLOB，JPEG/PNG 先解码再编码：

```rust
    // 按扩展名保存
    match ext {
        "png" => {
            // WebP → 解码 → PNG
            let img = ::image::load_from_memory_with_format(&webp_blob, ::image::ImageFormat::WebP)
                .map_err(|e| e.to_string())?;
            img.save_with_format(save_path, ::image::ImageFormat::Png)
                .map_err(|e| e.to_string())?;
        }
        "webp" => {
            // 直接写原始 WebP bytes（已是无损）
            std::fs::write(save_path, &webp_blob).map_err(|e| e.to_string())?;
        }
        _ => {
            // JPEG：WebP → 解码 → JPEG
            let img = ::image::load_from_memory_with_format(&webp_blob, ::image::ImageFormat::WebP)
                .map_err(|e| e.to_string())?;
            octopus_infra::image_util::save_as_jpeg_from_image(&img, save_path, q)
                .map_err(|e| e.to_string())?;
        }
    }
```

注意：infra::image_util 需要新增 `save_as_jpeg_from_image(img: &DynamicImage, ...)` 函数，或直接在 clipboard_commands 中内联 JPEG 编码。选择内联以减少跨 crate 变更：

```rust
    _ => {
        let img = ::image::load_from_memory_with_format(&webp_blob, ::image::ImageFormat::WebP)
            .map_err(|e| e.to_string())?;
        let rgb = img.to_rgb8();
        let mut buf = std::io::BufWriter::new(
            std::fs::File::create(save_path).map_err(|e| e.to_string())?
        );
        let mut encoder = ::image::codecs::jpeg::JpegEncoder::new_with_quality(&mut buf, q);
        encoder.encode(&rgb, rgb.width(), rgb.height(), ::image::ExtendedColorType::Rgb8)
            .map_err(|e| e.to_string())?;
    }
```

- [ ] **Step 2: 修改 ocr_image 从 DB 读 WebP BLOB**

将 `ocr_image` 中读文件的部分：

```rust
    // 旧代码：
    let orig_path = octopus_clipboard::image::clipboard_images_dir()
        .join(format!("{}.png", blob_hash));
    let png_bytes = std::fs::read(&orig_path).map_err(|e| e.to_string())?;
```

替换为：

```rust
    // 新代码：从 DB 读 WebP 无损 BLOB
    let webp_blob = octopus_infra::db::with_db(|conn| {
        octopus_clipboard::store::get_image_blob(conn, &blob_hash)
    })
    .map_err(|e| e.to_string())?
    .ok_or("图片数据不存在")?;
```

然后修改 `engine.recognize` 调用——传入 WebP bytes，OcrEngine::recognize 需要支持 WebP 格式：

修改 `crates/ocr/src/engine.rs` 的 recognize 方法，把 `ImageFormat::Png` 改为自动检测：

```rust
    pub fn recognize(&self, image_bytes: &[u8]) -> Result<String> {
        let img = ::image::load_from_memory(image_bytes)
            .context("Failed to decode image")?;
        // ... 其余不变
    }
```

- [ ] **Step 3: 新增 get_image_thumb 命令**

在 clipboard_commands.rs 末尾添加：

```rust
/// 获取图片缩略图（WebP bytes → 前端 base64 展示）。
#[tauri::command]
pub async fn get_image_thumb(id: i64) -> Result<Vec<u8>, String> {
    let item = octopus_infra::db::with_db(|conn| {
        let items = octopus_clipboard::store::query_history(conn, &QueryFilter {
            filter: "all".into(),
            search: None,
            page: 1,
            size: 1000,
        })?;
        Ok::<_, anyhow::Error>(items.into_iter().find(|i| i.id == id))
    })
    .map_err(|e| e.to_string())?;

    let item = item.ok_or("条目不存在")?;
    if item.item_type != octopus_clipboard::ItemType::Image {
        return Err("非图片条目".into());
    }

    let blob_hash = item.image_meta.as_ref().map(|m| m.blob_hash.clone())
        .ok_or("图片元数据缺失")?;

    let thumb = octopus_infra::db::with_db(|conn| {
        octopus_clipboard::store::get_image_thumb(conn, &blob_hash)
    })
    .map_err(|e| e.to_string())?
    .ok_or("缩略图不存在")?;

    Ok(thumb)
}
```

- [ ] **Step 4: main.rs 注册 get_image_thumb**

在 `ocr_image` 之后添加：

```rust
            clipboard_commands::get_image_thumb,
```

- [ ] **Step 5: 验证编译**

```bash
cargo build -p octopus-desktop --features embedded 2>&1 | tail -5
```

- [ ] **Step 6: Commit**

```bash
git add crates/desktop/src/clipboard_commands.rs crates/desktop/src/main.rs crates/ocr/src/engine.rs
git commit -m "feat(desktop): 从 DB 读图片 BLOB + get_image_thumb 命令"
```

---

### Task 7: 旧文件迁移

**Files:**
- Create: `crates/desktop/src/image_migration.rs`
- Modify: `crates/desktop/src/main.rs`

- [ ] **Step 1: 创建迁移模块**

```rust
//! 一次性迁移：~/.octopus/clipboard_images/ → image_data DB BLOB。
//! 幂等：已存在的 hash 跳过。迁移完成后删除目录。

use std::path::PathBuf;

fn clipboard_images_dir() -> PathBuf {
    octopus_infra::paths::octopus_config_home().join("clipboard_images")
}

/// 迁移文件系统图片到 DB。成功后删除目录。
pub fn migrate_images_to_db() {
    let dir = clipboard_images_dir();
    if !dir.exists() {
        return;
    }

    log::info!("Migrating clipboard_images/ to DB...");

    let mut migrated = 0;
    let mut skipped = 0;
    let mut errors = 0;

    let entries = match std::fs::read_dir(&dir) {
        Ok(e) => e,
        Err(e) => {
            log::warn!("Failed to read clipboard_images/: {}", e);
            return;
        }
    };

    for entry in entries.flatten() {
        let path = entry.path();
        let filename = path.file_name().unwrap_or_default().to_string_lossy().to_string();

        // 只处理 <hash>.png（跳过 _thumb.png）
        if !filename.ends_with(".png") || filename.contains("_thumb") {
            continue;
        }

        let hash = filename.trim_end_matches(".png").to_string();

        // 检查 DB 是否已有此 hash
        let exists = octopus_infra::db::with_db(|conn| {
            octopus_clipboard::store::get_image_blob(conn, &hash)
        }).map(|v| v.is_some()).unwrap_or(false);

        if exists {
            skipped += 1;
            continue;
        }

        // 读取 PNG → 编码 WebP → 存 DB
        match std::fs::read(&path) {
            Ok(png_bytes) => {
                match ::image::load_from_memory_with_format(&png_bytes, ::image::ImageFormat::Png) {
                    Ok(img) => {
                        let w = img.width();
                        let h = img.height();
                        match octopus_clipboard::image::encode_to_webp(&png_bytes, w, h) {
                            Ok(encoded) => {
                                let result = octopus_infra::db::with_db(|conn| {
                                    octopus_clipboard::store::insert_image_data(
                                        conn, &hash, &encoded.webp_blob, &encoded.thumb_blob,
                                        w as i64, h as i64,
                                    )
                                });
                                match result {
                                    Ok(_) => migrated += 1,
                                    Err(e) => {
                                        log::warn!("Failed to insert {}: {}", hash, e);
                                        errors += 1;
                                    }
                                }
                            }
                            Err(e) => {
                                log::warn!("Failed to encode {}: {}", hash, e);
                                errors += 1;
                            }
                        }
                    }
                    Err(e) => {
                        log::warn!("Failed to decode {}: {}", hash, e);
                        errors += 1;
                    }
                }
            }
            Err(e) => {
                log::warn!("Failed to read {}: {}", path.display(), e);
                errors += 1;
            }
        }
    }

    log::info!(
        "Image migration: {} migrated, {} skipped, {} errors",
        migrated, skipped, errors
    );

    // 全部成功（无错误）才删除目录
    if errors == 0 {
        if let Err(e) = std::fs::remove_dir_all(&dir) {
            log::warn!("Failed to remove clipboard_images/: {}", e);
        } else {
            log::info!("Removed clipboard_images/ directory");
        }
    }
}
```

- [ ] **Step 2: main.rs 注册模块 + 启动时调用**

在 main.rs 的 mod 声明区添加：

```rust
mod image_migration;
```

在 `setup` 中（FTS5 rebuild 之后）添加：

```rust
            // 迁移旧文件系统图片到 DB BLOB
            image_migration::migrate_images_to_db();
```

注意：main.rs 需要添加 `octopus_clipboard` 和 `image` crate 的引用。检查 desktop Cargo.toml 是否已有 `image` 依赖——可能需要添加。

- [ ] **Step 3: 验证编译**

```bash
cargo build -p octopus-desktop --features embedded 2>&1 | tail -5
```

- [ ] **Step 4: Commit**

```bash
git add crates/desktop/src/image_migration.rs crates/desktop/src/main.rs
git commit -m "feat(desktop): 旧文件系统图片迁移到 DB BLOB"
```

---

### Task 8: 前端 — 图片条目内联缩略图

**Files:**
- Modify: `crates/desktop/frontend/src/pages/Clipboard/ClipboardItem.tsx`
- Modify: `crates/desktop/frontend/src/pages/Settings/ClipboardPanel.tsx`

- [ ] **Step 1: ClipboardItem.tsx — 图片条目加载缩略图**

在组件内加缩略图状态：

```typescript
  const [thumbSrc, setThumbSrc] = useState<string | null>(null);
```

加 useEffect 加载缩略图：

```typescript
  useEffect(() => {
    if (item.item_type === "image") {
      invoke<number[]>("get_image_thumb", { id: item.id })
        .then((bytes) => {
          const base64 = btoa(bytes.map(b => String.fromCharCode(b)).join(""));
          setThumbSrc(`data:image/webp;base64,${base64}`);
        })
        .catch(() => {});
    }
  }, [item.id, item.item_type]);
```

修改图片条目的内容渲染——替换「图片 WxH」文字为缩略图 + 尺寸：

```tsx
        {item.item_type === "image" && item.image_meta ? (
          <div className="flex items-center gap-2">
            {thumbSrc && (
              <img src={thumbSrc} className="w-10 h-10 rounded object-cover flex-shrink-0" alt="" />
            )}
            <span className="text-xs text-muted-foreground">
              {item.image_meta.width}×{item.image_meta.height}
            </span>
          </div>
        ) : item.item_type === "file" ? (
```

- [ ] **Step 2: ClipboardPanel.tsx — 管理页同样加载缩略图**

在 ClipboardRow 内加同样的 thumbSrc 状态 + useEffect + 渲染。

- [ ] **Step 3: 构建前端**

```bash
cd crates/desktop/frontend && npm run build 2>&1 | tail -5
```

- [ ] **Step 4: Commit**

```bash
git add crates/desktop/frontend/ crates/desktop/dist/
git commit -m "feat(clipboard): 图片条目内联缩略图展示"
```

---

### Task 9: 清理旧代码

**Files:**
- Modify: `crates/clipboard/src/image.rs`（删除 clipboard_images_dir / save_image / ImageSaveResult / generate_thumbnail / cleanup_orphaned_blobs / delete_blob_files）
- Modify: `crates/clipboard/src/lib.rs`（确认 pub mod image 仍需）
- Modify: `crates/clipboard/src/store.rs`（删除 get_referenced_blob_hashes 如果不再使用）

- [ ] **Step 1: 确认所有旧引用已清除**

```bash
grep -rn "clipboard_images_dir\|save_image\|ImageSaveResult\|cleanup_orphaned_blobs\|delete_blob_files\|get_referenced_blob_hashes" crates/ --include="*.rs"
```

Expected: 无输出（全部已清除）。

- [ ] **Step 2: 验证编译 + 全量测试**

```bash
cargo test -p octopus-clipboard 2>&1 | tail -5
cargo build -p octopus-desktop --features embedded 2>&1 | tail -5
```

- [ ] **Step 3: Commit**

```bash
git add -A
git commit -m "refactor(clipboard): 清理旧文件系统图片代码"
```

---

### Task 10: 端到端验证

- [ ] **Step 1: 完整构建**

```bash
cd crates/desktop/frontend && npm run build
cd .. && cargo build --features embedded 2>&1 | tail -5
```

- [ ] **Step 2: 运行应用，截图测试**

```bash
./run-octopus.sh
```

验证：
1. 截图 → 剪贴板浮窗显示缩略图（不是纯文字）
2. 点击 OCR → 识别成功
3. 删除图片条目 → image_data 表对应行也删了：`sqlite3 ~/.octopus/octopus.db "SELECT COUNT(*) FROM image_data;"`
4. 导出图片为 JPEG/PNG/WebP → 文件正确
5. `~/.octopus/clipboard_images/` 目录被删除（迁移完成）

- [ ] **Step 3: Commit（如有修复）**

---

## Spec Coverage（自审）

| spec 章节 | 实现 task |
|---|---|
| §1 新增 image_data 表 | Task 1 |
| §2 存储策略（WebP 无损 + 20% 缩略图） | Task 2 |
| §3 编码流程 | Task 2 + Task 4 |
| §4.1 OCR 读取 | Task 6 |
| §4.2 前端缩略图 | Task 6 + Task 8 |
| §4.3 导出保存 | Task 6 |
| §5 删除引用计数 | Task 3 |
| §6 删除清单 | Task 9 |
| §7 迁移策略 | Task 7 |
| §8 依赖（已有） | 无需 task |
| §9 DB v7 | Task 1 |

---

## 实施偏差与补充记录

### 偏差 1：image_type 字段

spec 原设计无 `image_type` 列，实施时用户要求新增（预留未来 PNG/JPEG 格式扩展）。DB schema 和 `insert_image_data` 均含 `image_type TEXT NOT NULL DEFAULT 'webp'`。

### 偏差 2：encode_to_webp 参数未使用

`encode_to_webp(png_bytes, _width, _height)` 的 width/height 参数实际未使用（图片尺寸从 PNG 解码内部获取），保留下划线前缀兼容调用方签名（watcher.rs 传入 w/h）。

### 偏差 3：编译 warning 清理

- `store.rs` 删除未使用的 `use std::collections::HashSet`（`get_referenced_blob_hashes` 被删后无引用）
- `image.rs` `encode_to_webp` 参数加 `_` 前缀

### 偏差 4：desktop Cargo.toml 新增 image 依赖

`image_migration.rs` 模块需要 `image` crate 做格式转换，desktop Cargo.toml 新增 `image = { version = "0.25", features = ["png", "webp", "jpeg"] }`。

### 偏差 5：save_image_item 导出逻辑变更

原计划从文件系统读 PNG → `infra::image_util` 转码。实施改为从 DB 读 WebP BLOB → `image` crate 解码 → 按目标格式编码（JPEG/PNG 解码再编码，WebP 直接写原始 BLOB）。不再依赖 `infra::image_util`。

### 偏差 6：端到端验证通过

用户确认：截图 → 缩略图显示 → OCR 识别 → 删除条目 → image_data 引用计数回收 → 导出 JPEG/WebP/PNG 全部正常。
