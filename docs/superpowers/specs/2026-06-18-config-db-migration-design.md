# 配置 DB 迁移设计

## 背景

config.yaml 与 octopus.db 两套存储系统并存，yaml 需独立维护序列化/字段迁移逻辑。每次字段重命名都需在 `load_config()` 中添加 yaml Value 层手动迁移代码（serde alias 在两键共存时 panic），维护成本高。

## 方案

将 config.yaml 全部 21 个字段迁移到 SQLite `app_config` 表（key-value TEXT 存储），与模型配置/识别历史共用同一 `octopus.db`。

### 表结构

```sql
CREATE TABLE IF NOT EXISTS app_config (
    config_key   TEXT PRIMARY KEY,
    config_value TEXT NOT NULL,
    description  TEXT
);
```

值统一 TEXT 存储，由 `load_app_config()` 按字段类型解析（bool.parse()、f64.parse()、u8 枚举映射）。

### 关键决策

1. **TEXT 统一存储**：bool/f64/u8 序列化为 TEXT，load 时按字段类型 parse。避免 BLOB/多类型列的复杂度。
2. **seed 幂等**：`INSERT OR IGNORE`，已有配置不被覆盖。db.sql 21 行 seed 保证首次启动即有默认值。
3. **yaml 一次性迁移**：`init_schema` 在 v0/v1→v2 升级时检测旧 config.yaml → serde 解析（含字段名迁移 shortcut→asr_shortcut 等）→ `save_app_config_at` 覆盖 seed → 重命名 config.yaml 为 config.yaml.bak。
4. **v1→v2 迁移策略**：INIT_SQL 全部是 `CREATE TABLE IF NOT EXISTS` + `INSERT OR IGNORE`，幂等安全重跑。v1 数据库直接重跑 INIT_SQL 即可补建 app_config 表 + seed，无需单独迁移 SQL。
5. **单键写 vs 全量写**：`persist_*`（工具栏切换）用 `save_config_key`（单键 INSERT OR REPLACE，避免全量回写）；`set_config`（设置窗口表单）用 `save_app_config`（全量 21 字段 INSERT OR REPLACE）。
6. **AppConfig struct 保持不变**：仍然是 serde Serialize/Deserialize，用于：a) 前端 JSON 序列化（get_config 命令）；b) yaml 迁移路径中的 `serde_yaml::from_value` 解析旧 config.yaml。

### 排除方案

- **serde alias 迁移字段名**：两键共存时 duplicate field panic，改为 yaml Value 层手动迁移（在 `migrate_yaml_to_db` 中一次性完成）。
- **category 分组列**：YAGNI，21 个扁平 key-value 无分组需求。
- **多类型列（TEXT/INTEGER/REAL 分列）**：增加 schema 复杂度，parse 开销可忽略。

## 涉及文件

| 文件 | 变更 |
|------|------|
| `crates/infra/src/db.sql` | 新增 `app_config` 表 + 21 行 seed |
| `crates/infra/src/db.rs` | 新增 `load_app_config` / `save_app_config` / `save_config_key` / `migrate_yaml_to_db`；更新 `init_schema`（v0→v2, v1→v2） |
| `crates/infra/src/config.rs` | `load_config()` 改为薄包装 `db::load_app_config()`；移除 yaml 解析 + `migrate_key` |
| `crates/desktop/src/runtime_config.rs` | `persist_*` 改用 `db::save_config_key`；移除 `write_config_yaml` |
| `crates/desktop/src/settings_commands.rs` | `set_config` 改用 `db::save_app_config`；移除本地 `write_config_yaml` |

## 迁移流程

```
首次启动 (v0/v1 → v2):
  init_schema()
    → execute_batch(INIT_SQL)        // 幂等：建表 + seed（含 app_config）
    → migrate_yaml_to_db(conn)       // 检测 config.yaml
        → 存在？ → serde_yaml 解析 + 字段名迁移
                 → save_app_config_at() 覆盖 seed
                 → rename config.yaml → config.yaml.bak
        → 不存在？ → 直接返回
    → PRAGMA user_version = 2

后续启动 (v2+):
  init_schema() → v >= 2 → 跳过
```
