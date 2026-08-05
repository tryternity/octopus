// db/prompts.rs —— 润色提示词 CRUD（prompts 表）。

use super::{ensure_db, save_config_key, with_db, Connection, Result, params};
use anyhow::Context;

// ── 润色提示词 CRUD（prompts 表）──

/// prompts 表记录（设置窗口 prompt 管理页用）。
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PromptRecord {
    pub id: i64,
    pub title: String,
    pub content: String,
    pub description: String,
    pub is_system: bool,
    pub app_bundle_ids: String,   // JSON 数组 ["com.tencent.xinWeChat"]，空=全局
    pub inject_context: bool,     // 0=不注入 app 上下文，1=注入
}

const PROMPT_SELECT_COLS: &str = "id, title, content, description, is_system, app_bundle_ids, inject_context";

fn row_to_prompt(row: &rusqlite::Row) -> rusqlite::Result<PromptRecord> {
    Ok(PromptRecord {
        id: row.get(0)?,
        title: row.get(1)?,
        content: row.get(2)?,
        description: row.get(3)?,
        is_system: row.get::<_, i32>(4)? != 0,
        app_bundle_ids: row.get(5)?,
        inject_context: row.get::<_, i32>(6)? != 0,
    })
}

/// 列出所有 prompt（按 is_system 降序、id 升序）。
fn list_prompts_at(conn: &Connection) -> Result<Vec<PromptRecord>> {
    let sql = format!(
        "SELECT {} FROM prompts ORDER BY is_system DESC, id ASC",
        PROMPT_SELECT_COLS
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map([], row_to_prompt)?;
    let mut list = Vec::new();
    for r in rows {
        list.push(r?);
    }
    Ok(list)
}

pub fn list_prompts() -> Result<Vec<PromptRecord>> {
    ensure_db()?;
    with_db(list_prompts_at)
}

/// 按 id 加载单条 prompt。
fn load_prompt_at(conn: &Connection, id: i64) -> Result<Option<PromptRecord>> {
    let sql = format!("SELECT {} FROM prompts WHERE id=?1", PROMPT_SELECT_COLS);
    let mut stmt = conn.prepare(&sql)?;
    let mut rows = stmt.query_map(params![id], row_to_prompt)?;
    Ok(rows.next().transpose()?)
}

pub fn load_prompt(id: i64) -> Result<Option<PromptRecord>> {
    ensure_db()?;
    with_db(|conn| load_prompt_at(conn, id))
}

/// 新建用户 prompt。返回新 id。is_system 固定 0（用户 prompt）。
fn insert_prompt_at(
    conn: &Connection,
    title: &str,
    content: &str,
    description: &str,
    app_bundle_ids: &str,
    inject_context: bool,
) -> Result<i64> {
    conn.execute(
        "INSERT INTO prompts (title, category, content, description, is_system, app_bundle_ids, inject_context)
         VALUES (?1, 'voice_text_polish', ?2, ?3, 0, ?4, ?5)",
        params![title, content, description, app_bundle_ids, inject_context as i32],
    )?;
    Ok(conn.last_insert_rowid())
}

pub fn insert_prompt(
    title: &str,
    content: &str,
    description: &str,
    app_bundle_ids: &str,
    inject_context: bool,
) -> Result<i64> {
    ensure_db()?;
    with_db(|conn| insert_prompt_at(conn, title, content, description, app_bundle_ids, inject_context))
}

/// 按 id 更新 prompt（允许 system prompt 编辑——配合「复原默认」按钮）。
/// 注意：UPDATE 语句不修改 is_system 字段，即系统/用户身份保持不变。
fn update_prompt_at(
    conn: &Connection,
    id: i64,
    title: &str,
    content: &str,
    description: &str,
    app_bundle_ids: &str,
    inject_context: bool,
) -> Result<()> {
    conn.execute(
        "UPDATE prompts SET title=?1, content=?2, description=?3, app_bundle_ids=?4, inject_context=?5, updated_at=datetime('now')
         WHERE id=?6",
        params![title, content, description, app_bundle_ids, inject_context as i32, id],
    )?;
    Ok(())
}

pub fn update_prompt(
    id: i64,
    title: &str,
    content: &str,
    description: &str,
    app_bundle_ids: &str,
    inject_context: bool,
) -> Result<()> {
    ensure_db()?;
    with_db(|conn| update_prompt_at(conn, id, title, content, description, app_bundle_ids, inject_context))
}

/// 按 id 删除 prompt（拒绝 is_system=1）。
fn delete_prompt_at(conn: &Connection, id: i64) -> Result<()> {
    let is_system: i32 = conn
        .query_row("SELECT is_system FROM prompts WHERE id=?1", params![id], |r| r.get(0))
        .context("prompt 不存在")?;
    if is_system != 0 {
        anyhow::bail!("系统内置 prompt 不可删除");
    }
    conn.execute("DELETE FROM prompts WHERE id=?1", params![id])?;
    Ok(())
}

pub fn delete_prompt(id: i64) -> Result<()> {
    ensure_db()?;
    with_db(|conn| delete_prompt_at(conn, id))
}

/// 读取 active_polish_prompt 配置值（字符串 id）。不存在/解析失败返回 1（fallback）。
pub fn load_active_prompt_id() -> Result<i64> {
    ensure_db()?;
    with_db(|conn| {
        let val: Option<String> = conn
            .query_row(
                "SELECT config_value FROM app_config WHERE config_key='active_polish_prompt'",
                [],
                |r| r.get(0),
            )
            .ok();
        let id = val
            .as_deref()
            .and_then(|s| s.parse::<i64>().ok())
            .unwrap_or(1);
        Ok(id)
    })
}

/// 写入 active_polish_prompt 配置值。
pub fn save_active_prompt_id(id: i64) -> Result<()> {
    save_config_key("active_polish_prompt", &id.to_string())
}

/// 按 bundle_id 找关联模板（app_bundle_ids JSON 数组 LIKE 匹配，取 updated_at 最新）。
/// 无匹配返回 None。用于应用感知润色路由。
fn find_prompt_by_bundle_id_at(conn: &Connection, bundle_id: &str) -> Result<Option<PromptRecord>> {
    // LIKE %bid%：app_bundle_ids 是 JSON 数组，子串匹配即可（bundle_id 不含 % _ 等特殊字符）
    let pat = format!("%{}%", bundle_id);
    let sql = format!(
        "SELECT {} FROM prompts WHERE category='voice_text_polish' AND app_bundle_ids LIKE ?1
         ORDER BY updated_at DESC LIMIT 1",
        PROMPT_SELECT_COLS
    );
    let mut stmt = conn.prepare(&sql)?;
    let mut rows = stmt.query_map(params![pat], row_to_prompt)?;
    Ok(rows.next().transpose()?)
}

/// 按 bundle_id 找关联模板（pub 入口，走全局 DB）。
pub fn find_prompt_by_bundle_id(bundle_id: &str) -> Result<Option<PromptRecord>> {
    ensure_db()?;
    with_db(|conn| find_prompt_by_bundle_id_at(conn, bundle_id))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::test_support::{open_init, setup_test_db};

    #[test]
    fn prompts_table_seeded_with_default() {
        let conn = open_init();
        // prompts seed 已外置到 seeds/prompts/（v40 后 db.sql 不再内联），
        // init_schema 在生产路径会调 load_external_seeds——测试里显式调一次。
        crate::seeds::load_external_seeds(&conn).unwrap();
        // id=1 系统默认 prompt 存在
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM prompts WHERE id=1 AND is_system=1", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 1, "应有 id=1 的系统默认 prompt");
        // total 至少 1 条
        let total: i64 = conn
            .query_row("SELECT COUNT(*) FROM prompts", [], |r| r.get(0))
            .unwrap();
        assert!(total >= 1);
        // active_polish_prompt 配置项存在，默认值 '1'
        let val: String = conn
            .query_row(
                "SELECT config_value FROM app_config WHERE config_key='active_polish_prompt'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(val, "1");
    }

    #[test]
    fn prompts_table_init_sql_idempotent() {
        let conn = open_init();
        // db.sql 不再内联 prompts seed——通过外置 loader 加载，二次调用幂等（OR IGNORE）。
        crate::seeds::load_external_seeds(&conn).unwrap();
        crate::seeds::load_external_seeds(&conn).unwrap();
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM prompts WHERE id=1", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 1, "重跑外置 seed loader 不应重复 seed");
    }

    #[test]
    fn prompt_crud_round_trip() {
        let conn = open_init();
        // prompts seed 已外置到 seeds/prompts/——通过 loader 加载初始 3 条。
        crate::seeds::load_external_seeds(&conn).unwrap();
        // list 初值：3 条系统内置（id=1 忠实校对 + id=2 意图整理 + id=3 口语化整理）
        let list = list_prompts_at(&conn).unwrap();
        assert_eq!(list.len(), 3, "seed 应有 3 条系统内置 prompt");
        assert!(list[0].is_system);
        assert_eq!(list[0].title, "忠实校对");
        assert!(list[1].is_system);
        assert_eq!(list[1].title, "意图整理");
        assert!(list[2].is_system);
        assert_eq!(list[2].title, "场景自适应");

        // insert 用户 prompt（id 应大于 seed 最大 id）
        let id = insert_prompt_at(&conn, "技术写作", "rule1", "desc1", "", false).unwrap();
        assert!(id > 3, "用户 prompt id 应大于 seed 最大 id(3)");

        // load
        let loaded = load_prompt_at(&conn, id).unwrap().unwrap();
        assert_eq!(loaded.title, "技术写作");
        assert_eq!(loaded.content, "rule1");
        assert!(!loaded.is_system);

        // update（用户 prompt 可改）
        update_prompt_at(&conn, id, "技术写作V2", "rule2", "desc2", "", false).unwrap();
        let updated = load_prompt_at(&conn, id).unwrap().unwrap();
        assert_eq!(updated.title, "技术写作V2");
        assert_eq!(updated.content, "rule2");

        // update 系统 prompt 现在允许（配合「复原默认」按钮：编辑/复原都走 update）
        // 详见 update_prompt_at_allows_system_prompt 用例的完整断言。

        // delete 系统 prompt 被拒
        assert!(delete_prompt_at(&conn, 1).is_err());

        // delete 用户 prompt 成功
        delete_prompt_at(&conn, id).unwrap();
        assert!(load_prompt_at(&conn, id).unwrap().is_none());

        // delete 不存在的 id
        assert!(delete_prompt_at(&conn, 999).is_err());
    }

    #[test]
    fn prompt_title_allows_duplicate() {
        let conn = open_init();
        // 插入两条同名用户 prompt（title 允许重复）
        insert_prompt_at(&conn, "同名", "a", "", "", false).unwrap();
        insert_prompt_at(&conn, "同名", "b", "", "", false).unwrap();
        let list = list_prompts_at(&conn).unwrap();
        let dup_count = list.iter().filter(|p| p.title == "同名").count();
        assert_eq!(dup_count, 2, "title 允许重复");
    }

    /// update_prompt_at 允许更新 system prompt（is_system 字段保持不变）。
    /// 历史：曾因「不可编辑」bail，移除拒绝以支持「复原默认」按钮（先编辑再保存）。
    #[test]
    fn update_prompt_at_allows_system_prompt() {
        let conn = open_init();
        // open_init 只建表，不 seed——需手动加载外部 seed（id=1/2/3 系统 prompt）
        crate::seeds::load_external_seeds(&conn).unwrap();
        // seed 后 id=1 是系统内置（忠实校对）
        let before = load_prompt_at(&conn, 1).unwrap().unwrap();
        assert!(before.is_system, "seed id=1 应是 is_system=true");

        // 更新系统 prompt 成功
        update_prompt_at(&conn, 1, "改过的标题", "改过的内容", "改过的描述", "", false).unwrap();
        let updated = load_prompt_at(&conn, 1).unwrap().unwrap();
        assert_eq!(updated.title, "改过的标题");
        assert_eq!(updated.content, "改过的内容");
        assert_eq!(updated.description, "改过的描述");
        assert!(updated.is_system, "is_system 字段应保持 true（不被翻转）");
    }

    /// 应用感知路由：按 bundle_id 查关联 prompt（app_bundle_ids LIKE 匹配）。
    #[test]
    fn find_prompt_by_bundle_id_returns_matching() {
        let conn = open_init();
        crate::seeds::load_external_seeds(&conn).unwrap();
        // 无任何 app 关联的 seed → 查任意 bundle_id 返回 None
        assert!(find_prompt_by_bundle_id_at(&conn, "com.tencent.xinWeChat").unwrap().is_none());
        // 插入一条关联微信的 prompt
        insert_prompt_at(&conn, "微信专用", "weixin-rule", "", r#"["com.tencent.xinWeChat"]"#, true).unwrap();
        let found = find_prompt_by_bundle_id_at(&conn, "com.tencent.xinWeChat").unwrap().unwrap();
        assert_eq!(found.title, "微信专用");
        assert!(found.inject_context);
    }

    /// 不变量：3 个系统内置 seed 模板固定全局（app_bundle_ids=''）+ 固定 inject_context。
    /// 系统模板保持「全局 fallback」角色，不可绑特定 app——前端「路由配置」按钮灰禁，
    /// 后端 update_prompt 命令对 is_system=true 回写 DB 现有值（忽略传入）。本测试守护 seed 值。
    #[test]
    fn system_prompts_locked_global_routing() {
        let conn = open_init();
        crate::seeds::load_external_seeds(&conn).unwrap();
        let seeds = list_prompts_at(&conn).unwrap();
        let system_seeds: Vec<_> = seeds.iter().filter(|p| p.is_system).collect();
        assert_eq!(system_seeds.len(), 3, "应有 3 个系统内置 seed");
        for p in &system_seeds {
            assert_eq!(p.app_bundle_ids, "", "系统模板 {} 应全局（app_bundle_ids 空）", p.title);
        }
        // inject_context：app-casual（content=润色-场景自适应）=1，faithful/user-intent=0
        let casual = system_seeds.iter().find(|p| p.content == "润色-场景自适应").unwrap();
        assert!(casual.inject_context, "app-casual seed inject_context 应=1");
        let others: Vec<_> = system_seeds.iter().filter(|p| p.content != "润色-场景自适应").collect();
        assert!(others.iter().all(|p| !p.inject_context), "faithful/user-intent seed inject_context 应=0");
    }

    #[test]
    fn active_prompt_id_roundtrip() {
        setup_test_db();
        save_active_prompt_id(42).unwrap();
        assert_eq!(load_active_prompt_id().unwrap(), 42);
    }
}
