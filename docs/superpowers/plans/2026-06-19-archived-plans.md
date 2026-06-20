# 归档实施计划（2026-06-18 ~ 2026-06-19）

> **归档说明**（2026-06-19）：以下 7 个 plan 对应功能均已实现并合并 main，各自文档原样合并归档于此，原独立文件已删除。每个章节以 `📄 <原文件名>` 标注来源。
> **交叉引用**：正文内 `[xxx.md](./xxx.md)` 链接为合并前原文件名，现指向本归档文件内同名章节；对应 specs 见 `docs/superpowers/specs/2026-06-19-archived-design.md`。

---


## 📄 `2026-06-18-config-db-migration.md`

# 配置 DB 迁移实施计划（config.yaml → app_config 表）

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 将全部 21 个 `config.yaml` 配置字段迁移到 SQLite `app_config` 表，实现配置存储统一化（与模型配置/识别历史共用同一 DB），消除 yaml 序列化/字段迁移的复杂度。

**Architecture:** 在 `infra::db` 新增 `app_config` 表（key-value TEXT 存储），`load_config()` 改为从 DB 读取并按字段类型解析，`persist_*` / `set_config` 改为 DB 写入。首次启动时自动将旧 `config.yaml` 导入 DB 后重命名为 `.bak`。`user_version` 最终升至 3（v0/v1→v3 直跳，v2→v3 ALTER TABLE 补 category 列）。写策略用 `ON CONFLICT DO UPDATE SET config_value`（保留 description + category，不用 INSERT OR REPLACE）。

**Tech Stack:** rusqlite（现有）、serde（现有，JSON 序列化用）、serde_yaml（仅 yaml 迁移一次性使用）

> **状态（2026-06-18）：已实施并合并到 main**（`8441ca8` settings-ui 合并：app_config 表 + v1→v3 迁移 + category 列 + ON CONFLICT 写策略）。下方 checkbox 标记实际完成进度；v3 category 列增量见 `16a6f22`。

---

## 前置条件

- [x] **Step 0: 提交 polish_min_interval 重命名**

当前有 5 个文件未提交（`polish_interval` → `polish_min_interval` 重命名）。需先提交此变更，保持 DB 迁移分支干净。

```bash
git add crates/infra/src/config.rs crates/desktop/src/coordinator.rs crates/desktop/src/main.rs crates/desktop/src/settings_commands.rs crates/desktop/dist/settings/index.html
git commit -m "refactor: polish_interval → polish_min_interval 语义对齐（节流间隔）"
```

---

## 文件结构

| 文件 | 职责 | 变更类型 |
|------|------|---------|
| `crates/infra/src/db.sql` | DB schema + seed | 新增 `app_config` 表 + 21 行 seed |
| `crates/infra/src/db.rs` | DB 访问层 | 新增 `load_app_config` / `save_app_config` / `save_config_key` / yaml 迁移逻辑；更新 `init_schema` |
| `crates/infra/src/config.rs` | AppConfig 定义 + load_config | `load_config()` 改为薄包装调用 `db::load_app_config()`；移除 yaml 解析代码 |
| `crates/desktop/src/runtime_config.rs` | persist_* 命令 | `write_config_yaml` → `db::save_config_key`；移除 `write_config_yaml` 函数 |
| `crates/desktop/src/settings_commands.rs` | set_config 命令 | `write_config_yaml` → `db::save_app_config`；移除本地 `write_config_yaml` |
| `crates/desktop/src/main.rs` | 启动入口 | 无需改动（load_config 内部已变） |

---

## Task 1: db.sql — 新增 app_config 表 + seed

**Files:**
- Modify: `crates/infra/src/db.sql`（末尾追加）

- [x] **Step 1: 追加 app_config 建表 + seed SQL**

在 `db.sql` 末尾（最后一个 INSERT 之后）追加：

```sql

-- ── 应用配置（app_config 表）─────────────────────────────────────────────────
-- config.yaml 的 DB 化：所有应用行为配置（引擎/快捷键/润色/降噪等）以 key-value 存储。
-- 值统一 TEXT，由 Rust 侧 load_app_config 按字段类型解析。
-- 首次启动由 init_schema 执行 seed；后续 set_config / persist_* 通过 INSERT OR REPLACE 更新。

CREATE TABLE IF NOT EXISTS app_config (
    config_key   TEXT PRIMARY KEY,
    config_value TEXT NOT NULL,
    description  TEXT
);

INSERT OR IGNORE INTO app_config (config_key, config_value, description) VALUES
    ('engine_mode',              'embedded',                        'ASR 引擎模式: embedded | websocket | grpc'),
    ('remote_url',               'ws://127.0.0.1:3000/ws/stream',   'WebSocket 远程地址（engine_mode=websocket 时使用）'),
    ('grpc_endpoint',            'http://127.0.0.1:50051',          'gRPC 端点（engine_mode=grpc 时使用）'),
    ('asr_engine',               '',                                'ASR 引擎选择（DB models 表 model_name 精确匹配；空=兜底引擎）'),
    ('language',                 'auto',                            '识别语言: auto | zh | en | ja | ko'),
    ('asr_shortcut',             'CmdOrCtrl+Shift+Space',           '全局 ASR 激活/关闭快捷键'),
    ('edit_shortcut',            'Cmd+E',                           '结果窗进入编辑快捷键（保存固定 Cmd+Enter）'),
    ('paste_method',             'clipboard',                       '粘贴方式: clipboard | direct | none'),
    ('write_to_clipboard',       'true',                            '粘贴后是否把结果写入剪贴板'),
    ('microphone',               '',                                '麦克风名称（空=系统默认）'),
    ('overlay_position',         'top',                             'overlay 位置: top | bottom | none'),
    ('segment_silence',          '400',                             'VAD 静音触发识别阈值（毫秒）'),
    ('polish_mode',              '0',                               '润色模式: 0=关闭 / 1=仅最终 / 2=中间+最终'),
    ('polish_min_interval',      '5',                               '中间润色最小间隔（秒，节流用）'),
    ('pause_polish_threshold_ms','600',                             '停顿驱动中间润色的静音阈值（毫秒，必须 > 500）'),
    ('polish_llm',               'bigmodel:glm:glm-4-flashx',       '润色 LLM 模型 spec（PREFIX:CATEGORY:NAME）'),
    ('asr_hardware_accelerated', 'false',                           '是否使用 ASR 硬件加速'),
    ('asr_correct',              'false',                           '是否对 ASR 输出进行纠错'),
    ('output_simplified',        'true',                            'ASR 输出字形: true=简体 / false=繁体'),
    ('hide_toolbar',             'true',                            '结果展示区工具栏是否自动隐藏'),
    ('denoise_mode',             '1',                               '降噪模式: 0=无 / 1=轻度 / 2=深度');
```

- [x] **Step 2: 验证 SQL 语法**

```bash
# 确认无语法错误（编译时会 include_str!，cargo check 验证）
cargo check -p octopus-infra
```

Expected: 编译通过（INIT_SQL 是 include_str!，编译期不校验 SQL 内容，但确保无 Rust 错误）

- [x] **Step 3: Commit**

```bash
git add crates/infra/src/db.sql
git commit -m "feat(infra): 新增 app_config 表 schema + 21 字段 seed（DB 配置迁移基础）"
```

---

## Task 2: db.rs — DB 读写函数 + 迁移逻辑

**Files:**
- Modify: `crates/infra/src/db.rs`

- [x] **Step 1: 在 db.rs 添加 load_app_config 函数**

在 `// ── DB → AsrConfig（load_config 用）──` 区块之前，新增 app_config 读写区块。需要 `use crate::config::{AppConfig, PolishMode};`（同 crate，无循环依赖）。

```rust
// ── app_config 表读写（替代 config.yaml）──

/// 从 DB app_config 表加载完整应用配置。
/// 先构造 AppConfig::default()（保底），再用 DB 行按字段类型解析覆盖。
/// 缺失行或解析失败 → 保留 default 值（防御性，正常不应触发——seed 保证 21 行齐全）。
pub fn load_app_config() -> Result<AppConfig> {
    ensure_db()?;
    with_db(|conn| load_app_config_at(conn))
}

fn load_app_config_at(conn: &Connection) -> Result<AppConfig> {
    let mut cfg = crate::config::AppConfig::default();
    let mut stmt = conn.prepare("SELECT config_key, config_value FROM app_config")?;
    let rows = stmt.query_map([], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    })?;

    for row in rows {
        let (key, value) = row?;
        match key.as_str() {
            // 字符串字段：直接赋值
            "engine_mode" => cfg.engine_mode = value,
            "remote_url" => cfg.remote_url = value,
            "grpc_endpoint" => cfg.grpc_endpoint = value,
            "asr_engine" => cfg.asr_engine = value,
            "language" => cfg.language = value,
            "asr_shortcut" => cfg.asr_shortcut = value,
            "edit_shortcut" => cfg.edit_shortcut = value,
            "paste_method" => cfg.paste_method = value,
            "microphone" => cfg.microphone = value,
            "overlay_position" => cfg.overlay_position = value,
            "polish_llm" => cfg.polish_llm = value,
            // bool 字段：parse 失败保留 default
            "write_to_clipboard" => { if let Ok(v) = value.parse() { cfg.write_to_clipboard = v; } }
            "asr_hardware_accelerated" => { if let Ok(v) = value.parse() { cfg.asr_hardware_accelerated = v; } }
            "asr_correct" => { if let Ok(v) = value.parse() { cfg.asr_correct = v; } }
            "output_simplified" => { if let Ok(v) = value.parse() { cfg.output_simplified = v; } }
            "hide_toolbar" => { if let Ok(v) = value.parse() { cfg.hide_toolbar = v; } }
            // f64 字段
            "segment_silence" => { if let Ok(v) = value.parse() { cfg.segment_silence = v; } }
            "polish_min_interval" => { if let Ok(v) = value.parse() { cfg.polish_min_interval = v; } }
            "pause_polish_threshold_ms" => { if let Ok(v) = value.parse() { cfg.pause_polish_threshold_ms = v; } }
            // u8 枚举字段
            "polish_mode" => {
                if let Ok(n) = value.parse::<u8>() {
                    cfg.polish_mode = match n {
                        1 => PolishMode::FinalOnly,
                        2 => PolishMode::Intermediate,
                        _ => PolishMode::Disabled,
                    };
                }
            }
            "denoise_mode" => { if let Ok(v) = value.parse() { cfg.denoise_mode = v; } }
            _ => {} // 忽略未知 key（前向兼容）
        }
    }
    Ok(cfg)
}
```

- [x] **Step 2: 添加 save_app_config（全量写）和 save_config_key（单键写）**

```rust
/// 全量写入应用配置（21 字段 INSERT OR REPLACE）。set_config / yaml 迁移用。
pub fn save_app_config(cfg: &AppConfig) -> Result<()> {
    ensure_db()?;
    with_db(|conn| save_app_config_at(conn, cfg))
}

fn save_app_config_at(conn: &Connection, cfg: &AppConfig) -> Result<()> {
    let polish_mode_u8 = match cfg.polish_mode {
        PolishMode::Disabled => 0u8,
        PolishMode::FinalOnly => 1,
        PolishMode::Intermediate => 2,
    };
    let fields: [(&str, String); 21] = [
        ("engine_mode", cfg.engine_mode.clone()),
        ("remote_url", cfg.remote_url.clone()),
        ("grpc_endpoint", cfg.grpc_endpoint.clone()),
        ("asr_engine", cfg.asr_engine.clone()),
        ("language", cfg.language.clone()),
        ("asr_shortcut", cfg.asr_shortcut.clone()),
        ("edit_shortcut", cfg.edit_shortcut.clone()),
        ("paste_method", cfg.paste_method.clone()),
        ("write_to_clipboard", cfg.write_to_clipboard.to_string()),
        ("microphone", cfg.microphone.clone()),
        ("overlay_position", cfg.overlay_position.clone()),
        ("segment_silence", cfg.segment_silence.to_string()),
        ("polish_mode", polish_mode_u8.to_string()),
        ("polish_min_interval", cfg.polish_min_interval.to_string()),
        ("pause_polish_threshold_ms", cfg.pause_polish_threshold_ms.to_string()),
        ("polish_llm", cfg.polish_llm.clone()),
        ("asr_hardware_accelerated", cfg.asr_hardware_accelerated.to_string()),
        ("asr_correct", cfg.asr_correct.to_string()),
        ("output_simplified", cfg.output_simplified.to_string()),
        ("hide_toolbar", cfg.hide_toolbar.to_string()),
        ("denoise_mode", cfg.denoise_mode.to_string()),
    ];
    for (key, value) in &fields {
        conn.execute(
            "INSERT OR REPLACE INTO app_config (config_key, config_value) VALUES (?1, ?2)",
            params![key, value],
        )?;
    }
    Ok(())
}

/// 单键写入（persist_* 命令用，避免全量回写）。
pub fn save_config_key(key: &str, value: &str) -> Result<()> {
    ensure_db()?;
    with_db(|conn| {
        conn.execute(
            "INSERT OR REPLACE INTO app_config (config_key, config_value) VALUES (?1, ?2)",
            params![key, value],
        )?;
        Ok(())
    })
}
```

- [x] **Step 3: 简化 init_schema 支持 v0→v2 和 v1→v2**

**关键洞察**：INIT_SQL 全部是 `CREATE TABLE IF NOT EXISTS` + `INSERT OR IGNORE`，幂等。v1→v2 直接重跑 INIT_SQL 即可——旧表/旧 seed 行跳过，新 app_config 表 + seed 落地。无需单独的迁移 SQL。

替换现有 `init_schema` 函数：

```rust
/// 初始化 schema + 迁移：
/// - v0（全新安装）: 执行 INIT_SQL → yaml 迁移 → v2
/// - v1（旧版升级）: 重跑 INIT_SQL（幂等，补建 app_config + seed）→ yaml 迁移 → v2
/// - v2+: 跳过
///
/// INIT_SQL 全部为 CREATE TABLE IF NOT EXISTS + INSERT OR IGNORE，幂等安全重跑。
fn init_schema(conn: &Connection) -> Result<()> {
    let v: u32 = conn
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .context("query user_version")?;

    if v < 2 {
        // v0: 首次建表 + seed；v1: 幂等重跑（旧表跳过，app_config 新建 + seed）
        conn.execute_batch(INIT_SQL).context("执行 db.sql 初始化失败")?;
        // 一次性 yaml → DB 迁移
        migrate_yaml_to_db(conn)?;
        conn.execute("PRAGMA user_version = 2", [])?;
        log::info!("DB initialized (v2): schema + app_config + yaml migration");
    }
    Ok(())
}
```

- [x] **Step 4: 添加 migrate_yaml_to_db 函数**

```rust
/// 一次性 yaml → DB 迁移：config.yaml 存在时解析 → INSERT OR REPLACE 覆盖 seed → 重命名为 .bak。
/// 幂等：config.yaml 不存在时直接返回。
fn migrate_yaml_to_db(conn: &Connection) -> Result<()> {
    let config_path = crate::octopus_config_home().join("config.yaml");
    if !config_path.exists() {
        return Ok(());
    }

    let text = std::fs::read_to_string(&config_path)
        .with_context(|| format!("读取旧 config.yaml 失败: {}", config_path.display()))?;

    // 复用 config.rs 的字段名迁移逻辑（shortcut → asr_shortcut 等）
    let mut value: serde_yaml::Value = serde_yaml::from_str(&text)?;
    if let Some(map) = value.as_mapping_mut() {
        migrate_yaml_key(map, "shortcut", "asr_shortcut");
        migrate_yaml_key(map, "polish_interval", "polish_min_interval");
    }
    let cfg: crate::config::AppConfig = serde_yaml::from_value(value)?;

    // 覆盖 seed 默认值（INSERT OR REPLACE）
    save_app_config_at(conn, &cfg)?;

    // 重命名旧文件
    let bak = config_path.with_extension("yaml.bak");
    let _ = std::fs::rename(&config_path, &bak);
    log::info!(
        "config.yaml → app_config 迁移完成（备份: {}）",
        bak.display()
    );
    Ok(())
}

/// yaml 字段名迁移：旧键存在时，新键不存在则迁移、新键已存在则删旧留新。
fn migrate_yaml_key(map: &mut serde_yaml::Mapping, old: &str, new: &str) {
    let old_key = serde_yaml::Value::String(old.into());
    let new_key = serde_yaml::Value::String(new.into());
    if map.get(&old_key).is_some() {
        if map.get(&new_key).is_none() {
            let old_val = map.remove(&old_key).unwrap();
            map.insert(new_key, old_val);
        } else {
            map.remove(&old_key);
        }
    }
}
```

- [x] **Step 5: 编译验证**

```bash
cargo check -p octopus-infra
```

Expected: 编译通过（新增函数未被调用，仅有 unused warning，可接受）

- [x] **Step 6: 写测试——load/save 往返 + 单键写**

在 db.rs 的 `#[cfg(test)] mod tests` 中追加（如果已有 test mod 则追加，否则新建）：

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::AppConfig;

    /// 内存 DB + init_schema，返回 conn。
    fn open_test_db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(INIT_SQL).unwrap();
        conn
    }

    #[test]
    fn app_config_seed_provides_all_fields() {
        let conn = open_test_db();
        let cfg = load_app_config_at(&conn).unwrap();
        // seed 默认值校验（抽样关键字段）
        assert_eq!(cfg.engine_mode, "embedded");
        assert_eq!(cfg.language, "auto");
        assert!(cfg.write_to_clipboard);
        assert!(!cfg.asr_hardware_accelerated);
        assert_eq!(cfg.segment_silence, 400.0);
        assert_eq!(cfg.polish_min_interval, 5.0);
        assert_eq!(cfg.denoise_mode, 1);
        assert_eq!(cfg.edit_shortcut, "Cmd+E");
    }

    #[test]
    fn save_and_reload_preserves_overrides() {
        let conn = open_test_db();
        let mut cfg = load_app_config_at(&conn).unwrap();
        cfg.asr_engine = "whisper-small".into();
        cfg.polish_mode = PolishMode::Intermediate;
        cfg.microphone = "My Mic".into();
        cfg.segment_silence = 350.0;
        cfg.denoise_mode = 2;
        save_app_config_at(&conn, &cfg).unwrap();

        let cfg2 = load_app_config_at(&conn).unwrap();
        assert_eq!(cfg2.asr_engine, "whisper-small");
        assert_eq!(cfg2.polish_mode, PolishMode::Intermediate);
        assert_eq!(cfg2.microphone, "My Mic");
        assert_eq!(cfg2.segment_silence, 350.0);
        assert_eq!(cfg2.denoise_mode, 2);
        // 未改字段保持 seed 默认
        assert_eq!(cfg2.language, "auto");
    }

    #[test]
    fn save_config_key_overrides_single_field() {
        let conn = open_test_db();
        conn.execute(
            "INSERT OR REPLACE INTO app_config (config_key, config_value) VALUES (?1, ?2)",
            params!["asr_engine", "sensevoice-test"],
        ).unwrap();
        let cfg = load_app_config_at(&conn).unwrap();
        assert_eq!(cfg.asr_engine, "sensevoice-test");
        assert_eq!(cfg.language, "auto"); // 其余不变
    }

    #[test]
    fn load_with_missing_row_keeps_default() {
        let conn = open_test_db();
        // 删掉一行，load 应保留 default
        conn.execute("DELETE FROM app_config WHERE config_key='denoise_mode'", []).unwrap();
        let cfg = load_app_config_at(&conn).unwrap();
        assert_eq!(cfg.denoise_mode, 1); // AppConfig::default() 的值
    }
}
```

- [x] **Step 7: 运行测试验证通过**

```bash
cargo test -p octopus-infra -- db::tests
```

Expected: 4 tests passed

- [x] **Step 8: Commit**

```bash
git add crates/infra/src/db.rs
git commit -m "feat(infra): app_config DB 读写 + v1→v2 迁移 + yaml 导入逻辑"
```

---

## Task 3: config.rs — load_config 切换到 DB

**Files:**
- Modify: `crates/infra/src/config.rs:243-275`（load_config + migrate_key 函数）

- [x] **Step 1: 重写 load_config 为薄包装**

将 `config.rs:243-275`（`load_config` + `migrate_key` 函数整体）替换为：

```rust
/// 从 DB app_config 表加载应用配置。
///
/// 内部委托 `db::load_app_config()`（首次调用时触发 ensure_db + init_schema，
/// 包含 yaml → DB 一次性迁移）。不缓存——调用方各自决定是否缓存
/// （asr 侧引擎配置另有 OnceLock 缓存）。
pub fn load_config() -> Result<AppConfig> {
    crate::db::load_app_config()
}
```

**注意：** `migrate_key` 函数从 config.rs 移除（逻辑已移至 db.rs `migrate_yaml_key`）。yaml 迁移不再发生在 load_config 中，而是在 `init_schema` → `migrate_yaml_to_db` 中一次性完成。

- [x] **Step 2: 编译验证**

```bash
cargo check -p octopus-infra
```

Expected: 编译通过。如果有 unused import 警告（`serde_yaml` 不再在生产代码使用），暂忽略——测试仍在用。

- [x] **Step 3: 更新 config.rs 中的 yaml 相关测试**

config.rs 现有测试大量使用 `serde_yaml::from_str::<AppConfig>("")` 来测试默认值。这些测试验证的是 serde 反序列化默认值，**在 DB 化后仍有意义**（验证 AppConfig struct 的 serde 行为正确，用于 yaml 迁移路径中的 `serde_yaml::from_value`）。

但 `load_config_migrates_shortcut_to_asr_shortcut` 和 `load_config_both_keys_drops_old` 这两个测试验证的是迁移逻辑——现在迁移逻辑在 db.rs 的 `migrate_yaml_key` 中。需更新这两个测试：

删除 config.rs 中这两个测试（`load_config_migrates_shortcut_to_asr_shortcut` 和 `load_config_both_keys_drops_old`），因为：
1. 迁移逻辑已移至 db.rs
2. 它们测试的是 yaml 层迁移细节，不是 load_config 行为
3. db.rs 的测试已覆盖 load/save 行为

同时将 `write_to_clipboard_defaults_to_true`、`pause_polish_threshold_ms_defaults_to_600`、`asr_hardware_accelerated_defaults_to_false`、`asr_correct_defaults_to_false`、`denoise_mode_defaults_to_rnnoise_when_absent`、`edit_shortcut_defaults_to_cmd_e` 这些测默认值的测试改为验证 `AppConfig::default()`：

```rust
    #[test]
    fn app_config_default_values() {
        let cfg = AppConfig::default();
        assert!(cfg.write_to_clipboard, "write_to_clipboard 应默认 true");
        assert_eq!(cfg.pause_polish_threshold_ms, 600.0);
        assert!(!cfg.asr_hardware_accelerated);
        assert!(!cfg.asr_correct);
        assert_eq!(cfg.denoise_mode, 1);
        assert_eq!(cfg.edit_shortcut, "Cmd+E");
        assert_eq!(cfg.segment_silence, 400.0);
    }
```

删除被合并的 6 个独立默认值测试。保留 `polish_mode_deserialize_values`、`polish_mode_invalid_falls_back_to_disabled`、`polish_mode_default_is_disabled`（验证 PolishMode serde）、`app_config_serialize_round_trip_preserves_overrides`（验证序列化往返）、`denoise_mode_explicit_from_yaml` / `denoise_mode_legacy_denoise_enabled_ignored` / `edit_shortcut_explicit_from_yaml`（验证 serde 解析行为）。

- [x] **Step 4: 运行 infra 测试**

```bash
cargo test -p octopus-infra
```

Expected: 所有测试通过

- [x] **Step 5: Commit**

```bash
git add crates/infra/src/config.rs
git commit -m "refactor(infra): load_config 切换为 DB 读取 + 清理 yaml 迁移测试"
```

---

## Task 4: runtime_config.rs — persist_* 改用 DB 单键写

**Files:**
- Modify: `crates/desktop/src/runtime_config.rs:105-140`（persist_* + write_config_yaml）

- [x] **Step 1: 重写 4 个 persist_* 函数 + 移除 write_config_yaml**

将 `runtime_config.rs:105-140`（从 `// ── config.yaml 写回 ──` 到 `write_config_yaml` 函数结束）替换为：

```rust
// ── 配置持久化（DB app_config 表）──

/// 单键写入 DB（运行时由 RuntimeConfig 负责，此处仅持久化）。
pub fn persist_asr_engine(value: &str) -> Result<(), String> {
    octopus_infra::db::save_config_key("asr_engine", value).map_err(|e| e.to_string())
}

pub fn persist_polish_mode(value: u8) -> Result<(), String> {
    octopus_infra::db::save_config_key("polish_mode", &value.to_string()).map_err(|e| e.to_string())
}

pub fn persist_polish_llm(value: &str) -> Result<(), String> {
    octopus_infra::db::save_config_key("polish_llm", value).map_err(|e| e.to_string())
}

pub fn persist_denoise_mode(value: u8) -> Result<(), String> {
    octopus_infra::db::save_config_key("denoise_mode", &value.to_string()).map_err(|e| e.to_string())
}
```

- [x] **Step 2: 编译验证**

```bash
cargo check -p octopus-desktop --features embedded
```

Expected: 编译通过

- [x] **Step 3: Commit**

```bash
git add crates/desktop/src/runtime_config.rs
git commit -m "refactor(desktop): persist_* 改用 DB 单键写入（移除 write_config_yaml）"
```

---

## Task 5: settings_commands.rs — set_config 改用 DB 全量写

**Files:**
- Modify: `crates/desktop/src/settings_commands.rs:74,228-232`（set_config 中的 write_config_yaml 调用 + 本地 write_config_yaml 函数）

- [x] **Step 1: set_config 中的 write_config_yaml 替换为 DB 写入**

将 `settings_commands.rs:74` 处的 `write_config_yaml(&cfg)?;` 替换为：

```rust
    octopus_infra::db::save_app_config(&cfg).map_err(|e| e.to_string())?;
```

- [x] **Step 2: 移除 settings_commands.rs 中的 write_config_yaml 函数**

删除 `settings_commands.rs:228-232`：

```rust
fn write_config_yaml(cfg: &octopus_infra::config::AppConfig) -> Result<(), String> {
    let path = octopus_infra::octopus_config_home().join("config.yaml");
    let text = serde_yaml::to_string(cfg).map_err(|e| e.to_string())?;
    std::fs::write(&path, text).map_err(|e| e.to_string())
}
```

整个函数删除。

- [x] **Step 3: 编译 + 测试验证**

```bash
cargo test -p octopus-desktop --features "embedded dashscope"
```

Expected: 编译通过，所有测试通过（settings_commands 的测试测的是 apply_config_value，不涉及持久化）

- [x] **Step 4: Commit**

```bash
git add crates/desktop/src/settings_commands.rs
git commit -m "refactor(desktop): set_config 持久化切换到 DB（移除本地 write_config_yaml）"
```

---

## Task 6: 全量集成测试

- [x] **Step 1: 全量编译**

```bash
cargo build --release -p octopus-server -p octopus-cli
cargo build --release -p octopus-desktop --features embedded
```

Expected: 全部编译通过

- [x] **Step 2: 运行所有相关测试**

```bash
cargo test -p octopus-infra
cargo test -p octopus-desktop --features "embedded dashscope"
```

Expected: 全部通过

- [x] **Step 3: 手动验证迁移流程（开发环境）**

```bash
# 1. 确保 ~/.octopus/config.yaml 存在（当前开发环境的配置）
cat ~/.octopus/config.yaml

# 2. 备份当前 DB
cp ~/.octopus/octopus.db ~/.octopus/octopus.db.bak

# 3. 启动应用（触发迁移）
cargo run --release -p octopus-desktop --features embedded

# 4. 验证：config.yaml 被重命名为 config.yaml.bak
ls ~/.octopus/config.yaml*  # 应看到 .bak，无原文件

# 5. 验证：DB app_config 表有数据
sqlite3 ~/.octopus/octopus.db "SELECT config_key, config_value FROM app_config LIMIT 5"

# 6. 验证：设置界面能读取配置（打开设置窗口检查值是否正确）

# 7. 验证：修改设置后值持久化（改一个设置 → 重启 → 值保留）
```

- [x] **Step 4: 恢复开发环境（可选）**

如果手动验证修改了配置，恢复备份：
```bash
cp ~/.octopus/octopus.db.bak ~/.octopus/octopus.db
mv ~/.octopus/config.yaml.bak ~/.octopus/config.yaml
```

---

## Task 7: 文档同步

**Files:**
- Modify: `docs/architecture.md`
- Modify: `docs/configuration.md`
- Create: `docs/superpowers/specs/2026-06-18-config-db-migration-design.md`

- [x] **Step 1: 更新 architecture.md**

更新「两套配置系统」表格（原区分 config.yaml vs octopus.db，现在统一到 DB）：

```markdown
### 统一 DB 存储（v2+）

所有配置统一存储在 `~/.octopus/octopus.db`（SQLite），不再使用 config.yaml：

| 表 | 用途 | 初始化方式 |
|----|------|-----------|
| `models` | 引擎/LLM 模型配置 | db.sql seed |
| `transcriptions` | 识别历史 | 运行时写入 |
| `app_config` | 应用行为配置（21 字段） | db.sql seed + yaml 迁移 |

- **配置加载**：`infra::config::load_config()` → `db::load_app_config()` 从 app_config 表读取
- **配置持久化**：`persist_*`（单键 UPDATE）、`set_config`（全量 INSERT OR REPLACE）
- **yaml 迁移**：首次启动检测旧 config.yaml → 导入 DB → 重命名 .bak（一次性）
- **引擎激活唯一真相**：`app_config.asr_engine`（DB models 表无 is_active 列）
```

- [x] **Step 2: 更新 configuration.md**

更新配置文件描述（config.yaml → DB app_config 表），保留字段说明表格。

- [x] **Step 3: 创建 spec 文档**

创建 `docs/superpowers/specs/2026-06-18-config-db-migration-design.md`，记录设计决策：

```markdown
# 配置 DB 迁移设计

## 背景
config.yaml 与 octopus.db 两套存储系统并存，yaml 需独立维护序列化/字段迁移逻辑。

## 方案
将 config.yaml 21 个字段迁移到 SQLite app_config 表（key-value TEXT 存储）。

## 关键决策
1. **TEXT 统一存储**：bool/f64/u8 序列化为 TEXT，load 时按字段类型 parse
2. **seed 幂等**：INSERT OR IGNORE，已有配置不被覆盖
3. **yaml 一次性迁移**：init_schema 检测 config.yaml → 覆盖 seed → 重命名 .bak
4. **v1→v2 迁移**：user_version 升级补建 app_config 表
5. **单键写 vs 全量写**：persist_* 用 save_config_key（单键），set_config 用 save_app_config（全量）

## 排除方案
- **serde alias 迁移字段名**：两键共存时 duplicate field panic，改为 yaml Value 层手动迁移
- **category 分组列**：YAGNI，21 个扁平 key-value 无分组需求
```

- [x] **Step 4: Commit 文档**

```bash
git add docs/architecture.md docs/configuration.md docs/superpowers/specs/2026-06-18-config-db-migration-design.md docs/superpowers/plans/2026-06-18-config-db-migration.md
git commit -m "docs: 配置 DB 迁移设计文档 + 架构/配置文档同步"
```

---

## 风险与回退

### 风险
1. **DB 初始化失败** → load_config 返回 Err → 应用无法启动。缓解：ensure_db 已有 best-effort 逻辑。
2. **yaml 迁移解析失败** → migrate_yaml_to_db 返回 Err → init_schema 失败。缓解：解析用 serde 默认值兜底。
3. **并发写入** → 多线程同时 set_config。缓解：with_db 内部 Mutex<Connection> 串行化。

### 回退
如需回退到 yaml：
```bash
mv ~/.octopus/config.yaml.bak ~/.octopus/config.yaml
# 降级代码到迁移前版本
cargo build --release -p octopus-desktop --features embedded
```
DB 中 app_config 表对旧版代码无害（旧版不读此表）。


## 📄 `2026-06-18-dashscope-streaming.md`

# DashScope 云端流式 ASR 实施计划

> **状态：✅ 已实现**（2026-06-18）
>
> **For agentic workers:** REQUIRED SUB-SKILL: 用 superpowers:subagent-driven-development 或 superpowers:executing-plans 逐任务实现。

**Goal:** 实现 VAD-gated per-utterance streaming——VAD 检测语音 onset → 开 WSS 长连接推 PCM 收 partial → 静音 ≥ 700ms 断开。

**Architecture:** 新建 `dashscope_stream.rs`（`DashScopeStreamSession`），coordinator 新增 `Stage::CloudStreaming` + `CloudStreamingTick`。详见 spec `docs/superpowers/specs/2026-06-18-dashscope-streaming-design.md`。

**Tech Stack:** Rust + Tauri；tokio-tungstenite（WS）；tokio::select!（双向异步循环）。

---

## File Structure

- **新建：** `crates/desktop/src/dashscope_stream.rs`
- **改：** `crates/desktop/src/coordinator.rs`（Stage + Command + handler + toggle routing）
- **改：** `crates/desktop/src/main.rs`（注册模块）
- **改：** `crates/desktop/src/engine_dashscope.rs`（`samples_to_pcm_s16le` 改 `pub(crate)`）

---

## Task 1: `dashscope_stream.rs` — DashScopeStreamSession  ✅

**文件：** `crates/desktop/src/dashscope_stream.rs`（新建）

实现 `DashScopeStreamSession`：
- `open(rt, endpoint, key, model, language, pre_roll_samples) -> Result<Self>`
  - 在 tokio runtime 上 spawn `run_ws_session`
  - 两条 unbounded channel：PCM（coordinator→sender）、result（reader→coordinator）
- `push_pcm(&[f32]) -> Result<()>`：非阻塞
- `try_recv_text() -> Option<StreamEvent>`：非阻塞
- `close(self, rt) -> Result<String>`：发 Finish + 阻塞等最终结果

`run_ws_session` async fn：
- 建连（`connect_async` + bearer header）
- 发 run-task（含 `max_sentence_silence: 600`）
- 推 pre-roll PCM
- `tokio::select!` 双向循环：
  - `pcm_rx.recv()` → send binary / finish-task
  - `ws.next()` → parse result-generated / task-finished / task-failed

`StreamEvent` enum：`Text(String)` / `Finished` / `Failed(String)`

**验证：** `cargo check -p octopus-desktop --features "embedded dashscope"`

---

## Task 2: coordinator — Stage + Command + 常量  ✅

**文件：** `crates/desktop/src/coordinator.rs`

1. 新增 `Command::CloudStreamingTick`（`#[cfg(feature = "dashscope")]`）
2. 新增 `Stage::CloudStreaming` 变体（`#[cfg(feature = "dashscope")]`）：
   - `vad: SileroVad`（检测用，有状态累积）
   - `session: Option<DashScopeStreamSession>`（活跃 WSS）
   - `pre_roll_buffer: Vec<f32>`（滚动窗口 200ms）
   - `transcript: Transcript`
   - `silence_duration: f64`
   - `is_speaking: bool`
   - `tick_active: Arc<AtomicBool>`
3. 新增常量：`CLOUD_STREAMING_TICK_INTERVAL_MS=100` / `CLOUD_PREROLL_BUFFER_SAMPLES=3200` / `CLOUD_PREROLL_SAMPLES=1600`
4. 新增 `is_cloud_engine(&AppConfig) -> bool`

**验证：** `cargo check -p octopus-desktop --features "embedded dashscope"`

---

## Task 3: coordinator — Toggle 路由 + Tick 线程  ✅

**文件：** `crates/desktop/src/coordinator.rs`

1. Toggle Idle → CloudStreaming 分支：
   - 创建 VAD + pre-roll
   - `start_cloud_streaming_tick_thread`（tick → sleep，首 tick 立即触发）
2. `handle_toggle` 签名加 `use_cloud_streaming` 参数
3. command dispatch 加 `Command::CloudStreamingTick` → `handle_cloud_streaming_tick`
4. Toggle 停止（CloudStreaming → WaitingCompletion/Pasting）：
   - 停 tick + audio.stop
   - close WSS（如有）→ 拼接最终文本
   - 进入 Pasting

**验证：** `cargo check -p octopus-desktop --features "embedded dashscope"`

---

## Task 4: coordinator — `handle_cloud_streaming_tick`  ✅

**文件：** `crates/desktop/src/coordinator.rs`

实现 tick handler 逻辑：
1. `drain_samples()` → 追加到 pre_roll_buffer（超容量弹头）
2. VAD 检测 → `compute_speech_chunks`
3. 语音检测（≥2 chunks）→ `silence_duration=0` / 非语音 → `silence_duration += tick`
4. 无活跃 WSS + onset → 解析 DB（endpoint+key+model）→ `session.open()` + pre-roll + push PCM
5. 有活跃 WSS → push PCM + `try_recv_text` 更新 transcript + UI
6. 有活跃 WSS + silence ≥ pause_polish_threshold_ms → `close()` + 拼接文本 + 触发润色 + `session=None`

**验证：** `cargo check -p octopus-desktop --features "embedded dashscope"`

---

## Task 5: main.rs 注册 + engine_dashscope pub  ✅

**文件：** `crates/desktop/src/main.rs` + `crates/desktop/src/engine_dashscope.rs`

1. `main.rs` 加 `mod dashscope_stream;`（`#[cfg(feature = "dashscope")]`）
2. `engine_dashscope.rs`：`samples_to_pcm_s16le` 改 `pub(crate)`
3. 编译验证 + 测试

**验证：** `cargo test -p octopus-desktop --features "embedded dashscope"`

---

## Task 6: 文档同步 + 提交  ✅

- `docs/architecture.md`：补 CloudStreaming stage 说明
- spec/plan 标完成
- commit + merge


## 📄 `2026-06-18-editable-result-window.md`

# 结果展示区可编辑 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 在录音会话进行中允许用户编辑结果展示区文本（快捷键 edit_shortcut/按钮进入，快捷键/按钮退出），编辑期间 ASR 硬暂停；编辑后的文本作为后续展示与润色基准，新识别文本追加其上，停止粘贴时保留编辑；编辑后触发润色时只润色新增、保留已编辑（折回 edited + 边界提示词）。

**Architecture:** `Transcript` 新增 `edited` 字段，作为 `edited ≻ polished ≻ raw` 分层优先级最高层；`display_text()` = `committed + increase`。编辑是 coordinator 主循环里的 `editing` 标志——置位时两个 tick handler 跳过喂引擎、只排空丢弃音频（硬暂停）。提交时 `commit_edit` 写回 transcript 并 `UPDATE edited_text`。编辑×润色交互（spec §12）：`take_polish_input()` 返回 `(preserved=edited, to_polish=increase)`，LLM 仅润色 `to_polish`，`on_polish_done` 在 `has_edit()` 时把结果折回 `edited`（避免 edited 遮蔽 polished 导致丢字）。

**Tech Stack:** Rust（crates: infra, asr, llm, desktop）/ Tauri webview / 原生 HTML+JS（`dist/result/index.html`）/ SQLite（rusqlite）。

参考 spec：[`docs/superpowers/specs/2026-06-18-editable-result-window-design.md`](../specs/2026-06-18-editable-result-window-design.md)（§4 三文本模型、§5 提交语义、§12 编辑×润色交互）。

> **布局演进（2026-06-19）**：下方涉及 `edit-done`（完成编辑按钮）的实现步骤（DOM/CSS/JS）已被 [`2026-06-19-result-window-edit-layout`](2026-06-19-result-window-edit-layout.md) 取代——完成编辑按钮删除，保存入口迁 ✏️ toggle，编辑态文字不重排。下方 edit-done 代码块为历史实现记录，当前代码已无。
>
> **快捷键演进（2026-06-19）**：退出编辑已从「固定 `Cmd/Ctrl+Enter`」统一为 `edit_shortcut` **toggle**（进入/保存同键，默认 Cmd+Enter）。下方涉及 `Cmd/Ctrl+Enter` / 完成按钮的退出步骤为原始设计记录，当前代码已无。

---

## File Structure

| 文件 | 职责 | 改动任务 |
|---|---|---|
| `crates/desktop/src/transcript.rs` | 三文本状态机 | T1 编辑模型 + T5 润色模型 |
| `crates/llm/src/prompt.rs` + `client.rs` | 润色提示词 | T2 边界提示词 |
| `crates/infra/src/db.sql` + `db.rs` | DDL + DB 访问 | T3 edited_text 列 |
| `crates/desktop/src/coordinator.rs` | 状态机主循环 | T4 编辑机制 + T5 润色接线 + T6 停止路径 |
| `crates/desktop/src/main.rs` | Tauri 命令注册 | T4 |
| `crates/desktop/dist/result/index.html` + `icons/edit.svg` | 结果窗前端 | T7 |
| `docs/configuration.md` / `architecture.md` | 文档 | T8 |

> **编译绿原则**：每个任务结束 `cargo check -p <crate>` 通过。T1 只加「编辑模型」方法（不动 snapshot/on_polish_done，coordinator 照旧编译）；T2 改 polish 签名时同步把 coordinator 旧调用点改成 `polish(None, &text, ..)`；T5 才统一接 `(preserved, to_polish)`。

---

## Task 1: Transcript 编辑模型

**Files:**
- Modify: `crates/desktop/src/transcript.rs`

纯逻辑、单文件、完全可单测。只加「编辑相关」字段/方法（`edited`、`commit_edit`、`has_edit`、`edited_text`、`display_text` 优先级链、`edited_display`），**不动** `snapshot_for_polish` / `on_polish_done`（留给 T5），coordinator 照旧编译。`edited` 为空时 `display_text()` 与现有行为等价，现有测试不破。

- [x] **Step 1: 写失败测试**

在 `transcript.rs` 的 `#[cfg(test)] mod tests` 末尾追加：

```rust
#[test]
fn commit_edit_sets_edited_and_advances_boundary() {
    let mut t = Transcript::new(30, PolishMode::Intermediate);
    t.set_full("你好世界");
    t.snapshot_for_polish(); // T1 阶段仍用旧 snapshot；T5 替换
    t.on_polish_done("你好，世界。".into());
    assert_eq!(t.display_text(), "你好，世界。");

    t.commit_edit("你好世界（手改）");
    assert_eq!(t.edited_text(), Some("你好世界（手改）"));
    assert!(t.has_edit());
    // raw_len 推进到 full 末尾 → increase 清空
    assert_eq!(t.display_text(), "你好世界（手改）");
}

#[test]
fn commit_edit_preserves_raw_and_appends_new() {
    let mut t = Transcript::new(31, PolishMode::Intermediate);
    t.set_full("原文");
    t.commit_edit("原文（手改）");
    assert_eq!(t.full(), "原文"); // raw（full）原样保留
    t.set_full("原文新增");
    assert_eq!(t.display_text(), "原文（手改）新增"); // edited + 新增
}

#[test]
fn edited_takes_priority_over_polished_and_raw() {
    let mut t = Transcript::new(32, PolishMode::Intermediate);
    t.set_full("raw文本");
    t.snapshot_for_polish();
    t.on_polish_done("polished文本".into());
    t.commit_edit("edited文本".into());
    assert_eq!(t.display_text(), "edited文本"); // edited ≻ polished ≻ raw
}

#[test]
fn empty_commit_clears_edit_falls_back() {
    let mut t = Transcript::new(33, PolishMode::Intermediate);
    t.set_full("原文");
    t.commit_edit("手改".into());
    assert!(t.has_edit());
    t.commit_edit("");
    assert!(!t.has_edit());
    assert_eq!(t.edited_text(), None);
    assert_eq!(t.display_text(), "原文"); // 回退 raw
}

#[test]
fn edited_display_returns_display_when_edited_else_none() {
    let mut t = Transcript::new(34, PolishMode::Intermediate);
    t.set_full("原文");
    assert_eq!(t.edited_display(), None); // 未编辑
    t.commit_edit("手改".into());
    assert_eq!(t.edited_display().as_deref(), Some("手改"));
    t.set_full("原文新增");
    assert_eq!(t.edited_display().as_deref(), Some("手改新增")); // = display
}
```

- [x] **Step 2: 运行测试确认失败**

Run: `cargo test -p octopus-desktop transcript::tests`
Expected: 编译失败（`commit_edit`/`edited_text`/`has_edit`/`edited_display` 未定义）。

- [x] **Step 3: 加 `edited` 字段**

`Transcript` struct（`polished: String,` 下一行）加：

```rust
    /// 用户编辑后的 committed 文本（空 = 未编辑；非空时覆盖 polished/raw，优先级最高）。
    edited: String,
```

`new()`（`polished: String::new(),` 下一行）加：

```rust
            edited: String::new(),
```

- [x] **Step 4: 实现 `commit_edit` + 访问器**

`impl Transcript` 中（`on_polish_done` 附近）加：

```rust
/// 用户提交编辑：edited = 文本，raw_len 推进到 full 末尾（increase 清空），full（raw）不变。
/// 空串 → 清空 edited（回退到 polished/raw）。
pub fn commit_edit(&mut self, text: &str) {
    if text.is_empty() {
        self.edited.clear();
    } else {
        self.edited = text.to_string();
        self.raw_len = self.full.chars().count();
    }
}

/// 是否已编辑（edited 非空）。
pub fn has_edit(&self) -> bool {
    !self.edited.is_empty()
}

/// edited 文本（未编辑返回 None）。
pub fn edited_text(&self) -> Option<&str> {
    if self.edited.is_empty() {
        None
    } else {
        Some(&self.edited)
    }
}
```

- [x] **Step 5: 改 `display_text()` 优先级链**

替换现有 `display_text()`（123-132 行）：

```rust
/// 展示文本：committed 前缀 + increase。
/// committed 优先级：edited ≻ polished ≻ full[..raw_len]。
/// edited 为空时与旧行为等价（full[..raw_len] + full[raw_len..] = full）。
pub fn display_text(&self) -> String {
    let committed = if !self.edited.is_empty() {
        self.edited.clone()
    } else if !self.polished.is_empty() {
        self.polished.clone()
    } else {
        self.full.chars().take(self.raw_len).collect()
    };
    let inc: String = self.full.chars().skip(self.raw_len).collect();
    let mut s = committed;
    s.push_str(&inc);
    s
}
```

- [x] **Step 6: 加 `edited_display`**

`edited_text()` 旁加（停止路径无润色/兜底粘贴用，T6）：

```rust
/// 停止时喂给「无润色粘贴/兜底」的文本。
/// edited 非空 → display（用户编辑结果 + 新增，不补标点）。
/// 否则 None → 调用方走原 raw 逻辑（db_text + 按需补「。」）。
pub fn edited_display(&self) -> Option<String> {
    if self.edited.is_empty() {
        None
    } else {
        Some(self.display_text())
    }
}
```

- [x] **Step 7: 运行测试确认通过**

Run: `cargo test -p octopus-desktop transcript::tests`
Expected: 全 PASS（新增 5 个 + 现有测试；edited 空时 display 行为保持）。

- [x] **Step 8: 编译验证 desktop crate**

Run: `cargo check -p octopus-desktop --all-targets`
Expected: 通过（coordinator 未受影响——snapshot_for_polish/on_polish_done 未动）。

- [x] **Step 9: Commit**

```bash
git add crates/desktop/src/transcript.rs
git commit -m "feat(desktop): Transcript 编辑模型（edited 字段 + commit_edit + display 优先级链 + edited_display）"
```

---

## Task 2: llm 边界提示词（polish 加 preserved）

**Files:**
- Modify: `crates/llm/src/prompt.rs`
- Modify: `crates/llm/src/client.rs`（`polish` 签名）
- Modify: `crates/desktop/src/coordinator.rs`（2 处旧调用点改 `polish(None, ..)` 保持行为）
- Modify: `crates/llm/examples/test_polish.rs`（签名适配）

`polish` 签名加 `preserved: Option<&str>`；`user_prompt` 分块构造（已确认原样保留 + 新增润色）；system prompt 加增量保留规则。coordinator 旧调用点先传 `None`（保持现状），T5 再接真值。

- [x] **Step 1: 写失败测试 —— user_prompt 分块**

`crates/llm/src/prompt.rs` 末尾加测试模块（当前无 tests）：

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn user_prompt_without_preserved_is_plain() {
        let p = user_prompt(None, "你好");
        assert!(p.contains("请润色以下语音识别文本"));
        assert!(p.contains("你好"));
        assert!(!p.contains("已确认部分"));
    }

    #[test]
    fn user_prompt_with_preserved_marks_boundary() {
        let p = user_prompt(Some("已确认文本"), "新增文本");
        assert!(p.contains("已确认部分"));
        assert!(p.contains("原样保留"));
        assert!(p.contains("已确认文本"));
        assert!(p.contains("新增部分"));
        assert!(p.contains("新增文本"));
    }
}
```

- [x] **Step 2: 运行测试确认失败**

Run: `cargo test -p octopus-llm prompt::tests`
Expected: 编译失败（`user_prompt` 当前只接 `&str`）。

- [x] **Step 3: system prompt 加增量保留规则**

`DEFAULT_SYSTEM_PROMPT` 的 `# Rules` 列表（规则 6 后）加：

```markdown
7. [增量保留]：若用户提供【已确认部分】，该部分必须逐字原样保留、严禁修改，仅润色【新增部分】，最终输出两者拼接。
```

- [x] **Step 4: `user_prompt` 加 preserved**

替换 `user_prompt`：

```rust
/// 构建 user prompt。
/// - preserved=None：全量润色（to_polish = 完整文本）。
/// - preserved=Some：编辑后增量润色，告知 LLM 已确认部分原样保留、仅润色 to_polish。
pub fn user_prompt(preserved: Option<&str>, to_polish: &str) -> String {
    match preserved {
        None => format!("请润色以下语音识别文本：\n{}", to_polish),
        Some(confirmed) => format!(
            "以下文本中，【已确认部分】已经用户人工校对，必须原样保留、严禁修改；仅对【新增部分】进行润色。\n\n\
             【已确认部分（原样保留）】\n{}\n\n【新增部分（请润色）】\n{}\n\n\
             请输出：已确认部分 + 润色后的新增部分，拼接为完整文本，仅输出纯文本。",
            confirmed, to_polish
        ),
    }
}
```

- [x] **Step 5: `polish` 签名加 preserved**

`crates/llm/src/client.rs` 的 `polish`（55 行）签名 + 空检查 + user_prompt 调用改：

```rust
/// 对 ASR 识别文本进行润色。
/// - preserved=Some：增量润色，保留 preserved 原样、仅润色 to_polish（编辑后用）。
/// - preserved=None：全量润色 to_polish。
/// 返回润色后的完整文本。
pub fn polish(preserved: Option<&str>, to_polish: &str, config: &CompatibleLlmConfig) -> Result<String> {
    if to_polish.trim().is_empty() {
        return Ok(to_polish.to_string());
    }
    // ...max_tokens 仍按 to_polish 长度（新增部分）估；若 preserved 存在，输出更长，×1.2 余量已覆盖
    let max_tokens = ((to_polish.chars().count() as f64) * 1.2).ceil() as u64;
```

`messages` 里 user content 改：

```rust
            Message {
                role: "user".to_string(),
                content: prompt::user_prompt(preserved, to_polish),
            },
```

> 其余（thinking/enable_thinking 分派、请求发送、响应解析）不变。

- [x] **Step 6: coordinator 旧调用点改 `polish(None, ..)`**

`coordinator.rs:672`（最终润色）：

```rust
                let result = match octopus_llm::polish(None, &text_to_polish, &llm_config) {
```

`coordinator.rs:1044`（spawn_polish_thread）：

```rust
        let result = match octopus_llm::polish(None, &text, &llm_config) {
```

> 仅签名适配，行为不变（preserved=None）。T5 改为真值。

- [x] **Step 7: test_polish.rs example 适配**

`crates/llm/examples/test_polish.rs` 的 `octopus_llm::polish(...)` 调用加首参 `None`（具体行由实现者 grep 定位，仅改调用签名）。

- [x] **Step 8: 运行测试 + 编译**

Run: `cargo test -p octopus-llm` && `cargo check --workspace --all-targets`
Expected: llm 测试 PASS；workspace 编译通过。

- [x] **Step 9: Commit**

```bash
git add crates/llm/src/prompt.rs crates/llm/src/client.rs crates/desktop/src/coordinator.rs crates/llm/examples/test_polish.rs
git commit -m "feat(llm): polish 加 preserved 边界提示词（增量润色保留已确认部分）"
```

---

## Task 3: DB `edited_text` 列

**Files:**
- Modify: `crates/infra/src/db.sql`
- Modify: `crates/infra/src/db.rs`（`update_polished` 旁加 `update_edited_text`；`TranscriptionRecord` + `list_transcriptions_at`）

开发阶段删库重建（`~/.octopus/octopus.db`），与 db.sql 头注释约定一致，不写 ALTER 迁移。`finalize_transcription` **不改**——`edited_text` 由 commit_edit / 折回时单独 UPDATE。

- [x] **Step 1: 写失败测试**

`crates/infra/src/db.rs` 的 `#[cfg(test)] mod tests` 末尾加（复用内存 DB 辅助 `open_init`，约 538 行 `Connection::open_in_memory() + INIT_SQL`）：

```rust
#[test]
fn update_edited_text_persists_and_lists() {
    let conn = open_init();
    conn.execute(
        "INSERT INTO transcriptions (id, created_at, engine, raw_text, polish_status)
         VALUES (1, '2026-06-18', 'test', 'raw原文', 'off')",
        [],
    ).unwrap();

    let n = conn.execute(
        "UPDATE transcriptions SET edited_text=?1 WHERE id=?2",
        rusqlite::params!["手改文本", 1],
    ).unwrap();
    assert_eq!(n, 1);

    let edited: Option<String> = conn.query_row(
        "SELECT edited_text FROM transcriptions WHERE id=1", [], |r| r.get(0),
    ).unwrap();
    assert_eq!(edited.as_deref(), Some("手改文本"));
}
```

> 若 `open_init` 名称/签名不同，先 grep `fn open_init` 或 `open_in_memory` 确认实际辅助名再复用。

- [x] **Step 2: 运行测试确认失败**

Run: `cargo test -p octopus-infra update_edited_text_persists_and_lists`
Expected: 失败（`edited_text` 列不存在）。

- [x] **Step 3: DDL 加列**

`crates/infra/src/db.sql` 的 `transcriptions` 表（`polished_text TEXT,` 下一行）加：

```sql
    edited_text   TEXT,                     -- 用户编辑后的最终文本（未编辑为 NULL）
```

- [x] **Step 4: 加 `update_edited_text`**

`crates/infra/src/db.rs`，`update_polished` 函数后加（参照其 `with_db` 模式；`params` 已在 use 域）：

```rust
/// 用户提交编辑 / 中间润色折回后更新 edited_text。
pub fn update_edited_text(id: i64, edited_text: &str) -> Result<()> {
    with_db(|conn| {
        conn.execute(
            "UPDATE transcriptions SET edited_text=?1 WHERE id=?2",
            params![edited_text, id],
        )?;
        Ok(())
    })
}
```

- [x] **Step 5: `TranscriptionRecord` 加字段**

`TranscriptionRecord` struct（`polished_text` 字段下一行）加：

```rust
    pub edited_text: Option<String>,
```

- [x] **Step 6: `list_transcriptions_at` SELECT + 映射加列**

SELECT（`polished_text` 后加 `edited_text`）；`query_map` 映射按新列序（edited_text 在 polished_text 后，其余顺延）。实现者读现有 SELECT/映射块（约 453-471 行），在 `polished_text` 后插入 `edited_text` 列与 `edited_text: row.get(n)?`，后续列号 +1。

- [x] **Step 7: 运行测试确认通过**

Run: `cargo test -p octopus-infra`
Expected: PASS（含新测试 + 现有 db 测试）。

- [x] **Step 8: 编译验证**

Run: `cargo check -p octopus-infra --all-targets`
Expected: 通过。

- [x] **Step 9: Commit**

```bash
git add crates/infra/src/db.sql crates/infra/src/db.rs
git commit -m "feat(infra): transcriptions 加 edited_text 列 + update_edited_text + 历史查询"
```

---

## Task 4: coordinator 编辑命令 + tick 硬暂停闸门 + commit→DB

**Files:**
- Modify: `crates/desktop/src/coordinator.rs`
- Modify: `crates/desktop/src/main.rs`（invoke_handler）

编辑态：`editing: bool` + `edit_buffer: Option<String>` 为循环局部变量；tick 在 editing 时排空丢弃音频；commit 时写 transcript（T1 `commit_edit`）+ `UPDATE edited_text`（T3）。Toggle-期间-编辑 用 `edit_buffer`（前端 input 防抖推送）恢复。**本任务不碰润色路径**（T5）。

- [x] **Step 1: `Command` enum 加 3 变体**

`coordinator.rs:18` 的 `enum Command`（`PolishNow` 后）加：

```rust
    /// 进入编辑态（前端 edit_shortcut/编辑按钮触发；ASR 硬暂停）
    EnterEditMode,
    /// 更新编辑缓冲（前端 input 防抖推送；供 Toggle-期间-编辑 恢复）
    UpdateEditBuffer { text: String },
    /// 提交编辑（快捷键/完成按钮触发）
    CommitEdit { text: String },
```

- [x] **Step 2: `DbCommand` enum 加 `UpdateEdited`**

`enum DbCommand`（`Finalize` 后）加：

```rust
    UpdateEdited {
        id: i64,
        edited_text: String,
    },
```

- [x] **Step 3: `process_db_command` 加 arm**

`process_db_command` 的 `match cmd`（`Finalize` arm 后）加：

```rust
        DbCommand::UpdateEdited { id, edited_text } => {
            if let Err(e) = octopus_asr::db::update_edited_text(id, &edited_text) {
                warn!("Background DB update_edited_text failed: {}", e);
            }
        }
```

- [x] **Step 4: 主循环加 `editing` + `edit_buffer` 局部变量**

`let mut stage = Stage::Idle;` 旁加：

```rust
            let mut stage = Stage::Idle;
            // 编辑态：置位时 tick 跳过喂引擎、只排空丢弃音频（硬暂停）。
            let mut editing = false;
            // 编辑缓冲：前端 input 防抖推送的最新文本；Toggle-期间-编辑 时用作提交文本。
            let mut edit_buffer: Option<String> = None;
```

- [x] **Step 5: tick 分发加 editing 闸门**

`Command::StreamingTick` arm 改为（`set_mode` 后、`handle_streaming_tick` 前加闸门）：

```rust
                    Command::StreamingTick => {
                        {
                            let rc = runtime_config.read().unwrap();
                            config.polish_mode = rc.polish_mode;
                        }
                        if let Stage::Streaming { transcript, .. } = &mut stage {
                            transcript.set_mode(config.polish_mode);
                        }
                        if editing {
                            let _ = audio.drain_samples(); // 编辑期丢弃音频，不喂引擎
                        } else {
                            handle_streaming_tick(&mut stage, &audio, &config, &app_handle, &tx);
                        }
                    }
```

`Command::VadSegmentedTick` arm 同样在 `set_mode` 后、`handle_vad_segmented_tick` 前加：

```rust
                        if editing {
                            let _ = audio.drain_samples();
                        } else {
                            handle_vad_segmented_tick(
                                &mut stage, &audio, &engine, &config, &app_handle, &tx,
                            );
                        }
```

> 保留原 arm 其余结构；仅把 `handle_xxx_tick(...)` 调用包进 else。

- [x] **Step 6: 加 3 个编辑 Command 分发 arm + TranscriptionDone 守卫**

`Command::PolishNow` arm 后加：

```rust
                    Command::EnterEditMode => {
                        handle_enter_edit_mode(&mut stage, &mut editing, &mut edit_buffer);
                    }
                    Command::UpdateEditBuffer { text } => {
                        if editing {
                            edit_buffer = Some(text);
                        }
                    }
                    Command::CommitEdit { text } => {
                        if editing {
                            commit_edit_apply(&mut stage, &text, &app_handle);
                            editing = false;
                        }
                    }
```

`Command::TranscriptionDone` arm 改（编辑期忽略在途结果，spec §7）：

```rust
                    Command::TranscriptionDone { text, seq, session_id } => {
                        if editing {
                            debug!("TranscriptionDone ignored during edit");
                        } else {
                            handle_transcription_done(
                                &mut stage, text, seq, session_id, &config, &app_handle, &tx,
                            );
                        }
                    }
```

- [x] **Step 7: `Command::Toggle` 加编辑态先提交**

`Command::Toggle =>` 在 `handle_toggle(...)` 调用前插入（保持原 `handle_toggle` 所有参数不变）：

```rust
                    Command::Toggle => {
                        // 编辑态下停止：先用 edit_buffer 提交编辑，再走停止流程（spec §7）
                        if editing {
                            if let Some(text) = edit_buffer.take() {
                                commit_edit_apply(&mut stage, &text, &app_handle);
                            }
                            editing = false;
                            let _ = app_handle.emit("edit-force-exit", ());
                        }
                        handle_toggle(
                            /* …原参数不变… */
```

> **前置导入**：coordinator.rs 顶部 use 区（`use std::time::Instant;` 下一行）加 `use tauri::Emitter;`（`app_handle.emit` 需 Emitter trait）。

- [x] **Step 8: 实现 `handle_enter_edit_mode` + `commit_edit_apply`**

`handle_polish_now` / `start_final_polish_or_paste` 附近加：

```rust
/// 进入编辑态：仅活跃会话（Streaming/VadSegmented）有效；初始化 edit_buffer = 当前 display。
fn handle_enter_edit_mode(stage: &mut Stage, editing: &mut bool, edit_buffer: &mut Option<String>) {
    let transcript = match stage {
        Stage::Streaming { transcript, .. } | Stage::VadSegmented { transcript, .. } => transcript,
        _ => {
            debug!("enter_edit_mode ignored in non-active stage");
            return;
        }
    };
    *editing = true;
    *edit_buffer = Some(transcript.display_text());
    info!("Entered edit mode (transcript id={})", transcript.id);
}

/// 提交编辑：写回 transcript（commit_edit）+ UPDATE edited_text（行已存在）+ 刷新展示。
fn commit_edit_apply(stage: &mut Stage, text: &str, app_handle: &tauri::AppHandle) {
    let transcript = match stage {
        Stage::Streaming { transcript, .. } | Stage::VadSegmented { transcript, .. } => transcript,
        _ => {
            debug!("commit_edit ignored in non-active stage");
            return;
        }
    };
    transcript.commit_edit(text);
    if transcript.db_inserted() {
        let id = transcript.id;
        if let Err(e) = get_db_sender().send(DbCommand::UpdateEdited {
            id,
            edited_text: text.to_string(),
        }) {
            warn!("Queue DB UpdateEdited failed: {}", e);
        }
    }
    crate::result_window::update_result(app_handle, &transcript.display_text());
    info!("Edit committed ({} chars)", text.chars().count());
}
```

> or-pattern `Stage::Streaming { transcript, .. } | Stage::VadSegmented { transcript, .. }` 合法：两变体都有 `transcript: Transcript` 字段，绑定同类型。

- [x] **Step 9: 加 Coordinator 公开方法 + Tauri 命令**

`impl Coordinator` 内（`polish_now` 方法后）加 3 方法；其 Tauri 命令（`pub fn polish_now(...)` 后）加 3 命令：

```rust
    /// 进入编辑态
    pub fn enter_edit_mode(&self) {
        if let Ok(tx) = self.tx.lock() {
            if tx.send(Command::EnterEditMode).is_err() {
                error!("Coordinator channel closed");
            }
        }
    }

    /// 更新编辑缓冲（前端 input 防抖推送）
    pub fn update_edit_buffer(&self, text: String) {
        if let Ok(tx) = self.tx.lock() {
            if tx.send(Command::UpdateEditBuffer { text }).is_err() {
                error!("Coordinator channel closed");
            }
        }
    }

    /// 提交编辑
    pub fn commit_edit(&self, text: String) {
        if let Ok(tx) = self.tx.lock() {
            if tx.send(Command::CommitEdit { text }).is_err() {
                error!("Coordinator channel closed");
            }
        }
    }
```

```rust
/// 前端命令：进入编辑态（edit_shortcut/编辑按钮触发）。
#[tauri::command]
pub fn enter_edit_mode(coordinator: tauri::State<'_, Coordinator>) {
    coordinator.enter_edit_mode();
}

/// 前端命令：更新编辑缓冲（input 防抖推送）。
#[tauri::command]
pub fn update_edit_buffer(coordinator: tauri::State<'_, Coordinator>, text: String) {
    coordinator.update_edit_buffer(text);
}

/// 前端命令：提交编辑（快捷键/完成按钮触发）。
#[tauri::command]
pub fn commit_edit(coordinator: tauri::State<'_, Coordinator>, text: String) {
    coordinator.commit_edit(text);
}
```

- [x] **Step 10: main.rs 注册 3 命令**

`invoke_handler` 的 `generate_handler!`（`coordinator::polish_now,` 后）加：

```rust
            coordinator::enter_edit_mode,
            coordinator::update_edit_buffer,
            coordinator::commit_edit,
```

- [x] **Step 11: 编译 + 测试**

Run: `cargo check -p octopus-desktop --all-targets && cargo test -p octopus-desktop`
Expected: 编译通过；现有测试 PASS（transcript + coordinator 测试）。

- [x] **Step 12: Commit**

```bash
git add crates/desktop/src/coordinator.rs crates/desktop/src/main.rs
git commit -m "feat(desktop): 编辑态命令 + tick 硬暂停闸门 + commit→DB edited_text"
```

---

## Task 5: 编辑×润色接线（take_polish_input + preserved + 折回）

**Files:**
- Modify: `crates/desktop/src/transcript.rs`（`take_polish_input` 替代 `snapshot_for_polish`；`on_polish_done` 折回）
- Modify: `crates/desktop/src/coordinator.rs`（`spawn_polish_thread` + 两条润色路径接 `(preserved, to_polish)`；`handle_polish_done` 折回 DB 分支）

T1/T2/T3/T4 已就绪。本任务把「润色输入 = (edited, 新增)」与「结果折回 edited」贯通（spec §12）。`on_polish_done` 在 `has_edit()` 时折回，避免 edited 遮蔽 polished 丢字。

- [x] **Step 1: 写失败测试 —— take_polish_input + 折回**

`transcript.rs` tests 末尾加：

```rust
#[test]
fn take_polish_input_no_edit_returns_full() {
    let mut t = Transcript::new(40, PolishMode::Intermediate);
    t.set_full("第一段第二段");
    let (preserved, to_polish) = t.take_polish_input();
    assert_eq!(preserved, None);
    assert_eq!(to_polish, "第一段第二段");
}

#[test]
fn take_polish_input_after_edit_returns_preserved_and_increase() {
    let mut t = Transcript::new(41, PolishMode::Intermediate);
    t.set_full("原文");
    t.commit_edit("原文（手改）"); // edited="原文（手改）", raw_len=2
    t.set_full("原文新增"); // increase="新增"
    let (preserved, to_polish) = t.take_polish_input();
    assert_eq!(preserved.as_deref(), Some("原文（手改）"));
    assert_eq!(to_polish, "新增");
}

#[test]
fn on_polish_done_folds_into_edited_when_has_edit() {
    let mut t = Transcript::new(42, PolishMode::Intermediate);
    t.set_full("原文");
    t.commit_edit("原文（手改）");
    t.set_full("原文新增");
    let _ = t.take_polish_input(); // 推进 raw_len
    // LLM 返回 edited + 润色后新增
    t.on_polish_done("原文（手改）新增（润色）".into());
    assert_eq!(t.edited_text(), Some("原文（手改）新增（润色）"));
    assert_eq!(t.display_text(), "原文（手改）新增（润色）"); // 折回 edited，无丢字
}

#[test]
fn on_polish_done_no_edit_writes_polished() {
    let mut t = Transcript::new(43, PolishMode::Intermediate);
    t.set_full("原文");
    let _ = t.take_polish_input();
    t.on_polish_done("润色".into());
    assert_eq!(t.polished(), "润色"); // 无编辑 → polished（现状）
    assert_eq!(t.display_text(), "润色");
}
```

- [x] **Step 2: 运行测试确认失败**

Run: `cargo test -p octopus-desktop transcript::tests`
Expected: 编译失败（`take_polish_input` 未定义）。

- [x] **Step 3: Transcript 加 `take_polish_input`，删 `snapshot_for_polish`**

替换 `snapshot_for_polish`（82-85 行）为：

```rust
/// 取润色输入并推进 raw_len 边界（increase 清空）。
/// - has_edit：(Some(edited), increase) —— 已确认=edited（LLM 须原样保留），待润色=increase（新增）
/// - 否则：(None, full) —— 全量原始 ASR（保持现状）
pub fn take_polish_input(&mut self) -> (Option<String>, String) {
    let preserved = if self.has_edit() {
        Some(self.edited.clone())
    } else {
        None
    };
    let to_polish = if self.has_edit() {
        self.full.chars().skip(self.raw_len).collect()
    } else {
        self.full.clone()
    };
    self.raw_len = self.full.chars().count();
    (preserved, to_polish)
}
```

> 同步更新其上方 doc 注释里「raw_len 已在 snapshot_for_polish 推进」之类措辞为 take_polish_input。

- [x] **Step 4: `on_polish_done` 折回**

替换 `on_polish_done`（88-92 行）为：

```rust
/// 润色完成：
/// - has_edit：结果折回 edited（= edited + 润色后新增），避免 edited 遮蔽 polished 丢字（spec §12）。
/// - 否则：写 polished（raw_len 已在 take_polish_input 推进）。
pub fn on_polish_done(&mut self, result: String) {
    if self.has_edit() {
        self.edited = result;
    } else {
        self.polished = result;
    }
    self.polish_pending = false;
    self.last_polish_time = Instant::now();
}
```

- [x] **Step 5: 迁移 transcript 测试中的 snapshot_for_polish 调用**

grep `snapshot_for_polish` in transcript.rs tests，逐处改 take_polish_input：
- `let snap = t.snapshot_for_polish();`（断言 snap）→ `let (preserved, snap) = t.take_polish_input();`（断言 `preserved, None` + `snap`）
- `t.snapshot_for_polish();`（不断言）→ `let _ = t.take_polish_input();`

确保无 `snapshot_for_polish` 残留（含 doc）。

- [x] **Step 6: `spawn_polish_thread` 签名加 preserved**

`spawn_polish_thread`（1027 行）签名 + body 改：

```rust
fn spawn_polish_thread(
    preserved: Option<String>,
    to_polish: String,
    config: &AppConfig,
    tx: &Sender<Command>,
    ignore_mode: bool,
) {
    let llm_config = if ignore_mode {
        crate::config::llm_config_ignore_mode(&config)
    } else {
        crate::config::llm_config(&config)
    };
    let llm_config = match llm_config {
        Some(c) => c,
        None => return,
    };
    let tx = tx.clone();
    std::thread::spawn(move || {
        let result = match octopus_llm::polish(preserved.as_deref(), &to_polish, &llm_config) {
            Ok(polished) => Ok(polished),
            Err(e) => {
                log::warn!("Polish thread error: {}", e);
                Err(e.to_string())
            }
        };
        let _ = tx.send(Command::PolishDone { result });
    });
}
```

- [x] **Step 7: 中间润色 + PolishNow 接 take_polish_input**

`check_and_trigger_polish`（1086-1089 行）：

```rust
    // 取润色输入（编辑态: preserved+increase；否则 full）+ 标记 pending + 送 LLM
    let (preserved, to_polish) = transcript.take_polish_input();
    transcript.mark_polish_pending();
    spawn_polish_thread(preserved, to_polish, config, tx, false);
```

`handle_polish_now`（1476-1479 行）：

```rust
    let (preserved, to_polish) = transcript.take_polish_input();
    transcript.mark_polish_pending();
    info!("PolishNow triggered, polishing {} chars", to_polish.chars().count());
    spawn_polish_thread(preserved, to_polish, config, tx, true);
```

- [x] **Step 8: 最终润色入口接 take_polish_input**

`start_final_polish_or_paste` 的 polish 分支（670-683 行）。当前 `let text_to_polish = text.to_string();` → 改为从 owned transcript 取边界（transcript 此时还在，未移入 Polishing）：

```rust
            let id = transcript.id;
            let raw_text = transcript.db_text();
            let (preserved, to_polish) = transcript.take_polish_input();

            *stage = Stage::Polishing {
                id,
                raw_text: raw_text.clone(),
            };

            let tx = tx.clone();
            std::thread::spawn(move || {
                let result = match octopus_llm::polish(preserved.as_deref(), &to_polish, &llm_config) {
                    Ok(polished) => {
                        if polished.is_empty() {
                            Err("Final polish returned empty".to_string())
                        } else {
                            Ok(polished)
                        }
                    }
                    Err(e) => Err(e.to_string()),
                };
                let _ = tx.send(Command::FinalPolishDone { result });
            });
```

> `take_polish_input` 推进 raw_len，但 `db_text()` 返回 full（不受 raw_len 影响），顺序 OK。无润色分支（`None => do_paste(text, ..)`）仍用调用方传入的 `text`（T6 改为 edited_display）。

- [x] **Step 9: `handle_polish_done` 折回 DB 分支**

`handle_polish_done`（1413-1425+ 行）的 `Ok(polished) => { ... }` 块：`on_polish_done` 后按 `has_edit()` 决定 DB 命令。读现有 `DbCommand::UpdatePolished { ... }` 块，改为：

```rust
        Ok(polished) => {
            if polished.is_empty() {
                warn!("Polish returned empty, keeping previous");
                transcript.on_polish_failed();
            } else {
                transcript.on_polish_done(polished.clone());
                // 折回→UpdateEdited（保持 edited_text 与 display 一致）；否则 UpdatePolished（现状）
                let cmd = if transcript.has_edit() {
                    DbCommand::UpdateEdited {
                        id: transcript.id,
                        edited_text: polished,
                    }
                } else {
                    DbCommand::UpdatePolished {
                        /* …原字段不变（id/text/status/model）… */
                    }
                };
                // …原 send cmd 逻辑…
            }
        }
```

> 实现者读现有 UpdatePolished 块的字段，搬进 else 分支；`polished` 变量在 if 分支 move 进 UpdateEdited，故先 `on_polish_done(polished.clone())`。

- [x] **Step 10: 编译 + 测试**

Run: `cargo check -p octopus-desktop --all-targets && cargo test -p octopus-desktop`
Expected: 编译通过；transcript 新测试 + 现有测试 PASS。

- [x] **Step 11: Commit**

```bash
git add crates/desktop/src/transcript.rs crates/desktop/src/coordinator.rs
git commit -m "feat(desktop): 编辑×润色接线（take_polish_input 边界 + on_polish_done 折回 edited）"
```

---

## Task 6: 停止路径无润色/兜底用 edited_display

**Files:**
- Modify: `crates/desktop/src/coordinator.rs`

两部分：**Part A** 三处「无润色粘贴 / finish 兜底」文本从 `db_text()` 改为 edited 优先（T1 `edited_display`）：edited 非空用 display（不补句末标点）；否则走原 raw 逻辑（含补「。」）。DB raw 仍用 `db_text()`。**Part B** 最终润色失败兜底保留编辑——`Stage::Polishing` 加 `fallback_text`（= 停止时 final_text），LLM 失败时 `do_paste(&fallback_text)` 而非 raw ASR。最终润色输入已由 T5 的 `take_polish_input` 处理。

### Part A：三处无润色/兜底站点改 edited_display

- [x] **Step 1: VadSegmented 停止路径（handle_toggle VadSegmented 分支）**

替换 `let final_text = if ... else ...` 块为 edited 优先：

```rust
                let final_text = if let Some(edited) = transcript.edited_display() {
                    edited
                } else if transcript.full().is_empty() {
                    String::new()
                } else if transcript
                    .full()
                    .ends_with(|c: char| ",.，。！？!?\n".contains(c))
                {
                    transcript.db_text()
                } else {
                    format!("{}。", transcript.db_text())
                };
```

- [x] **Step 2: handle_transcription_done 停止路径**

同样替换（结构与 Step 1 相同的 `final_text` 块）。

- [x] **Step 3: Streaming 停止路径（combined + finish 失败兜底）**

`let combined = transcript.db_text();` 改：

```rust
            let combined = transcript
                .edited_display()
                .unwrap_or_else(|| transcript.db_text());
```

`finish()` 失败兜底 `transcript.db_text()` 改：

```rust
                    Err(e) => {
                        error!("Streaming finish failed: {}", e);
                        transcript
                            .edited_display()
                            .unwrap_or_else(|| transcript.db_text())
                    }
```

### Part B：最终润色失败兜底保留编辑

> Part A 后调用方传入 `start_final_polish_or_paste` 的 `text` = edited_display（含编辑）或 raw(+「。」）。复用它作最终润色失败的兜底粘贴文本，避免失败时丢编辑。

- [x] **Step 4: Stage::Polishing 加 fallback_text 字段**

`enum Stage` 的 `Polishing { id, raw_text }` 加字段：

```rust
    Polishing {
        id: i64,
        raw_text: String,
        /// 最终润色失败时的兜底粘贴文本（= 停止时 display，含编辑；成功时不用）
        fallback_text: String,
    },
```

- [x] **Step 5: 构造 Polishing 时设 fallback_text**

`start_final_polish_or_paste` 的 `*stage = Stage::Polishing { id, raw_text: raw_text.clone() }` 改：

```rust
            *stage = Stage::Polishing {
                id,
                raw_text: raw_text.clone(),
                fallback_text: text.to_string(),
            };
```

- [x] **Step 6: handle_final_polish_done 解构 + Err 分支用 fallback_text**

解构 `Stage::Polishing { id, raw_text }` → 加 `fallback_text`：

```rust
    let (id, raw_text, fallback_text) = match stage {
        Stage::Polishing { id, raw_text, fallback_text } => {
            (*id, raw_text.clone(), fallback_text.clone())
        }
        _ => { ... }
    };
```

Err 分支（原 `do_paste(stage, &raw_text, id, &raw_text, "failed", ...)`）改为第一参用 fallback_text、第四参（DB raw）仍 raw_text：

```rust
        Err(e) => {
            warn!("Final polish failed: {}, using fallback (display)", e);
            do_paste(stage, &fallback_text, id, &raw_text, "failed", config, app_handle, tx);
        }
```

> Ok 分支 `do_paste(&polished, id, &raw_text, "done", ...)` 不变。其他 `Stage::Polishing { .. }` 解构点（用 `{ .. }` 忽略）无需改。

- [x] **Step 7: 编译 + 测试**

Run: `cargo check -p octopus-desktop --all-targets && cargo test -p octopus-desktop`
Expected: 通过；`edited_display` dead_code 警告消失（已被多处消费）。

- [x] **Step 8: Commit**

```bash
git add crates/desktop/src/coordinator.rs
git commit -m "feat(desktop): 停止路径用 edited_display（无润色/兜底/最终润色失败均保留编辑）"
```

> **T8 e2e 重点**：流式中途编辑→停止→最终润色失败 的路径（Streaming `set_full` 后 edited_display 切片 `full[raw_len..]` 在越界时 Rust 返回空、不 panic，但需 e2e 确认拼接符合预期）。

---

## Task 7: 前端编辑交互

**Files:**
- Create: `crates/desktop/dist/result/icons/edit.svg`
- Modify: `crates/desktop/dist/result/index.html`

`#result-text` 默认不可编辑；按 `edit_shortcut`（默认 Cmd+E）或点编辑按钮 → `contenteditable=true` + 聚焦 + `enter_edit_mode`；`Cmd/Ctrl+Enter` / 完成按钮 → `commit_edit`；input 防抖推 `update_edit_buffer`；编辑态加边框、禁 mouseleave 收起、冻结 update-result。

- [x] **Step 1: 新建 edit.svg**

`crates/desktop/dist/result/icons/edit.svg`：

```svg
<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M12 20h9"/><path d="M16.5 3.5a2.121 2.121 0 0 1 3 3L7 19l-4 1 1-4Z"/></svg>
```

- [x] **Step 2: 加编辑按钮 + 完成按钮 HTML**

工具栏（`#tool-polish-now` 按钮 `</button>` 后）加编辑按钮：

```html
        <button class="tool" id="tool-edit" title="编辑" aria-label="编辑">
          <span class="icon"></span>
        </button>
```

`#text-wrapper`（`<div id="text-wrapper">` 内、`#result-text` 前）加完成按钮：

```html
    <div id="text-wrapper">
      <button id="edit-done" hidden>完成编辑</button>
      <div id="result-text"></div>
    </div>
```

- [x] **Step 3: 加 CSS**

`<style>`（`#tool-polish-now .icon` 行后）加编辑按钮图标：

```css
    #tool-edit .icon { -webkit-mask-image: url(icons/edit.svg?v=1); mask-image: url(icons/edit.svg?v=1); }
```

`#result-text` 规则后加完成按钮 + 编辑态：

```css
    /* 完成编辑按钮：编辑态显示，浮于文本区右上 */
    #edit-done {
      position: absolute;
      top: 4px;
      right: 8px;
      z-index: 15;
      font-size: 12px;
      padding: 2px 10px;
      border: 0.5px solid rgba(0,122,255,0.4);
      border-radius: 6px;
      background: rgba(0,122,255,0.08);
      color: #007aff;
      cursor: pointer;
    }
    #edit-done:hover { background: rgba(0,122,255,0.16); }

    /* 编辑态：淡蓝边框包住整个展示区（text-wrapper），完成按钮落在框内（e2e 后） */
    #container.editing #text-wrapper {
      border: 1px solid rgba(0, 122, 255, 0.5);
      border-radius: 6px;
      background: rgba(0, 122, 255, 0.04);
    }
    #container.editing #result-text {
      padding: 1px 90px 7px 13px;   /* right 90px 给完成按钮让位 */
    }
    #container.editing #result-text:focus { background: transparent; }
```

- [x] **Step 4: 加编辑态 JS**

`<script>`（润色逻辑后）加：

```js
    // ── 编辑态 ──
    let editing = false;
    const btnEdit = document.getElementById('tool-edit');
    const btnEditDone = document.getElementById('edit-done');

    function enterEdit() {
      if (editing) return;
      editing = true;
      resultText.setAttribute('contenteditable', 'true');
      container.classList.add('editing');
      btnEditDone.hidden = false;
      btnEdit.classList.add('active');
      currentWindow.setFocus();
      resultText.focus();
      invoke('enter_edit_mode');
      updateEditBuffer();
    }

    function commitEdit() {
      if (!editing) return;
      const text = resultText.innerText;
      editing = false;
      resultText.setAttribute('contenteditable', 'false');
      container.classList.remove('editing');
      btnEditDone.hidden = true;
      btnEdit.classList.remove('active');
      invoke('commit_edit', { text });
    }

    function updateEditBuffer() {
      if (!editing) return;
      invoke('update_edit_buffer', { text: resultText.innerText });
    }

    // 进入编辑：曾用 dblclick，因 WKWebView 在 user-select:text 区域 dblclick 难触发而弃用；
    // 改为 edit_shortcut（默认 Cmd+E）keydown + ✏️ 按钮。详见 spec §3.1。
    btnEdit.addEventListener('click', (e) => { e.preventDefault(); enterEdit(); });

    document.addEventListener('keydown', (e) => {
      if (editing && (e.metaKey || e.ctrlKey) && e.key === 'Enter') {
        e.preventDefault();
        commitEdit();
      }
    });

    btnEditDone.addEventListener('mousedown', (e) => e.preventDefault());
    btnEditDone.addEventListener('click', (e) => { e.preventDefault(); commitEdit(); });

    // （e2e 后去除 blur 触发：完成按钮 + Cmd/Ctrl+Enter 已足够，点别处/toolbar 不再自动提交）

    let editBufTimer = null;
    resultText.addEventListener('input', () => {
      clearTimeout(editBufTimer);
      editBufTimer = setTimeout(updateEditBuffer, 150);
    });

    listen('edit-force-exit', () => {
      if (editing) {
        editing = false;
        resultText.setAttribute('contenteditable', 'false');
        container.classList.remove('editing');
        btnEditDone.hidden = true;
        btnEdit.classList.remove('active');
      }
    });
```

- [x] **Step 5: 编辑态冻结 update-result + 禁 mouseleave 收起**

`listen('update-result', ...)` 加 editing 守卫：

```js
    listen('update-result', (event) => {
      if (editing) return;               // 编辑态冻结，不覆盖用户输入
      resultText.textContent = event.payload;
      resultText.scrollTop = resultText.scrollHeight;
    });
```

`hideToolbar` 函数开头加守卫：

```js
    function hideToolbar() {
      if (!toolbarVisible || editing) return;   // 编辑态不收起
      /* …原逻辑… */
```

- [x] **Step 6: 手动构建验证** (待用户手动验证：T8 环境无 GUI；前端 dist 已构建并通过 snapshot/编译检查，行为验证需在本地 GUI 跑)

Run: `cargo run -p octopus-desktop`
手动验证（debug 构建自动开 devtools）：
1. 录音 → 说一句 → 结果窗出文本。
2. 按 Cmd+E → 蓝边框 + 「完成编辑」→ 继续说话，窗口不刷新（硬暂停）。
3. 改字 → 点「完成编辑」→ 边框消失 → 继续说 → 新文本追加在编辑结果后。
4. 按 `edit_shortcut` → 改 → 再按 `edit_shortcut`（toggle）→ 同样生效。
5. 编辑态点工具栏 ASR 按钮 → 不退出编辑，直接弹浮层（e2e：完成按钮足够显眼，去除 blur 退出）。

Expected: 行为符合预期，devtools 无 JS 报错。

- [x] **Step 7: Commit**

```bash
git add crates/desktop/dist/result/index.html crates/desktop/dist/result/icons/edit.svg
git commit -m "feat(desktop): 结果窗可编辑（双击/按钮进入，快捷键/按钮退出，硬暂停）"
```

---

## Task 8: 文档同步 + 端到端验证

**Files:**
- Modify: `docs/configuration.md`
- Modify: `docs/architecture.md`
- Modify: `docs/superpowers/specs/2026-06-18-editable-result-window-design.md`（状态行）
- Modify: `docs/superpowers/plans/2026-06-18-editable-result-window.md`（checkbox 勾选）

- [x] **Step 1: configuration.md 加编辑能力说明**

结果窗/工具栏相关段加：

```markdown
### 结果展示区编辑

录音过程中可随时修正识别/润色文本：
- **进入编辑**：按 `edit_shortcut`（默认 `Cmd+E`，窗口内），或点工具栏 ✏️ 编辑按钮。
- **编辑期间 ASR 硬暂停**（音频丢弃），改完恢复。
- **退出编辑**（择一）：`Cmd/Ctrl+Enter`、点「完成编辑」按钮。（e2e：完成按钮足够显眼，去除失焦/点 toolbar 退出。）
- 编辑后的文本作为后续展示与润色基准；新识别文本追加其上；停止粘贴时保留编辑。
- 编辑后再触发润色时，仅润色新增部分、保留已编辑（润色结果折回）。
- 未编辑时行为与旧版完全一致。
```

- [x] **Step 2: architecture.md 同步**

Transcript 相关段加：

```markdown
- `Transcript` 三文本分层：`edited ≻ polished ≻ raw`。`display_text()` = committed + increase；
  `full`（原始 ASR）独立保留为 DB `raw_text`。
- 编辑态：coordinator 主循环 `editing` 标志置位时，Streaming/VadSegmented tick 跳过喂引擎、
  只排空丢弃音频（硬暂停）。`commit_edit` 写回 transcript 并 `UPDATE edited_text`。
- 编辑×润色（spec §12）：`take_polish_input()` 返回 `(preserved=edited, to_polish=increase)`，
  LLM 仅润色新增；`on_polish_done` 在 `has_edit()` 时折回 `edited`（避免遮蔽丢字）。
- `transcriptions` 表加 `edited_text` 列（commit + 中间润色折回时写）。
- 停止路径：润色输入 = `take_polish_input`；无润色/兜底粘贴 = `edited_display()`；DB raw 仍 = `db_text()`。
```

- [x] **Step 3: spec 状态行置已实现**

`docs/superpowers/specs/2026-06-18-editable-result-window-design.md` 顶部 `> Status:` 行改为：

```
> Status: ✅ 已实现（2026-06-18，plan 2026-06-18-editable-result-window.md v2）。会话中编辑（快捷键 edit_shortcut/按钮进入，快捷键/按钮退出，硬暂停）+ 三文本分层 + 编辑×润色折回 + DB edited_text 均已落地。
```

- [x] **Step 4: plan checkbox 勾选**

本文件所有 `- [ ]` → `- [x]`（实现者确认每步已做）。

- [x] **Step 5: 删库重建 + 全流程 e2e** (待用户手动验证：T8 环境无 GUI，e2e 检查清单已附在 T8 任务报告中)

```bash
cp ~/.octopus/octopus.db ~/.octopus/octopus.db.bak.$(date +%s)
rm ~/.octopus/octopus.db
cargo run -p octopus-desktop
```

手动 e2e（三种 PolishMode 各验一次）：
1. `polish_mode=2`：录音 → 说一段（出中间润色）→ 按 Cmd+E 改错 → 完成 → 继续说 → 停顿触发润色（仅润色新增，edited 保留）→ 停止 → 粘贴 = edited + 润色后新增；DB `edited_text` 非空、`raw_text` 为原始 ASR。
2. `polish_mode=0`：录音 → 按 Cmd+E 改 → 完成 → 停止 → 粘贴 = edited。
3. 编辑态按停止热键 → edit_buffer 提交编辑后停止 → 粘贴含编辑。
4. `sqlite3 ~/.octopus/octopus.db "SELECT raw_text, polished_text, edited_text FROM transcriptions ORDER BY id DESC LIMIT 3;"` 验证三列互不干扰。

- [x] **Step 6: workspace 全量编译 + 测试**

```bash
cargo check --workspace --all-targets
cargo test --workspace
```
Expected: 全绿。

- [x] **Step 7: Commit**

```bash
git add docs/configuration.md docs/architecture.md docs/superpowers/specs/2026-06-18-editable-result-window-design.md docs/superpowers/plans/2026-06-18-editable-result-window.md
git commit -m "docs: 同步结果窗可编辑（configuration/architecture/spec 状态/plan v2）"
```

---

## 验证总结

| 场景 | 预期 |
|---|---|
| 未编辑（任意 mode） | 行为与旧版完全一致（display 公式等价；polish(None, full)） |
| Cmd+E 编辑 → 完成 → 继续说 | 新文本追加在 edited 后；display = edited + increase |
| 编辑后停顿润色（mode=2） | take_polish_input=(edited, 新增)；LLM 仅润色新增；结果折回 edited，无丢字 |
| 停止粘贴（有润色） | 粘贴 = polish(edited, 新增) 的结果；DB raw 仍原始 ASR |
| 停止粘贴（无润色/兜底） | 粘贴 = edited_display（含 edited） |
| 编辑态点 toolbar | 不退出编辑，直接执行按钮动作（e2e 去除 blur 退出） |
| 编辑态按停止热键 | edit_buffer 提交编辑后停止 |
| DB | raw/polished/edited 三列独立、互不干扰 |

## 关键文件

- `crates/desktop/src/transcript.rs`（edited + commit_edit + display 优先级链 + edited_display + take_polish_input + on_polish_done 折回）
- `crates/llm/src/prompt.rs` + `client.rs`（user_prompt/preserved + polish 签名）
- `crates/infra/src/db.sql` + `db.rs`（edited_text 列 + update_edited_text + 历史查询）
- `crates/desktop/src/coordinator.rs`（Command/DbCommand 变体 + editing 闸门 + commit→DB + 润色接线 take_polish_input/preserved + 折回 DB 分支 + 停止路径 edited_display）
- `crates/desktop/src/main.rs`（invoke_handler 注册 3 命令）
- `crates/desktop/dist/result/index.html` + `icons/edit.svg`（编辑交互）


## 📄 `2026-06-19-connection-test.md`

# 设置页连接测试按钮 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

> ⚠️ 命令实现为 `async fn`——见 [2026-06-19-connection-test-async.md](./2026-06-19-connection-test-async.md)（LLM `spawn_blocking` / ASR 直接 `await connect_async`）。本 plan Task 1（`test_connection` 函数）+ Task 3（前端 UI）仍有效；Task 2 命令实现已迁至 async plan，仅保留注册步骤。

**Goal:** 在设置页「语音识别引擎」和「文本润色模型」两个 select 旁加连接测试按钮，远程模型可点（WS 握手 / chat max_tokens=1），本地模型灰掉；三态视觉反馈。

**Architecture:**
- 后端新增 2 个 Tauri 命令（`crates/desktop/src/settings_commands.rs`）：`test_llm_connection(spec)` + `test_asr_connection(bare_name)`，均为 `async fn`（LLM `spawn_blocking` 包 `reqwest::blocking`，ASR 直接 `await connect_async`）——命令实现详见 [2026-06-19-connection-test-async.md](./2026-06-19-connection-test-async.md)
- LLM 测试逻辑抽到 `crates/llm/src/client.rs::test_connection`（复用 `ChatRequest`，`max_tokens=1`）
- ASR 测试内联在命令实现里（`#[cfg(feature="dashscope")]` 包围，仅握手不发协议帧）
- 前端 `crates/desktop/dist/settings/index.html`：每个 select 包 flex 容器 + `.test-btn`，三态 CSS + JS 联动

**Tech Stack:** Rust + Tauri 2 + reqwest blocking + tokio-tungstenite + vanilla HTML/CSS/JS

**设计 spec:** `docs/superpowers/specs/2026-06-19-connection-test-design.md`

> **状态（2026-06-19）：已实施**（commits `819777d` LLM 按钮 + `3f96a31` ASR 按钮 + `e2cd7a8` check.svg 图标）。下方 checkbox 标记实际完成进度。

---

## 文件结构

| 文件 | 责任 | 动作 |
|---|---|---|
| `crates/llm/src/client.rs` | `test_connection()` 实现 | **新增函数** |
| `crates/llm/src/lib.rs` | 公开导出 | **修改 re-export** |
| `crates/desktop/src/settings_commands.rs` | 2 个 Tauri 命令 | **新增命令** |
| `crates/desktop/src/main.rs` | `invoke_handler` 注册 | **追加 2 项** |
| `crates/desktop/dist/settings/index.html` | UI（DOM + CSS + JS） | **修改** |
| `crates/desktop/dist/result/icons/check.svg` | FontAwesome check 图标源 | **新增资源** |

## 测试策略

无单测（UI + 网络命令，YAGNI）。每个 task 手动验证：
- Task 1：`cargo build -p octopus-llm` 编译通过
- Task 2：`cargo build -p octopus-desktop --features dashscope` 编译通过
- Task 3：启动应用 → 设置页 → ASR 选本地 → 按钮灰 / 选远程 → 点击 → 绿/红切换；LLM 选任意 → 点击 → 绿/红切换

---

## Task 1: LLM `test_connection` 函数

- [x] `crates/llm/src/client.rs` 新增 `pub fn test_connection(config: &CompatibleLlmConfig) -> Result<()>`
  - 复用 `ChatRequest` 结构 + `needs_disable_thinking()` 逻辑
  - messages=[{"role":"user","content":"Hi"}]，`max_tokens=1`，`temperature=0.0`
  - `reqwest::blocking::Client::builder().timeout(10s).build()`
  - 失败：`anyhow::context` 网络错误 / `bail!` 非 2xx + body
- [x] `crates/llm/src/lib.rs` re-export：`pub use client::{polish, test_connection};`
- [x] `cargo build -p octopus-llm` 通过

## Task 2: 两个 Tauri 命令注册

> **命令实现已重构为 `async fn`**——见 [2026-06-19-connection-test-async.md](./2026-06-19-connection-test-async.md) Task 1/2（LLM `spawn_blocking`、ASR 直接 `await connect_async`，删 `thread::spawn` + `Runtime::new`）。下方仅保留注册步骤（async/sync 注册方式一致）。

- [x] `crates/desktop/src/main.rs` 的 `invoke_handler![...]` 追加 `test_llm_connection` + `test_asr_connection`（async command 注册方式与 sync 相同，Tauri 自动适配）
- [x] `cargo build --release -p octopus-desktop --features "embedded dashscope"` 通过

## Task 3: 前端 UI（DOM + CSS + JS）

- [x] **资源**：`crates/desktop/dist/result/icons/check.svg` 新增（FontAwesome check 640×640 viewBox path）
- [x] **CSS**（`<style>` 内）：新增 `.test-btn`（32×32 圆角，hover 边框/图标变 primary）、`.test-btn.ok`（绿 #22c55e）、`.test-btn.fail`（红 #ef4444）、`.test-btn.loading`（半透明 + `pointer-events:none`）、`.test-btn.disabled`（`opacity:0.3` + `pointer-events:none`）、`.select-with-test`（flex 容器）
- [x] **JS 常量**：`const checkIconSvg = '<svg>...check.svg path...</svg>'`（内联，避免运行时加载）；`let asrEnginesData = []`（缓存引擎列表供 `updateAsrTestBtn` 查 `is_local`）
- [x] **renderSettings 改动**：
  - 缓存 `asrEnginesData = resp.asr_engines`
  - 求当前选中 ASR 引擎的 `is_local`（`currentAsrLocal`）
  - ASR select 包 `.select-with-test` + `<button class="test-btn{disabled}" id="asr-test-btn" onclick="testAsrConnection()">`，select 加 `onchange="...updateAsrTestBtn(this.value)"`
  - Polish LLM select 同样包 `.select-with-test` + `<button class="test-btn" id="llm-test-btn" onclick="testLlmConnection()">`（无 disabled 初始态）
- [x] **JS 函数**：
  - `testLlmConnection()`：取 select value → 先 `invoke('set_config', {key:'polish_llm', value:bareName})` 持久化 → `invoke('test_llm_connection', {spec:bareName})` → 切 ok/fail + showToast
  - `testAsrConnection()`：取 select value → `disabled` class 直接 return → `invoke('test_asr_connection', {bareName})` → 切 ok/fail + showToast
  - `updateAsrTestBtn(bareName)`：从 `asrEnginesData` 查 `is_local` → 切 `disabled` class + title
  - 三个函数都 `window.xxx = xxx` 显式挂全局（Tauri webview inline event handler 限制）
- [x] 启动应用 e2e：本地 ASR 灰 / 远程 ASR 可点 / LLM 可点 / 成功绿失败红 / loading 半透明

---

## 已知后续工作

- ASR 测试目前只验握手——未来可考虑发一个空 PCM 帧跑完整协议初始化（消耗 1 次 DashScope 调用，但能验 model_name 拼写）
- 抽 `check.svg` 的 path 为前端共享常量（当前 HTML 内联 + 独立 SVG 文件并存）


## 📄 `2026-06-19-connection-test-async.md`

# 连接测试 async 重构 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development（推荐）或 superpowers:executing-plans 按任务实施。步骤用 `- [ ]` 跟踪。

**Goal:** `test_llm_connection` / `test_asr_connection` 改 `async fn`，跑在 `tauri::async_runtime`，删除手动 `thread::spawn + join` 与 `Runtime::new()`，前端 invoke 契约不变。

**Architecture:** 两个 `#[tauri::command]` 改 `async`。LLM（`reqwest::blocking`）包 `tauri::async_runtime::spawn_blocking`；ASR（`tokio-tungstenite` WS）直接 `.await`。`generate_handler!` 注册方式不变，前端 `invoke` 契约不变。

**Tech Stack:** Tauri 2 async command, `tauri::async_runtime`, `tokio-tungstenite`, `reqwest::blocking`。

**Spec:** `docs/superpowers/specs/2026-06-19-connection-test-async-design.md`

> **状态：已实现**（commits `b2b67b3` + `6bd791a`，merge main；GUI 已验证）。Task 1 实施时发现 `test_connection` 返回 `anyhow::Error`，闭包内补 `.map_err(|e| format!("{}", e))` 转 `String`（plan 代码已同步，commit `af809c8`）。

---

## File Structure

- `crates/desktop/src/settings_commands.rs` — 仅此一个文件，改 2 个 fn。纯逻辑单测保持不变。

---

### Task 1: test_llm_connection 改 async

**Files:** Modify `crates/desktop/src/settings_commands.rs:260-278`

- [x] **Step 1: 改签名 + 实现**

将 `pub fn test_llm_connection` 整体替换为：

```rust
/// 测试 LLM 连接是否可用（发一个 max_tokens=1 的极简请求）。
/// spec 为 polish_llm 配置值（3-part spec 或裸名），从 DB 加载配置后测试连通性。
#[tauri::command]
pub async fn test_llm_connection(spec: String) -> Result<String, String> {
    if spec.is_empty() {
        return Err("未选择润色模型".into());
    }
    let llm_cfg = octopus_infra::db::load_llm_model(&spec)
        .map_err(|e| format!("从 DB 加载 LLM 配置失败: {}", e))?
        .ok_or_else(|| format!("DB 中未找到 LLM 模型 '{}'", spec))?;

    // reqwest::blocking 客户端跑在 spawn_blocking 线程池，不占用 async runtime worker。
    // test_connection 返回 Result<(), anyhow::Error>：闭包内先 map_err 转 String，
    // 使 spawn_blocking 返回 JoinHandle<Result<(), String>>，.await 后链式匹配 Result<String, String>。
    tauri::async_runtime::spawn_blocking(move || {
        octopus_llm::test_connection(&llm_cfg).map_err(|e| format!("{}", e))
    })
        .await
        .map_err(|_| "测试线程异常终止".to_string())?
        .map(|_| "连接成功".to_string())
}
```

说明：`spawn_blocking` 返回 `JoinHandle<Result<()>>`，`.await` 得 `Result<Result<()>, JoinError>`——外层 `map_err` 处理线程 panic/取消，内层 `map` 处理 `test_connection` 成功。

- [x] **Step 2: 编译验证**

Run: `cargo check -p octopus-desktop --features dashscope`
Expected: PASS（`main.rs` 的 `generate_handler!` 注册不变，async command 自动支持）

- [x] **Step 3: commit**

```bash
git add crates/desktop/src/settings_commands.rs
git commit -m "refactor(desktop): test_llm_connection 改 async + spawn_blocking"
```

---

### Task 2: test_asr_connection 改 async

**Files:** Modify `crates/desktop/src/settings_commands.rs:280-339`

- [x] **Step 1: 改签名 + 删 Runtime::new，WS 直接 await**

将 `pub fn test_asr_connection` 整体替换为（前置校验逻辑不变，仅签名 + WS 测试段改）：

```rust
/// 测试 ASR 远程引擎连接是否可用。
/// 本地模型返回 Err 提示无需连接测试；远程模型（provider=aliyun）检查 secret_key + WS 连通性。
#[tauri::command]
pub async fn test_asr_connection(bare_name: String) -> Result<String, String> {
    let engines = octopus_asr::config::list_engines().map_err(|e| e.to_string())?;
    let engine = engines.iter().find(|e| e.name == bare_name)
        .ok_or_else(|| format!("ASR 引擎 '{}' 不存在", bare_name))?;

    if engine.is_local {
        return Err("本地模型无需连接测试".into());
    }

    // 远程引擎：从 DB 取配置（source = WS endpoint, secret_key = API Key）
    let asr_cfg = octopus_asr::config::load_config().map_err(|e| e.to_string())?;
    let model_name = octopus_infra::db::parse_model_spec(&bare_name).model_name().to_string();
    let entry = asr_cfg.asr.aliyun.as_ref()
        .and_then(|m| m.get(model_name.as_str()))
        .ok_or_else(|| format!("远程 ASR 模型 '{}' 未在 DB 配置", bare_name))?;

    if entry.secret_key.is_empty() {
        return Err(format!("ASR 模型 '{}' 的 secret_key 为空", bare_name));
    }

    #[cfg(feature = "dashscope")]
    {
        use tokio_tungstenite::tungstenite::client::IntoClientRequest;
        let mut req = entry.source.clone().into_client_request()
            .map_err(|e| format!("WS 端点无效: {}", e))?;
        req.headers_mut().insert(
            "Authorization",
            format!("bearer {}", entry.secret_key).parse().unwrap(),
        );
        // 直接在 tauri::async_runtime 上 await，不再 thread::spawn + Runtime::new + block_on
        match tokio::time::timeout(
            std::time::Duration::from_secs(3),
            tokio_tungstenite::connect_async(req),
        ).await {
            Ok(Ok(_)) => Ok("连接成功".into()),
            Ok(Err(e)) => Err(format!("WS 连接失败: {}", e)),
            Err(_) => Err("WS 连接超时（3s）".into()),
        }
    }
    #[cfg(not(feature = "dashscope"))]
    {
        Err("远程 ASR 连接测试需要 dashscope feature".into())
    }
}
```

- [x] **Step 2: 编译验证**

Run: `cargo check -p octopus-desktop --features dashscope`
Expected: PASS（`connect_async` 在 tauri runtime 上下文，删 nested runtime 后无冲突）

- [x] **Step 3: commit**

```bash
git add crates/desktop/src/settings_commands.rs
git commit -m "refactor(desktop): test_asr_connection 改 async，删 Runtime::new"
```

---

### Task 3: 回归验证

- [x] **Step 1: 现有单测通过**

Run: `cargo test -p octopus-desktop`
Expected: PASS（纯逻辑单测——spec 解析、`is_local` 判定、`secret_key` 空检查——不受 async 改造影响）

- [x] **Step 2: 手动验证契约不变（需 GUI 环境）**

- 设置窗口选远程 LLM → 点测试 → 成功/失败文案与重构前一致
- 设置窗口选 aliyun ASR → 点测试 → 成功/失败文案一致
- 本地 ASR → 按钮灰 + 提示「本地模型无需连接测试」

- [x] **Step 3: workspace 整体编译**

Run: `cargo check --workspace --all-targets`
Expected: PASS，零 warning 回归

---

## Self-Review

- **Spec 覆盖**：§4.1 LLM async（Task 1）、§4.2 ASR async（Task 2）、§4.3 契约不变（Task 3 手动）✓
- **Placeholder 扫描**：无 TBD/TODO；两个 fn 给完整代码 ✓
- **类型一致**：`spawn_blocking` → `JoinHandle<Result<()>>` → `.await` → `Result<Result<()>, JoinError>` → `map_err` + `map` 链正确；ASR `connect_async` 返回 `Result<(WSStream, Response), Error>`，`timeout` 包一层 → 三臂 match 覆盖全 ✓


## 📄 `2026-06-19-result-window-edit-layout.md`

# 结果窗编辑布局调整 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 调整结果窗编辑布局——进入/退出编辑文字水平不重排，保存入口从文本区「完成编辑」按钮迁移到 toolbar 的 ✏️ toggle（编辑态切 💾 icon）。

**Architecture:** 仅改 `crates/desktop/dist/result/index.html`（单文件前端）+ 已就位 `icons/save.svg`。无 Rust/config 改动。删 `#edit-done` 按钮 + 编辑态 `padding-right:90px`（消除水平重排根因）；✏️ 按钮复用 toggle + CSS mask 切换 icon；编辑态强制 toolbar 常驻保证保存可见。

**Tech Stack:** vanilla HTML/CSS/JS（无构建）+ Tauri webview

**设计 spec:** `docs/superpowers/specs/2026-06-19-result-window-edit-layout-design.md`

> **状态（2026-06-19）：已实施并合并到 main**（`d4401cb`：✏️ toggle + 删 edit-done + 文字不重排 + 编辑态 toolbar 常驻；e2e 通过）。下方 checkbox 标记实际完成进度。快捷键后续统一为 `edit_shortcut` toggle（`370e21e`）。

---

## 文件结构

| 文件 | 责任 | 动作 |
|---|---|---|
| `crates/desktop/dist/result/index.html` | 结果窗前端（编辑态 DOM/CSS/JS） | **修改** |
| `crates/desktop/dist/result/icons/save.svg` | 保存图标（CSS mask 源，FA 软盘） | 已就位（`596cf86` 入库） |

## 测试策略

前端为 vanilla HTML/CSS/JS，无自动测试框架（YAGNI，不为单次改动引入）。每个 task 改完用 `cargo run -p octopus-desktop` 启动应用按步骤手动验证；Task 4 做全量 e2e（spec §7 六项）。无 Rust 单测（后端命令 `enter_edit_mode`/`commit_edit` 不变）。

> **行号说明**：下方行号基于当前 `main`（`596cf86`）。前序 task 改动后行号会偏移，**以代码内容（old_string）定位**，勿依赖行号。

## 实现 worktree（用户要求隔离）

执行本 plan 前，先 `EnterWorktree`（或 `superpowers:using-git-worktrees`）创建新 worktree，在 worktree 内逐 task 实现 + commit，最后按 `superpowers:finishing-a-development-branch` merge 回 main。

---

### Task 1: ✏️ 按钮复用 toggle + 图标切换

**Files:**
- Modify: `crates/desktop/dist/result/index.html`（CSS `#tool-edit .icon` 规则约 L103 + JS `btnEdit` click 约 L466 + `enterEdit`/`commitEdit` 约 L428-457）

- [x] **Step 1: CSS 加编辑态 icon 切换**

在 `#tool-edit .icon` 规则（约 L103）之后新增：

```css
    /* 编辑态：✏️ 图标切为 💾（save.svg）；点 tool-edit = 保存（toggle 语义） */
    #container.editing #tool-edit .icon {
      -webkit-mask-image: url(icons/save.svg?v=1);
      mask-image: url(icons/save.svg?v=1);
    }
```

- [x] **Step 2: JS `btnEdit` click 改 toggle 语义**

old:
```js
    btnEdit.addEventListener('click', (e) => { e.preventDefault(); enterEdit(); });
```
new:
```js
    btnEdit.addEventListener('click', (e) => {
      e.preventDefault();
      editing ? commitEdit() : enterEdit();
    });
```

- [x] **Step 3: `enterEdit`/`commitEdit` 加 title/aria-label 切换**

`enterEdit()` 中，在 `btnEdit.classList.add('active');` 之后加：
```js
      btnEdit.title = '保存编辑';
      btnEdit.setAttribute('aria-label', '保存编辑');
```

`commitEdit()` 中，在 `btnEdit.classList.remove('active');` 之后加：
```js
      btnEdit.title = '编辑';
      btnEdit.setAttribute('aria-label', '编辑');
```

- [x] **Step 4: 手动验证**

Run: `cargo run -p octopus-desktop`
Expected: 识别出文字 → 点 ✏️ 进入编辑 → 图标变 💾、tooltip「保存编辑」→ 点 💾 → 保存退出、图标回 ✏️。
（此 task 后 `#edit-done` 按钮仍在，两保存入口临时并存——Task 2 删除。）

- [x] **Step 5: Commit**

```bash
git add crates/desktop/dist/result/index.html
git commit -m "feat(desktop): ✏️ 按钮复用 toggle——编辑态切 save icon + click toggle 保存"
```

---

### Task 2: 删除 `#edit-done` 按钮 + 移除编辑态 padding（文字不重排）

**Files:**
- Modify: `crates/desktop/dist/result/index.html`（DOM 约 L241 + CSS 约 L184-209 + JS `btnEditDone` 引用 L426/433/454/482-483/497）

- [x] **Step 1: 删 DOM**

删 `#text-wrapper` 内这一行：
```html
      <button id="edit-done" hidden>完成编辑</button>
```
（删后 `#text-wrapper` 内只剩 `<div id="result-text"></div>`）

- [x] **Step 2: 删 CSS**

删 `#edit-done` 全部规则（约 L184-198，含注释「完成编辑按钮：编辑态显示，浮于文本区右上」+ `#edit-done` + `#edit-done:hover`）。

删编辑态 `#result-text` 的 padding 规则（约 L206-208）：
```css
    #container.editing #result-text {
      padding: 1px 90px 7px 13px;   /* right 90px 给完成按钮让位 */
    }
```
**保留**其下的 `#container.editing #result-text:focus { background: transparent; }` 与 `#container.editing #text-wrapper` 淡蓝边框规则。

- [x] **Step 3: 删 JS `btnEditDone` 全部引用**

- 删 `const btnEditDone = document.getElementById('edit-done');`（约 L426）
- `enterEdit()` 删 `btnEditDone.hidden = false;`（约 L433）
- `commitEdit()` 删 `btnEditDone.hidden = true;`（约 L454）
- 删两行 `btnEditDone.addEventListener(...)`（mousedown/click，约 L482-483）
- `edit-force-exit` 处理删 `btnEditDone.hidden = true;`（约 L497）

- [x] **Step 4: 手动验证**

Run: `cargo run -p octopus-desktop`
Expected: ✏️ 进入编辑 → **文字水平位置不变（不重排）**；文本区无「完成编辑」按钮；点 💾 保存正常。

- [x] **Step 5: Commit**

```bash
git add crates/desktop/dist/result/index.html
git commit -m "refactor(desktop): 删 edit-done 按钮 + 移除编辑态 padding（文字不重排）"
```

---

### Task 3: 编辑态 toolbar 强制常驻

**Files:**
- Modify: `crates/desktop/dist/result/index.html`（`enterEdit()` 约 L428-445）

- [x] **Step 1: `enterEdit` 末尾调 `showToolbar()`**

在 `enterEdit()` 内、`invoke('enter_edit_mode');` 之前加：
```js
      showToolbar();
```
（`showToolbar` 内部 `if (toolbarVisible) return`——点 ✏️ 进入时 toolbar 已 visible，no-op 无跳动；Cmd+Enter 进入若 hidden 则显示。`hideToolbar` 已有 `editing` 拦截，编辑中不会隐藏，无需改。）

- [x] **Step 2: 确认 force-exit 自动恢复 icon（CSS 驱动，无需额外 JS）**

icon 切换靠 `#container.editing #tool-edit .icon` CSS。`edit-force-exit` 处理（约 L491-500）已 `container.classList.remove('editing')`（移除 editing class → CSS 不再匹配 → 图标自动回 `edit.svg`）+ `btnEdit.classList.remove('active')`。**无需补图标恢复代码**，确认这两行存在即可。

- [x] **Step 3: 手动验证**

Run: `cargo run -p octopus-desktop`
Expected:
1. 鼠标移出结果窗使 toolbar 隐藏 → Cmd+Enter 进入 → toolbar 出现（窗口增高、文字下移 24px）→ 💾 可见可点
2. 编辑中 mouseleave → toolbar **不隐藏**（editing 拦截）
3. 编辑中触发新录音（force-exit）→ 图标自动回 ✏️、退出编辑态

- [x] **Step 4: Commit**

```bash
git add crates/desktop/dist/result/index.html
git commit -m "feat(desktop): 编辑态强制 toolbar 常驻（enterEdit showToolbar）"
```

---

### Task 4: 全量 e2e + 文档同步 + 收尾

- [x] **Step 1: 全量 e2e（spec §7 六项）**

Run: `cargo run -p octopus-desktop`，逐项验证：
1. 识别出文字 → ✏️ 进入 → **文字水平位置不变（不重排）** ✓
2. 编辑态图标 💾 → 点 💾 保存 → 退出、图标回 ✏️ ✓
3. `edit_shortcut` 进入 → 再按 `edit_shortcut` 保存（toggle）✓
4. Cmd+Enter 进入（toolbar 此前 hidden）→ toolbar 出现 → 💾 可见可点 ✓
5. 编辑中 mouseleave → toolbar 不隐藏 ✓
6. 编辑中触发新录音（force-exit）→ 图标回 ✏️、退出编辑态 ✓

- [x] **Step 2: 文档同步检查**

Run: `grep -rn "edit-done" docs/ crates/`（排除本 spec/plan）
Expected: 若 `architecture.md` L198-203「结果窗可编辑」段或 editable-result spec 提到 `edit-done`/「完成编辑」按钮，更新为「✏️ toggle（编辑态 💾）」。spec §8 已声明不改 editable-result spec 机制描述，预期改动小或无。

- [x] **Step 3: Commit（若有文档改动）**

```bash
git add docs/
git commit -m "docs: 同步结果窗编辑布局调整（保存按钮移 toolbar toggle）"
```
（若无改动跳过）

- [x] **Step 4: 收尾**

按 `superpowers:finishing-a-development-branch`：workspace 测试（手动 e2e 已过）→ merge worktree 分支回 main（ff）→ 删 worktree 分支。

---

## Spec Coverage（自审）

| spec 章节 | 实现 task |
|---|---|
| §4.1 删 `#edit-done`（DOM/CSS/JS） | Task 2 |
| §4.2 移除编辑态 padding（文字不重排） | Task 2 |
| §4.3 ✏️ toggle + 图标切换（CSS mask + JS toggle） | Task 1 |
| §4.4 编辑态 toolbar 强制常驻（`enterEdit` showToolbar） | Task 3 |
| §4.5 不变项（快捷键/后端命令） | 无改动（验证 Task 4） |
| §6 force-exit 图标恢复 | Task 3 Step 2（CSS 驱动自动） |
| §7 e2e 六项 | Task 4 Step 1 |


## 📄 `2026-06-19-vad-preheat.md`

# 启动/录音性能优化 Implementation Plan：VAD session 缓存（①），lock-free 音频（③可选）

> 状态：**已实现**（commits `c15c159` + `569f94b` + `07a1503`，merge main `07a1503`）
> Spec：`docs/superpowers/specs/2026-06-19-vad-preheat-design.md`（v3）

**Goal:** VAD 的 ONNX Session 全局缓存，`SileroVad::new()` 廉价化，消除首次按快捷键的录音启动延迟；preheat 预加载 VAD。

**实际实现偏离原 plan 两点**（subagent-driven 实施中发现，已修正）：

1. **struct 字段 `Arc<Session>` → `Arc<Mutex<Session>>`**：原 plan 假定 `Session::run(&self)`，实施时验证 ort 源码（`session/mod.rs:212`）`run` 是 `&mut self`——`Arc<Session>` 编译失败（deref 只给 `&Session`）。改 `Arc<Mutex<Session>>`，`compute()` 里 `self.session.lock().unwrap()` 拿 `&mut Session`。`Session: Send + Sync` 断言通过（回退非因 Send/Sync）。
2. **新增持锁 get-or-insert 修复 TOCTOU**（commit `07a1503`）：原 plan 的 `get→drop lock→load→re-lock insert` 有 TOCTOU 窗口（并发 miss 重复加载 + 互相覆盖致 `Arc::ptr_eq` 失败）。code review 后改为整个 get-or-insert 在持 cache lock 期间完成，消除 race + 删掉为掩盖它而加的 TEST_GATE 测试 hack。

---

## File Structure（实际）

- `crates/asr/src/vad.rs` — `SileroVad.session: Arc<Mutex<Session>>`；`VAD_SESSIONS` 缓存（持锁 get-or-insert）；`compute` lock；2 单测；Send+Sync 断言。
- `crates/desktop/src/main.rs` — preheat 后台线程追加 VAD 预加载。
- `crates/desktop/src/coordinator.rs` — **不改**（零改动已验证）。

---

## Tasks（已完成）

### Task 1: vad.rs session 缓存 + Arc<Mutex<Session)>  ✅ commit c15c159

- [x] Step 1: `Session: Send + Sync` 静态断言（通过）
- [x] Step 2-3: import + `VAD_SESSIONS` static + `vad_sessions()` helper；struct `session: Arc<Mutex<Session>>`
- [x] Step 4: `new()` 缓存（命中 clone Arc + zeros；miss 加载 + insert）
- [x] Step 5: 单测 `same_path_shares_session`（`Arc::ptr_eq`）+ `compute_returns_probability_in_range`
- [x] Step 6: commit `c15c159`

### Task 2: main.rs preheat 预加载 VAD session  ✅ commit 569f94b

- [x] Step 1: preheat 后台线程闭包内（ASR switch_model 之后）追加 VAD `SileroVad::new` 预加载，失败降级 warn
- [x] Step 2: commit `569f94b`

### Task 2.5: 持锁 get-or-insert 修复 TOCTOU  ✅ commit 07a1503

（code review 后追加，非原 plan）
- [x] `new()` 改为持 cache lock 完成 get-or-insert（消除 TOCTOU + 重复加载）
- [x] 删除 TEST_GATE 测试 hack（持锁后并发 miss 也命中同一 Arc，`ptr_eq` 恒成立）
- [x] 3 次多线程 `cargo test` 无 flake
- [x] commit `07a1503`

### Task 3（可选，未实现）: ③ lock-free 音频 ring buffer

> 不做。无证据锁是瓶颈。①完成后若 profiling 显示热路径延迟再启动。

- [ ] 评估而非实现（条件未满足）

### Task 4: 验证  ✅

- [x] `cargo test -p octopus-asr`：42 passed, 6 ignored
- [x] `cargo check --workspace --all-targets`：clean
- [x] coordinator 零改动（`git diff af809c8..07a1503 -- coordinator.rs` 空）
- [x] 手动 e2e：通过，无回归；延迟降幅未量化（无改动前基线对比）

---

## 实施记录（subagent-driven）

- implementer：task1+2 DONE_WITH_CONCERNS（发现 `run &mut self`，走 Mutex 回退）
- spec review：✅ 回退方案逐项正确（lock 作用域、reset 不锁、无死锁、coordinator 零改动）
- code review：Approved with recommendations（TEST_GATE + TOCTOU 指向同根因）
- implementer 修：持锁 get-or-insert + 删 TEST_GATE → DONE，3 次测试无 flake
- 未做（非阻塞）：`.lock().unwrap()` poison 容错（`into_inner()`）、compute lock 作用域收窄、commit `c15c159` message 准确性（rebase 限制未 amend）
