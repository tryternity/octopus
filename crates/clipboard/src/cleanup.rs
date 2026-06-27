use anyhow::Result;
use rusqlite::Connection;

/// 清理结果
pub struct CleanupResult {
    pub deleted_items: usize,
    pub reclaimed_blobs: usize,
}

/// 执行自动清理：
/// 1. 按天数删除（is_favorite=0）
/// 2. 按数量删除（is_favorite=0，超出限额按 created_at ASC）
/// 3. 孤立 blob 回收
/// 4. FTS5 索引重建
pub fn run_cleanup(conn: &Connection, max_age_days: u32, max_items: u32) -> Result<CleanupResult> {
    let mut deleted = 0;

    // 1. 按天数删除
    deleted += delete_by_age(conn, max_age_days)?;

    // 2. 按数量删除
    deleted += delete_by_count(conn, max_items)?;

    // 3. 无引用 image_data BLOB 清理
    let reclaimed = crate::store::cleanup_unreferenced_images(conn)?;

    // 4. FTS5 重建
    let _ = conn.execute(
        "INSERT INTO clipboard_history_fts(clipboard_history_fts) VALUES('rebuild')",
        [],
    );

    Ok(CleanupResult {
        deleted_items: deleted,
        reclaimed_blobs: reclaimed,
    })
}

fn delete_by_age(conn: &Connection, max_age_days: u32) -> Result<usize> {
    let rows = conn.execute(
        "DELETE FROM clipboard_history
         WHERE is_favorite = 0
         AND created_at < datetime('now', ?)",
        [format!("-{} days", max_age_days)],
    )?;
    Ok(rows)
}

fn delete_by_count(conn: &Connection, max_items: u32) -> Result<usize> {
    let total: i64 = conn.query_row(
        "SELECT COUNT(*) FROM clipboard_history WHERE is_favorite = 0",
        [],
        |r| r.get(0),
    )?;

    if total <= max_items as i64 {
        return Ok(0);
    }

    let excess = total - max_items as i64;
    let rows = conn.execute(
        "DELETE FROM clipboard_history
         WHERE id IN (
             SELECT id FROM clipboard_history
             WHERE is_favorite = 0
             ORDER BY created_at ASC
             LIMIT ?
         )",
        [excess],
    )?;
    Ok(rows)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store;

    fn open_test_db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        let sql = include_str!("../../infra/src/db.sql");
        conn.execute_batch(sql).unwrap();
        conn.execute("PRAGMA foreign_keys = OFF", []).unwrap();
        conn
    }

    fn insert_text(conn: &Connection, id: i64, text: &str, age_seconds: u64) {
        use std::time::{SystemTime, UNIX_EPOCH};
        let old_secs = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs() - age_seconds)
            .unwrap_or(0);
        let (y, mo, d, h, mi, s) = store::epoch_to_ymd_hms(old_secs);
        let created = format!("{:04}-{:02}-{:02} {:02}:{:02}:{:02}", y, mo, d, h, mi, s);
        conn.execute(
            "INSERT INTO clipboard_history (id, item_type, source, content, search_text, created_at, is_rich)
             VALUES (?, 'text', 'clipboard', ?, ?, ?, 0)",
            rusqlite::params![id, text, text, created],
        ).unwrap();
    }

    #[test]
    fn test_cleanup_by_count() {
        let conn = open_test_db();
        for i in 0..5 {
            insert_text(&conn, 7000 + i, &format!("item{}", i), 0);
        }
        // 收藏第一条
        conn.execute("UPDATE clipboard_history SET is_favorite = 1 WHERE id = 7000", []).unwrap();

        let result = run_cleanup(&conn, 365, 3).unwrap();
        assert!(result.deleted_items >= 1, "Should delete at least 1 item (excess beyond limit)");
        // 收藏的不被删
        let count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM clipboard_history WHERE is_favorite = 1", [], |r| r.get(0)
        ).unwrap();
        assert_eq!(count, 1);
    }
}
