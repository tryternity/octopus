# 阿里云云端 API 接入（LLM + ASR）设计

> Date: 2026-06-17
> Branch: `worktree-aliyun-apis`（worktree 路径 `.claude/worktrees/aliyun-apis`）
> Status: 设计已确认，待 spec 复核 → 写 plan

## 1. 背景与目标

接入两个阿里云云端能力，并顺带统一模型配置的 taxonomy：

1. **阿里云 LLM**（DashScope 百炼，OpenAI 兼容端点）—— 用于语音润色。
2. **阿里云 FunASR Realtime WebSocket ASR**（百炼 Model Studio）—— 远程实时语音识别。
   - 官方协议文档：
     - Fun-ASR：https://help.aliyun.com/zh/model-studio/fun-asr-realtime-websocket-api
     - Qwen-ASR：https://help.aliyun.com/zh/model-studio/qwen-asr-realtime-interaction-process
     - Paraformer-realtime：https://help.aliyun.com/zh/model-studio/websocket-for-paraformer-real-time-service

为此引入 `provider` 维度，把「模型由谁提供 / 在哪运行」（vendor：`local` vs `aliyun`）与「引擎族 / 模型系列」（category）正交分离。ASR 与 LLM 采用**统一**的 `{provider}:{category}:{model_name}` 选择规格。

## 2. 现状（关键事实，设计依据）

- **`models` 表**（`infra/src/db.rs` + `infra/src/db.sql`）：列 `domain`(asr/llm) / `category` / `name` / `source` / `secret_key` / `is_local` / `is_thinking` / `is_streaming` / `is_enabled` / `language` / `description`。唯一键 `UNIQUE(domain, name, is_local, category)`。`ModelEntry` 已含 `secret_key` 字段。
- **`parse_model_spec`**（`infra/src/db.rs`）：2-part `{category}:{name}` / `local:{name}` / 裸名。`asr_engine` 与 `polish_llm` 共用此解析。
- **`EngineCategory`**（`asr/src/config.rs`）：5 个本地族（Whisper/SenseVoice/Paraformer/Qwen3Asr/Zipformer）；`engine_category_from_str("aliyun")` 返回 `None`，未知 category 在 `load_models_at` 被 `_ => continue` 跳过。
- **`AsrSection`**：5 个 category 字段（whisper/sensevoice/paraformer/qwen3_asr/zipformer），各 `Option<HashMap<String, ModelEntry>>`。
- **桌面 `TranscriptionEngine` trait**（`desktop/src/engine.rs`）：分块式 `async fn transcribe(&self, samples: &[f32], language: &str, engine: &str) -> Result<String>`。
- **coordinator**（`desktop/src/coordinator.rs`）：`use_streaming = resolve_active_engine(...).entry.is_streaming`。
  - `true` → `StreamingSession::new(&config.asr_engine)`（asr crate 本地流式引擎，云引擎无法用）。
  - `false` → chunk 路径 `engine.transcribe(&speech_samples, &language, &asr_engine)`（line ~983，传入 `config.asr_engine` spec 字符串）。
- **`polish()`**（`llm/src/client.rs`）：OpenAI 兼容 `{base_url}/chat/completions` + `Authorization: Bearer` + `enable_thinking:false`（Qwen3 思考模型已覆盖）。provider 仅用于日志与 thinking 分派。
- **db.sql 约定**（文件头注释）：开发阶段调 schema 直接删 `~/.octopus/octopus.db` 重初始化，**不写 ALTER 迁移**。`init_schema` 仅 `user_version=0` 时执行 `INIT_SQL`。

## 3. 数据模型变更

### 3.1 `models` 表 schema

- **新增列** `provider TEXT NOT NULL DEFAULT 'local'`：vendor / 运行位置（`local` / `aliyun` / `deepseek` / `bigmodel` / ...）。
- **`name` → `model_name`**：重命名，语义明确（与 provider/category 并列时 `name` 模糊）。
- **`is_local` 保留**：`provider='local'` ⟺ `is_local=1`。二者并存——`is_local` 供现有过滤逻辑与本地标记继续用，`provider` 用于 vendor 路由。最小爆破半径。
- **唯一键**改为 `UNIQUE(domain, provider, category, model_name)`（允许同名模型跨 provider 共存，如 deepseek-v4-flash 在 deepseek 直连与 aliyun 代管下各一行）。
- **迁移方式**：删库重建（用户确认历史数据无所谓）。仅改 `db.sql`，不写 ALTER、不 bump user_version。

### 3.2 统一 taxonomy

| 字段 | ASR 取值 | LLM 取值 |
|---|---|---|
| `provider` | `local` / `aliyun` | `local` / `aliyun` / `deepseek` / `bigmodel` |
| `category` | 引擎族：`zipformer` / `whisper` / `sensevoice` / `paraformer` / `qwen3-asr` / `Fun-ASR` | 模型系列：`qwen` / `glm` / `deepseek` |
| `model_name` | 具体模型：`zipformer-small-ctc` / `fun-asr-2025-11-07` | `qwen-plus` / `glm-4-flashx` |

### 3.3 选择规格 `parse_model_spec` → 3-part

`asr_engine` 与 `polish_llm` 统一为 `{provider}:{category}:{model_name}`：

- `local:zipformer:zipformer-small-ctc`
- `aliyun:Fun-ASR:fun-asr-2025-11-07`
- `aliyun:qwen:qwen-plus`
- `deepseek:deepseek:deepseek-v4-flash`

```rust
pub enum ModelSpec<'a> {
    /// "{provider}:{category}:{model_name}"
    Full { provider: &'a str, category: &'a str, model_name: &'a str },
    /// 裸 "{model_name}"，仅全局默认 fallback 路径用（跨 provider/category 按 model_name 搜，优先 local）
    NameOnly(&'a str),
}
```

解析：按 `:` 分割。
- 2 个冒号（3 段）→ `Full`。
- 0 冒号 → `NameOnly`（仅 `resolve_active_engine` 全局默认 fallback 用）。
- 1 个冒号（2 段，旧格式）→ 视为非法，记录 warn（迁移期用户更新配置；删库重建后 seed 已是 3-part）。

DB 查询统一 3 字段精确匹配：`WHERE domain=? AND provider=? AND category=? AND model_name=?`。

## 4. Feature 1 — 阿里云 LLM（零 Rust 代码）

`llm/client.rs::polish()` **不改**。`load_llm_model` 改 3 字段查询，返回的 `CompatibleLlmConfig` 仍 `{ provider, model: model_name, base_url: source, secret_key, is_thinking, is_local, is_enabled }`。

- **seed 新增**（`db.sql`，`is_enabled=0` 用户启用）：
  ```sql
  ('llm','aliyun','qwen','qwen-plus','https://dashscope.aliyuncs.com/compatible-mode/v1','Qwen Plus（非思考）',0,0,0),
  ('llm','aliyun','qwen','qwen-turbo','https://dashscope.aliyuncs.com/compatible-mode/v1','Qwen Turbo（非思考，快）',0,0,0);
  ```
  （列序：domain, provider, category, model_name, source, description, is_thinking, is_local, is_enabled）
- **启用**：`polish_llm: "aliyun:qwen:qwen-plus"`，或工具栏润色模型下拉直接选。
- **Qwen3 思考模型**：seed 标 `is_thinking=1` → 现有 `needs_disable_thinking()` → `enable_thinking:false` 自动生效（`polish()` 已覆盖非 deepseek 分支）。

## 5. Feature 2 — 阿里云 FunASR Realtime WS ASR

### 5.1 集成点：chunk 路径

aliyun ASR 行 **`is_streaming=0`** → `is_streaming_engine()` 返回 false → coordinator 走 `engine.transcribe` chunk 路径，避开本地 `StreamingSession`。每段 VAD 内部仍用 realtime WS 流式发送音频，coordinator 层为伪流式（与 sensevoice 等本地非流式引擎一致）。段长 1–3s，WS 连接开销可接受。

### 5.2 `EngineCategory` 扩展

- 新增变体 `EngineCategory::Aliyun`。
- 解析规则：`provider='aliyun'` → `EngineCategory::Aliyun`；`provider='local'` → 按 `category` 字符串映射 5 个本地族。**实现方式**：新增 `resolve_category(provider, category)`——`provider='aliyun'`（不区分大小写）直接返回 `Some(Aliyun)`，否则回落 `engine_category_from_str(category)`。`engine_category_from_str("aliyun")` **仍返回 `None`**——aliyun 不进 5 个本地族字符串映射，只经 provider 分支识别。
- `AsrSection` 新增 `pub aliyun: Option<HashMap<String /*model_name*/, ModelEntry>>`。
- `load_models_at`：按 `(provider, category)` 路由——`provider='local'` 入对应本地 category 字段；`provider='aliyun'` 入 `aliyun` 字段。
- `pick_entry` / `all_sections` / `list_engines` 加 aliyun 臂（`EngineCategory::Aliyun` → `cfg.asr.aliyun`）。
- DB `category='Fun-ASR'`（协议族）由 DashscopeEngine 按 `model_name` 分派协议，不进 `EngineCategory` 枚举。

### 5.3 `DashscopeEngine`（新 `desktop/src/engine_dashscope.rs`）

impl `TranscriptionEngine`：

```
transcribe(samples, language, "aliyun:Fun-ASR:fun-asr-2025-11-07"):
  1. parse_model_spec → (provider=aliyun, category=Fun-ASR, model_name=fun-asr-2025-11-07)
  2. load AsrConfig；cfg.asr.aliyun[model_name] → entry{ source=端点, secret_key, ... }
     （secret_key 空 → anyhow::bail 明确报错）
  3. 按 model_name 分派协议变体（Fun-ASR / paraformer-realtime-v2 / qwen-asr-realtime，事件/参数略异——实现时以官方三文档为准）
  4. 连 WS source，header `Authorization: bearer <secret_key>`
  5. 发 run-task（text frame）：header.action="run-task"/streaming="duplex"，payload.model=<model_name>, task_group="audio", function="recognition", parameters.format="pcm", sample_rate=16000, language_hints=[language]
  6. 等 task-started（拿 task_id）
  7. 流式发二进制 PCM 帧：f32[-1,1] → s16le，按固定块（如 3200 样本=200ms）切分发送
  8. 收 result-generated，累积 payload.output.sentence.text（保留最终 definite 结果）
  9. 发 finish-task，等最终 result-generated / task-finished
  10. 返回累积文本
health_check(): 返回 true（预留）。
段级超时：8s（与 engine_grpc 一致），tokio::time::timeout 包裹。
```

### 5.4 `main.rs` 路由

启动时 `resolve_active_engine(&config.asr_engine)`；若 `resolved.category == EngineCategory::Aliyun` → `Arc::new(DashscopeEngine::new())`；否则现有 `embedded` / `websocket` / `grpc`（按 `engine_mode`）。

### 5.5 cargo feature

`dashscope = ["tokio-tungstenite", "futures-util"]`（与 `remote-ws` 同模式）。`engine_dashscope.rs` + `main.rs` 的 aliyyun 臂置于 `#[cfg(feature = "dashscope")]`。默认不开（与 remote-ws/remote-grpc 一致），用户按需启用。

### 5.6 seed（`db.sql`）

```sql
('asr','aliyun','Fun-ASR','fun-asr-2025-11-07',
  'wss://dashscope.aliyuncs.com/api-ws/v1/inference','auto',
  '阿里云百炼 FunASR 实时（DashScope key 填 secret_key）',0,0,0);
```
（is_local=0, is_enabled=0, is_streaming=0；secret_key 默认空，用户 sqlite3 填）

可选追加 paraformer-realtime / qwen-asr-realtime 行（同结构，不同 model_name + 端点）。

## 6. 波及面（重构清单）

删 `name`→`model_name` 会破坏所有引用，须**原子提交**（中间态不编译）。

- **`infra/src/db.sql`**：DDL（provider 列 + name→model_name + 唯一键）+ 全部 seed 迁移。
  - ASR 8 行：加 `provider='local'`，`name`→`model_name` 列位。
  - LLM 4 行：拆分——provider=原 category（aliyun/deepseek/bigmodel），category=模型系列（deepseek→`deepseek`、glm→`glm`）。
  - 新增 qwen（LLM）+ Fun-ASR（ASR）行。
- **`infra/src/db.rs`**：`parse_model_spec` 3-part + `ModelSpec` enum；`load_models_at`/`load_llm_model_at`/`list_llm_models_at` 查询改 3 字段 + `model_name`；`LlmModelInfo.model_name`；相关测试更新。
- **`asr/src/config.rs`**：`EngineCategory::Aliyun` + `resolve_category(provider, category)` provider 感知解析；`AsrSection.aliyun`；`pick_entry`/`all_sections`/`list_engines`/`resolve_engine_in_config`/`resolve_active_engine` 加 aliyun；测试更新（`engine_category_from_str("aliyun")` **仍返回 `None`**，断言保持；`resolve_category("aliyun", _)` → `Some(Aliyun)`）。
- **`desktop/src/engine_dashscope.rs`**（新）+ `main.rs` 路由 + `Cargo.toml` feature。
- **`desktop/src/runtime_config.rs`**：`build_llm_options`/`build_asr_options*` 的 `.name` → `.model_name` 访问。
- **`llm/src/client.rs`**：无改。
- **`cli/src/main.rs` / `server/src/main.rs`**：若引用 spec 解析或 `.name`，适配 3-part / `.model_name`（grep 确认）。

## 7. 已知限制

- **云↔本地切换**：改 `config.yaml` 的 `asr_engine` 后**重启**生效（engine 实例启动时固定为 cloud 或 embedded）。
- **工具栏 ASR 下拉**运行时不切云端（用户已选 config 方案）；润色模型下拉仍可列/选 aliyun LLM。
- **`secret_key` 暂无 UI**：用户需 `sqlite3 ~/.octopus/octopus.db "UPDATE models SET secret_key='sk-...' WHERE domain='asr' AND model_name='fun-asr-2025-11-07'"`（后续可接 settings 窗口）。
- **旧 2-part spec 配置**需用户更新为 3-part（删库重建后 DB seed 已 3-part；用户 `config.yaml` 的 `asr_engine`/`polish_llm` 需手改）。

## 8. 测试策略

- **`infra/db.rs`**：`parse_model_spec` 3-part 各分支；`load_llm_model_at` 3 字段查询（`aliyun:qwen:qwen-plus` 命中 / 跨 provider 同名 `deepseek:deepseek:deepseek-v4-flash` vs `aliyun:deepseek:deepseek-v4-flash`）；`load_models_at` aliyun section；seed 行数断言更新。
- **`asr/config.rs`**：`resolve_engine_in_config("aliyun:Fun-ASR:fun-asr-2025-11-07")` → `EngineCategory::Aliyun`；`resolve_active_engine` 云路由命中；fallback 仍 zipformer-small-ctc；`pick_entry` aliyun 臂。
- **`engine_dashscope.rs`**：`run-task` JSON 构造单测；f32→s16le PCM 转换单测；result-generated 文本累积单测。WS 端到端为集成测试（需真实 key，标 `#[ignore]`）。
- **手动 e2e**（删库重建 + 配 dashscope key）：
  - `asr_engine: aliyun:Fun-ASR:fun-asr-2025-11-07` → 录音识别出文本。
  - `polish_llm: aliyun:qwen:qwen-plus` → 润色生效。

## 9. 文档同步（CLAUDE.md 强制）

- **`docs/configuration.md`**：`asr_engine`/`polish_llm` 改 3-part 说明 + DashScope 示例 + secret_key 填法 + 删库重建提示 + `dashscope` feature 开启。
- **`docs/architecture.md`**：`models` 表 schema（provider / model_name）、provider×category taxonomy、云引擎路由（category==Aliyun → DashscopeEngine）。
- 本 spec + 对应 plan（`docs/superpowers/plans/2026-06-17-aliyun-cloud-apis.md`）。
