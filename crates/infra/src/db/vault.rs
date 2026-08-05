// db/vault.rs —— Password Vault 模型（schema v38，2026-07-18）。
//
// 表：vault_meta / vault_ciphers / vault_folders。
//
// 双层 API 模式（同 ActionBarItem）：
// - 公开 `with_db` 包装函数（业务层调用，单连接线程安全）
// - 私有 `_at` 内部函数（接 `&Connection`，便于测试和复用）

use super::{ensure_db, with_db, Connection, Result, params};

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct VaultMeta {
    pub id: i64,
    pub kdf_type: i64,
    pub kdf_salt: Vec<u8>,
    pub kdf_iterations: i64,
    pub kdf_memory_kib: i64,
    pub kdf_parallelism: i64,
    pub protected_user_vault_key: String,
    pub app_key_local_enc: String,
    pub app_key_sync_enc: String,
    pub security_stamp: String,
    pub equivalent_domains: String,
    pub public_key: Option<String>,
    pub protected_private_key: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct VaultCipher {
    pub id: String, // UUID v4 字符串（2026-07-21 v39：支持 git 同步）
    pub folder_id: Option<String>,
    pub favorite: bool,
    pub atype: i64,
    pub name: String,
    pub notes: Option<String>,
    pub data: String,
    pub fields: Option<String>,
    pub password_history: Option<String>,
    pub reprompt: i64,
    /// 软删标记（schema v60 起 i64）：0=活跃，>0=删除时刻 epoch 秒（tombstone）。
    /// 与 hotword/clipboard 的 tombstone 语义统一，sync merge 据此传播删除意图。
    pub is_deleted: i64,
    pub sync_md5: Option<String>, // md5 内容指纹（v45：增量同步 diff，详见 vault::sync::fingerprint）
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct VaultFolder {
    pub id: String, // UUID v4 字符串
    pub name: String,
    pub sort_order: i64,
    /// 软删标记（schema v60 起 i64）：0=活跃，>0=删除时刻 epoch 秒（tombstone）。
    pub is_deleted: i64,
    pub sync_md5: Option<String>, // md5 内容指纹（v45）
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone)]
pub struct VaultMetaInput {
    pub kdf_type: i64,
    pub kdf_salt: Vec<u8>,
    pub kdf_iterations: i64,
    pub kdf_memory_kib: i64,
    pub kdf_parallelism: i64,
    pub protected_user_vault_key: String,
    pub app_key_local_enc: String,
    pub app_key_sync_enc: String,
    pub security_stamp: String,
    pub equivalent_domains: String,
    pub public_key: Option<String>,
    pub protected_private_key: Option<String>,
}

/// 从 VaultMeta（DB 行）构造 VaultMetaInput（DB 写入），丢弃 id/created_at/updated_at。
///
/// 2026-08-05 抽取（vault 审查问题 2）：消除 vault crate 6+ 处逐字构造重复。
/// 调用方可用 struct update syntax 覆盖个别字段（如 `VaultMetaInput::from(&meta) {
/// protected_user_vault_key: new_key, .. }`）。
impl From<&VaultMeta> for VaultMetaInput {
    fn from(m: &VaultMeta) -> Self {
        Self {
            kdf_type: m.kdf_type,
            kdf_salt: m.kdf_salt.clone(),
            kdf_iterations: m.kdf_iterations,
            kdf_memory_kib: m.kdf_memory_kib,
            kdf_parallelism: m.kdf_parallelism,
            protected_user_vault_key: m.protected_user_vault_key.clone(),
            app_key_local_enc: m.app_key_local_enc.clone(),
            app_key_sync_enc: m.app_key_sync_enc.clone(),
            security_stamp: m.security_stamp.clone(),
            equivalent_domains: m.equivalent_domains.clone(),
            public_key: m.public_key.clone(),
            protected_private_key: m.protected_private_key.clone(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct VaultCipherInput {
    pub id: String, // UUID v4 字符串——调用方生成（不再 AUTOINCREMENT）
    pub folder_id: Option<String>,
    pub favorite: bool,
    pub atype: i64,
    pub name: String,
    pub notes: Option<String>,
    pub data: String,
    pub fields: Option<String>,
    pub password_history: Option<String>,
    pub reprompt: i64,
    /// 软删除标志（H2 修复 2026-07-24）：sync pull/clone 必须从文件取值传入，
    /// 否则软删密码在新机 clone 时复活（is_deleted=0），且跨设备软删状态不同步。
    /// 软删除（sync 用，schema v60 起 i64）：0=活跃，>0=删除时刻 epoch 秒（tombstone）。
    /// 本机 soft_delete/restore 走专用 UPDATE 路径（不经此结构），此处仅 sync 用。
    pub is_deleted: i64,
    /// md5 内容指纹（v45：增量同步 diff，由调用方算好传入）。
    /// None 表示调用方未算（向后兼容旧调用方），sync 时按需重算。
    pub sync_md5: Option<String>,
}

const VAULT_META_COLS: &str = "id, kdf_type, kdf_salt, kdf_iterations, kdf_memory_kib, kdf_parallelism, \
                               protected_user_vault_key, app_key_local_enc, app_key_sync_enc, security_stamp, \
                               equivalent_domains, public_key, protected_private_key, created_at, updated_at";

const VAULT_CIPHER_COLS: &str = "id, folder_id, favorite, atype, name, notes, data, fields, \
                                 password_history, reprompt, is_deleted, sync_md5, created_at, updated_at";

fn row_to_vault_meta(row: &rusqlite::Row) -> rusqlite::Result<VaultMeta> {
    Ok(VaultMeta {
        id: row.get(0)?,
        kdf_type: row.get(1)?,
        kdf_salt: row.get(2)?,
        kdf_iterations: row.get(3)?,
        kdf_memory_kib: row.get(4)?,
        kdf_parallelism: row.get(5)?,
        protected_user_vault_key: row.get(6)?,
        app_key_local_enc: row.get(7)?,
        app_key_sync_enc: row.get(8)?,
        security_stamp: row.get(9)?,
        equivalent_domains: row.get(10)?,
        public_key: row.get(11)?,
        protected_private_key: row.get(12)?,
        created_at: row.get(13)?,
        updated_at: row.get(14)?,
    })
}

fn row_to_vault_cipher(row: &rusqlite::Row) -> rusqlite::Result<VaultCipher> {
    Ok(VaultCipher {
        id: row.get(0)?,
        folder_id: row.get(1)?,
        favorite: row.get::<_, i32>(2)? != 0,
        atype: row.get(3)?,
        name: row.get(4)?,
        notes: row.get(5)?,
        data: row.get(6)?,
        fields: row.get(7)?,
        password_history: row.get(8)?,
        reprompt: row.get(9)?,
        is_deleted: row.get::<_, i64>(10)?,
        sync_md5: row.get(11)?,
        created_at: row.get(12)?,
        updated_at: row.get(13)?,
    })
}

// ── vault_meta CRUD ──

pub fn load_vault_meta() -> Result<Option<VaultMeta>> {
    ensure_db()?;
    with_db(load_vault_meta_at)
}

pub(crate) fn load_vault_meta_at(conn: &Connection) -> Result<Option<VaultMeta>> {
    let mut stmt = conn.prepare(&format!("SELECT {} FROM vault_meta WHERE id = 1", VAULT_META_COLS))?;
    let mut rows = stmt.query([])?;
    match rows.next()? {
        Some(row) => Ok(Some(row_to_vault_meta(row)?)),
        None => Ok(None),
    }
}

pub fn upsert_vault_meta(input: &VaultMetaInput) -> Result<()> {
    ensure_db()?;
    with_db(|conn| upsert_vault_meta_at(conn, input))
}

pub(crate) fn upsert_vault_meta_at(conn: &Connection, input: &VaultMetaInput) -> Result<()> {
    conn.execute(
        "INSERT INTO vault_meta (id, kdf_type, kdf_salt, kdf_iterations, kdf_memory_kib, kdf_parallelism,
                                  protected_user_vault_key, app_key_local_enc, app_key_sync_enc, security_stamp,
                                  equivalent_domains, public_key, protected_private_key)
         VALUES (1, ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)
         ON CONFLICT(id) DO UPDATE SET
            kdf_type = excluded.kdf_type,
            kdf_salt = excluded.kdf_salt,
            kdf_iterations = excluded.kdf_iterations,
            kdf_memory_kib = excluded.kdf_memory_kib,
            kdf_parallelism = excluded.kdf_parallelism,
            protected_user_vault_key = excluded.protected_user_vault_key,
            app_key_local_enc = excluded.app_key_local_enc,
            app_key_sync_enc = excluded.app_key_sync_enc,
            security_stamp = excluded.security_stamp,
            equivalent_domains = excluded.equivalent_domains,
            public_key = excluded.public_key,
            protected_private_key = excluded.protected_private_key,
            updated_at = datetime('now')",
        params![
            input.kdf_type,
            input.kdf_salt,
            input.kdf_iterations,
            input.kdf_memory_kib,
            input.kdf_parallelism,
            input.protected_user_vault_key,
            input.app_key_local_enc,
            input.app_key_sync_enc,
            input.security_stamp,
            input.equivalent_domains,
            input.public_key,
            input.protected_private_key,
        ],
    )?;
    Ok(())
}

pub fn update_vault_security_stamp(stamp: &str) -> Result<()> {
    ensure_db()?;
    with_db(|conn| update_vault_security_stamp_at(conn, stamp))
}

pub(crate) fn update_vault_security_stamp_at(conn: &Connection, stamp: &str) -> Result<()> {
    conn.execute(
        "UPDATE vault_meta SET security_stamp = ?1, updated_at = datetime('now') WHERE id = 1",
        params![stamp],
    )?;
    Ok(())
}

/// 删除 vault_meta 单行（id=1）。
///
/// ⚠️ **危险操作 / 卫生约束（第六轮审查）**：此函数语义是「销毁整个 vault 元数据」，
/// 正常流程**绝不**调用。唯一合法调用方是 `octopus_vault::unlock::setup_vault`
/// 在迁移失败时的显式回滚路径（A1 修复，第五轮审查）。
///
/// `#[doc(hidden)]` 是语言层软约束——vault 是独立 crate，必须 `pub` 才能跨 crate
/// 调用，无法降为 `pub(crate)`。hidden 标记让该函数不出现在 rustdoc / IDE 自动补全
/// 中，防其他模块（或误用）发现后调用清空 vault。未来若 vault 收编回 infra 或
/// 出现其他合法调用方，可重新评估可见性。
///
/// 背景语义：setup 流程先把 vault_meta INSERT 落盘（独立 commit），再触发
/// `migrate_secret_keys_to_encrypted`。若迁移失败，旧实现会让 vault_meta
/// 已初始化 + secret_key 仍全明文 + `ensure!(!is_initialized())` 阻止重跑 →
/// 不可恢复的「已初始化但部分明文」状态。此函数让 setup 失败路径能显式清掉
/// vault_meta，让 `is_initialized()` 回到 false，用户可重新走 setup。
#[doc(hidden)]
pub fn delete_vault_meta_row() -> Result<()> {
    ensure_db()?;
    with_db(|conn| {
        conn.execute("DELETE FROM vault_meta WHERE id = 1", [])?;
        Ok(())
    })
}

// ── vault_ciphers CRUD ──

pub fn list_vault_ciphers() -> Result<Vec<VaultCipher>> {
    ensure_db()?;
    with_db(list_vault_ciphers_at)
}

pub(crate) fn list_vault_ciphers_at(conn: &Connection) -> Result<Vec<VaultCipher>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {} FROM vault_ciphers ORDER BY updated_at DESC",
        VAULT_CIPHER_COLS
    ))?;
    let rows = stmt.query_map([], row_to_vault_cipher)?;
    let mut list = Vec::new();
    for r in rows {
        list.push(r?);
    }
    Ok(list)
}

pub fn load_vault_cipher(id: &str) -> Result<Option<VaultCipher>> {
    ensure_db()?;
    with_db(|conn| load_vault_cipher_at(conn, id))
}

pub fn load_vault_cipher_at(conn: &Connection, id: &str) -> Result<Option<VaultCipher>> {
    let mut stmt = conn.prepare(&format!("SELECT {} FROM vault_ciphers WHERE id = ?1", VAULT_CIPHER_COLS))?;
    let mut rows = stmt.query(params![id])?;
    match rows.next()? {
        Some(row) => Ok(Some(row_to_vault_cipher(row)?)),
        None => Ok(None),
    }
}

/// 查所有软删 cipher 的 id（is_deleted > 0），轻量查询——不解密、不读字段，
/// 仅供 `vault_empty_trash` 批量永久删除用。
pub fn list_trash_cipher_ids() -> Result<Vec<String>> {
    ensure_db()?;
    with_db(|conn| {
        let mut stmt = conn.prepare("SELECT id FROM vault_ciphers WHERE is_deleted > 0")?;
        let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
        let mut ids = Vec::new();
        for r in rows {
            ids.push(r?);
        }
        Ok(ids)
    })
}

pub fn insert_vault_cipher(input: &VaultCipherInput) -> Result<()> {
    ensure_db()?;
    with_db(|conn| insert_vault_cipher_at(conn, input))
}

/// 批量插入 cipher（L8 修复，2026-07-24）——事务化，全成功或全回滚。
///
/// 用于 Bitwarden import：之前逐条 `insert_vault_cipher` 各自 autocommit，
/// 中途失败留部分数据。现在包一个 `unchecked_transaction`，任一失败 → 整批回滚。
/// 调用方应在循环阶段先过滤掉加密失败的条目（加密是纯内存操作，不破坏事务），
/// 只把加密成功的传进来 batch insert。
pub fn insert_vault_ciphers_batch(inputs: &[VaultCipherInput]) -> Result<()> {
    ensure_db()?;
    with_db(|conn| {
        let tx = conn.unchecked_transaction()?;
        for input in inputs {
            insert_vault_cipher_at(&tx, input)?;
        }
        tx.commit()?;
        Ok(())
    })
}

pub(crate) fn insert_vault_cipher_at(conn: &Connection, input: &VaultCipherInput) -> Result<()> {
    conn.execute(
        "INSERT INTO vault_ciphers (id, folder_id, favorite, atype, name, notes, data, fields, password_history, reprompt, is_deleted, sync_md5)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
        params![
            input.id,
            input.folder_id,
            input.favorite as i32,
            input.atype,
            input.name,
            input.notes,
            input.data,
            input.fields,
            input.password_history,
            input.reprompt,
            input.is_deleted,
            input.sync_md5,
        ],
    )?;
    Ok(())
}

pub fn update_vault_cipher(id: &str, input: &VaultCipherInput) -> Result<()> {
    ensure_db()?;
    with_db(|conn| update_vault_cipher_at(conn, id, input))
}

/// M-CIPHER-RMW 修复（2026-07-25）：改 pub 供 vault crate 的 save_cipher 在
/// 事务内调用（load_vault_cipher_at + update_vault_cipher_at 合并单事务，
/// 防 load→update 间隙并发致软删 cipher 复活——与 #4 meta_lock 同构问题）。
pub fn update_vault_cipher_at(conn: &Connection, id: &str, input: &VaultCipherInput) -> Result<()> {
    conn.execute(
        "UPDATE vault_ciphers SET
            folder_id = ?1, favorite = ?2, atype = ?3, name = ?4, notes = ?5, data = ?6,
            fields = ?7, password_history = ?8, reprompt = ?9, is_deleted = ?10, sync_md5 = ?11, updated_at = datetime('now')
         WHERE id = ?12",
        params![
            input.folder_id,
            input.favorite as i32,
            input.atype,
            input.name,
            input.notes,
            input.data,
            input.fields,
            input.password_history,
            input.reprompt,
            input.is_deleted,
            input.sync_md5,
            id,
        ],
    )?;
    Ok(())
}

// ── sync-only cipher upsert（保留远程时间戳，第十一轮 P1 修复）──
//
// 与业务版 `insert_vault_cipher_at` / `update_vault_cipher_at` 的区别：
// 业务版硬编 `updated_at = datetime('now')`（本机编辑刷新时间戳，标记「我改了」），
// 但 sync pull 路径复用业务版会丢失远程时间戳 → 跨设备 ping-pong（详见 spec 第十一轮 P1）。
// sync 版显式写入 row.created_at / row.updated_at（来自 .sync 文件的远程值），
// 让「最后修改者」的时间戳跨设备存活，merge 的 updated_ms 比对才能收敛。
//
// 数据源：row: &VaultCipher 直接来自 `CipherFile::to_vault_cipher()`（已含远程时间戳），
// 不经 VaultCipherInput（丢时间戳），调用方算好 sync_md5 填 row.sync_md5。

pub fn insert_vault_cipher_sync_at(conn: &Connection, row: &VaultCipher) -> Result<()> {
    conn.execute(
        "INSERT INTO vault_ciphers (id, folder_id, favorite, atype, name, notes, data, fields,
            password_history, reprompt, is_deleted, sync_md5, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
        params![
            row.id,
            row.folder_id,
            row.favorite as i32,
            row.atype,
            row.name,
            row.notes,
            row.data,
            row.fields,
            row.password_history,
            row.reprompt,
            row.is_deleted,
            row.sync_md5,
            row.created_at,
            row.updated_at,
        ],
    )?;
    Ok(())
}

pub fn update_vault_cipher_sync_at(conn: &Connection, id: &str, row: &VaultCipher) -> Result<()> {
    conn.execute(
        "UPDATE vault_ciphers SET
            folder_id = ?1, favorite = ?2, atype = ?3, name = ?4, notes = ?5, data = ?6,
            fields = ?7, password_history = ?8, reprompt = ?9, is_deleted = ?10, sync_md5 = ?11,
            created_at = ?12, updated_at = ?13
         WHERE id = ?14",
        params![
            row.folder_id,
            row.favorite as i32,
            row.atype,
            row.name,
            row.notes,
            row.data,
            row.fields,
            row.password_history,
            row.reprompt,
            row.is_deleted,
            row.sync_md5,
            row.created_at,
            row.updated_at,
            id,
        ],
    )?;
    Ok(())
}

pub fn permanent_delete_vault_cipher(id: &str) -> Result<()> {
    ensure_db()?;
    with_db(|conn| permanent_delete_vault_cipher_at(conn, id))
}

pub(crate) fn permanent_delete_vault_cipher_at(conn: &Connection, id: &str) -> Result<()> {
    conn.execute("DELETE FROM vault_ciphers WHERE id = ?1", params![id])?;
    Ok(())
}

// ── tombstone GC（2026-08-05，对称 hotword/clipboard GC）──软删 cipher/folder 超期后硬删 ──

/// vault tombstone 保留期：30 天（对称 `CLIPBOARD_TOMBSTONE_RETENTION_SECS`）。
///
/// 超期 cipher/folder tombstone 被 GC 硬删 + sync merge 拒绝复活（pull 拒绝超期
/// tombstone，跨设备自洽——对端旧 outline 有也 skip）。详见 pipeline.rs 的
/// `is_tombstone_expired` 与 spec §7.2。
pub const VAULT_TOMBSTONE_RETENTION_SECS: i64 = 30 * 86400;

/// 硬删超期 vault tombstone——`is_deleted > 0` 且 `now - is_deleted > RETENTION`。
///
/// GC 范围：`vault_ciphers` + `vault_folders` 的超期 tombstone 行（活跃行不动）。
/// 返回硬删的总行数（cipher + folder）。`now_secs` 是当前 epoch 秒。
///
/// 对应 `.sync` 文件清理由调用方（`SyncEntity::purge_expired_tombstones`）负责——
/// 本函数只管 DB，保持与 hotword `purge_expired_hotword_tombstones_at` /
/// clipboard `purge_expired_clipboard_favorites_at` 的职责对称（DB GC，文件 GC
/// 由 export 重建完成）。
///
/// 跨设备自洽：sync merge 按年龄过滤（pull 拒绝复活超期 tombstone），GC 后 export
/// 不含超期 tombstone → 对端 pull 时即使旧 outline 有也 skip → 收敛。
///
/// **FK 约束**：cipher.folder_id 引用 folder.id——若 folder 被硬删但 cipher 仍在
/// 引用，cipher 的 folder_id 变悬空引用（FK 是 non-deferring，但本表 schema 实际
/// 未声明 FK，仅逻辑约束——见 `vault_ciphers` CREATE TABLE）。删除顺序上先 cipher
/// 后 folder 不影响正确性（cipher 删完后再删 folder，无引用残留），但即使有残留
/// cipher 也无 FK 报错。返回值含两侧硬删总数。
pub fn purge_expired_vault_tombstones(now_secs: i64) -> Result<usize> {
    ensure_db()?;
    with_db(|conn| purge_expired_vault_tombstones_at(conn, now_secs))
}

/// 裸连接版（供测试直接调）。
pub(crate) fn purge_expired_vault_tombstones_at(
    conn: &Connection,
    now_secs: i64,
) -> Result<usize> {
    let cutoff = now_secs - VAULT_TOMBSTONE_RETENTION_SECS;
    // 先 cipher 后 folder（逻辑 FK 顺序——cipher 是 folder 的引用方，
    // 删 cipher tombstone 不影响 folder；反之 folder tombstone 删掉后 cipher 引用
    // 悬空也无害——schema 无 FK 约束）。
    let n_ciphers = conn.execute(
        "DELETE FROM vault_ciphers WHERE is_deleted > 0 AND is_deleted < ?1",
        params![cutoff],
    )?;
    let n_folders = conn.execute(
        "DELETE FROM vault_folders WHERE is_deleted > 0 AND is_deleted < ?1",
        params![cutoff],
    )?;
    let n = n_ciphers + n_folders;
    if n > 0 {
        log::info!(
            "[vault-gc] purged {} tombstones ({} ciphers + {} folders, cutoff={})",
            n,
            n_ciphers,
            n_folders,
            cutoff
        );
    }
    Ok(n)
}

// ── vault_folders CRUD ──

pub fn list_vault_folders() -> Result<Vec<VaultFolder>> {
    ensure_db()?;
    with_db(list_vault_folders_at)
}

/// P-FOLDER-SCAN 修复（2026-07-25）：单条查询 folder（与 load_vault_cipher 对称）。
///
/// 之前 upsert_folder_with_sort 用 list_vault_folders().iter().any() 全表扫判断存在，
/// 每次 upsert 都 O(N) → pull 的 folder 循环 O(N²)。改用本函数 O(1) 单条查询。
pub fn load_vault_folder(id: &str) -> Result<Option<VaultFolder>> {
    ensure_db()?;
    with_db(|conn| load_vault_folder_at(conn, id))
}

/// 事务内单条查询 folder（与 load_vault_cipher_at 对称）。
///
/// 供 storage::folder::delete_folder 在软删事务内 load + 算 md5 + UPDATE sync_md5
/// 单事务原子化（与 cipher 的 soft_delete 同构）。
pub fn load_vault_folder_at(conn: &Connection, id: &str) -> Result<Option<VaultFolder>> {
    let mut stmt = conn.prepare(
        "SELECT id, name, sort_order, is_deleted, sync_md5, created_at, updated_at FROM vault_folders WHERE id = ?1",
    )?;
    let mut rows = stmt.query_map(params![id], |row| {
        Ok(VaultFolder {
            id: row.get(0)?,
            name: row.get(1)?,
            sort_order: row.get(2)?,
            is_deleted: row.get::<_, i64>(3)?,
            sync_md5: row.get(4)?,
            created_at: row.get(5)?,
            updated_at: row.get(6)?,
        })
    })?;
    match rows.next() {
        Some(r) => Ok(Some(r?)),
        None => Ok(None),
    }
}

pub(crate) fn list_vault_folders_at(conn: &Connection) -> Result<Vec<VaultFolder>> {
    let mut stmt = conn.prepare(
        "SELECT id, name, sort_order, is_deleted, sync_md5, created_at, updated_at FROM vault_folders ORDER BY sort_order ASC",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok(VaultFolder {
            id: row.get(0)?,
            name: row.get(1)?,
            sort_order: row.get(2)?,
            is_deleted: row.get::<_, i64>(3)?,
            sync_md5: row.get(4)?,
            created_at: row.get(5)?,
            updated_at: row.get(6)?,
        })
    })?;
    let mut list = Vec::new();
    for r in rows {
        list.push(r?);
    }
    Ok(list)
}

/// 创建 folder（调用方生成 UUID）。
/// 2026-07-21 v39：id 从 AUTOINCREMENT 改 UUID 字符串（git 同步）。
pub fn insert_vault_folder(id: &str, name: &str, sync_md5: &str) -> Result<()> {
    ensure_db()?;
    with_db(|conn| insert_vault_folder_at(conn, id, name, sync_md5))
}

pub(crate) fn insert_vault_folder_at(conn: &Connection, id: &str, name: &str, sync_md5: &str) -> Result<()> {
    conn.execute(
        "INSERT INTO vault_folders (id, name, sync_md5) VALUES (?1, ?2, ?3)",
        params![id, name, sync_md5],
    )?;
    Ok(())
}

/// E5 修复（2026-07-24）：insert folder 含 sort_order（一次写，不再 insert+update 两次）。
/// 第八轮 P0：加 is_deleted 参数——pull 路径需写入文件中的真实软删状态，对齐 cipher。
pub fn insert_vault_folder_with_sort(
    id: &str,
    name: &str,
    sort_order: i64,
    sync_md5: &str,
    is_deleted: i64,
) -> Result<()> {
    ensure_db()?;
    with_db(|conn| {
        conn.execute(
            "INSERT INTO vault_folders (id, name, sort_order, sync_md5, is_deleted) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![id, name, sort_order, sync_md5, is_deleted],
        )?;
        Ok(())
    })
}

/// 重命名 folder（参数应是已用 user_vault_key.encrypt 加密过的密文）。
///
/// follow-up #6：folder.name 与 cipher.name 一致存密文；调用方负责加解密。
/// sync_md5 由调用方算好传入（name 变 → md5 变）。
pub fn update_vault_folder_name(id: &str, new_name_encrypted: &str, sync_md5: &str) -> Result<()> {
    ensure_db()?;
    with_db(|conn| {
        conn.execute(
            "UPDATE vault_folders SET name = ?1, sync_md5 = ?2, updated_at = datetime('now') WHERE id = ?3",
            params![new_name_encrypted, sync_md5, id],
        )?;
        Ok(())
    })
}

/// 更新 folder 的 name + sort_order + sync_md5 + is_deleted（sync pull 用，#6 修复 + P0）。
///
/// 与 `update_vault_folder_name` 的区别：同时更新 sort_order + is_deleted，让远程 folder 的
/// 排序变化 + 软删状态能同步到本地（之前 pull 硬编码 sort_order=0 + 丢 is_deleted，导致
/// 排序永不同步 + 软删 folder 复活）。
/// 返回受影响行数（0 表示 id 不存在——调用方可据此判断）。
pub fn update_vault_folder_fields(
    id: &str,
    new_name_encrypted: &str,
    sort_order: i64,
    sync_md5: &str,
    is_deleted: i64,
) -> Result<usize> {
    ensure_db()?;
    with_db(|conn| {
        let affected = conn.execute(
            "UPDATE vault_folders SET name = ?1, sort_order = ?2, sync_md5 = ?3, is_deleted = ?5, updated_at = datetime('now') WHERE id = ?4",
            params![new_name_encrypted, sort_order, sync_md5, id, is_deleted],
        )?;
        Ok(affected)
    })
}

/// Upsert folder 含 is_deleted（sync pull 用——传播跨设备软删状态）。
///
/// 与 `update_vault_folder_fields` 的区别：同时设置 `is_deleted`——
/// 远程 tombstone folder pull 到本地时，必须把 `is_deleted` 也写进 DB，
/// 否则删除状态不会传播（2026-08-05 folder tombstone 传播 bug 修复）。
pub fn upsert_vault_folder_sync(
    id: &str,
    encrypted_name: &str,
    sort_order: i64,
    sync_md5: &str,
    is_deleted: i64,
) -> Result<()> {
    ensure_db()?;
    with_db(|conn| {
        let existing: Option<i64> = conn
            .query_row(
                "SELECT 1 FROM vault_folders WHERE id = ?1",
                params![id],
                |row| row.get(0),
            )
            .ok();
        if existing.is_some() {
            conn.execute(
                "UPDATE vault_folders SET name = ?1, sort_order = ?2, sync_md5 = ?3, is_deleted = ?4, updated_at = datetime('now') WHERE id = ?5",
                params![encrypted_name, sort_order, sync_md5, is_deleted, id],
            )?;
        } else {
            conn.execute(
                "INSERT INTO vault_folders (id, name, sort_order, sync_md5, is_deleted) VALUES (?1, ?2, ?3, ?4, ?5)",
                params![id, encrypted_name, sort_order, sync_md5, is_deleted],
            )?;
        }
        Ok(())
    })
}

// ── sync-only folder upsert（保留远程时间戳，第十一轮 P1 修复）──
//
// 同 cipher 的 sync 版：业务版 `update_vault_folder_fields` 硬编 `updated_at = datetime('now')`，
// sync pull 复用会丢远程时间戳 → folder 改名/排序跨设备 ping-pong。
// sync 版显式写 row.created_at / row.updated_at（来自 .sync 文件的远程值）。
// 与第八轮 P0（folder 软删同步）的 `upsert_folder_with_sort` 区别：后者仍硬编 now，
// 仅解决 is_deleted 传播；本版同时解决时间戳收敛。

pub fn insert_vault_folder_sync_at(conn: &Connection, row: &VaultFolder) -> Result<()> {
    conn.execute(
        "INSERT INTO vault_folders (id, name, sort_order, is_deleted, sync_md5, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![
            row.id,
            row.name,
            row.sort_order,
            row.is_deleted,
            row.sync_md5,
            row.created_at,
            row.updated_at,
        ],
    )?;
    Ok(())
}

pub fn update_vault_folder_sync_at(conn: &Connection, id: &str, row: &VaultFolder) -> Result<usize> {
    let affected = conn.execute(
        "UPDATE vault_folders SET
            name = ?1, sort_order = ?2, is_deleted = ?3, sync_md5 = ?4,
            created_at = ?5, updated_at = ?6
         WHERE id = ?7",
        params![
            row.name,
            row.sort_order,
            row.is_deleted,
            row.sync_md5,
            row.created_at,
            row.updated_at,
            id,
        ],
    )?;
    Ok(affected)
}

/// 软删除 folder（统一 cipher+folder 语义，2026-07-27 v53）。
///
/// 仅打 `is_deleted=1` 标记——行仍在表里，sync 走标准 merge 路径传播删除状态。
/// cipher.folder_id 仍指向此 folder（FK 不触发 SET NULL，因为不是 DELETE）——
/// list_folders 在 storage 层过滤 is_deleted=0，UI 看不到软删 folder。
///
/// **注意**：sync_md5 重算 + 单事务原子性由上层 `storage::folder::delete_folder` 负责
/// （与 cipher 的 soft_delete 对称——infra 不依赖 vault，无法直接算 folder_md5）。
///
/// follow-up #6 / spec §1.2。
pub fn delete_vault_folder(id: &str) -> Result<()> {
    ensure_db()?;
    with_db(|conn| {
        // schema v60：is_deleted 存删除时刻 epoch 秒（与 hotword/clipboard tombstone 一致），
        // 不再用字面量 1。
        let now_secs: i64 = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(1);
        conn.execute(
            "UPDATE vault_folders SET is_deleted = ?2, updated_at = datetime('now') WHERE id = ?1",
            params![id, now_secs],
        )?;
        Ok(())
    })
}

/// 返回所有需要迁移的 model：(id, 明文 secret_key)。
/// 仅 source_type=2（cloud）且不以密文前缀（如 `v1:`）开头的行。
///
/// `encrypted_prefix` 由调用方传入（vault crate 传 `crypto::symmetric::CIPHERTEXT_PREFIX`），
/// 而非在此硬编码——M1(b) 修复（2026-07-24）：之前 SQL 守卫字面量 `'v1:%'` 与
/// CIPHERTEXT_PREFIX 是两份独立字面量，未来密文格式升级（v2:）时若只改一处会导致
/// v2 密文被当明文再加密（数据损坏）/ v1 密文漏保护。参数化绑定后调用方传常量，
/// 单点维护。
///
/// 注意：infra 不依赖 vault（依赖方向是 vault → infra），所以不能直接引用
/// CIPHERTEXT_PREFIX 常量，必须由调用方注入。
pub fn list_models_for_secret_migration(encrypted_prefix: &str) -> Result<Vec<(i64, String)>> {
    // SQL LIKE 模式：前缀 + % 通配。前缀本身不含 LIKE 特殊字符（v1: 等），无需 escape。
    let pattern = format!("{}%", encrypted_prefix);
    ensure_db()?;
    with_db(|conn| {
        let mut stmt = conn.prepare(
            "SELECT id, secret_key FROM models WHERE source_type = 2 AND secret_key != '' AND secret_key NOT LIKE ?1",
        )?;
        let rows = stmt.query_map(params![pattern], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
        })?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    })
}

/// 更新指定 model 的 secret_key 字段。
///
/// 注意：models 表无 `updated_at` 列（与 prompts / vault_meta 等带时间戳的表不同）。
/// 早期版本误把 `updated_at = datetime('now')` 写进 UPDATE，导致整个迁移路径抛
/// "no such column: updated_at"——调用方（unlock.rs setup_vault）虽 catch 不阻塞，
/// 但用户的明文 secret_key 永远迁不掉。Wave 2 测试发现此 bug 后修复：仅更新 secret_key。
pub fn update_model_secret_key(model_id: i64, new_secret_key: &str) -> Result<()> {
    ensure_db()?;
    with_db(|conn| {
        conn.execute(
            "UPDATE models SET secret_key = ? WHERE id = ?",
            rusqlite::params![new_secret_key, model_id],
        )?;
        Ok(())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;

    /// 在内存 DB 上执行 db.sql，得到含全部 schema（含 vault v38 表）的连接。
    fn test_db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(crate::resources::db_schema_sql()).unwrap();
        conn.execute("PRAGMA user_version = 43", []).unwrap();
        conn
    }

    #[test]
    fn test_vault_meta_upsert_and_load() {
        let conn = test_db();
        // 全新库无 meta 行
        assert!(load_vault_meta_at(&conn).unwrap().is_none());

        let input = VaultMetaInput {
            kdf_type: 0,
            kdf_salt: vec![1u8; 32],
            kdf_iterations: 3,
            kdf_memory_kib: 65_536,
            kdf_parallelism: 4,
            protected_user_vault_key: "v1:aaa".into(),
            app_key_local_enc: "v1:bbb".into(),
            app_key_sync_enc: "v1:ccc".into(),
            security_stamp: "stamp-1".into(),
            equivalent_domains: "[]".into(),
            public_key: None,
            protected_private_key: None,
        };
        upsert_vault_meta_at(&conn, &input).unwrap();

        let loaded = load_vault_meta_at(&conn).unwrap().unwrap();
        assert_eq!(loaded.id, 1);
        assert_eq!(loaded.kdf_type, 0);
        assert_eq!(loaded.kdf_salt, vec![1u8; 32]);
        assert_eq!(loaded.kdf_iterations, 3);
        assert_eq!(loaded.kdf_memory_kib, 65_536);
        assert_eq!(loaded.kdf_parallelism, 4);
        assert_eq!(loaded.protected_user_vault_key, "v1:aaa");
        assert_eq!(loaded.app_key_local_enc, "v1:bbb");
        assert_eq!(loaded.app_key_sync_enc, "v1:ccc");
        assert_eq!(loaded.security_stamp, "stamp-1");
        assert_eq!(loaded.equivalent_domains, "[]");
        assert!(loaded.public_key.is_none());
        assert!(loaded.protected_private_key.is_none());

        // Upsert（覆盖）—— security_stamp 变更，其他字段保持。
        let mut input2 = input.clone();
        input2.security_stamp = "stamp-2".into();
        upsert_vault_meta_at(&conn, &input2).unwrap();
        let loaded2 = load_vault_meta_at(&conn).unwrap().unwrap();
        assert_eq!(loaded2.security_stamp, "stamp-2");
        // 仍是单行
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM vault_meta", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 1);
    }

    #[test]
    fn test_vault_cipher_crud() {
        let conn = test_db();
        let id = "crud-test-uuid-1";
        let input = VaultCipherInput {
            id: id.to_string(),
            folder_id: None,
            favorite: false,
            atype: 1,
            name: "v1:enc-name".into(),
            notes: None,
            data: "v1:enc-data".into(),
            fields: None,
            password_history: None,
            reprompt: 0,
            is_deleted: 0,
            sync_md5: None,
        };
        insert_vault_cipher_at(&conn, &input).unwrap();

        let loaded = load_vault_cipher_at(&conn, id).unwrap().unwrap();
        assert_eq!(loaded.id, id);
        assert_eq!(loaded.name, "v1:enc-name");
        assert_eq!(loaded.atype, 1);
        assert!(!loaded.favorite);
        assert_eq!(loaded.is_deleted, 0);

        // 更新
        let mut input2 = input.clone();
        input2.name = "v1:enc-name-2".into();
        input2.favorite = true;
        update_vault_cipher_at(&conn, id, &input2).unwrap();
        let loaded2 = load_vault_cipher_at(&conn, id).unwrap().unwrap();
        assert_eq!(loaded2.name, "v1:enc-name-2");
        assert!(loaded2.favorite);

        // 注：软删/恢复（soft_delete/restore）+ sync_md5 原子一致性已在
        // crates/vault/src/storage/cipher.rs::soft_delete_and_restore_update_sync_md5_atomically
        // 完整守护。本测试不再覆盖 db 层 soft_delete/restore（函数已删，见 F1）。

        // 物理删除
        permanent_delete_vault_cipher_at(&conn, id).unwrap();
        assert!(load_vault_cipher_at(&conn, id).unwrap().is_none());
    }

    #[test]
    fn test_vault_meta_check_constraint() {
        let conn = test_db();
        // 尝试插入 id=2 应失败（CHECK id=1）
        let result = conn.execute(
            "INSERT INTO vault_meta (id, kdf_type, kdf_salt, kdf_iterations, kdf_memory_kib, kdf_parallelism,
                                      protected_user_vault_key, app_key_local_enc, app_key_sync_enc, security_stamp)
             VALUES (2, 0, X'00', 0, 0, 0, '', '', '', '')",
            [],
        );
        assert!(result.is_err(), "CHECK(id=1) 应阻止 id=2 的插入");
    }

    // ── purge_expired_vault_tombstones 测试（2026-08-05，对称 hotword/clipboard GC）──

    /// 辅助：插一个 cipher（可指定 is_deleted）。sync_at 路径保留远程时间戳 + is_deleted。
    fn insert_cipher_with_deletion(conn: &Connection, id: &str, is_deleted: i64) {
        let row = VaultCipher {
            id: id.to_string(),
            folder_id: None,
            favorite: false,
            atype: 1,
            name: "v1:n".into(),
            notes: None,
            data: "v1:d".into(),
            fields: None,
            password_history: None,
            reprompt: 0,
            is_deleted,
            sync_md5: None,
            created_at: "2026-08-05 00:00:00".into(),
            updated_at: "2026-08-05 00:00:00".into(),
        };
        insert_vault_cipher_sync_at(conn, &row).unwrap();
    }

    /// 辅助：插一个 folder（可指定 is_deleted）。
    fn insert_folder_with_deletion(conn: &Connection, id: &str, is_deleted: i64) {
        let row = VaultFolder {
            id: id.to_string(),
            name: "v1:n".into(),
            sort_order: 0,
            is_deleted,
            sync_md5: None,
            created_at: "2026-08-05 00:00:00".into(),
            updated_at: "2026-08-05 00:00:00".into(),
        };
        insert_vault_folder_sync_at(conn, &row).unwrap();
    }

    /// purge_expired_vault_tombstones：超期 cipher+folder tombstone 硬删，active + 近期 tombstone 不动。
    #[test]
    fn purge_expired_vault_tombstones_deletes_only_expired() {
        let conn = test_db();
        let now = 1_700_000_000;
        // active（不应删）
        insert_cipher_with_deletion(&conn, "c-active", 0);
        insert_folder_with_deletion(&conn, "f-active", 0);
        // 近期 tombstone（未超期，不应删）
        insert_cipher_with_deletion(&conn, "c-recent", now);
        insert_folder_with_deletion(&conn, "f-recent", now);
        // 超期 tombstone（应删）——删除时刻比 now 早 31 天（> 30 天 retention）
        insert_cipher_with_deletion(&conn, "c-old", now - 31 * 86400);
        insert_folder_with_deletion(&conn, "f-old", now - 31 * 86400);

        let purged = purge_expired_vault_tombstones_at(&conn, now).unwrap();
        assert_eq!(purged, 2, "应硬删 2 条超期 tombstone（1 cipher + 1 folder）");

        // active + 近期 tombstone 仍在
        assert!(load_vault_cipher_at(&conn, "c-active").unwrap().is_some());
        assert!(load_vault_cipher_at(&conn, "c-recent").unwrap().is_some());
        assert!(load_vault_folder_at(&conn, "f-active").unwrap().is_some());
        assert!(load_vault_folder_at(&conn, "f-recent").unwrap().is_some());
        // 超期 tombstone 已删
        assert!(load_vault_cipher_at(&conn, "c-old").unwrap().is_none());
        assert!(load_vault_folder_at(&conn, "f-old").unwrap().is_none());
    }

    /// 恰好等于 retention 不应删（严格 `>` 判定）。
    #[test]
    fn purge_expired_vault_tombstones_boundary_not_expired() {
        let conn = test_db();
        let now = 1_700_000_000;
        insert_cipher_with_deletion(&conn, "c-bound", now - VAULT_TOMBSTONE_RETENTION_SECS);
        insert_folder_with_deletion(&conn, "f-bound", now - VAULT_TOMBSTONE_RETENTION_SECS);

        let purged = purge_expired_vault_tombstones_at(&conn, now).unwrap();
        assert_eq!(purged, 0, "恰好等于 retention 不应删（严格 >）");
        assert!(load_vault_cipher_at(&conn, "c-bound").unwrap().is_some());
        assert!(load_vault_folder_at(&conn, "f-bound").unwrap().is_some());
    }

    /// 无 tombstone（全 active）→ purge 不报错，返 0。
    #[test]
    fn purge_expired_vault_tombstones_no_active_deleted() {
        let conn = test_db();
        insert_cipher_with_deletion(&conn, "c-active-only", 0);
        insert_folder_with_deletion(&conn, "f-active-only", 0);
        let purged = purge_expired_vault_tombstones_at(&conn, 1_700_000_000).unwrap();
        assert_eq!(purged, 0);
    }
}
