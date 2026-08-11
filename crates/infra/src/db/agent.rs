// db/agent.rs —— Agent Adapter + Agent Task（agent_adapters / agent_tasks 表）CRUD。

use super::{ensure_db, with_db, Connection, Result, params};
use anyhow::Context;

// ── Agent Adapter（agent 适配器：内置 + 用户自定义）──────────────────────
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentAdapterRecord {
    pub id: i64,
    pub key: String,
    pub display_name: String,
    pub detect_binary: String,
    pub command_template: String,
    pub is_system: bool,
    pub is_default: bool,
}

const AGENT_ADAPTER_SELECT_COLS: &str = "id, key, display_name, detect_binary, command_template, is_system, is_default";

pub fn list_agent_adapter_records() -> Result<Vec<AgentAdapterRecord>> {
    ensure_db()?;
    with_db(|conn| {
        let mut stmt = conn.prepare(
            &format!("SELECT {} FROM agent_adapters ORDER BY is_system DESC, id ASC", AGENT_ADAPTER_SELECT_COLS)
        )?;
        let rows = stmt.query_map([], |r| Ok(AgentAdapterRecord {
            id: r.get(0)?,
            key: r.get(1)?,
            display_name: r.get(2)?,
            detect_binary: r.get(3)?,
            command_template: r.get(4)?,
            is_system: r.get::<_, i32>(5)? != 0,
            is_default: r.get::<_, i32>(6)? != 0,
        }))?;
        let mut list = Vec::new();
        for r in rows { list.push(r?); }
        Ok(list)
    })
}

pub fn insert_agent_adapter_record(
    key: &str, display_name: &str, detect_binary: &str, command_template: &str,
) -> Result<i64> {
    ensure_db()?;
    with_db(|conn| {
        // 用户自建项 is_system=0；is_default 由 set_default_agent 单独管
        conn.execute(
            "INSERT INTO agent_adapters (key, display_name, detect_binary, command_template, is_system, is_default) VALUES (?1, ?2, ?3, ?4, 0, 0)",
            params![key, display_name, detect_binary, command_template],
        )?;
        Ok(conn.last_insert_rowid())
    })
}

/// 设为默认 agent（全局唯一）。先把全部置 0，再把目标置 1。
pub fn set_default_agent(id: i64) -> Result<()> {
    ensure_db()?;
    with_db(|conn| set_default_agent_at(conn, id))
}

/// 接裸连接版本（供测试用）。
pub(crate) fn set_default_agent_at(conn: &Connection, id: i64) -> Result<()> {
    let exists: i64 = conn.query_row(
        "SELECT COUNT(*) FROM agent_adapters WHERE id=?1", params![id], |r| r.get(0)
    )?;
    if exists == 0 {
        anyhow::bail!("agent adapter id={} 不存在", id);
    }
    // 第二十二轮 P2-i4：清零 + 置 1 两步必须包事务。原先 autocommit 下清零成功、置 1
    // 失败 → 全表无 default。对齐 P2-i1/i2/i3 + insert_vault_ciphers_batch 范式。
    let tx = conn.unchecked_transaction()?;
    tx.execute("UPDATE agent_adapters SET is_default=0", [])?;
    tx.execute("UPDATE agent_adapters SET is_default=1 WHERE id=?1", params![id])?;
    tx.commit()?;
    Ok(())
}

/// 清除默认（无默认 agent；菜单 agent='' 时将走 fallback 到「第一个可用」）。
pub fn clear_default_agent() -> Result<()> {
    ensure_db()?;
    with_db(clear_default_agent_at)
}

/// 接裸连接版本（供测试用）。
pub(crate) fn clear_default_agent_at(conn: &Connection) -> Result<()> {
    conn.execute("UPDATE agent_adapters SET is_default=0", [])?;
    Ok(())
}

pub fn update_agent_adapter_record(
    id: i64, key: &str, display_name: &str, detect_binary: &str, command_template: &str,
) -> Result<()> {
    ensure_db()?;
    with_db(|conn| {
        conn.execute(
            "UPDATE agent_adapters SET key=?1, display_name=?2, detect_binary=?3, command_template=?4, updated_at=datetime('now') WHERE id=?5",
            params![key, display_name, detect_binary, command_template, id],
        )?;
        Ok(())
    })
}

pub fn delete_agent_adapter_record(id: i64) -> Result<()> {
    ensure_db()?;
    with_db(|conn| delete_agent_adapter_record_at(conn, id))
}

/// 接裸连接版本（供测试用）。
pub(crate) fn delete_agent_adapter_record_at(conn: &Connection, id: i64) -> Result<()> {
    // 内置不可删（与 update 对称保护）
    let is_system: i32 = conn.query_row(
        "SELECT is_system FROM agent_adapters WHERE id=?1", params![id], |r| r.get(0)
    ).context("agent adapter 不存在")?;
    if is_system != 0 {
        anyhow::bail!("系统内置 agent 不可删除");
    }
    conn.execute("DELETE FROM agent_adapters WHERE id=?1", params![id])?;
    Ok(())
}

// ── Agent Task（agent × 语音识别联动）──────────────────────
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentTask {
    pub id: String,
    pub status: String,
    pub agent_key: String,
    pub context: String,
    pub transcribed_text: String,
    pub error_msg: String,
    pub created_at: String,
    pub updated_at: String,
}

pub fn insert_agent_task(id: &str, agent_key: &str, context: &str) -> Result<()> {
    ensure_db()?;
    with_db(|conn| {
        conn.execute(
            "INSERT INTO agent_tasks (id, status, agent_key, context) VALUES (?1, 'pending', ?2, ?3)",
            params![id, agent_key, context],
        )?;
        Ok(())
    })
}

pub fn load_agent_task(id: &str) -> Result<Option<AgentTask>> {
    ensure_db()?;
    with_db(|conn| {
        let mut stmt = conn.prepare(
            "SELECT id, status, agent_key, context, transcribed_text, error_msg, created_at, updated_at FROM agent_tasks WHERE id=?1"
        )?;
        let mut rows = stmt.query_map(params![id], |r| Ok(AgentTask {
            id: r.get(0)?, status: r.get(1)?, agent_key: r.get(2)?, context: r.get(3)?,
            transcribed_text: r.get(4)?, error_msg: r.get(5)?, created_at: r.get(6)?, updated_at: r.get(7)?,
        }))?;
        match rows.next() {
            Some(r) => Ok(Some(r?)),
            None => Ok(None),
        }
    })
}

pub fn update_agent_task_result(id: &str, transcribed_text: &str) -> Result<()> {
    ensure_db()?;
    with_db(|conn| {
        conn.execute(
            "UPDATE agent_tasks SET transcribed_text=?1, status='executing', updated_at=datetime('now') WHERE id=?2",
            params![transcribed_text, id],
        )?;
        Ok(())
    })
}

pub fn update_agent_task_status(id: &str, status: &str, error_msg: &str) -> Result<()> {
    ensure_db()?;
    with_db(|conn| {
        conn.execute(
            "UPDATE agent_tasks SET status=?1, error_msg=?2, updated_at=datetime('now') WHERE id=?3",
            params![status, error_msg, id],
        )?;
        Ok(())
    })
}

pub fn list_agent_tasks(limit: i64) -> Result<Vec<AgentTask>> {
    ensure_db()?;
    with_db(|conn| {
        let mut stmt = conn.prepare(
            "SELECT id, status, agent_key, context, transcribed_text, error_msg, created_at, updated_at FROM agent_tasks ORDER BY created_at DESC LIMIT ?1"
        )?;
        let rows = stmt.query_map(params![limit], |r| Ok(AgentTask {
            id: r.get(0)?, status: r.get(1)?, agent_key: r.get(2)?, context: r.get(3)?,
            transcribed_text: r.get(4)?, error_msg: r.get(5)?, created_at: r.get(6)?, updated_at: r.get(7)?,
        }))?;
        let mut list = Vec::new();
        for r in rows { list.push(r?); }
        Ok(list)
    })
}

pub fn delete_agent_task(id: &str) -> Result<()> {
    ensure_db()?;
    with_db(|conn| {
        conn.execute("DELETE FROM agent_tasks WHERE id=?1", params![id])?;
        Ok(())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::test_support::open_init;

    #[test]
    fn agent_adapters_table_exists() {
        let conn = open_init();
        let count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='agent_adapters'",
            [], |r| r.get(0),
        ).unwrap();
        assert_eq!(count, 1, "agent_adapters table should exist");
    }

    #[test]
    fn agent_adapter_crud_roundtrip() {
        let conn = open_init();
        // v42 起 db.sql seed 内置 Pi + Claude（2 行），用 WHERE 过滤到测试项验证 CRUD
        conn.execute(
            "INSERT INTO agent_adapters (key, display_name, detect_binary, command_template) VALUES ('myagent', 'My Agent', 'myagent-bin', 'myagent {prompt}')",
            [],
        ).unwrap();
        let id = conn.last_insert_rowid();

        let row: (String, String, String, String) = conn.query_row(
            "SELECT key, display_name, detect_binary, command_template FROM agent_adapters WHERE id=?1",
            params![id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        ).unwrap();
        assert_eq!(row.0, "myagent");
        assert_eq!(row.3, "myagent {prompt}");

        conn.execute(
            "UPDATE agent_adapters SET key='myagent2', display_name='My Agent 2', detect_binary='myagent2-bin', command_template='myagent2 {prompt} {files}' WHERE id=?1",
            params![id],
        ).unwrap();
        let updated_key: String = conn.query_row(
            "SELECT key FROM agent_adapters WHERE id=?1", params![id], |r| r.get(0),
        ).unwrap();
        assert_eq!(updated_key, "myagent2");

        conn.execute("DELETE FROM agent_adapters WHERE id=?1", params![id]).unwrap();
        let count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM agent_adapters WHERE key='myagent2'",
            [], |r| r.get(0),
        ).unwrap();
        assert_eq!(count, 0, "删除后该 key 不存在");
    }

    #[test]
    fn agent_adapter_duplicate_key_rejected() {
        let conn = open_init();
        conn.execute(
            "INSERT INTO agent_adapters (key, display_name, detect_binary, command_template) VALUES ('dup', 'A', 'a-bin', 'a {prompt}')",
            [],
        ).unwrap();
        // 同 key 再插 → UNIQUE 约束拒绝
        let result = conn.execute(
            "INSERT INTO agent_adapters (key, display_name, detect_binary, command_template) VALUES ('dup', 'B', 'b-bin', 'b {prompt}')",
            [],
        );
        assert!(result.is_err(), "duplicate key should be rejected");
    }

    /// v42 seed：Pi + Claude 应自动入表，is_system=1。
    #[test]
    fn agent_adapter_seed_inserts_builtin_pi_claude() {
        let conn = open_init();
        let claude: (i64, String) = conn.query_row(
            "SELECT is_system, command_template FROM agent_adapters WHERE key='claude'",
            [], |r| Ok((r.get(0)?, r.get(1)?)),
        ).unwrap();
        assert_eq!(claude.0, 1, "claude is_system 应为 1");
        assert_eq!(claude.1, "claude --add-dir {cwd} {prompt}");

        let pi: (i64, i64) = conn.query_row(
            "SELECT is_system, is_default FROM agent_adapters WHERE key='pi'",
            [], |r| Ok((r.get(0)?, r.get(1)?)),
        ).unwrap();
        assert_eq!(pi.0, 1, "pi is_system 应为 1");
        assert_eq!(pi.1, 1, "pi 默认 is_default=1（PPT 菜单等场景的兜底）");
    }

    /// set_default_agent 必须保证全局唯一（先清零再置 1）。
    #[test]
    fn set_default_agent_is_mutually_exclusive() {
        let conn = open_init();
        // 先插一个用户自定义 agent
        conn.execute(
            "INSERT INTO agent_adapters (key, display_name, detect_binary, command_template) VALUES ('custom', 'Custom', 'custom-bin', 'custom {prompt}')",
            [],
        ).unwrap();
        let custom_id: i64 = conn.query_row(
            "SELECT id FROM agent_adapters WHERE key='custom'", [], |r| r.get(0),
        ).unwrap();

        // 初始：pi 是 default
        let pi_default: i64 = conn.query_row(
            "SELECT is_default FROM agent_adapters WHERE key='pi'", [], |r| r.get(0),
        ).unwrap();
        assert_eq!(pi_default, 1);

        // 设 custom 为 default
        set_default_agent_at(&conn, custom_id).unwrap();
        let defaults: Vec<String> = conn.prepare(
            "SELECT key FROM agent_adapters WHERE is_default=1"
        ).unwrap()
        .query_map([], |r| r.get::<_, String>(0)).unwrap()
        .filter_map(|r| r.ok()).collect();
        assert_eq!(defaults.len(), 1, "全局只能有 1 个 default");
        assert_eq!(defaults[0], "custom");
    }

    /// clear_default_agent 把所有 is_default 置 0。
    #[test]
    fn clear_default_agent_zeroes_all() {
        let conn = open_init();
        clear_default_agent_at(&conn).unwrap();
        let count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM agent_adapters WHERE is_default=1", [], |r| r.get(0),
        ).unwrap();
        assert_eq!(count, 0);
    }

    /// 内置 agent（is_system=1）不可删除。
    #[test]
    fn delete_agent_adapter_rejects_system() {
        let conn = open_init();
        let result = delete_agent_adapter_record_at(&conn, 1);  // id=1 是 claude（首条 seed）
        assert!(result.is_err(), "内置 agent 删除应被拒绝");
    }

    #[test]
    fn agent_task_crud_roundtrip() {
        let conn = open_init();
        conn.execute(
            "INSERT INTO agent_tasks (id, agent_key, context) VALUES ('test-1', 'claude', '{\"kind\":\"files\"}')",
            [],
        ).unwrap();
        let row: Vec<(String, String)> = conn.prepare(
            "SELECT status, agent_key FROM agent_tasks WHERE id='test-1'"
        ).unwrap().query_map([], |r| Ok((r.get(0)?, r.get(1)?))).unwrap()
        .filter_map(|r| r.ok()).collect();
        assert_eq!(row[0].0, "pending");
        assert_eq!(row[0].1, "claude");
        conn.execute("UPDATE agent_tasks SET transcribed_text='hello', status='executing' WHERE id='test-1'", []).unwrap();
        let text: String = conn.query_row("SELECT transcribed_text FROM agent_tasks WHERE id='test-1'", [], |r| r.get(0)).unwrap();
        assert_eq!(text, "hello");
        conn.execute("DELETE FROM agent_tasks WHERE id='test-1'", []).unwrap();
        let count: i64 = conn.query_row("SELECT COUNT(*) FROM agent_tasks", [], |r| r.get(0)).unwrap();
        assert_eq!(count, 0);
    }

    #[test]
    fn agent_task_lifecycle_pending_to_done() {
        let conn = open_init();
        // 创建 task（pending）
        conn.execute(
            "INSERT INTO agent_tasks (id, agent_key, context) VALUES ('life-1', 'claude', '{\"kind\":\"files\",\"files\":[\"/a\"]}')",
            [],
        ).unwrap();
        let status: String = conn.query_row("SELECT status FROM agent_tasks WHERE id='life-1'", [], |r| r.get(0)).unwrap();
        assert_eq!(status, "pending");

        // 录音回调 → executing
        conn.execute("UPDATE agent_tasks SET transcribed_text='帮我整理', status='executing', updated_at=datetime('now') WHERE id='life-1'", []).unwrap();
        let (status, text): (String, String) = conn.query_row(
            "SELECT status, transcribed_text FROM agent_tasks WHERE id='life-1'", [], |r| Ok((r.get(0)?, r.get(1)?))
        ).unwrap();
        assert_eq!(status, "executing");
        assert_eq!(text, "帮我整理");

        // 执行完成 → done
        conn.execute("UPDATE agent_tasks SET status='done', updated_at=datetime('now') WHERE id='life-1'", []).unwrap();
        let status: String = conn.query_row("SELECT status FROM agent_tasks WHERE id='life-1'", [], |r| r.get(0)).unwrap();
        assert_eq!(status, "done");

        // 清理
        conn.execute("DELETE FROM agent_tasks WHERE id='life-1'", []).unwrap();
    }

    #[test]
    fn agent_task_lifecycle_pending_to_failed() {
        let conn = open_init();
        conn.execute("INSERT INTO agent_tasks (id, agent_key) VALUES ('life-2', 'pi')", []).unwrap();
        // 空识别 → failed
        conn.execute("UPDATE agent_tasks SET status='failed', error_msg='识别结果为空', updated_at=datetime('now') WHERE id='life-2'", []).unwrap();
        let (status, err): (String, String) = conn.query_row(
            "SELECT status, error_msg FROM agent_tasks WHERE id='life-2'", [], |r| Ok((r.get(0)?, r.get(1)?))
        ).unwrap();
        assert_eq!(status, "failed");
        assert_eq!(err, "识别结果为空");
        conn.execute("DELETE FROM agent_tasks WHERE id='life-2'", []).unwrap();
    }

    #[test]
    fn agent_task_context_json_storage() {
        let conn = open_init();
        let complex_context = r#"{"kind":"files","files":["/a/b.pdf","/c d/e.pdf"],"cwd":"/Users/x","prompt_template":"{{voice}}\n\n{{files}}"}"#;
        conn.execute(
            "INSERT INTO agent_tasks (id, agent_key, context) VALUES (?1, ?2, ?3)",
            params!["ctx-1", "claude", complex_context],
        ).unwrap();
        let stored: String = conn.query_row("SELECT context FROM agent_tasks WHERE id='ctx-1'", [], |r| r.get(0)).unwrap();
        // JSON 往返无损
        let parsed: serde_json::Value = serde_json::from_str(&stored).unwrap();
        assert_eq!(parsed["files"][0], "/a/b.pdf");
        assert_eq!(parsed["files"][1], "/c d/e.pdf");
        assert_eq!(parsed["cwd"], "/Users/x");
        assert_eq!(parsed["prompt_template"], "{{voice}}\n\n{{files}}");
        conn.execute("DELETE FROM agent_tasks WHERE id='ctx-1'", []).unwrap();
    }

    #[test]
    fn agent_task_list_ordered_by_created_at_desc() {
        let conn = open_init();
        conn.execute("INSERT INTO agent_tasks (id, agent_key) VALUES ('old', 'claude')", []).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(1100));
        conn.execute("INSERT INTO agent_tasks (id, agent_key) VALUES ('new', 'pi')", []).unwrap();
        let ids: Vec<String> = conn.prepare(
            "SELECT id FROM agent_tasks ORDER BY created_at DESC"
        ).unwrap().query_map([], |r| r.get::<_, String>(0)).unwrap()
        .filter_map(|r| r.ok()).collect();
        assert_eq!(ids.len(), 2);
        assert_eq!(ids[0], "new"); // 新的在前
        assert_eq!(ids[1], "old");
    }

    #[test]
    fn agent_task_default_status_is_pending() {
        let conn = open_init();
        conn.execute("INSERT INTO agent_tasks (id, agent_key) VALUES ('def-1', 'claude')", []).unwrap();
        let status: String = conn.query_row("SELECT status FROM agent_tasks WHERE id='def-1'", [], |r| r.get(0)).unwrap();
        assert_eq!(status, "pending");
    }

    #[test]
    fn agent_task_default_context_is_empty_json() {
        let conn = open_init();
        conn.execute("INSERT INTO agent_tasks (id, agent_key) VALUES ('def-2', 'claude')", []).unwrap();
        let context: String = conn.query_row("SELECT context FROM agent_tasks WHERE id='def-2'", [], |r| r.get(0)).unwrap();
        assert_eq!(context, "{}");
    }
}
