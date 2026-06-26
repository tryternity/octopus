# 模型管理 GUI 接入 实施计划

> spec：`docs/superpowers/specs/2026-06-21-model-management-gui-design.md`。worktree `model-mgmt-ui`。
>
> **For agentic workers:** REQUIRED SUB-SKILL: 用 superpowers:executing-plans 按 task 实施。Steps 用 checkbox（`- [ ]`）跟踪。
>
> **v1（Task 1–5）已合并 main `7fd0682`（2026-06-21）**，下方标 `[x]`。
> **v2（Task 6–12，2026-06-22，已合并 main `08e1bef`+`bb33237`）**：就绪逻辑重构——`is_enabled` 改就绪语义、直读 DB 列全部、点下载先探查、`secret_key` 自举 sha256 清单（manifest 下沉 `asr::manifest`，map 格式）、`verify_model` 复核、`RUNTIME_CONFIG` 可刷新；cli `sync-models` 批量填 secret_key。
> **Task 13（2026-06-22）**：本地 ASR seed 按 DB `is_local=1` 的 12 行重生成，全 `is_enabled=0`，兜底引擎移出 seed（代码写死）。

**Goal**：模型管理页列出所有本地 ASR 模型（含未下载），按 `is_enabled` 显示就绪/下载；点下载先探查（命中即就绪不重下），下载/校验后写 sha256 清单到 `secret_key`，损坏可复核置 false；改 `is_enabled` 后引擎下拉即时更新。

**Architecture**：infra 加 3 个直读/写 DB 函数（不过滤 is_enabled）；asr `RUNTIME_CONFIG` 改 `RwLock` + `reload_models_config`（对齐 APP_CONFIG）；model_commands 改 list/download + 新增 verify_model；models.js 卡片按 is_enabled + 下载/重新校验。

---

## Task 1：后端 model_commands.rs（v1）✅
- [x] `DownloadableModel` DTO + `is_hf_repo`
- [x] `list_downloadable_models` / `download_model` / `set_download_mirror`
- [x] `is_hf_repo` 单测

## Task 2：后端接线（v1）✅
- [x] Cargo.toml `octopus-download`、main.rs `mod model_commands` + 注册

## Task 3：前端 models.js（v1）✅
- [x] renderModels / 下载进度 / 镜像输入 / initModelsPage

## Task 4：index.html 两处改动（v1）✅
- [x] `#page-models` 容器 + `<script src="models.js">`

## Task 5：验证 + 收尾（v1）✅
- [x] cargo check/clippy/test + architecture.md + memory

---

## Task 6：infra/db.rs 新增 3 函数 + 单测（v2）✅

**Files：** `crates/infra/src/db.rs`

- [x] `list_all_local_asr_models_at(conn) -> Result<Vec<LocalAsrModelRow>>`：SQL `SELECT category, model_name, source, secret_key, description, is_enabled, is_streaming FROM models WHERE domain='asr' AND is_local=1`，平铺返回（含 `is_enabled`，**不过滤**）。
- [x] 公开包装 `list_all_local_asr_models()`：`with_db(list_all_local_asr_models_at)`（免冗余闭包）。
- [x] `set_model_enabled_at(conn, model_name, enabled: bool)`：`UPDATE models SET is_enabled=? WHERE model_name=? AND domain='asr' AND is_local=1`。
- [x] 公开包装 `set_model_enabled(model_name, enabled)`。
- [x] `set_model_secret_key_at(conn, model_name, json: &str)`：`UPDATE models SET secret_key=? WHERE model_name=? AND domain='asr' AND is_local=1`。
- [x] 公开包装 `set_model_secret_key(model_name, json)`。
- [x] `LocalAsrModelRow` 结构（字段对齐 SELECT）。
- [x] 单测：`list_all_local_asr_models_includes_disabled` / `set_model_enabled_persists` / `set_model_secret_key_persists`（3 个，全绿）。

## Task 7：asr/config.rs RUNTIME_CONFIG 可刷新化（v2）✅

**Files：** `crates/asr/src/config.rs`

- [x] `static RUNTIME_CONFIG: OnceLock<AsrConfig>` → `static RUNTIME_CONFIG: RwLock<Option<Arc<AsrConfig>>>`（`use std::sync::{Arc, RwLock};`）。
- [x] `load_config()`：读 `RUNTIME_CONFIG.read()`；`None` 则 `ensure_db` + `load_models` + 写 `Some(Arc::new(cfg))`；返回 clone。
- [x] 新增 `pub fn reload_models_config()`：`load_models()` 成功则替换 `RUNTIME_CONFIG.write()` 为 `Some(Arc::new(c))`，失败 log::warn 保留旧值（对齐 `reload_app_config`）。
- [x] reload **不单测**：asr 测试惯例为纯函数内核（手工构造 AsrConfig，不碰全局/真实 DB）；reload 是 3 行胶水，靠 model_commands 集成 + 手动 GUI 覆盖。
- [x] `cargo check -p octopus-asr-local` 全调用点通过。

## Task 8：model_commands list 改造 + DTO（v2）✅

**Files：** `crates/desktop/src/model_commands.rs`

- [x] `DownloadableModel`：`downloaded: bool` → `is_enabled: bool`。
- [x] `list_downloadable_models()`：改用 `octopus_infra::db::list_all_local_asr_models()`，`is_hf_repo(&row.source)` 过滤，映射 `{ name, repo: source, category, description, is_enabled }`（不再 `list_engines`/`resolve_model_dir`）。

## Task 9：model_commands download 改造 + verify_model（v2）✅

**Files：** `crates/desktop/src/model_commands.rs`、`crates/asr/src/manifest.rs`（新）

- [x] **manifest 逻辑下沉 `asr::manifest`**（desktop 与 cli `sync-models` 共用）：`bootstrap_manifest(dir) -> Result<String>` 遍历目录常规文件（递归，跳过隐藏，follow symlink 适配 HF cache），序列化为 **map 格式** `Manifest = BTreeMap<String, {sha256,size}>`（`{"<path>":{"sha256","size"}}`，BTreeMap 字母序）。原计划 `{"files":[...]}` 数组 → 改 map（用户 2026-06-22 要求，紧凑可读）。
- [x] `verify_against_manifest(dir, &Manifest) -> Vec<String>`：逐文件算 sha256 比对，返回损坏/缺失路径。
- [x] `download_model` 改造：先 `resolve_model_dir(&repo)`：
  - 命中 → `bootstrap_manifest` + `set_model_secret_key` + `set_model_enabled(true)` + `reload_models_config()` + emit `download-done{repo, already_ready:true}`，**不下载**。
  - 未命中 → resolve_tasks + 逐文件下载（emit progress/file）→ 完成后 bootstrap + secret_key + enabled(true) + reload + emit `download-done{already_ready:false}`。
- [x] 新增 `verify_model(model_name, repo)`：resolve_model_dir → 读 DB secret_key → 空→自举+置 true；非空→`verify_against_manifest`，全 ok→确保 true；有损坏→`set_model_enabled(false)` + reload，返回 `{ok, broken_files}`。
- [x] 写 DB 后均 `reload_models_config()`。
- [x] 单测：移至 `asr::manifest`（`bootstrap_manifest_hashes_files` / `verify_detects_tamper`）；desktop 仅留 `is_hf_repo` 4 测试。

## Task 10：main.rs 接线 verify_model（v2）✅

**Files：** `crates/desktop/src/main.rs`

- [x] invoke_handler 增加 `model_commands::verify_model`。

## Task 11：models.js 前端（v2）✅

**Files：** `crates/desktop/dist/settings/models.js`

- [x] 卡片：`is_enabled` → 「✓ 已就绪」+「重新校验」按钮；否则「下载」按钮。
- [x] 下载按钮：`invoke('download_model', {repo})` + listen `download-file`/`download-progress`/`download-done`（done 时按 `already_ready` toast「已就绪」/「下载完成」，刷新列表）。
- [x] 重新校验按钮：`invoke('verify_model', {model_name, repo})` → toast ok/损坏清单 + 刷新。
- [x] 下载中禁用按钮（防连点）。

## Task 12：验证 + 文档同步（v2）✅

- [x] `cargo check --workspace --all-targets` 通过、零新 warning。
- [x] clippy 零新 warning。
- [x] `cargo test -p octopus-infra list_all/set_model`、`-p octopus-asr-local manifest`、`-p octopus-desktop model_commands`（reload 不单测，见 Task 7）。
- [x] architecture.md 更新（is_enabled 就绪语义 / verify_model / secret_key 校验 / RUNTIME_CONFIG 可刷新 / manifest-asr）。
- [x] spec §9 v2 详述 + memory `parallel-workstreams` 更新。

## Task 13：本地 ASR seed 重生成（2026-06-22）✅

**Files：** `crates/infra/src/db.sql`

- [x] 本地 ASR seed（is_local=1）以实时 DB 12 行为准重写：moonshine×2 / paraformer×4 / qwen3-asr×2 / sensevoice / whisper / zipformer×2，**全部 `is_enabled=0`**（待下载就绪）。
- [x] 默认/兜底引擎 `zipformer-small-ctc` 移出 seed（代码 `FALLBACK_ASR_ENGINE_NAME` 写死，`fallback_engine` 硬构造，不依赖 DB）。
- [x] `app_config.asr_engine` seed 改空（空=代码兜底引擎，开箱可用）。
- [x] 云端 ASR / LLM seed 保留（is_local=0，不在「以 is_local=true 为基础」范围）。
- [x] 临时空 DB 验证：12 行/全 false/无 zipformer-small-ctc/云端8+LLM6 保留/asr_engine 空/二跑幂等（26 行）。
- [x] spec §2.3/§9.1 + architecture models 表同步。
