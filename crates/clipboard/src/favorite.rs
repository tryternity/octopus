//! Favorite 业务逻辑——toggle/list 收藏，桥接到 DB 层。
//!
//! 与 [`crate::store::toggle_favorite`]（DB 层，仅翻转 `clipboard_history.is_favorite`）
//! 不同：本模块是收藏的业务入口，负责维护独立的 [`octopus_infra::db::clipboard_favorite`] 表
//! 行（active / tombstone 三态），让收藏状态可被 sync 跨设备传播。
//!
//! 三态语义（history_id 直接作 PK，无独立 favorite id）：
//! - 无 favorite 行 → INSERT 新 active favorite（history_id 即主键）
//! - active (is_deleted=0) → soft_delete（写 tombstone）→ 同步给其他设备删除
//! - tombstone (is_deleted>0) → restore（重新激活）
//!
//! 同时把 `clipboard_history.is_favorite` 镜像更新——前端 UI 仍读这列以避免每次都
//! JOIN favorites 表。
use anyhow::Result;
// clipboard_favorite 子模块在 infra 里是私有的，但通过 pub use glob 重导出到
// octopus_infra::db 命名空间（与 sync crate 同习惯）。
use octopus_infra::db::{
    self as infra_db, ClipboardFavorite,
};

/// 切换某条 `clipboard_history` 行的收藏状态。
///
/// - 已有 active favorite → soft delete（取消收藏）
/// - 已有 tombstone       → restore（重新收藏）
/// - 无 favorite 行        → 新建 favorite（history_id 即主键，无独立 favorite id）
///
/// 返回 `true` 表示当前已收藏，`false` 表示已取消收藏。
///
/// # 同步语义
/// 三态转换都会改 `clipboard_favorites` 表的 `updated_at` + `is_deleted`，
/// sync engine 据 updated_at 把变更传播到其他设备。
pub fn toggle_favorite(history_id: &str) -> Result<bool> {
    let existing = infra_db::load_favorite(history_id)?;
    match existing {
        Some(fav) if fav.is_deleted == 0 => {
            // active → un-favorite（写 tombstone，epoch 作为 is_deleted 标记）
            let epoch = now_epoch();
            infra_db::soft_delete_favorite(history_id, epoch)?;
            infra_db::set_clipboard_is_favorite(history_id, false)?;
            Ok(false)
        }
        Some(_) => {
            // tombstone → restore
            infra_db::restore_favorite(history_id)?;
            infra_db::set_clipboard_is_favorite(history_id, true)?;
            Ok(true)
        }
        None => {
            // 新收藏——history_id 直接作 PK，无需生成独立 favorite id
            infra_db::insert_favorite(history_id)?;
            infra_db::set_clipboard_is_favorite(history_id, true)?;
            Ok(true)
        }
    }
}

/// 当前 Unix 秒（用作 favorite tombstone 的 is_deleted 时间戳）。
fn now_epoch() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// 列出当前所有 active 收藏。前端 favorites tab / sync export 共用。
pub fn list_favorites() -> Result<Vec<ClipboardFavorite>> {
    infra_db::list_active_favorites()
}
