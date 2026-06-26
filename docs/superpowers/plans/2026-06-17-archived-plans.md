# 归档实施计划（2026-06-17，已实现）

> 本文件合并以下**已实现功能**的原始实施 plan，作为历史记录归档（2026-06-18）。
> 各功能已在 main 实现，**权威现状以 [`architecture.md`](../../architecture.md) 为准**。
> 归档内各 plan 之间的交叉引用可能指向已归档的同级文件——所需内容均在本文内，请按下方标题搜索。

## 包含的原 plan

- `2026-06-17-aliyun-cloud-apis.md`
- `2026-06-17-denoise-deepfilternet3-integration.md`
- `2026-06-17-settings-window.md`

---

## `aliyun-cloud-apis.md`

# 阿里云云端 API 接入（LLM + ASR）实施计划

> **For agentic workers:** REQUIRED SUB-SKILL: 用 superpowers:subagent-driven-development（推荐）或 superpowers:executing-plans 逐任务实现。步骤用 checkbox（`- [x]`）跟踪。

**Goal:** 接入阿里云 LLM（DashScope OpenAI 兼容，零代码）+ 阿里云 FunASR Realtime WS ASR（新 engine），并统一模型配置 taxonomy（加 `provider` 列、`name`→`model_name`、3-part spec）。

**Architecture:** `models` 表加 `provider` 维度（vendor/local vs aliyun）正交于 `category`（引擎族/模型系列），ASR 与 LLM 统一 `{provider}:{category}:{model_name}` 选择规格。ASR 云引擎走桌面分块 `TranscriptionEngine` 路径（`is_streaming=0`），每段开 WS 连接跑 DashScope duplex 协议。详见 spec `docs/superpowers/specs/2026-06-17-aliyun-cloud-apis-design.md`。

**Tech Stack:** Rust + Tauri（desktop）/ axum（server）；SQLite（rusqlite 0.31 bundled，含 db.sql `include_str!`）；tokio-tungstenite（WS）；DashScope `api-ws` 协议。

> 状态：✅ 全部完成（2026-06-17，合并 main commit `ca53db8`）。原 `worktree-aliyun-apis` 分支已合并并清理。WS e2e 集成测试标 `#[ignore]`，待手动验证（见末尾「验证」节）。

---

## File Structure

- **改：** `crates/infra/src/db.sql`（DDL + 全部 seed 迁移）
- **改：** `crates/infra/src/db.rs`（`parse_model_spec` 3-part、`ModelSpec`、load 查询、`LlmModelInfo.model_name`、测试）
- **改：** `crates/asr/src/config.rs`（`EngineCategory::Aliyun`、`AsrSection.aliyun`、resolver/pick/list/loads、测试）
- **改：** `crates/desktop/src/runtime_config.rs`（`.name`→`.model_name` 访问点）
- **改：** `crates/desktop/src/main.rs`（云引擎路由）
- **改：** `crates/desktop/Cargo.toml`（`dashscope` feature）
- **改：** `crates/cli/src/main.rs`、`crates/server/src/main.rs`（grep 命中点适配，预期极少）
- **新建：** `crates/desktop/src/engine_dashscope.rs`
- **改：** `docs/configuration.md`、`docs/architecture.md`

> ⚠️ **原子性：** Task 1（taxonomy 重构）必须一次性完成再提交——`name`→`model_name` + 3-part parse 使中间态全 workspace 不编译。Task 2/3 在 Task 1 绿之后独立提交。

---

## Task 1: 统一 taxonomy 重构（provider + model_name + 3-part）— 原子提交

**Files:** `crates/infra/src/db.sql`、`crates/infra/src/db.rs`、`crates/asr/src/config.rs`、`crates/desktop/src/runtime_config.rs`、`crates/cli/src/main.rs`、`crates/server/src/main.rs`

- [x] **Step 1.1：改 `db.sql` DDL + seed（schema 部分）**

`models` 表 DDL（`crates/infra/src/db.sql`）：把 `name` 列改 `model_name`，加 `provider` 列，改唯一键。

```sql
CREATE TABLE IF NOT EXISTS models (
    id            INTEGER PRIMARY KEY AUTOINCREMENT,
    domain        TEXT    NOT NULL,            -- 'asr' | 'llm'
    provider      TEXT    NOT NULL DEFAULT 'local', -- vendor/运行位置：local/aliyun/deepseek/bigmodel
    category      TEXT    NOT NULL,            -- ASR引擎族(zipformer/whisper/Fun-ASR) ; LLM模型系列(qwen/glm/deepseek)
    model_name    TEXT    NOT NULL,            -- 具体模型标识，精确匹配
    source        TEXT    NOT NULL,            -- ASR: 本地路径/HF repo/云wss端点 ; LLM: API base URL
    secret_key    TEXT    NOT NULL DEFAULT '', -- 远程 API Key（本地模型留空）
    language      TEXT    NOT NULL DEFAULT '',
    is_local      INTEGER NOT NULL DEFAULT 0,
    is_thinking   INTEGER NOT NULL DEFAULT 0,
    is_streaming  INTEGER NOT NULL DEFAULT 0,
    is_enabled    INTEGER NOT NULL DEFAULT 1,
    description   TEXT    NOT NULL DEFAULT '',
    UNIQUE(domain, provider, category, model_name)
);
```

ASR seed（全部本地，加 `provider='local'`，列序 `domain,provider,category,model_name,source,language,description,is_local,is_enabled,is_streaming`）：

```sql
INSERT OR IGNORE INTO models (domain, provider, category, model_name, source, language, description, is_local, is_enabled, is_streaming)
VALUES
    ('asr','local','zipformer','zipformer-small-ctc','models/zipformer','zh','zipformer-small-ctc, 27M（随应用打包，兜底引擎）',1,1,1),
    ('asr','local','zipformer','zipformer-multi','k2-fsa/sherpa-onnx-streaming-zipformer-ctc-multi-zh-hans-int8-2023-12-13','zh','zipformer-multi, 80M',1,0,1),
    ('asr','local','zipformer','zipformer-ctc','csukuangfj/sherpa-onnx-streaming-zipformer-ctc-zh-int8-2025-06-30','zh','zipformer-ctc, 163M',1,0,1),
    ('asr','local','paraformer','paraformer-streaming','csukuangfj/sherpa-onnx-streaming-paraformer-zh','zh','paraformer-streaming, 230M',1,0,1),
    ('asr','local','sensevoice','sherpa-onnx-sense-voice-funasr-nano-int8','csukuangfj/sherpa-onnx-sense-voice-funasr-nano-int8-2025-12-17','auto','SenseVoice FunASR Nano INT8, 265M',1,0,0),
    ('asr','local','qwen3-asr','qwen3-asr-0.6B','csukuangfj2/sherpa-onnx-qwen3-asr-0.6B-int8-2026-03-25','auto','qwen3-asr-0.6B, 1G',1,0,0),
    ('asr','local','qwen3-asr','qwen3-asr-1.7B','ilmina/qwen3-asr-1.7b-sherpa-onnx','auto','qwen3-asr-1.7B, 约2.7G',1,0,0),
    ('asr','local','whisper','whisper-small','onnx-community/whisper-small','auto','Whisper Small - 快速轻量, 250M',1,0,0),
    -- 阿里云 FunASR 实时（Feature 2 seed；is_streaming=0 走 chunk 路径；secret_key 用户填）
    ('asr','aliyun','Fun-ASR','fun-asr-2025-11-07','wss://dashscope.aliyuncs.com/api-ws/v1/inference','auto','阿里云百炼 FunASR 实时（DashScope key 填 secret_key）',0,0,0);
```

LLM seed（原 category=vendor 迁移到 provider；category=模型系列；列序 `domain,provider,category,model_name,source,description,is_thinking,is_local,is_enabled`）：

```sql
INSERT OR IGNORE INTO models (domain, provider, category, model_name, source, description, is_thinking, is_local, is_enabled)
VALUES
    ('llm','deepseek','deepseek','deepseek-v4-flash','https://api.deepseek.com/','DeepSeek V4 Flash（思考模型，需关闭 thinking）',1,0,0),
    ('llm','aliyun','deepseek','deepseek-v4-flash','https://dashscope.aliyuncs.com/compatible-mode/v1','DeepSeek V4 Flash 经 DashScope（思考模型）',1,0,0),
    ('llm','bigmodel','glm','glm-4-flashx','https://open.bigmodel.cn/api/paas/v4','GLM-4 FlashX（非思考）',0,0,0),
    ('llm','bigmodel','glm','glm-4.5-flash','https://open.bigmodel.cn/api/paas/v4','GLM-4.5 Flash（思考模型，需关闭 thinking）',1,0,0),
    -- Feature 1：阿里云 Qwen 原生（DashScope OpenAI 兼容端点）
    ('llm','aliyun','qwen','qwen-plus','https://dashscope.aliyuncs.com/compatible-mode/v1','Qwen Plus（非思考）',0,0,0),
    ('llm','aliyun','qwen','qwen-turbo','https://dashscope.aliyuncs.com/compatible-mode/v1','Qwen Turbo（非思考，快）',0,0,0);
```

- [x] **Step 1.2：改 `parse_model_spec` + `ModelSpec`（infra/db.rs）**

替换 `ModelSpec` enum 与 `parse_model_spec`、`impl ModelSpec`（删除旧 `Local`/`Category`/`NameOnly` 与 `.name()`，改为 `Full`/`NameOnly` 与 `.model_name()`）：

```rust
/// 模型选择规格，统一 `asr_engine` 和 `polish_llm` 的 3-part 格式
/// `{provider}:{category}:{model_name}`。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelSpec<'a> {
    /// `{provider}:{category}:{model_name}` 三段精确匹配
    Full { provider: &'a str, category: &'a str, model_name: &'a str },
    /// 裸 `{model_name}`：仅全局默认 fallback 用（跨 provider/category 搜 name，优先 local）
    NameOnly(&'a str),
}

/// 解析 3-part 规格字符串。
/// - 2 个冒号（3 段）→ Full
/// - 0 冒号 → NameOnly
/// - 1 冒号（旧 2-part 格式）→ warn + 按 NameOnly 兜底
pub fn parse_model_spec(spec: &str) -> ModelSpec<'_> {
    let parts: Vec<&str> = spec.split(':').collect();
    match parts.len() {
        3 => ModelSpec::Full { provider: parts[0], category: parts[1], model_name: parts[2] },
        1 => ModelSpec::NameOnly(parts[0]),
        _ => {
            log::warn!(
                "模型 spec '{}' 非合法 3-part '{{provider}}:{{category}}:{{model_name}}'，按裸名兜底",
                spec
            );
            ModelSpec::NameOnly(spec)
        }
    }
}

impl<'a> ModelSpec<'a> {
    /// 返回 model_name（去掉 provider:/category: 前缀）。
    pub fn model_name(&self) -> &'a str {
        match self {
            ModelSpec::Full { model_name, .. } | ModelSpec::NameOnly(model_name) => model_name,
        }
    }
}
```

- [x] **Step 1.3：改 load 查询（infra/db.rs）**

`load_models_at`：SELECT 列 `name`→`model_name`，新增 `provider`；路由按 `(provider, category)`——`provider='aliyun'` 入 `asr.aliyun`，其余按本地 category。改 AsrConfig 的 AsrSection 需先加字段（见 Step 1.5，但本步 db.rs 只读；AsrSection 字段在 infra/db.rs 定义）。先改 SELECT + tuple + 路由：

```rust
fn load_models_at(conn: &Connection) -> Result<AsrConfig> {
    let mut stmt = conn.prepare(
        "SELECT provider, category, model_name, source, language, description, secret_key, is_local, is_enabled, is_streaming
         FROM models WHERE domain='asr' AND is_enabled = 1",
    )?;
    let rows: Vec<(String, String, String, String, String, String, String, i32, i32, i32)> = stmt
        .query_map([], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?,
                row.get(5)?, row.get(6)?, row.get(7)?, row.get(8)?, row.get(9)?))
        })?
        .filter_map(|r| r.ok())
        .collect();

    let mut asr = AsrSection {
        whisper: None, sensevoice: None, paraformer: None,
        qwen3_asr: None, zipformer: None, aliyun: None,
    };
    for (provider, category, model_name, source, language, description, secret_key, is_local, is_enabled, is_streaming) in rows {
        let entry = ModelEntry { source, language, description, secret_key,
            is_local: is_local != 0, is_enabled: is_enabled != 0, is_streaming: is_streaming != 0 };
        let map: &mut Option<HashMap<String, ModelEntry>> = match (provider.as_str(), category.as_str()) {
            ("aliyun", _) => &mut asr.aliyun,
            (_, "whisper") => &mut asr.whisper,
            (_, "sensevoice") => &mut asr.sensevoice,
            (_, "paraformer") => &mut asr.paraformer,
            (_, "qwen3-asr") => &mut asr.qwen3_asr,
            (_, "zipformer") => &mut asr.zipformer,
            _ => continue,
        };
        map.get_or_insert_with(HashMap::new).insert(model_name, entry);
    }
    Ok(AsrConfig { asr })
}
```

`load_llm_model_at`：3 字段查询；NameOnly 兜底跨 provider 搜、优先 local：

```rust
fn load_llm_model_at(conn: &Connection, spec: &str) -> Result<Option<CompatibleLlmConfig>> {
    let parsed = parse_model_spec(spec);
    let row = match parsed {
        ModelSpec::Full { provider, category, model_name } => {
            let mut stmt = conn.prepare(
                "SELECT source, secret_key, is_thinking, is_local, is_enabled
                 FROM models
                 WHERE domain='llm' AND provider=?1 AND category=?2 AND model_name=?3 AND is_enabled = 1",
            )?;
            let mut rows = stmt.query_map(params![provider, category, model_name], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?,
                    row.get::<_, i32>(2)?, row.get::<_, i32>(3)?, row.get::<_, i32>(4)?))
            })?;
            rows.next().transpose()?
        }
        ModelSpec::NameOnly(model_name) => {
            let mut stmt = conn.prepare(
                "SELECT source, secret_key, is_thinking, is_local, is_enabled
                 FROM models
                 WHERE domain='llm' AND model_name=?1 AND is_enabled = 1
                 ORDER BY is_local DESC",
            )?;
            let mut rows = stmt.query_map(params![model_name], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?,
                    row.get::<_, i32>(2)?, row.get::<_, i32>(3)?, row.get::<_, i32>(4)?))
            })?;
            rows.next().transpose()?
        }
    };
    let model_name = parsed.model_name();
    Ok(row.map(|(source, secret_key, is_thinking, is_local, is_enabled)| CompatibleLlmConfig {
        provider: match parsed { ModelSpec::Full { provider, .. } => provider.to_string(), _ => String::new() },
        model: model_name.to_string(),
        base_url: source,
        secret_key,
        is_thinking: is_thinking != 0,
        is_local: is_local != 0,
        is_enabled: is_enabled != 0,
    }))
}
```

`list_llm_models_at`：SELECT 加 `provider`；`LlmModelInfo` 字段 `name`→`model_name`：

```rust
pub struct LlmModelInfo { pub model_name: String, pub category: String, pub is_local: bool }
// SELECT provider, category, model_name, is_local ... ORDER BY is_local DESC, category
// 注意：排序键 category 字母序保留；provider 不进 LlmModelInfo（仅 name/category/is_local 用于菜单）
```
> 排序与显示若需 provider 前缀，可后续加；当前菜单项 label 已含 category。保持最小改动：`LlmModelInfo` 只换字段名。

- [x] **Step 1.4：改 `AsrSection`（infra/db.rs）加 `aliyun` 字段**

```rust
pub struct AsrSection {
    pub whisper: Option<HashMap<String, ModelEntry>>,
    pub sensevoice: Option<HashMap<String, ModelEntry>>,
    #[serde(default)] pub paraformer: Option<HashMap<String, ModelEntry>>,
    #[serde(default, rename = "qwen3-asr")] pub qwen3_asr: Option<HashMap<String, ModelEntry>>,
    #[serde(default)] pub zipformer: Option<HashMap<String, ModelEntry>>,
    #[serde(default)] pub aliyun: Option<HashMap<String, ModelEntry>>,
}
```

- [x] **Step 1.5：改 infra/db.rs 测试**

更新 `tests` 模块：`init_sql_is_idempotent` 的 ASR 计数（原 8 → 现 9，含 Fun-ASR）；`seed_then_load_round_trips` 用 `model_name` 字段访问；`test_load_llm_model` 全改 3-part spec（`deepseek:deepseek:deepseek-v4-flash`、`aliyun:deepseek:deepseek-v4-flash`、`bigmodel:glm:glm-4-flashx` 等）+ 新增 `aliyun:qwen:qwen-plus` 命中断言；`parse_model_spec_variants`/`model_spec_*` 改 Full/NameOnly；`list_llm_models_*` 适配新 category（bigmodel→glm, deepseek→deepseek）与 `.model_name`。

关键新断言（示例）：
```rust
let qwen = load_llm_model_at(&conn, "aliyun:qwen:qwen-plus").unwrap().unwrap();
assert_eq!(qwen.provider, "aliyun");
assert_eq!(qwen.model, "qwen-plus");
assert_eq!(qwen.base_url, "https://dashscope.aliyuncs.com/compatible-mode/v1");
// 同名跨 provider
let ds_local = load_llm_model_at(&conn, "deepseek:deepseek:deepseek-v4-flash").unwrap().unwrap();
assert_eq!(ds_local.base_url, "https://api.deepseek.com/");
let ds_aliyun = load_llm_model_at(&conn, "aliyun:deepseek:deepseek-v4-flash").unwrap().unwrap();
assert_eq!(ds_aliyun.base_url, "https://dashscope.aliyuncs.com/compatible-mode/v1");
```

- [x] **Step 1.6：改 `asr/config.rs` — EngineCategory + resolver + pick/list/loads**

加 `EngineCategory::Aliyun` 变体；新增 provider 感知 category 解析；`AsrSection` 由 infra 重导出（已含 aliyun）。改：

```rust
pub enum EngineCategory { Whisper, SenseVoice, Paraformer, Qwen3Asr, Zipformer, Aliyun }

/// provider + category → EngineCategory。
/// provider='aliyun' → Aliyun（云）；其余按 category 字符串映射本地族。
fn resolve_category(provider: &str, category: &str) -> Option<EngineCategory> {
    if provider.eq_ignore_ascii_case("aliyun") {
        return Some(EngineCategory::Aliyun);
    }
    engine_category_from_str(category)
}
// engine_category_from_str 保持 5 本地族（不再返回 aliyun，因 aliyun 走 provider 分支）
```

`all_sections` / `list_engines` 的 sections 数组加 `(cfg.asr.aliyun.as_ref(), EngineCategory::Aliyun)`（数组长度 5→6）。`pick_entry` 加 `EngineCategory::Aliyun => cfg.asr.aliyun.as_ref()` 臂。

`resolve_engine_in_config` 改：
```rust
pub fn resolve_engine_in_config<'a, 'b>(cfg: &'a AsrConfig, spec: &'b str)
    -> Option<(EngineCategory, &'b str, &'a ModelEntry)>
{
    match parse_model_spec(spec) {
        ModelSpec::Full { provider, category, model_name } => {
            let cat = resolve_category(provider, category)?;
            pick_entry(cfg, cat, model_name).map(|e| (cat, model_name, e))
        }
        ModelSpec::NameOnly(model_name) => {
            for (section, cat) in all_sections(cfg) {
                if let Some(map) = section {
                    if let Some(e) = map.get(model_name) { return Some((cat, model_name, e)); }
                }
            }
            None
        }
    }
}
```

`category_label` 加 `Aliyun => "aliyun"`。`fallback_engine` 的硬构造 `ModelEntry` 不变（zipformer 兜底）。

- [x] **Step 1.7：改 asr/config.rs 测试**

`make_entry` 不变。`cfg_with_zipformer` 加 `aliyun: None`。新增：`resolve_engine_in_config("aliyun:Fun-ASR:fun-asr-2025-11-07")`（需构造 aliyun section）→ `(EngineCategory::Aliyun, "fun-asr-2025-11-07", entry)`；`pick_entry(&cfg, Aliyun, ...)`；`engine_category_from_str("aliyun")` 仍 None（aliyun 走 provider 分支，断言保持）；`resolve_unknown_category_prefix_returns_none` 改用合法 3-part（如 `whisper:zipformer-small-ctc` 在 whisper section 缺 → None）。

- [x] **Step 1.8：改 desktop/runtime_config.rs（.name → .model_name）**

`build_llm_options` / `build_llm_options_public` / `build_asr_options_public` 中所有 `m.name` → `m.model_name`；`LlmOption` 结构体的 `name` 字段（前端契约）可保留键名 `name`（前端用），但其**值**取自 `m.model_name`。`parse_model_spec(current).name()` → `.model_name()`。

> grep 全 workspace 确认无遗漏：`rg "\.name\b" crates/desktop/src/runtime_config.rs`、`rg "parse_model_spec" crates/`。

- [x] **Step 1.9：改 cli/server（grep 命中点）**

`rg "\.name\b|parse_model_spec|asr\.active" crates/cli/src crates/server/src`。预期：cli `show_config`/`do_transcribe` 若硬编码 2-part spec 则改 3-part；server `resolve_active_engine` 调用不变（已接 spec 字符串）。按命中点最小适配。

- [x] **Step 1.10：编译 + 测试**

```bash
cargo check --workspace --all-targets
cargo test -p octopus-infra
cargo test -p octopus-asr-local
```
预期：全绿。若 `is_streaming_engine` / `resolve_active_engine` 报错，确认 fallback 路径用 NameOnly 兜底正确。

- [x] **Step 1.11：删本地 DB 验证 + 提交**

```bash
rm -f ~/.octopus/octopus.db   # 删库重建（用户确认历史无所谓）
cargo test -p octopus-infra   # 触发 ensure_db 重建新 schema
sqlite3 ~/.octopus/octopus.db "SELECT domain,provider,category,model_name FROM models ORDER BY domain,provider,category;"
```
预期：ASR 9 行（8 local + 1 aliyun Fun-ASR）、LLM 6 行（deepseek/deepseek、aliyun/deepseek、bigmodel/glm×2、aliyun/qwen×2）。

```bash
git add -A
git commit -m "refactor(db): 统一模型 taxonomy（provider 列 + name→model_name + 3-part spec）

- models 表加 provider（local/aliyun/...），name 重命名为 model_name，唯一键改 (domain,provider,category,model_name)
- parse_model_spec 改 3-part {provider}:{category}:{model_name}，asr_engine 与 polish_llm 统一
- EngineCategory::Aliyun + AsrSection.aliyun；load 查询 3 字段
- seed：ASR 8→9（+aliyun Fun-ASR），LLM 4→6（category 拆模型系列 +aliyun qwen×2）
- 删库重建，无 ALTER 迁移"
```

---

## Task 2: DashscopeEngine + 云引擎路由 + cargo feature

**Files:** `crates/desktop/Cargo.toml`、`crates/desktop/src/engine_dashscope.rs`（新）、`crates/desktop/src/main.rs`

- [x] **Step 2.1：Cargo.toml 加 `dashscope` feature**

`crates/desktop/Cargo.toml`：
```toml
tokio-tungstenite = { version = "0.24", optional = true, features = ["native-tls"] }
uuid = { version = "1", features = ["v4"], optional = true }   # 若已有非可选 uuid 则复用
futures-util = { version = "0.3", optional = true }
serde_json = "1"   # 若已有则跳过

[features]
dashscope = ["tokio-tungstenite", "uuid", "futures-util"]
```
> TLS feature 与 `engine_ws.rs`（remote-ws）保持一致——先 `rg "tokio-tungstenite" crates/desktop/Cargo.toml` 看 remote-ws 用的 TLS 后端（native-tls / rustls），dashscope 沿用同一后端避免双 TLS 链接。

- [x] **Step 2.2：写 `engine_dashscope.rs`（含单测）**

先写单测（PCM 转换 + run-task JSON 构造），再实现：

```rust
//! 阿里云百炼 FunASR Realtime WebSocket ASR engine。
//! 集成点：桌面分块 TranscriptionEngine（每 VAD 段开一条 WS 跑 DashScope duplex 协议）。
//! 协议：run-task → task-started → 二进制 PCM 帧 → result-generated → finish-task。

use anyhow::{anyhow, Context, Result};
use async_trait::async_trait;
use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use std::time::Duration;
use tokio_tungstenite::{connect_async, tungstenite::Message};

use crate::engine::TranscriptionEngine;

pub struct DashscopeEngine;

impl DashscopeEngine {
    pub fn new() -> Self { Self }
}

#[async_trait]
impl TranscriptionEngine for DashscopeEngine {
    async fn transcribe(&self, samples: &[f32], language: &str, engine: &str) -> Result<String> {
        let model_name = octopus_infra::db::parse_model_spec(engine).model_name().to_string();

        let cfg = octopus_asr_local::config::load_config()?;
        let entry = cfg.asr.aliyun.as_ref()
            .and_then(|m| m.get(model_name.as_str()))
            .with_context(|| format!("aliyun ASR 模型 '{}' 未在 DB 配置", model_name))?;
        if entry.secret_key.is_empty() {
            anyhow::bail!(
                "aliyun ASR 模型 '{}' 的 secret_key(DashScope API Key) 为空，请 sqlite3 填写：\n\
                 sqlite3 ~/.octopus/octopus.db \"UPDATE models SET secret_key='sk-...' WHERE model_name='{}'\"",
                model_name, model_name
            );
        }

        let endpoint = entry.source.clone();
        let key = entry.secret_key.clone();
        let samples = samples.to_vec();
        let language = language.to_string();

        tokio::time::timeout(Duration::from_secs(8), async move {
            run_session(&endpoint, &key, &model_name, &samples, &language).await
        })
        .await
        .map_err(|_| anyhow!("dashscope transcription timeout (8s)"))?
    }

    async fn health_check(&self) -> bool { true }
}

async fn run_session(endpoint: &str, key: &str, model: &str, samples: &[f32], language: &str) -> Result<String> {
    let (mut ws, _resp) = connect_async(endpoint).await.context("dashscope WS 连接失败")?;

    let task_id = uuid::Uuid::new_v4().to_string();
    let lang_hints = if language.is_empty() || language == "auto" {
        json!(["zh", "en"])
    } else {
        json!([language])
    };
    let payload = json!({
        "model": model, "task_group": "audio", "task": "asr", "function": "recognition",
        "parameters": { "format": "pcm", "sample_rate": 16000, "language_hints": lang_hints }
    });
    let run_task = json!({
        "header": { "action": "run-task", "task_id": task_id, "streaming": "duplex" },
        "payload": payload, "input": {}
    });
    ws.send(Message::Text(run_task.to_string())).await?;

    // 发 PCM（f32→s16le，分块 200ms）
    let pcm = samples_to_pcm_s16le(samples);
    const CHUNK_BYTES: usize = 6400; // 3200 样本 × 2 = 200ms @16kHz
    for chunk in pcm.chunks(CHUNK_BYTES) {
        ws.send(Message::binary(chunk.to_vec())).await?;
    }

    // finish-task
    let finish = json!({
        "header": { "action": "finish-task", "task_id": task_id, "streaming": "duplex" },
        "payload": payload, "input": {}
    });
    ws.send(Message::Text(finish.to_string())).await?;

    // 收 result-generated，取最终文本
    let mut text = String::new();
    while let Some(msg) = ws.next().await {
        let msg = msg.context("dashscope WS 读")?;
        if let Message::Text(t) = msg {
            let v: Value = match serde_json::from_str(&t) { Ok(v) => v, Err(_) => continue };
            match v["header"]["event"].as_str() {
                Some("result-generated") => {
                    if let Some(t) = v["payload"]["output"]["sentence"]["text"].as_str() {
                        text = t.to_string(); // 取最新（最终）句文本
                    }
                }
                Some("task-finished") => break,
                Some("task-failed") => {
                    let m = v["header"]["error_message"].as_str().unwrap_or("unknown");
                    anyhow::bail!("dashscope task-failed: {}", m);
                }
                _ => {}
            }
        }
    }
    Ok(text)
}

/// f32[-1,1] 样本 → s16le PCM 字节。
fn samples_to_pcm_s16le(samples: &[f32]) -> Vec<u8> {
    let mut out = Vec::with_capacity(samples.len() * 2);
    for &s in samples {
        let v = (s.clamp(-1.0, 1.0) * 32767.0).round() as i16;
        out.extend_from_slice(&v.to_le_bytes());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pcm_conversion_roundtrip_mono() {
        let samples = vec![0.0, 1.0, -1.0, 0.5];
        let pcm = samples_to_pcm_s16le(&samples);
        assert_eq!(pcm.len(), 8);
        // 0.0 → 0
        assert_eq!(i16::from_le_bytes([pcm[0], pcm[1]]), 0);
        // 1.0 → 32767
        assert_eq!(i16::from_le_bytes([pcm[2], pcm[3]]), 32767);
        // -1.0 → -32768（clamp(-1)*32767 round → -32768）
        assert!(i16::from_le_bytes([pcm[4], pcm[5]]) < 0);
    }

    #[test]
    fn run_task_json_has_required_fields() {
        let task_id = "abc";
        let payload = json!({"model":"fun-asr-2025-11-07","task_group":"audio","task":"asr","function":"recognition","parameters":{"format":"pcm","sample_rate":16000,"language_hints":["zh","en"]}});
        let run_task = json!({"header":{"action":"run-task","task_id":task_id,"streaming":"duplex"},"payload":payload,"input":{}});
        assert_eq!(run_task["header"]["action"], "run-task");
        assert_eq!(run_task["payload"]["model"], "fun-asr-2025-11-07");
        assert_eq!(run_task["payload"]["parameters"]["format"], "pcm");
    }
}
```

> ⚠️ DashScope 三个模型（Fun-ASR / paraformer-realtime-v2 / qwen-asr-realtime）事件结构略有差异——实现后用真实 key 跑 `cargo test -p octopus-desktop -- --ignored` 的集成测试验证；若某模型字段不同，按其官方文档在 `run_session` 内按 `model` 名分派。当前实现以 Fun-ASR 为主（用户首选用例）。

- [x] **Step 2.3：main.rs 路由 + mod 声明**

`crates/desktop/src/main.rs` 顶部加：
```rust
#[cfg(feature = "dashscope")]
mod engine_dashscope;
```

engine 构建处（`match config.engine_mode.as_str()` 之前）插入云路由：
```rust
use octopus_asr_local::config::{resolve_active_engine, EngineCategory};

// 云引擎优先：asr_engine 解析为 aliyun → DashscopeEngine
#[cfg(feature = "dashscope")]
let cloud = resolve_active_engine(&config.asr_engine)
    .map(|r| r.category == EngineCategory::Aliyun)
    .unwrap_or(false);

let engine: Arc<dyn TranscriptionEngine> = {
    #[cfg(feature = "dashscope")]
    if cloud {
        info!("ASR 引擎：阿里云 Dashscope（cloud）");
        Arc::new(engine_dashscope::DashscopeEngine::new())
    } else {
        build_local_engine(&config, engine_manager.clone())
    }
    #[cfg(not(feature = "dashscope"))]
    { build_local_engine(&config, engine_manager.clone()) }
};
```
把原 `match config.engine_mode.as_str() { ... }` 抽成 `fn build_local_engine(config, engine_manager) -> Arc<dyn TranscriptionEngine>`（保持 embedded/websocket/grpc 三臂不变）。

> 注意 `resolve_active_engine` 在 main.rs setup 中已被调用预热（embedded 分支）。这里复用其结果即可，避免重复解析——可把预热处的 `resolved_model` / category 一并用于 cloud 判定。

- [x] **Step 2.4：编译 + 测试**

```bash
cargo check -p octopus-desktop --features dashscope
cargo test -p octopus-desktop --features dashscope
cargo check --workspace   # 确认默认（不开 dashscope）仍绿
```

- [x] **Step 2.5：提交**

```bash
git add -A
git commit -m "feat(desktop): 接入阿里云 FunASR Realtime WS engine

- 新 engine_dashscope.rs：impl TranscriptionEngine，每段开 WS 跑 DashScope duplex 协议（run-task→PCM→result-generated→finish-task），bearer 鉴权复用 DB secret_key
- main.rs：asr_engine 解析为 EngineCategory::Aliyun 时建 DashscopeEngine，否则本地
- Cargo feature dashscope（tokio-tungstenite+uuid+futures-util），默认不开"
```

---

## Task 3: 文档同步（CLAUDE.md 强制）

**Files:** `docs/configuration.md`、`docs/architecture.md`

- [x] **Step 3.1：configuration.md**

- `asr_engine` / `polish_llm` 字段说明改 3-part `{provider}:{category}:{model_name}`，给 DashScope 示例：
  - `asr_engine: "aliyun:Fun-ASR:fun-asr-2025-11-07"`
  - `polish_llm: "aliyun:qwen:qwen-plus"`
- 加 `models` 表 `provider`/`model_name` 字段说明 + provider×category taxonomy 表。
- 加「阿里云接入」小节：DashScope key 填法（`sqlite3 ~/.octopus/octopus.db "UPDATE models SET secret_key='sk-...' WHERE ..."`）+ `dashscope` feature 开启（`cargo build --features dashscope`）+ 删库重建提示。
- 删旧 2-part / `local:` 前缀 / `is_active` 残留描述。

- [x] **Step 3.2：architecture.md**

- 「模型管理」段：models 表 schema（provider / model_name）、provider×category taxonomy、引擎选择 `resolve_active_engine` → `EngineCategory::Aliyun` 路由 DashscopeEngine。
- 云引擎说明（DashscopeEngine 分块路径、is_streaming=0）。

- [x] **Step 3.3：提交**

```bash
git add docs/configuration.md docs/architecture.md
git commit -m "docs: 阿里云云端 API 接入 + provider/model_name taxonomy"
```

---

## 验证（手动 e2e）

删库重建 + 配 key：
```bash
rm -f ~/.octopus/octopus.db
sqlite3 ~/.octopus/octopus.db "UPDATE models SET secret_key='sk-你的dashscope-key' WHERE domain='asr' AND model_name='fun-asr-2025-11-07';"
```
`~/.octopus/config.yaml`：
```yaml
asr_engine: "aliyun:Fun-ASR:fun-asr-2025-11-07"
polish_llm: "aliyun:qwen:qwen-plus"
polish_mode: 2
```
```bash
cargo run -p octopus-desktop --features dashscope
```
预期：录音 → FunASR 识别出文本（log 见 `ASR 引擎：阿里云 Dashscope`）；停顿 → Qwen 润色生效。

---

## Spec Coverage

| Spec section | Task |
|---|---|
| §3 数据模型（provider/model_name/unique/删库） | Task 1.1, 1.11 |
| §3.3 parse_model_spec 3-part | Task 1.2, 1.5 |
| §4 Feature 1 LLM（零代码 + qwen seed） | Task 1.1（seed）+ 1.5（测试）+ 3.1（文档） |
| §5.1 is_streaming=0 chunk 路径 | Task 1.1（seed is_streaming=0） |
| §5.2 EngineCategory::Aliyun + AsrSection.aliyun | Task 1.4, 1.6 |
| §5.3 DashscopeEngine | Task 2.2 |
| §5.4 main.rs 路由 | Task 2.3 |
| §5.5 cargo feature | Task 2.1 |
| §5.6 Fun-ASR seed | Task 1.1 |
| §6 波及面（runtime_config/cli/server） | Task 1.8, 1.9 |
| §9 文档 | Task 3 |


---

## `denoise-deepfilternet3-integration.md`

# DeepFilterNet3 原生降噪整合 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 将 DeepFilterNet3（官方 libDF v0.5.6 + tract 0.19）作为 `denoise_mode=2` 整合进采集层，与现有 RNNoise（mode=1）并存，`DenoiseProcessor` 对外接口不变。

**Architecture:** `DenoiseProcessor` 重构为 trait 后端分发器——新增 `FrameDenoise` trait（`[-1,1]` 契约），`RnnoiseBackend` 包装现有 nnnoiseless，`Df3Backend` 包装 libDF `DfTract`（`unsafe impl Send/Sync` 照 VST3）。ndarray 0.15（libDF）与 0.17（asr）靠 trait 的 slice 边界隔离。spike 已验证 v0.5.6+tract 0.19 在 native gain=0.958（不压语音）、RTF=0.015。

**Tech Stack:** Rust、libDF(deep_filter v0.5.6)、tract 0.19、ndarray 0.15/0.17 共存、nnnoiseless、serde(config)。

参考 spec：`docs/superpowers/specs/2026-06-17-denoise-deepfilternet3-integration-design.md`。

---

## File Structure

| 文件 | 责任 | 操作 |
|---|---|---|
| `crates/asr/Cargo.toml` | 加 `df` git 依赖 + `ndarray_015` rename | 改 |
| `crates/asr/src/denoise.rs` | `FrameDenoise` trait + `DenoiseMode` + `RnnoiseBackend` + `Df3Backend` + `DenoiseProcessor` 重构 + 测试 | 改 |
| `crates/infra/src/config.rs` | `denoise_mode: Option<u8>` + `effective_denoise_mode()` + 测试 | 改 |
| `crates/desktop/src/audio.rs` | `denoise_enabled` → `effective_denoise_mode()`（:98 读、:211 构造） | 改 |
| `docs/configuration.md`、`docs/architecture.md` | denoise_mode 说明 | 改 |

**隔离边界**：`FrameDenoise::process_frame(pcm: &[f32;480], out: &mut [f32;480])` 只用原生 slice，绝不暴露 ndarray——asr 的 0.17 与 libDF 的 0.15 在此隔离。`Df3Backend` 内部用 `ndarray_015` 构造 `ArrayView2` 喂 `DfTract::process`。

**测试策略**：现有 `denoise.rs` 测试保持全绿（mode=Rnnoise 回归）；DF3 功能测试加 `#[ignore]`（需加载 7.9MB 模型，慢，手动 `cargo test -- --ignored`）；Send 正确性由 `audio.rs:312` 编译期断言守护。

---

## Task 1: 加 libDF 依赖 + ndarray 0.15 隔离 + time patch

**Files:**
- Modify: `crates/asr/Cargo.toml`
- 注：`Cargo.lock` 由 cargo update 生成但**仓库不跟踪**（既有策略），不进 commit。

**背景**：tract 0.19 间接拉 `time`，默认解析到 0.3.28，在 rustc 1.96 下 E0282 编译失败。必须先升 time 再 check。ndarray 0.15 与现有 0.17 共存靠 package rename。

- [x] **Step 1: 改 `crates/asr/Cargo.toml`，加 df 与 ndarray_015**

在 `nnnoiseless = { version = "0.5", default-features = false }`（约 :25）下方插入：

```toml
# DeepFilterNet3 原生降噪（libDF v0.5.6 + tract 0.19，spec 2026-06-17）。
# ndarray 0.15（libDF 版本）与上方 0.17（ort/asr）共存：rename 隔离，Df3Backend 边界转换。
ndarray_015 = { package = "ndarray", version = "0.15", default-features = false }
# df URL：原设计写 fork tryternity，实测该 fork 无 tag（git ls-remote --tags 空），
# 改用上游官方 Rikorose/DeepFilterNet tag v0.5.6（commit 978576aa，与 fork 同 commit 等价）。
df = { git = "https://github.com/Rikorose/DeepFilterNet.git", tag = "v0.5.6", package = "deep_filter", default-features = false, features = ["tract", "default-model", "transforms"] }
```

- [x] **Step 2: time 版本检查（通常无需手动 patch）**

tract 0.19 间接拉 `time`，rustc 1.96 下若解析到 `0.3.28` 会 E0282 失败。但 octopus workspace 已有
`tauri → plist` 链要求 `time ^0.3.47`（Cargo.lock 解析到 `0.3.49`），**已远高于规避 E0282 的 0.3.35
阈值**，故引入 df 后通常无需任何手动 time patch。

先检查（仓库根）:
```bash
grep -A1 '^name = "time"' Cargo.lock
```
Expected: `version = "0.3.47"` 或更高（≥0.3.35 即可）。

**仅当**解析到 `<0.3.35` 时（如未来 tauri/plist 链变动）才需手动钉版本:
```bash
cargo update -p time --precise 0.3.36
```
Expected: `Updating time v0.3.x -> v0.3.36`（或 "Already up to date" 若已 ≥0.3.36）。若无输出报错
`Package does not feature`，先 `cargo fetch` 再重试。

- [x] **Step 3: 验证 asr 编译（首次拉 df + 编译 tract，约 1–3 分钟）**

Run:
```bash
cargo check -p octopus-asr-local
```
Expected: `Finished` 无错误。
诊断：
- 若报 `time ... E0282 type annotations needed` → time 未升级，回 Step 2。
- 若报 ndarray 版本冲突 → 确认 `ndarray_015` 用了 `package = "ndarray"` rename。
- 若报 df git 拉取失败 → 确认网络/上游 tag `v0.5.6` 存在（`git ls-remote --tags https://github.com/Rikorose/DeepFilterNet.git v0.5.6`）。注意：**用上游 Rikorose 不是 fork tryternity**（后者无 tag）。

- [x] **Step 4: 提交**

> 注：仓库不跟踪 `Cargo.lock`（既有策略），故只 add `Cargo.toml`。

```bash
git add crates/asr/Cargo.toml
git commit -m "build(asr): 加 libDF v0.5.6 依赖 + ndarray_0.15 隔离 + time patch"
```

---

## Task 2: config 加 denoise_mode + effective_denoise_mode()

> ⚠️ **实施修正（2026-06-17 合并后）**：本 Task 原设计 `denoise_mode: Option<u8>` + `effective_denoise_mode()`（向后兼容旧 `denoise_enabled`）。DF3 分支合并时与 main 工具栏的 `u8` 版本语义冲突（`git merge-tree` 未报文本冲突、却留下重复字段），经决策**统一为 main 的 `denoise_mode: u8`**——`Option<u8>` 字段与 `effective_denoise_mode()` **未保留**，旧 `denoise_enabled` 字段彻底删除。下方 Step 1–5 记录的是原设计步骤（历史执行轨迹）；最终落盘代码见 `crates/infra/src/config.rs`（`denoise_mode: u8` + `default_denoise_mode()=1`）。完整决策动机详见 spec §8「实施修正（2026-06-17 合并后）」。

**Files:**
- Modify: `crates/infra/src/config.rs:143-145`（字段）、`config.rs`（impl 块加方法）、`config.rs:230`（Default）、`config.rs:324-332`（测试）

**背景**：`denoise_enabled: bool`（默认 true）在 `infra/src/config.rs:144`。新增 `denoise_mode: Option<u8>`（None=未配置）。`effective_denoise_mode()`：mode 显式优先，否则 `denoise_enabled` 映射（true→1, false→0），实现旧配置向后兼容。

- [x] **Step 1: 写失败测试（先于实现）**

在 `crates/infra/src/config.rs` 测试模块（`denoise_enabled_override_from_yaml` 测试附近，约 :330）后追加：

```rust
    #[test]
    fn denoise_mode_explicit_wins() {
        let cfg: AppConfig =
            serde_yaml::from_str("denoise_mode: 2\ndenoise_enabled: false\n").unwrap();
        assert_eq!(cfg.effective_denoise_mode(), 2);
    }

    #[test]
    fn denoise_mode_absent_falls_back_to_enabled() {
        let cfg: AppConfig = serde_yaml::from_str("denoise_enabled: true\n").unwrap();
        assert_eq!(cfg.effective_denoise_mode(), 1);
        let cfg: AppConfig = serde_yaml::from_str("denoise_enabled: false\n").unwrap();
        assert_eq!(cfg.effective_denoise_mode(), 0);
    }

    #[test]
    fn denoise_mode_absent_defaults_to_rnnoise() {
        let cfg: AppConfig = serde_yaml::from_str("").unwrap();
        assert_eq!(cfg.effective_denoise_mode(), 1);
    }
```

- [x] **Step 2: 跑测试确认失败**

Run:
```bash
cargo test -p octopus-infra denoise_mode 2>&1 | tail -15
```
Expected: 编译失败 `no field or method effective_denoise_mode` / `no field denoise_mode`。

- [x] **Step 3: 加字段 + 默认函数**

改 `crates/infra/src/config.rs:143-145`，在 `denoise_enabled` 字段**之后**插入 `denoise_mode`：

```rust
    /// 是否启用 RNNoise 环境降噪（录音送 ASR 前降噪）
    #[serde(default = "default_denoise_enabled")]
    pub denoise_enabled: bool,

    /// 环境降噪模式：0=关闭，1=RNNoise（默认），2=DeepFilterNet3。
    /// None=未配置 → 回退看 denoise_enabled（向后兼容旧配置）。
    #[serde(default)]
    pub denoise_mode: Option<u8>,
```

- [x] **Step 4: 加 effective_denoise_mode() 方法 + Default 初始化**

先定位 AppConfig 的 impl 块与 Default：
```bash
grep -n "impl AppConfig\|impl Default for AppConfig\|denoise_enabled: default_denoise_enabled" crates/infra/src/config.rs
```

在 `impl AppConfig { ... }` 块内（若无 impl 块则新增 `impl AppConfig { ... }`）加方法：

```rust
    /// 解析最终降噪模式（denoise_mode 显式优先，否则 denoise_enabled 映射）。
    /// 0=关闭，1=RNNoise，2=DeepFilterNet3。
    pub fn effective_denoise_mode(&self) -> u8 {
        if let Some(m) = self.denoise_mode {
            return m;
        }
        if self.denoise_enabled {
            1
        } else {
            0
        }
    }
```

在 `impl Default for AppConfig` 的构造体里 `denoise_enabled: default_denoise_enabled(),`（约 :230）下方加：

```rust
            denoise_mode: None,
```

- [x] **Step 5: 跑测试确认通过**

Run:
```bash
cargo test -p octopus-infra denoise 2>&1 | tail -15
```
Expected: `denoise_mode_explicit_wins ... ok`、`denoise_mode_absent_falls_back_to_enabled ... ok`、`denoise_mode_absent_defaults_to_rnnoise ... ok`、现有 `denoise_enabled_*` 仍 ok。

- [x] **Step 6: 提交**

```bash
git add crates/infra/src/config.rs
git commit -m "feat(config): denoise_mode 0/1/2 + effective_denoise_mode 向后兼容"
```

---

## Task 3: FrameDenoise trait + RnnoiseBackend + Df3Backend + DenoiseProcessor 重构

**Files:**
- Modify: `crates/asr/src/denoise.rs`（整体重构，含两后端 + 分发器，保持现有测试绿）

**背景**：`DenoiseProcessor` 从直接持有 nnnoiseless 改为 trait 后端分发。trait 用 `[-1,1]` 契约，PCM_SCALE 下沉到 `RnnoiseBackend`。`Df3Backend`（依赖 Task 1 的 df）一并定义，使 `DenoiseProcessor` 的 mode=Df3 分支可编译。现有测试调 `DenoiseProcessor::new()` → 改 `new(DenoiseMode::Rnnoise)`。

**注意**：本 task 一次写完 trait + 两后端 + processor，否则 `DenoiseProcessor` 引用 `Df3Backend` 无法编译。

- [x] **Step 1: 重写 `crates/asr/src/denoise.rs` 的非测试部分**

替换文件顶部模块文档到 `impl Default for DenoiseProcessor` 之前（含模块文档 + 常量 + 枚举 + trait + 两后端 + DenoiseProcessor 结构 + impl + Default）。新内容：

```rust
//! 环境降噪：可插拔后端（RNNoise / DeepFilterNet3），由 denoise_mode 选择。
//!
//! ## 后端
//! - `RnnoiseBackend`（mode=1）：nnnoiseless（Xiph RNNoise 纯 Rust 移植），内置默认模型。
//! - `Df3Backend`（mode=2）：libDF v0.5.6 + tract 0.19，DeepFilterNet3，48kHz 全频带。
//! - mode=0：无后端（直通）。
//!
//! ## 契约
//! `FrameDenoise::process_frame` 用 `[-1, 1]` 归一化单声道（与 octopus pipeline 一致）。
//! 各后端内部按模型需求转换（RNNoise 转 i16 PCM 等价；DF3 直接喂 [-1,1]）。
//! 帧大小 FRAME_SIZE=480（10ms @48kHz），与 octopus HOP 一致。
//!
//! ## 历史
//! 曾用第三方 dfn3.onnx（压语音 gain≈0.10），已弃用。见
//! `docs/superpowers/specs/2026-06-17-denoise-deepfilternet3-integration-design.md`。

use anyhow::Result;

/// 帧大小（480 样本 = 10ms @48kHz）。
const FRAME_SIZE: usize = 480;

/// 降噪模式。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DenoiseMode {
    Off = 0,
    Rnnoise = 1,
    Df3 = 2,
}

impl DenoiseMode {
    pub fn from_u8(v: u8) -> Self {
        match v {
            0 => Self::Off,
            1 => Self::Rnnoise,
            _ => Self::Df3,
        }
    }
}

/// 单帧（FRAME_SIZE，48k，[-1,1]）降噪后端抽象。
///
/// 仅用原生 slice，不暴露 ndarray——隔离 libDF(ndarray 0.15) 与 asr(ndarray 0.17)。
/// `Send + Sync`：`DenoiseProcessor` 经 `Mutex` 在 SharedAudioState 跨线程（audio.rs:305 断言）。
trait FrameDenoise: Send + Sync {
    fn process_frame(&mut self, pcm: &[f32; FRAME_SIZE], out: &mut [f32; FRAME_SIZE]);
    /// 清状态（会话边界）。各后端自行决定轻量清零 vs 重建。
    fn reset(&mut self);
}

// ── RNNoise 后端 ──

/// nnnoiseless 内部以 i16 PCM 等价值域运算；边界 [-1,1] ↔ PCM 转换在此。
const PCM_SCALE: f32 = 32768.0;

struct RnnoiseBackend {
    denoise: Box<nnnoiseless::DenoiseState<'static>>,
}

impl RnnoiseBackend {
    fn new() -> Self {
        Self {
            denoise: nnnoiseless::DenoiseState::new(),
        }
    }
}

impl FrameDenoise for RnnoiseBackend {
    fn process_frame(&mut self, pcm: &[f32; FRAME_SIZE], out: &mut [f32; FRAME_SIZE]) {
        let pcm_scaled: [f32; FRAME_SIZE] = std::array::from_fn(|i| pcm[i] * PCM_SCALE);
        self.denoise.process_frame(out, &pcm_scaled);
        // nnnoiseless 输出沿用输入值域（i16 PCM 等价），转回 [-1,1]
        for s in out.iter_mut() {
            *s /= PCM_SCALE;
        }
    }
    fn reset(&mut self) {
        self.denoise = nnnoiseless::DenoiseState::new();
    }
}

// ── DeepFilterNet3 后端（libDF v0.5.6 + tract 0.19）──

use df::tract::DfTract;

/// DeepFilterNet3 降噪后端。包装 libDF `DfTract`（48kHz 全频带，内嵌 DeepFilterNet3 模型）。
///
/// `DfTract: !Send`（含 `Arc<dyn RealToComplex>` 无 Send bound）。此处 unsafe impl 仅满足
/// `DenoiseProcessor: Send`（audio.rs:312 断言）的类型约束——实际由 coordinator 单线程串行
/// 访问（audio.rs:94），无跨线程并发。同 VST3 plugin/src/lib.rs:9-11。
pub struct Df3Backend(DfTract);

impl Df3Backend {
    /// 加载内嵌 DeepFilterNet3 模型。失败返回 Err（供懒加载降级，绝不 panic）。
    pub fn new() -> Result<Self> {
        let model = std::panic::catch_unwind(std::panic::AssertUnwindSafe(DfTract::default))
            .map_err(|e| anyhow::anyhow!("DF3 模型加载失败（panic）: {:?}", e))?;
        Ok(Self(model))
    }
}

// 安全性：coordinator 单线程串行访问（audio.rs:94），Mutex 保护，无跨线程并发。
unsafe impl Send for Df3Backend {}
unsafe impl Sync for Df3Backend {}

impl FrameDenoise for Df3Backend {
    fn process_frame(&mut self, pcm: &[f32; FRAME_SIZE], out: &mut [f32; FRAME_SIZE]) {
        // DfTract::process 接 ndarray 0.15 的 ArrayView2/ArrayViewMut2 [ch=1, hop]。
        // 用 ndarray_015（与 libDF 同一 crate 实例）构造；契约 [-1,1]（DfTract 期望归一化）。
        use ndarray_015::{ArrayView2, ArrayViewMut2};
        let noisy = match ArrayView2::from_shape((1, FRAME_SIZE), pcm.as_slice()) {
            Ok(v) => v,
            Err(e) => {
                log::warn!("DF3 frame shape 错误，直通：{:?}", e);
                out.copy_from_slice(pcm);
                return;
            }
        };
        let mut enh = match ArrayViewMut2::from_shape((1, FRAME_SIZE), out.as_mut_slice()) {
            Ok(v) => v,
            Err(e) => {
                log::warn!("DF3 enh shape 错误，直通：{:?}", e);
                return;
            }
        };
        if let Err(e) = self.0.process(noisy, enh.view_mut()) {
            log::warn!("DF3 process 失败，本帧直通：{:?}", e);
        }
    }
    fn reset(&mut self) {
        // DfTract 无轻量状态重置；重建 = 重载模型（仅会话边界调用）。
        match Self::new() {
            Ok(b) => *self = b,
            Err(e) => log::warn!("DF3 reset 重载失败：{:?}", e),
        }
    }
}

// ── DenoiseProcessor（mode 分发器）──

/// 流式降噪处理器。对外接口与旧 RNNoise-only 实现一致（new/reset/process_samples/flush）。
pub struct DenoiseProcessor {
    mode: DenoiseMode,
    backend: Option<Box<dyn FrameDenoise>>, // None = 直通(mode=0 或加载失败降级)
    in_buf: Vec<f32>,  // 48k [-1,1] 累积输入
    out_buf: Vec<f32>, // 48k [-1,1] 已降噪待输出
    df_pending: bool,  // DF3 懒加载：mode=Df3 但尚未首次 process
}

impl DenoiseProcessor {
    /// 按 mode 创建降噪器。mode=Df3 时延迟到首次 process_samples 加载（避免 new 热路径开销）。
    pub fn new(mode: DenoiseMode) -> Result<Self> {
        let mut p = Self {
            mode,
            backend: None,
            in_buf: Vec::with_capacity(FRAME_SIZE),
            out_buf: Vec::new(),
            df_pending: false,
        };
        match mode {
            DenoiseMode::Off => {}
            DenoiseMode::Rnnoise => {
                p.backend = Some(Box::new(RnnoiseBackend::new()));
            }
            DenoiseMode::Df3 => {
                p.df_pending = true; // 懒加载
            }
        }
        Ok(p)
    }

    /// 全状态清零（重建后端）。DF3 reset 重载模型——仅会话边界调用。
    pub fn reset(&mut self) {
        self.in_buf.clear();
        self.out_buf.clear();
        match self.mode {
            DenoiseMode::Off => self.backend = None,
            DenoiseMode::Rnnoise => self.backend = Some(Box::new(RnnoiseBackend::new())),
            DenoiseMode::Df3 => {
                self.backend = match Df3Backend::new() {
                    Ok(b) => Some(Box::new(b)),
                    Err(e) => {
                        log::warn!("DF3 reset 重建失败，降级直通：{:?}", e);
                        None
                    }
                };
                self.df_pending = false;
            }
        }
    }

    /// 增量处理 48k [-1,1] 样本：累积到 FRAME_SIZE，逐帧降噪，返回已降噪样本。
    pub fn process_samples(&mut self, samples: &[f32]) -> Vec<f32> {
        if self.df_pending {
            self.backend = match Df3Backend::new() {
                Ok(b) => Some(Box::new(b)),
                Err(e) => {
                    log::warn!("DF3 模型加载失败，降级直通（不阻断录音）：{:?}", e);
                    None
                }
            };
            self.df_pending = false;
        }
        self.in_buf.extend_from_slice(samples);
        let mut out_frame = [0.0f32; FRAME_SIZE];
        while self.in_buf.len() >= FRAME_SIZE {
            let frame: Vec<f32> = self.in_buf.drain(..FRAME_SIZE).collect();
            let pcm: [f32; FRAME_SIZE] = std::array::from_fn(|i| frame[i]);
            if let Some(b) = self.backend.as_mut() {
                b.process_frame(&pcm, &mut out_frame);
                for &s in &out_frame {
                    self.out_buf.push(s);
                }
            } else {
                for &s in &pcm {
                    self.out_buf.push(s); // 直通
                }
            }
        }
        std::mem::take(&mut self.out_buf)
    }

    /// 尾部 flush：零填残差到 FRAME_SIZE，处理一帧排出尾部。
    pub fn flush(&mut self) -> Vec<f32> {
        if !self.in_buf.is_empty() {
            self.in_buf.resize(FRAME_SIZE, 0.0);
            let pcm: [f32; FRAME_SIZE] = std::array::from_fn(|i| self.in_buf[i]);
            let mut out_frame = [0.0f32; FRAME_SIZE];
            if let Some(b) = self.backend.as_mut() {
                b.process_frame(&pcm, &mut out_frame);
                for &s in &out_frame {
                    self.out_buf.push(s);
                }
            } else {
                for &s in &pcm {
                    self.out_buf.push(s);
                }
            }
            self.in_buf.clear();
        }
        std::mem::take(&mut self.out_buf)
    }
}

impl Default for DenoiseProcessor {
    fn default() -> Self {
        Self::new(DenoiseMode::Rnnoise).expect("RNNoise new 仅在 OOM 失败")
    }
}
```

- [x] **Step 2: 改现有测试的 `new()` 调用为 `new(DenoiseMode::Rnnoise)`**

在 `crates/asr/src/denoise.rs` 测试模块，把所有 `DenoiseProcessor::new().unwrap()` 改为 `DenoiseProcessor::new(DenoiseMode::Rnnoise).unwrap()`。涉及：`processor_basic_roundtrip`、`length_invariant_within_one_frame`、`streaming_incremental_equals_batch`、`diag_pure_noise_suppressed`、`diag_clean_speech_preserved`、`diag_silence_output`、`diag_denoise_tts_wav`、`diag_real_speech_noisy_denoise_effect`。

- [x] **Step 3: 跑测试确认 RNNoise 回归全绿**

Run:
```bash
cargo test -p octopus-asr-local --lib denoise 2>&1 | tail -25
```
Expected: 所有非 `#[ignore]` 测试 ok。
诊断：若 `streaming_incremental_equals_batch` 失败（max_diff≠0）→ 检查 `RnnoiseBackend::process_frame` 的 PCM_SCALE 双向转换（`*PCM_SCALE` 喂、`/PCM_SCALE` 收）。

- [x] **Step 4: 验证 Send 断言（含 Df3Backend 的 unsafe impl）**

Run:
```bash
cargo check -p octopus-desktop 2>&1 | tail -10
```
Expected: `Finished`（`audio.rs:312` 的 `_assert_send_sync::<DenoiseProcessor>()` 通过——RnnoiseBackend 天然 Send，Df3Backend 经 unsafe impl 满足）。

- [x] **Step 5: 提交**

```bash
git add crates/asr/src/denoise.rs
git commit -m "refactor(asr): FrameDenoise trait + RnnoiseBackend + Df3Backend + DenoiseProcessor 重构"
```

---

## Task 4: Df3Backend 行为测试（验证不压语音 / 噪声抑制）

**Files:**
- Modify: `crates/asr/src/denoise.rs`（测试模块追加 DF3 测试）

**背景**：Df3Backend 已在 Task 3 实现。本 task 验证其行为：长度守恒、不压语音（gain≥0.5，反 dfn3 回归）、噪声抑制。DF3 测试需加载 7.9MB 模型，加 `#[ignore]` 手动跑。

> **⚠ DF3 测试输入必须用真实语音**（Task 4 实施发现）：DF3 训练于真实语音时频动态，把恒幅稳态谐波
> （如 `synth_speech` 简单正弦叠加）正确识别为非语音（类啸叫/feedback）并压制——实测合成谐波
> gain≈0.005（比 dfn3 缺陷 0.10 还低！），真实语音 gain≈0.999（spike 真实音频 0.958）。故「不压语音」
> gain 断言**不能用 `synth_speech`**，必须用真实 wav（如 `/tmp/voice48k.wav` TTS 或真实录音，
> `hound::WavReader` 读取；文件不存在则 `#[ignore]` 跳过）。合成谐波对 DF3 是固有代理失真，非「压语音」
> 回归。RNNoise 用频带能量特征，合成谐波测试对它有效（gain≥0.5），故 RNNoise 测试可继续用 `synth_speech`。

- [x] **Step 1: 写 DF3 测试**

在 `crates/asr/src/denoise.rs` 测试模块末尾追加（均 `#[ignore]`，复用现有 `white_noise`/`rms` helper；
**真实语音输入用 `read_wav_48k` helper 读 `/tmp/voice48k.wav`**——见上方背景说明，不能用 `synth_speech`）：

先加真实语音读取 helper（若测试模块尚无）：
```rust
    /// 读取 /tmp/voice48k.wav（48k mono i16）→ [-1,1] f32。
    fn read_wav_48k() -> Vec<f32> {
        let mut reader = hound::WavReader::open("/tmp/voice48k.wav").expect("/tmp/voice48k.wav");
        reader
            .samples::<i16>()
            .map(|s| s.unwrap() as f32 / 32768.0)
            .collect()
    }
```

生成 `/tmp/voice48k.wav`（macOS，48k mono i16）：
```bash
say -o /tmp/voice.aiff "这是一段用于降噪测试的真实中文语音，包含正常语速与停顿。" \
  && ffmpeg -y -i /tmp/voice.aiff -ar 48000 -ac 1 -sample_fmt s16 /tmp/voice48k.wav
```

然后追加测试：

```rust
    // ── DF3 后端测试（需加载 7.9MB 模型，慢，手动 cargo test -- --ignored）──

    /// DF3 加载 + 长度守恒（同 RNNoise 断言）。
    #[test]
    #[ignore]
    fn df3_length_invariant() {
        for &n in &[480usize, 481, 960, 4800] {
            let mut p = DenoiseProcessor::new(DenoiseMode::Df3).unwrap();
            let input: Vec<f32> = (0..n)
                .map(|i| (2.0 * std::f32::consts::PI * 440.0 * i as f32 / 48000.0).sin() * 0.3)
                .collect();
            let mut out = p.process_samples(&input);
            out.extend(p.flush());
            let diff = (out.len() as i64 - n as i64).abs();
            assert!(diff < FRAME_SIZE as i64, "n={n} out={} diff={diff}", out.len());
        }
    }

    /// DF3 不压语音：**必须用真实语音 `/tmp/voice48k.wav`**（非 synth_speech）。
    /// 真实语音 gain 应 ≥0.5（spike 实测 0.96，实施实测 0.999）。
    /// 原因：DF3 把 synth_speech 的稳态谐波当非语音（类啸叫）压制（gain≈0.005），是代理失真非缺陷。
    #[test]
    #[ignore] // 需 /tmp/voice48k.wav
    fn df3_clean_speech_preserved() {
        let input = read_wav_48k();
        let n = input.len();
        let mut p = DenoiseProcessor::new(DenoiseMode::Df3).unwrap();
        let mut out = p.process_samples(&input);
        out.extend(p.flush());
        let lo = FRAME_SIZE * 2;
        let hi = n - FRAME_SIZE * 2;
        let in_rms = rms(&input, lo, hi);
        let out_rms = rms(&out, lo, hi);
        let gain = out_rms / in_rms.max(1e-12);
        eprintln!("DIAG df3_clean: gain={:.3}（真实语音，应 ≥0.5；dfn3 缺陷≈0.10）", gain);
        assert!(gain >= 0.5, "DF3 压语音：gain={:.3}", gain);
    }

    /// DF3 抑制噪声：纯白噪声 out_rms < in_rms。
    #[test]
    #[ignore]
    fn df3_noise_suppressed() {
        let n = 48000 * 3;
        let input = white_noise(n, 0.1);
        let mut p = DenoiseProcessor::new(DenoiseMode::Df3).unwrap();
        let mut out = p.process_samples(&input);
        out.extend(p.flush());
        let lo = FRAME_SIZE * 100;
        let hi = n - FRAME_SIZE * 2;
        let in_rms = rms(&input, lo, hi);
        let out_rms = rms(&out, lo, hi);
        eprintln!("DIAG df3_noise: in_rms={:.4} out_rms={:.4}", in_rms, out_rms);
        assert!(out_rms < in_rms, "DF3 未抑制噪声：out={:.4} in={:.4}", out_rms, in_rms);
    }

    /// 诊断：合成谐波被 DF3 压制的对照（**仅打印 gain，不断言**）。
    /// 用以记录「合成稳态谐波 → DF3 gain≈0.005」这一代理失真现象，警示勿用合成语音测 DF3。
    #[test]
    #[ignore]
    fn df3_synth_speech_gain_diag() {
        let n = 48000 * 2;
        let input = synth_speech(n);
        let mut p = DenoiseProcessor::new(DenoiseMode::Df3).unwrap();
        let mut out = p.process_samples(&input);
        out.extend(p.flush());
        let lo = FRAME_SIZE * 2;
        let hi = n - FRAME_SIZE * 2;
        let gain = rms(&out, lo, hi) / rms(&input, lo, hi).max(1e-12);
        eprintln!("DIAG df3_synth: gain={:.3}（合成稳态谐波；DF3 应压低 ~0.005，非缺陷）", gain);
    }
```

- [x] **Step 2: 跑 DF3 测试（手动，加载模型慢）**

Run:
```bash
cargo test -p octopus-asr-local --lib denoise -- --ignored 2>&1 | tail -25
```
Expected: `df3_length_invariant ... ok`、`df3_clean_speech_preserved ... ok`（真实语音 gain≥0.5，
实测≈0.999）、`df3_noise_suppressed ... ok`、`df3_synth_speech_gain_diag` 仅打印（gain≈0.005）。
诊断：
- 若 `df3_clean_speech_preserved` 报 `/tmp/voice48k.wav` 不存在 → 先用 `say + ffmpeg` 生成（见 Step 1）。
- 若 `df3_clean_speech_preserved` gain<0.5 且输入是真实语音 → 真异常，检查 Df3Backend 实现或模型版本。
  （若误用 `synth_speech` 得 gain≈0.005，是代理失真非缺陷——改用真实 wav。）
- 若 `DfTract::default` panic 未 catch → 确认 `AssertUnwindSafe(DfTract::default)` 包裹正确。
- 若 ndarray 类型不匹配 → 确认 `ndarray_015` 与 libDF 同版本（`grep -A1 'name = "ndarray"' Cargo.lock` 应只有 0.15.x 与 0.17.x 各一）。

- [x] **Step 3: 提交**

```bash
git add crates/asr/src/denoise.rs
git commit -m "test(asr): Df3Backend 行为测试（长度守恒 / 不压语音 / 噪声抑制）"
```

---

## Task 5: audio.rs 接入 denoise_mode

**Files:**
- Modify: `crates/desktop/src/audio.rs:96-98`（读 mode）、`audio.rs:208-220`（构造）、注释 :88

**背景**：audio.rs 经 `octopus_asr_local::config::load_app_config_cached()`（返回 `&AppConfig`）读 `effective_denoise_mode()`。mode=0 直通（不走 down_sampler/denoise），mode=1/2 走 denoise 路径（DenoiseProcessor 内部按 mode 选后端）。

- [x] **Step 1: 改 process_pipeline 读 mode（audio.rs:96-98）**

把：
```rust
        let cfg = octopus_asr_local::config::load_app_config_cached();
        let denoise_on = cfg.denoise_enabled;
```
改为：
```rust
        let cfg = octopus_asr_local::config::load_app_config_cached();
        let denoise_on = cfg.effective_denoise_mode() != 0;
```

- [x] **Step 2: 改 start() 构造传 mode（audio.rs:208-220）**

把：
```rust
        let cfg = octopus_asr_local::config::load_app_config_cached();
        {
            let mut g = self.denoise.lock().unwrap();
            if cfg.denoise_enabled {
                match octopus_asr_local::denoise::DenoiseProcessor::new() {
```
改为：
```rust
        let cfg = octopus_asr_local::config::load_app_config_cached();
        let mode = octopus_asr_local::denoise::DenoiseMode::from_u8(cfg.effective_denoise_mode());
        {
            let mut g = self.denoise.lock().unwrap();
            if mode != octopus_asr_local::denoise::DenoiseMode::Off {
                match octopus_asr_local::denoise::DenoiseProcessor::new(mode) {
```

- [x] **Step 3: 改日志文案（区分 mode）**

把该 match 块内的：
```rust
                        info!("RNNoise 环境降噪已启用（nnnoiseless，48k）");
```
改为：
```rust
                        info!("环境降噪已启用（mode={:?}，48k）", mode);
```
以及降级 warn 文案 `RNNoise 降噪初始化失败` 改为 `环境降噪初始化失败`。

- [x] **Step 4: 更新注释（audio.rs:88）**

把：
```rust
    /// 降级（spec §9）：denoise_enabled=false / 模型缺失 / 实例未就绪 → 走直通（原生→16k），
```
改为：
```rust
    /// 降级（spec §9）：denoise_mode=0 / 模型缺失 / 实例未就绪 → 走直通（原生→16k），
```

- [x] **Step 5: 编译验证**

Run:
```bash
cargo check --workspace --all-targets 2>&1 | tail -10
```
Expected: `Finished` 无错误。

- [x] **Step 6: 提交**

```bash
git add crates/desktop/src/audio.rs
git commit -m "feat(desktop): audio 接入 denoise_mode（effective_denoise_mode 分发）"
```

---

## Task 6: 文档同步

**Files:**
- Modify: `docs/configuration.md`、`docs/architecture.md`

**背景**：CLAUDE.md 强制——需求/接口变更同步文档。

- [x] **Step 1: docs/configuration.md 加 denoise_mode 说明**

grep 定位：
```bash
grep -n "denoise" docs/configuration.md
```
把对应字段说明改为（若无则新增）：
```markdown
- `denoise_mode`（可选，默认看 `denoise_enabled`）：环境降噪模式
  - `0`：关闭（直通）
  - `1`：RNNoise（nnnoiseless，默认）
  - `2`：DeepFilterNet3（libDF v0.5.6，48kHz 全频带，质量最佳，~7.9MB 模型）
  - 未配置时回退旧 `denoise_enabled`（true→1，false→0）以向后兼容。
```

- [x] **Step 2: docs/architecture.md 更新降噪段**

grep 定位：
```bash
grep -n "降噪\|denoise\|RNNoise" docs/architecture.md
```
更新为说明：降噪为可插拔后端（`FrameDenoise` trait），`denoise_mode` 0/1/2 选择；DF3 用 libDF v0.5.6 + tract 0.19（git 依赖），ndarray 0.15 与 asr 0.17 靠 slice 边界隔离；Df3Backend 经 unsafe impl Send（单线程串行访问）。

- [x] **Step 3: 提交**

```bash
git add docs/configuration.md docs/architecture.md
git commit -m "docs: 同步 denoise_mode 0/1/2 与 DF3 整合说明"
```

---

## 验收（手动 e2e）

备份 `~/.octopus/` 后：

```bash
# mode=2 加载 DF3、不压语音
# 编辑 ~/.octopus/config.yaml 设 denoise_mode: 2
cargo run -p octopus-desktop  # 录音 → ASR 结果不应退化（DF3 gain≈0.96）
# mode=1 RNNoise（现状）
# 设 denoise_mode: 1（或删 denoise_mode，留 denoise_enabled: true）
# mode=0 直通
# 设 denoise_mode: 0
```

```bash
cargo test -p octopus-asr-local --lib denoise -- --ignored  # DF3 单元测试（手动，慢）
cargo test -p octopus-asr-local --lib denoise               # RNNoise 回归
cargo test -p octopus-infra denoise                   # config 测试
```


---

## `settings-window.md`

# 设置窗口实施计划（Settings Window）

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 新建独立 Tauri 窗口，提供 GUI 设置界面替代手编 config.yaml，含识别记录浏览、系统设置、模型管理占位三个页面。

**Architecture:** 新建 `settings_window.rs`（窗口创建）+ 扩展 `runtime_config.rs`（`get_config` / `set_config` / `get_history` 通用命令）+ 新建 `dist/settings/index.html`（vanilla HTML 三页面）。入口为工具栏设置按钮 + 托盘菜单"设置..."项，实时保存写 config.yaml + RuntimeConfig。

**Tech Stack:** Rust + Tauri 2 + vanilla HTML/CSS/JS（无构建步骤）+ serde_json + rusqlite

**设计 spec:** `docs/superpowers/specs/2026-06-17-settings-window-design.md`

> ⚠️ **实现演进**（2026-06-18，settings-ui 精简，commit `eb1d249`）：`segment_duration` / `segment_overlap` 两个设置项已从配置 UI 移除——它们属实现细节（用户不可感知），改为 `crates/infra/src/consts.rs` 常量 `SEGMENT_DURATION_S`（20s，连续语音强制截断阈值）/ `SEGMENT_OVERLAP_MS`（200ms，仅强制切断时保留 overlap）。**仅 `segment_silence` 保留为配置项**（默认 400ms）。因此下文 Task 3 的 `set_config` 中 `segment_duration` / `segment_overlap` 分支、相关测试、Task 6 前端 input 中这两字段的代码为**历史实现记录**，当前代码已移除；`segment_silence` 部分仍有效。权威现状见 `architecture.md` / `configuration.md`。

---

## 文件结构

| 文件 | 责任 | 动作 |
|---|---|---|
| `crates/desktop/src/settings_window.rs` | 设置窗口创建 + `open_settings` 命令 + 单例管理 | **新建** |
| `crates/desktop/src/settings_commands.rs` | `get_config` / `set_config` / `get_history` 命令 + 类型校验逻辑 | **新建**（独立文件避免 `runtime_config.rs` 膨胀） |
| `crates/desktop/src/runtime_config.rs` | RuntimeConfig 新增 `asr_correct` / `output_simplified` / `hide_toolbar` 字段 | **修改** |
| `crates/desktop/src/tray.rs` | 托盘菜单新增"设置..."项 | **修改** |
| `crates/desktop/src/main.rs` | 注册新命令 + 设置窗口模块声明 | **修改** |
| `crates/desktop/src/mod.rs` (或 `lib.rs`) | 模块声明 `settings_window` + `settings_commands` | **修改**（如存在） |
| `crates/infra/src/db.rs` | 新增 `list_transcriptions(limit, offset)` 查询函数 + DTO | **修改** |
| `crates/desktop/dist/settings/index.html` | 三页面 vanilla HTML | **新建** |

---

### Task 1: DB 历史查询函数

**Files:**
- Modify: `crates/infra/src/db.rs`
- Test: `crates/infra/src/db.rs`（内联 `#[cfg(test)]` 模块）

- [x] **Step 1: 在 `db.rs` 新增 `TranscriptionRecord` DTO 和 `list_transcriptions` 查询函数**

在 `crates/infra/src/db.rs` 的 `finalize_transcription` 函数之后（约 line 392），添加：

```rust
/// 历史识别记录（设置窗口识别记录页用）。
#[derive(Debug, serde::Serialize)]
pub struct TranscriptionRecord {
    pub id: i64,
    pub created_at: String,
    pub engine: String,
    pub raw_text: String,
    pub polished_text: Option<String>,
    pub polish_status: String,
    pub duration_ms: Option<i64>,
}

/// 分页查询历史识别记录（按 id 降序 = 最新在前）。
pub fn list_transcriptions(limit: u32, offset: u32) -> Result<Vec<TranscriptionRecord>> {
    with_db(|conn| {
        let mut stmt = conn.prepare(
            "SELECT id, created_at, engine, raw_text, polished_text, polish_status, duration_ms
             FROM transcriptions ORDER BY id DESC LIMIT ?1 OFFSET ?2"
        )?;
        let rows = stmt.query_map(params![limit, offset], |row| {
            Ok(TranscriptionRecord {
                id: row.get(0)?,
                created_at: row.get(1)?,
                engine: row.get(2)?,
                raw_text: row.get(3)?,
                polished_text: row.get(4)?,
                polish_status: row.get(5)?,
                duration_ms: row.get(6)?,
            })
        })?;
        let mut records = Vec::new();
        for row in rows {
            records.push(row?);
        }
        Ok(records)
    })
}
```

- [x] **Step 2: 编译验证**

Run: `cargo check -p octopus-infra`
Expected: 编译通过（`serde::Serialize` 需确认 infra crate 已依赖 serde — 检查 `ModelEntry` 等 DTO 已有 `#[derive(Serialize)]` 确认）。

- [x] **Step 3: 在 db.rs 内联测试模块新增测试**

在 db.rs 的 `#[cfg(test)]` 模块末尾添加（需确认测试模块位置——在 `days_to_ymd` 测试之后）：

```rust
    #[test]
    fn list_transcriptions_returns_records_descending() {
        let conn = create_test_db().unwrap();
        // 插入两条记录
        conn.execute(
            "INSERT INTO transcriptions (id, created_at, engine, raw_text, polish_status)
             VALUES (100, '2026-06-17 10:00:00', 'whisper', '你好', 'off')",
            [],
        ).unwrap();
        conn.execute(
            "INSERT INTO transcriptions (id, created_at, engine, raw_text, polished_text, polish_status)
             VALUES (200, '2026-06-17 11:00:00', 'qwen3', '你好世界', '你好，世界。', 'done')",
            [],
        ).unwrap();
        // 查询全部
        let rows = list_transcriptions_at(&conn, 10, 0).unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].id, 200, "最新在前（id 降序）");
        assert_eq!(rows[1].id, 100);
        assert_eq!(rows[0].raw_text, "你好世界");
        assert_eq!(rows[0].polished_text.as_deref(), Some("你好，世界。"));
        // 分页：第一页只取 1 条
        let page1 = list_transcriptions_at(&conn, 1, 0).unwrap();
        assert_eq!(page1.len(), 1);
        assert_eq!(page1[0].id, 200);
        // 第二页
        let page2 = list_transcriptions_at(&conn, 1, 1).unwrap();
        assert_eq!(page2.len(), 1);
        assert_eq!(page2[0].id, 100);
        // 越界：空
        let page3 = list_transcriptions_at(&conn, 10, 2).unwrap();
        assert!(page3.is_empty());
    }
```

注意：测试用 `list_transcriptions_at(&conn, ...)`（直接传 Connection 的版本），需要把 `list_transcriptions` 的核心逻辑拆出一个 `_at` 版本（与现有 `load_models` / `load_models_at` 模式一致）。

- [x] **Step 4: 重构 `list_transcriptions` 拆出 `_at` 版本**

```rust
/// 分页查询历史识别记录（按 id 降序 = 最新在前）。
pub fn list_transcriptions(limit: u32, offset: u32) -> Result<Vec<TranscriptionRecord>> {
    with_db(|conn| list_transcriptions_at(conn, limit, offset))
}

fn list_transcriptions_at(
    conn: &rusqlite::Connection,
    limit: u32,
    offset: u32,
) -> Result<Vec<TranscriptionRecord>> {
    let mut stmt = conn.prepare(
        "SELECT id, created_at, engine, raw_text, polished_text, polish_status, duration_ms
         FROM transcriptions ORDER BY id DESC LIMIT ?1 OFFSET ?2"
    )?;
    let rows = stmt.query_map(params![limit, offset], |row| {
        Ok(TranscriptionRecord {
            id: row.get(0)?,
            created_at: row.get(1)?,
            engine: row.get(2)?,
            raw_text: row.get(3)?,
            polished_text: row.get(4)?,
            polish_status: row.get(5)?,
            duration_ms: row.get(6)?,
        })
    })?;
    let mut records = Vec::new();
    for row in rows {
        records.push(row?);
    }
    Ok(records)
}
```

- [x] **Step 5: 运行测试验证通过**

Run: `cargo test -p octopus-infra`
Expected: 全部通过（含新增 `list_transcriptions_returns_records_descending`）。

- [x] **Step 6: Commit**

```bash
git add crates/infra/src/db.rs
git commit -m "feat(infra): list_transcriptions 分页查询历史识别记录"
```

---

### Task 2: RuntimeConfig 扩展新增字段

**Files:**
- Modify: `crates/desktop/src/runtime_config.rs:13-28`（`RuntimeConfig` struct + `from_config`）

- [x] **Step 1: 扩展 `RuntimeConfig` struct**

在 `crates/desktop/src/runtime_config.rs` line 13 的 `RuntimeConfig` struct 新增 3 个字段：

```rust
pub struct RuntimeConfig {
    pub asr_engine: String,
    pub polish_mode: PolishMode,
    pub polish_llm: String,
    pub denoise_mode: u8,
    pub asr_correct: bool,
    pub output_simplified: bool,
    pub hide_toolbar: bool,
}
```

- [x] **Step 2: 扩展 `from_config`**

```rust
impl RuntimeConfig {
    pub fn from_config(cfg: &octopus_infra::config::AppConfig) -> Self {
        Self {
            asr_engine: cfg.asr_engine.clone(),
            polish_mode: cfg.polish_mode,
            polish_llm: cfg.polish_llm.clone(),
            denoise_mode: cfg.denoise_mode,
            asr_correct: cfg.asr_correct,
            output_simplified: cfg.output_simplified,
            hide_toolbar: cfg.hide_toolbar,
        }
    }
}
```

- [x] **Step 3: 更新 `from_config_mirrors_fields` 测试**

在 `runtime_config.rs` 测试模块中扩展 `from_config_mirrors_fields`：

```rust
    #[test]
    fn from_config_mirrors_fields() {
        let mut cfg = octopus_infra::config::AppConfig::default();
        cfg.asr_engine = "qwen3-asr-0.6B".into();
        cfg.polish_mode = PolishMode::Intermediate;
        cfg.asr_correct = true;
        cfg.output_simplified = false;
        cfg.hide_toolbar = false;
        let rc = RuntimeConfig::from_config(&cfg);
        assert_eq!(rc.asr_engine, "qwen3-asr-0.6B");
        assert_eq!(rc.polish_mode, PolishMode::Intermediate);
        assert!(rc.asr_correct);
        assert!(!rc.output_simplified);
        assert!(!rc.hide_toolbar);
    }
```

- [x] **Step 4: 编译 + 测试**

Run: `cargo check -p octopus-desktop --features embedded && cargo test -p octopus-desktop --features embedded`
Expected: 编译通过，16+ 测试通过（`from_config_mirrors_fields` 更新）。

- [x] **Step 5: Commit**

```bash
git add crates/desktop/src/runtime_config.rs
git commit -m "feat(desktop): RuntimeConfig 新增 asr_correct/output_simplified/hide_toolbar"
```

---

### Task 3: `set_config` 通用写命令 — 类型校验逻辑

**Files:**
- Create: `crates/desktop/src/settings_commands.rs`
- Modify: `crates/desktop/src/main.rs`（模块声明，在 Task 6 统一注册命令时改）

- [x] **Step 1: 创建 `settings_commands.rs`，实现 `set_config` 命令 + 类型校验**

```rust
//! 设置窗口的 Tauri 命令：get_config / set_config / get_history。
//!
//! 与 runtime_config.rs 的区别：后者是工具栏专用命令（每个字段一个命令），
//! 本模块提供通用 get/set（方案 A），供设置窗口 GUI 表单使用。

use serde::Serialize;
use serde_json::Value;
use tauri::State;

use crate::runtime_config::SharedRuntimeConfig;
use crate::config::PolishMode;

// ── get_config 返回 DTO ──

#[derive(Serialize)]
pub struct ConfigResponse {
    pub config: Value,
    pub asr_engines: Vec<crate::runtime_config::EngineOption>,
    pub llm_models: Vec<crate::runtime_config::LlmOption>,
    pub microphones: Vec<String>,
}

// ── get_config 命令 ──

#[tauri::command]
pub fn get_config(rc: State<'_, SharedRuntimeConfig>) -> Result<ConfigResponse, String> {
    let cfg = octopus_infra::config::load_config().map_err(|e| e.to_string())?;
    let config_json = serde_json::to_value(&cfg).map_err(|e| e.to_string())?;

    // ASR 引擎列表（复用 runtime_config 的逻辑）
    let g = rc.read().unwrap();
    let engines = octopus_asr_local::config::list_engines().map_err(|e| e.to_string())?;
    let asr_engines = crate::runtime_config::build_asr_options_public(&g.asr_engine, engines);

    // LLM 模型列表
    let llms = octopus_infra::db::list_llm_models().map_err(|e| e.to_string())?;
    let llm_models = crate::runtime_config::build_llm_options_public(&g.polish_llm, llms);

    // 麦克风设备列表
    let microphones = list_microphones();

    Ok(ConfigResponse {
        config: config_json,
        asr_engines,
        llm_models,
        microphones,
    })
}

/// 枚举系统麦克风设备（cpal 跨平台）。
fn list_microphones() -> Vec<String> {
    let host = match cpal::default_host() {
        h => h,
    };
    match host.input_devices() {
        Ok(devices) => {
            devices
                .filter_map(|d| d.name().ok())
                .collect()
        }
        Err(_) => Vec::new(),
    }
}

// ── set_config 命令 ──

#[tauri::command]
pub fn set_config(
    key: String,
    value: Value,
    rc: State<'_, SharedRuntimeConfig>,
) -> Result<(), String> {
    // 读当前 config.yaml（而非 OnceLock 缓存——确保写回时保留所有字段）
    let mut cfg = octopus_infra::config::load_config().map_err(|e| e.to_string())?;

    // 按字段校验 + 赋值
    apply_config_value(&mut cfg, &key, &value)?;

    // 写 RuntimeConfig（如字段属于运行时镜像）
    sync_runtime_config(&rc, &key, &cfg);

    // 持久化 config.yaml
    write_config_yaml(&cfg)?;

    Ok(())
}

/// 按字段名校验类型/范围并赋值到 AppConfig。非法值返回 Err。
fn apply_config_value(
    cfg: &mut octopus_infra::config::AppConfig,
    key: &str,
    value: &Value,
) -> Result<(), String> {
    match key {
        // ── 字符串枚举 ──
        "language" => {
            let v = value.as_str().ok_or("language 需要字符串")?;
            if !["auto", "zh", "en", "ja", "ko"].contains(&v) {
                return Err(format!("language 非法值 '{}'（应为 auto/zh/en/ja/ko）", v));
            }
            cfg.language = v.to_string();
        }
        "engine_mode" => {
            let v = value.as_str().ok_or("engine_mode 需要字符串")?;
            if !["embedded", "websocket", "grpc"].contains(&v) {
                return Err(format!("engine_mode 非法值 '{}'（应为 embedded/websocket/grpc）", v));
            }
            cfg.engine_mode = v.to_string();
        }
        // ── u8 枚举 ──
        "polish_mode" => {
            let v = value.as_u64().ok_or("polish_mode 需要 0/1/2")? as u8;
            cfg.polish_mode = match v {
                0 => PolishMode::Disabled,
                1 => PolishMode::FinalOnly,
                2 => PolishMode::Intermediate,
                _ => return Err(format!("polish_mode={} 非法（应为 0/1/2）", v)),
            };
        }
        "denoise_mode" => {
            let v = value.as_u64().ok_or("denoise_mode 需要 0/1/2")? as u8;
            if v > 2 {
                return Err(format!("denoise_mode={} 非法（应为 0/1/2）", v));
            }
            cfg.denoise_mode = v;
        }
        // ── bool ──
        "asr_hardware_accelerated" => {
            cfg.asr_hardware_accelerated = value.as_bool().ok_or("asr_hardware_accelerated 需要 bool")?;
        }
        "asr_correct" => {
            cfg.asr_correct = value.as_bool().ok_or("asr_correct 需要 bool")?;
        }
        "output_simplified" => {
            cfg.output_simplified = value.as_bool().ok_or("output_simplified 需要 bool")?;
        }
        "hide_toolbar" => {
            cfg.hide_toolbar = value.as_bool().ok_or("hide_toolbar 需要 bool")?;
        }
        // ── f64 正数 ──
        "segment_duration" => {
            let v = value.as_f64().ok_or("segment_duration 需要数值")?;
            if v <= 0.0 { return Err("segment_duration 必须大于 0".into()); }
            cfg.segment_duration = v;
        }
        "segment_silence" => {
            let v = value.as_f64().ok_or("segment_silence 需要数值")?;
            if v <= 0.0 { return Err("segment_silence 必须大于 0".into()); }
            cfg.segment_silence = v;
        }
        "segment_overlap" => {
            let v = value.as_f64().ok_or("segment_overlap 需要数值")?;
            if v < 0.0 { return Err("segment_overlap 不能为负".into()); }
            cfg.segment_overlap = v;
        }
        "polish_interval" => {
            let v = value.as_f64().ok_or("polish_interval 需要数值")?;
            if v < 0.0 { return Err("polish_interval 不能为负".into()); }
            cfg.polish_interval = v;
        }
        "pause_polish_threshold_ms" => {
            let v = value.as_f64().ok_or("pause_polish_threshold_ms 需要数值")?;
            if v <= 500.0 {
                return Err("pause_polish_threshold_ms 必须 > 500（Active Flush 阈值）".into());
            }
            cfg.pause_polish_threshold_ms = v;
        }
        // ── string（自由）──
        "shortcut" => {
            cfg.shortcut = value.as_str().ok_or("shortcut 需要字符串")?.to_string();
        }
        "microphone" => {
            cfg.microphone = value.as_str().ok_or("microphone 需要字符串")?.to_string();
        }
        "asr_engine" => {
            let bare_name = value.as_str().ok_or("asr_engine 需要字符串")?;
            // 前端传裸 model_name，需构造 3-part spec
            cfg.asr_engine = build_asr_engine_spec(bare_name)?;
        }
        "polish_llm" => {
            let bare_name = value.as_str().ok_or("polish_llm 需要字符串")?;
            // 前端传裸 model_name，空串=不选择模型，其余构造 3-part spec
            cfg.polish_llm = build_polish_llm_spec(bare_name)?;
        }
        _ => return Err(format!("未知配置字段: {}", key)),
    }
    Ok(())
}

/// 字段属于 RuntimeConfig 镜像范围的，同步更新。
fn sync_runtime_config(
    rc: &SharedRuntimeConfig,
    key: &str,
    cfg: &octopus_infra::config::AppConfig,
) {
    let mut g = rc.write().unwrap();
    match key {
        "asr_engine" => g.asr_engine = cfg.asr_engine.clone(),
        "polish_mode" => g.polish_mode = cfg.polish_mode,
        "polish_llm" => g.polish_llm = cfg.polish_llm.clone(),
        "denoise_mode" => g.denoise_mode = cfg.denoise_mode,
        "asr_correct" => g.asr_correct = cfg.asr_correct,
        "output_simplified" => g.output_simplified = cfg.output_simplified,
        "hide_toolbar" => g.hide_toolbar = cfg.hide_toolbar,
        _ => {}
    }
}

fn write_config_yaml(cfg: &octopus_infra::config::AppConfig) -> Result<(), String> {
    let path = octopus_infra::octopus_config_home().join("config.yaml");
    let text = serde_yaml::to_string(cfg).map_err(|e| e.to_string())?;
    std::fs::write(&path, text).map_err(|e| e.to_string())
}

// ── get_history 命令 ──

#[tauri::command]
pub fn get_history(limit: u32, offset: u32) -> Result<Vec<octopus_infra::db::TranscriptionRecord>, String> {
    octopus_infra::db::list_transcriptions(limit, offset).map_err(|e| e.to_string())
}

// ── 单测（纯逻辑校验，不触文件 IO / Tauri State）──

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn apply_config_valid_bool() {
        let mut cfg = octopus_infra::config::AppConfig::default();
        apply_config_value(&mut cfg, "asr_correct", &json!(true)).unwrap();
        assert!(cfg.asr_correct);
        apply_config_value(&mut cfg, "asr_correct", &json!(false)).unwrap();
        assert!(!cfg.asr_correct);
    }

    #[test]
    fn apply_config_invalid_bool() {
        let mut cfg = octopus_infra::config::AppConfig::default();
        assert!(apply_config_value(&mut cfg, "asr_correct", &json!("yes")).is_err());
    }

    #[test]
    fn apply_config_valid_f64() {
        let mut cfg = octopus_infra::config::AppConfig::default();
        apply_config_value(&mut cfg, "segment_duration", &json!(10.0)).unwrap();
        assert_eq!(cfg.segment_duration, 10.0);
    }

    #[test]
    fn apply_config_invalid_f64_zero() {
        let mut cfg = octopus_infra::config::AppConfig::default();
        assert!(apply_config_value(&mut cfg, "segment_duration", &json!(0.0)).is_err());
        assert!(apply_config_value(&mut cfg, "segment_duration", &json!(-1.0)).is_err());
    }

    #[test]
    fn apply_config_pause_polish_threshold_must_ge_500() {
        let mut cfg = octopus_infra::config::AppConfig::default();
        assert!(apply_config_value(&mut cfg, "pause_polish_threshold_ms", &json!(499.0)).is_err());
        assert!(apply_config_value(&mut cfg, "pause_polish_threshold_ms", &json!(500.0)).is_ok());
        assert!(apply_config_value(&mut cfg, "pause_polish_threshold_ms", &json!(600.0)).is_ok());
    }

    #[test]
    fn apply_config_valid_polish_mode() {
        let mut cfg = octopus_infra::config::AppConfig::default();
        for n in 0..=2u8 {
            apply_config_value(&mut cfg, "polish_mode", &json!(n)).unwrap();
        }
        assert!(apply_config_value(&mut cfg, "polish_mode", &json!(3)).is_err());
    }

    #[test]
    fn apply_config_valid_language() {
        let mut cfg = octopus_infra::config::AppConfig::default();
        apply_config_value(&mut cfg, "language", &json!("zh")).unwrap();
        assert_eq!(cfg.language, "zh");
        assert!(apply_config_value(&mut cfg, "language", &json!("fr")).is_err());
    }

    #[test]
    fn apply_config_unknown_key() {
        let mut cfg = octopus_infra::config::AppConfig::default();
        assert!(apply_config_value(&mut cfg, "nonexistent_field", &json!(1)).is_err());
    }

    #[test]
    fn apply_config_string_fields() {
        let mut cfg = octopus_infra::config::AppConfig::default();
        apply_config_value(&mut cfg, "shortcut", &json!("Ctrl+Alt+Z")).unwrap();
        assert_eq!(cfg.shortcut, "Ctrl+Alt+Z");
        apply_config_value(&mut cfg, "microphone", &json!("External Mic")).unwrap();
        assert_eq!(cfg.microphone, "External Mic");
    }
}
```

- [x] **Step 2: 在 `runtime_config.rs` 暴露 `build_asr_options` / `build_llm_options` 的公开包装**

`settings_commands.rs` 需要调用 `build_asr_options` 和 `build_llm_options`，但它们目前是私有的。在 `runtime_config.rs` 添加公开包装函数：

```rust
/// 公开包装（供 settings_commands 调用）。
pub fn build_asr_options_public(
    current_effective: &str,
    engines: Vec<octopus_asr_local::config::EngineInfo>,
) -> Vec<EngineOption> {
    build_asr_options(current_effective, engines)
}

pub fn build_llm_options_public(
    current: &str,
    llms: Vec<octopus_infra::db::LlmModelInfo>,
) -> Vec<LlmOption> {
    build_llm_options(current, llms)
}
```

- [x] **Step 3: 在 `main.rs` 添加模块声明**

在 `main.rs` 的模块声明区域（`mod runtime_config;` 附近）添加：

```rust
mod settings_commands;
mod settings_window;
```

（`settings_window` 在 Task 4 创建，先声明不影响编译——如编译报错可先注释 `settings_window` 行。）

- [x] **Step 4: 检查 `Cargo.toml` 是否已有 `cpal` 依赖**

Run: `grep cpal crates/desktop/Cargo.toml`
Expected: 已有（audio.rs 使用）。如无，添加 `cpal = { workspace = true }`（但应已有）。

- [x] **Step 5: 编译 + 测试**

Run: `cargo check -p octopus-desktop --features embedded && cargo test -p octopus-desktop --features embedded`
Expected: 编译通过，新增 8 个测试通过。

- [x] **Step 6: Commit**

```bash
git add crates/desktop/src/settings_commands.rs crates/desktop/src/runtime_config.rs crates/desktop/src/main.rs
git commit -m "feat(desktop): set_config/get_config/get_history 通用命令 + 类型校验"
```

---

### Task 4: 设置窗口创建（`settings_window.rs`）

**Files:**
- Create: `crates/desktop/src/settings_window.rs`

- [x] **Step 1: 创建 `settings_window.rs`**

```rust
//! 设置窗口：独立 Tauri 窗口，原生标题栏，800×600 可调大小。
//!
//! 单例管理：已打开则 set_focus，不重复创建。
//! 参考 result_window.rs 但更简单——设置窗无需 ready/pending 机制，
//! 前端加载后主动 invoke('get_config') 拉数据。

use tauri::{Manager, WebviewUrl, WebviewWindowBuilder};

const SETTINGS_WIDTH: f64 = 800.0;
const SETTINGS_HEIGHT: f64 = 600.0;
const MIN_WIDTH: f64 = 640.0;
const MIN_HEIGHT: f64 = 480.0;
const WINDOW_LABEL: &str = "settings_window";

/// 打开设置窗口（单例：已存在则 set_focus）。
#[tauri::command]
pub fn open_settings(app_handle: tauri::AppHandle) {
    if let Some(window) = app_handle.get_webview_window(WINDOW_LABEL) {
        let _ = window.set_focus();
        return;
    }
    let _ = WebviewWindowBuilder::new(
        &app_handle,
        WINDOW_LABEL,
        WebviewUrl::App("settings/index.html".into()),
    )
    .title("Octopus 设置")
    .inner_size(SETTINGS_WIDTH, SETTINGS_HEIGHT)
    .min_inner_size(MIN_WIDTH, MIN_HEIGHT)
    .decorations(true)
    .visible(true)
    .build();
}
```

- [x] **Step 2: 编译验证**

Run: `cargo check -p octopus-desktop --features embedded`
Expected: 编译通过。

- [x] **Step 3: Commit**

```bash
git add crates/desktop/src/settings_window.rs
git commit -m "feat(desktop): settings_window 模块 — 窗口创建 + open_settings 命令"
```

---

### Task 5: 工具栏设置按钮接通 + 托盘菜单"设置..."

**Files:**
- Modify: `crates/desktop/src/main.rs`（注册命令）
- Modify: `crates/desktop/src/tray.rs`（托盘菜单加项）
- Modify: `crates/desktop/dist/result/index.html`（工具栏设置按钮 invoke）

- [x] **Step 1: 在 `main.rs` 注册新命令**

在 `main.rs` 的 `invoke_handler` 中（line 152 附近），添加 4 个新命令：

```rust
        .invoke_handler(tauri::generate_handler![
            runtime_config::toolbar_state,
            runtime_config::list_asr_engines,
            runtime_config::switch_asr_engine,
            runtime_config::set_polish_mode,
            runtime_config::list_llm_models,
            runtime_config::switch_polish_llm,
            runtime_config::set_denoise_mode,
            coordinator::cancel_recording,
            coordinator::polish_now,
            result_window::result_window_ready,
            // 设置窗口命令
            settings_window::open_settings,
            settings_commands::get_config,
            settings_commands::set_config,
            settings_commands::get_history,
        ])
```

- [x] **Step 2: 在 `tray.rs` 托盘菜单添加"设置..."项**

在 `create_tray` 函数中，`quit` 菜单项之前添加 `settings` 菜单项：

```rust
    let settings = MenuItem::with_id(app, "settings", "设置...", true, None::<&str>)
        .expect("failed to create settings menu item");
    let quit = MenuItem::with_id(app, "quit", "退出", true, None::<&str>)
        .expect("failed to create quit menu item");

    let menu = Menu::with_items(app, &[&toggle, &engine_info, &settings, &quit])
        .expect("failed to create tray menu");
```

在 `on_menu_event` 闭包中，`"quit"` 之前添加：

```rust
            "settings" => {
                info!("Tray: open settings");
                crate::settings_window::open_settings(app.clone());
            }
```

注意：`open_settings` 的签名是 `#[tauri::command] pub fn open_settings(app_handle: tauri::AppHandle)`，直接调用时传 `app.clone()`。

- [x] **Step 3: 修改工具栏设置按钮点击事件**

在 `crates/desktop/dist/result/index.html` 中找到设置按钮的点击处理（当前是占位，无动作），改为：

找到工具栏设置按钮的 `addEventListener` 或其 `onclick`（如果没有，在 JS 初始化区域添加）：

```javascript
    document.getElementById('tool-settings').addEventListener('click', async () => {
      try { await invoke('open_settings'); }
      catch (e) { showToast('打开设置失败：' + e); }
    });
```

- [x] **Step 4: 编译验证**

Run: `cargo check -p octopus-desktop --features embedded`
Expected: 编译通过。

- [x] **Step 5: Commit**

```bash
git add crates/desktop/src/main.rs crates/desktop/src/tray.rs crates/desktop/dist/result/index.html
git commit -m "feat(desktop): 工具栏设置按钮 + 托盘菜单接通 open_settings"
```

---

### Task 6: 前端设置页面骨架（侧边栏 + 3 页面切换）

**Files:**
- Create: `crates/desktop/dist/settings/index.html`

这是最大的单步。先创建包含完整 CSS + JS 骨架 + 侧边栏导航 + 3 页面容器的 HTML，页面内容（设置表单 / 历史列表）在后续 Task 填充。

- [x] **Step 1: 创建 `dist/settings/index.html` 骨架**

```html
<!DOCTYPE html>
<html lang="zh-CN">
<head>
<meta charset="UTF-8">
<meta name="viewport" content="width=device-width, initial-scale=1.0">
<title>Octopus 设置</title>
<style>
* { margin: 0; padding: 0; box-sizing: border-box; }
:root {
  --sidebar-bg: #f5f5f7;
  --content-bg: #ffffff;
  --primary: #007aff;
  --text-primary: #1d1d1f;
  --text-secondary: #86868b;
  --border: #e5e5e7;
  --card-bg: #ffffff;
  --toggle-on: #34c759;
  --toggle-off: #e5e5e7;
  --radius: 8px;
}
body {
  font-family: -apple-system, "Segoe UI", "Noto Sans", sans-serif;
  color: var(--text-primary);
  display: flex;
  height: 100vh;
  overflow: hidden;
}
/* ── 侧边栏 ── */
#sidebar {
  width: 180px;
  background: var(--sidebar-bg);
  border-right: 1px solid var(--border);
  display: flex;
  flex-direction: column;
  flex-shrink: 0;
}
#sidebar .logo {
  padding: 20px 16px 12px;
  font-size: 18px;
  font-weight: 700;
}
#sidebar nav { flex: 1; padding: 8px 0; }
#sidebar nav .nav-item {
  display: flex;
  align-items: center;
  gap: 10px;
  padding: 8px 16px;
  cursor: pointer;
  color: var(--text-primary);
  transition: background 0.15s;
  font-size: 14px;
}
#sidebar nav .nav-item:hover { background: rgba(0,0,0,0.05); }
#sidebar nav .nav-item.active { color: var(--primary); background: rgba(0,122,255,0.08); }
#sidebar nav .nav-item .icon { width: 18px; height: 18px; background: currentColor;
  -webkit-mask-size: contain; mask-size: contain; -webkit-mask-repeat: no-repeat; mask-repeat: no-repeat;
  -webkit-mask-position: center; mask-position: center; flex-shrink: 0; }
/* ── 主内容区 ── */
#content { flex: 1; overflow-y: auto; background: var(--content-bg); }
.page { display: none; padding: 24px; }
.page.active { display: block; }
/* ── 卡片 ── */
.card { background: var(--card-bg); border: 1px solid var(--border); border-radius: var(--radius); padding: 16px; margin-bottom: 16px; }
.card h3 { font-size: 14px; font-weight: 600; margin-bottom: 12px; color: var(--text-primary); }
.card .row { display: flex; align-items: center; justify-content: space-between; padding: 8px 0; border-bottom: 1px solid var(--border); }
.card .row:last-child { border-bottom: none; }
.card .row .label-group { display: flex; flex-direction: column; gap: 2px; }
.card .row .label-text { font-size: 14px; }
.card .row .label-hint { font-size: 12px; color: var(--text-secondary); }
.card .row .badge { font-size: 11px; color: var(--text-secondary); background: var(--sidebar-bg); padding: 2px 6px; border-radius: 4px; margin-left: 8px; }
/* ── Toggle switch ── */
.toggle { position: relative; width: 44px; height: 24px; background: var(--toggle-off); border-radius: 12px; cursor: pointer; transition: background 0.2s; flex-shrink: 0; }
.toggle::after { content: ''; position: absolute; top: 2px; left: 2px; width: 20px; height: 20px; background: white; border-radius: 50%; transition: transform 0.2s; box-shadow: 0 1px 3px rgba(0,0,0,0.2); }
.toggle.on { background: var(--toggle-on); }
.toggle.on::after { transform: translateX(20px); }
/* ── select / input ── */
select, input[type="text"], input[type="number"] {
  padding: 6px 10px; border: 1px solid var(--border); border-radius: 6px;
  font-size: 14px; color: var(--text-primary); background: white; min-width: 180px;
}
select:focus, input:focus { outline: none; border-color: var(--primary); }
/* ── 历史记录 ── */
.history-item { padding: 12px 0; border-bottom: 1px solid var(--border); }
.history-item .timestamp { font-size: 12px; color: var(--text-secondary); margin-bottom: 4px; }
.history-item .raw-text { font-size: 14px; line-height: 1.5; }
.history-item .polished-text { font-size: 14px; color: var(--text-secondary); margin-top: 4px; display: none; }
.history-item.expanded .polished-text { display: block; }
.history-item .meta { font-size: 11px; color: var(--text-secondary); margin-top: 4px; display: flex; gap: 12px; }
.history-item .expand-btn { color: var(--primary); cursor: pointer; font-size: 12px; user-select: none; }
/* ── toast ── */
#toast { position: fixed; bottom: 24px; left: 50%; transform: translateX(-50%);
  background: rgba(0,0,0,0.8); color: white; padding: 8px 16px; border-radius: 8px;
  font-size: 14px; z-index: 9999; display: none; }
/* ── 占位 ── */
.placeholder-page { display: flex; align-items: center; justify-content: center; flex-direction: column; height: 100%; color: var(--text-secondary); }
.placeholder-page .icon-big { width: 48px; height: 48px; margin-bottom: 16px; opacity: 0.3; }
</style>
</head>
<body>
<!-- 侧边栏 -->
<div id="sidebar">
  <div class="logo">Octopus</div>
  <nav>
    <div class="nav-item active" data-page="history" onclick="switchPage('history')">
      <div class="icon" style="-webkit-mask-image: url(../result/icons/settings.svg?v=2); mask-image: url(../result/icons/settings.svg?v=2);"></div>
      <span>识别记录</span>
    </div>
    <div class="nav-item" data-page="settings" onclick="switchPage('settings')">
      <div class="icon" style="-webkit-mask-image: url(../result/icons/settings.svg?v=2); mask-image: url(../result/icons/settings.svg?v=2);"></div>
      <span>系统设置</span>
    </div>
    <div class="nav-item" data-page="models" onclick="switchPage('models')">
      <div class="icon" style="-webkit-mask-image: url(../result/icons/settings.svg?v=2); mask-image: url(../result/icons/settings.svg?v=2);"></div>
      <span>模型管理</span>
    </div>
  </nav>
</div>

<!-- 主内容区 -->
<div id="content">
  <!-- 页面 1: 识别记录 -->
  <div class="page active" id="page-history">
    <div id="history-current" style="margin-bottom: 24px;">
      <!-- 当前识别文本（录音中实时更新） -->
    </div>
    <div id="history-list">
      <!-- 历史记录列表 -->
    </div>
    <div id="history-loading" style="text-align: center; padding: 16px; color: var(--text-secondary); display: none;">
      加载中...
    </div>
  </div>

  <!-- 页面 2: 系统设置 -->
  <div class="page" id="page-settings">
    <!-- 由 JS 动态渲染 -->
  </div>

  <!-- 页面 3: 模型管理（占位） -->
  <div class="page" id="page-models">
    <div class="placeholder-page">
      <div style="font-size: 48px; margin-bottom: 16px; opacity: 0.2;">📦</div>
      <p>功能开发中，敬请期待</p>
    </div>
  </div>
</div>

<div id="toast"></div>

<script>
const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;
let historyOffset = 0;
let historyLoading = false;
let historyDone = false;
let currentConfig = null;

// ── 页面切换 ──
function switchPage(pageName) {
  document.querySelectorAll('.nav-item').forEach(el => el.classList.toggle('active', el.dataset.page === pageName));
  document.querySelectorAll('.page').forEach(el => el.classList.toggle('active', el.id === 'page-' + pageName));
}

// ── Toast ──
function showToast(msg) {
  const toast = document.getElementById('toast');
  toast.textContent = msg;
  toast.style.display = 'block';
  setTimeout(() => { toast.style.display = 'none'; }, 3000);
}
window.showToast = showToast;
window.switchPage = switchPage;

// ── 初始化 ──
async function init() {
  // 加载系统设置
  try {
    const resp = await invoke('get_config');
    currentConfig = resp.config;
    renderSettings(resp);
  } catch (e) {
    showToast('加载配置失败：' + e);
  }
  // 加载历史记录第一页
  await loadHistory();
  // 监听实时识别更新
  listen('update-result', (event) => {
    document.getElementById('history-current').innerHTML =
      '<div style="padding:12px;background:var(--sidebar-bg);border-radius:8px;"><div style="font-size:12px;color:var(--text-secondary);margin-bottom:4px;">当前识别</div><div style="font-size:14px;">' + event.payload + '</div></div>';
  });
}

// ── 历史记录 ──
async function loadHistory() {
  if (historyLoading || historyDone) return;
  historyLoading = true;
  document.getElementById('history-loading').style.display = 'block';
  try {
    const records = await invoke('get_history', { limit: 20, offset: historyOffset });
    if (records.length < 20) { historyDone = true; }
    const list = document.getElementById('history-list');
    if (historyOffset === 0) list.innerHTML = '';
    records.forEach(r => {
      const div = document.createElement('div');
      div.className = 'history-item';
      const time = r.created_at.split(' ')[1] || r.created_at;
      const statusText = { done: '已润色', failed: '润色失败', off: '未润色' }[r.polish_status] || r.polish_status;
      const duration = r.duration_ms ? (r.duration_ms / 1000).toFixed(1) + 's' : '';
      div.innerHTML = `
        <div class="timestamp">${time}</div>
        <div class="raw-text">${escapeHtml(r.raw_text)}</div>
        ${r.polished_text ? `<div class="polished-text">${escapeHtml(r.polished_text)}</div><div class="expand-btn" onclick="this.parentElement.classList.toggle('expanded')">展开/折叠润色</div>` : ''}
        <div class="meta"><span>${escapeHtml(r.engine)}</span><span>${statusText}</span><span>${duration}</span></div>
      `;
      list.appendChild(div);
    });
    historyOffset += records.length;
  } catch (e) {
    showToast('加载历史失败：' + e);
  }
  historyLoading = false;
  document.getElementById('history-loading').style.display = 'none';
}

function escapeHtml(text) {
  const div = document.createElement('div');
  div.textContent = text;
  return div.innerHTML;
}
window.loadHistory = loadHistory;

// 滚动加载
document.getElementById('content').addEventListener('scroll', (e) => {
  const el = e.target;
  if (el.scrollHeight - el.scrollTop - el.clientHeight < 100) {
    loadHistory();
  }
});

// ── 系统设置渲染 ──
function renderSettings(resp) {
  const cfg = resp.config;
  const container = document.getElementById('page-settings');
  const asrOptions = resp.asr_engines.map(e => `<option value="${e.name}" ${e.current ? 'selected' : ''}>${escapeHtml(e.label)}</option>`).join('');
  const llmOptions = resp.llm_models.map(m => `<option value="${m.name}" ${m.current ? 'selected' : ''}>${escapeHtml(m.label)}</option>`).join('');
  const micOptions = ['<option value="">系统默认</option>'].concat(resp.microphones.map(m => `<option value="${escapeHtml(m)}" ${cfg.microphone === m ? 'selected' : ''}>${escapeHtml(m)}</option>`)).join('');

  container.innerHTML = `
    <!-- 识别 -->
    <div class="card">
      <h3>识别</h3>
      <div class="row">
        <div class="label-group"><span class="label-text">语言</span></div>
        <select onchange="setVal('language', this.value)"><option value="auto" ${cfg.language==='auto'?'selected':''}>自动</option><option value="zh" ${cfg.language==='zh'?'selected':''}>中文</option><option value="en" ${cfg.language==='en'?'selected':''}>英语</option><option value="ja" ${cfg.language==='ja'?'selected':''}>日语</option><option value="ko" ${cfg.language==='ko'?'selected':''}>韩语</option></select>
        <span class="badge">下次录音</span>
      </div>
      <div class="row">
        <div class="label-group"><span class="label-text">ASR 引擎</span></div>
        <select onchange="setVal('asr_engine', this.value)">${asrOptions}</select>
        <span class="badge">下次录音</span>
      </div>
      <div class="row">
        <div class="label-group"><span class="label-text">硬件加速</span><span class="label-hint">GPU/CoreML/DirectML 加速</span></div>
        <div class="toggle ${cfg.asr_hardware_accelerated?'on':''}" onclick="toggleVal('asr_hardware_accelerated', this)"></div>
        <span class="badge">下次录音</span>
      </div>
      <div class="row">
        <div class="label-group"><span class="label-text">ASR 纠错</span><span class="label-hint">拼音映射 + bigram 校正</span></div>
        <div class="toggle ${cfg.asr_correct?'on':''}" onclick="toggleVal('asr_correct', this)"></div>
        <span class="badge">立即</span>
      </div>
      <div class="row">
        <div class="label-group"><span class="label-text">简繁输出</span><span class="label-hint">开启=简体，关闭=繁体</span></div>
        <div class="toggle ${cfg.output_simplified?'on':''}" onclick="toggleVal('output_simplified', this)"></div>
        <span class="badge">立即</span>
      </div>
    </div>
    <!-- 润色 -->
    <div class="card">
      <h3>润色</h3>
      <div class="row">
        <div class="label-group"><span class="label-text">润色模式</span></div>
        <select onchange="setVal('polish_mode', parseInt(this.value))"><option value="0" ${cfg.polish_mode.Disabled!==undefined?'':''}>关闭</option><option value="1">仅最终润色</option><option value="2">中间+最终润色</option></select>
        <span class="badge">立即</span>
      </div>
      <div class="row">
        <div class="label-group"><span class="label-text">润色模型</span></div>
        <select onchange="setVal('polish_llm', this.value)">${llmOptions}</select>
        <span class="badge">下次录音</span>
      </div>
      <div class="row">
        <div class="label-group"><span class="label-text">润色间隔</span><span class="label-hint">秒</span></div>
        <input type="number" min="0" step="0.5" value="${cfg.polish_interval}" onchange="setVal('polish_interval', parseFloat(this.value))">
        <span class="badge">下次录音</span>
      </div>
      <div class="row">
        <div class="label-group"><span class="label-text">停顿润色阈值</span><span class="label-hint">毫秒（须 > 500）</span></div>
        <input type="number" min="501" step="50" value="${cfg.pause_polish_threshold_ms}" onchange="setVal('pause_polish_threshold_ms', parseFloat(this.value))">
        <span class="badge">下次录音</span>
      </div>
    </div>
    <!-- 降噪 -->
    <div class="card">
      <h3>降噪</h3>
      <div class="row">
        <div class="label-group"><span class="label-text">降噪模式</span></div>
        <select onchange="setVal('denoise_mode', parseInt(this.value))"><option value="0" ${cfg.denoise_mode===0?'selected':''}>无</option><option value="1" ${cfg.denoise_mode===1?'selected':''}>轻度</option><option value="2" ${cfg.denoise_mode===2?'selected':''}>深度</option></select>
        <span class="badge">立即</span>
      </div>
    </div>
    <!-- VAD 分段 -->
    <div class="card">
      <h3>VAD 分段</h3>
      <div class="row">
        <div class="label-group"><span class="label-text">分段时长</span><span class="label-hint">秒</span></div>
        <input type="number" min="1" step="0.5" value="${cfg.segment_duration}" onchange="setVal('segment_duration', parseFloat(this.value))">
        <span class="badge">下次录音</span>
      </div>
      <div class="row">
        <div class="label-group"><span class="label-text">静音阈值</span><span class="label-hint">毫秒</span></div>
        <input type="number" min="100" step="50" value="${cfg.segment_silence}" onchange="setVal('segment_silence', parseFloat(this.value))">
        <span class="badge">下次录音</span>
      </div>
      <div class="row">
        <div class="label-group"><span class="label-text">分段重叠</span><span class="label-hint">毫秒</span></div>
        <input type="number" min="0" step="50" value="${cfg.segment_overlap}" onchange="setVal('segment_overlap', parseFloat(this.value))">
        <span class="badge">下次录音</span>
      </div>
    </div>
    <!-- 音频 -->
    <div class="card">
      <h3>音频</h3>
      <div class="row">
        <div class="label-group"><span class="label-text">麦克风设备</span></div>
        <select onchange="setVal('microphone', this.value)">${micOptions}</select>
        <span class="badge">下次录音</span>
      </div>
    </div>
    <!-- 交互 -->
    <div class="card">
      <h3>交互</h3>
      <div class="row">
        <div class="label-group"><span class="label-text">全局快捷键</span></div>
        <input type="text" value="${escapeHtml(cfg.shortcut)}" onchange="setVal('shortcut', this.value)">
        <span class="badge">重启</span>
      </div>
      <div class="row">
        <div class="label-group"><span class="label-text">工具栏自动隐藏</span><span class="label-hint">关闭=工具栏始终显示</span></div>
        <div class="toggle ${cfg.hide_toolbar?'on':''}" onclick="toggleVal('hide_toolbar', this)"></div>
        <span class="badge">立即</span>
      </div>
    </div>
    <!-- 引擎模式 -->
    <div class="card">
      <h3>引擎模式</h3>
      <div class="row">
        <div class="label-group"><span class="label-text">引擎接入模式</span><span class="label-hint">embedded=本地推理</span></div>
        <select onchange="setVal('engine_mode', this.value)"><option value="embedded" ${cfg.engine_mode==='embedded'?'selected':''}>embedded</option><option value="websocket" ${cfg.engine_mode==='websocket'?'selected':''}>websocket</option><option value="grpc" ${cfg.engine_mode==='grpc'?'selected':''}>grpc</option></select>
        <span class="badge">重启</span>
      </div>
    </div>
  `;
}

// ── 设置写入 ──
async function setVal(key, value) {
  try {
    await invoke('set_config', { key, value });
  } catch (e) {
    showToast(e);
    // 失败：重新加载配置以恢复控件旧值
    const resp = await invoke('get_config');
    renderSettings(resp);
  }
}
window.setVal = setVal;

async function toggleVal(key, el) {
  const newVal = !el.classList.contains('on');
  try {
    await invoke('set_config', { key, value: newVal });
    el.classList.toggle('on', newVal);
  } catch (e) {
    showToast(e);
  }
}
window.toggleVal = toggleVal;

// polish_mode 需要 u8，select 返回的是字符串数字 — setVal 已在 onchange 用 parseInt 转换
// 但 polish_mode 的当前值选中状态需要修正：cfg.polish_mode 在 JSON 中可能是对象
// 修正 polish_mode 下拉框初始选中
function fixPolishModeSelected(cfg) {
  // polish_mode 被 serde 序列化为 "Disabled"/"FinalOnly"/"Intermediate" 字符串
  // 但实际 serde_json 对 unit struct 会序列化为其他形式——需测试确认
}
// 注意：polish_mode 的序列化值需运行时确认，可能需要调整。

// ── 启动 ──
init();
</script>
</body>
</html>
```

- [x] **Step 2: 编译验证（确保 dist/settings/ 目录被 Tauri 识别）**

Run: `cargo check -p octopus-desktop --features embedded`
Expected: 编译通过（Tauri 的 `frontendDist: "dist"` 相对路径包含 `settings/` 子目录）。

- [x] **Step 3: Commit**

```bash
git add crates/desktop/dist/settings/index.html
git commit -m "feat(desktop): 设置窗口前端 — 侧边栏 + 3 页面 + 设置表单 + 历史记录"
```

---

### Task 7: polish_mode 序列化修正 + e2e 联调

**Files:**
- Modify: `crates/desktop/dist/settings/index.html`（如需修正 polish_mode）
- Modify: `crates/desktop/src/settings_commands.rs`（如需修正序列化）

`PolishMode` 是一个 enum，serde 默认序列化为 `{"Disabled":{}}` 形式或字符串。需要确认实际序列化形式并修正前端下拉框选中逻辑。

- [x] **Step 1: 构建 release 并运行**

```bash
cd crates/desktop && cargo run --release --features embedded
```

- [x] **Step 2: 手动测试 — 基本功能**

打开应用后：
1. 点击工具栏设置按钮 → 设置窗口打开
2. 切换三个页面正常
3. 识别记录页显示历史
4. 系统设置页控件渲染正确

- [x] **Step 3: 手动测试 — 实时保存**

1. 切换"ASR 纠错"开关 → 检查 `~/.octopus/config.yaml` 中 `asr_correct` 值已更新
2. 修改"分段时长" → 检查 config.yaml
3. 输入非法值（停顿润色阈值=100）→ 检查 toast 错误提示
4. 切换润色模式下拉 → 确认 polish_mode 值正确（需确认序列化形式）

- [x] **Step 4: 手动测试 — 历史记录**

1. 做几次录音 → 打开设置 → 识别记录页正确显示
2. 滚动到底部 → 确认翻页加载

- [x] **Step 5: 如有 polish_mode 序列化问题，修正**

`PolishMode` 在 `infra::config` 中定义为 `#[derive(Serialize)]` 的 enum。serde 默认序列化为 `{"Disabled":{}}` 或 `"Disabled"`（取决于是否加了 `#[serde(rename_all = ...)]`）。检查实际序列化形式：

```bash
# 查看当前 config.yaml 中 polish_mode 的值（已被 serde_yaml 序列化过）
cat ~/.octopus/config.yaml | grep polish_mode
```

如果序列化为 `polish_mode: Disabled`（字符串），前端下拉框需用字符串值匹配。如果序列化为数字（自定义 Serialize），则按数字处理。根据实际形式修正 `renderSettings` 中的 polish_mode 选中逻辑。

- [x] **Step 6: Commit 联调修正**

```bash
git add -A
git commit -m "fix(desktop): polish_mode 序列化修正 + e2e 联调"
```

---

### Task 8: 文档同步 + 最终提交

**Files:**
- Modify: `docs/architecture.md`
- Modify: `docs/configuration.md`

- [x] **Step 1: 在 architecture.md 补充设置窗口说明**

在 architecture.md 的 desktop 窗口管理表格中（`result_window` 行之后），添加 `settings_window` 行：

```
| `settings_window` | GUI 设置界面（原生标题栏、可调大小 800×600、单例）。三页面：识别记录（transcriptions 表浏览）/ 系统设置（config.yaml 19 字段实时保存）/ 模型管理（占位）。入口：工具栏设置按钮 + 托盘"设置..."菜单。通用 `get_config`/`set_config(key,value)` 命令 + `get_history` 分页查询。 |
```

在 architecture.md 的 desktop 模块说明中补充设置窗口子系统段落（参考 RuntimeConfig 段落的风格）。

- [x] **Step 2: 在 configuration.md 补注 GUI 编辑入口**

在 configuration.md 的 config.yaml 表格之后，添加：

```markdown
> **GUI 编辑**：`config.yaml` 的上述字段现可经设置窗口 GUI 编辑（工具栏设置按钮或托盘菜单"设置..."打开），实时保存 + 持久化。部分字段标注生效时机（立即 / 下次录音 / 重启）。
```

- [x] **Step 3: 最终编译 + 测试**

Run: `cargo check -p octopus-desktop --features embedded && cargo test -p octopus-desktop --features embedded`
Expected: 编译零警告，全部测试通过。

- [x] **Step 4: Commit**

```bash
git add docs/architecture.md docs/configuration.md
git commit -m "docs: 设置窗口架构 + 配置 GUI 编辑说明"
```

---

### Task 9: 识别记录页面增强（工具栏 + 批量删除 + 文本顺序反转 + 拷贝）

**Files:**
- Modify: `crates/infra/src/db.rs`（新增 `delete_transcriptions` / `delete_transcriptions_at`）
- Modify: `crates/desktop/src/settings_commands.rs`（新增 `delete_history` 命令）
- Modify: `crates/desktop/src/main.rs`（注册 `delete_history`）
- Modify: `crates/desktop/dist/settings/index.html`（历史页 UI 大改）

- [x] **Step 1: 后端 — `db.rs` 新增批量删除函数**

在 `list_transcriptions` 之后新增 `delete_transcriptions(ids: &[i64])`（公开，走 `with_db`）和 `delete_transcriptions_at(conn, ids)`（内部，可直连 Connection 测试）。SQL：`DELETE FROM transcriptions WHERE id IN (?,?,...)`，空列表直接返回 `Ok(0)` 不执行 SQL。

测试（3 个）：
- `delete_transcriptions_removes_specified_ids`：插入 3 条删 2 条，验证剩余 1 条
- `delete_transcriptions_at_empty_is_noop`：空列表不报错、不删数据
- `delete_transcriptions_at_via_internal_fn`：正常批量删除

- [x] **Step 2: 后端 — `settings_commands.rs` 新增 `delete_history` 命令**

```rust
#[tauri::command]
pub fn delete_history(ids: Vec<i64>) -> Result<usize, String> {
    octopus_infra::db::delete_transcriptions(&ids).map_err(|e| e.to_string())
}
```

`main.rs` invoke_handler 追加 `settings_commands::delete_history`。

- [x] **Step 3: 前端 — 历史页 UI 重构（`dist/settings/index.html`）**

**CSS 新增：**
- `#history-toolbar`：flex 布局，全选 checkbox + 已选计数（左侧）+ 删除按钮（右侧，红色边框，disabled 时灰）
- `.history-item` 改为 flex 布局：checkbox + item-body + item-actions
- `.item-check`：18×18 checkbox
- `.item-body`：flex:1（时间 + 润色 text + 原始 text 折叠 + meta）
- `.item-actions`：拷贝按钮
- `.polished-text`：主文本（黑色，默认显示）
- `.raw-text`：次要文本（灰色，默认 `display:none`，`.expanded` 时显示）— **逻辑反转**

**HTML 结构变更：**
- `#page-history` 内 `#history-current` 之后新增 `#history-toolbar`（全选 checkbox + 删除按钮），初始 `display:none`（有数据时显示）

**JS 变更：**
- `loadHistory()`：记录渲染改为「checkbox + 润色优先 + 拷贝按钮」结构。`data-id` 挂在 `.history-item` 上。首屏时显隐 toolbar。
- 新增 `selectedIds: Set<number>` 状态
- 新增 `onItemCheck(checkbox, id)`：增删 selectedIds + updateSelectedCount
- 新增 `updateSelectedCount()`：更新计数文字、删除按钮 disabled、全选 checkbox 状态（含 indeterminate）
- 新增 `toggleSelectAll(checked)`：批量勾选/取消可见记录
- 新增 `deleteSelected()`：`confirm()` → `invoke('delete_history', {ids})` → 刷新列表（重置 offset）
- 新增 `copyRecord(id)`：取 `.polished-text` 文本 → `navigator.clipboard.writeText`（fallback `execCommand`）

- [x] **Step 4: 测试验证**

```bash
cargo test -p octopus-infra  # 25 tests pass
cargo check -p octopus-desktop --features embedded  # 编译通过
node -e "..."  # JS 语法检查通过
```

- [x] **Step 5: Commit**

```bash
git add -A
git commit -m "feat(desktop): 识别记录页增强—工具栏批量删除 + 文本顺序反转 + 拷贝"
```

---

## 自检清单

### Task 10: macOS Dock 图标动态显隐 + UI 微调

**Files:**
- Modify: `crates/desktop/src/settings_window.rs`（`open_settings` 加 `Regular`、新增 `on_settings_closed`）
- Modify: `crates/desktop/src/main.rs`（启动设 `Accessory`、run 回调监听窗口 Destroyed）
- Modify: `crates/desktop/dist/settings/index.html`（去 logo、拷贝图标、删除确认态 reset）

- [x] **Step 1: macOS 动态激活策略**

启动 `main.rs` 在 `app.run()` 前设 `ActivationPolicy::Accessory`（无 Dock 图标）。`open_settings` 创建窗口前切 `Regular`（Dock 图标出现）。新增 `on_settings_closed(&AppHandle)` 切回 `Accessory`。`main.rs` 的 `app.run()` 回调监听 `RunEvent::WindowEvent { event: Destroyed, label: "settings_window" }` 触发该回调。全 `#[cfg(target_os = "macos")]` 条件编译。

- [x] **Step 2: 侧边栏去 logo**

去掉 `dist/settings/index.html` 侧边栏的 `<div class="logo">Octopus</div>`（窗口 title 已有「Octopus 设置」）。

- [x] **Step 3: 拷贝按钮改图标**

将文字「拷贝」按钮替换为内联 `copy.svg` SVG 图标（16×16，灰色，hover 蓝色）。CSS `.btn-copy` 改为无边框透明背景 icon button。

- [x] **Step 4: 删除确认态自动 reset**

Tauri webview 不支持 `window.confirm()`（返回 `undefined` → falsy → 删除被跳过，数据库不删）。改为两次点击确认（首次变红「确认删除?」3 秒超时）。提取 `resetDeleteConfirm(btn)` 函数，在 `updateSelectedCount()` 中统一调用——勾选/取消任何条目、全选/全不选、超时均自动取消确认态恢复按钮。

- [x] **Step 5: 验证 + Commit**

```bash
cargo test -p octopus-infra -p octopus-desktop --features embedded  # 全绿
cargo build --release -p octopus-desktop --features embedded        # 编译通过
```

---

### Task 11: UI 精细化调整 + 快捷键热重载

**Files:**
- Modify: `crates/desktop/dist/settings/index.html`（侧边栏图标 / section 标题 / label 内联 badge / 语言选项 / 润色间隔+阈值改下拉 / 快捷键捕获）
- Modify: `crates/desktop/src/settings_commands.rs`（`check_shortcut` 命令、`set_config` 快捷键热重载、`pause_polish_threshold_ms >= 500`）
- Modify: `crates/desktop/src/main.rs`（注册 `check_shortcut`）

- [x] **Step 1: 侧边栏图标替换**
- 识别记录 → `message.svg`
- 模型管理 → `model.svg`
- 系统设置 → 保持 `settings.svg`

- [x] **Step 2: 去掉侧边栏 logo**
- 删掉 `<div class="logo">Octopus</div>`（窗口 title 已有「Octopus 设置」）

- [x] **Step 3: 语言选项精简**
- 语言下拉去掉日语/韩语，只保留 自动/中文/英语
- 「语言」label 改为「语言识别」

- [x] **Step 4: 卡片标题精简 + 交互卡置顶**
- 交互卡片移到第一位（在识别之前）
- 去掉交互/识别/润色/降噪/音频/引擎模式的 `<h3>` 标题，只保留 VAD 分段标题

- [x] **Step 5: 生效时间标签内联到 label**
- 去掉独立的右侧 `<span class="badge">`，改为 label 文字后面的灰色小字 `(.label-effect)` 带括号，如「语言识别 (下次录音)」

- [x] **Step 6: 快捷键捕获 + 冲突检测 + 热重载**
- 「全局快捷键」改名「激活/关闭快捷键」，text input 改为快捷键捕获按钮
- 捕获逻辑：点击 → 显示「按下快捷键…」→ keydown 捕获组合键（修饰键+主键）→ Esc 取消
- `check_shortcut` 后端命令：尝试 `on_shortcut` 注册 → 立即 `unregister` → 检测冲突
- 前端流程：捕获 → `check_shortcut` → 成功才 `setVal`，失败 toast + 恢复
- `set_config` 快捷键热重载：注销旧快捷键 + `register_shortcut` 新的，标签从「重启」改为「立即」

- [x] **Step 7: 润色间隔 / 说话换气间隔改下拉**
- 润色间隔：number input → 下拉（仅最后=0 / 每3~8秒），去掉 hint
- 停顿润色阈值 → 改名「说话换气间隔」，number input → 下拉（500~1000ms 六档），去掉 hint
- 后端约束从 `> 500` 改为 `>= 500`

---

## 自检清单

### Spec 覆盖
- ✅ 窗口架构（Task 4）
- ✅ get_config / set_config / get_history / delete_history / check_shortcut 命令（Task 3 + 9 + 11）
- ✅ RuntimeConfig 扩展（Task 2）
- ✅ 工具栏 + 托盘入口（Task 5）
- ✅ 前端三页面（Task 6）
- ✅ 识别记录页—增强（Task 9：工具栏 + 批量删除 + 文本反转 + 拷贝图标）
- ✅ 系统设置页（Task 6 + Task 11 精细化：标题精简 / label 内联 badge / 快捷键捕获 / 下拉化）
- ✅ 模型管理占位（Task 6 HTML）
- ✅ macOS Dock 动态显隐（Task 10）
- ✅ 快捷键冲突检测 + 热重载（Task 11）
- ✅ 侧边栏图标（Task 11：message.svg / model.svg / settings.svg）
- ✅ 跨平台（vanilla HTML + cpal + Tauri 标准 API + macOS 条件编译）
- ✅ 错误处理（Task 3 校验 + Task 6/9/10/11 toast）
- ✅ 文档同步（Task 8 + Task 10 + Task 11）

### 已知风险
- **PolishMode 序列化**：已确认序列化为 `u8`（0/1/2），前端 select 用数字 value（Task 7 已修）。
- **Tauri confirm() 不可用**：所有需要确认的操作均用两次点击替代（Task 10 已处理删除场景）。
- **macOS Dock 图标**：release 裸二进制无 .app bundle，通过 `objc2` 手动 `setApplicationIconImage`（Task 10）。
- **check_shortcut 注册+注销时序**：检测时短暂注册可能与其他应用极小概率竞争，实测可接受。


---
