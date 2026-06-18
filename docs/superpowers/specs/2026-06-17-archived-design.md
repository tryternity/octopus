# 归档设计文档（2026-06-17，已实现）

> 本文件合并以下**已实现功能**的原始设计 spec，作为历史记录归档（2026-06-18）。
> 各功能已在 main 实现，**权威现状以 [`architecture.md`](../../architecture.md) / [`configuration.md`](../../configuration.md) 为准**。
> 归档内各 spec 之间的交叉引用可能指向已归档的同级文件——所需内容均在本文内，请按下方标题搜索。

## 包含的原 spec

- `2026-06-17-aliyun-cloud-apis-design.md`
- `2026-06-17-asr-output-hans-variant-design.md`
- `2026-06-17-denoise-deepfilternet3-integration-design.md`
- `2026-06-17-paste-enigo-macos-sigtrap-design.md`
- `2026-06-17-settings-window-design.md`

---

## `aliyun-cloud-apis-design.md`

# 阿里云云端 API 接入（LLM + ASR）设计

> Date: 2026-06-17
> 状态：✅ 已实现（2026-06-17 合并 main，commit `ca53db8`）。LLM DashScope + ASR FunASR Realtime WS（`engine_dashscope.rs`）+ 3-part `{provider}:{category}:{model_name}` taxonomy 均已落地。WS 端到端集成测试标 `#[ignore]`（需真实 key，待手动 e2e）。

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
  5. 发 run-task（text frame）：header.action="run-task"/streaming="duplex"，payload.model=<model_name>, task_group="audio", function="recognition", parameters.format="pcm", sample_rate=16000, language_hints=[language], **payload.input={}（在 payload 内部，不在顶层）**
  6. 等 task-started（拿 task_id）
  7. 流式发二进制 PCM 帧：f32[-1,1] → s16le，按固定块（如 3200 样本=200ms）切分发送
  8. 收 result-generated，累积 payload.output.sentence.text（保留最终 definite 结果）
  9. 发 finish-task（只需 header + payload.input={}，不带 model/parameters），等最终 result-generated / task-finished
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


---

## `asr-output-hans-variant-design.md`

# ASR 输出简繁归一化（output_simplified 开关）设计

> 日期：2026-06-17
> 状态：✅ 已实现

## 背景与目标

用户反馈：Qwen3-ASR 识别结果「有些部分是繁体」。

**根因**：`qwen3-asr-1.7B` + `language: auto`。`qwen3_asr.rs:96-103` 在 auto 时**故意不注入 `language zh` 提示**（保持多语言/中英混合能力，避免英文丢失）。但 Qwen3-ASR 训练语料含繁体，auto 模式不强制简体 → 中文段混入繁体。

sherpa-onnx [#3509](https://github.com/k2-fsa/sherpa-onnx/issues/3509) 显示 qwen3-asr 的 `language` 参数有 bug（连空音频都影响输出），故「config 改 `language=zh`」不可靠。

**目标**：在 ASR 输出后做字形归一化，由开关控制输出**简体或繁体**（用户需求：`true`=简，`false`=繁），保持 auto 多语言优势。

## 选型权衡

| 方案 | 结论 |
|---|---|
| config 改 `language=zh` | ✗ sherpa #3509 不可靠，且可能英文丢失 |
| `ferrous-opencc` (crate) | ✗ 依赖 zstd(C) 违背纯 Rust 偏好；且本环境网络禁无法 `cargo add` |
| `pinyin` / `jieba`（已有依赖） | ✗ 不提供繁简转换（pinyin 的 "hans" 仅是变量名） |
| 内嵌 OpenCC `TSCharacters.txt` | ✗ 网络禁无法下载 |
| **fanjian 对照表（用户提供）** | ✓ CC-BY 3.0，纯 Rust `include_str!`，单字级足够 ASR 场景 |

## 方案

单字级"愚能"字形转换（仅转字形，不转地域用词，如「電腦→电脑」而非「计算机」）：

- **数据**：开放词典网 (kaifangcidian.com) 繁简对照表，[CC-BY 3.0](https://creativecommons.org/licenses/by/3.0/)。vendor 到 `crates/asr/data/`：
  - `t2s.txt`（繁→简，3106 条，一对一）
  - `s2t.txt`（简→繁，4955 条，简→繁一对多**已消歧**取首选，如「发→發」）
- **嵌入**：`include_str!` 编译期嵌入 + `OnceCell<HashMap<char,char>>`，零运行时文件依赖、零新 crate 依赖。

## 设计

### 配置
`infra::config::AppConfig` 新增 `output_simplified: bool`（默认 `true`）。`true`→繁转简，`false`→简转繁。

### 模块 `crates/asr/src/hans.rs`
- `to_simplified(&str) -> String` / `to_traditional(&str) -> String`：单字级查表，未命中字符（已是目标字形/非中文）原样保留。
- `normalize_variant(&str) -> String`：读 `output_simplified` 决定方向（调用方无需传参）。

### 注入点（2 处，覆盖最终输出）
1. `engine.rs::transcribe_with_vad` 返回前（offline 统一出口，在 corrector 之后）。
2. `streaming_engine.rs::finish` 返回前（streaming 统一出口，包装 Paraformer/Zipformer）。

增量中间显示段（`process`/`flush`）不转换——短暂过程显示，最终 paste/入库的文本归一化即可。

### License
数据 CC-BY 3.0，`crates/asr/data/NOTICE` 保留署名（按要求）。

## 测试（hans.rs，无 `#[ignore]`）

- `t2s_first_entry` / `s2t_first_entry`：数据首行映射（`丟→丢`、`专→專`）。
- `t2s_common_phrase` / `s2t_common_phrase`：「語言識別↔语言识别」「電腦↔电脑」。
- `preserves_length_and_non_cjk`：长度不变、英文/数字保留。
- `missing_char_unchanged`：已是简体/无繁体源 → 不变。
- `roundtrip_simplified_via_traditional`：简→繁→简 往返稳定。

全 7 测试通过；`cargo check --workspace --all-targets` + 39 个 asr 测试全过。

## 验收

1. `output_simplified=true`（默认）：ASR 输出含繁体字时自动转简体（解决用户繁体问题）。
2. `output_simplified=false`：输出转繁体。
3. 中英混合 / 非中文不受影响（单字查表，未命中保留）。
4. 切换无需改 ASR 引擎或 `language` 配置。

## 非目标

- 不做词级 / 地域用词转换（"愚能"字面转换，符合数据设计意图）。
- 不转换增量中间显示段（仅最终输出）。
- 不做简繁自动检测（用户显式开关决定）。


---

## `denoise-deepfilternet3-integration-design.md`

# 环境降噪（DeepFilterNet3 原生整合）设计

> 本文是 `2026-06-16-denoise-deepfilternet-design.md` 的续作。上一版因第三方逐帧 ONNX 导出
> （`penta2himajin/dfn3.onnx`）模型层缺陷（压语音至 ~10%）而弃用 DF3、改用 RNNoise。本版
> 用**官方原生 libDF + tract** 重新整合 DF3，经 spike 验证可行，作为 `denoise_mode=2` 与
> RNNoise（mode=1）并存。
>
> 状态：✅ 已实现（2026-06-17，plan `2026-06-17-denoise-deepfilternet3.md` ✓27/☐0 全完成）。`denoise_mode=2` 经官方 libDF + tract 落地，与 RNNoise（mode=1）并存。

## 0. spike 验证结论（2026-06-17）

在新 worktree 对官方源（`Rikorose/DeepFilterNet`）`v0.5.6` tag + tract `^0.19.4` 跑通完整
逐帧 spike（`libDF/examples/verify_gain.rs`，资产 `assets/clean_freesound_33711.wav` 与
`noisy_snr0.wav`）。> 注：spike 起初在 fork `tryternity/DeepFilterNet` 上进行，后续发现该 fork
> 无任何 tag，正式实施改用上游官方源（tag v0.5.6 = commit `978576aa`，与 fork 本地同一
> commit 等价），见 §3.2 修正说明。

| 指标 | 结果 | 判据 | 结论 |
|---|---|---|---|
| 干净语音 gain | **0.958** | 官方应 0.8–1.0 / dfn3 缺陷 0.10 | ✅ 不压语音 |
| 带噪 gain | 0.604 | 应 < 干净 | ✅ 压 ~40% 噪声、保留语音 |
| RTF | 0.015–0.036 | <1.0 即可实时 | ✅ 比实时快 28–66 倍 |

**崩溃根因坐实**：此前失败的 `libDF HEAD`（0.5.7-pre）依赖 tract `^0.21.4`，而 tract 0.21.4 在
native 有 codegen bug（`duplicate name /convt3/Conv.bias` + Conv kernel pack 后权重 NaN），连
官方 `deep-filter` bin 也崩。`v0.5.6` 的 tract `^0.19.4`（解析到 0.19.16）无此 bug。唯一补丁是
`time 0.3.28 → 0.3.44`（rustc 1.96 下 time 0.3.28 的 E0282 类型推断 bug，与 tract 无关）。

**VST3 参考佐证**：`DeepFilterNet3-VST3`（native cdylib 插件，macOS 生产可用）正是
`df = { git="...DeepFilterNet.git", tag="v0.5.6", features=["tract","default-model","transforms"] }`，
证明该组合在 native 可用。

---

## 1. 背景与目标

octopus 采集层已用 RNNoise（`nnnoiseless`）做实时环境降噪（`denoise.rs`）。DF3 是 48kHz 全频带
语音增强，质量优于 RNNoise，但此前因模型缺陷被搁置。spike 已证明官方原生路径可用。

**目标**：将 DF3 作为可选降噪后端整合，与 RNNoise 并存，由配置 `denoise_mode` 切换：

- `0` = 关闭降噪（直通）
- `1` = RNNoise（现状，默认）
- `2` = DeepFilterNet3

**非目标**：替换 RNNoise；改动采集层 audio pipeline 结构；改变 `DenoiseProcessor` 对外接口。

## 2. 范围

### 2.1 在范围内
- `DenoiseProcessor` 重构为 mode 分发器 + trait 后端（对外接口不变）。
- 新增 `Df3Backend`（包装 libDF `DfTract`）。
- 配置 `denoise_mode` 0/1/2 + 向后兼容旧 `denoise_enabled`。
- git 依赖 `deep_filter v0.5.6`（fork tag）+ time patch。
- 测试：RNNoise 回归 + DF3 gain/噪声抑制断言。

### 2.2 不在范围内
- DF3 的低延迟模型（`default-model-ll`）——后续可选。
- DF3 参数（attenuation limit / mix）暴露给用户——YAGNI，先用默认。
- 降噪后处理的可视化/调节 UI。
- 替换 ort——DF3 用 tract（流式 GRU 状态所需），与 ASR 的 ort 无关。

## 3. 模型与依赖选型

### 3.1 为什么是官方 libDF + tract（v0.5.6）

| 路径 | 状态 | 结论 |
|---|---|---|
| 第三方 ONNX `dfn3.onnx` + ort | 模型压语音 gain≈0.10 | ✗ 已弃用（上一版） |
| libDF HEAD(0.5.7-pre) + tract 0.21.4 | native codegen 崩（权重 NaN） | ✗ spike 失败 |
| **libDF v0.5.6 + tract 0.19.4** | **spike gain=0.958，RTF=0.015** | **✓ 采用** |

DF3 的流式 GRU 需要跨帧保持隐状态。tract 的 `PulsedModel` + `SimpleState` 原生支持（`DfTract::process`
每次喂一帧 hop，内部维护状态）。ort 无法等价复刻（需每帧重置或重新导出有状态 ONNX——即 dfn3 失败路）。
故 tract 非冗余，是 DF3 流式的必需。

### 3.2 依赖声明

```toml
# crates/asr/Cargo.toml
# 引用 octopus fork `tryternity/DeepFilterNet` tag v0.5.6（= commit `978576aa`，与上游官方
# `Rikorose/DeepFilterNet` v0.5.6 等价）。自控仓库避免上游删库/移 tag；精确 commit 由 Cargo.lock 锁定。
# 演进：初版用上游官方 Rikorose（fork 当时无 tag）；2026-06-17 在 fork 打同名 tag v0.5.6 后改回 fork。
df = { git = "https://github.com/tryternity/DeepFilterNet.git", tag = "v0.5.6",
       package = "deep_filter", default-features = false,
       features = ["tract", "default-model", "transforms"] }
```

- `default-features = false`：关闭 vorbis/flac（octopus 不解码 ogg/flac）。
- `default-model`：编译期内嵌 `DeepFilterNet3_onnx.tar.gz`（~7.9MB），无需运行时外部模型文件。
- `transforms`：提供 `resample`（octopus 采集已是 48k，实际不调用，但 libDF trait 约束需要）。

### 3.3 time patch

tract 0.19 间接依赖 `time`，默认解析到 `0.3.28`，在 rustc 1.96 下 E0282 编译失败。须确保 octopus
`Cargo.lock` 锁 `time ≥ 0.3.35`（规避 E0282 的最低版本）。

**实施阶段实际情况（2026-06-17）**：workspace 已有 `tauri → plist` 依赖链要求 `time ^0.3.47`，
Cargo.lock 解析到 `0.3.49`，**远高于 0.3.35 阈值**，故 DF3 引入后全程 `cargo check` 无任何 time
E0282 错误，**无需手动 `cargo update -p time`**。

**兜底（仅新克隆环境）**：若未来某环境 tauri/plist 链变动导致 time 解析到 `<0.3.35`，才需手动钉版本：

```bash
cargo update -p time --precise 0.3.36
```

若仅靠 lock 钉版本在 CI/新 clone 不可靠，则在 workspace 根 `Cargo.toml` 加
`time = "0.3.36"` 直接依赖约束固化（不实际使用，仅抬高最小版本）。

## 4. 架构

### 4.1 方案选择：trait 抽象（方案 A）

`DenoiseProcessor` 从"具体 RNNoise 实现"重构为"mode 分发器 + 可插拔后端"。对外接口
（`new` / `reset` / `process_samples` / `flush`）与 `Default` **完全不变**，audio.rs 仅把
`denoise_enabled` 读法换成 `denoise_mode`。

对比备选：enum 后端（process 内 match，重复分支）、双 Processor（audio.rs 按 mode 选，改动大）。
trait 方案改动最小、最易扩展。

### 4.2 数据流（不变）

```
cpal 回调 → samples(原生sr) → down_sampler → 48k s48k
  → DenoiseProcessor::process_samples(s48k)   ← mode 分发到 backend
  → enhanced_48k → resampler(48k→16k) → ASR
```

DF3/RNNoise 在边界同构：都是 hop=480 逐帧。`DfTract::process` 内部透明维护 GRU 状态 +
lookahead（延迟 `fft_size-hop + lookahead*hop = 960-480+2·480 = 1440` 样本 ≈ 30ms，由流式状态
与现有 `in_buf`/`out_buf` 累积 + `flush` 尾部补零天然吸收，pipeline 无感）。

## 5. 组件

### 5.1 `FrameDenoise` trait（新增）

```rust
/// 单帧（FRAME_SIZE=480，48k，i16 PCM 等价值域）降噪后端抽象。
/// 仅用原生 slice，不暴露 ndarray —— 隔离 libDF(0.15) 与 asr(0.17)。
trait FrameDenoise: Send + Sync {
    fn process_frame(&mut self, pcm: &[f32; FRAME_SIZE], out: &mut [f32; FRAME_SIZE]);
    /// 清状态（会话边界调用）。各 backend 自行决定轻量清零 vs 重建。
    fn reset(&mut self);
}
```

### 5.2 `RnnoiseBackend`（重构自现有实现）

包装 `nnnoiseless::DenoiseState<'static>`，impl `FrameDenoise`（逻辑即现有
`self.denoise.process_frame(out, pcm)`）。`reset` 重建 `DenoiseState`。

### 5.3 `Df3Backend`（新增）

```rust
use df::tract::DfTract;

pub struct Df3Backend(DfTract);

impl Df3Backend {
    pub fn new() -> Result<Self> {
        // DfTract::default() 加载内嵌 DeepFilterNet3（7.9MB + tract init）
        Ok(Self(DfTract::default()))
    }
}

impl FrameDenoise for Df3Backend {
    fn process_frame(&mut self, pcm: &[f32; FRAME_SIZE], out: &mut [f32; FRAME_SIZE]) {
        // 构造 libDF 的 ArrayView2 [1,480] / ArrayViewMut2 [1,480]
        // 调 self.0.process(noisy_view, enh_view_mut)
        // enh_view_mut → out（libDF 内部 ndarray 0.15，边界转换）
    }
}
```

### 5.4 `DenoiseProcessor` 重构

```rust
pub struct DenoiseProcessor {
    mode: DenoiseMode,                        // 决定 reset 时重建哪个 backend
    backend: Option<Box<dyn FrameDenoise>>,  // None = 直通(mode=0 或加载失败降级)
    in_buf: Vec<f32>,                         // 48k [-1,1] 累积输入
    out_buf: Vec<f32>,                        // 48k [-1,1] 已降噪待输出
}
```

`process_samples`：累积/分帧/PCM_SCALE 逻辑原样保留，核心改为
`if let Some(b) = self.backend.as_mut() { b.process_frame(&pcm, &mut out_frame) } else { out_frame = pcm /* 直通 */ }`。

`flush`：尾部补零逻辑不变（DF3/RNNoise 都按 FRAME_SIZE 补齐）。

`reset`：清 `in_buf`/`out_buf`；调 `backend.reset()`（trait 方法，各 backend 自实现）。
DF3 reset：实施时优先查 libDF 是否提供轻量状态重置（不重载权重）；若无，`Df3Backend::reset`
重建 `DfTract`（成本 = 重载 7.9MB，仅在会话边界 `start()` 调用可接受——VAD 段间不调 denoise
reset，与现有 RNNoise 语义一致，见 denoise.rs:35-36）。

## 6. Send/Sync 安全

`SharedAudioState` 经 `unsafe impl Send/Sync`（audio.rs:302-303）跨 cpal 回调/coordinator，
故 `DenoiseProcessor` 必须 `Send + Sync`（编译期断言 audio.rs:305-312）。

`DfTract` 含 `Arc<dyn RealToComplex<f32>>`（无 `+ Send`）→ `DfTract: !Send` → `Df3Backend: !Send`。
照搬 VST3（`plugin/src/lib.rs:9-11`）：

```rust
// 安全性论证（同 SharedAudioState）：
// - DenoiseProcessor 在 Mutex<Option<..>> 内（audio.rs:26），coordinator 单线程串行 lock+process
//   （audio.rs:94 注释：全在 coordinator 单线程串行调用，无跨线程并发访问）；
// - 实际不存在跨线程并发，unsafe impl 仅满足类型约束，不引入数据竞争。
unsafe impl Send for Df3Backend {}
unsafe impl Sync for Df3Backend {}
```

`RnnoiseBackend`（`Box<DenoiseState<'static>>`）天然 `Send + Sync`，无需 unsafe。

## 7. ndarray 版本隔离

- libDF（deep_filter）依赖 ndarray `0.15`；asr 现有 ndarray `0.17`（ort/whisper 等用）。
- Cargo 允许同 workspace 内 ndarray 0.15 与 0.17 共存（不同 major）。
- **隔离点**：`FrameDenoise` trait 方法只用 `&[f32]` / `&mut [f32]`，绝不暴露 ndarray 类型。
- `Df3Backend::process_frame` 内部用 libDF 的 ndarray 0.15 构造 `ArrayView2` 喂 `DfTract::process`，
  再从 `ArrayViewMut2` 取回 `&mut [f32]`。asr 的 0.17 类型完全不触及。

## 8. 配置

`AppConfig` / `DesktopConfig` 加 `denoise_mode: u8`，serde 默认 `1`（RNNoise，保持当前行为）。
向后兼容旧 `denoise_enabled: bool`：

- `denoise_mode` 存在 → 用它（0/1/2）。
- 缺失但 `denoise_enabled: true` → 映射为 `1`；`false` → `0`。
- 两者皆缺 → 默认 `1`。

audio.rs:98 `let denoise_on = cfg.denoise_enabled;` → 改读 `cfg.denoise_mode`，按 mode 构造 backend。
audio.rs:211-213 `DenoiseProcessor::new()` → `DenoiseProcessor::new(mode)`。

> **实施修正（2026-06-17 合并后）**：上述「向后兼容旧 `denoise_enabled`」逻辑**最终未保留**。
> 合并时发现 main 已独立引入 `denoise_mode: u8`（接工具栏 `set_denoise_mode` 命令 + 持久化），
> 与本设计的 `Option<u8>` + `effective_denoise_mode()` 向后兼容方案冲突。经决策**以 main 的
> `denoise_mode: u8`（固定默认 `1`）为唯一真相**，删除：
> - feature 的 `Option<u8>` 字段与 `effective_denoise_mode()` 方法；
> - 旧 `denoise_enabled: bool` 字段本身（彻底移除，不再保留作回退）。
>
> 现状：audio.rs 直接读 `cfg.denoise_mode`；`default_denoise_mode() = 1`；旧 config.yaml 里残留的
> `denoise_enabled` 被 serde 静默忽略（`AppConfig` 无 `deny_unknown_fields`），不影响解析。

## 9. 懒加载与降级

**懒加载**（mode=2）：

- `DenoiseProcessor::new(mode=2)`：backend 先留 `None` 占位，**不立即加载 DfTract**。
- 首次 `process_samples` 时才 `Df3Backend::new()`（加载 7.9MB + tract init）。
- mode=0 → backend 永远 None（直通）；mode=1 → 构造 `RnnoiseBackend`（同现状）。

**降级**（沿用 audio.rs:88-89 现有语义）：DF3 加载失败 / 单帧推理失败 → warn 日志 + backend 置 None
→ 直通，绝不 panic、绝不阻断录音。start() 在非实时路径（audio.rs:211），首帧加载延迟可接受。

### 模型加载日志（DF3 特有）

tract 加载 DF3 模型时会刷出极大量 DEBUG 日志（`tract_core::optim` 的 `applying patch`
数百行、`tract_hir::infer` 的 `Refined` / `Can't infer shape` 等），且 `df::tract` 自身也打
`Info`/`Debug`（`Init encoder` / `Start init ERB decoder` / `ERB decoder input:` 等），严重
污染 octopus 启动日志。`crates/desktop/src/main.rs` 的 `tauri_plugin_log::Builder`（全局
`level(Debug)`）对这些 target 一律 `level_for(Warn)`：`tract_core` / `tract_hir` /
`tract_onnx` / `tract_linalg` / `df::tract`。

> **2026-06-17 修订**：初版曾有意保留 `df::tract` 的 `Info` 作加载进度信号，实测其 `Info`/`Debug`
> 仍刷屏（`ERB decoder input:` 等），改为一并压到 `Warn`。RNNoise 无 tract 依赖，不受此策略影响。

## 10. 测试策略

**RNNoise 回归**（mode=1）：现有 `denoise.rs` 测试（`processor_basic_roundtrip` /
`length_invariant_within_one_frame` / `streaming_incremental_equals_batch` /
`diag_*`）保持全绿，验证 trait 重构未破坏 RNNoise。

**DF3 新增**（mode=2）：

- 长度守恒：输入 N → process+flush 输出与 N 差 < FRAME_SIZE（同 RNNoise 断言）。
- 干净语音 gain ≥ 0.5（反 dfn3 压语音回归；spike 实测 0.958）。
- 噪声抑制：纯白噪声 → out_rms < in_rms（同 `diag_pure_noise_suppressed` 思路）。
- 用 spike 已验证资产（`assets/clean_freesound_33711.wav` gain≈0.96、`noisy_snr0.wav` gain≈0.60）
  作断言基准。DF3 加载耗资源，DF3 测试加 `#[ignore]` 或独立 feature gate，避免拖慢常规 `cargo test`。

**⚠ DF3 测试输入必须用真实语音**（Task 4 实施发现，2026-06-17）：

DF3 的「干净语音 gain」断言**不能用合成稳态谐波**（如现有 `synth_speech` 的简单正弦叠加），**必须用真实
语音 wav**（如 `/tmp/voice48k.wav` TTS 输出或真实录音）。原因：DF3 训练于真实语音的时频动态，会把恒幅
稳态谐波（持续不变的单一频率/简单谐波叠加）**正确识别为非语音信号**（类啸叫/feedback）并压制——这不是
缺陷，而是 DF3 的设计目标（啸叫抑制）。实测对比：

| 输入 | gain | 判定 |
|---|---|---|
| 合成稳态谐波（`synth_speech`） | **≈0.005** | DF3 当稳态噪声压掉（比 dfn3 缺陷 0.10 还低！） |
| 真实语音 `/tmp/voice48k.wav` | **≈0.999** | 正常保留（spike 真实音频 0.958） |

合成谐波对 DF3 是**固有代理失真**（proxy distortion），**不是「DF3 压语音」回归**。用合成谐波测 DF3 会得到
假阳性失败。RNNoise 用频带能量特征（不依赖时频动态建模），合成谐波测试对它有效（gain≥0.5），故 RNNoise
测试可继续用 `synth_speech`。

**实践**：DF3 gain 断言的输入源用真实 wav 文件路径（如 `/tmp/voice48k.wav`，测试中 `hound::WavReader`
读取）；若文件不存在则 `#[ignore]` 跳过（避免 CI 缺资产失败）。

**Send 守护**：audio.rs:312 编译期 `_assert_send_sync::<DenoiseProcessor>()` 继续生效——
验证 `Df3Backend` 的 unsafe impl 没破坏 `DenoiseProcessor: Send + Sync`。

## 11. 验收标准

- [x] `cargo check --workspace --all-targets` 通过（ndarray 0.15/0.17 共存，time patch 生效）。
- [x] mode=1：所有现有 `denoise.rs` 测试全绿（RNNoise 行为不变）。
- [x] mode=2：DF3 测试通过（gain/噪声抑制/长度守恒）。
- [x] mode=0：直通，输出 = 输入。
- [x] 配置：`denoise_mode: 2` 加载 DF3；`denoise_mode: 1`（缺省）RNNoise；`denoise_mode: 0` 直通（旧 `denoise_enabled` 已删除，详见 §8 实施修正）。
- [x] 手动 e2e：备份 `~/.octopus/` 后，`denoise_mode: 2` 录音 → ASR 不退化（DF3 不压语音）。
- [x] Send 断言编译通过。

## 12. 关键文件

- `crates/asr/Cargo.toml`：加 `df` git 依赖（v0.5.6）+ 可选 time 约束。
- `crates/asr/src/denoise.rs`：`FrameDenoise` trait + `RnnoiseBackend` + `Df3Backend` + `DenoiseProcessor` 重构。
- `crates/desktop/src/audio.rs`：`denoise_enabled` → `denoise_mode`（:98 读、:211 构造）。
- `crates/infra/src/config.rs`：`denoise_mode: u8` 字段 + `default_denoise_mode()`（旧 `denoise_enabled` 已删除，详见 §8 实施修正）。
- workspace `Cargo.toml` / `Cargo.lock`：time patch。

## 13. 历史与关联

- 前作：[`2026-06-16-denoise-deepfilternet-design.md`](./2026-06-16-archived-design.md)
  （dfn3.onnx 弃用记录、RNNoise 现状）。
- spike 证据：`DeepFilterNet/libDF/examples/verify_gain.rs`（fork worktree）。
- 参考：`DeepFilterNet3-VST3/plugin/src/lib.rs`（Send 解法、v0.5.6 依赖范本）。


---

## `paste-enigo-macos-sigtrap-design.md`

# 粘贴崩溃修复（enigo macOS SIGTRAP）设计

> 日期：2026-06-17
> 状态：✅ 已实现

## 现象

`paste_method: clipboard`（默认）模式下，识别完成触发粘贴时应用闪退，终端报：

```
Trace/BPT trap: 5
```

无 Rust panic backtrace、无 macOS 崩溃报告（`.ips`）。日志停在：

```
[paste] step 6: mod pressed, clicking V
./run-octopus.sh: line 23: <pid> Trace/BPT trap: 5
```

即崩溃发生在 `enigo.key(Key::Unicode('v'), Direction::Click)` 调用内部。

## 根因

enigo 0.6.1 在 macOS 上对 `Key::Unicode(c)` 的处理（`enigo/src/macos/macos_impl.rs:1005`）会调用 `get_layoutdependent_keycode(&c.to_string())`。该函数（`:1034`）循环 128 个 keycode，每个都调用 Carbon HIToolbox API：

- `TISCopyCurrentKeyboardInputSource()` / `TISCopyCurrentKeyboardLayoutInputSource()`
- `TISGetInputSourceProperty(.., kTISPropertyUnicodeKeyLayoutData)`
- `UCKeyTranslate(..)`

这些 HIToolbox API **非线程安全**。而粘贴在 `coordinator::do_paste` → `tauri::async_runtime::spawn` → `tokio::task::spawn_blocking` 的**非主线程**中执行（`coordinator.rs:696-697`），触发 macOS 线程断言 → **SIGTRAP**（`Trace/BPT trap: 5`）。

SIGTRAP 不是 Rust panic（不走 `std::panic::set_hook`），故无 backtrace；macOS 对 trap 进程不生成 `.ips` 崩溃报告，只能靠在组件边界插桩日志逐步二分定位。

> 为什么 `Key::Meta`（Cmd）不崩：Cmd 键在 enigo 的 `TryFrom<Key> for CGKeyCode` 中直接映射到固定 keycode `COMMAND=55`（`:1012`），不经过 `get_layoutdependent_keycode`，不触碰 Carbon layout API。

## 方案

macOS 上用**固定虚拟键码** `Key::Other(9)`（`kVK_ANSI_V = 0x09`）替代 `Key::Unicode('v')`。`Key::Other(u32)` 在 `try_from` 中（`:1006-1011`）直接作为 keycode 使用，绕过 `get_layoutdependent_keycode` → 不调用非线程安全的 Carbon API。

Linux / Windows 不受影响（它们的 `Key::Unicode` 处理是线程安全的），保留 `Key::Unicode('v')`。

### 注入点

`crates/desktop/src/paste.rs::paste_via_clipboard`（唯一使用 `Key::Unicode('v')` 的地方）：

```rust
#[cfg(target_os = "macos")]
let v_key = Key::Other(9);          // kVK_ANSI_V，绕过 Carbon layout 查找
#[cfg(not(target_os = "macos"))]
let v_key = Key::Unicode('v');
```

### 附带：panic hook

`crates/desktop/src/main.rs` 安装 `std::panic::set_hook`，把 panic 信息 + backtrace 同时打到 `log` 和 stderr。本 bug 是 SIGTRAP 不被捕获，但 hook 对未来 Rust panic 类故障（如 `unwrap` on None）有诊断价值，故保留。

## 验证

- `cargo check -p octopus-desktop --features embedded` 通过
- E2E：识别→粘贴→结果落地，无闪退（用户确认）

## 非目标

- 不升级 enigo（0.6→0.7 架构变动大，且非线程安全的 Carbon layout 查找在新版仍存在，根治需 enigo 在主线程做 keycode 解析或缓存）
- 不把粘贴移到主线程（会阻塞 UI，违反现有"粘贴异步化"架构）
- 不改 `paste_direct`（direct 模式用 `enigo.text()`，走的是 CGEvent Unicode payload 而非 keycode 映射，不受此 bug 影响）


---

## `settings-window-design.md`

# 设置窗口设计（settings window）

- 日期：2026-06-17
- 状态：✅ 已实现（合并 main，见 plan Task 1–11 全部完成）
- 相关代码：`crates/desktop/src/settings_window.rs`（窗口 + `open_settings` + macOS Dock 图标）、`crates/desktop/src/settings_commands.rs`（`get_config` / `set_config` / `get_history` / `delete_history` / `check_shortcut`）、`crates/desktop/src/runtime_config.rs`（扩展）、`crates/desktop/dist/settings/index.html`（前端）
- 参考界面风格：用户提供 1.png / 2.png（左侧固定侧边栏 + 右侧主内容区，浅色主题，卡片式分块，非原生系统风格的自定义 Web 界面）

---

## 1. 背景与动机

当前 octopus desktop 的配置完全依赖手编 `~/.octopus/config.yaml`（25 字段）+ DB `models` 表。结果窗工具栏已支持运行时快捷切换（ASR 引擎 / 降噪 / 润色模式 / 润色模型 / 立即润色），但系统设置的第一个工具栏按钮仍是占位状态。用户需要一个完整的 GUI 设置界面，替代手编配置文件，降低使用门槛。

参考两张设计图（语音输入类工具的设置界面）：共同风格为**左侧固定侧边栏 + 右侧主内容区**，浅色主题，卡片式分块，图标+文字导航。octopus 的设置窗口将沿用此风格。

---

## 2. 功能范围

三个页面：

| # | 页面 | 内容 | 本轮状态 |
|---|---|---|---|
| 1 | 识别记录 | 当前识别实时文本（录音中）+ 历史记录浏览（transcriptions 表）：工具栏（全选 + 删除）、checkbox 批量选择、润色优先显示、单条拷贝 | ✅ 实现 |
| 2 | 系统设置 | config.yaml 19 个可配置字段的 GUI 编辑（分组卡片 + 实时保存） | ✅ 实现 |
| 3 | 模型管理 | 外部模型 API 配置 + 本地模型下载 | 🔴 占位（本轮不实现） |

---

## 3. 已确认决策（brainstorming 结论）

1. **前端技术**：纯 vanilla HTML（单 `index.html`，内联 CSS/JS），与 result_window 一致，无构建步骤、无 npm 依赖。
2. **入口**：工具栏设置按钮（第一个图标，现有占位）+ 系统托盘菜单新增"设置..."项，两者均调 `open_settings` 命令。
3. **窗口属性**：独立 Tauri 窗口，原生标题栏（`decorations: true`），默认 `800×600`，最小 `640×480`，可调整大小，非置顶，单例（已打开则 `set_focus`）。macOS 下采用 **动态激活策略**：启动/无设置窗时 `Accessory`（仅托盘，无 Dock 图标）；打开设置窗时切 `Regular`（Dock 图标出现）；关闭设置窗时切回 `Accessory`。
4. **保存语义**：实时保存——每个控件改动即时写 `config.yaml` + RuntimeConfig（如适用），无确认按钮。
5. **生效时机标注**：每个控件旁标注"立即"/"下次录音"/"重启"生效标签。
6. **跨平台**：macOS / Windows / Linux 三端 UI 一致。macOS 额外有动态激活策略（Dock 图标显隐），`#[cfg(target_os = "macos")]` 条件编译。
7. **后端命令**：通用读写命令（方案 A）——`get_config` + `set_config(key, value)` + `get_history` + `delete_history(ids)` + `open_settings`，最少样板代码。
8. **隐藏字段**：`denoise_enabled`（废弃，忽略不改代码）、`paste_method`/`write_to_clipboard`/`overlay_position`/`remote_url`/`grpc_endpoint`（暂未定，后续再加）。

---

## 4. 架构

### 4.1 文件布局

```
crates/desktop/
├── src/
│   ├── settings_window.rs   # 新建：窗口创建 + open_settings + macOS Dock 图标 / on_settings_closed
│   ├── settings_commands.rs # 新建：get_config / set_config / get_history / delete_history / check_shortcut
│   ├── runtime_config.rs    # 扩展：RuntimeConfig 新增字段 + 暴露 build_*_options_public 供 settings 复用
│   ├── tray.rs              # 修改：托盘菜单加"设置..."项
│   └── main.rs              # 修改：注册 4 个新命令 + 托盘事件
├── dist/
│   ├── result/index.html    # 现有（不动）
│   └── settings/index.html  # 新建：3 页面 vanilla HTML（单文件内联 CSS/JS + 图标）
```

### 4.2 窗口创建与 macOS 激活策略（`settings_window.rs`）

```rust
// 单例：已存在 → set_focus；不存在 → 创建
pub fn open_settings(app_handle: tauri::AppHandle) {
    if let Some(window) = app_handle.get_webview_window("settings_window") {
        let _ = window.set_focus();
        return;
    }
    // macOS: 打开设置窗口 → Dock 显示图标
    #[cfg(target_os = "macos")]
    app_handle.set_activation_policy(tauri::ActivationPolicy::Regular);

    let _ = WebviewWindowBuilder::new(&app_handle, "settings_window", ...)
        .title("Octopus 设置")
        .inner_size(800.0, 600.0)
        .min_inner_size(640.0, 480.0)
        .decorations(true)
        .visible(true)
        .build();
}

// macOS: 设置窗口关闭后回调 — 切回仅托盘模式
#[cfg(target_os = "macos")]
pub fn on_settings_closed(app_handle: &tauri::AppHandle) {
    app_handle.set_activation_policy(tauri::ActivationPolicy::Accessory);
}
```

`main.rs` 的 `app.run()` 回调监听 `RunEvent::WindowEvent { event: Destroyed, label: "settings_window" }`，触发 `on_settings_closed`。启动时 `main.rs` 直接 `app.set_activation_policy(Accessory)`。macOS 下 `open_settings` 还调 `set_dock_icon()`——release 裸二进制无 .app bundle，Tauri 仅 debug 自动设图标，故需用 `objc2` 手动 `setApplicationIconImage`（`include_bytes!` 内嵌 `icons/icon.png`）。

### 4.3 Tauri 命令

| 命令 | 签名 | 说明 |
|---|---|---|
| `open_settings` | `() -> ()` | 创建/聚焦设置窗口（单例） |
| `get_config` | `() -> Value` | 返回全量 AppConfig（19 个展示字段 JSON）+ ASR/LLM 模型列表（DB `models` 表）+ 系统麦克风设备列表（cpal 跨平台枚举） |
| `set_config` | `(key: String, value: Value) -> Result<(), String>` | 通用写：match key → 校验类型/范围 → 写 AppConfig + RuntimeConfig（如适用）+ 持久化 config.yaml。非法值返回 `Err` |
| `get_history` | `(limit: u32, offset: u32) -> Vec<TranscriptionRecord>` | 分页读 transcriptions 表（倒序） |
| `delete_history` | `(ids: Vec<i64>) -> Result<usize, String>` | 批量删除 transcriptions（IN 子句），返回删除行数 |
| `check_shortcut` | `(shortcut: String) -> Result<(), String>` | 检测快捷键是否被占用：尝试 `on_shortcut` 注册 → 立即 `unregister`，仅做检测不持久化 |

**`get_config` 返回结构：**
```json
{
  "config": {
    "asr_engine": "local:qwen3-asr:qwen3-asr-0.6B",
    "language": "auto",
    "shortcut": "CmdOrCtrl+Shift+Space",
    "segment_silence": 400.0,
    "polish_mode": 0,
    "polish_interval": 5.0,
    "pause_polish_threshold_ms": 600,
    "polish_llm": "bigmodel:glm:glm-4-flashx",
    "asr_hardware_accelerated": false,
    "asr_correct": false,
    "output_simplified": true,
    "hide_toolbar": true,
    "denoise_mode": 1,
    "engine_mode": "embedded",
    "microphone": ""
  },
  "asr_engines": [
    {"name": "zipformer-small-ctc", "label": "本地:zipformer:zipformer-small-ctc", "current": false},
    ...
  ],
  "llm_models": [
    {"name": "glm-4-flashx", "label": "bigmodel:glm:glm-4-flashx", "current": true},
    ...
  ],
  "microphones": ["MacBook Pro 麦克风", "外接 USB 麦克风", ...]
}
```

**`set_config` 类型校验：**
```
match key {
    // 字符串枚举
    "engine_mode" => as_str() ∈ {"embedded","websocket","grpc"}
    "language" => as_str() ∈ {"auto","zh","en","ja","ko"}
    // u8 枚举
    "polish_mode" => as_u64() ∈ {0,1,2}
    "denoise_mode" => as_u64() ∈ {0,1,2}
    // bool
    "asr_hardware_accelerated" / "asr_correct" / "output_simplified" / "hide_toolbar" => as_bool()
    // f64 正数
    "segment_silence" / "polish_interval" => as_f64() > 0.0
    "pause_polish_threshold_ms" => as_f64() >= 600.0
    // string（自由）
    "shortcut" / "microphone" => as_str()
    // string（裸 model_name → 构造 3-part spec）
    "asr_engine" => build_asr_engine_spec(as_str())
    "polish_llm" => build_polish_llm_spec(as_str())
    // 非法 key
    _ => Err("未知配置字段: {key}")
}
```

**RuntimeConfig 写入：** `set_config` 成功后，如字段属于 RuntimeConfig 镜像范围（asr_engine / polish_mode / polish_llm / denoise_mode / asr_correct / output_simplified / hide_toolbar），同步更新 RuntimeConfig。

### 4.4 生效时机分类

| 时机 | 字段 | 机制 |
|---|---|---|
| **立即** | polish_mode, denoise_mode, asr_correct, output_simplified, hide_toolbar, **shortcut**（热重载：注销旧 + 注册新）, **polish_llm**（2026-06-18 改进：通过 `Command::UpdateRuntime` 同步到 coordinator config 快照，录音中改也立即生效） | 写 RuntimeConfig / 热重载 / `update_runtime`，即时生效 |
| **下次录音** | asr_engine, microphone, language, asr_hardware_accelerated, segment_silence, polish_interval, pause_polish_threshold_ms | 写 AppConfig 缓存，Coordinator Toggle 进入 Idle 时重读（asr_engine 需重建引擎实例） |
| **重启** | engine_mode | 需重启进程（引擎初始化等） |

---

## 5. 前端设计（`dist/settings/index.html`）

### 5.1 整体布局

```
┌─────────────┬──────────────────────────────────┐
│  Octopus    │                                  │
│             │                                  │
│  📋 识别记录 │         主内容区                  │
│  ⚙  系统设置 │    （随侧边栏切换）                 │
│  📦 模型管理 │                                  │
│             │                                  │
└─────────────┴──────────────────────────────────┘
  侧边栏 180px           剩余自适应
```

- **侧边栏**：固定 180px 宽，浅灰背景（`#f5f5f7`），三个导航项（图标 + 文字），当前项高亮蓝色。
- **主内容区**：白色背景，左侧边栏右有 1px 分割线，内容区可垂直滚动。
- **字体栈**：`-apple-system, "Segoe UI", "Noto Sans", sans-serif`（三端系统字体）。
- **配色**：主背景白 / 侧边栏浅灰 / 强调色蓝（`#007aff`）/ 文字深灰（`#1d1d1f`）/ 次要灰（`#86868b`）。

### 5.2 页面 1 — 识别记录

- **顶部区域**：当前正在识别的实时文本（若在录音中）。listen `update-result` 事件，显示当前 display_text。
- **工具栏**：全选 checkbox + 已选计数（"已选 N 项"）+ 删除按钮（红色边框，无选中时禁用）。全选 checkbox 支持 indeterminate 状态（部分选中时）。
- **历史列表**（倒序，最新在前）：每条记录：
  - 左侧 checkbox（选中后可批量删除）
  - 时间戳（`2026-06-17 14:30:25`）
  - **润色 text 优先显示**（`polished_text`，黑色主文本）；无润色则显示 `raw_text`
  - **原始 text 折叠隐藏**（`raw_text`，灰色次要文本）；点击"展开/折叠原始"切换
  - 元数据行：引擎名 + 润色状态 + 时长（`qwen3-asr · 已润色 · 3.2s`）
  - 右侧「拷贝」按钮：拷贝最终 text（润色优先，无润色拷贝原始）
- **删除流程**：选中记录 → 点击删除 → `confirm()` 确认 → `invoke('delete_history', { ids })` → 刷新列表（重置 offset）。
- **滚动加载**：初始加载 20 条（`get_history(20, 0)`），滚到底部加载下一页（`offset += 20`），空结果停止。

### 5.3 页面 2 — 系统设置

卡片顺序：交互 → 识别 → 润色 → 降噪 → 引擎模式。**全部无标题**（仅保留行内容）。每行控件后无独立 badge，生效时间作为灰色小字跟在 label 后面，加括号如「(立即)」「(下次录音)」「(重启)」。

**卡片「交互」（首位，无标题）：**
| 控件 | 类型 | 字段 | 生效 |
|---|---|---|---|
| 激活/关闭快捷键 | 快捷键捕获按钮（点击后捕获键盘组合，含冲突检测 `check_shortcut`） | `shortcut` | 立即 |
| 工具栏自动隐藏 | toggle switch | `hide_toolbar` | 立即 |
| 麦克风设备 | 下拉（microphones 列表） | `microphone` | 下次录音 |

**卡片「识别」（无标题）：**
| 控件 | 类型 | 字段 | 生效 |
|---|---|---|---|
| 语言识别 | 下拉（auto/zh/en） | `language` | 下次录音 |
| ASR 引擎 | 下拉（asr_engines 列表） | `asr_engine` | 下次录音 |
| 硬件加速 | toggle switch | `asr_hardware_accelerated` | 下次录音 |
| ASR 纠错 | toggle switch | `asr_correct` | 立即 |
| 简繁输出 | toggle switch（true=简体） | `output_simplified` | 立即 |
| 句间停顿 | select（300/400/500/600ms） | `segment_silence` | 下次录音 |

**卡片「润色」（无标题）：**
| 控件 | 类型 | 字段 | 生效 |
|---|---|---|---|
| 润色模式 | 下拉（关闭/仅最终/中间+最终） | `polish_mode` | 立即 |
| 润色模型 | 下拉（llm_models 列表） | `polish_llm` | 立即（2026-06-18 改进，原「下次录音」） |
| 润色间隔 | 下拉（仅最后=0/每3~8秒） | `polish_interval` | 下次录音 |
| 润色停顿阈值 | 下拉（600/700/800/900/1000ms，>= 600） | `pause_polish_threshold_ms` | 下次录音 |

**卡片「降噪」（无标题）：**
| 控件 | 类型 | 字段 | 生效 |
|---|---|---|---|
| 降噪模式 | 下拉（无/轻度/深度） | `denoise_mode` | 立即 |

**卡片「引擎模式」（无标题）：**
| 控件 | 类型 | 字段 | 生效 |
|---|---|---|---|
| 引擎接入模式 | 下拉（embedded/websocket/grpc） | `engine_mode` | 重启 |

**控件交互：**
- 改动即调 `invoke('set_config', { key, value })`。
- 成功：控件保持新值，无额外提示（静默保存）。
- 失败：toast 提示错误信息，控件回退到旧值。
- 生效时间标签：跟在 label 文字后面的灰色小字括号，如「语言识别 (下次录音)」。
- **快捷键捕获**：点击按钮 → 显示「按下快捷键…（Esc 取消）」→ 捕获组合键（`Cmd` / `Ctrl` / `Alt` / `Shift` + 主键）→ 先调 `check_shortcut` 检测冲突 → 成功保存 + 热重载（注销旧快捷键 + 注册新快捷键），失败 toast 提示。

### 5.4 页面 3 — 模型管理

占位页面：居中显示"功能开发中，敬请期待"图标 + 文字。

---

## 6. 数据流

### 6.1 打开设置窗口
```
click[工具栏设置] 或 click[托盘"设置..."]
  → invoke('open_settings')
  → Rust: get_webview_window("settings_window")
     → 已存在: set_focus()
     → 不存在: WebviewWindowBuilder 创建
```

### 6.2 前端初始化
```
settings/index.html 加载完成
  → invoke('get_config') → 渲染系统设置页面的全部控件 + 填充当前值
  → invoke('get_history', {limit:20, offset:0}) → 渲染识别记录列表
```

### 6.3 修改配置
```
用户改动控件
  → invoke('set_config', {key, value})
  → Rust: 类型校验 → 写 AppConfig 字段 → 写 RuntimeConfig（如适用）
           → write_config_yaml() 写 config.yaml
           → 如为 shortcut 字段：注销旧快捷键 + 注册新快捷键（热重载）
  → 成功: 控件保持新值
  → 失败: toast 错误 + 控件回退
```

**快捷键专用流程：**
```
用户点击快捷键按钮 → 进入捕获模式
  → 按下组合键（修饰键 + 主键）
  → invoke('check_shortcut', {shortcut})
     → Rust: 尝试 on_shortcut 注册 → 立即 unregister → 仅检测
     → 成功: 继续保存
     → 失败: toast「快捷键注册失败，可能被其他应用占用」+ 恢复原值
  → invoke('set_config', {key:'shortcut', value})
     → Rust: 注销旧快捷键 + register_shortcut(新的)
```

### 6.4 历史记录翻页
```
历史列表滚到底部
  → invoke('get_history', {limit:20, offset: 当前数量})
  → 追加到列表尾部；空结果 → 标记已加载完，不再请求
```

---

## 7. 错误处理

| 场景 | 处理 |
|---|---|
| `set_config` 类型错误（如 bool 字段传字符串） | `Err("字段 X 需要 bool 类型")`，前端 toast |
| `set_config` 值越界（如 segment_silence ≤ 0） | `Err("segment_silence 必须大于 0")`，前端 toast |
| `set_config` 未知 key | `Err("未知配置字段: {key}")`，前端 toast |
| `pause_polish_threshold_ms` < 600 | `Err("pause_polish_threshold_ms 必须 >= 600（需大于句间停顿最大值）")`，前端 toast |
| config.yaml 写失败 | `warn` log + `Err("保存失败，本次仍生效，重启后回退")` |
| `get_history` DB 错误 | 返回空数组 + `warn` log |
| 设置窗口已打开再次 `open_settings` | `set_focus` 聚焦已有窗口，不重复创建 |

---

## 8. 跨平台

- 窗口创建：`WebviewWindowBuilder` 标准 API，三端一致（`decorations:true` 各平台自动渲染原生标题栏）。
- **macOS 动态激活策略**：启动时 `Accessory`（无 Dock 图标）；`open_settings` 切 `Regular`（Dock 图标出现）；窗口 Destroyed 事件触发 `on_settings_closed` 切回 `Accessory`。`#[cfg(target_os = "macos")]` 条件编译，Windows/Linux 无此逻辑。
- 麦克风列表：后端复用现有 infra 代码（cpal 跨平台枚举设备）。
- 字体栈：`-apple-system, "Segoe UI", "Noto Sans", sans-serif`。
- 图标：拷贝按钮内联 SVG（`copy.svg`），侧边栏导航用 CSS mask。

---

## 9. 测试

### Rust 单测
- `set_config` 类型校验：合法值通过 / 非法值返回 Err（覆盖 bool / f64 / u8 / 枚举 / string 各类型）。
- `set_config` 写盘：改单字段后其他字段保留（复用 `persist_config_override` 现有测试模式）。
- `set_config` 范围校验：`pause_polish_threshold_ms >= 600`、`segment_silence > 0`。
- `get_history` 分页：limit/offset 正确切片，offset 越界返回空。
- `delete_transcriptions` 批量删除：指定 id 删除 + 空 id 列表不报错（内部函数可直连 Connection 测试）。

### 手动 e2e
- 工具栏设置按钮 / 托盘菜单均能打开设置窗。
- 设置窗单例：重复打开不创建多窗口。
- 三个页面切换正常。
- 系统设置：改 polish_mode 立即生效（边录音边看润色行为变化）。
- 系统设置：改 asr_engine 后下次录音生效。
- 系统设置：非法值（如 pause_polish_threshold_ms=100）弹出 toast 错误。
- 识别记录：历史列表正确加载、滚动翻页。
- 识别记录：润色 text 在前、原始 text 在后（折叠）。
- 识别记录：checkbox 选择 + 全选 + 删除流程。
- 识别记录：拷贝按钮拷贝最终 text。
- 识别记录：录音中实时显示当前文本。
- 模型管理：显示占位页面。
- 跨平台验证（macOS / Windows / Linux）。

---

## 10. 非目标（YAGNI）

- **模型管理页面（页面 3）**：本轮仅占位，外部模型 API 配置 + 本地模型下载后续开发。
- **隐藏字段**（`denoise_enabled` / `paste_method` / `write_to_clipboard` / `overlay_position` / `remote_url` / `grpc_endpoint`）：不在设置界面展示，后续想清楚再加。
- **config.yaml 注释保留**：`serde_yaml` 整体序列化丢注释（与 result_window toolbar 的 `persist_config_override` 一致）。
- **设置搜索**：不做设置项搜索功能。
- **多语言**：仅中文 UI。
- **识别记录搜索/过滤**：不做，仅时间倒序浏览 + 分页。
- **识别记录批量导出**：仅支持删除，不支持批量导出。

---

## 11. 相关文件清单

| 文件 | 改动 |
|---|---|
| `crates/desktop/src/settings_window.rs` | **新建**：窗口创建 + `open_settings` 命令 + macOS `set_dock_icon` / `on_settings_closed` |
| `crates/desktop/src/settings_commands.rs` | **新建**：`get_config` / `set_config`（含 `apply_config_value` 类型校验 + `sync_runtime_config` + `write_config_yaml` + shortcut 热重载）/ `get_history` / `delete_history` / `check_shortcut` 命令（独立文件避免 `runtime_config.rs` 膨胀） |
| `crates/desktop/src/runtime_config.rs` | **修改**：RuntimeConfig 新增字段（asr_correct / output_simplified / hide_toolbar）+ 暴露 `build_asr_options_public` / `build_llm_options_public` 供 settings 复用 |
| `crates/desktop/src/tray.rs` | **修改**：托盘菜单新增"设置..."项 |
| `crates/desktop/src/main.rs` | **修改**：注册 6 个命令（`open_settings` / `get_config` / `set_config` / `get_history` / `delete_history` / `check_shortcut`）+ 设置窗口模块声明 + `Destroyed` 事件回调 |
| `crates/desktop/dist/settings/index.html` | **新建**：3 页面 vanilla HTML（单文件内联 CSS/JS + 图标） |
| `crates/desktop/tauri.conf.json` | **修改**：`frontendDist` 需包含 `settings/` 目录（或确认相对路径解析） |
| `docs/architecture.md` | 补充设置窗口子系统说明 |
| `docs/configuration.md` | 补注：config.yaml 字段现可经设置界面 GUI 编辑 |


---
