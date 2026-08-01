// db/config.rs —— app_config 表读写（替代 config.yaml）+ env_vars（category='env'）。

use super::{ensure_db, with_db, Connection, Result, params};
use anyhow::Context;

// ── app_config 表读写（替代 config.yaml）──

/// 从 DB app_config 表加载完整应用配置。
/// 先构造 AppConfig::default()（保底），再用 DB 行按字段类型解析覆盖。
/// 缺失行或解析失败 → 保留 default 值（防御性，正常不应触发——seed 保证 21 行齐全）。
/// 只读 category='setting' 的行（用户配置项）。
pub fn load_app_config() -> Result<crate::config::AppConfig> {
    ensure_db()?;
    with_db(load_app_config_at)
}

pub(crate) fn load_app_config_at(conn: &Connection) -> Result<crate::config::AppConfig> {
    // 以 AppConfig::default() 的 JSON 形态作为类型模板——每个 DB 字段按模板类型还原，
    // 不靠字符串内容猜类型（避免把值恰为数字的 String 字段误判为 Number）。
    // 字段增删自动反映，无需手动维护 match arms。parse 失败保留 default（同旧行为）。
    let mut result = serde_json::to_value(crate::config::AppConfig::default())
        .expect("AppConfig default 序列化不会失败");
    let type_hints = result
        .as_object()
        .expect("AppConfig 序列化为 JSON object")
        .clone();

    let mut stmt = conn.prepare(
        "SELECT config_key, config_value FROM app_config WHERE category = 'setting'",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    })?;

    for row in rows {
        let (key, value) = row?;
        // 未知 key 跳过（前向兼容，同旧 _ => {}）
        if let Some(hint) = type_hints.get(&key) {
            if let Some(slot) = result.get_mut(&key) {
                *slot = coerce_db_string(&value, hint);
            }
        }
    }
    Ok(serde_json::from_value(result).unwrap_or_default())
}

/// 按 JSON 类型模板把 DB TEXT 还原为 serde_json::Value。
/// - Bool: "true"/"false"
/// - Number: 先 i64 后 f64，parse 失败返回 hint（保留 default）
/// - String / 其他: 原样返回字符串
pub(crate) fn coerce_db_string(s: &str, hint: &serde_json::Value) -> serde_json::Value {
    use serde_json::Value;
    match hint {
        Value::Bool(_) => Value::Bool(s == "true"),
        Value::Number(_) => {
            if let Ok(n) = s.parse::<i64>() {
                Value::Number(n.into())
            } else if let Ok(f) = s.parse::<f64>() {
                serde_json::Number::from_f64(f)
                    .map(Value::Number)
                    .unwrap_or_else(|| hint.clone())
            } else {
                hint.clone()
            }
        }
        _ => Value::String(s.to_string()),
    }
}

/// 全量写入应用配置（serde 自动遍历所有字段，ON CONFLICT DO UPDATE）。set_config / yaml 迁移用。
/// 仅更新 config_value，保留 description + category（不同于 INSERT OR REPLACE 会清空非指定列）。
/// 字段增删自动反映，无需手动维护字段数组。
pub fn save_app_config(cfg: &crate::config::AppConfig) -> Result<()> {
    ensure_db()?;
    with_db(|conn| save_app_config_at(conn, cfg))
}

pub(crate) fn save_app_config_at(conn: &Connection, cfg: &crate::config::AppConfig) -> Result<()> {
    // serde 序列化为 JSON Map 后逐字段 upsert——字段增删自动反映，无需手动维护字段数组。
    let value = serde_json::to_value(cfg).context("序列化 AppConfig")?;
    let obj = value.as_object().context("AppConfig 序列化非 object")?;

    // 包事务：所有字段写入要么全部成功要么全部回滚，避免中途崩溃导致配置半更新。
    // unchecked_transaction 可在已有事务上下文中调用（不会 panic），commit 原子提交。
    let tx = conn.unchecked_transaction()?;
    for (key, val) in obj {
        // 还原为 DB 存储的 TEXT：字符串直接取值，数字/bool to_string。
        let s = match val {
            serde_json::Value::String(s) => s.clone(),
            _ => val.to_string(),
        };
        tx.execute(
            "INSERT INTO app_config (config_key, config_value) VALUES (?1, ?2)
             ON CONFLICT(config_key) DO UPDATE SET config_value = excluded.config_value",
            params![key, s],
        )?;
    }
    tx.commit()?;
    Ok(())
}

/// 单键写入（persist_* 命令用，避免全量回写）。
/// 使用 ON CONFLICT DO UPDATE 仅改 config_value，保留 description + category。
pub fn save_config_key(key: &str, value: &str) -> Result<()> {
    ensure_db()?;
    with_db(|conn| {
        conn.execute(
            "INSERT INTO app_config (config_key, config_value) VALUES (?1, ?2)
             ON CONFLICT(config_key) DO UPDATE SET config_value = excluded.config_value",
            params![key, value],
        )?;
        Ok(())
    })
}

/// 按 key 读取单个 config_value（不存在返回 None）。
pub fn load_config_key(key: &str) -> Result<Option<String>> {
    ensure_db()?;
    with_db(|conn| {
        let mut stmt = conn.prepare("SELECT config_value FROM app_config WHERE config_key = ?1")?;
        let row = stmt.query_row(params![key], |r| r.get::<_, String>(0));
        match row {
            Ok(v) => Ok(Some(v)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
    })
}

// ── 环境变量（category='env'）──

/// 列出所有 env 变量，返回 (key, value) 列表。
/// key 去掉 `env.` 前缀（返回裸名如 "huggingface"）。
pub fn list_env_vars() -> Result<Vec<(String, String)>> {
    ensure_db()?;
    with_db(|conn| {
        let mut stmt = conn.prepare(
            "SELECT config_key, config_value FROM app_config WHERE category = 'env' ORDER BY config_key"
        )?;
        let rows = stmt.query_map([], |r| {
            let key: String = r.get(0)?;
            let value: String = r.get(1)?;
            Ok((key, value))
        })?;
        rows.collect::<std::result::Result<Vec<_>, _>>().map_err(Into::into)
    })
}

/// 保存 env 变量（category='env'，config_key 不带 env. 前缀）。
pub fn save_env_var(key: &str, value: &str) -> Result<()> {
    ensure_db()?;
    with_db(|conn| {
        conn.execute(
            "INSERT INTO app_config (config_key, config_value, category) VALUES (?1, ?2, 'env')
             ON CONFLICT(config_key) DO UPDATE SET config_value = excluded.config_value",
            params![key, value],
        )?;
        Ok(())
    })
}

/// 删除 env 变量。内置 3 个（huggingface/modelscope/github）不可删，返回 Ok(false)。
pub fn delete_env_var(key: &str) -> Result<bool> {
    const BUILTIN: &[&str] = &["huggingface", "modelscope", "github"];
    if BUILTIN.contains(&key) {
        return Ok(false);
    }
    ensure_db()?;
    with_db(|conn| {
        conn.execute(
            "DELETE FROM app_config WHERE config_key = ?1 AND category = 'env'",
            params![key],
        )?;
        Ok(true)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::INIT_SQL;
    use rusqlite::Connection;

    /// 在内存 DB 上执行 INIT_SQL，返回初始化好的连接。
    fn open_init() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(INIT_SQL).unwrap();
        conn
    }

    /// AppConfig 全字段 DB 往返：save → load 必须完整还原每个字段。
    /// 这是 serde 自动 load/save 的回归守卫——新增字段后若遗漏注册（旧手动枚举的坑），
    /// 此测试会因该字段回到 default 而失败。历史踩坑 4 次，见 archived specs 2026-06-28。
    #[test]
    fn app_config_roundtrip_all_fields() {
        use crate::config::{AppConfig, PolishMode};
        let conn = open_init();

        let mut cfg = AppConfig::default();
        // 每个字段设一个与 default 不同的哨兵值
        cfg.engine_mode = "websocket".into();
        cfg.remote_url = "http://rt:9999".into();
        cfg.grpc_endpoint = "http://grpc:50051".into();
        cfg.language = "en".into();
        cfg.asr_shortcut = "Alt+1".into();
        cfg.paste_method = "direct".into();
        cfg.write_to_clipboard = false;
        cfg.switch_input_source_on_paste = false;
        cfg.microphone = "Sentinel Mic".into();
        cfg.segment_silence = 1234.5;
        cfg.overlay_position = "bottom".into();
        cfg.polish_mode = PolishMode::Intermediate;
        cfg.polish_min_interval = 7.5;
        cfg.pause_polish_threshold_ms = 999.0;
        cfg.asr_hardware_accelerated = false;
        cfg.asr_correct = false;
        cfg.output_simplified = false;
        cfg.hide_toolbar = false;
        cfg.denoise_mode = 2;
        cfg.edit_shortcut = "Alt+2".into();
        cfg.edit_global_shortcut = "Alt+3".into();
        cfg.download_mirror = "https://mirror.test".into();
        cfg.clipboard_shortcut = "Alt+5".into();
        cfg.clipboard_max_items = 42;
        cfg.clipboard_max_age_days = 7;
        cfg.clipboard_enabled = false;
        cfg.screenshot_shortcut = "Alt+6".into();

        save_app_config_at(&conn, &cfg).unwrap();
        let loaded = load_app_config_at(&conn).unwrap();

        // Debug 格式全比较——任何字段未往返都会暴露差异。
        assert_eq!(format!("{:?}", loaded), format!("{:?}", cfg));
    }

    // ── app_config 表测试 ──

    #[test]
    fn app_config_seed_provides_all_fields() {
        let conn = open_init();
        let cfg = load_app_config_at(&conn).unwrap();
        // seed 默认值校验（抽样关键字段）
        assert_eq!(cfg.engine_mode, "embedded");
        assert_eq!(cfg.language, "auto");
        assert!(cfg.write_to_clipboard);
        assert!(!cfg.asr_hardware_accelerated);
        assert_eq!(cfg.segment_silence, 400.0);
        assert_eq!(cfg.polish_min_interval, 5.0);
        assert_eq!(cfg.denoise_mode, 1);
        assert_eq!(cfg.edit_shortcut, "CmdOrCtrl+Enter");
        assert_eq!(cfg.download_mirror, "");
    }

    #[test]
    fn save_and_reload_preserves_overrides() {
        use crate::config::PolishMode;
        let conn = open_init();
        let mut cfg = load_app_config_at(&conn).unwrap();
        cfg.polish_mode = PolishMode::Intermediate;
        cfg.microphone = "My Mic".into();
        cfg.segment_silence = 350.0;
        cfg.denoise_mode = 2;
        cfg.download_mirror = "https://hf-mirror.com".to_string();
        save_app_config_at(&conn, &cfg).unwrap();

        let cfg2 = load_app_config_at(&conn).unwrap();
        assert_eq!(cfg2.polish_mode, PolishMode::Intermediate);
        assert_eq!(cfg2.microphone, "My Mic");
        assert_eq!(cfg2.segment_silence, 350.0);
        assert_eq!(cfg2.denoise_mode, 2);
        assert_eq!(cfg2.download_mirror, "https://hf-mirror.com");
        // 未改字段保持 seed 默认
        assert_eq!(cfg2.language, "auto");
    }

    #[test]
    fn save_config_key_overrides_single_field() {
        let conn = open_init();
        conn.execute(
            "INSERT OR REPLACE INTO app_config (config_key, config_value) VALUES (?1, ?2)",
            params!["language", "ja"],
        ).unwrap();
        let cfg = load_app_config_at(&conn).unwrap();
        assert_eq!(cfg.language, "ja");
        assert_eq!(cfg.engine_mode, "embedded"); // 其余不变
    }

    #[test]
    fn load_with_missing_row_keeps_default() {
        let conn = open_init();
        // 删掉一行，load 应保留 default
        conn.execute("DELETE FROM app_config WHERE config_key='denoise_mode'", []).unwrap();
        let cfg = load_app_config_at(&conn).unwrap();
        assert_eq!(cfg.denoise_mode, 1); // AppConfig::default() 的值
    }

    #[test]
    fn save_preserves_description_and_category() {
        let conn = open_init();
        // 验证 seed 有 description
        let desc: String = conn
            .query_row(
                "SELECT description FROM app_config WHERE config_key='language'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(!desc.is_empty(), "seed 的 description 不应为空");

        // 单键写入后 description 应保留（INSERT OR REPLACE 会清空，ON CONFLICT 不会）
        conn.execute(
            "INSERT INTO app_config (config_key, config_value) VALUES (?1, ?2)\n             ON CONFLICT(config_key) DO UPDATE SET config_value = excluded.config_value",
            params!["language", "zh"],
        ).unwrap();
        let (val, desc2): (String, String) = conn
            .query_row(
                "SELECT config_value, description FROM app_config WHERE config_key='language'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(val, "zh");
        assert_eq!(desc2, desc, "description 应被保留");

        // save_config_key 路径也保留
        // （save_config_key 走 with_db，需全局 DB 初始化；这里测底层 SQL 一致性即可）

        // save_app_config_at 全量写也保留
        let mut cfg = load_app_config_at(&conn).unwrap();
        cfg.language = "en".into();
        save_app_config_at(&conn, &cfg).unwrap();
        let (val3, desc3, cat3): (String, String, String) = conn
            .query_row(
                "SELECT config_value, description, category FROM app_config WHERE config_key='language'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!(val3, "en");
        assert_eq!(desc3, desc, "save_app_config_at 应保留 description");
        assert_eq!(cat3, "setting", "category 应为 setting");
    }

    #[test]
    fn app_config_category_defaults_to_setting() {
        let conn = open_init();
        let categories: Vec<String> = conn
            .prepare("SELECT DISTINCT category FROM app_config")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .filter_map(|r| r.ok())
            .collect();
        assert!(
            categories.contains(&"setting".to_string()) && categories.contains(&"env".to_string()),
            "category 应包含 'setting' 和 'env'，实际: {:?}", categories
        );
    }

    /// DB seed 中 env 变量 config_key 不含 env. 前缀。
    #[test]
    fn env_var_keys_have_no_env_prefix() {
        let conn = open_init();
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM app_config WHERE category='env' AND config_key LIKE 'env.%'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 0, "env 变量 config_key 不应含 env. 前缀");

        // 验证 bare key 存在
        let hf: String = conn
            .query_row(
                "SELECT config_value FROM app_config WHERE config_key='huggingface' AND category='env'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(!hf.is_empty(), "huggingface 环境变量应有值");
    }
}
