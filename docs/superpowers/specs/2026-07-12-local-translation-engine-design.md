# 本地翻译引擎设计（m2m100 ONNX）

> **状态**：已实现
> **日期**：2026-07-12
> **scope**：接入 m2m100-418M ONNX int8 本地翻译引擎，新建 `octopus-translation` crate，支持中⇄英离线翻译，与现有 LLM 翻译共存，用户可配置切换
> **前置文档**：[`2026-07-09-action-bar-menu-db-design.md`](./2026-07-09-action-bar-menu-db-design.md)（action bar DB 化，翻译功能走 `auto_translate`）

---

## 1. 背景与动机

当前 action bar 的"翻译"项（`auto_translate`）完全依赖远程 LLM（`chat_text_with_prompt` 发 HTTP 请求到 OpenAI-compatible endpoint）。缺点：

- 需要网络连接
- 有延迟（网络往返 + LLM 推理）
- 消耗 API 额度

接入本地 m2m100-418M ONNX int8 模型后：
- 完全离线，零网络延迟
- 中⇄英翻译质量足够日常使用（418M 参数，支持 100+ 语言互译）
- 与现有 ASR ONNX 架构一致（`ort` crate + `apply_session_acceleration`）

### 设计约束（已确认）

| 决策点 | 选择 |
|--------|------|
| 翻译方案 | **m2m100-418M ONNX int8 优先**（与 `ort` 架构契合） |
| 与远程 LLM 关系 | **用户配置选择**：配置了本地引擎则用本地；未配置则 fallback LLM |
| 引擎架构 | **新建 `octopus-translation` crate**（独立于 ASR） |
| 解码策略 | **先 greedy**（一期跑通），后续视质量决定是否加 beam search |
| Tokenizer | **`sentencepiece` Rust crate**（加载 `.model` 文件） |
| 引擎发现 | **动态扫描**（HF cache + `~/.octopus/models/`） |
| 配置 UI | **TranslateTab 内**（模型管理 + 引擎选择一站式） |

---

## 2. 模型信息

### 2.1 m2m100-418M-onnx-int8

| 属性 | 值 |
|------|-----|
| HF repo | `venddair/m2m100-418M-onnx-int8` |
| 架构 | M2M100 encoder-decoder（FairSeq） |
| 参数量 | 418M |
| 精度 | int8 量化 |
| Encoder ONNX | 448MB |
| Decoder ONNX | 274MB |
| Tokenizer | `sentencepiece.bpe.model`（2.3MB，SentencePiece BPE） |
| vocab_size | 128,112 |
| d_model | 1024 |
| 语言数 | 100+（语言标记：`__en__` / `__zh__` / `__ja__` 等） |

### 2.2 ONNX 节点结构

**Encoder**：
- 输入：`input_ids` [batch, enc_seq] + `attention_mask` [batch, enc_seq]
- 输出：`last_hidden_state` [batch, enc_seq, 1024]

**Decoder**（`use_cache: false`，无 past_key_values）：
- 输入：`input_ids` [batch, dec_seq] + `encoder_hidden_states` [batch, enc_seq, 1024] + `encoder_attention_mask` [batch, enc_seq]
- 输出：`logits` [batch, dec_seq, 128112]

### 2.3 关键 token IDs

| token | id | 用途 |
|-------|----|------|
| `<s>` (bos) | 0 | 序列起始 |
| `<pad>` | 1 | 填充 |
| `</s>` (eos) | 2 | 序列结束 / decoder 起始 |
| `<unk>` | 3 | 未知 token |
| `__en__` / `__zh__` / ... | special_tokens | 语言标记 |

### 2.4 generation_config

- `decoder_start_token_id`: 2
- `eos_token_id`: 2
- `max_length`: 200
- `num_beams`: 5（一期不使用，走 greedy）
- `early_stopping`: true

---

## 3. Crate 结构

```
crates/translation/              # octopus-translation
├── Cargo.toml
└── src/
    ├── lib.rs                   # 公开 API + re-exports
    ├── engine.rs                # TranslationEngine trait + TranslationManager
    ├── m2m100.rs                # m2m100 ONNX 引擎实现
    ├── tokenizer.rs             # SentencePiece BPE 封装 + 语言标记处理
    └── discovery.rs             # 模型发现（HF cache + 本地目录扫描）
```

**依赖**：`ort`（ONNX Runtime）、`sentencepiece`（tokenizer）、`octopus-infra`（模型发现路径 + 错误类型）。不依赖 `asr-local`、`llm`、`desktop`。

---

## 4. 引擎抽象

### 4.1 TranslationEngine trait

```rust
/// 本地翻译引擎 trait。支持多引擎扩展（m2m100 / NLLB / 等）。
pub trait TranslationEngine: Send + Sync {
    /// 翻译文本。source_lang / target_lang 用 ISO 639-1 代码（"zh" / "en"）。
    fn translate(&self, text: &str, source_lang: &str, target_lang: &str) -> Result<String>;

    /// 引擎显示名（如 "m2m100-418M"）。
    fn name(&self) -> &str;
}
```

### 4.2 TranslationManager

```rust
/// 翻译引擎管理器——lazy load + 缓存，类似 AsrEngineManager。
pub struct TranslationManager {
    engine: parking_lot::Mutex<Option<Arc<dyn TranslationEngine>>>,
    engine_spec: String,  // 如 "local:m2m100"
}

impl TranslationManager {
    /// 创建管理器。engine_spec 为空表示"自动"或"无本地引擎"。
    pub fn new(engine_spec: &str) -> Self;

    /// 获取引擎（lazy load：首次调用时加载模型）。
    pub fn engine(&self) -> Result<Option<Arc<dyn TranslationEngine>>>;

    /// 引擎是否已就绪（模型已下载 + 可加载）。
    pub fn is_available(&self) -> bool;
}
```

- 一期只支持 `local:m2m100` 一种引擎 spec
- 缓存 `Arc<dyn TranslationEngine>`，进程生命周期内不重复加载
- 线程安全（`parking_lot::Mutex`，与 ASR 引擎一致）

---

## 5. m2m100 引擎实现

### 5.1 模型加载

```rust
pub struct M2M100Engine {
    encoder: parking_lot::Mutex<Session>,
    decoder: parking_lot::Mutex<Session>,
    tokenizer: M2M100Tokenizer,
}
```

**模型发现**：复用 `resolve_model_dir` 模式（从 `octopus-asr-local/src/config.rs` 提取到 `octopus-infra` 或在本 crate 内实现等效逻辑）：
1. `~/.octopus/models/<source>` 
2. HF cache：`~/.cache/huggingface/hub/models--venddair--m2m100-418M-onnx-int8/snapshots/<hash>/`

**加载文件**：`encoder_model.onnx` + `decoder_model.onnx` + `sentencepiece.bpe.model`

**ONNX session**：`apply_session_acceleration`（CoreML/CUDA/DirectML，从 ASR 层复用或提取到 infra）

### 5.2 Tokenizer 封装

```rust
pub struct M2M100Tokenizer {
    sp: sentencepiece::SentenceProcessor,
}

impl M2M100Tokenizer {
    pub fn load(model_path: &Path) -> Result<Self>;

    /// 编码：text → token ids。在 tokens 前插入源语言标记（如 `__zh__`）。
    pub fn encode(&self, text: &str, source_lang: &str) -> Result<Vec<i64>>;

    /// 解码：token ids → text。过滤语言标记 token。
    pub fn decode(&self, ids: &[i64]) -> Result<String>;
}
```

**语言代码映射**：ISO 639-1 → m2m100 标记

```rust
fn lang_code_to_token(lang: &str) -> &str {
    match lang {
        "zh" => "__zh__",
        "en" => "__en__",
        "ja" => "__ja__",
        "ko" => "__ko__",
        "fr" => "__fr__",
        "de" => "__de__",
        "es" => "__es__",
        "ru" => "__ru__",
        // ... 完整 100+ 语言映射
        _ => "__en__",
    }
}
```

### 5.3 推理流程（Greedy）

```
输入：text = "你好世界", source_lang = "zh", target_lang = "en"

1. Encode
   tokens = tokenizer.encode("你好世界", "zh")
   → input_ids = [lang_token_id(zh), ...text_tokens, eos_id]
   attention_mask = [1] * len(input_ids)

2. Encoder forward
   encoder_hidden = encoder.run(input_ids, attention_mask)
   → [1, seq_len, 1024]

3. Decoder greedy loop (max_length=200)
   decoder_ids = [decoder_start_token_id(2)]
   loop:
     logits = decoder.run(decoder_ids, encoder_hidden, attention_mask)
     next_token = argmax(logits[-1])  // 取最后位置
     if next_token == eos_token_id(2): break
     decoder_ids.append(next_token)

4. Decode
   result = tokenizer.decode(decoder_ids[1:])  // 跳过 start token
   → "Hello world"
```

**性能考量**：
- `use_cache: false` → 每步 decoder 都传完整已生成序列，序列越长越慢
- encoder 只跑一次，结果缓存
- 实际短文本（句子级翻译）dec_seq 通常 < 50，性能可接受

---

## 6. 模型发现（动态扫描）

### 6.1 发现函数

```rust
/// 扫描本地已下载的翻译模型。
pub fn discover_translation_models() -> Vec<TranslationModelInfo>;

pub struct TranslationModelInfo {
    pub name: String,          // "m2m100-418M"
    pub source: String,        // "venddair/m2m100-418M-onnx-int8"
    pub downloaded: bool,      // 模型文件是否完整
    pub size_mb: u64,          // 总大小（encoder + decoder + tokenizer）
    pub path: String,          // 模型目录路径
}
```

### 6.2 可下载模型列表

```rust
/// 返回支持的翻译模型列表（含未下载的），用于设置页展示。
pub fn list_downloadable_translation_models() -> Vec<DownloadableTranslationModel>;

pub struct DownloadableTranslationModel {
    pub name: String,
    pub repo: String,          // HF repo id
    pub description: String,
    pub size_mb: u64,
}
```

一期列表硬编码：

| name | repo | description | size |
|------|------|-------------|------|
| m2m100-418M (int8) | `venddair/m2m100-418M-onnx-int8` | 多语言翻译（100+ 语言互译） | ~724MB |

### 6.3 下载验证

复用现有 `download_model` / `verify_model` 命令（`model_commands.rs`），通过 repo id 下载。模型完整性校验：检查 `encoder_model.onnx` + `decoder_model.onnx` + `sentencepiece.bpe.model` 三个文件是否存在。

---

## 7. 配置

### 7.1 AppConfig 新增字段

```rust
pub struct AppConfig {
    // ... 现有字段 ...
    pub translate_engine: String,  // "" = 自动, "local:m2m100" = 指定本地引擎, "llm" = 强制 LLM
}
```

默认值：`""`（自动模式）

### 7.2 引擎解析逻辑

```rust
/// 解析翻译引擎配置，返回翻译策略。
pub enum TranslateStrategy {
    Local(Arc<dyn TranslationEngine>),  // 本地引擎已就绪
    Llm,                                 // 使用远程 LLM
}

pub fn resolve_translate_strategy(config: &AppConfig) -> TranslateStrategy {
    match config.translate_engine.as_str() {
        "" => {
            // 自动：扫描本地模型，有则用本地，否则 LLM
            let models = discover_translation_models();
            if let Some(m) = models.into_iter().find(|m| m.downloaded) {
                // lazy load 本地引擎
                TranslateStrategy::Local(load_engine(&m))
            } else {
                TranslateStrategy::Llm
            }
        }
        "llm" => TranslateStrategy::Llm,
        spec if spec.starts_with("local:") => {
            // 指定本地引擎
            let engine_name = &spec["local:".len()..];
            load_engine_by_name(engine_name)
                .map(TranslateStrategy::Local)
                .unwrap_or(TranslateStrategy::Llm)  // 加载失败 fallback
        }
        _ => TranslateStrategy::Llm,
    }
}
```

---

## 8. action bar 翻译执行流程

### 8.1 修改 `execute_action_bar_inner`（`action_bar_commands.rs`）

当前 `auto_translate` 分支：

```rust
"ai" => {
    let prompt = if item.action_data == "auto_translate" {
        auto_translate_prompt(&text)
    } else {
        &item.action_data
    };
    let result = octopus_llm::chat_text_with_prompt(prompt, &text, &llm_config)?;
    // ...
}
```

改为：

```rust
"ai" => {
    let result = if item.action_data == "auto_translate" {
        // 翻译：先查本地引擎配置
        let (source_lang, target_lang) = detect_translate_direction(&text);
        match resolve_translate_strategy(&config) {
            TranslateStrategy::Local(engine) => {
                engine.translate(&text, source_lang, target_lang)?
            }
            TranslateStrategy::Llm => {
                let prompt = auto_translate_prompt(&text);
                octopus_llm::chat_text_with_prompt(prompt, &text, &llm_config)?
            }
        }
    } else {
        // 非 auto_translate 的 AI 操作（润色/摘要/解释），仍走 LLM
        octopus_llm::chat_text_with_prompt(&item.action_data, &text, &llm_config)?
    };
    action_bar_show_result(result, text, item.title, app.clone(), true);
    Ok(true)
}
```

### 8.2 方向检测

复用现有 `auto_translate_prompt` 的 CJK 检测逻辑：

```rust
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

---

## 9. 前端 UI

### 9.1 TranslateTab（`Models/TranslateTab.tsx`）

替换现有 placeholder，参照 `AsrTab` 模式：

**本地模型区**（`CollapsibleSection` "翻译模型"）：
- 调用 `list_downloadable_translation_models` 获取可下载列表
- 每行：模型名 + 描述 + 大小 + 下载/校验按钮 + 当前使用标记
- 下载进度条（复用 `download-progress` / `download-done` 事件）

**引擎选择区**：
- 下拉选择翻译引擎，选项动态生成：
  - "自动（推荐）" — `translate_engine = ""`
  - 扫描到的已下载本地引擎（如 "m2m100-418M 本地"）— `translate_engine = "local:m2m100"`
  - "LLM（远程）" — `translate_engine = "llm"`
- 自动模式下显示当前实际使用的引擎（扫描到本地则标注"将使用 m2m100 本地"，否则"将使用 LLM"）

### 9.2 Tauri 命令

```rust
#[tauri::command]
fn list_downloadable_translation_models() -> Result<Vec<DownloadableTranslationModel>, String>;

#[tauri::command]
fn discover_translation_models() -> Result<Vec<TranslationModelInfo>, String>;
```

下载 / 校验复用现有 `download_model` / `verify_model`（按 repo id）。

---

## 10. 不在本次范围

- **Beam search 解码** — 一期 greedy，视质量决定是否加
- **其他翻译模型**（NLLB / Argos Translate / 云端翻译 API）— 架构预留，后续按需加
- **per-action-bar-item 翻译引擎配置** — 一期全局配置
- **批量翻译** — 一期仅支持 action bar 选中文本翻译
- **翻译缓存** — 不缓存翻译结果
- **翻译质量对比** — 不提供本地 vs LLM 对比视图
