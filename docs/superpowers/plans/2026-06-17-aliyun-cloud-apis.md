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
cargo test -p octopus-asr
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

        let cfg = octopus_asr::config::load_config()?;
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
use octopus_asr::config::{resolve_active_engine, EngineCategory};

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
