# ASR 句间分隔符 i18n 统一

> **状态**：已实施（2026-06-26）。源自 bug 报告审查 → 用户「统一处理」。

## 背景

ASR 多句/多段文本拼接的句间分隔符，全 workspace 原硬编码中文逗号 `'，'`（U+FF0C）。
`language=en` 时，英文文本被插入中文全角逗号，不规范（如 `"Hello world，How are you"`）。

审查发现共约 16 处同类拼接，分布在 3 个 crate、多条路径：

| crate | 文件 | 处 | 路径语义 |
|---|---|---|---|
| asr-cloud | `aliyun_stream.rs` | 4 | 云端流式 Fun-ASR/Qwen 句间拼接 |
| asr-local | `streaming_engine.rs` | 9 | 本地流式静音分句（7 `push('，')` + 2 `format!("{}，{}")`） |
| desktop | `engine_aliyun.rs` | 4 | 桌面分块云端 `collect_results` |
| desktop | `coordinator.rs` | 1 | `finalize_cloud` 跨 utterance |
| desktop | `pipeline.rs` | 1 | `consume_completed_results_vad` 段间 |
| desktop | `cloud_pipeline.rs` | 1 | `drain_cloud_session` 提交 |

> asr-cloud `aliyun_stream.rs` 已先于本次统一单独修复（commit `e487aaf`，私有 helper）；
> 本次将其提升为公共 helper 并推广到全 workspace。

## 设计

### 共享 helper

`sentence_separator(language: &str) -> &'static str`，落点 **`asr-local/src/paraformer.rs`**
（紧邻 `smart_append`），`lib.rs` re-export 为 `octopus_asr_local::sentence_separator`。

- **落点理由**：`asr-local` 是 asr-cloud / desktop / server / cli 的共同依赖底座
  （asr-cloud → asr-local，desktop → asr-local + asr-cloud），放此处零循环依赖、零新依赖边。
- **取值**：`en`（大小写不敏感）→ `" "`（空格）；其他（`zh`/`auto`/空）→ `"，"`。
- **英文用空格的理由**：英文 ASR 句子常自带句末标点（`.`/`!`/`?`），空格连接最自然且
  不与之冲突；若用英文逗号 `,` 或句号 `.` 会与服务端标点打架。中文/auto 保持 `，`（口语
  连续叙述的连贯感）。

### 接口变化

| 接口 | 变化 |
|---|---|
| `StreamingSession::new` | 加 `language: &str` 参数；三 variant（Paraformer/ZipformerCtc/ZipformerTransducer）各存 `separator: &'static str` 字段，构造时由 `sentence_separator(language)` 算出 |
| `consume_completed_results_vad` | 加 `language: &str` 参数（pub(crate) fn，3 调用方：1 生产 + 2 测试） |
| `collect_results`（desktop engine_aliyun） | 加 `language: &str` 参数 |
| `drain_cloud_session` / `CloudDrainState` | `CloudDrainState` 加 `language: &'a str` 字段（借用结构），构造处（`CloudPipelineEngine::tick`）传 `&self.language` |

### language 可达性

各拼接点均已能拿到 language（本次确认）：

- **本地流式**：两调用方（`coordinator.rs:613/620`、`server/main.rs:202`）创建时已持有
  `config.language` / query `language`（server 原 `_language` 未用，改回 `language`）。
  从 DB `ModelEntry.language` 推断不可靠（字段可能空 + 模型本质单语言，与用户意图冲突），
  故走「调用方传 user-intent language」路径。
- **desktop cloud/段间**：`coordinator` 构造各 pipeline 时已快照 `config.language.clone()`
  给 `CloudPipelineEngine.language` / `VadSegmentedPipeline.language`；`finalize_cloud`
  直接读 `config.language`。
- **engine_aliyun**：`transcribe` 已收 `language`，透传给 `collect_results`。

## 验证

- `cargo test -p octopus-asr-local -p octopus-asr-cloud -p octopus-server -p octopus-desktop`：全绿
  （含新增 `paraformer::tests::sentence_separator_by_language`）。
- `cargo clippy`（desktop `--features cloud`）：改动文件零新 warning（命中的 `*acc` deref /
  collapsible-if 均为既有周边结构，separator 改动不引入新类型）。

## 不在范围

- coordinator/desktop 侧历史 plan（`stage2c2.md`/`vad-segmented-rehome.md`）里的「逗号拼接」
  描述为已合并的历史实施记录，按惯例不回溯修改；本 spec 作为现状设计文档。
