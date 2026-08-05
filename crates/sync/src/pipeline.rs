//! 同步流水线基础设施——`trait SyncEntity` + 泛型 `merge_three_way`。
//!
//! 2026-08-05 抽象（spec `2026-08-05-sync-entity-trait-unification-design.md`）：
//! vault cipher/folder + hotword set/word + clipboard favorite 共 6 段 3-way merge
//! 主循环逐行同构（outline 有/无 × DB 有/无 → pull/push/conflict/skip，共 ~430 行重复）。
//! 本模块定义统一 trait + 泛型骨架，后续 Task 为各实体 impl 本 trait 后即可消除重复。
//!
//! **本 Task（阶段 1）仅落地基础设施**——trait 暂无 impl，编译通过即可
//! （`dead_code` warning 预期，待阶段 3-5 各模块 impl 后消除）。
//!
//! ## 3-way 判定顺序（不变量——与现有 6 段 merge 完全一致）
//!
//! 1. **tombstone 单向优先**（2026-08-04/05 fix）：远程文件是未超期 tombstone 时
//!    无条件 pull——防本地 active 写回文件覆盖远程 tombstone，否则第三台从没见过
//!    该实体的机器会把 active 当新增拉进来 → 复活。
//! 2. **updated_at 比较**：`remote > local` → pull；`local > remote` → push。
//! 3. **md5 冲突 DB 赢**：时间戳相等时比对 md5，不一致则 push 本地 DB（DB 是真相源）。
//! 4. **md5 相同** → skip（双方一致，无操作）。
//!
//! 外加：outline 无 + DB 有 → push（本地新增传播到远程）。

use std::collections::{HashMap, HashSet};

use anyhow::Result;

use crate::outline::OutlineEntry;
use crate::store::now_secs;

// === Report ============================================================

/// 3-way merge 计数报告——4 字段统一 vault/hotword/clipboard 的 report 结构。
///
/// 替代各模块原有的 `HotwordMergeReport` / `ClipboardMergeReport`（阶段 3-5 迁移）。
/// vault 的 `MergeReport` 已用此名（`vault/src/sync/engine.rs`），后续也切到此类型。
#[derive(Debug, Clone, Default)]
pub struct MergeReport {
    /// 远程 → 本地 pull 次数（DB 无 / 远程更新 / 远程 tombstone）。
    pub pulled: usize,
    /// 本地 → 远程 push 次数（DB 更新 / md5 冲突 DB 赢 / DB-only）。
    pub pushed: usize,
    /// md5 冲突次数（时间戳相等但 md5 不一致 → DB 赢，记 conflict）。
    pub conflicts: usize,
    /// 跳过次数（超期 tombstone 拒绝复活 / pull/push 出错 / md5 相同）。
    pub skipped: usize,
}

// === Trait =============================================================

/// 同步实体——每个可同步的实体类型实现此 trait，由 [`merge_three_way`] 驱动 3-way merge。
///
/// 实现者负责所有模块特有逻辑（加密 / md5 分隔 / tombstone 类型 / outline 结构），
/// trait 只抽象同构的 merge 控制流。详见 spec §3。
pub trait SyncEntity {
    /// DB 行类型（如 `HotwordSet` / `VaultCipher`）。需 Clone 供 push 拷贝。
    type Row: Clone;
    /// `.sync` 文件类型（如 `HotwordSetMeta` / `CipherFile`）。
    type File;

    /// 实体标签（如 `"热词词典"` / `"密码"`），用于日志。
    const LABEL: &'static str;

    /// tombstone 超期保留秒数。`0` = 永久保留（不做 GC，vault/clipboard 当前默认）。
    /// hotword = 864000（10 天）。spec §7.2。
    fn tombstone_retention_secs() -> i64 {
        0
    }

    // ── DB 操作 ──

    /// 列出 DB 全部行（含 tombstone——merge 需感知软删态）。order 不限。
    fn list_db_rows() -> Result<Vec<Self::Row>>;
    /// 行的同步主键（uuid / history_id 等）。
    fn sync_key(row: &Self::Row) -> &str;
    /// 行的最后更新时间（Unix 毫秒）——由实现者从 `updated_at` 字段转换。
    fn updated_ms(row: &Self::Row) -> i64;
    /// 是否为 tombstone（`is_deleted > 0`）。
    fn is_tombstone(row: &Self::Row) -> bool;
    /// 行的 md5 指纹（hex 32 字符）——实现者负责各自的分隔策略。
    fn md5_of(row: &Self::Row) -> String;

    // ── 文件操作 ──

    /// 读 `.sync` 文件（按 key 派生路径）。
    fn read_file(key: &str) -> Result<Self::File>;
    /// 文件 → DB 行（反序列化 + 字段映射；不写 DB）。
    fn file_to_row(file: &Self::File) -> Self::Row;
    /// 文件是否为 tombstone（`is_deleted > 0`）。
    fn file_is_tombstone(file: &Self::File) -> bool;
    /// 文件 tombstone 的时间戳（Unix 秒）——用于 GC 超期判定。
    /// 非 tombstone 时返回 0（调用方应先过 `file_is_tombstone` 短路）。
    fn file_tombstone_timestamp(file: &Self::File) -> i64;
    /// DB 行 → `.sync` 文件（序列化 + 写盘）。
    fn write_file(row: &Self::Row) -> Result<()>;

    // ── merge 操作 ──

    /// 文件行 → upsert DB。返回 `Ok(true)` 成功写入；`Ok(false)` 拒绝复活（超期
    /// tombstone / name 冲突等）——调用方记 skipped。
    fn upsert_db_from_file(row: &Self::Row) -> Result<bool>;

    // ── GC（默认 noop；hotword 已实现，vault/clipboard 阶段 3/5 新增）──

    /// 硬删超期 tombstone 的 DB 行 + 对应 `.sync` 文件。返回清理条数。
    /// 默认 noop（retention_secs = 0 的实体无 GC）。
    fn purge_expired_tombstones(_now: i64) -> Result<usize> {
        Ok(0)
    }

    // ── 导出 ──

    /// 从 DB 最新状态重建所有 `.sync` 文件 + outline（DB 是单一真相源）。
    /// merge_three_way 末尾调用。
    fn export_all() -> Result<()>;

    // ── outline ──

    /// 读远程 outline 条目（key → entry）。单层 outline（cipher/folder/favorite）
    /// 直接返回；双层 outline（hotword set + 每 set word）由实现者负责选层。
    fn read_outline_entries() -> Result<Vec<(String, OutlineEntry)>>;
}

// === 3-way merge 骨架 ==================================================

/// 泛型 3-way merge——按 outline 驱动各实体的 pull/push/conflict/skip。
///
/// 判定顺序（详见模块文档）：
/// 1. DB 无 → pull
/// 2. DB 有 + 远程未超期 tombstone → pull（单向优先 fix）
/// 3. DB 有 + remote 更新 → pull
/// 4. DB 有 + local 更新 → push
/// 5. DB 有 + 时间戳相等 + md5 不一致 → push + conflict（DB 赢）
/// 6. md5 相同 → skip
/// 7. outline 无 + DB 有 → push
///
/// 末尾调 `E::export_all()` 重建所有文件 + outline。
///
/// **tombstone 检查的容错**（与现有 hotword/clipboard 一致）：读文件失败时不传播
/// `Err`，而是 `.unwrap_or(false)` 视为「非 tombstone」继续 updated_at/md5 判定——
/// 后续 `pull_entity` 会再做一次读文件并在失败时 log_warn_skip。spec §4.1 草稿中
/// 的 `?` 会破坏「零行为变更」不变量（§8.1），故此处采用 `.ok().map().unwrap_or(false)`。
pub fn merge_three_way<E: SyncEntity>(report: &mut MergeReport, now: i64) -> Result<()> {
    let outline_entries = E::read_outline_entries()?;
    let db_rows = E::list_db_rows()?;
    let db_by_id: HashMap<&str, &E::Row> = db_rows
        .iter()
        .map(|r| (E::sync_key(r), r))
        .collect();
    let outline_keys: HashSet<&str> = outline_entries.iter().map(|(k, _)| k.as_str()).collect();

    let retention = E::tombstone_retention_secs();

    for (key, entry) in &outline_entries {
        let remote_updated = entry.updated_ms;
        match db_by_id.get(key.as_str()) {
            None => {
                // DB 无 → pull（读文件 upsert，回填 sync_md5）
                pull_entity::<E>(key, report, now)?;
            }
            Some(db_row) => {
                let local_updated = E::updated_ms(db_row);
                // 🔴 复活 bug 修复（2026-08-04/05）：删除是单向终态——远程已 tombstone
                // （未超期）时，**无论本地时间戳多新都应 pull tombstone 到 DB**，而不是
                // 把本地 active 写回文件覆盖远程 tombstone。否则第三台从没见过该实体的
                // 机器会把 active 当新实体拉进来 → 复活。
                //
                // retention <= 0 时永不超期（永久保留）；retention > 0 时检查超期。
                // 读文件失败 → `.unwrap_or(false)` 视为非 tombstone（与现有 hotword/
                // clipboard 对称），后续 pull_entity 会重试读并在失败时 skip。
                let remote_is_tombstone = E::read_file(key)
                    .ok()
                    .map(|f| {
                        E::file_is_tombstone(&f)
                            && (retention <= 0
                                || !is_tombstone_expired(
                                    retention,
                                    E::file_tombstone_timestamp(&f),
                                    now,
                                ))
                    })
                    .unwrap_or(false);

                if remote_is_tombstone {
                    pull_entity::<E>(key, report, now)?;
                } else if remote_updated > local_updated {
                    // 远程更新 → pull 覆盖 DB
                    pull_entity::<E>(key, report, now)?;
                } else if local_updated > remote_updated {
                    // DB 更新 → push 覆盖文件
                    push_or_skip::<E>(db_row, key, report);
                } else if E::md5_of(db_row) != entry.md5 {
                    // 时间戳相等 + md5 冲突 → DB 赢，push 并记 conflict
                    if push_or_skip::<E>(db_row, key, report) {
                        report.conflicts += 1;
                    }
                }
                // md5 相同 → skip（双方一致，无操作）
            }
        }
    }

    // DB 有 + outline 无 → push（本地新增传播到远程）
    for row in &db_rows {
        let key = E::sync_key(row);
        if !outline_keys.contains(key) {
            push_or_skip::<E>(row, key, report);
        }
    }

    E::export_all()?;
    Ok(())
}

/// Pull 单个实体（读文件 → file_to_row → upsert DB），累加 report。
///
/// 严格「Err 不冒泡」——所有错误都 log warn + `skipped += 1`，返回 `Ok(())`。
/// 这与现有 hotword 的 `acc_pull` / clipboard 内联 match 一致：merge 主循环不因
/// 单条 pull 失败而中断整批同步。
///
/// `_now` 参数为签名对称保留（pull 当前不直接用 now——超期 tombstone 的拒绝复活
/// 逻辑由 `upsert_db_from_file` 实现内部处理）。
pub fn pull_entity<E: SyncEntity>(key: &str, report: &mut MergeReport, _now: i64) -> Result<()> {
    match E::read_file(key) {
        Ok(file) => {
            let row = E::file_to_row(&file);
            match E::upsert_db_from_file(&row) {
                Ok(true) => report.pulled += 1,
                Ok(false) => report.skipped += 1, // 拒绝复活 / name 冲突 / 超期 tombstone
                Err(e) => log_warn_skip::<E>(key, "pull upsert", e, report),
            }
        }
        Err(e) => log_warn_skip::<E>(key, "read file", e, report),
    }
    Ok(())
}

// === 辅助函数 ==========================================================

/// Push 单个实体（DB 行 → 写文件），成功 `pushed += 1` 返 `true`；失败 log warn +
/// `skipped += 1` 返 `false`。返值供 md5 冲突分支决定是否额外记 `conflicts += 1`。
fn push_or_skip<E: SyncEntity>(row: &E::Row, key: &str, report: &mut MergeReport) -> bool {
    match E::write_file(row) {
        Ok(()) => {
            report.pushed += 1;
            true
        }
        Err(e) => {
            log_warn_skip::<E>(key, "push file", e, report);
            false
        }
    }
}

/// tombstone 是否超期——`deleted_at > 0` 且 `now - deleted_at > retention_secs`。
///
/// `retention_secs <= 0` 时调用方应已短路（视为永久保留，不调本函数）；本函数对
/// `retention_secs <= 0` 也保守返回 `false`（不超期，保留）以防误调用。
///
/// 与 hotword.rs 的 `is_tombstone_expired` 对称，只是参数顺序不同（这里把
/// retention 作为第一参数，因为是 trait 级配置；hotword 的是模块级常量）。
pub fn is_tombstone_expired(retention_secs: i64, deleted_at: i64, now: i64) -> bool {
    if retention_secs <= 0 {
        return false;
    }
    deleted_at > 0 && now - deleted_at > retention_secs
}

/// 单条实体操作失败的统一日志 + skipped 累加。
///
/// 日志格式与现有 hotword/clipboard 一致：`[sync] {LABEL} merge: {key} {op} 跳过：{e}`。
pub fn log_warn_skip<E: SyncEntity>(key: &str, op: &str, e: anyhow::Error, report: &mut MergeReport) {
    log::warn!("[sync] {} merge: {} {} 跳过：{}", E::LABEL, key, op, e);
    report.skipped += 1;
}

/// GC → merge 编排（spec §4.3）。retention_secs > 0 时先清理超期 tombstone，再 merge。
///
/// 当前 Task 暂未对外暴露调用（各模块的 `merge_*` 入口在阶段 3-5 切到此函数），
/// 但本函数已是完整可用形态。
#[allow(dead_code)]
pub fn run_sync_pipeline<E: SyncEntity>() -> Result<MergeReport> {
    let now = now_secs();
    if E::tombstone_retention_secs() > 0 {
        let purged = E::purge_expired_tombstones(now)?;
        if purged > 0 {
            log::info!("[sync] {} GC: 清理 {} 条超期 tombstone", E::LABEL, purged);
        }
    }
    let mut report = MergeReport::default();
    merge_three_way::<E>(&mut report, now)?;
    log::info!(
        "[sync] {} merge 完成：pulled={} pushed={} conflicts={} skipped={}",
        E::LABEL,
        report.pulled,
        report.pushed,
        report.conflicts,
        report.skipped
    );
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merge_report_default_all_zero() {
        let r = MergeReport::default();
        assert_eq!(r.pulled, 0);
        assert_eq!(r.pushed, 0);
        assert_eq!(r.conflicts, 0);
        assert_eq!(r.skipped, 0);
    }

    #[test]
    fn merge_report_clone() {
        let mut r = MergeReport::default();
        r.pulled = 3;
        r.conflicts = 1;
        let c = r.clone();
        assert_eq!(c.pulled, 3);
        assert_eq!(c.conflicts, 1);
    }

    // ── is_tombstone_expired ──

    #[test]
    fn expired_zero_retention_never_expires() {
        // retention <= 0：永久保留，永不超期
        assert!(!is_tombstone_expired(0, 1_700_000_000, 9_999_999_999));
        assert!(!is_tombstone_expired(-1, 1_700_000_000, 9_999_999_999));
    }

    #[test]
    fn expired_active_never_expires() {
        // deleted_at <= 0（活跃）永不超期
        assert!(!is_tombstone_expired(864_000, 0, 9_999_999_999));
        assert!(!is_tombstone_expired(864_000, -1, 9_999_999_999));
    }

    #[test]
    fn expired_recent_tombstone_within_window() {
        // 删除 1 小时前，retention 10 天 → 未超期
        let now = 1_700_000_000;
        let deleted_at = now - 3_600;
        assert!(!is_tombstone_expired(864_000, deleted_at, now));
    }

    #[test]
    fn expired_old_tombstone_past_window() {
        // 删除 11 天前，retention 10 天 → 超期
        let now = 1_700_000_000;
        let deleted_at = now - 11 * 86_400;
        assert!(is_tombstone_expired(864_000, deleted_at, now));
    }

    #[test]
    fn expired_exact_boundary_not_expired() {
        // 刚好等于 retention（now - deleted_at == retention）→ `>` 严格，不超期
        let now = 1_700_000_000;
        let retention = 864_000;
        let deleted_at = now - retention;
        assert!(!is_tombstone_expired(retention, deleted_at, now));
    }

    #[test]
    fn expired_one_sec_past_boundary() {
        // 刚好超过 retention 一秒 → 超期
        let now = 1_700_000_000;
        let retention = 864_000;
        let deleted_at = now - retention - 1;
        assert!(is_tombstone_expired(retention, deleted_at, now));
    }
}
