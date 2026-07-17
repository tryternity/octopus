# 云端翻译引擎接入设计

> 2026-07-17 · 接入 OpenAI / DeepSeek / Moonshot / 智谱 / 百炼 5 家云端翻译模型，统一走 `TranslationEngine` trait

## 1. 背景与目标

### 1.1 现状

octopus 已有翻译能力，但存在两个问题：

1. **云端翻译是硬编码 fallback**——`action_bar_commands.rs::do_translate` 里一条独立分支，复用润色 LLM（`polish_llm` 配置），与 `TranslationEngine` trait 完全脱钩。用户无法选择用哪个云端模型翻译。
2. **`TranslationEngine` trait 仅同步**——只有 2 个本地实现（m2m100 / opus-mt），无云端实现。trait 缓存用 `Arc<dyn TranslationEngine>`（对象安全）。

### 1.2 目标

- 新增 OpenAI / DeepSeek / Moonshot / 智谱 GLM / 阿里云百炼 5 家云端翻译模型
- 云端翻译统一实现 `TranslationEngine` trait（与本地引擎同构）
- 通过 DB 激活机制选择翻译引擎（本地或云端，最多激活一个）
- 无激活翻译模型时 fallback 到 `polish_llm`（润色模型）

### 1.3 参考实现

CopyTranslator（`/Users/wudarui/workspace/agent/CopyTranslator`）的 `openai.ts`：所有 OpenAI 兼容服务商共用一个客户端，仅靠 `apiBase` + `model` 区分。本设计借鉴其统一客户端思路 + prompt 模板，但落地到 Rust trait 体系。

## 2. 核心决策（brainstorming 已确认）

| 决策点 | 选择 | 理由 |
|---|---|---|
| trait async | 改为 async | 云端 HTTP 天然异步，语义清晰 |
| async dyn 实现 | `#[async_trait]` 宏 | 现有 `Arc<dyn TranslationEngine>` 缓存零改动 |
| spec 编码 | `translate_engine` 存激活模型 DB 行 id | 本地/云端统一，id 为空或非法 → fallback |
| 旧值迁移 | 不迁移，降级 fallback | 开发期用户少，旧值 `"local:opus-mt"` 静默降级可接受 |
| fallback LLM | 复用 `polish_llm`，不新增字段 | 少一个配置项，简洁 |
| 云端实现位置 | `octopus-translation` crate 新增 `cloud.rs`，依赖 `octopus-llm` | 所有翻译引擎统一在一个 crate |
| token 流式 | 不做 | 当前段落级伪流式够用 |
| 语言对 | 保持中英双向 | 本 spec 聚焦云端接入，不扩多语言对 |

## 3. 架构设计

### 3.1 TranslationEngine trait async 化

`crates/translation/src/engine.rs`：

```rust
use async_trait::async_trait;

#[async_trait]
pub trait TranslationEngine: Send + Sync {
    async fn translate(&self, text: &str, source_lang: &str, target_lang: &str) -> Result<String>;
    fn name(&self) -> &str;
}
```

- 缓存 `type EngineCache = parking_lot::Mutex<HashMap<String, Arc<dyn TranslationEngine>>>;` 结构不变（async_trait 自动把 Future box 成 `Pin<Box<dyn Future + Send>>`，对象安全）。
- 本地 2 个实现（`M2M100Engine` / `OpusMTEngine`）加 `#[async_trait]`，translate 函数体不变（同步推理逻辑直接包在 async fn 里——本地推理是 CPU 密集，在调用方 `spawn_blocking` 线程里执行）。
- `cached_engine` / `load_opus_mt` 改为 async（内部可能需 await 加载云端引擎）。

### 3.2 云端引擎 CloudLlmEngine

**新增文件** `crates/translation/src/cloud.rs`：

```rust
use async_trait::async_trait;
use crate::engine::TranslationEngine;

/// 云端 LLM 翻译引擎——OpenAI 兼容协议，覆盖 OpenAI/DeepSeek/Moonshot/智谱/百炼。
/// 差异仅在 DB models 行的 provider/source（base_url）/secret_key（api_key）/model_name。
pub struct CloudLlmEngine {
    config: octopus_llm::CompatibleLlmConfig,
    name: String,
}

impl CloudLlmEngine {
    /// 从 DB models 行（domain='translate', is_local=0）构造。
    pub fn from_db_row(row: &ModelRow) -> Result<Self> { ... }
}

#[async_trait]
impl TranslationEngine for CloudLlmEngine {
    async fn translate(&self, text: &str, source_lang: &str, target_lang: &str) -> Result<String> {
        let prompt = build_translate_prompt(source_lang, target_lang);
        // 复用 octopus-llm::client（reqwest::blocking）——在 async fn 里直接调，
        // 由调用方 spawn_blocking 隔离（与现有 do_translate_streaming 同模式）。
        octopus_llm::chat_text_with_prompt(&prompt, text, &self.config, None)
            .map_err(|e| anyhow::anyhow!(e.to_string()))
    }

    fn name(&self) -> &str { &self.name }
}
```

**Prompt 构造**（参考 CopyTranslator，语言代码用英文全称增强 LLM 理解）：

```rust
fn build_translate_prompt(source_lang: &str, target_lang: &str) -> String {
    // 现有中英双向检测：zh→en / 否则 en→zh
    // source_lang/target_lang 传 "zh"/"en"，映射成英文全称
    let from = if source_lang == "zh" { "Chinese" } else { "English" };
    let to = if target_lang == "zh" { "Chinese" } else { "English" };
    format!(
        "Translate the following text from {} to {}. Only output the translation, without any explanation.",
        from, to
    )
}
```

### 3.3 translate_engine 配置语义变更

`crates/infra/src/config.rs`：

```rust
/// 翻译引擎：激活的 models 表行 id（domain='translate'）。
/// "" = 未激活，fallback 到 polish_llm。
/// 旧值 "local:opus-mt" / "llm" 不迁移，解析为非法 id → fallback。
#[serde(default)]
pub translate_engine: String,
```

**不新增 `translate_llm` 字段**——fallback 直接用 `polish_llm`。

### 3.4 策略调度改造

`crates/desktop/src/action_bar_commands.rs`：

```rust
enum TranslateStrategy {
    Engine(Arc<dyn TranslationEngine>),  // 本地或云端（已加载的引擎实例）
    FallbackLlm,                          // 用 polish_llm
}

async fn resolve_translate_strategy(config: &AppConfig) -> TranslateStrategy {
    // 1. parse translate_engine 为 i64，空/非法 → FallbackLlm
    let Ok(id) = config.translate_engine.parse::<i64>() else {
        return TranslateStrategy::FallbackLlm;
    };
    // 2. 查 DB models 行（domain='translate'），不存在/未启用 → FallbackLlm
    let Ok(Some(row)) = octopus_infra::db::get_translate_model_by_id(id) else {
        return TranslateStrategy::FallbackLlm;
    };
    // 3. 按 is_local 分流加载引擎
    let engine = if row.is_local {
        octopus_translation::load_local_engine(&row).await.ok()
    } else {
        Some(Arc::new(CloudLlmEngine::from_db_row(&row)?) as Arc<dyn TranslationEngine>)
    };
    match engine {
        Some(e) => TranslateStrategy::Engine(e),
        None => TranslateStrategy::FallbackLlm,
    }
}
```

### 3.5 调用链 async 化

`do_translate` / `do_translate_streaming` 改为 async：

```rust
async fn do_translate(text: &str, config: &AppConfig) -> Result<String, String> {
    let (source_lang, target_lang) = detect_translate_direction(text);  // 不变
    match resolve_translate_strategy(config).await {
        TranslateStrategy::Engine(engine) => {
            engine.translate(text, source_lang, target_lang).await
                .map_err(|e| e.to_string())
        }
        TranslateStrategy::FallbackLlm => {
            let llm_config = crate::config::llm_config_ignore_mode(config)
                .ok_or("翻译 fallback LLM 未配置")?;
            let prompt = auto_translate_prompt(text);  // 保留现有硬编码 prompt
            octopus_llm::chat_text_with_prompt(prompt, text, &llm_config, None)
                .map_err(|e| e.to_string())
        }
    }
}
```

**调用方适配**：现有 `do_translate_streaming` / `translate_text` 命令已在 worker 线程（`std::thread::spawn`），async 用 `tokio::runtime::Runtime::new().block_on()` 或改造为 tokio task。具体：`translate_text` 命令里 `std::thread::spawn` 内用 `Runtime::block_on(do_translate_streaming(...))`。

### 3.6 provider 预设补 Moonshot（app_config 表）

**不在 models 表 seed 云端翻译模型**——云端模型必须用户填 secret_key（API key）才能用，seed 进去也是空壳无效。models 表的翻译云端行由用户在 TranslateTab 的「添加模型」UI 手动创建（`add_cloud_model` 命令）。

provider 下拉的来源是 `app_config` 表 `category='llm_provider'`（`db.sql:247-251`），含每家的 base_url + 推荐模型列表。这个 provider 预设表**跨 llm/translate domain 共享**——translate 的 CloudModelForm 复用同一个 `list_llm_provider_presets` 命令。

现有 5 家（deepseek/aliyun/bigmodel/openai/ollama）缺 **moonshot**（=Kimi）和 **minimax**，补两行：

```sql
-- db.sql app_config seed，在 ollama 行后追加
('moonshot', '{"base_url":"https://api.moonshot.cn/v1","models":["moonshot-v1-8k","moonshot-v1-32k","moonshot-v1-128k"]}', 'Moonshot/Kimi', 'llm_provider'),
('minimax', '{"base_url":"https://api.minimaxi.com/v1","models":["MiniMax-M3"]}', 'MiniMax', 'llm_provider'),
```

- moonshot 即 Kimi（Moonshot AI 的产品），API 域名 `api.moonshot.cn`
- minimax 国内端点 `api.minimaxi.com/v1`，模型 MiniMax-M3

这样 llm 和 translate 的 provider 下拉都会有这两家选项（用户填 key 后即可用于润色或翻译）。

### 3.7 前端 TranslateTab 加云端 section

参考 `LlmTab.tsx` 模式（已有完整云端模型 CRUD UI）：

```
TranslateTab
├── 本地模型 section（现有，下载/激活 m2m100 + opus-mt）
└── 云端模型 section（新增）
    ├── 模型列表（数据源：get_config 返回 translate_cloud_models，或新 list 命令）
    ├── 「添加模型」按钮 → CloudModelForm（复用，domain 扩 "translate"）
    └── 每行：激活/编辑/删除
```

**激活操作**：`set_config({key: "translate_engine", value: String(model.id)})`——本地/云端统一用 DB id。

**CloudModelForm 改造**：`domain` 类型从 `"asr" | "llm"` 扩为 `"asr" | "llm" | "translate"`。translate 分支复用 llm 的 provider→base_url 自动填充逻辑（provider 列表加 moonshot）。

### 3.8 连接测试

`crates/desktop/src/model_commands.rs::add_cloud_model` 现仅 `domain=="llm"` 时做连接测试（第 402-407 行）。扩展为 `domain=="llm" || domain=="translate"` 都测试——translate 测试用翻译 prompt 跑一次短文本翻译。

## 4. 数据流

```
用户在 TranslateTab 激活云端模型（如 deepseek-chat）
  → set_config({key:"translate_engine", value:"<model.id>"})
  → 触发翻译（ActionBar 翻译菜单项 / translate_text 命令）
  → do_translate_streaming(text) [worker thread, async runtime]
    → resolve_translate_strategy(config)
      → parse translate_engine = model.id
      → db::get_translate_model_by_id(id) → row (is_local=0, provider=deepseek)
      → CloudLlmEngine::from_db_row(row) → Arc<dyn TranslationEngine>
    → engine.translate(text, "zh"/"en", "en"/"zh").await
      → build_translate_prompt
      → octopus_llm::chat_text_with_prompt(prompt, text, config, None)
      → reqwest::blocking POST {base_url}/chat/completions
      → choices[0].message.content.trim()
    → emit translate-progress/done（事件链不变）
```

## 5. 不变量

1. **润色/ASR 零影响**——polish_llm 配置和调用链不动，ASR 引擎不动
2. **翻译事件链不变**——`translate-progress`/`translate-done` + `compact-editor://translate-*` + `TRANSLATE_RESULTS` 竞态缓存机制保留
3. **本地翻译引擎行为不变**——仅加 async 包装，推理逻辑零改动
4. **语言检测不变**——CJK 启发式中英双向（`detect_translate_direction` 不动）
5. **旧配置不迁移**——`"local:opus-mt"` / `"llm"` / `""` 在新逻辑下都走 fallback（前两者解析为非法 id，后者本就是空）

## 6. 降级路径

| 场景 | 行为 |
|---|---|
| `translate_engine` 为空 | fallback `polish_llm` |
| `translate_engine` 是非法 id（含旧值） | parse 失败 → fallback `polish_llm` |
| DB 无对应 id 的行 | fallback `polish_llm` |
| 云端模型 secret_key 为空 | 引擎构造失败 → fallback `polish_llm` |
| 云端 API 调用失败（网络/认证/超时） | 返回错误，前端 toast 提示（不 fallback） |
| `polish_llm` 也未配置 | 返回错误「翻译 fallback LLM 未配置」 |

## 7. 影响面

### 7.1 需改造的文件

| 文件 | 改动 |
|---|---|
| `crates/translation/Cargo.toml` | 加 `async-trait` + `octopus-llm` 依赖 |
| `crates/translation/src/engine.rs` | trait 加 `#[async_trait]`，缓存加载函数改 async |
| `crates/translation/src/m2m100.rs` | 加 `#[async_trait]` |
| `crates/translation/src/opus_mt.rs` | 加 `#[async_trait]` |
| `crates/translation/src/cloud.rs` | **新增** `CloudLlmEngine` + `build_translate_prompt` |
| `crates/translation/src/lib.rs` | 导出 `CloudLlmEngine` + 新加载函数 |
| `crates/translation/src/discovery.rs` | 加云端模型发现（列 `domain='translate' AND is_local=0`） |
| `crates/infra/src/config.rs` | `translate_engine` 注释更新（无新字段） |
| `crates/infra/src/db.sql` | 加 moonshot provider 预设（app_config llm_provider category）|
| `crates/infra/src/db.rs` | 加 `get_translate_model_by_id`（或参数化现有函数） |
| `crates/desktop/src/action_bar_commands.rs` | `TranslateStrategy` 重构 + `do_translate` async + 调用方 block_on |
| `crates/desktop/src/translation_commands.rs` | `translate_status` 适配新策略 |
| `crates/desktop/src/settings_commands.rs` | `translate_engine` 写入校验（可选：校验 id 合法性） |
| `crates/desktop/src/model_commands.rs` | 连接测试扩展到 translate domain |
| `crates/desktop/frontend/.../TranslateTab.tsx` | 加云端模型 section |
| `crates/desktop/frontend/.../CloudModelForm.tsx` | domain 类型扩 `"translate"` + moonshot provider |
| `crates/desktop/frontend/src/locales/*.yaml` | 翻译云端相关 i18n key（参考 llm 的） |

### 7.2 不受影响

- ASR 引擎、VAD、录音流程
- 润色（polish）调用链
- 剪贴板、命令面板、热词
- DB schema 版本（models 表已有 `domain='translate'`，云端翻译行由用户 UI 添加，不 seed；仅 app_config 的 llm_provider 补 moonshot 预设，不需 schema 迁移）

## 8. 验证策略

1. **单元测试**：`build_translate_prompt` 输出格式、`translate_engine` id parse 边界、fallback 决策逻辑
2. **编译验证**：trait async 改造后所有翻译调用链编译通过（0 error 0 warning）
3. **集成测试**：mock 云端 API 验证 CloudLlmEngine 请求构造（URL/headers/body）和响应解析
4. **端到端**（手动）：设置页激活 deepseek 翻译模型 + 填 key → ActionBar 翻译 → 验证结果；清空 translate_engine → 验证 fallback 到 polish_llm
5. **回归**：本地翻译（m2m100/opus-mt）激活后翻译正常（仅 async 包装，行为不变）

## 9. 未来扩展（不在本 spec 范围）

- 多语言对支持（源/目标语言可选，非仅中英）
- token 级流式翻译（需 `octopus-llm` 加 SSE 支持）
- 翻译质量对比（多引擎并排）
- Ollama 本地 LLM 翻译（走同一 CloudLlmEngine，base_url 指向 localhost）
