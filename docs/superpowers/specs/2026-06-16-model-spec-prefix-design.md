# 模型选择 spec 设计（`PREFIX:NAME` 统一格式）

> 状态：✅ 已实现（2026-06-16）。详见 [`architecture.md`](../../../architecture.md)「模型管理」段。

## 背景

重构前 `config.yaml.asr_engine` 和 `polish_llm` 仅按 DB `models.name` 精确匹配。但 DB schema 的唯一键是 `UNIQUE(domain, name, is_local, category)`——**不同 category 下允许同名模型**（例如 `deepseek` 和 `aliyun` 两个 category 下都有 `deepseek-v4-flash`）。旧查询仅按 name 过滤，遇到同名模型时 SQLite 返回不确定行，导致取错 provider / base_url / API Key。

此外，ASR 本地引擎（`is_local=1`）与远程 API 引擎（`is_local=0`）未来可能同名，需要一种方式在配置字符串中显式区分。

## 目标

1. **统一 `asr_engine` 和 `polish_llm` 的配置格式**为 `PREFIX:NAME`，从 DB `models` 表唯一定位模型。
2. **`local` 作为特殊前缀**——映射 `is_local=true`，不对应 DB `category` 列值。
3. **其他前缀按 DB `category` 精确匹配**（如 `bigmodel`、`deepseek`、`aliyun`）。
4. **向后兼容**——不含冒号的裸名仍按 name 查询（旧行为）。
5. **ASR 与 LLM 统一语义**——同一套 `parse_model_spec` 规则服务两个 domain。

## 设计决策

### ModelSpec 枚举（`infra/src/db.rs`）

```rust
pub enum ModelSpec<'a> {
    Local(&'a str),          // "local:NAME" → is_local=true AND name
    Category(&'a str, &'a str), // "CATEGORY:NAME" → category AND name
    NameOnly(&'a str),       // "NAME" → name only（向后兼容）
}
```

`parse_model_spec(spec: &str) -> ModelSpec` 按第一个冒号分割：
- `local:` 前缀 → `Local`
- 其他前缀 → `Category`
- 无冒号 → `NameOnly`

`ModelSpec::name()` 返回去掉前缀后的裸名（生命周期绑定到原 `&str`）。

### 为什么 `local` 是特殊前缀

ASR 引擎的 category（`whisper` / `sensevoice` / `paraformer` / `qwen3-asr` / `zipformer`）是引擎**类型**分类，而 `is_local` 是**部署位置**标记。`local:zipformer-small-ctc` 比 `zipformer:zipformer-small-ctc` 更贴近用户心智（「我要本地的那个 zipformer」），且 `local` 前缀可跨 category 复用（任何 `is_local=1` 的模型都能用 `local:NAME` 命中）。

远程模型（如未来 `aliyun:zipformer-small-ctc`）直接用 category 前缀精确匹配。

### LLM 查询（`load_llm_model_at`）

按 `ModelSpec` 三分支构建不同 SQL：

| spec | SQL WHERE 子句 |
|------|---------------|
| `Local(name)` | `domain='llm' AND is_local=1 AND name=?` |
| `Category(cat, name)` | `domain='llm' AND category=? AND name=?` |
| `NameOnly(name)` | `domain='llm' AND name=?`（旧行为） |

### ASR 引擎解析（`asr::config`）

- `engine_category_from_str(s)` — DB `category` 字符串 → `EngineCategory` 枚举映射（5 个 ASR 类型；远程 category 如 `aliyun` 返回 `None`）。
- `resolve_engine_in_config(cfg, spec)` — 统一解析入口：
  - `Local` → 遍历 5 个 section，找 `is_local=true AND name` 的条目
  - `Category` → `engine_category_from_str` 映射后 `pick_entry`
  - `NameOnly` → 遍历 5 个 section 按 name 查找（旧行为）
- `resolve_engine_category(spec)` / `resolve_active_engine(spec)` 内部调用 `resolve_engine_in_config`。

### 裸名传播

下游组件（引擎缓存、流式构造器、transcribe 函数）都按**裸名**工作，不感知前缀：
- `AsrEngineManager.switch_model(spec)` — 解析 spec → 裸名做缓存键
- `StreamingSession::new(spec)` — 解析 spec → 裸名传给 `StreamingParaformer::new` / `StreamingZipformer::new`
- CLI `do_transcribe` / `run_e2e` / `stream_test` — 剥离前缀后传给各引擎 `transcribe` 函数

`ResolvedEngine.name` 始终是裸名（去掉前缀），保证缓存命中率。

## 接口

### 公开 API（`infra::db`）

```rust
pub enum ModelSpec<'a> { Local(&'a str), Category(&'a str, &'a str), NameOnly(&'a str) }

pub fn parse_model_spec(spec: &str) -> ModelSpec<'_>;
impl<'a> ModelSpec<'a> { pub fn name(&self) -> &'a str; }

pub fn load_llm_model(spec: &str) -> Result<Option<CompatibleLlmConfig>>;
```

### 公开 API（`asr::config`）

```rust
pub fn resolve_engine_in_config<'a, 'b>(cfg: &'a AsrConfig, spec: &'b str)
    -> Option<(EngineCategory, &'b str, &'a ModelEntry)>;
pub fn resolve_engine_category(spec: &str) -> Option<EngineCategory>;
pub fn resolve_active_engine(asr_engine: &str) -> Result<ResolvedEngine>;
```

## 配置示例

```yaml
# ASR 引擎
asr_engine: "local:zipformer-small-ctc"     # 本地模型（is_local=true）
# asr_engine: "zipformer:zipformer-small-ctc" # 按 category 精确匹配
# asr_engine: "zipformer-small-ctc"           # 裸名（向后兼容）

# LLM 润色
polish_llm: "bigmodel:glm-4-flashx"          # category:name
# polish_llm: "local:qwen3:8b"                # 本地 LLM（Ollama 等）
# polish_llm: "glm-4-flashx"                  # 裸名（向后兼容）
```

## 关键约束

- **向后兼容**：裸名格式（无冒号）仍按旧行为工作，老 `config.yaml` 无需修改。
- **`local` 前缀跨 category**：`local:NAME` 遍历所有 section，若多个 category 下有同名且 `is_local=true` 的模型，返回第一个匹配（按 whisper→sensevoice→paraformer→qwen3-asr→zipformer 顺序）。建议避免此情况。
- **Category 前缀仅限 ASR 已知类型**：`aliyun:zipformer-small-ctc` 中 `aliyun` 不是已知 ASR 引擎 category → `resolve_engine_in_config` 返回 `None`。远程 ASR 路由（如阿里云远程 ASR）尚未实现，当前所有 ASR 均为本地。
- **`OnceLock` 缓存不变**：手编 DB `models` 表后仍需重启进程生效。

## 影响范围

| 模块 | 变更 |
|------|------|
| `infra/src/db.rs` | 新增 `ModelSpec` + `parse_model_spec`；`load_llm_model_at` 按 spec 三分支查询 |
| `asr/src/config.rs` | 新增 `engine_category_from_str` / `all_sections` / `resolve_engine_in_config`；`resolve_engine_category` / `resolve_active_engine` 改走 spec 解析 |
| `asr/src/engine.rs` | `switch_model` 解析 spec → 裸名缓存 |
| `asr/src/streaming_engine.rs` | `StreamingSession::new` 解析 spec → 裸名传构造器 |
| `cli/src/main.rs` | `do_transcribe` / `run_e2e` / `stream_test` 剥离前缀 |
| `infra/src/config.rs` | `polish_llm` 默认值 `glm-4-flashx` → `bigmodel:glm-4-flashx` |
| `docs/configuration.md` | 新增「模型选择 spec」节 + 表格行更新 |
