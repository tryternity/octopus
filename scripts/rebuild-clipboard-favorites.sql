-- clipboard_favorites 表重建脚本（旧 6 字段 → 新 4 字段）
-- 使用方法：
--   sqlite3 ~/.octopus/octopus.db < scripts/rebuild-clipboard-favorites.sql

-- 1. 删旧表
DROP TABLE IF EXISTS clipboard_favorites;

-- 2. 建新表（4 字段）
CREATE TABLE clipboard_favorites (
    history_id   TEXT PRIMARY KEY,
    is_deleted   INTEGER NOT NULL DEFAULT 0,
    updated_at   TEXT NOT NULL,
    sync_md5     TEXT
);
CREATE INDEX idx_clip_fav_active ON clipboard_favorites(is_deleted) WHERE is_deleted = 0;

-- 3. 从 clipboard_history.is_favorite=1 回填（把已有的收藏标记导入新表）
INSERT INTO clipboard_favorites (history_id, is_deleted, updated_at, sync_md5)
SELECT id, 0, datetime('now'), NULL
FROM clipboard_history
WHERE is_favorite = 1;

-- 4. 验证
SELECT 'favorites rows:', COUNT(*) FROM clipboard_favorites;
SELECT 'history is_favorite=1:', COUNT(*) FROM clipboard_history WHERE is_favorite = 1;
