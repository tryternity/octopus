//! 外置 seed 数据加载——长文本 seed 从仓库内 seeds/ 目录读取，运行期拼装 SQL 插入 DB。
//! 仅 schema 升级（v<39）时执行一次；失败时 log::error 跳过该项，绝不阻塞 schema 升级。
//!
//! 设计动机：db.sql 内联长 prompt / 多 provider JSON 让 schema 真相难读，
//! 改为本模块从 `seeds/` 目录读 markdown / JSON 运行期拼装。db.sql 只保留表结构 +
//! 短种子（参考 Task 2/3）。

use anyhow::{Context, Result};
use rusqlite::Connection;
use std::path::PathBuf;

/// seeds 目录绝对路径。
/// dev（cargo run / cargo test）：$CARGO_MANIFEST_DIR/seeds
/// release（裸二进制）：通过 Cargo.toml `package.include` 打包到 exe 同级/seeds
pub fn seeds_dir() -> PathBuf {
    // dev 路径——编译期取 Cargo.toml 所在目录
    let dev = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("seeds");
    if dev.exists() {
        return dev;
    }
    // release 路径——exe 同级/seeds
    if let Ok(exe) = std::env::current_exe() {
        if let Some(parent) = exe.parent() {
            let release = parent.join("seeds");
            if release.exists() {
                return release;
            }
        }
    }
    // fallback：dev 路径（即使不存在也返回，调用方处理 Err）
    dev
}

/// 入口：依次加载所有外置 seed。失败时 log::error 跳过该项，不阻塞整体。
///
/// 调用方（init_schema）传入裸连接；本函数仅返回 Ok——单个 seed 失败不传播，
/// schema 升级永远不被 seed 缺失/格式错误阻塞。
pub fn load_external_seeds(conn: &Connection) -> Result<()> {
    // 顺序：prompts → llm_providers → agent_actions
    // 任一失败只 log，不传播 Err
    if let Err(e) = load_prompt_seeds(conn) {
        log::error!("[seeds] 加载 prompts seed 失败: {}", e);
    }
    if let Err(e) = load_llm_providers_seed(conn) {
        log::error!("[seeds] 加载 llm_providers seed 失败: {}", e);
    }
    if let Err(e) = load_agent_action_seeds(conn) {
        log::error!("[seeds] 加载 agent_actions seed 失败: {}", e);
    }
    Ok(())
}

/// 加载 prompts/*.md。已知 prompt name → (id, title, description) 映射固定。
/// INSERT OR IGNORE：id 已存在则跳过（保护用户编辑）。
fn load_prompt_seeds(conn: &Connection) -> Result<()> {
    let prompts_dir = seeds_dir().join("prompts");
    // (id, filename, title, description)
    let seeds = [
        (1i64, "default-polish.md", "默认润色", "默认润色（系统内置）"),
        (2i64, "advanced-polish.md", "进阶润色（断续纠正）",
         "进阶版：针对断续纠正、重复修正、同音漂移场景强化的润色 prompt（系统内置）"),
    ];
    for (id, filename, title, desc) in seeds {
        let path = prompts_dir.join(filename);
        let content = std::fs::read_to_string(&path)
            .with_context(|| format!("读 prompt seed: {:?}", path))?;
        // INSERT OR IGNORE：id 已存在则跳过（保护用户编辑）
        conn.execute(
            "INSERT OR IGNORE INTO prompts (id, title, category, content, description, is_system)
             VALUES (?1, ?2, 'voice_text_polish', ?3, ?4, 1)",
            rusqlite::params![id, title, content, desc],
        ).with_context(|| format!("插入 prompt seed id={}", id))?;
    }
    Ok(())
}

/// LLM provider 预设 seed（app_config.category='llm_provider'）。
/// 由 db.sql 早期 INSERT 提供的旧 in-line 长文本已迁到 seeds/llm_providers.json。
#[derive(serde::Deserialize)]
struct LlmProviderSeed {
    config_key: String,
    config_value: serde_json::Value,
    description: String,
    category: String,
}

fn load_llm_providers_seed(conn: &Connection) -> Result<()> {
    let path = seeds_dir().join("llm_providers.json");
    let json = std::fs::read_to_string(&path)
        .with_context(|| format!("读 llm_providers.json: {:?}", path))?;
    let providers: Vec<LlmProviderSeed> = serde_json::from_str(&json)
        .with_context(|| "解析 llm_providers.json")?;
    for p in &providers {
        let value_str = serde_json::to_string(&p.config_value)
            .with_context(|| "序列化 config_value")?;
        conn.execute(
            "INSERT OR IGNORE INTO app_config (config_key, config_value, description, category)
             VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params![p.config_key, value_str, p.description, p.category],
        ).with_context(|| format!("插入 llm_provider: {}", p.config_key))?;
    }
    Ok(())
}

/// 加载 agent_actions/*.prompt.md → 在 action_bar_items 中创建对应子菜单项。
///
/// 当前固定目标：Agent 主菜单（title='Agent'，accepts='file'）+ 「制作 PPT」子项。
/// 后续可迭代为目录扫描。
///
/// **自愈策略**（2026-07-19 v40 修复）：早期版本用 INSERT WHERE NOT EXISTS，
/// 一旦菜单项存在（即使 action_data 为空）就跳过——导致 Task 1 实施期插过的
/// action_data='' 的 PPT 菜单永远填不上 prompt。新版分四步：
///   1. 插 Agent 主菜单（WHERE NOT EXISTS 防重）
///   2. **去重 PPT 子菜单**（历史 bug 可能留多条；保留 id 最小的）
///   3. 插 PPT 子菜单（WHERE NOT EXISTS 防重）
///   4. UPDATE 自愈——空 action_data 补 prompt、need_voice 强制 1、用户改过的保留
fn load_agent_action_seeds(conn: &Connection) -> Result<()> {
    let make_ppt_prompt = seeds_dir().join("agent_actions/make-ppt.prompt.md");
    let prompt_content = std::fs::read_to_string(&make_ppt_prompt)
        .with_context(|| format!("读 make-ppt.prompt.md: {:?}", make_ppt_prompt))?;

    // 1. 插 Agent 主菜单（title 去重，accepts='file'）
    conn.execute(
        "INSERT INTO action_bar_items (parent_id, title, icon, action_type, action_data, sort_order, is_system, accepts)
         SELECT NULL, 'Agent', 'bot', 'submenu', '', 5, 1, 'file'
         WHERE NOT EXISTS (SELECT 1 FROM action_bar_items WHERE title='Agent' AND parent_id IS NULL)",
        [],
    ).context("插入 Agent 主菜单")?;

    // 2. 查 Agent id（不复用固定 id——避免与用户自建项冲突）
    let agent_id: i64 = conn
        .query_row(
            "SELECT id FROM action_bar_items WHERE title='Agent' AND parent_id IS NULL",
            [], |r| r.get(0),
        )
        .context("查 Agent 主菜单 id")?;

    // 3. 去重 PPT 子菜单：早期版本 INSERT OR IGNORE（无 UNIQUE 约束）曾留下多条 PPT 子菜单。
    //    保留 id 最小的（最早创建的），其余删除。
    conn.execute(
        "DELETE FROM action_bar_items
         WHERE title = '制作 PPT' AND parent_id = ?1
           AND id NOT IN (
               SELECT MIN(id) FROM action_bar_items
               WHERE title = '制作 PPT' AND parent_id = ?1
           )",
        rusqlite::params![agent_id],
    ).context("去重 PPT 子菜单")?;

    // 4. 插 PPT 子菜单（WHERE NOT EXISTS——还没创建则插入）
    conn.execute(
        "INSERT INTO action_bar_items (parent_id, title, icon, action_type, action_data, agent, accepts, sort_order, is_system, need_voice)
         SELECT ?1, '制作 PPT', 'presentation', 'agent', ?2, 'pi', 'file', 0, 1, 1
         WHERE NOT EXISTS (
             SELECT 1 FROM action_bar_items
             WHERE title='制作 PPT' AND parent_id = ?1
         )",
        rusqlite::params![agent_id, prompt_content],
    ).context("插入 PPT 子菜单")?;

    // 5. 自愈：现有 PPT 菜单项如果 action_data 为空（早期版本残留）或 need_voice 未开，补上。
    //    用户在设置里改过的 action_data（非空）保留；need_voice 总是强制设为 1（PPT 菜单语义）。
    conn.execute(
        "UPDATE action_bar_items
         SET action_data = CASE WHEN action_data = '' THEN ?2 ELSE action_data END,
             need_voice = 1,
             agent = CASE WHEN agent = '' THEN 'pi' ELSE agent END,
             accepts = 'file'
         WHERE title = '制作 PPT' AND parent_id = ?1",
        rusqlite::params![agent_id, prompt_content],
    ).context("自愈 PPT 菜单 action_data + need_voice")?;
    Ok(())
}

/// 给 desktop crate 复原按钮用——按 prompt 简称返回 seed 文件路径。
/// name 示例："default-polish" / "advanced-polish"
pub fn seed_prompt_path(name: &str) -> Option<PathBuf> {
    let path = seeds_dir().join("prompts").join(format!("{}.md", name));
    if path.exists() {
        Some(path)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seeds_dir_returns_existing_path_in_dev() {
        let dir = seeds_dir();
        // dev 模式必须存在（仓库内）
        assert!(dir.exists(), "seeds_dir() 在 dev 模式应存在: {:?}", dir);
        assert!(dir.join("prompts/default-polish.md").exists());
    }

    #[test]
    fn seed_prompt_path_returns_some_for_known_name() {
        let path = seed_prompt_path("default-polish");
        assert!(path.is_some());
        assert!(path.unwrap().exists());
    }

    #[test]
    fn seed_prompt_path_returns_none_for_unknown_name() {
        assert!(seed_prompt_path("nonexistent-prompt").is_none());
    }
}

#[cfg(test)]
mod load_tests {
    use super::*;

    /// 进程级 Mutex——串行化所有「改写共享 seed 文件」的测试，防 Rust 默认并行
    /// 执行 race（load_prompt_seeds_missing_file / load_external_seeds_never_propagates
    /// 等会重命名/改写真实 seeds/ 文件，与读同文件的测试并发会假阳性失败）。
    /// 只读测试无需持锁（它们只在锁内串行化，锁外仍可并行）。
    static SEEDS_FILE_MUTEX: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn fresh_db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(include_str!("db.sql")).unwrap();
        conn
    }

    #[test]
    fn load_prompt_seeds_inserts_two_prompts() {
        let _guard = SEEDS_FILE_MUTEX.lock().unwrap();
        let conn = fresh_db();
        load_prompt_seeds(&conn).unwrap();
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM prompts WHERE category='voice_text_polish'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 2, "应插入默认润色 + 进阶润色两条");
    }

    #[test]
    fn load_prompt_seeds_is_idempotent_via_insert_or_ignore() {
        let _guard = SEEDS_FILE_MUTEX.lock().unwrap();
        let conn = fresh_db();
        load_prompt_seeds(&conn).unwrap();
        // 用户改了 prompt 内容（直接 UPDATE）
        conn.execute("UPDATE prompts SET content='用户改的' WHERE id=1", []).unwrap();
        // 再次加载——id 已存在，OR IGNORE 跳过，用户修改保留
        load_prompt_seeds(&conn).unwrap();
        let content: String = conn
            .query_row("SELECT content FROM prompts WHERE id=1", [], |r| r.get(0))
            .unwrap();
        assert_eq!(content, "用户改的", "OR IGNORE 应保护用户编辑");
    }

    #[test]
    fn load_prompt_seeds_missing_file_returns_err() {
        let _guard = SEEDS_FILE_MUTEX.lock().unwrap();
        let conn = fresh_db();
        // 暂时把 default-polish.md 改名
        let path = seeds_dir().join("prompts/default-polish.md");
        let backup = seeds_dir().join("prompts/default-polish.md.bak");
        std::fs::rename(&path, &backup).unwrap();
        let result = load_prompt_seeds(&conn);
        std::fs::rename(&backup, &path).unwrap(); // 恢复，防污染其他测试
        assert!(result.is_err(), "文件缺失应返回 Err");
    }

    #[test]
    fn load_llm_providers_seed_inserts_all_providers() {
        let _guard = SEEDS_FILE_MUTEX.lock().unwrap();
        let conn = fresh_db();
        // 清空 app_config 防 db.sql 残留干扰
        conn.execute("DELETE FROM app_config WHERE category='llm_provider'", []).unwrap();
        load_llm_providers_seed(&conn).unwrap();
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM app_config WHERE category='llm_provider'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 7, "应插入 7 个 LLM provider");
    }

    #[test]
    fn load_llm_providers_seed_skips_existing_keys() {
        let _guard = SEEDS_FILE_MUTEX.lock().unwrap();
        let conn = fresh_db();
        conn.execute("DELETE FROM app_config WHERE category='llm_provider'", []).unwrap();
        load_llm_providers_seed(&conn).unwrap();
        // 用户改了 deepseek 的 models
        conn.execute("UPDATE app_config SET config_value='{\"user\":\"edited\"}' WHERE config_key='deepseek'", []).unwrap();
        // 重跑——OR IGNORE 跳过 deepseek
        load_llm_providers_seed(&conn).unwrap();
        let v: String = conn
            .query_row("SELECT config_value FROM app_config WHERE config_key='deepseek'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(v, "{\"user\":\"edited\"}", "用户修改应保留");
    }

    #[test]
    fn load_agent_action_seeds_creates_agent_menu_and_ppt_item() {
        let _guard = SEEDS_FILE_MUTEX.lock().unwrap();
        let conn = fresh_db();
        load_agent_action_seeds(&conn).unwrap();
        let agent_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM action_bar_items WHERE title='Agent' AND parent_id IS NULL", [], |r| r.get(0))
            .unwrap();
        assert_eq!(agent_count, 1, "应创建 1 个 Agent 主菜单");
        let ppt_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM action_bar_items WHERE title='制作 PPT'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(ppt_count, 1, "应创建 1 个 PPT 子菜单");
        let ppt: (String, String, String) = conn
            .query_row("SELECT action_type, agent, accepts FROM action_bar_items WHERE title='制作 PPT'", [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .unwrap();
        assert_eq!(ppt.0, "agent");
        assert_eq!(ppt.1, "pi");
        assert_eq!(ppt.2, "file");
    }

    #[test]
    fn load_agent_action_seeds_is_idempotent() {
        let _guard = SEEDS_FILE_MUTEX.lock().unwrap();
        let conn = fresh_db();
        load_agent_action_seeds(&conn).unwrap();
        load_agent_action_seeds(&conn).unwrap(); // 重跑
        let agent_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM action_bar_items WHERE title='Agent' AND parent_id IS NULL", [], |r| r.get(0))
            .unwrap();
        assert_eq!(agent_count, 1, "重跑后 Agent 仍只有 1 个");
    }

    /// v40 自愈：早期版本（INSERT WHERE NOT EXISTS）若先插过 action_data='' 的 PPT 菜单，
    /// 后续加载无法补内容。新版用 INSERT OR IGNORE + UPDATE 兜底修复。
    /// 模拟场景：先插一个 action_data='' 的 PPT 菜单，再调 load_agent_action_seeds，
    /// 验证 action_data 被填上 + need_voice=1。
    #[test]
    fn load_agent_action_seeds_self_heals_empty_action_data() {
        let _guard = SEEDS_FILE_MUTEX.lock().unwrap();
        let conn = fresh_db();
        // 先插 Agent 主菜单 + 一个 action_data='' 的 PPT 子项（模拟早期 bug 残留）
        conn.execute(
            "INSERT INTO action_bar_items (parent_id, title, icon, action_type, action_data, sort_order, is_system, accepts)
             VALUES (NULL, 'Agent', 'bot', 'submenu', '', 5, 1, 'file')",
            [],
        ).unwrap();
        let agent_id: i64 = conn
            .query_row("SELECT id FROM action_bar_items WHERE title='Agent' AND parent_id IS NULL", [], |r| r.get(0))
            .unwrap();
        conn.execute(
            "INSERT INTO action_bar_items (parent_id, title, icon, action_type, action_data, agent, accepts, sort_order, is_system)
             VALUES (?1, '制作 PPT', 'presentation', 'agent', '', 'pi', 'file', 0, 1)",
            rusqlite::params![agent_id],
        ).unwrap();
        // 验证模拟状态
        let before_data: String = conn
            .query_row("SELECT action_data FROM action_bar_items WHERE title='制作 PPT'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(before_data, "", "测试前置：PPT 菜单 action_data 为空");

        // 跑 load——应自愈
        load_agent_action_seeds(&conn).unwrap();

        let after_data: String = conn
            .query_row("SELECT action_data FROM action_bar_items WHERE title='制作 PPT'", [], |r| r.get(0))
            .unwrap();
        assert!(!after_data.is_empty(), "自愈后 action_data 应非空");
        assert!(after_data.contains("{{task}}"), "自愈后 action_data 应含 {{task}}");

        let need_voice: i64 = conn
            .query_row("SELECT need_voice FROM action_bar_items WHERE title='制作 PPT'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(need_voice, 1, "自愈后 need_voice 应为 1");
    }

    /// v40 自愈：用户改过 action_data（非空）→ 加载时保留用户内容，不覆盖。
    #[test]
    fn load_agent_action_seeds_preserves_user_edited_action_data() {
        let _guard = SEEDS_FILE_MUTEX.lock().unwrap();
        let conn = fresh_db();
        // 用户已自定义 PPT prompt
        conn.execute(
            "INSERT INTO action_bar_items (parent_id, title, icon, action_type, action_data, agent, accepts, sort_order, is_system, need_voice)
             VALUES (NULL, 'Agent', 'bot', 'submenu', '', 5, 1, 'file', 0, 0)",  // Agent 父菜单
            [],
        ).unwrap();
        let agent_id: i64 = conn
            .query_row("SELECT id FROM action_bar_items WHERE title='Agent' AND parent_id IS NULL", [], |r| r.get(0))
            .unwrap();
        conn.execute(
            "INSERT INTO action_bar_items (parent_id, title, icon, action_type, action_data, agent, accepts, sort_order, is_system)
             VALUES (?1, '制作 PPT', 'presentation', 'agent', '用户自定义 prompt', 'pi', 'file', 0, 1)",
            rusqlite::params![agent_id],
        ).unwrap();

        load_agent_action_seeds(&conn).unwrap();

        let after_data: String = conn
            .query_row("SELECT action_data FROM action_bar_items WHERE title='制作 PPT'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(after_data, "用户自定义 prompt", "用户改过的 action_data 应保留");
        // need_voice 仍会被强制设为 1（PPT 菜单语义）
        let need_voice: i64 = conn
            .query_row("SELECT need_voice FROM action_bar_items WHERE title='制作 PPT'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(need_voice, 1, "need_voice 仍应强制为 1");
    }

    /// v40 去重：早期 INSERT OR IGNORE（无 UNIQUE）曾留下多条 PPT 子菜单。
    /// load 应保留 id 最小的，删除其余。
    #[test]
    fn load_agent_action_seeds_dedupes_duplicate_ppt_items() {
        let _guard = SEEDS_FILE_MUTEX.lock().unwrap();
        let conn = fresh_db();
        // 模拟早期 bug：Agent 下挂 3 条「制作 PPT」
        conn.execute(
            "INSERT INTO action_bar_items (parent_id, title, icon, action_type, action_data, sort_order, is_system, accepts)
             VALUES (NULL, 'Agent', 'bot', 'submenu', '', 5, 1, 'file')",
            [],
        ).unwrap();
        let agent_id: i64 = conn
            .query_row("SELECT id FROM action_bar_items WHERE title='Agent' AND parent_id IS NULL", [], |r| r.get(0))
            .unwrap();
        for _ in 0..3 {
            conn.execute(
                "INSERT INTO action_bar_items (parent_id, title, icon, action_type, action_data, agent, accepts, sort_order, is_system)
                 VALUES (?1, '制作 PPT', 'presentation', 'agent', '', 'pi', 'file', 0, 1)",
                rusqlite::params![agent_id],
            ).unwrap();
        }
        let before_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM action_bar_items WHERE title='制作 PPT'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(before_count, 3, "测试前置：3 条重复");

        load_agent_action_seeds(&conn).unwrap();

        let after_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM action_bar_items WHERE title='制作 PPT'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(after_count, 1, "去重后应只剩 1 条");
    }

    /// load_external_seeds 永远 Ok——单个 seed 失败仅 log::error。
    /// 通过临时把 llm_providers.json 写成非法 JSON 验证：解析失败被吞，整体仍 Ok。
    ///
    /// ⚠️ 共享文件干扰：本测试改写 `llm_providers.json` 真实文件——Rust 默认并行
    /// 测试与 `load_llm_providers_seed_*` 读同文件存在 race。这里用进程级 Mutex 串行化，
    /// 保证改写-恢复窗口独占。Mutex 在 `load_tests` 模块作用域（下方文件末尾）声明。
    #[test]
    fn load_external_seeds_never_propagates_errors() {
        let _guard = SEEDS_FILE_MUTEX.lock().unwrap();
        let conn = fresh_db();
        // 制造 llm_providers 解析失败（非法 JSON）
        let path = seeds_dir().join("llm_providers.json");
        let original = std::fs::read(&path).unwrap();
        std::fs::write(&path, b"not valid json").unwrap();
        let result = load_external_seeds(&conn);
        std::fs::write(&path, original).unwrap(); // 恢复
        assert!(result.is_ok(), "load_external_seeds 不应传播单 seed 错误: {:?}", result);
    }

    /// PPT prompt 必须包含 {{task}} / {{files}} 占位符（octopus 的 render_agent_prompt
    /// 只替换这两个），且必须推荐 guizang-ppt-skill 与 ppt-master 两个候选 skill
    /// （spec § 3 要求的核心 skill 清单）。
    #[test]
    fn make_ppt_prompt_contains_required_placeholders() {
        let path = seeds_dir().join("agent_actions/make-ppt.prompt.md");
        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains("{{task}}"), "PPT prompt 必须含 {{task}} 占位符");
        assert!(content.contains("{{files}}"), "PPT prompt 必须含 {{files}} 占位符");
        assert!(content.contains("guizang-ppt-skill"), "应推荐 guizang skill");
        assert!(content.contains("ppt-master"), "应推荐 ppt-master skill");
    }
}
