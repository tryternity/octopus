use anyhow::Result;
use rusqlite::Connection;

/// 清理结果
pub struct CleanupResult {
    pub deleted_items: usize,
    pub reclaimed_blobs: usize,
}

/// 执行自动清理：
/// 1. 按天数删除（is_favorite=0，超龄）——全部物理 DELETE（容量管理，不进回收站）
/// 2. 按数量删除（总条数超 max_items）——永久删最老的（回收站优先 → 活跃项）
/// 3. 孤立 blob 回收
/// 4. FTS5 索引重建（物理删除触发 DB trigger 已保证一致性，此处 rebuild 为冗余保险）
///
/// 注：自动清理属于容量管理，不走软删——容量超限时软删进回收站毫无意义
/// （回收站本身就在超限，下一轮又被永久删）。软删仅由前端 delete_item 等入口触发（仅 voice）。
pub fn run_cleanup(conn: &Connection, max_age_days: u32, max_items: u32) -> Result<CleanupResult> {
    let mut deleted = 0;

    // 1. 按天数删除（全部物理删）
    deleted += delete_by_age(conn, max_age_days)?;

    // 2. 按数量删除（先清回收站，再清活跃项，全部物理删）
    deleted += delete_by_count(conn, max_items)?;

    // 3. 无引用 image_data BLOB 清理
    let reclaimed = crate::store::cleanup_unreferenced_images(conn)?;

    // 4. FTS5 重建（物理删除已触发 trigger，rebuild 作为冗余保险）
    if deleted > 0 || reclaimed > 0 {
        let _ = conn.execute(
            "INSERT INTO clipboard_history_fts(clipboard_history_fts) VALUES('rebuild')",
            [],
        );
    }

    Ok(CleanupResult {
        deleted_items: deleted,
        reclaimed_blobs: reclaimed,
    })
}

/// 按天数清理（is_favorite=0，超龄）：全部物理 DELETE（容量管理，不进回收站）。
fn delete_by_age(conn: &Connection, max_age_days: u32) -> Result<usize> {
    let age_clause = format!("-{} days", max_age_days);
    let deleted = conn.execute(
        "DELETE FROM clipboard_history
         WHERE is_favorite = 0 AND is_deleted = 0
         AND created_at < datetime('now', ?)",
        [&age_clause],
    )?;
    Ok(deleted)
}

/// 按数量清理（总条数超 max_items 时）。
///
/// **清理顺序**（用户要求：先清回收站，再清正常项）：
/// 1. 计算总条数（活跃 + 回收站，is_favorite=0）
/// 2. 超出 max_items 的部分，**先永久删回收站最老的**（物理 DELETE，腾出空间）
/// 3. 如果回收站清空后仍超限，**再永久删活跃项**（物理 DELETE，最老的优先）
///
/// 全部物理删（容量管理不走软删——容量超限时软删进回收站毫无意义，
/// 回收站本身就在超限，下一轮又被永久删）。返回删除条数。
fn delete_by_count(conn: &Connection, max_items: u32) -> Result<usize> {
    // 总条数 = 活跃 + 回收站（非收藏）
    let total: i64 = conn.query_row(
        "SELECT COUNT(*) FROM clipboard_history WHERE is_favorite = 0",
        [],
        |r| r.get(0),
    )?;

    if total <= max_items as i64 {
        return Ok(0);
    }

    let mut excess = total - max_items as i64;
    let mut deleted = 0usize;

    // 第一步：先永久删回收站最老的（物理 DELETE 腾空间）
    if excess > 0 {
        let trash_count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM clipboard_history WHERE is_favorite = 0 AND is_deleted = 1",
            [], |r| r.get(0),
        )?;
        if trash_count > 0 {
            let to_delete = excess.min(trash_count);
            deleted += conn.execute(
                "DELETE FROM clipboard_history WHERE id IN (
                    SELECT id FROM clipboard_history
                    WHERE is_favorite = 0 AND is_deleted = 1
                    ORDER BY created_at ASC LIMIT ?
                )",
                [to_delete],
            )?;
            excess -= to_delete;
        }
    }

    // 第二步：回收站清空后仍超限 → 永久删活跃项（物理 DELETE，最老的优先）。
    if excess > 0 {
        deleted += conn.execute(
            "DELETE FROM clipboard_history
             WHERE id IN (
                 SELECT id FROM clipboard_history
                 WHERE is_favorite = 0 AND is_deleted = 0
                 ORDER BY created_at ASC LIMIT ?
             )",
            [excess],
        )?;
    }

    Ok(deleted)
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
            "INSERT INTO clipboard_history (id, item_type, content, created_at, is_rich)
             VALUES (?, 'text', ?, ?, 0)",
            rusqlite::params![id, text, created],
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
