# LLM 润色提示词表设计

**日期**：2026-06-21
**类型**：新功能（DB schema 变化 + prompt 组装重构）
**相关文件**：`crates/infra/src/db.sql`、`crates/infra/src/db.rs`、`crates/llm/src/prompt.rs`、`crates/desktop/src/main.rs`、`crates/desktop/src/settings_commands.rs`、`crates/desktop/src/coordinator.rs`、`crates/desktop/src/runtime_config.rs`、`docs/architecture.md`、`docs/configuration.md`

## 1. 目标

把当前单文件 `~/.octopus/VOICE_POLISH.md` 的润色 prompt 机制改为 DB 多 prompt 管理：

- DB `prompts` 表存多条润色 prompt
- `app_config.active_polish_prompt` 指定当前激活的一条（id）
- 有一条 `is_system=true` 的默认兜底 prompt（seed，不可编辑/删除）
- 用户可添加任意特色 prompt（如「日常沟通」「技术写作」「会议纪要」），激活其一
- 删除 `VOICE_POLISH.md` 文件读取机制（开发阶段无历史遗留）

## 2. DB Schema

### 2.1 新增 `prompts` 表

```sql
CREATE TABLE IF NOT EXISTS prompts (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,    -- 系统主键，app_config.active_polish_prompt 引用此字段（用户不可编辑）
    title       TEXT    NOT NULL,                     -- 用户可读别名（允许重复，用户自行区分）
    category    TEXT    NOT NULL DEFAULT 'voice_text_polish', -- 用途分类（当前固定 voice_text_polish 语音文本润色）
    content     TEXT    NOT NULL,                     -- system prompt 的「风格规则」部分（不含增量保留规则）
    description TEXT    NOT NULL DEFAULT '',          -- 用户可读描述
    is_system   INTEGER NOT NULL DEFAULT 0,           -- 1=系统内置（不可编辑/删除），0=用户自建
    created_at  TEXT    NOT NULL DEFAULT (datetime('now')),
    updated_at  TEXT    NOT NULL DEFAULT (datetime('now'))
);
```

**设计决策**：
- `id` 作为系统主键（全表唯一、系统生成、用户不可编辑），`app_config.active_polish_prompt` 存 id（以字符串形式，与其他 app_config 一致）
- `title` 作为用户可读别名，**允许重复**（用户自行区分即可，不做唯一约束）
- `category` 标记 prompt 用途，当前固定 `voice_text_polish`（语音文本润色）
- `is_system` 用 INTEGER（0/1），与 `models.is_local` 等现有列一致
- `content` 只存「风格规则」部分，增量保留规则由代码强制拼接（见 §3）
- 不设 `is_enabled` 列——prompt 不需要禁用，不激活就不用

### 2.2 Seed 默认 prompt

```sql
INSERT OR IGNORE INTO prompts (id, title, category, content, description, is_system) VALUES
    (1, '默认润色', 'voice_text_polish', '<当前 DEFAULT_SYSTEM_PROMPT 的前 6 条规则>', '默认润色（系统内置）', 1);
```

固定 `id=1` 作为系统默认 prompt。Seed content = 现有 `DEFAULT_SYSTEM_PROMPT` 去掉第 7 条（增量保留规则），第 7 条改为代码常量强制拼接。

### 2.3 新增 app_config key

```sql
INSERT OR IGNORE INTO app_config (config_key, config_value, description) VALUES
    ('active_polish_prompt', '1', '激活的润色 prompt id（prompts 表 id 字段）');
```

默认值 `'1'` 指向 seed 的系统内置 prompt（id=1）。存为字符串（与其他 app_config 一致，TEXT 列）。

### 2.4 Schema 版本迁移

`init_schema` 新增 `v3 → v4` 迁移：
- 执行 `CREATE TABLE IF NOT EXISTS prompts` + `INSERT OR IGNORE` seed（幂等，IF NOT EXISTS / OR IGNORE）
- 执行 `INSERT OR IGNORE INTO app_config` seed `active_polish_prompt`
- `PRAGMA user_version = 4`

同时更新 `INIT_SQL`（`db.sql`）包含新表 + seed，保证全新安装一步到位。

## 3. Prompt 组装重构（`crates/llm/src/prompt.rs`）

### 3.1 当前结构

```
DEFAULT_SYSTEM_PROMPT = 第 1~6 条风格规则 + 第 7 条增量保留规则
PROMPT_OVERRIDE = VOICE_POLISH.md 内容（整体覆盖）
system_prompt() = PROMPT_OVERRIDE 或 DEFAULT_SYSTEM_PROMPT
```

### 3.2 新结构

```
INCREMENTAL_RULE = 第 7 条增量保留规则（代码常量，含 CONFIRMED_MARKER）
system_prompt(user_content: &str) = user_content + "\n" + INCREMENTAL_RULE
```

**关键变化**：
- `set_system_prompt_override` / `PROMPT_OVERRIDE` / `DEFAULT_SYSTEM_PROMPT` **删除**
- 新增 `pub fn build_system_prompt(content: &str) -> String`：拼接用户 prompt + 强制增量规则
- `user_prompt()` 不变（它构造 user message，与 system prompt 解耦）
- `CONFIRMED_MARKER` 不变（`INCREMENTAL_RULE` 复用它）

### 3.3 调用方改造

**`crates/desktop/src/main.rs`**（启动时加载 prompt）：
- 删除 `VOICE_POLISH.md` 读取逻辑（约 130-145 行）
- 改为从 DB 读 `active_polish_prompt` → 查 `prompts` 表取 content → `build_system_prompt(content)` 传给润色流程

**润色调用链**（`coordinator.rs` → `spawn_polish_thread` → `octopus_llm::polish`）：
- 当前 `octopus_llm::polish` 内部调 `system_prompt()` 取全局静态值
- 改为：调用方传入 `system_prompt: &str` 参数（由 `build_system_prompt` 构建）
- 或：保留全局静态，但改为运行时可切换（`set_system_prompt` 接受新 content → 重新 build）

**推荐方案：运行时可切换的全局静态**。理由：
- 改动最小（`polish` 签名不变，内部仍调 `system_prompt()`）
- 切换 prompt 时只需 `set_system_prompt(new_content)` → 下次润色生效
- `system_prompt()` 返回 `&'static str` 改为 `&str`（指向 `RwLock<String>`）

具体实现：
```rust
static SYSTEM_PROMPT: RwLock<String> = RwLock::new(String::new());

/// 设置当前 system prompt（content 为用户 prompt 部分，内部自动拼接增量规则）
pub fn set_system_prompt(content: &str) {
    *SYSTEM_PROMPT.write().unwrap() = build_system_prompt(content);
}

/// 获取当前 system prompt（已含增量规则）
pub fn system_prompt() -> String {
    SYSTEM_PROMPT.read().unwrap().clone()
}
```

**注意**：`system_prompt()` 从 `&'static str` 改为 `String`（clone），调用方需适配。影响范围：`crates/llm/src/lib.rs`（`polish` 函数）。

## 4. 设置窗口 Tauri 命令

新增 5 个命令（`settings_commands.rs`）：

| 命令 | 签名 | 说明 |
|------|------|------|
| `list_prompts` | `() -> Vec<PromptInfo>` | 列出所有 prompt（按 id 排序，系统内置优先） |
| `get_active_prompt` | `() -> i64` | 返回当前激活的 prompt id |
| `set_active_prompt(id: i64)` | `-> Result<()>` | 设置激活 prompt（校验 id 存在 + 写 app_config + 调 `set_system_prompt` 即时生效） |
| `create_prompt(title, content, description)` | `-> Result<i64>` | 新建用户 prompt（校验 title 非空）返回新 id |
| `update_prompt(id, title, content, description)` | `-> Result<()>` | 更新用户 prompt（拒绝 is_system=true） |
| `delete_prompt(id)` | `-> Result<()>` | 删除用户 prompt（拒绝 is_system=true；若删的是激活项，回退到 id=1） |

**`PromptInfo` 结构**：
```rust
#[derive(Serialize)]
pub struct PromptInfo {
    pub id: i64,
    pub title: String,
    pub content: String,
    pub description: String,
    pub is_system: bool,
}
```

## 5. 运行时切换

工具栏已有「润色模型」切换（`switch_polish_llm`）。是否需要工具栏加「润色 prompt」切换？

**决定：不加**。理由：
- prompt 切换是低频操作（不像润色模式 / 引擎切换那样需要快速访问）
- 设置窗口的 prompt 管理页足够
- `set_active_prompt` 即时生效（写 app_config + `set_system_prompt`），下次润色就用新 prompt

若后续需要快速切换，可在工具栏加二级菜单，当前 YAGNI。

## 6. DB CRUD 函数（`crates/infra/src/db.rs`）

新增函数：

```rust
pub struct PromptRecord {
    pub id: i64,
    pub title: String,
    pub content: String,
    pub description: String,
    pub is_system: bool,
}

pub fn list_prompts() -> Result<Vec<PromptRecord>>       // 按 id 排序，is_system 优先
pub fn load_prompt(id: i64) -> Result<Option<PromptRecord>>
pub fn insert_prompt(title: &str, content: &str, description: &str) -> Result<i64>  // 返回新 id
pub fn update_prompt(id: i64, title: &str, content: &str, description: &str) -> Result<()>
pub fn delete_prompt(id: i64) -> Result<()>
```

**约束**（DB 层或应用层）：
- `id` 主键唯一由 DB 保证；`title` 无唯一约束（允许重复）
- `update_prompt` / `delete_prompt`：应用层检查 `is_system`，拒绝系统 prompt

## 7. 不变量

1. **prompts 表永远有一条 id=1 的记录**（seed 保证，用户不能删）
2. **`active_polish_prompt` 永远指向存在的 prompt id**（set_active_prompt 校验；若指向的 prompt 被外部删除，fallback 到 id=1）
3. **system prompt 永远含增量保留规则**（`build_system_prompt` 强制拼接，用户 content 无论写什么都会追加）
4. **is_system=true 的 prompt 不可编辑/删除**（update/delete 应用层拒绝）
5. **切换 prompt 即时生效**（`set_active_prompt` 调 `set_system_prompt`，下次润色用新 prompt；进行中的润色不受影响——LLM 请求已发出）

## 8. 降级路径

- **DB 读 prompt 失败**：fallback 到 `INCREMENTAL_RULE` 拼接空 content（等价于无风格规则，仅保留增量逻辑）+ warn 日志
- **`active_polish_prompt` 指向不存在的 id**：fallback 到 id=1 + warn 日志 + 自动修正 app_config
- **prompt content 为空**：允许（等价于纯增量规则，用户可能只想做标点修正）

## 9. 验证方法

- **单元测试**（`db.rs`）：list/load/insert/update/delete prompt 的 CRUD + is_system 保护
- **单元测试**（`prompt.rs`）：`build_system_prompt` 拼接正确（用户 content + 增量规则）
- **集成验证**（手动）：
  1. 启动 → 确认默认 prompt = id=1（默认润色），润色结果与改动前一致
  2. 新建 prompt「技术写作」→ 激活 → 确认润色风格变化
  3. 切回 id=1 → 确认风格恢复
  4. 尝试删除 id=1 → 确认被拒绝
  5. 尝试编辑 id=1 → 确认被拒绝
  6. mode=2 中间润色 → 确认增量保留规则生效（已确认部分不被 LLM 改）
- **构建验证**：`cargo build -p octopus-desktop --features embedded,cloud` + `cargo test`

## 10. 文件变更清单

| 文件 | 变更 |
|------|------|
| `crates/infra/src/db.sql` | 新增 prompts 表 + seed + app_config seed |
| `crates/infra/src/db.rs` | v3→v4 迁移 + PromptRecord struct + 5 个 CRUD 函数 + 测试 |
| `crates/llm/src/prompt.rs` | 删除 PROMPT_OVERRIDE/DEFAULT_SYSTEM_PROMPT，新增 INCREMENTAL_RULE/build_system_prompt/set_system_prompt，system_prompt 改返回 String |
| `crates/llm/src/lib.rs` | 适配 system_prompt() 返回类型变化 |
| `crates/desktop/src/main.rs` | 删除 VOICE_POLISH.md 读取；改为从 DB 读 active prompt → set_system_prompt |
| `crates/desktop/src/settings_commands.rs` | 新增 6 个 Tauri 命令 + PromptInfo struct |
| `crates/desktop/src/runtime_config.rs` | set_active_prompt 即时生效（调 set_system_prompt） |
| `crates/infra/src/consts.rs` | 删除 VOICE_POLISH_FILE 常量 |
| `crates/desktop/src/main.rs` | 注册新 Tauri 命令 |
| `docs/architecture.md` | 同步 prompt 管理章节 |
| `docs/configuration.md` | 新增 active_polish_prompt 字段说明 |
