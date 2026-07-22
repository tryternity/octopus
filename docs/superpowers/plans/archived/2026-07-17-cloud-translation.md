# 云端翻译引擎接入 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 接入 OpenAI / DeepSeek / Moonshot / 智谱 / 百炼 / MiniMax 云端翻译模型，统一走 `TranslationEngine` trait，通过 DB 激活选择引擎，无激活时 fallback 到 `polish_llm`。

**Architecture:** `TranslationEngine` trait 改 async（`#[async_trait]`）；新增 `CloudLlmEngine`（复用 `octopus-llm::client`，5+1 家服务商靠 DB 行的 base_url/api_key/model 区分）；`translate_engine` 配置语义从 spec 字符串改为 DB 行 id；`do_translate` 调用链 async 化（调用方在 worker 线程用 Runtime::block_on）。

**Tech Stack:** Rust + `async-trait` + `reqwest::blocking`（复用现有 llm client）+ Tauri + React/TypeScript

**Spec:** [2026-07-17-cloud-translation-design.md](../specs/2026-07-17-cloud-translation-design.md)

## Global Constraints

- 所有翻译引擎（本地 + 云端）统一实现 `TranslationEngine` trait（async）
- `translate_engine` 存激活模型 DB 行 id（`domain='translate'`），空/非法 → fallback `polish_llm`
- 不新增 `translate_llm` 配置字段，fallback 直接用 `polish_llm`
- 不在 models 表 seed 云端翻译（用户 UI 手动添加，需填 secret_key）
- 不迁移旧 `translate_engine` 值（`"local:opus-mt"` 等降级 fallback）
- 不做 token 级流式，语言对保持中英双向
- 翻译事件链不变（`translate-progress/done` + `compact-editor://translate-*` + 竞态缓存）
- 润色/ASR 零影响

---

## File Structure

| 文件 | 责任 |
|---|---|
| `crates/translation/Cargo.toml` | 加 `async-trait` + `octopus-llm` 依赖 |
| `crates/translation/src/engine.rs` | trait async 化 + 缓存加载函数 async |
| `crates/translation/src/m2m100.rs` | `#[async_trait]` |
| `crates/translation/src/opus_mt.rs` | `#[async_trait]` |
| `crates/translation/src/cloud.rs` | **新增** `CloudLlmEngine` + prompt 构造 |
| `crates/translation/src/discovery.rs` | 加云端模型发现 |
| `crates/translation/src/lib.rs` | 导出新 API |
| `crates/infra/src/db.rs` | 加 `get_translate_model_by_id` + `ModelRow` |
| `crates/infra/src/db.sql` | app_config 补 moonshot + minimax provider 预设 |
| `crates/infra/src/config.rs` | `translate_engine` 注释更新 |
| `crates/desktop/src/action_bar_commands.rs` | strategy 重构 + `do_translate` async + 调用方 block_on |
| `crates/desktop/src/translation_commands.rs` | `translate_status` 适配 |
| `crates/desktop/src/model_commands.rs` | 连接测试支持 translate domain |
| `crates/desktop/frontend/.../TranslateTab.tsx` | 加云端模型 section |
| `crates/desktop/frontend/.../CloudModelForm.tsx` | domain 扩 translate |
| `crates/desktop/frontend/src/locales/*.yaml` | i18n |

---

## Task 1: translation crate trait async 化

**Files:**
- Modify: `crates/translation/Cargo.toml`
- Modify: `crates/translation/src/engine.rs`
- Modify: `crates/translation/src/m2m100.rs`
- Modify: `crates/translation/src/opus_mt.rs`

**Interfaces:**
- Produces: `#[async_trait] pub trait TranslationEngine { async fn translate(&self, text: &str, source_lang: &str, target_lang: &str) -> Result<String>; fn name(&self) -> &str; }`（签名变 async，方法名不变）

- [x] **Step 1: Cargo.toml 加 async-trait 依赖**

```toml
# crates/translation/Cargo.toml [dependencies] 段追加
async-trait = "0.1"
```

- [x] **Step 2: engine.rs trait 加 async_trait**

把 `crates/translation/src/engine.rs` 的 trait 定义改为：

```rust
use anyhow::Result;
use async_trait::async_trait;
use std::collections::HashMap;
use std::sync::{Arc, OnceLock};

/// 翻译引擎 trait（本地 + 云端统一）。async 以支持云端 HTTP 调用。
#[async_trait]
pub trait TranslationEngine: Send + Sync {
    async fn translate(&self, text: &str, source_lang: &str, target_lang: &str) -> Result<String>;
    fn name(&self) -> &str;
}
```

注意：`cached_engine` 和 `load_opus_mt` 暂时保持同步签名（本地引擎加载不含 IO 等待，`ort` 加载是同步的），仅 trait 方法变 async。缓存类型 `HashMap<String, Arc<dyn TranslationEngine>>` 不变（async_trait 自动 box future）。

- [x] **Step 3: m2m100.rs 加 async_trait**

在 `crates/translation/src/m2m100.rs` 的 `impl TranslationEngine for M2M100Engine` 块上加 `#[async_trait]`，并把 `fn translate` 改为 `async fn translate`（函数体不变）：

```rust
#[async_trait::async_trait]
impl TranslationEngine for M2M100Engine {
    async fn translate(&self, text: &str, source_lang: &str, target_lang: &str) -> Result<String> {
        // 原函数体保持不变
    }
    fn name(&self) -> &str { "m2m100-418M" }
}
```

- [x] **Step 4: opus_mt.rs 加 async_trait**

同 Step 3，在 `crates/translation/src/opus_mt.rs` 的 impl 块加 `#[async_trait::async_trait]`，`fn translate` → `async fn translate`。

- [x] **Step 5: 编译 translation crate**

Run: `cargo build -p octopus-translation`
Expected: 0 error。若报 `do_translate` 调用方 `.await` 缺失——这是预期的（desktop crate 还没改），此时只确认 translation crate 自身编译通过（`cargo build -p octopus-translation` 不依赖 desktop）。

- [x] **Step 6: Commit**

```bash
git add crates/translation/
git commit -m "refactor(translation): TranslationEngine trait async 化（#[async_trait]）"
```

---

## Task 2: 新增 CloudLlmEngine（云端翻译引擎）

**Files:**
- Modify: `crates/translation/Cargo.toml`（加 octopus-llm 依赖）
- Create: `crates/translation/src/cloud.rs`
- Modify: `crates/translation/src/lib.rs`

**Interfaces:**
- Consumes: `octopus_llm::chat_text_with_prompt(system, user, config, timeout) -> Result<String>`、`octopus_infra::db::CompatibleLlmConfig`
- Produces: `pub struct CloudLlmEngine { ... }`、`impl TranslationEngine for CloudLlmEngine`、`CloudLlmEngine::new(provider, model, base_url, secret_key, is_thinking)`、`fn build_translate_prompt(source_lang, target_lang) -> String`

- [x] **Step 1: Cargo.toml 加 octopus-llm 依赖**

```toml
# crates/translation/Cargo.toml [dependencies] 段追加
octopus-llm = { path = "../llm" }
```

- [x] **Step 2: 创建 cloud.rs**

创建 `crates/translation/src/cloud.rs`：

```rust
//! 云端 LLM 翻译引擎——OpenAI 兼容协议。
//! 覆盖 OpenAI/DeepSeek/Moonshot/智谱/百炼/MiniMax，差异仅在 DB models 行的
//! provider/source(base_url)/secret_key(api_key)/model_name。
//! 复用 octopus-llm::client 的 reqwest::blocking HTTP 客户端。

use crate::engine::TranslationEngine;
use anyhow::Result;
use async_trait::async_trait;

/// 云端 LLM 翻译引擎。
pub struct CloudLlmEngine {
    config: octopus_llm::CompatibleLlmConfig,
    name: String,
}

impl CloudLlmEngine {
    /// 从 DB models 行字段构造。
    /// is_thinking 模型翻译时会被 octopus-llm 自动关闭思考（needs_disable_thinking）。
    pub fn new(
        provider: &str,
        model: &str,
        base_url: &str,
        secret_key: &str,
        is_thinking: bool,
    ) -> Self {
        Self {
            config: octopus_llm::CompatibleLlmConfig {
                provider: provider.to_string(),
                model: model.to_string(),
                base_url: base_url.to_string(),
                secret_key: secret_key.to_string(),
                is_thinking,
                is_local: false,
                is_enabled: true,
            },
            name: format!("{}:{}", provider, model),
        }
    }
}

#[async_trait]
impl TranslationEngine for CloudLlmEngine {
    async fn translate(&self, text: &str, source_lang: &str, target_lang: &str) -> Result<String> {
        let prompt = build_translate_prompt(source_lang, target_lang);
        // 复用 octopus-llm::client（reqwest::blocking）。
        // 在 async fn 里直接调 blocking——由调用方 spawn_blocking 隔离
        // （与现有 do_translate_streaming 在 worker 线程执行同模式）。
        octopus_llm::chat_text_with_prompt(&prompt, text, &self.config, None)
            .map_err(|e| anyhow::anyhow!("云端翻译失败: {}", e))
    }

    fn name(&self) -> &str {
        &self.name
    }
}

/// 构造翻译 prompt。语言代码（"zh"/"en"）映射成英文全称增强 LLM 理解。
/// 参考 CopyTranslator openai.ts 的 prompt 设计。
pub fn build_translate_prompt(source_lang: &str, target_lang: &str) -> String {
    let from = lang_to_english(source_lang);
    let to = lang_to_english(target_lang);
    format!(
        "Translate the following text from {} to {}. Only output the translation, without any explanation or extra text.",
        from, to
    )
}

fn lang_to_english(lang: &str) -> &'static str {
    match lang {
        "zh" => "Chinese",
        "en" => "English",
        "ja" => "Japanese",
        "ko" => "Korean",
        _ => "the original language",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_translate_prompt_zh_to_en() {
        let p = build_translate_prompt("zh", "en");
        assert!(p.contains("from Chinese to English"));
        assert!(p.contains("Only output the translation"));
    }

    #[test]
    fn test_build_translate_prompt_en_to_zh() {
        let p = build_translate_prompt("en", "zh");
        assert!(p.contains("from English to Chinese"));
    }

    #[test]
    fn test_cloud_engine_name() {
        let e = CloudLlmEngine::new("deepseek", "deepseek-chat", "https://api.deepseek.com", "sk-test", false);
        assert_eq!(e.name(), "deepseek:deepseek-chat");
    }
}
```

- [x] **Step 3: lib.rs 注册 cloud 模块**

在 `crates/translation/src/lib.rs` 加：

```rust
pub mod cloud;

pub use cloud::CloudLlmEngine;
```

（在现有 `pub mod` 块和 `pub use` 块对应位置追加）

- [x] **Step 4: 运行测试**

Run: `cargo test -p octopus-translation --lib`
Expected: 3 tests passed（新增的 prompt + name 测试）。

- [x] **Step 5: 编译**

Run: `cargo build -p octopus-translation`
Expected: 0 error 0 warning。

- [x] **Step 6: Commit**

```bash
git add crates/translation/
git commit -m "feat(translation): 新增 CloudLlmEngine——OpenAI 兼容云端翻译引擎"
```

---

## Task 3: infra 层 DB 函数 + provider 预设

**Files:**
- Modify: `crates/infra/src/db.rs`
- Modify: `crates/infra/src/db.sql`
- Modify: `crates/infra/src/config.rs`

**Interfaces:**
- Consumes: 现有 `models` 表 schema（`domain='translate'`）
- Produces: `pub struct ModelRow { id, domain, provider, category, model_name, source, secret_key, is_local, is_thinking, is_streaming, is_enabled }`、`pub fn get_translate_model_by_id(id: i64) -> Result<Option<ModelRow>>`

- [x] **Step 1: db.rs 加 ModelRow 结构 + 查询函数**

在 `crates/infra/src/db.rs` 加（放在 `load_llm_model` 附近）：

```rust
/// models 表的通用行（用于翻译引擎按 id 查询，不限于 llm domain）。
#[derive(Debug, Clone)]
pub struct ModelRow {
    pub id: i64,
    pub domain: String,
    pub provider: String,
    pub category: String,
    pub model_name: String,
    pub source: String,
    pub secret_key: String,
    pub is_local: bool,
    pub is_thinking: bool,
    pub is_streaming: bool,
    pub is_enabled: bool,
}

/// 按 id 查询 models 表行（不限 domain）。用于 translate_engine 存 DB id 时反查引擎。
pub fn get_model_by_id(id: i64) -> Result<Option<ModelRow>> {
    with_db(|conn| {
        let mut stmt = conn.prepare(
            "SELECT id, domain, provider, category, model_name, source, secret_key,
                    is_local, is_thinking, is_streaming, is_enabled
             FROM models WHERE id = ?1",
        )?;
        let row = stmt.query_row(params![id], |r| {
            Ok(ModelRow {
                id: r.get(0)?,
                domain: r.get(1)?,
                provider: r.get(2)?,
                category: r.get(3)?,
                model_name: r.get(4)?,
                source: r.get(5)?,
                secret_key: r.get(6)?,
                is_local: r.get::<_, i64>(7)? != 0,
                is_thinking: r.get::<_, i64>(8)? != 0,
                is_streaming: r.get::<_, i64>(9)? != 0,
                is_enabled: r.get::<_, i64>(10)? != 0,
            })
        }).optional()?;
        Ok(row)
    })
}
```

注意：`optional()` 来自 `rusqlite::OptionalExtension`，确认 db.rs 顶部已 `use rusqlite::OptionalExtension;`（现有代码已大量用 `.optional()`）。

- [x] **Step 2: db.rs 加测试**

在 db.rs 的 `#[cfg(test)] mod tests` 里加：

```rust
#[test]
fn test_get_model_by_id() {
    init_test_db();
    // seed 已有 translate domain 的 opus-mt 行，查它的 id
    let row = db::list_local_models_by_domain("translate").unwrap();
    let first = row.first().expect("seed 应有 translate 本地模型");
    let id = first.id; // 需确认 list_local_models_by_domain 返回类型含 id
    let got = db::get_model_by_id(id).unwrap().expect("应查到");
    assert_eq!(got.domain, "translate");
    assert!(got.is_local);
}
```

注：若 `list_local_models_by_domain` 返回类型不含 id，直接用 `get_model_by_id(1)` 测（id=1 是 auto increment 第一行）。先 grep 确认。

- [x] **Step 3: db.sql 补 moonshot + minimax provider 预设**

在 `crates/infra/src/db.sql` 的 app_config seed 里，ollama 行（第 251 行）后追加：

```sql
        ('moonshot', '{"base_url":"https://api.moonshot.cn/v1","models":["moonshot-v1-8k","moonshot-v1-32k","moonshot-v1-128k"]}', 'Moonshot/Kimi', 'llm_provider'),
        ('minimax', '{"base_url":"https://api.minimaxi.com/v1","models":["MiniMax-M3"]}', 'MiniMax', 'llm_provider');
```

注意 ollama 行末尾的 `;` 要改成 `,`（因为后面还有行）。

- [x] **Step 4: config.rs 更新 translate_engine 注释**

`crates/infra/src/config.rs:223-225` 改注释：

```rust
/// 翻译引擎：激活的 models 表行 id（domain='translate'）。
/// "" = 未激活 → fallback polish_llm；旧值 "local:xxx"/"llm" 不迁移 → fallback。
#[serde(default)]
pub translate_engine: String,
```

- [x] **Step 5: 编译 + 测试**

Run: `cargo test -p octopus-infra --lib`
Expected: 全过（含新 `test_get_model_by_id`）。

Run: `cargo build -p octopus-infra`
Expected: 0 error 0 warning。

- [x] **Step 6: Commit**

```bash
git add crates/infra/
git commit -m "feat(infra): get_model_by_id + moonshot/minimax provider 预设"
```

---

## Task 4: 策略调度 + do_translate async 化

**Files:**
- Modify: `crates/desktop/src/action_bar_commands.rs`

**Interfaces:**
- Consumes: Task 1 的 async `TranslationEngine`、Task 2 的 `CloudLlmEngine`、Task 3 的 `get_model_by_id` + `ModelRow`
- Produces: `async fn do_translate(...)`、新 `TranslateStrategy` 枚举

这是核心改造任务。先改 strategy + do_translate，再改两个调用点。

- [x] **Step 1: 重构 TranslateStrategy 枚举**

把 `crates/desktop/src/action_bar_commands.rs:802-806` 的枚举改为：

```rust
use std::sync::Arc;
use octopus_translation::TranslationEngine;

enum TranslateStrategy {
    /// 已加载的引擎实例（本地或云端）。
    Engine(Arc<dyn TranslationEngine>),
    /// 无激活翻译模型 → fallback polish_llm。
    FallbackLlm,
}
```

- [x] **Step 2: 重写 resolve_translate_strategy 为 async**

替换 `crates/desktop/src/action_bar_commands.rs:808-824`（原 `resolve_translate_strategy` 函数）：

```rust
async fn resolve_translate_strategy(config: &octopus_infra::config::AppConfig) -> TranslateStrategy {
    // translate_engine 存激活模型 DB id；空/非法/不存在 → fallback
    let Ok(id) = config.translate_engine.parse::<i64>() else {
        return TranslateStrategy::FallbackLlm;
    };
    let Ok(Some(row)) = octopus_infra::db::get_model_by_id(id) else {
        return TranslateStrategy::FallbackLlm;
    };
    if row.domain != "translate" || !row.is_enabled {
        return TranslateStrategy::FallbackLlm;
    }
    let (source_lang, target_lang) = detect_translate_direction(""); // 仅用于 opus-mt 方向
    let engine: Option<Arc<dyn TranslationEngine>> = if row.is_local {
        // 本地引擎：按 model_name 分流
        match row.model_name.as_str() {
            name if name.starts_with("opus-mt") => {
                octopus_translation::load_opus_mt("zh", "en") // 默认方向，实际翻译时按文本重载
                    .ok().map(|e| e as Arc<dyn TranslationEngine>)
            }
            _ => { // m2m100 等
                octopus_translation::TranslationManager::new(&format!("local:{}", row.model_name))
                    .engine().ok().flatten()
            }
        }
    } else {
        // 云端引擎
        if row.secret_key.is_empty() {
            None // 未填 key → fallback
        } else {
            Some(Arc::new(octopus_translation::CloudLlmEngine::new(
                &row.provider, &row.model_name, &row.source, &row.secret_key, row.is_thinking,
            )) as Arc<dyn TranslationEngine>)
        }
    };
    match engine {
        Some(e) => TranslateStrategy::Engine(e),
        None => TranslateStrategy::FallbackLlm,
    }
}
```

注意：opus-mt 方向问题——`resolve_translate_strategy` 无文本输入无法判断方向。改为在 `do_translate` 里对 opus-mt 特殊处理（见 Step 3）。简化：strategy 里只判断"能否加载引擎"，opus-mt 统一用 zh-en 兜底加载（实际 translate 时 opus_mt.rs 内部会按 source/target 重载）。

实际上更干净的做法：**strategy 不预加载引擎，只决定路径**。重写为：

```rust
enum TranslateStrategy {
    LocalModel { row: octopus_infra::db::ModelRow },
    CloudModel { row: octopus_infra::db::ModelRow },
    FallbackLlm,
}

async fn resolve_translate_strategy(config: &octopus_infra::config::AppConfig) -> TranslateStrategy {
    let Ok(id) = config.translate_engine.parse::<i64>() else {
        return TranslateStrategy::FallbackLlm;
    };
    let Ok(Some(row)) = octopus_infra::db::get_model_by_id(id) else {
        return TranslateStrategy::FallbackLlm;
    };
    if row.domain != "translate" || !row.is_enabled {
        return TranslateStrategy::FallbackLlm;
    }
    if row.is_local {
        TranslateStrategy::LocalModel { row }
    } else if row.secret_key.is_empty() {
        TranslateStrategy::FallbackLlm // 云端未填 key
    } else {
        TranslateStrategy::CloudModel { row }
    }
}
```

- [x] **Step 3: 重写 do_translate 为 async**

替换 `crates/desktop/src/action_bar_commands.rs:839-866`（原 `do_translate`）：

```rust
pub(crate) async fn do_translate(text: &str, config: &octopus_infra::config::AppConfig) -> Result<String, String> {
    let (source_lang, target_lang) = detect_translate_direction(text);
    match resolve_translate_strategy(config).await {
        TranslateStrategy::LocalModel { row } => {
            // opus-mt 按方向加载子目录
            if row.model_name.starts_with("opus-mt") {
                let engine = octopus_translation::load_opus_mt(source_lang, target_lang)
                    .map_err(|e| e.to_string())?;
                return engine.translate(text, source_lang, target_lang).await
                    .map_err(|e| e.to_string());
            }
            // m2m100 等
            let manager = octopus_translation::TranslationManager::new(&format!("local:{}", row.model_name));
            let engine = manager.engine()
                .map_err(|e| e.to_string())?
                .ok_or_else(|| "本地翻译引擎加载失败".to_string())?;
            engine.translate(text, source_lang, target_lang).await
                .map_err(|e| e.to_string())
        }
        TranslateStrategy::CloudModel { row } => {
            let engine = octopus_translation::CloudLlmEngine::new(
                &row.provider, &row.model_name, &row.source, &row.secret_key, row.is_thinking,
            );
            engine.translate(text, source_lang, target_lang).await
                .map_err(|e| e.to_string())
        }
        TranslateStrategy::FallbackLlm => {
            let llm_config = crate::config::llm_config_ignore_mode(config)
                .ok_or_else(|| "翻译 fallback LLM 未配置，请在设置中配置润色模型".to_string())?;
            let prompt = auto_translate_prompt(text);
            // LLM 调用是同步阻塞 HTTP——spawn_blocking 防卡 runtime
            let text_owned = text.to_string();
            let llm_config_owned = llm_config.clone();
            tokio::task::spawn_blocking(move || {
                octopus_llm::chat_text_with_prompt(prompt, &text_owned, &llm_config_owned, None)
            }).await
                .map_err(|e| format!("LLM 线程异常: {}", e))?
                .map_err(|e| e.to_string())
        }
    }
}
```

注意：`auto_translate_prompt(text)` 返回 `&'static str`，移进 spawn_blocking 闭包需 `'static`——`&'static str` 满足。

- [x] **Step 4: do_translate_streaming 适配 async**

`crates/desktop/src/action_bar_commands.rs` 原 `do_translate_streaming`（第 988 行附近）改为用 `tauri::async_runtime::block_on`（**不要用 `tokio::runtime::Runtime::new()`**——Tauri 已有全局 tokio runtime，新建会嵌套 panic）：

```rust
fn do_translate_streaming(text: &str, app: &AppHandle, target: TranslateEmitTarget) {
    let config = match octopus_infra::config::load_config() {
        Ok(c) => c,
        Err(e) => { target.emit_done(app, &format!("❌ 配置加载失败: {}", e)); return; }
    };
    let segments: Vec<&str> = text.split('\n').collect();
    let total = segments.len();
    let mut accumulated = String::new();
    for (i, seg) in segments.iter().enumerate() {
        if seg.trim().is_empty() {
            if i < total - 1 { accumulated.push('\n'); }
            continue;
        }
        // tauri::async_runtime::block_on 复用 Tauri 全局 tokio runtime（cloud_pipeline.rs 同模式）
        match tauri::async_runtime::block_on(do_translate(seg, &config)) {
            Ok(t) => accumulated.push_str(&t),
            Err(e) => { accumulated = format!("❌ 翻译失败: {}", e); break; }
        }
        if i < total - 1 { accumulated.push('\n'); }
        target.emit_progress(app, &accumulated);
    }
    target.emit_done(app, &accumulated);
}
```

注意：`do_translate_streaming` 在 `std::thread::spawn` 的 worker 线程里跑（translate_text 命令 / execute_action 都这样调），`tauri::async_runtime::block_on` 在非 runtime 线程里 block 是安全的（参考 `cloud_pipeline.rs:122` 同模式）。

- [x] **Step 5: execute_action 里 Llm 分支的 do_translate 调用适配**

`crates/desktop/src/action_bar_commands.rs:1441-1499`（execute_action 的 translate 分支）。这个分支原来分 `Local` / `Llm` 两条路径。新策略是 `LocalModel` / `CloudModel` / `FallbackLlm`。

关键决策：**CloudModel 也走流式 CompactEditor**（和 LocalModel 一样，体验更好），FallbackLlm 走 `action_bar_show_result`（单次返回）。

替换该 match 块（1441 行附近）：

```rust
                match resolve_translate_strategy(&config).await {
                    TranslateStrategy::LocalModel { .. } | TranslateStrategy::CloudModel { .. } => {
                        // 流式翻译：隐藏浮窗 + 打开 contrast tab
                        if action_bar_visible {
                            if let Some(win) = app.get_webview_window(crate::action_bar_window::WINDOW_LABEL) {
                                let _ = win.hide();
                            }
                            #[cfg(target_os = "macos")]
                            { crate::activation::after_floating_window_hide_keep_active(&app); }
                            finalize_action_bar(&app);
                        }
                        let original_text = text.clone();
                        let session_id = uuid::Uuid::new_v4().to_string();
                        let payload = crate::compact_editor_commands::TempTabPayload {
                            text: "【翻译】\n⏳ 正在翻译…".into(),
                            mode: Some("contrast".into()),
                            original_text: Some(original_text.clone()),
                            translated_text: Some("⏳ 正在翻译…".into()),
                            translate_session_id: Some(session_id.clone()),
                            ..Default::default()
                        };
                        let app_for_editor = app.clone();
                        let _ = app.run_on_main_thread(move || {
                            crate::compact_editor_commands::open_temp_compact_editor(&app_for_editor, &payload);
                        });
                        let app_clone = app.clone();
                        let target = TranslateEmitTarget::CompactEditor { session_id };
                        std::thread::spawn(move || {
                            do_translate_streaming(&original_text, &app_clone, target);
                        });
                        return Ok(true);
                    }
                    TranslateStrategy::FallbackLlm => {
                        let llm_config = crate::config::llm_config_ignore_mode(&config)
                            .ok_or("润色模型未配置，请在设置中配置 LLM")?;
                        let prompt = auto_translate_prompt(&text).to_string();
                        let text_clone = text.clone();
                        let config_clone = llm_config.clone();
                        let result = tokio::task::spawn_blocking(move || {
                            octopus_llm::chat_text_with_prompt(&prompt, &text_clone, &config_clone, None)
                        }).await
                            .map_err(|e| format!("LLM 线程异常: {}", e))?
                            .map_err(|e| e.to_string())?;
                        action_bar_show_result(result, text, "translate".into(), app.clone(), true);
                        return Ok(true);
                    }
                }
```

注意：此函数本身已是 async（execute_action 在 async 上下文，见原代码 `.await`），`resolve_translate_strategy(&config).await` 可直接用。

- [x] **Step 6: 编译 desktop crate**

Run: `cargo build -p octopus-desktop --features embedded`
Expected: 0 error。可能有 warning 关于未用 import，修掉。

- [x] **Step 7: 测试**

Run: `cargo test -p octopus-desktop --bin octopus-desktop`
Expected: 全过（311+ passed）。

- [x] **Step 8: Commit**

```bash
git add crates/desktop/src/action_bar_commands.rs
git commit -m "refactor(desktop): translate 策略改为 DB id 驱动 + do_translate async 化"
```

---

## Task 5: translate_status 适配新策略

**Files:**
- Modify: `crates/desktop/src/translation_commands.rs`

- [x] **Step 1: 读现有 translate_status**

Run: `cat crates/desktop/src/translation_commands.rs`

理解它返回的 `TranslateStatus { strategy, engineName, available }`。

- [x] **Step 2: 适配新策略**

`translate_status` 原来读 `config.translate_engine` 判断 strategy（llm/auto/local）。新语义下：
- `translate_engine` 空 → strategy `"fallback_llm"`，engineName 取 polish_llm 的 model
- 非空且能查到 DB 行 → strategy `"local"` / `"cloud"`，engineName 取 model_name
- 非空但查不到 → strategy `"fallback_llm"`

改为同步查 DB（translate_status 是同步命令，不 async）：

```rust
// translation_commands.rs translate_status 函数内
let (strategy, engine_name, available) = match config.translate_engine.parse::<i64>() {
    Ok(id) => match octopus_infra::db::get_model_by_id(id) {
        Ok(Some(row)) if row.domain == "translate" && row.is_enabled => {
            let s = if row.is_local { "local" } else { "cloud" };
            (s.to_string(), row.model_name, true)
        }
        _ => ("fallback_llm".into(), config.polish_llm.clone(), !config.polish_llm.is_empty()),
    },
    Err(_) => ("fallback_llm".into(), config.polish_llm.clone(), !config.polish_llm.is_empty()),
};
```

- [x] **Step 3: 编译 + 测试**

Run: `cargo build -p octopus-desktop --features embedded`
Run: `cargo test -p octopus-desktop --bin octopus-desktop`
Expected: 0 error，全过。

- [x] **Step 4: Commit**

```bash
git add crates/desktop/src/translation_commands.rs
git commit -m "refactor(desktop): translate_status 适配 DB id 驱动的新策略"
```

---

## Task 6: 前端 TranslateTab 加云端模型 section

**Files:**
- Modify: `crates/desktop/frontend/src/pages/Settings/Models/TranslateTab.tsx`
- Modify: `crates/desktop/frontend/src/pages/Settings/Models/CloudModelForm.tsx`
- Modify: `crates/desktop/frontend/src/pages/Settings/Models/LlmTab.tsx`（参考）
- Modify: `crates/desktop/frontend/src/locales/zh-CN.yaml` + `en.yaml`

**Interfaces:**
- Consumes: 后端 `add_cloud_model` / `edit_cloud_model` / `remove_cloud_model`（已 domain 参数化）、`list_llm_provider_presets`（provider 下拉，含新增 moonshot/minimax）
- Produces: TranslateTab 含本地 + 云端两个 section，激活写 `set_config({key:"translate_engine", value: String(id)})`

- [x] **Step 1: CloudModelForm domain 类型扩展**

`crates/desktop/frontend/src/pages/Settings/Models/CloudModelForm.tsx`：

把 domain 类型从 `"asr" | "llm"` 改为 `"asr" | "llm" | "translate"`（grep 定位所有 `"asr" | "llm"` 出现处，约 2 处：props interface + 内部分支）。

translate 分支复用 llm 的逻辑（provider→base_url 自动填充）——在 provider 变更的 `useEffect` 里，`domain === "llm" || domain === "translate"` 都走 preset 填充。

- [x] **Step 2: 读 LlmTab 作为云端 section 模板**

Run: `cat crates/desktop/frontend/src/pages/Settings/Models/LlmTab.tsx`

理解它的 `cloudRows` section 结构（添加按钮 + CloudModelForm + 每行激活/编辑/删除）。

- [x] **Step 3: TranslateTab 加云端 section**

在 `crates/desktop/frontend/src/pages/Settings/Models/TranslateTab.tsx` 添加云端模型管理。关键改动：

1. 拉取云端模型：`get_config` 返回里需有 translate 云端模型列表。检查后端 `get_config` 是否返回 `translate_models`——若没有，加一个 invoke `list_cloud_models`（需后端新增命令）或复用现有。

   **简化方案**：新增轻量 Tauri 命令 `list_translate_cloud_models` 返回 `Vec<ModelRow>`（domain='translate' AND is_local=0）。

2. 渲染两个 CollapsibleSection：本地（现有）+ 云端（新增，含添加按钮 + CloudModelForm + 行操作）。
3. 激活操作：`set_config({key:"translate_engine", value:String(model.id)})`。

- [x] **Step 4: 后端加 list_translate_cloud_models 命令**

`crates/desktop/src/model_commands.rs` 加：

```rust
#[tauri::command]
pub fn list_translate_cloud_models() -> Result<Vec<TranslateCloudModel>, String> {
    octopus_infra::db::list_cloud_models_by_domain("translate")
        .map(|rows| rows.into_iter().map(TranslateCloudModel::from).collect())
        .map_err(|e| e.to_string())
}
```

若 `list_cloud_models_by_domain` 不存在，在 db.rs 加（参考 `list_llm_models`，把 `domain='llm'` 参数化）。

main.rs 注册命令。

- [x] **Step 5: i18n key**

zh-CN.yaml / en.yaml 补翻译云端相关 key（参考 llm 的 `settings.models.llm.*`，新增 `settings.models.translate.cloud.*`）。

- [x] **Step 6: tsc + vite build**

Run: `cd crates/desktop/frontend && npx tsc --noEmit && npx vite build`
Expected: 0 error。

- [x] **Step 7: Commit**

```bash
git add crates/desktop/frontend/src/pages/Settings/Models/ crates/desktop/src/model_commands.rs crates/desktop/src/main.rs crates/desktop/frontend/src/locales/
git commit -m "feat(translate): TranslateTab 加云端模型 section + CloudModelForm 扩 translate domain"
```

---

## Task 7: 连接测试 + 文档同步

**Files:**
- Modify: `crates/desktop/src/model_commands.rs`（add_cloud_model 连接测试）
- Modify: `docs/architecture.md`

- [x] **Step 1: add_cloud_model 连接测试扩展到 translate**

`crates/desktop/src/model_commands.rs:402-407` 附近，现有：

```rust
if domain == "llm" {
    // 测试连接...
}
```

改为：

```rust
if domain == "llm" || domain == "translate" {
    // 用翻译 prompt 测试一次短文本
    let test_prompt = "Translate the following text from English to Chinese. Only output the translation.";
    octopus_llm::chat_text_with_prompt(test_prompt, "hello", &test_config, Some(15))
        .map_err(|e| format!("连接测试失败: {}", e))?;
}
```

- [x] **Step 2: architecture.md 同步**

更新 `docs/architecture.md` 的翻译章节（第 73-74 行附近）：
- TranslationEngine trait 改 async（#[async_trait]）
- 新增 CloudLlmEngine（云端 OpenAI 兼容翻译）
- translate_engine 语义改为 DB id（本地/云端统一，空/非法 fallback polish_llm）
- provider 预设补 moonshot/minimax
- 移除"自动策略"描述（不再有 auto）

- [x] **Step 3: 全量编译验证**

Run: `cargo build --release -p octopus-desktop --features embedded`
Expected: 0 error 0 warning。

- [x] **Step 4: 全量测试**

Run: `cargo test -p octopus-translation --lib && cargo test -p octopus-infra --lib && cargo test -p octopus-desktop --bin octopus-desktop`
Expected: 全过。

- [x] **Step 5: 前端验证**

Run: `cd crates/desktop/frontend && npx tsc --noEmit && npx vite build`
Expected: 0 error。

- [x] **Step 6: Commit**

```bash
git add crates/desktop/src/model_commands.rs docs/architecture.md
git commit -m "feat(translate): 连接测试支持 translate domain + 文档同步"
```

---

## Self-Review

### Spec coverage
- ✅ trait async（Task 1）
- ✅ CloudLlmEngine（Task 2）
- ✅ translate_engine DB id 语义（Task 3 config + Task 4 strategy）
- ✅ fallback polish_llm（Task 4 FallbackLlm 分支）
- ✅ moonshot/minimax provider 预设（Task 3 db.sql）
- ✅ 调用链 async（Task 4）
- ✅ translate_status 适配（Task 5）
- ✅ 前端云端 section（Task 6）
- ✅ 连接测试（Task 7）
- ✅ 旧值不迁移（Task 4 parse 失败自然 fallback）
- ✅ 不新增 translate_llm（全局约束）

### 已知风险点
1. **Task 4 opus-mt 方向**：strategy 不预加载引擎，do_translate 里按文本方向 load_opus_mt——但 strategy 的 LocalModel 分支只存 row，do_translate 里再判断 opus-mt 重载。已在 Step 3 处理。
2. **Task 6 后端命令**：`list_translate_cloud_models` 需新增 + main.rs 注册，CloudModelForm 的 get_config 数据源需确认。
3. ~~tokio runtime 嵌套~~（已解决）：改用 `tauri::async_runtime::block_on`（复用 Tauri 全局 runtime，参考 cloud_pipeline.rs:122），不用 `Runtime::new()`。
