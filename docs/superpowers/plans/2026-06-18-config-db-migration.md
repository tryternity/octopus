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
