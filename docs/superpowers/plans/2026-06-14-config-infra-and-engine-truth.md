# config.yaml 下沉 infra + ASR 引擎选择单一真相 实施计划

> **For agentic workers:** REQUIRED SUB-SKILL: superpowers:subagent-driven-development 或 superpowers:executing-plans。**本计划已全部实现，存档备查。**

**Goal:** config.yaml schema 与读取统一下沉到 `infra::AppConfig`；引擎激活以 `config.yaml.asr_engine` 为唯一真相（DB name 精确匹配 + 兜底）；删除 DB `models.is_active` 列。

**Architecture:** infra 新增 `config` 模块承载统一 `AppConfig`；asr 侧 `AppConfig` 重命名为 `AsrConfig` 并新增 `resolve_active_engine` 兜底解析；5 引擎模块级 transcribe 加 name 参数修正多引擎取错 bug；desktop/cli/server 适配。

**Tech Stack:** Rust workspace, serde/serde_yaml, rusqlite (bundled, DROP COLUMN)

---

## 任务分解（全部已完成 ✅）

### Task 1: infra 新增 config 模块（阶段 A，独立提交）

- [x] `crates/infra/Cargo.toml`：加 `serde`/`serde_yaml`/`anyhow`
- [x] `crates/infra/src/config.rs`：新建 `AppConfig`（18 字段，从 desktop/config.rs 整体迁移）+ 所有 `default_*` + `Default` impl + `load_config()`（读 `octopus_config_home()/config.yaml`，缺失返回 Default）
- [x] **有意变更**：`asr_engine` serde 默认值 `"sensevoice"` → `""`（幽灵值，改空后兜底语义清晰）
- [x] `crates/infra/src/lib.rs`：`pub mod config;`
- [x] `cargo check -p octopus-infra`：0 error

### Task 2: asr/db 删 is_active + v1→v2 migration（阶段 B）

- [x] `struct DefaultModel` 删 `is_active` 字段；7 条 seed 各删 `is_active`
- [x] `create_tables` models 表去 `is_active` 列
- [x] `seed_default_models` INSERT 去 is_active 列与占位
- [x] `init_schema` 改 match user_version：`0`→建表+seed→v2；`1`→`ALTER TABLE models DROP COLUMN is_active`（transaction）→v2；`_`→no-op
- [x] `load_models_at`：SELECT 去 is_active、query_map 去 7 列、删 `if is_active==1 { asr.active = name }`
- [x] 测试：删 `cfg.asr.active` 断言

### Task 3: asr/config 删 active + 新增兜底解析（阶段 C）

- [x] `AppConfig` → `AsrConfig`（重命名消除与 infra 的同名冲突）
- [x] `AsrSection` 删 `active: String` 字段
- [x] 删 `AppYamlConfig` + `load_app_config`（被 infra 取代）
- [x] 新增 `ResolvedEngine { name, category, entry }`
- [x] 新增 `resolve_active_engine(asr_engine)`：命中用 / 空·不匹配 → 兜底
- [x] 新增 `fallback_engine(cfg)`：DB zipformer-small-ctc 优先，否则硬构造 DEFAULT_ASR_MODEL_DIR
- [x] 新增 `pick_entry(cfg, category, name)`：统一查找（含 lifetime 标注）
- [x] 新增 5 个单测：pick_entry 命中/缺失/section 缺失、fallback 用 DB/硬构造

### Task 4: asr 各引擎模块级 transcribe 加 name（阶段 D）

- [x] `whisper.rs` / `sensevoice.rs`：`iter().next()` → `xxx_cfg.get(name)` + bail
- [x] `paraformer.rs` / `qwen3_asr.rs` / `zipformer.rs`：`if cfg.asr.active / iter().next()` → `xxx_cfg.get(name)` + bail
- [x] 5 个签名 `transcribe(samples, language)` → `transcribe(name: &str, samples, language)`
- [x] `engine.rs`：switch_model 用 `pick_entry` 简化 5 臂 match（去重）

### Task 5: desktop 改用 infra::AppConfig（阶段 E）

- [x] `config.rs`：删 DesktopConfig + 所有 default_* + load_desktop_config；`pub use octopus_infra::config::AppConfig`
- [x] `is_streaming_engine` / `llm_config` 改为接 `&AppConfig` 的自由函数
- [x] `coordinator.rs`：`DesktopConfig` → `AppConfig`（10 处）；`config.is_streaming_engine()` → `crate::config::is_streaming_engine(&config)`；`config.llm_config()` → `crate::config::llm_config(&config)`
- [x] `main.rs`：`load_desktop_config()` → `octopus_infra::config::load_config()`
- [x] `tray.rs` / `overlay.rs` / `paste.rs`：DesktopConfig → AppConfig

### Task 6: cli / server 适配（阶段 F）

- [x] cli `do_transcribe`：5 分支把 `model` 传入模块级 transcribe
- [x] cli `show_config`：`config.asr.active` → `resolve_active_engine` 解析结果展示
- [x] cli 3 处 `load_app_config` → `octopus_infra::config::load_config`
- [x] cli clap 默认值 `"sensevoice"`（幽灵值）→ `"sherpa-onnx-sense-voice-funasr-nano-int8"`（合法 DB name）
- [x] server `config.asr.active` → `resolve_active_engine(&app_cfg.asr_engine)?.name`
- [x] server Cargo.toml 加 `octopus-infra` 依赖

### Task 7: 文档同步（阶段 G）

- [x] `docs/configuration.md`：models 表删 active 列、asr_engine 默认值改空、新增「引擎选择与兜底」专节、示例改 qwen3-asr-0.6B
- [x] `docs/architecture.md`：infra 加 config 模块、asr config 描述更新、模型管理段重写「两份配置 + 引擎选择单一真相」
- [x] 新建本 spec + plan

## 验证

- [x] `cargo check --workspace --all-targets`：0 error
- [x] `cargo test -p octopus-asr -p octopus-infra`：asr 14 passed / 0 failed（含 5 新增 config 单测 + 2 streaming 集成测试）
- [x] e2e `octopus-cli config`：`ASR active: qwen3-asr-0.6B (category: Qwen3Asr, from config.yaml asr_engine='qwen3-asr-0.6B')`
- [x] DB migration：`PRAGMA user_version`=2，`PRAGMA table_info(models)` 无 is_active

## 过程问题（记录备查）

1. **同名冲突**：infra 的 `AppConfig`（yaml）与 asr 的 `AppConfig`（DB）同名。→ asr 侧重命名为 `AsrConfig`（含义更准确），desktop 用 `pub use` re-export infra AppConfig 保持调用简洁。
2. **streaming 测试首跑竞态**：首次 `cargo test` 时真实 DB 还是 v1（含 is_active），并行测试线程触发 migration 时报 "no such column: is_active"。migration 持久化（DB→v2）后重跑全绿。单进程用户不受影响（不会并行 hammer 全局 DB）。
3. **clap 默认值幽灵 bug**：cli 默认 `model="sensevoice"` 不是合法 DB name（DB 里是 `sherpa-onnx-sense-voice-funasr-nano-int8`），原靠 `iter().next()` 隐式兜底。改造后显式暴露 → 默认值改合法 DB name。
4. **pick_entry lifetime**：返回 `Option<&ModelEntry>` 借用自 `cfg`，需显式 `<'a>` lifetime 标注（编译器报 E0106）。
