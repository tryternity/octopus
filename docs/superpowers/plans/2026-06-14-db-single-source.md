# DB 单一配置源实施计划

> 状态：✅ 全部完成（2026-06-14）。对应 spec：[`specs/2026-06-14-db-single-source-design.md`](../specs/2026-06-14-db-single-source-design.md)

## 阶段 A：asr 引入 DB ✅
- [x] `asr/Cargo.toml` 加 `rusqlite`（bundled）+ `log`
- [x] 新增 `asr/src/db.rs`（从 desktop/db.rs 下沉 models + transcriptions；加 `seed_default_models`；删 `migrate_history` / `migrate_model_json` / `active_engine` / `HistoryEntry` / `parse_history_entries`）
- [x] `ensure_db` 幂等（user_version 门控 + lazy init）
- [x] `lib.rs` 注册 `pub mod db`

## 阶段 B：config 读 DB + VAD 固定 ✅
- [x] `load_config()` 改读 DB（`ensure_db` + `load_models`，缓存 `OnceLock`）
- [x] `find_silero_vad()` 固定 `~/.octopus/models/silero_vad_v4.onnx`
- [x] 新增 `resolve_model_dir(source)`（本地优先 / HF 回退）
- [x] 删 `VadSection` / `SimpleModelEntry` / `AppConfig.vad` / `set_runtime_config`

## 阶段 C：引擎统一 resolve_model_dir ✅
- [x] 7 引擎模块：whisper(×3) / sensevoice / paraformer / qwen3_asr / zipformer / streaming_zipformer / streaming_paraformer

## 阶段 D：desktop 瘦身 ✅
- [x] 删 `desktop/src/db.rs` + `main.rs` 的 `mod db;`
- [x] `main.rs`: `db::init`→`octopus_asr::db::ensure_db`；删 `load_app_config` + `set_runtime_config` 注入两步
- [x] `coordinator.rs`: `insert_transcription` 改调 `octopus_asr::db`
- [x] `desktop/Cargo.toml` 移除直接 `rusqlite` 依赖（asr 传递提供）

## 阶段 E：cli/server 注释 + Config 展示 ✅
- [x] cli / server 注释「from model.json」→「from DB」
- [x] cli `Config` 子命令 `find_hf_cache`→`resolve_model_dir`（5 处）+ 删 `vad active` 展示（VAD 固定路径无 active）

## 阶段 F：文档同步 ✅
- [x] `architecture.md` 重写「文本持久化」+「模型管理」段（DB 唯一源、固定路径、resolve_model_dir、三端统一 load_config）
- [x] 本 spec + plan

## 验证

- [x] `cargo check --workspace` 通过（含 desktop embedded）
- [x] `cargo test -p octopus-asr` 9 测试通过（6 新 db 单测 + 3 原有 zipformer/streaming）
- [x] **手动端到端**（用户执行，2026-06-14 通过）：
  - 备份后删 `~/.octopus/octopus.db` → 启动 desktop → 确认自动建表 + seed（zipformer-small-ctc active）
  - `config.yaml` asr_engine=`zipformer-small-ctc` → 录音识别（走本地 `~/.octopus/models/zipformer`）
  - asr_engine=`sensevoice` → 识别（走 HF 缓存，验证 resolve_model_dir 回退）
  - 确认运行后 `model.json` / `history.txt` 未被读写
  - `octopus-cli config` → 显示 DB 引擎列表
