//! RecordStore：录屏元数据入库（recordings / recordings_thumbnails 表）。

use crate::error::RecordResult;
use std::collections::HashSet;

#[derive(Debug, Clone, PartialEq)]
pub struct RecordingMeta {
    pub id: i64,
    pub file_path: String,
    pub title: String,
    pub duration_ms: i64,
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub codec: String,
    pub has_system_audio: bool,
    pub has_microphone: bool,
    pub source_type: String,
    pub file_size: u64,
    pub has_thumbnail: bool,
    pub is_favorite: bool,
    pub created_at: String,
    pub deleted_at: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct ListFilter {
    pub limit: u32,
    pub offset: u32,
    pub include_deleted: bool,
    pub favorites_only: bool,
}

pub struct RecordStore<'a> {
    conn: &'a rusqlite::Connection,
}

impl<'a> RecordStore<'a> {
    pub fn new(conn: &'a rusqlite::Connection) -> Self {
        Self { conn }
    }

    pub fn insert(&self, meta: &RecordingMeta, thumbnail: Option<&[u8]>) -> RecordResult<()> {
        self.conn.execute(
            "INSERT INTO recordings
             (id, file_path, title, duration_ms, width, height, fps, codec,
              has_system_audio, has_microphone, source_type, file_size,
              has_thumbnail, is_favorite, created_at, deleted_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, NULL)",
            rusqlite::params![
                meta.id, meta.file_path, meta.title, meta.duration_ms,
                meta.width, meta.height, meta.fps, meta.codec,
                meta.has_system_audio as i32, meta.has_microphone as i32,
                meta.source_type, meta.file_size,
                thumbnail.is_some() as i32, meta.is_favorite as i32,
                meta.created_at,
            ],
        )?;
        if let Some(thumb) = thumbnail {
            self.conn.execute(
                "INSERT INTO recordings_thumbnails (recording_id, blob, width, height, created_at)
                 VALUES (?1, ?2, 240, 135, ?3)",
                rusqlite::params![meta.id, thumb, meta.created_at],
            )?;
        }
        Ok(())
    }

    pub fn get(&self, id: i64) -> RecordResult<Option<RecordingMeta>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, file_path, title, duration_ms, width, height, fps, codec,
                    has_system_audio, has_microphone, source_type, file_size,
                    has_thumbnail, is_favorite, created_at, deleted_at
             FROM recordings WHERE id = ?1",
        )?;
        let mut rows = stmt.query(rusqlite::params![id])?;
        if let Some(row) = rows.next()? {
            Ok(Some(self.row_to_meta(row)?))
        } else {
            Ok(None)
        }
    }

    pub fn list(&self, filter: &ListFilter) -> RecordResult<Vec<RecordingMeta>> {
        let mut sql = String::from(
            "SELECT id, file_path, title, duration_ms, width, height, fps, codec,
                    has_system_audio, has_microphone, source_type, file_size,
                    has_thumbnail, is_favorite, created_at, deleted_at
             FROM recordings WHERE 1=1",
        );
        if !filter.include_deleted {
            sql.push_str(" AND deleted_at IS NULL");
        }
        if filter.favorites_only {
            sql.push_str(" AND is_favorite = 1");
        }
        sql.push_str(" ORDER BY created_at DESC LIMIT ?1 OFFSET ?2");

        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map(
            rusqlite::params![filter.limit, filter.offset],
            |row| self.row_to_meta(row),
        )?;
        let mut result = Vec::new();
        for r in rows {
            result.push(r?);
        }
        Ok(result)
    }

    pub fn rename(&self, id: i64, title: &str) -> RecordResult<()> {
        let affected = self.conn.execute(
            "UPDATE recordings SET title = ?1 WHERE id = ?2",
            rusqlite::params![title, id],
        )?;
        if affected == 0 {
            return Err(crate::error::RecordError::NotFound(id));
        }
        Ok(())
    }

    pub fn soft_delete(&self, id: i64, now_iso: &str) -> RecordResult<()> {
        let affected = self.conn.execute(
            "UPDATE recordings SET deleted_at = ?1 WHERE id = ?2 AND deleted_at IS NULL",
            rusqlite::params![now_iso, id],
        )?;
        if affected == 0 {
            return Err(crate::error::RecordError::NotFound(id));
        }
        Ok(())
    }

    pub fn restore(&self, id: i64) -> RecordResult<()> {
        let affected = self.conn.execute(
            "UPDATE recordings SET deleted_at = NULL WHERE id = ?1",
            rusqlite::params![id],
        )?;
        if affected == 0 {
            return Err(crate::error::RecordError::NotFound(id));
        }
        Ok(())
    }

    pub fn toggle_favorite(&self, id: i64) -> RecordResult<()> {
        let affected = self.conn.execute(
            "UPDATE recordings SET is_favorite = NOT is_favorite WHERE id = ?1",
            rusqlite::params![id],
        )?;
        if affected == 0 {
            return Err(crate::error::RecordError::NotFound(id));
        }
        Ok(())
    }

    pub fn get_thumbnail(&self, id: i64) -> RecordResult<Option<Vec<u8>>> {
        let result: Option<Vec<u8>> = self.conn
            .query_row(
                "SELECT blob FROM recordings_thumbnails WHERE recording_id = ?1",
                rusqlite::params![id],
                |r| r.get(0),
            )
            .ok()
            .flatten();
        Ok(result)
    }

    /// 列出 DB 里所有 file_path（孤儿清理用）。
    pub fn list_all_file_paths(&self) -> RecordResult<HashSet<String>> {
        let mut stmt = self.conn.prepare("SELECT file_path FROM recordings")?;
        let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
        let mut set = HashSet::new();
        for r in rows {
            set.insert(r?);
        }
        Ok(set)
    }

    /// 从 DB 删除行（permanent_delete 的 DB 部分，文件由调用方删）。
    pub fn delete_db_row(&self, id: i64) -> RecordResult<()> {
        let affected = self.conn.execute(
            "DELETE FROM recordings WHERE id = ?1",
            rusqlite::params![id],
        )?;
        if affected == 0 {
            return Err(crate::error::RecordError::NotFound(id));
        }
        Ok(())
    }

    fn row_to_meta(&self, row: &rusqlite::Row<'_>) -> rusqlite::Result<RecordingMeta> {
        Ok(RecordingMeta {
            id: row.get(0)?,
            file_path: row.get(1)?,
            title: row.get(2)?,
            duration_ms: row.get(3)?,
            width: row.get(4)?,
            height: row.get(5)?,
            fps: row.get(6)?,
            codec: row.get(7)?,
            has_system_audio: row.get::<_, i32>(8)? != 0,
            has_microphone: row.get::<_, i32>(9)? != 0,
            source_type: row.get(10)?,
            file_size: row.get(11)?,
            has_thumbnail: row.get::<_, i32>(12)? != 0,
            is_favorite: row.get::<_, i32>(13)? != 0,
            created_at: row.get(14)?,
            deleted_at: row.get(15)?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;

    fn test_db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        // 执行 db.sql 全文建表（spec §5）
        let sql = include_str!("../../infra/src/db.sql");
        conn.execute_batch(sql).unwrap();
        conn
    }

    fn sample_meta(id: i64) -> RecordingMeta {
        RecordingMeta {
            id,
            file_path: format!("recordings/{}.mp4", id),
            title: format!("测试录屏 {}", id),
            duration_ms: 30000,
            width: 1920,
            height: 1080,
            fps: 30,
            codec: "h264".into(),
            has_system_audio: true,
            has_microphone: false,
            source_type: "display".into(),
            file_size: 1048576,
            has_thumbnail: false,
            is_favorite: false,
            created_at: "2026-07-25T14:30:22Z".into(),
            deleted_at: None,
        }
    }

    #[test]
    fn insert_and_get() {
        let conn = test_db();
        let store = RecordStore::new(&conn);
        let meta = sample_meta(1001);
        store.insert(&meta, None).unwrap();
        let got = store.get(1001).unwrap().unwrap();
        assert_eq!(got.id, 1001);
        assert_eq!(got.title, "测试录屏 1001");
        assert!(got.has_system_audio);
        assert!(!got.has_microphone);
    }

    #[test]
    fn list_excludes_soft_deleted_by_default() {
        let conn = test_db();
        let store = RecordStore::new(&conn);
        store.insert(&sample_meta(1), None).unwrap();
        store.insert(&sample_meta(2), None).unwrap();
        store.soft_delete(1, "2026-07-25T15:00:00Z").unwrap();

        let active = store.list(&ListFilter { limit: 100, offset: 0, include_deleted: false, favorites_only: false }).unwrap();
        assert_eq!(active.len(), 1);
        assert_eq!(active[0].id, 2);

        let all = store.list(&ListFilter { limit: 100, offset: 0, include_deleted: true, favorites_only: false }).unwrap();
        assert_eq!(all.len(), 2);
    }

    #[test]
    fn rename_updates_title() {
        let conn = test_db();
        let store = RecordStore::new(&conn);
        store.insert(&sample_meta(1), None).unwrap();
        store.rename(1, "新标题").unwrap();
        let got = store.get(1).unwrap().unwrap();
        assert_eq!(got.title, "新标题");
    }

    #[test]
    fn rename_nonexistent_returns_not_found() {
        let conn = test_db();
        let store = RecordStore::new(&conn);
        let err = store.rename(9999, "x").unwrap_err();
        assert!(matches!(err, crate::error::RecordError::NotFound(9999)));
    }

    #[test]
    fn soft_delete_and_restore() {
        let conn = test_db();
        let store = RecordStore::new(&conn);
        store.insert(&sample_meta(1), None).unwrap();
        store.soft_delete(1, "2026-07-25T15:00:00Z").unwrap();
        assert!(store.get(1).unwrap().unwrap().deleted_at.is_some());

        store.restore(1).unwrap();
        assert!(store.get(1).unwrap().unwrap().deleted_at.is_none());
    }

    #[test]
    fn toggle_favorite_flips() {
        let conn = test_db();
        let store = RecordStore::new(&conn);
        store.insert(&sample_meta(1), None).unwrap();
        assert!(!store.get(1).unwrap().unwrap().is_favorite);

        store.toggle_favorite(1).unwrap();
        assert!(store.get(1).unwrap().unwrap().is_favorite);

        store.toggle_favorite(1).unwrap();
        assert!(!store.get(1).unwrap().unwrap().is_favorite);
    }

    #[test]
    fn insert_with_thumbnail() {
        let conn = test_db();
        let store = RecordStore::new(&conn);
        let thumb = vec![0x89, 0x50, 0x4E, 0x47]; // PNG magic
        store.insert(&sample_meta(1), Some(&thumb)).unwrap();

        let got = store.get(1).unwrap().unwrap();
        assert!(got.has_thumbnail);

        let t = store.get_thumbnail(1).unwrap().unwrap();
        assert_eq!(t, thumb);
    }

    #[test]
    fn get_thumbnail_none_for_no_thumb() {
        let conn = test_db();
        let store = RecordStore::new(&conn);
        store.insert(&sample_meta(1), None).unwrap();
        assert!(store.get_thumbnail(1).unwrap().is_none());
    }

    #[test]
    fn list_all_file_paths() {
        let conn = test_db();
        let store = RecordStore::new(&conn);
        store.insert(&sample_meta(1), None).unwrap();
        store.insert(&sample_meta(2), None).unwrap();
        let paths = store.list_all_file_paths().unwrap();
        assert_eq!(paths.len(), 2);
        assert!(paths.contains("recordings/1.mp4"));
        assert!(paths.contains("recordings/2.mp4"));
    }

    #[test]
    fn delete_db_row_removes_record() {
        let conn = test_db();
        let store = RecordStore::new(&conn);
        store.insert(&sample_meta(1), None).unwrap();
        store.delete_db_row(1).unwrap();
        assert!(store.get(1).unwrap().is_none());
    }
}
