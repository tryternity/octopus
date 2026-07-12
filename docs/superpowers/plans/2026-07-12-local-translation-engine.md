# 本地翻译引擎（m2m100 ONNX int8）Implementation Plan

> **状态**：已实现（Task 0-6 全部完成）
> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**实际实现偏差记录**：
- Task 0：新建 `onnx-infra` crate 抽取公共 ONNX 基础设施（模型路径查找 + session 加速），asr-local re-export 保持兼容
- Task 1-2：`venddair/m2m100-418M-onnx-int8` 模型 int8 量化精度有损（输出乱码），切换到 `lazycodepersona/m2m100_418m`（IR=6, opset=11，ort rc.12 完全兼容）
- Task 2：原计划用 `sentencepiece` C++ crate → 与 ORT 静态库 protobuf 符号冲突 → 换 `sentencepiece-rs`（纯 Rust）→ 再换 HF `tokenizers` crate（与 lazycodepersona 的 `tokenizer.json` 匹配）
- Task 2：encoder 输入需手动构建 `[source_lang_id] + text_tokens + [eos]`（Rust `tokenizers` 不自动加特殊 token）
- Task 2：decoder 初始输入为 `[eos, target_lang_id]`（m2m100 需要 forced BOS = 目标语言标记）
- Task 2：greedy 解码加入重复 token 检测（连续 5 个相同 token 则停止）
- Task 4：`translate_engine` 配置字段加到 `AppConfig` + `apply_config_value`

**Goal:** 新建 `octopus-translation` crate，接入 m2m100-418M ONNX int8 本地翻译引擎，action bar 翻译优先本地引擎，未配置时 fallback LLM。

**Architecture:** 新建独立 crate（trait + m2m100 实现 + 模型发现），infra 加配置字段，desktop 改翻译执行流程 + 前端 TranslateTab 模型管理 UI。m2m100 encoder-decoder greedy 解码，SentencePiece BPE tokenizer。

**Tech Stack:** Rust（`ort` 2.0.0-rc.12 ONNX Runtime + `sentencepiece` 0.13.1 static + `ndarray` 0.17）、React + TypeScript + Tailwind（前端）

## Global Constraints

- m2m100 模型 repo：`venddair/m2m100-418M-onnx-int8`
- 解码策略：greedy（每步 argmax，max_length=200，eos_token_id=2）
- Decoder `use_cache: false`：每步传完整已生成序列
- Tokenizer：`tokenizer.json`，`tokenizers` crate（HuggingFace tokenizers，含 lang special tokens）
- 语言标记：ISO 639-1（`"zh"` / `"en"`）→ m2m100 标记（`__zh__` / `__en__`）
- `translate_engine` 配置：`""` = 自动，`"local:m2m100"` = 指定本地，`"llm"` = 强制 LLM
- 翻译方向检测：CJK 字符 → zh→en，否则 → en→zh（复用现有逻辑）
- `ort` 版本与 asr-local 一致：`2.0.0-rc.12`，macOS 用 `coreml` feature
- ONNX session：标准 `Session::builder()`（未使用硬件加速）

---

## 文件结构

| 文件 | 职责 | 操作 |
|------|------|------|
| `crates/translation/Cargo.toml` | crate 定义 | 创建 |
| `crates/translation/src/lib.rs` | 公开 API + re-exports | 创建 |
| `crates/translation/src/engine.rs` | TranslationEngine trait + TranslationManager | 创建 |
| `crates/translation/src/m2m100.rs` | m2m100 ONNX 引擎实现 | 创建 |
| `crates/translation/src/tokenizer.rs` | SentencePiece BPE 封装 + 语言标记 | 创建 |
| `crates/translation/src/discovery.rs` | 模型发现（HF cache + 本地扫描） | 创建 |
| `Cargo.toml` (workspace) | 加入 translation member | 修改 |
| `crates/infra/src/config.rs` | AppConfig 加 translate_engine 字段 | 修改 |
| `crates/desktop/Cargo.toml` | 加 octopus-translation 依赖 | 修改 |
| `crates/desktop/src/action_bar_commands.rs` | 翻译执行流程改造 | 修改 |
| `crates/desktop/src/translation_commands.rs` | Tauri 命令（模型列表 + 发现） | 创建 |
| `crates/desktop/src/main.rs` | 注册翻译命令 | 修改 |
| `crates/desktop/src/settings_commands.rs` | apply_config_value 加 translate_engine | 修改 |
| `crates/desktop/frontend/.../TranslateTab.tsx` | 模型管理 + 引擎选择 UI | 修改 |

---

### Task 1: 创建 octopus-translation crate 骨架 + 引擎 trait + Tokenizer

**Files:**
- Create: `crates/translation/Cargo.toml`
- Create: `crates/translation/src/lib.rs`
- Create: `crates/translation/src/engine.rs`
- Create: `crates/translation/src/tokenizer.rs`
- Modify: `Cargo.toml`（workspace root，加 member）

**Interfaces:**
- Produces: `TranslationEngine` trait、`TranslationManager`、`M2M100Tokenizer`

- [ ] **Step 1: 创建 Cargo.toml**

```toml
[package]
name = "octopus-translation"
version = "0.1.0"
edition = "2021"

[dependencies]
ort = { version = "2.0.0-rc.12", features = ["download-binaries"] }
ndarray = "0.17"
anyhow = "1"
log = "0.4"
parking_lot = { workspace = true }
sentencepiece = { version = "0.13", features = ["static"] }
octopus-infra = { path = "../infra" }

[target.'cfg(target_os = "macos")'.dependencies]
ort = { version = "2.0.0-rc.12", features = ["coreml"] }

[target.'cfg(target_os = "linux")'.dependencies]
ort = { version = "2.0.0-rc.12", features = ["cuda"] }

[target.'cfg(target_os = "windows")'.dependencies]
ort = { version = "2.0.0-rc.12", features = ["directml"] }
```

- [ ] **Step 2: 创建 src/lib.rs**

```rust
pub mod engine;
pub mod tokenizer;
pub mod m2m100;
pub mod discovery;

pub use engine::{TranslationEngine, TranslationManager};
pub use m2m100::M2M100Engine;
pub use tokenizer::M2M100Tokenizer;
pub use discovery::{discover_translation_models, list_downloadable_translation_models,
    TranslationModelInfo, DownloadableTranslationModel};
```

- [ ] **Step 3: 创建 src/engine.rs**

```rust
use anyhow::Result;
use std::sync::Arc;

/// 本地翻译引擎 trait。
pub trait TranslationEngine: Send + Sync {
    /// 翻译文本。source_lang / target_lang 用 ISO 639-1 代码（"zh" / "en"）。
    fn translate(&self, text: &str, source_lang: &str, target_lang: &str) -> Result<String>;

    /// 引擎显示名（如 "m2m100-418M"）。
    fn name(&self) -> &str;
}

/// 翻译引擎管理器——lazy load + 缓存。
pub struct TranslationManager {
    engine: parking_lot::Mutex<Option<Arc<dyn TranslationEngine>>>,
    engine_spec: String,
}

impl TranslationManager {
    pub fn new(engine_spec: &str) -> Self {
        Self {
            engine: parking_lot::Mutex::new(None),
            engine_spec: engine_spec.to_string(),
        }
    }

    /// 获取引擎（lazy load：首次调用时加载模型）。
    pub fn engine(&self) -> Result<Option<Arc<dyn TranslationEngine>>> {
        let mut guard = self.engine.lock();
        if guard.is_some() {
            return Ok(guard.clone());
        }
        // 只支持 local:m2m100
        if self.engine_spec == "local:m2m100" {
            let e = Arc::new(crate::m2m100::M2M100Engine::load()? as Arc<dyn TranslationEngine>);
            *guard = Some(e.clone());
            return Ok(Some(e));
        }
        Ok(None)
    }
}
```

- [ ] **Step 4: 创建 src/tokenizer.rs**

```rust
use anyhow::{Context, Result};
use std::path::Path;

/// m2m100 SentencePiece BPE tokenizer 封装。
pub struct M2M100Tokenizer {
    sp: sentencepiece::SentenceProcessor,
}

/// 特殊 token IDs（来自 config.json）
pub const BOS_ID: i64 = 0;
pub const PAD_ID: i64 = 1;
pub const EOS_ID: i64 = 2;
pub const UNK_ID: i64 = 3;
pub const DECODER_START_TOKEN_ID: i64 = 2;

/// ISO 639-1 → m2m100 语言标记 token
fn lang_code_to_token(lang: &str) -> Option<&'static str> {
    match lang {
        "zh" => Some("__zh__"),
        "en" => Some("__en__"),
        "ja" => Some("__ja__"),
        "ko" => Some("__ko__"),
        "fr" => Some("__fr__"),
        "de" => Some("__de__"),
        "es" => Some("__es__"),
        "ru" => Some("__ru__"),
        "it" => Some("__it__"),
        "pt" => Some("__pt__"),
        "ar" => Some("__ar__"),
        "th" => Some("__th__"),
        "vi" => Some("__vi__"),
        "id" => Some("__id__"),
        "tr" => Some("__tr__"),
        "nl" => Some("__nl__"),
        "pl" => Some("__pl__"),
        "uk" => Some("__uk__"),
        "hi" => Some("__hi__"),
        _ => None,
    }
}

impl M2M100Tokenizer {
    pub fn load(model_path: &Path) -> Result<Self> {
        let sp = sentencepiece::SentenceProcessor::load_file(
            model_path.to_str().context("invalid path")?,
        )
        .context("加载 SentencePiece 模型失败")?;
        Ok(Self { sp })
    }

    /// 编码：text → token ids。在 tokens 前插入源语言标记 + 后接 EOS。
    pub fn encode(&self, text: &str, source_lang: &str) -> Result<Vec<i64>> {
        let lang_token = lang_code_to_token(source_lang).unwrap_or("__en__");
        let lang_id = self.sp.piece_to_id(lang_token).unwrap_or(UNK_ID);
        let pieces = self.sp.encode(text);
        let mut ids: Vec<i64> = vec![lang_id];
        ids.extend(pieces.iter().map(|p| p as i64));
        ids.push(EOS_ID);
        Ok(ids)
    }

    /// 解码：token ids → text。跳过特殊 token 和语言标记。
    pub fn decode(&self, ids: &[i64]) -> Result<String> {
        let filtered: Vec<u32> = ids
            .iter()
            .filter(|&&id| id > UNK_ID) // 过滤 special tokens (0-3) 和语言标记
            .map(|&id| id as u32)
            .collect();
        let pieces: Vec<&str> = filtered
            .iter()
            .filter_map(|&id| self.sp.id_to_piece(id))
            .collect();
        // SentencePiece 用 ▁ 表示空格
        let raw = pieces.join("");
        let text = raw.replace('\u{2581}', " ").trim_start().to_string();
        Ok(text)
    }
}
```

- [ ] **Step 5: 创建占位的 m2m100.rs 和 discovery.rs**

```rust
// src/m2m100.rs（Task 2 完整实现）
use anyhow::Result;

pub struct M2M100Engine;

impl M2M100Engine {
    pub fn load() -> Result<Self> {
        anyhow::bail!("M2M100Engine 尚未实现（Task 2）")
    }
}

impl super::engine::TranslationEngine for M2M100Engine {
    fn translate(&self, _text: &str, _source_lang: &str, _target_lang: &str) -> Result<String> {
        anyhow::bail!("M2M100Engine 尚未实现")
    }
    fn name(&self) -> &str { "m2m100-418M" }
}
```

```rust
// src/discovery.rs（Task 3 完整实现）
use serde::Serialize;

#[derive(Serialize, Clone)]
pub struct TranslationModelInfo {
    pub name: String,
    pub source: String,
    pub downloaded: bool,
    pub size_mb: u64,
    pub path: String,
}

#[derive(Serialize, Clone)]
pub struct DownloadableTranslationModel {
    pub name: String,
    pub repo: String,
    pub description: String,
    pub size_mb: u64,
}

pub fn discover_translation_models() -> Vec<TranslationModelInfo> {
    Vec::new()
}

pub fn list_downloadable_translation_models() -> Vec<DownloadableTranslationModel> {
    vec![DownloadableTranslationModel {
        name: "m2m100-418M (int8)".into(),
        repo: "venddair/m2m100-418M-onnx-int8".into(),
        description: "多语言翻译（100+ 语言互译）".into(),
        size_mb: 724,
    }]
}
```

- [ ] **Step 6: 加入 workspace members**

在根 `Cargo.toml` 的 `members` 数组中加入 `"crates/translation"`：

```toml
members = ["crates/infra", "crates/asr-local", "crates/asr-cloud", "crates/server", "crates/cli", "crates/desktop", "crates/llm", "crates/dlp", "crates/download", "crates/clipboard", "crates/ocr", "crates/paddle-ocr", "crates/capx", "crates/translation"]
```

- [ ] **Step 7: 编译验证**

Run: `cargo build -p octopus-translation`
Expected: 编译通过（sentencepiece static 编译可能需要较长时间）

- [ ] **Step 8: Commit**

```bash
git add crates/translation/ Cargo.toml
git commit -m "feat(translation): create octopus-translation crate skeleton"
```

---

### Task 2: m2m100 ONNX 引擎实现

**Files:**
- Modify: `crates/translation/src/m2m100.rs`
- Create: `crates/translation/tests/m2m100_test.rs`

**Interfaces:**
- Consumes: Task 1 的 `TranslationEngine` trait、`M2M100Tokenizer`
- Produces: 可运行的 `M2M100Engine::load()` + `translate()` 方法

- [ ] **Step 1: 实现 M2M100Engine::load()**

完整替换 `src/m2m100.rs`：

```rust
use anyhow::{Context, Result};
use ort::session::Session;
use parking_lot::Mutex;
use std::path::PathBuf;

use crate::engine::TranslationEngine;
use crate::tokenizer::{M2M100Tokenizer, DECODER_START_TOKEN_ID, EOS_ID};

const MAX_LENGTH: usize = 200;

pub struct M2M100Engine {
    encoder: Mutex<Session>,
    decoder: Mutex<Session>,
    tokenizer: M2M100Tokenizer,
}

/// ONNX session：标准 Session::builder（未使用硬件加速，翻译模型小）。
/// 独立实现（避免引入 asr-local 依赖）。
fn apply_acceleration(builder: ort::session::builder::SessionBuilder) -> Result<ort::session::builder::SessionBuilder> {
    // 一期纯 CPU——翻译模型不大，CoreML 对 decoder 自回归可能不稳定
    Ok(builder)
}

/// 模型发现：复用 infra 的路径逻辑 + HF cache 查找
fn resolve_model_dir(source: &str) -> Result<PathBuf> {
    // 先查 ~/.octopus/models/<source>
    let home = octopus_infra::paths::octopus_config_home();
    let local = home.join("models").join(source);
    if local.exists() {
        return Ok(local);
    }
    // HF cache：models--<source with -->/snapshots/<hash>/
    let model_dir_name = source.replace('/', "--");
    let hf_dir = std::env::var("HOME")
        .context("HOME not set")?
        .parse::<PathBuf>()
        .unwrap()
        .join(".cache/huggingface/hub")
        .join(format!("models--{}", model_dir_name));
    if !hf_dir.exists() {
        anyhow::bail!("模型未找到: {}（本地: {:?}, HF: {:?}）", source, local, hf_dir);
    }
    // 取最新 snapshot
    let snapshots = hf_dir.join("snapshots");
    let mut latest: Option<PathBuf> = None;
    if let Ok(entries) = std::fs::read_dir(&snapshots) {
        for entry in entries.flatten() {
            let p = entry.path();
            if p.is_dir() {
                if latest.is_none()
                    || entry.metadata().map(|m| m.modified().is_ok())
                        .unwrap_or(false)
                {
                    latest = Some(p);
                }
            }
        }
    }
    latest.context("HF cache 中无 snapshot")
}

impl M2M100Engine {
    pub fn load() -> Result<Self> {
        let source = "venddair/m2m100-418M-onnx-int8";
        let model_dir = resolve_model_dir(source)?;

        let encoder_path = model_dir.join("onnx/encoder_model_quantized.onnx");
        let decoder_path = model_dir.join("decoder_model.onnx");
        let tokenizer_path = model_dir.join("tokenizer.json");

        for (name, path) in [("encoder", &encoder_path), ("decoder", &decoder_path), ("tokenizer", &tokenizer_path)] {
            if !path.exists() {
                anyhow::bail!("模型文件缺失: {} ({:?})", name, path);
            }
        }

        let encoder = apply_acceleration(Session::builder()?)?
            .commit_from_file(&encoder_path)
            .context("加载 encoder ONNX 失败")?;
        let decoder = apply_acceleration(Session::builder()?)?
            .commit_from_file(&decoder_path)
            .context("加载 decoder ONNX 失败")?;
        let tokenizer = M2M100Tokenizer::load(&tokenizer_path)?;

        log::info!("m2m100 引擎加载完成: {:?}", model_dir);
        Ok(Self {
            encoder: Mutex::new(encoder),
            decoder: Mutex::new(decoder),
            tokenizer,
        })
    }
}

impl TranslationEngine for M2M100Engine {
    fn name(&self) -> &str {
        "m2m100-418M"
    }

    fn translate(&self, text: &str, source_lang: &str, target_lang: &str) -> Result<String> {
        // 1. Encode
        let input_ids = self.tokenizer.encode(text, source_lang)?;
        let seq_len = input_ids.len();
        let input_ids_arr = ndarray::Array1::from_iter(input_ids.iter().copied())
            .insert_axis(ndarray::Axis(0)); // [1, seq_len]
        let attention_mask = ndarray::Array1::from_elem(seq_len, 1i64)
            .insert_axis(ndarray::Axis(0));

        // 2. Encoder forward
        let encoder = self.encoder.lock();
        let encoder_outputs = encoder.run(ort::inputs! {
            "input_ids" => input_ids_arr.view(),
            "attention_mask" => attention_mask.view(),
        }?)?;
        let encoder_hidden_states: ndarray::ArrayView3<f32, _> = encoder_outputs["last_hidden_state"]
            .try_extract_tensor()?
            .try_into_array()?
            .insert_axis(ndarray::Axis(0)); // 需要从 [1, seq, 1024] 提取
        drop(encoder);

        // 3. Decoder greedy loop
        let mut decoder_ids: Vec<i64> = vec![DECODER_START_TOKEN_ID];
        let decoder = self.decoder.lock();

        for _ in 0..MAX_LENGTH {
            let dec_arr = ndarray::Array1::from_iter(decoder_ids.iter().copied())
                .insert_axis(ndarray::Axis(0)); // [1, dec_seq]
            let outputs = decoder.run(ort::inputs! {
                "input_ids" => dec_arr.view(),
                "encoder_hidden_states" => encoder_hidden_states.view(),
                "encoder_attention_mask" => attention_mask.view(),
            }?)?;

            let logits: ndarray::ArrayView3<f32, _> = outputs["logits"]
                .try_extract_tensor()?
                .try_into_array()?;
            // 取最后位置的 logits，argmax
            let last_logits = logits.slice(ndarray::s![0, -1, ..]);
            let next_token = last_logits
                .iter()
                .enumerate()
                .max_by(|(_, a), (_, b)| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal))
                .map(|(i, _)| i as i64)
                .unwrap_or(0);

            if next_token == EOS_ID {
                break;
            }
            decoder_ids.push(next_token);
        }
        drop(decoder);

        // 4. Decode（跳过 start token）
        let result_ids: Vec<i64> = decoder_ids[1..].to_vec();
        let text = self.tokenizer.decode(&result_ids)?;
        Ok(text)
    }
}
```

> **注意**：ONNX tensor 的 `try_extract_tensor` / `try_into_array` 返回维度可能需要调整。ORT 2.0 API 中 tensor 形状是 `[batch, seq, dim]`，`try_into_array()` 返回 `ArrayD<f32>`。实现时根据编译器错误调整 `insert_axis` / `view` 操作。

- [ ] **Step 2: 编译验证（修复类型错误）**

Run: `cargo build -p octopus-translation`
Expected: 编译通过。可能有 ndarray / ort API 类型不匹配，逐步修复。

- [ ] **Step 3: 写集成测试**

创建 `crates/translation/tests/m2m100_test.rs`：

```rust
use octopus_translation::{TranslationEngine, M2M100Engine};

#[test]
fn test_m2m100_zh_to_en() {
    let engine = M2M100Engine::load().expect("模型加载失败——请确保 m2m100 已下载");
    let result = engine.translate("你好世界", "zh", "en").expect("翻译失败");
    println!("zh→en: 你好世界 → {}", result);
    assert!(!result.is_empty());
    // m2m100 对"你好世界"通常翻译为 "Hello world" 或类似
    let lower = result.to_lowercase();
    assert!(lower.contains("hello") || lower.contains("hi"), "翻译结果: {}", result);
}

#[test]
fn test_m2m100_en_to_zh() {
    let engine = M2M100Engine::load().expect("模型加载失败");
    let result = engine.translate("Hello world", "en", "zh").expect("翻译失败");
    println!("en→zh: Hello world → {}", result);
    assert!(!result.is_empty());
}
```

- [ ] **Step 4: 运行测试**

Run: `cargo test -p octopus-translation -- --nocapture`
Expected: 两个测试通过，输出翻译结果

- [ ] **Step 5: Commit**

```bash
git add crates/translation/
git commit -m "feat(translation): m2m100 ONNX engine with greedy decoding"
```

---

### Task 3: 模型发现 + AppConfig 字段

**Files:**
- Modify: `crates/translation/src/discovery.rs`
- Modify: `crates/infra/src/config.rs:56-227`（AppConfig struct）
- Modify: `crates/infra/src/config.rs:332-376`（Default impl）

**Interfaces:**
- Produces: `discover_translation_models()` 返回已下载模型列表，`AppConfig.translate_engine` 字段

- [ ] **Step 1: 完整实现 discovery.rs**

替换 `src/discovery.rs`：

```rust
use serde::Serialize;
use std::path::PathBuf;

#[derive(Serialize, Clone)]
pub struct TranslationModelInfo {
    pub name: String,
    pub source: String,
    pub downloaded: bool,
    pub size_mb: u64,
    pub path: String,
}

#[derive(Serialize, Clone)]
pub struct DownloadableTranslationModel {
    pub name: String,
    pub repo: String,
    pub description: String,
    pub size_mb: u64,
}

/// 已知翻译模型清单（硬编码，后续可扩展）
const KNOWN_MODELS: &[(&str, &str, u64)] = &[
    ("m2m100-418M (int8)", "venddair/m2m100-418M-onnx-int8", 724),
];

/// 扫描本地已下载的翻译模型
pub fn discover_translation_models() -> Vec<TranslationModelInfo> {
    let mut result = Vec::new();
    for (name, repo, size_mb) in KNOWN_MODELS {
        let downloaded = check_model_downloaded(repo);
        let path = find_model_path(repo).map(|p| p.to_string_lossy().to_string()).unwrap_or_default();
        result.push(TranslationModelInfo {
            name: name.to_string(),
            source: repo.to_string(),
            downloaded,
            size_mb: *size_mb,
            path,
        });
    }
    result
}

/// 可下载的翻译模型列表
pub fn list_downloadable_translation_models() -> Vec<DownloadableTranslationModel> {
    KNOWN_MODELS.iter().map(|(name, repo, size_mb)| {
        DownloadableTranslationModel {
            name: name.to_string(),
            repo: repo.to_string(),
            description: "多语言翻译（100+ 语言互译）".to_string(),
            size_mb: *size_mb,
        }
    }).collect()
}

/// 检查模型是否已下载（encoder + decoder + tokenizer 三文件齐全）
fn check_model_downloaded(repo: &str) -> bool {
    find_model_path(repo)
        .map(|dir| {
            dir.join("onnx/encoder_model_quantized.onnx").exists()
                && dir.join("decoder_model.onnx").exists()
                && dir.join("tokenizer.json").exists()
        })
        .unwrap_or(false)
}

/// 查找模型目录路径
fn find_model_path(repo: &str) -> Option<PathBuf> {
    // ~/.octopus/models/<repo>
    let home = octopus_infra::paths::octopus_config_home();
    let local = home.join("models").join(repo);
    if local.exists() {
        return Some(local);
    }
    // HF cache
    if let Ok(hf_home) = std::env::var("HOME") {
        let model_dir_name = repo.replace('/', "--");
        let snapshots = PathBuf::from(hf_home)
            .join(".cache/huggingface/hub")
            .join(format!("models--{}", model_dir_name))
            .join("snapshots");
        if let Ok(entries) = std::fs::read_dir(&snapshots) {
            for entry in entries.flatten() {
                let p = entry.path();
                if p.is_dir() {
                    return Some(p); // 取第一个 snapshot
                }
            }
        }
    }
    None
}
```

- [ ] **Step 2: AppConfig 加 translate_engine 字段**

在 `crates/infra/src/config.rs` 的 `AppConfig` struct 中（L226 `ocr_model` 之后）加入：

```rust
    /// 翻译引擎："" = 自动（有本地用本地，否则 LLM），"local:m2m100" = 指定本地，"llm" = 强制 LLM
    #[serde(default)]
    pub translate_engine: String,
```

在 `impl Default for AppConfig`（L332-376）末尾加入：

```rust
            translate_engine: String::new(),
```

- [ ] **Step 3: 编译验证**

Run: `cargo build -p octopus-translation -p octopus-infra`
Expected: 编译通过

- [ ] **Step 4: Commit**

```bash
git add crates/translation/src/discovery.rs crates/infra/src/config.rs
git commit -m "feat(translation): model discovery + translate_engine config field"
```

---

### Task 4: Tauri 命令 + action bar 翻译流程改造

**Files:**
- Create: `crates/desktop/src/translation_commands.rs`
- Modify: `crates/desktop/Cargo.toml`（加 octopus-translation 依赖）
- Modify: `crates/desktop/src/main.rs`（注册命令）
- Modify: `crates/desktop/src/action_bar_commands.rs:653-748`（翻译分支改造）
- Modify: `crates/desktop/src/settings_commands.rs:232-391`（apply_config_value）

**Interfaces:**
- Consumes: Task 2-3 的引擎 + 发现 + 配置
- Produces: 翻译 Tauri 命令 + action bar 本地翻译路径

- [ ] **Step 1: Cargo.toml 加依赖**

在 `crates/desktop/Cargo.toml` 的 `[dependencies]` 中加入：

```toml
octopus-translation = { path = "../translation" }
```

- [ ] **Step 2: 创建 translation_commands.rs**

```rust
use octopus_translation::{
    discover_translation_models, list_downloadable_translation_models,
    DownloadableTranslationModel, TranslationModelInfo,
};
use serde::Serialize;

#[tauri::command]
pub fn list_downloadable_translation_models() -> Result<Vec<DownloadableTranslationModel>, String> {
    Ok(list_downloadable_translation_models())
}

#[tauri::command]
pub fn discover_translation_models() -> Result<Vec<TranslationModelInfo>, String> {
    Ok(discover_translation_models())
}

/// 翻译预检：返回当前配置下将使用的翻译方式
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TranslateStatus {
    pub strategy: String,   // "local" | "llm" | "auto"
    pub engine_name: String, // 自动模式下实际使用的引擎名
    pub available: bool,
}

#[tauri::command]
pub fn translate_status() -> Result<TranslateStatus, String> {
    let config = octopus_infra::config::load_config().map_err(|e| e.to_string())?;
    let spec = &config.translate_engine;

    if spec == "llm" {
        return Ok(TranslateStatus {
            strategy: "llm".into(),
            engine_name: "LLM".into(),
            available: true,
        });
    }

    if spec.is_empty() {
        // 自动
        let models = discover_translation_models();
        if let Some(m) = models.iter().find(|m| m.downloaded) {
            return Ok(TranslateStatus {
                strategy: "auto".into(),
                engine_name: m.name.clone(),
                available: true,
            });
        }
        return Ok(TranslateStatus {
            strategy: "auto".into(),
            engine_name: "LLM".into(),
            available: true,
        });
    }

    // local:xxx
    let models = discover_translation_models();
    if let Some(m) = models.iter().find(|m| format!("local:{}", m.name.split(' ').next().unwrap_or("").to_lowercase()) == *spec) {
        return Ok(TranslateStatus {
            strategy: "local".into(),
            engine_name: m.name.clone(),
            available: m.downloaded,
        });
    }

    Ok(TranslateStatus {
        strategy: "unknown".into(),
        engine_name: spec.clone(),
        available: false,
    })
}
```

- [ ] **Step 3: 注册 Tauri 命令**

在 `crates/desktop/src/main.rs` 中找到 `tauri::generate_handler!` 列表，加入：

```rust
    crate::translation_commands::list_downloadable_translation_models,
    crate::translation_commands::discover_translation_models,
    crate::translation_commands::translate_status,
```

在文件头加入模块声明：

```rust
mod translation_commands;
```

- [ ] **Step 4: 改造 action bar 翻译分支**

在 `crates/desktop/src/action_bar_commands.rs` 的 `execute_action_bar_inner` 函数中（L659-671），修改 `"ai"` 分支：

```rust
        "ai" => {
            let config = octopus_infra::config::load_config().map_err(|e| e.to_string())?;

            // 翻译特殊处理：优先本地引擎
            if item.action_data == "auto_translate" {
                let (source_lang, target_lang) = detect_translate_direction(&text);
                let result = match resolve_translate_strategy(&config) {
                    TranslateStrategy::Local(engine) => {
                        engine.translate(&text, source_lang, target_lang)
                        .map_err(|e| e.to_string())?
                    }
                    TranslateStrategy::Llm => {
                        let llm_config = crate::config::llm_config_ignore_mode(&config)
                            .ok_or("润色模型未配置，请在设置中配置 LLM")?;
                        let prompt = auto_translate_prompt(&text);
                        octopus_llm::chat_text_with_prompt(prompt, &text, &llm_config)
                        .map_err(|e| e.to_string())?
                    }
                };
                action_bar_show_result(result, text, item.title, app.clone(), true);
                return Ok(true);
            }

            // 非 auto_translate 的 AI 操作（润色/摘要/解释），仍走 LLM
            let llm_config = crate::config::llm_config_ignore_mode(&config)
                .ok_or("润色模型未配置，请在设置中配置 LLM")?;
            let result = octopus_llm::chat_text_with_prompt(&item.action_data, &text, &llm_config)
                .map_err(|e| e.to_string())?;
            action_bar_show_result(result, text, item.title, app.clone(), true);
            Ok(true)
        }
```

在文件中加入辅助函数：

```rust
use octopus_translation::{TranslationEngine, TranslationManager};

enum TranslateStrategy {
    Local(std::sync::Arc<dyn TranslationEngine>),
    Llm,
}

fn resolve_translate_strategy(config: &octopus_infra::config::AppConfig) -> TranslateStrategy {
    match config.translate_engine.as_str() {
        "llm" => TranslateStrategy::Llm,
        spec if spec.starts_with("local:") => {
            let manager = TranslationManager::new(spec);
            match manager.engine() {
                Ok(Some(e)) => TranslateStrategy::Local(e),
                _ => {
                    log::warn!("本地翻译引擎 {} 加载失败，fallback 到 LLM", spec);
                    TranslateStrategy::Llm
                }
            }
        }
        _ => {
            // 自动：有本地则用本地
            let models = octopus_translation::discover_translation_models();
            if models.iter().any(|m| m.downloaded) {
                let manager = TranslationManager::new("local:m2m100");
                match manager.engine() {
                    Ok(Some(e)) => TranslateStrategy::Local(e),
                    _ => TranslateStrategy::Llm,
                }
            } else {
                TranslateStrategy::Llm
            }
        }
    }
}

fn detect_translate_direction(text: &str) -> (&'static str, &'static str) {
    let has_cjk = text.chars().any(|c| {
        matches!(c as u32, 0x4e00..=0x9fff | 0x3040..=0x30ff | 0xac00..=0xd7af)
    });
    if has_cjk {
        ("zh", "en")
    } else {
        ("en", "zh")
    }
}
```

- [ ] **Step 5: apply_config_value 加 translate_engine**

在 `crates/desktop/src/settings_commands.rs` 的 `apply_config_value` 函数中（L388 `_ =>` 之前）加入：

```rust
        "translate_engine" => {
            cfg.translate_engine = value.as_str()
                .ok_or("translate_engine 必须为字符串")?
                .to_string();
        }
```

- [ ] **Step 6: 编译验证**

Run: `cargo check -p octopus-desktop --features embedded`
Expected: 编译通过

- [ ] **Step 7: Commit**

```bash
git add crates/desktop/ crates/translation/
git commit -m "feat(desktop): translation engine integration + action bar local translate"
```

---

### Task 5: 前端 TranslateTab 模型管理 UI

**Files:**
- Modify: `crates/desktop/frontend/src/pages/Settings/Models/TranslateTab.tsx`
- Modify: `crates/desktop/frontend/src/pages/Settings/ModelsPanel.tsx`（传 showToast prop）

**Interfaces:**
- Consumes: Task 4 的 Tauri 命令

- [ ] **Step 1: ModelsPanel 传 showToast 给 TranslateTab**

在 `crates/desktop/frontend/src/pages/Settings/ModelsPanel.tsx` 中，找到 TranslateTab 渲染处（约 L45），改为：

```tsx
        {tab === "翻译模型" && <TranslateTab showToast={showToast} />}
```

同时在组件签名上加 `showToast` prop（如果 ModelsPanel 已接收 showToast，直接传递）。

检查 ModelsPanel 是否已接收 `showToast`。如果是，直接传。如果不是，需要从父组件传入。

- [ ] **Step 2: 重写 TranslateTab.tsx**

完整替换 `TranslateTab.tsx`，参照 AsrTab 模式。代码较长，以下是完整组件：

```tsx
import { useState, useEffect, useCallback } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { Download, CheckCircle2, Loader2, Languages } from "lucide-react";
import { cn } from "@/lib/utils";
import CollapsibleSection from "./CollapsibleSection";

interface DownloadableModel {
  name: string;
  repo: string;
  description: string;
  sizeMb: number;
}

interface TranslationModelInfo {
  name: string;
  source: string;
  downloaded: boolean;
  sizeMb: number;
  path: string;
}

interface TranslateStatus {
  strategy: string;
  engineName: string;
  available: boolean;
}

interface EngineOption {
  value: string;
  label: string;
  isLocal: boolean;
  downloaded: boolean;
}

interface DownloadProgress {
  repo: string;
  downloaded: number;
  total: number;
}

export default function TranslateTab({ showToast }: { showToast: (msg: string) => void }) {
  const [models, setModels] = useState<TranslationModelInfo[]>([]);
  const [downloadable, setDownloadable] = useState<DownloadableModel[]>([]);
  const [status, setStatus] = useState<TranslateStatus | null>(null);
  const [engineConfig, setEngineConfig] = useState("");
  const [progress, setProgress] = useState<DownloadProgress | null>(null);
  const [busyRepo, setBusyRepo] = useState<string | null>(null);

  const loadData = useCallback(async () => {
    try {
      const [dl, disc, st, cfg] = await Promise.all([
        invoke<DownloadableModel[]>("list_downloadable_translation_models"),
        invoke<TranslationModelInfo[]>("discover_translation_models"),
        invoke<TranslateStatus>("translate_status"),
        invoke<{ config: Record<string, string | number | boolean> }>("get_config"),
      ]);
      setDownloadable(dl);
      setModels(disc);
      setStatus(st);
      setEngineConfig((cfg.config.translate_engine as string) || "");
    } catch (e) {
      showToast("加载翻译模型失败：" + e);
    }
  }, [showToast]);

  useEffect(() => {
    loadData();
    const unlistenProg = listen<DownloadProgress>("download-progress", (e) => {
      setProgress(e.payload);
    });
    const unlistenDone = listen<{ repo: string; error?: string }>("download-done", (e) => {
      setBusyRepo(null);
      setProgress(null);
      if (e.payload.error) {
        showToast("下载失败：" + e.payload.error);
      } else {
        showToast("下载完成");
        loadData();
      }
    });
    return () => {
      unlistenProg.then((fn) => fn());
      unlistenDone.then((fn) => fn());
    };
  }, [loadData]);

  const handleDownload = async (model: DownloadableModel) => {
    if (busyRepo) return;
    setBusyRepo(model.repo);
    try {
      await invoke("download_model", { repo: model.repo });
    } catch (e) {
      setBusyRepo(null);
      showToast("下载启动失败：" + e);
    }
  };

  const handleSetEngine = async (value: string) => {
    setEngineConfig(value);
    try {
      await invoke("set_config", { key: "translate_engine", value });
      showToast(value === "" ? "已切换为自动模式" : `已切换引擎：${value}`);
      loadData();
    } catch (e) {
      showToast("设置失败：" + e);
    }
  };

  // 引擎选项
  const engineOptions: EngineOption[] = [
    { value: "", label: "自动（推荐）", isLocal: false, downloaded: true },
    ...models.map((m) => ({
      value: `local:${m.name.split(" ")[0].toLowerCase()}`,
      label: `${m.name}（本地）`,
      isLocal: true,
      downloaded: m.downloaded,
    })),
    { value: "llm", label: "LLM（远程）", isLocal: false, downloaded: true },
  ];

  return (
    <div className="max-w-[560px]">
      {/* 引擎选择 */}
      <CollapsibleSection icon={Languages} label="翻译引擎" count={status?.engineName || ""}>
        <div className="space-y-2 py-1">
          <select
            className="w-full bg-background border border-border rounded-md px-3 py-2 text-sm outline-none focus:border-voice/50 focus:ring-1 focus:ring-voice/20 transition-all"
            value={engineConfig}
            onChange={(e) => handleSetEngine(e.target.value)}
          >
            {engineOptions.map((opt) => (
              <option key={opt.value} value={opt.value} disabled={opt.isLocal && !opt.downloaded}>
                {opt.label}{opt.isLocal && !opt.downloaded ? "（未下载）" : ""}
              </option>
            ))}
          </select>
          {status?.strategy === "auto" && (
            <p className="text-[11px] text-muted-foreground">
              {models.some((m) => m.downloaded)
                ? `当前将使用本地引擎：${status.engineName}`
                : "未检测到本地翻译模型，将使用 LLM 翻译"}
            </p>
          )}
        </div>
      </CollapsibleSection>

      {/* 模型管理 */}
      <CollapsibleSection
        icon={Download}
        label="翻译模型"
        count={`${models.filter((m) => m.downloaded).length}/${models.length}`}
      >
        {downloadable.map((model) => {
          const local = models.find((m) => m.source === model.repo);
          const downloaded = local?.downloaded ?? false;
          const isBusy = busyRepo === model.repo;
          return (
            <div key={model.repo} className="flex items-center gap-3 py-2 px-3 rounded-md hover:bg-muted/30">
              <div className="flex-1 min-w-0">
                <div className="flex items-center gap-2">
                  <span className="text-xs font-medium">{model.name}</span>
                  {downloaded && <CheckCircle2 className="w-3.5 h-3.5 text-emerald-500 shrink-0" />}
                </div>
                <span className="text-[11px] text-muted-foreground">{model.description} · {model.sizeMb}MB</span>
              </div>
              {isBusy && progress ? (
                <div className="flex items-center gap-2 shrink-0">
                  <div className="w-20 h-1.5 bg-muted rounded-full overflow-hidden">
                    <div
                      className="h-full bg-voice transition-all"
                      style={{ width: `${(progress.downloaded / progress.total) * 100}%` }}
                    />
                  </div>
                  <Loader2 className="w-3.5 h-3.5 animate-spin text-voice shrink-0" />
                </div>
              ) : downloaded ? (
                <span className="text-[11px] text-emerald-600 shrink-0">已下载</span>
              ) : (
                <button
                  onClick={() => handleDownload(model)}
                  disabled={!!busyRepo}
                  className="shrink-0 rounded-md bg-voice/10 px-2.5 py-1 text-[11px] font-medium text-voice transition-colors hover:bg-voice/20 disabled:opacity-40"
                >
                  下载
                </button>
              )}
            </div>
          );
        })}
      </CollapsibleSection>
    </div>
  );
}
```

- [ ] **Step 3: 前端编译验证**

Run: `cd crates/desktop/frontend && ./node_modules/.bin/tsc --noEmit`
Expected: 无类型错误

- [ ] **Step 4: Commit**

```bash
git add crates/desktop/frontend/
git commit -m "feat(frontend): TranslateTab model management + engine selector UI"
```

---

### Task 6: 文档同步 + 整体验证

**Files:**
- Modify: `docs/architecture.md`
- Modify: spec 状态

- [ ] **Step 1: 全量编译**

Run: `cargo build -p octopus-translation -p octopus-infra`
Expected: 编译通过

- [ ] **Step 2: 全量测试**

Run: `cargo test -p octopus-translation -p octopus-infra`
Expected: 全部 PASS

- [ ] **Step 3: Desktop 编译检查**

Run: `cargo check -p octopus-desktop --features embedded`
Expected: 编译通过

- [ ] **Step 4: 前端编译检查**

Run: `cd crates/desktop/frontend && ./node_modules/.bin/tsc --noEmit`
Expected: 无错误

- [ ] **Step 5: 更新 architecture.md**

在 `docs/architecture.md` 中加入翻译引擎模块描述：
- workspace crate 列表加 `crates/translation/`
- 模块表加 `octopus-translation`（m2m100 本地翻译引擎）
- action bar 翻译流程描述更新（本地引擎优先 + LLM fallback）

- [ ] **Step 6: 更新 spec 状态**

将 `docs/superpowers/specs/2026-07-12-local-translation-engine-design.md` 状态改为"已实现"。

- [ ] **Step 7: Commit**

```bash
git add docs/
git commit -m "docs: sync architecture + spec status for translation engine"
```


---

## 代码审查修复记录（2026-07-12）

| 问题 | 修复 |
|------|------|
| A. 魔数 128022 静默回退 | `lang_code_to_id` miss 时 `bail!` 并附 context；新增 `FALLBACK_LANG_ID` 具名常量 |
| B. 翻译线程结果投递到错误 temp tab | `translate-done` 事件改为 `{ key, text }` JSON 格式（前端兼容旧 string 格式） |
| C. 多段翻译 `\\n` 拼接改变原文结构 | `results.join("\\n")` → `results.join("")`（句子切分已保留标点/换行） |
| D. 测试旧 repo 名 `venddair/...` + 无模型 panic | 改为 `lazycodepersona/m2m100_418m` + `#[ignore]` |
| E. 文档引用旧文件名 | spec/plan 中 `encoder_model.onnx` → `*_quantized.onnx`、`sentencepiece.bpe.model` → `tokenizer.json`、删除 `apply_session_acceleration` |
| F. clippy | engine.rs 提 type 别名 `GlobalEngine` |
