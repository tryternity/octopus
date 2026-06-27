# 图片存储迁移：文件系统 → DB BLOB

**日期**: 2026-06-27
**状态**: 设计完成，待实施
**分支**: `feature/clipboard-research`

## 0. 概述

将剪贴板图片从文件系统（`~/.octopus/clipboard_images/`）迁移到 SQLite DB BLOB 存储。消除文件与 DB 不一致风险，防止用户误删，简化回收逻辑。

## 1. 新增表

```sql
CREATE TABLE IF NOT EXISTS image_data (
    hash       TEXT PRIMARY KEY,     -- SHA-256(PNG bytes)，去重键
    blob       BLOB NOT NULL,        -- 图片原图 BLOB（格式见 image_type）
    thumb      BLOB NOT NULL,        -- 缩略图 BLOB（240×240 resize）
    image_type TEXT NOT NULL DEFAULT 'webp',  -- BLOB 格式：webp（预留 png/jpeg 扩展）
    width      INTEGER NOT NULL,
    height     INTEGER NOT NULL,
    created_at TEXT NOT NULL
);
```

`clipboard_history.blob_hash` 通过应用层引用 `image_data.hash`（不用外键约束——SQLite 外键默认关闭，且跨表 CASCADE 在 PRAGMA foreign_keys=OFF 下不生效，改用应用层引用计数）。

## 2. 存储策略

| 存储项 | 格式 | 用途 |
|---|---|---|
| `blob` | WebP 100% 无损 | OCR 识别 + 用户导出（转 JPEG/PNG） |
| `thumb` | WebP 20%，240×240 Lanczos resize | 前端列表内联展示 |

**体积估算**：典型截图 PNG 200-500KB → WebP 无损 100-300KB；缩略图 3-8KB。500 张约 50-150MB + 缩略图 2-4MB。

**为什么 WebP 无损而非有损**：
- 剪贴板截图含文字/图标，有损压缩会产生伪影影响 OCR 精度
- WebP 无损对截图类图片压缩率优于 PNG（约 20-30% 体积缩减）
- 最大 500 条，体积可控

## 3. 编码流程（watcher.rs 修改）

```
剪贴板图片事件
  │
  ├─ clipboard-rs get_image() → RGBA pixels
  ├─ encode_and_hash() → PNG bytes + SHA-256
  ├─ 去重：hash 已在 image_data 表？→ 跳过编码，只插入 clipboard_history 行
  ├─ PNG → WebP 100% 无损（webp crate Encoder::encode_lossless）
  ├─ resize 240×240 (image crate Lanczos3) → WebP 20%（Encoder::encode(20.0)）
  └─ INSERT INTO image_data (hash, blob, thumb, width, height, created_at)
```

## 4. 读取流程

### 4.1 OCR 识别

```
ocr_image 命令
  → clipboard_history.blob_hash
  → SELECT blob FROM image_data WHERE hash = ?
  → image::load_from_memory(webp_bytes) → DynamicImage
  → OcrEngine::recognize(&DynamicImage)
```

### 4.2 前端缩略图展示

新增 Tauri 命令 `get_image_thumb(id: i64) -> Vec<u8>`：
```
→ clipboard_history.blob_hash
→ SELECT thumb FROM image_data WHERE hash = ?
→ 返回 WebP bytes
```

前端 `<img src="data:image/webp;base64,${base64}">` 内联展示。

### 4.3 导出保存

`save_image_item` 修改：
```
→ SELECT blob FROM image_data WHERE hash = ?  （WebP 无损 bytes）
→ 用户选格式（JPEG/WebP/PNG）
  ├─ PNG: image crate 解码 WebP → 重新编码 PNG
  ├─ JPEG: image crate 解码 WebP → JpegEncoder
  └─ WebP: 直接写原始 bytes（已是无损 WebP）
→ 写入 ~/Downloads/octopus/
```

## 5. 删除流程（引用计数）

```
delete_item(id)
  → 读 clipboard_history.blob_hash
  → DELETE FROM clipboard_history WHERE id = ?
  → SELECT COUNT(*) FROM clipboard_history WHERE blob_hash = ?
  → count == 0 ? DELETE FROM image_data WHERE hash = ? : 跳过
```

`clear_history` 同理：批量删 clipboard_history 后，清理无引用的 image_data 行。

**不再需要**：`cleanup_orphaned_blobs`、`delete_blob_files`、`clipboard_images/` 目录。

## 6. 删除清单

| 删除项 | 原因 |
|---|---|
| `clipboard/src/image.rs::clipboard_images_dir()` | 不再使用文件系统 |
| `clipboard/src/image.rs::save_image()` | 替换为 DB 写入 |
| `clipboard/src/image.rs::generate_thumbnail()` | 缩略图在编码时一步生成 |
| `clipboard/src/image.rs::cleanup_orphaned_blobs()` | 改为 DB 引用计数 |
| `clipboard/src/image.rs::delete_blob_files()` | 改为 DB DELETE |
| `clipboard/src/cleanup.rs::run_cleanup` 中 blob 回收步骤 | 改为 DB 清理 |
| `~/.octopus/clipboard_images/` 目录 | 历史数据，迁移后删除 |

## 7. 迁移策略

一次性迁移（应用启动时检测）：
```
if clipboard_images/ 目录存在:
  for each <hash>.png in 目录:
    if image_data 中无此 hash:
      PNG → WebP 无损 + 缩略图 → INSERT image_data
  迁移完成后删除 clipboard_images/ 目录
```

迁移幂等——已存在的 hash 跳过。

## 8. 依赖

已有依赖，无需新增：
- `webp = "0.3"`（infra 已有）— WebP 编码
- `image = "0.25"`（clipboard 已有）— resize + 格式转换

## 9. DB 版本

v6 → v7 迁移：新增 `image_data` 表。

## 10. 风险

| 风险 | 概率 | 影响 | 缓解 |
|---|---|---|---|
| DB 膨胀影响查询性能 | 低 | Mutex 阻塞 | SQLite BLOB 读写不经 Mutex 行锁，页级锁；实测 200KB BLOB 读写 <1ms |
| 迁移中断 | 低 | 部分图片未迁移 | 幂等设计，下次启动继续 |
| WebP 编码性能 | 低 | 监听延迟 | webp crate 编码 200KB PNG 约 5-10ms，不阻塞 |
| 旧 clipboard_images 目录残留 | 低 | 磁盘浪费 | 迁移成功后删除目录 |
