# DB 单一配置源设计（删 model.json / history.txt）

> 状态：✅ 已实现（2026-06-14）。详见 [`architecture.md`](../../../architecture.md)「模型管理」段。

## 背景

重构前模型配置散落三处：`model.json`（asr 读）、SQLite `models` 表（desktop 启动时注入）、HF 缓存发现。架构文档称 cli/server 读 model.json、desktop 注入 DB——但 DB 已是更优运行时源，model.json / history.txt 是历史遗留，且 cli/server 读 model.json 与 desktop 读 DB 的分裂带来维护负担。

## 目标

1. **Silero VAD 固定** `~/.octopus/models/silero_vad_v4.onnx`（随应用打包，不再读配置）
2. **彻底删 history.txt 代码**（DB 模式已接管）
3. **彻底删 model.json 代码** —— DB 成为模型配置唯一来源，cli/server/desktop 统一从 `~/.octopus/octopus.db` 读；默认 ASR = zipformer（27M，`~/.octopus/models/zipformer`，随应用打包）

## 设计决策

- **DB 唯一源**：`models` 表是模型配置唯一真相。`config::load_config()` 读 DB（lazy init：首次 `ensure_db` 建表 + seed）。
- **DB 承载在 infra crate**：`crates/infra/src/db.rs` + `crates/infra/src/db.sql`（schema 经历 desktop/db.rs → asr/db.rs → infra/db.rs 三次下沉，最终落 infra 供全 workspace 共用）。asr crate 经 `pub use octopus_infra::db` 以 `crate::db` 暴露；cli/server/desktop/asr 四端共用。
- **固定路径 + HF 双模式 source**：`resolve_model_dir(source)` 优先本地（`octopus_config_home()/source` 或绝对路径），回退 HF 缓存。zipformer-small-ctc 走本地打包路径，其他引擎走 HF repo 名。
- **路径常量与 home 解析集中**：VAD 路径（`SILERO_VAD_PATH`）、默认 ASR 目录（`DEFAULT_ASR_MODEL_DIR`）与 `octopus_config_home()`（原 `handy_home()`，三端各处自建）统一收敛到 [`infra` crate](2026-06-14-infra-crate-design.md)，单一来源。
- **VAD 固定**：`find_silero_vad()` 固定返回 `~/.octopus/models/silero_vad_v4.onnx`，删 `VadSection` / `AppConfig.vad`。
- **seed 默认引擎集**：首次建库 `db.sql` 的 `INSERT OR IGNORE` 写入默认引擎集（见下表），删 model.json 零功能损失。

## 数据流

```
load_config() ─首次→ db::ensure_db()(建表+seed) → db::load_models() → AppConfig(缓存 OnceLock)
                        ↑                                          ↑
            cli / server / desktop 三端无差别统一调用        读 models 表 domain='asr'
```

## 默认引擎集（db.sql seed）

ASR 引擎每行还带三个标志列：`is_local`（本地/远程）、`is_enabled`（`load_models_at` 仅读 `is_enabled=1`）、`is_streaming`（流式判定，见下）。激活**不靠表内标志**，而由 `config.yaml.asr_engine` 按 `name` 精确匹配（见 [config-infra 设计](2026-06-14-config-infra-and-engine-truth-design.md)）。

| category | name | source | is_local | is_enabled | is_streaming |
|---|---|---|---|---|---|
| zipformer | zipformer-small-ctc | `models/zipformer`（本地打包，兜底） | 1 | 1 | 1 |
| zipformer | zipformer-multi | k2-fsa/sherpa-onnx-streaming-zipformer-ctc-multi-zh-hans-int8-2023-12-13 | 1 | 0 | 1 |
| zipformer | zipformer-ctc | csukuangfj/sherpa-onnx-streaming-zipformer-ctc-zh-int8-2025-06-30 | 1 | 0 | 1 |
| paraformer | paraformer-streaming | csukuangfj/sherpa-onnx-streaming-paraformer-zh | 1 | 0 | 1 |
| sensevoice | sherpa-onnx-sense-voice-funasr-nano-int8 | csukuangfj/sherpa-onnx-sense-voice-funasr-nano-int8-2025-12-17 | 1 | 0 | 0 |
| qwen3-asr | qwen3-asr-0.6B | csukuangfj2/sherpa-onnx-qwen3-asr-0.6B-int8-2026-03-25 | 1 | 0 | 0 |
| qwen3-asr | qwen3-asr-1.7B | ilmina/qwen3-asr-1.7b-sherpa-onnx | 1 | 0 | 0 |
| whisper | whisper-small | onnx-community/whisper-small | 1 | 0 | 0 |

`is_streaming`（zipformer/paraformer=1，whisper/sensevoice/qwen3=0）驱动 `is_streaming_engine()`（数据驱动，不再按 category 硬编码）。

VAD 不进表（固定路径，`find_silero_vad` 直接返回）。

## 关键约束

- 手编 `models` 表需重启进程生效（`OnceLock` 缓存 AppConfig，运行中不可热更新）。
- 删迁移后老用户 `history.txt` / `model.json` 不再迁移（用户已用 DB 模式；新机器直接 seed）。
- seed 硬编码引擎集，更新需改 `crates/infra/src/db.sql`（模型配置低频变动，可接受）。
