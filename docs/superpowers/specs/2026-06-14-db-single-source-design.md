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
- **asr crate 承担 DB**：`crates/asr/src/db.rs`（从 desktop/db.rs 下沉）统一管 models + transcriptions，避免 user_version 门控分裂、避免两 crate 各连一个 DB。cli/server/desktop 三端共用。
- **固定路径 + HF 双模式 source**：`resolve_model_dir(source)` 优先本地（`octopus_config_home()/source` 或绝对路径），回退 HF 缓存。zipformer-small-ctc 走本地打包路径，其他引擎走 HF repo 名。
- **路径常量与 home 解析集中**：VAD 路径（`SILERO_VAD_PATH`）、默认 ASR 目录（`DEFAULT_ASR_MODEL_DIR`）与 `octopus_config_home()`（原 `handy_home()`，三端各处自建）统一收敛到 [`infra` crate](2026-06-14-infra-crate-design.md)，单一来源。
- **VAD 固定**：`find_silero_vad()` 固定返回 `~/.octopus/models/silero_vad_v4.onnx`，删 `VadSection` / `AppConfig.vad`。
- **seed 默认引擎集**：首次建库 `seed_default_models` 写入 7 引擎（见下表），删 model.json 零功能损失。

## 数据流

```
load_config() ─首次→ db::ensure_db()(建表+seed) → db::load_models() → AppConfig(缓存 OnceLock)
                        ↑                                          ↑
            cli / server / desktop 三端无差别统一调用        读 models 表 domain='asr'
```

## 默认引擎集（DEFAULT_MODELS）

| category | name | source | active |
|---|---|---|---|
| zipformer | zipformer-small-ctc | `models/zipformer`（本地打包） | ✅ |
| zipformer | zipformer-multi | k2-fsa/sherpa-onnx-streaming-zipformer-ctc-multi-zh-hans-int8-2023-12-13 | |
| zipformer | zipformer-ctc | csukuangfj/sherpa-onnx-streaming-zipformer-ctc-zh-int8-2025-06-30 | |
| paraformer | paraformer-streaming | csukuangfj/sherpa-onnx-streaming-paraformer-zh | |
| sensevoice | sherpa-onnx-sense-voice-funasr-nano-int8 | csukuangfj/sherpa-onnx-sense-voice-funasr-nano-int8-2025-12-17 | |
| qwen3-asr | qwen3-asr-0.6B | csukuangfj2/sherpa-onnx-qwen3-asr-0.6B-int8-2026-03-25 | |
| whisper | whisper-small | onnx-community/whisper-small | |

VAD 不进表（固定路径，`find_silero_vad` 直接返回）。

## 关键约束

- 手编 `models` 表需重启进程生效（`OnceLock` 缓存 AppConfig，运行中不可热更新）。
- 删迁移后老用户 `history.txt` / `model.json` 不再迁移（用户已用 DB 模式；新机器直接 seed）。
- seed 硬编码引擎集，更新需改 `DEFAULT_MODELS` 常量（模型配置低频变动，可接受）。
