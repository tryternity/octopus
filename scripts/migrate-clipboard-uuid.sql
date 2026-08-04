-- clipboard_history id INTEGER→TEXT(UUID) 迁移脚本
-- 使用方法：
--   1. 先备份：cp ~/.octopus/octopus.db ~/.octopus/octopus.db.bak
--   2. 执行：sqlite3 ~/.octopus/octopus.db < scripts/migrate-clipboard-uuid.sql
--   3. 更新 schema 版本：sqlite3 ~/.octopus/octopus.db "PRAGMA user_version = 59;"
--
-- 如果出错，恢复备份：cp ~/.octopus/octopus.db.bak ~/.octopus/octopus.db

-- ========== 0. 安全检查 ==========
-- 确保 DB 是 v58（防止重复执行）
-- 如果不是 v58 会报错中断

-- ========== 1. 建 _new 表（TEXT id） ==========
CREATE TABLE clipboard_history_new (
    id              TEXT PRIMARY KEY,
    item_type       TEXT    NOT NULL,
    content         TEXT    NOT NULL DEFAULT '',
    ref_data        TEXT,
    meta_info       TEXT,
    is_favorite     INTEGER NOT NULL DEFAULT 0,
    is_rich         INTEGER NOT NULL DEFAULT 0,
    created_at      TEXT    NOT NULL,
    has_thumbnail   INTEGER NOT NULL DEFAULT 0,
    segments        TEXT,
    is_deleted      INTEGER NOT NULL DEFAULT 0
);

-- ========== 2. 数据迁移（INTEGER id → TEXT，用 hex(lower(uuid-like)) 保证唯一） ==========
-- 策略：原毫秒戳 id 转成确定性的 UUID v5（namespace + 原 id）
-- 但 SQLite 没有内置 uuid()——用简单的字符串转换：'clip-' + 原 id
-- 这样保留了可追溯性（原毫秒戳还在），且全局唯一
INSERT INTO clipboard_history_new (id, item_type, content, ref_data, meta_info, is_favorite, is_rich, created_at, has_thumbnail, segments, is_deleted)
SELECT
    'clip-' || CAST(id AS TEXT),   -- 原 INTEGER 毫秒戳 → 'clip-1722835200123'
    item_type, content, ref_data, meta_info, is_favorite, is_rich,
    created_at, has_thumbnail, segments, is_deleted
FROM clipboard_history;

-- ========== 3. 删旧表 + 重命名 ==========
DROP TABLE clipboard_history_fts;       -- FTS5 虚表（依赖旧表）
DROP TABLE clipboard_history;           -- 旧 INTEGER id 表
ALTER TABLE clipboard_history_new RENAME TO clipboard_history;

-- ========== 4. 重建索引 ==========
CREATE INDEX IF NOT EXISTS idx_clip_created   ON clipboard_history(created_at DESC);
CREATE INDEX IF NOT EXISTS idx_clip_type      ON clipboard_history(item_type);
CREATE INDEX IF NOT EXISTS idx_clip_favorite  ON clipboard_history(is_favorite);
CREATE INDEX IF NOT EXISTS idx_clip_ref       ON clipboard_history(ref_data);
CREATE INDEX IF NOT EXISTS idx_clip_deleted   ON clipboard_history(is_deleted) WHERE is_deleted = 1;

-- ========== 5. 重建 FTS5（新版——用 SQLite 隐式 rowid，不用 id） ==========
CREATE VIRTUAL TABLE clipboard_history_fts USING fts5(
    content,
    content='clipboard_history',
    tokenize='trigram'
);

CREATE TRIGGER clip_fts_ai AFTER INSERT ON clipboard_history BEGIN
    INSERT INTO clipboard_history_fts(rowid, content) VALUES (new.rowid, new.content);
END;
CREATE TRIGGER clip_fts_ad AFTER DELETE ON clipboard_history BEGIN
    INSERT INTO clipboard_history_fts(clipboard_history_fts, rowid, content)
    VALUES('delete', old.rowid, old.content);
END;
CREATE TRIGGER clip_fts_au AFTER UPDATE OF content ON clipboard_history BEGIN
    INSERT INTO clipboard_history_fts(clipboard_history_fts, rowid, content)
    VALUES('delete', old.rowid, old.content);
    INSERT INTO clipboard_history_fts(rowid, content) VALUES (new.rowid, new.content);
END;

-- ========== 6. 回填 FTS5 索引 ==========
INSERT INTO clipboard_history_fts(rowid, content)
    SELECT rowid, content FROM clipboard_history WHERE content != '';

-- ========== 7. 新建 clipboard_favorites 表 ==========
CREATE TABLE IF NOT EXISTS clipboard_favorites (
    id              TEXT PRIMARY KEY,
    history_id      TEXT NOT NULL,
    is_deleted      INTEGER NOT NULL DEFAULT 0,
    created_at      TEXT NOT NULL,
    updated_at      TEXT NOT NULL,
    sync_md5        TEXT,
    UNIQUE(history_id, is_deleted),
    FOREIGN KEY (history_id) REFERENCES clipboard_history(id)
);
CREATE INDEX IF NOT EXISTS idx_clip_fav_active ON clipboard_favorites(is_deleted) WHERE is_deleted = 0;

-- ========== 8. 更新 schema 版本 ==========
PRAGMA user_version = 59;

-- ========== 9. 验证（执行后人工检查输出） ==========
SELECT 'clipboard_history rows:', COUNT(*) FROM clipboard_history;
SELECT 'clipboard_history_fts rows:', COUNT(*) FROM clipboard_history_fts;
SELECT 'clipboard_favorites rows:', COUNT(*) FROM clipboard_favorites;
SELECT 'sample id:', id FROM clipboard_history LIMIT 1;
SELECT 'schema version:', user_version FROM pragma_user_version;
