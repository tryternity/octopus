# 设计文档：config.yaml 下沉 infra + ASR 引擎选择单一真相

> 统一 config.yaml 的 schema 定义到 `infra`；引擎激活以 `config.yaml.asr_engine` 为唯一真相，删除 DB `models.is_active` 列。

## 0. 背景

octopus 存在两组耦合债务：

### 0.1 config.yaml 双 schema

- `asr::AppYamlConfig { microphone }`（cli 用，只读麦克风）
- `desktop::DesktopConfig { 18 字段 }`（desktop 用）

两者各自定义、各自读 `~/.octopus/config.yaml`，重复且易漂移。

### 0.2 引擎选择双真相源

- DB `models.is_active` 列驱动 `asr.active`：被 server `main.rs` 当默认引擎、被各引擎模块级 `transcribe` 当 entry 选择依据（`cfg.asr.active` 或 `iter().next()`）。
- desktop 独立用 `config.yaml.asr_engine`。

两者脱节：seed 里 `is_active=1` 的是 `zipformer-small-ctc`，与各端 `asr_engine` 配置无关。更严重的是模块级 `transcribe` 用 `cfg.asr.active` 取 entry 时，与 cli `do_transcribe(--model)` 传入的 name 不一致——**多引擎 category（zipformer 3 条）会取错引擎**（既有 bug）。

## 1. 目标

1. **config.yaml schema 统一下沉**：`infra::config::AppConfig` 作为统一定义，asr/desktop/cli 共享读取 `load_config()`。各端只读自己关心的字段，多余字段无害。
2. **引擎选择单一真相** = `config.yaml.asr_engine`：按 DB `models` 表 `name` 精确匹配，命中用；空/匹配不到 → 回退兜底 `zipformer-small-ctc`（`DEFAULT_ASR_MODEL_DIR` 本地打包路径）。
3. **DB `models.is_active` 列删除**（v1→v2 migration 自动 `DROP COLUMN`）。
4. **显式参数优先级最高**：cli `--model`、server 请求 `engine`、`AsrEngineManager.switch_model(name)` 直接按 name 精确匹配，不走兜底流程。

## 2. 架构

```
┌──────────────────────────────────────────────────────┐
│ octopus-infra                                         │
│  config::AppConfig  ← config.yaml 统一 schema（18 字段）│
│  config::load_config()  读 ~/.octopus/config.yaml     │
└──────────────────────────────────────────────────────┘
            ▲              ▲              ▲
            │              │              │
       asr::config    desktop::config    cli::main
                        ▼
┌──────────────────────────────────────────────────────┐
│ octopus-asr                                           │
│  config::AsrConfig  ← DB models 表（asr section 目录） │
│  config::resolve_active_engine(asr_engine)            │
│     → ResolvedEngine { name, category, entry }        │
│     命中用 / 空·不匹配 → 兜底 zipformer-small-ctc      │
│  config::pick_entry(cfg, category, name)  统一查找     │
└──────────────────────────────────────────────────────┘
```

**两份配置清晰分离：**
- `infra::config::AppConfig` = 应用行为参数（config.yaml）
- `asr::config::AsrConfig` = DB 模型目录（`models` 表）

## 3. 关键设计决策

### 3.1 命名分离：AppConfig vs AsrConfig

原 `asr::config::AppConfig { asr: AsrSection }` 与新 `infra::config::AppConfig` 同名会冲突。将 asr 侧重命名为 **`AsrConfig`**（更准确——它是 DB 的 asr section 目录，不是整个应用配置）。

### 3.2 asr_engine 默认值改空

`asr_engine` 的 serde 默认值从 `"sensevoice"` 改为 `""`。理由：`"sensevoice"` 在 DB 里无对应 name（DB name 是 `sherpa-onnx-sense-voice-funasr-nano-int8`），本就是匹配不到的幽灵值；改空后「未配置 → 直接兜底 zipformer」语义清晰。

### 3.3 模块级 transcribe 加 name 参数

5 个引擎模块（whisper/sensevoice/paraformer/qwen3_asr/zipformer）的模块级 `transcribe` 加 `name: &str` 参数，内部 `xxx_cfg.get(name)` 精确取 entry，匹配不到 `bail`。

**修正既有 bug**：原 `iter().next()` / `cfg.asr.active` 路径会让 cli `transcribe --model zipformer-multi` 在多引擎 category 里取错（取到第一条 small-ctc）。

### 3.4 resolve_active_engine 兜底级联

```
resolve_active_engine(asr_engine):
  1. asr_engine 非空 + resolve_engine_category 命中 + pick_entry 命中 → 用命中项
  2. 否则 → fallback_engine(cfg):
     a. DB zipformer section 有 "zipformer-small-ctc" → 用 DB 条目（用户手编 source 生效）
     b. 否则硬构造 ModelEntry { source: DEFAULT_ASR_MODEL_DIR, language: "zh", secret_key: "" }
```

仅服务「全局默认」。显式 name 路径（cli `--model`、AsrEngineManager）直接 `resolve_engine_category + pick_entry`，不经此函数。

### 3.5 DB migration v1→v2

`init_schema` 按 `user_version` 分派：
- `0` → 建表（新 schema 无 is_active）+ seed → v2
- `1` → `ALTER TABLE models DROP COLUMN is_active`（transaction 包裹）→ v2
- `2+` → no-op

bundled SQLite 3.45+ 支持 `DROP COLUMN`（3.35+ 起）。现有用户 DB 启动即自动迁移、不丢数据。

### 3.6 desktop is_streaming_engine / llm_config 改自由函数

这两个函数依赖 `octopus_asr`/`octopus_llm`，不能放进 infra（infra 无项目内依赖）。改为接 `&AppConfig` 的自由函数留在 `desktop::config`，desktop 内部用 `pub use octopus_infra::config::AppConfig` re-export 保持调用简洁。

## 4. 影响范围

| crate | 改动 |
|---|---|
| infra | 新增 `config` 模块（`AppConfig` + `load_config()`）；Cargo.toml 加 serde/serde_yaml/anyhow |
| asr | `db.rs` 删 is_active + v1→v2 migration；`config.rs` 删 active/AppYamlConfig/load_app_config、`AppConfig`→`AsrConfig`、新增 resolve_active_engine/pick_entry/fallback_engine；5 引擎模块 transcribe 加 name；engine.rs 用 pick_entry 简化 |
| desktop | `config.rs` 删 DesktopConfig、保留 is_streaming_engine/llm_config 为自由函数；coordinator/main/tray/overlay/paste 改用 AppConfig |
| cli | do_transcribe 传 name；show_config 用 resolve_active_engine；`load_app_config` → `infra::config::load_config`；clap 默认值改合法 DB name |
| server | `config.asr.active` → `resolve_active_engine`；加 octopus-infra 依赖 |

## 5. 验证

- `cargo check --workspace --all-targets`：0 error
- `cargo test -p octopus-asr`：14 passed（含 5 个新增 config 单测：pick_entry / fallback_engine）
- e2e：`octopus-cli config` 显示 `ASR active: qwen3-asr-0.6B (category: Qwen3Asr)` 精确命中
- DB migration：`PRAGMA user_version` = 2，`models` 表无 is_active 列

详见实施计划 [2026-06-14-config-infra-and-engine-truth.md](../plans/2026-06-14-config-infra-and-engine-truth.md)。
