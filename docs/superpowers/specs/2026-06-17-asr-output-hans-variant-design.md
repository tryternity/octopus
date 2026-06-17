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
