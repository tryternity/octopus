# 模型管理 GUI 接入 实施计划

> spec：`docs/superpowers/specs/2026-06-21-model-management-gui-design.md`。worktree `model-mgmt-ui`。
>
> **For agentic workers:** REQUIRED SUB-SKILL: 用 superpowers:executing-plans 按 task 实施。Steps 用 checkbox（`- [ ]`）跟踪。
>
> **v1（Task 1–5）已合并 main `7fd0682`（2026-06-21）**，下方标 `[x]`。
> **v2（Task 6–12，2026-06-22）**：就绪逻辑重构——`is_enabled` 改就绪语义、直读 DB 列全部、点下载先探查、`secret_key` 自举 sha256 清单、`verify_model` 复核、`RUNTIME_CONFIG` 可刷新。

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

## Task 6：infra/db.rs 新增 3 函数 + 单测（v2）

**Files：** `crates/infra/src/db.rs`

- [ ] `list_all_local_asr_models_at(conn) -> Result<Vec<LocalAsrModelRow>>`：SQL `SELECT category, model_name, source, secret_key, description, is_enabled, is_streaming FROM models WHERE domain='asr' AND is_local=1`，平铺返回（含 `is_enabled`，**不过滤**）。
- [ ] 公开包装 `list_all_local_asr_models()`：`ensure_db()?` + `list_all_local_asr_models_at(&conn)`。
- [ ] `set_model_enabled_at(conn, model_name, enabled: bool)`：`UPDATE models SET is_enabled=? WHERE model_name=? AND domain='asr' AND is_local=1`。
- [ ] 公开包装 `set_model_enabled(model_name, enabled)`。
- [ ] `set_model_secret_key_at(conn, model_name, json: &str)`：`UPDATE models SET secret_key=? WHERE model_name=? AND domain='asr' AND is_local=1`。
- [ ] 公开包装 `set_model_secret_key(model_name, json)`。
- [ ] `LocalAsrModelRow` 结构（`#[derive(Debug)]`，字段对齐 SELECT）。
- [ ] 单测（`#[cfg(test)]`）：seed 后 `list_all_local_asr_models` 含 `is_enabled=0` 的（如 paraformer-streaming）；`set_model_enabled_at("paraformer-streaming", true)` 后重读=1；`set_model_secret_key_at` 写入 JSON 后重读一致。

## Task 7：asr/config.rs RUNTIME_CONFIG 可刷新化（v2）

**Files：** `crates/asr/src/config.rs`

- [ ] `static RUNTIME_CONFIG: OnceLock<AsrConfig>` → `static RUNTIME_CONFIG: RwLock<Option<Arc<AsrConfig>>>`（`use std::sync::{Arc, RwLock};`）。
- [ ] `load_config()`：读 `RUNTIME_CONFIG.read()`；`None` 则 `ensure_db` + `load_models` + 写 `Some(Arc::new(cfg))`；返回 `(**cfg).clone()`。
- [ ] 新增 `pub fn reload_models_config()`：`load_models()` 成功则替换 `RUNTIME_CONFIG.write()` 为 `Some(Arc::new(c))`，失败 log::warn 保留旧值（对齐 `reload_app_config`）。
- [ ] reload **不单测**：asr 测试惯例为纯函数内核（手工构造 AsrConfig，不碰全局 RUNTIME_CONFIG / 真实 ~/.octopus DB）；reload 是 3 行胶水（load_models + RwLock write），靠 model_commands 集成 + 手动 GUI 覆盖。
- [ ] `cargo check -p octopus-asr` 全调用点（13+）通过。

## Task 8：model_commands list 改造 + DTO（v2）

**Files：** `crates/desktop/src/model_commands.rs`

- [ ] `DownloadableModel`：`downloaded: bool` → `is_enabled: bool`。
- [ ] `list_downloadable_models()`：改用 `octopus_infra::db::list_all_local_asr_models()`，`is_hf_repo(&row.source)` 过滤，映射 `{ name: model_name, repo: source, category, description, is_enabled }`（不再 `list_engines`/`resolve_engine_in_config`/`resolve_model_dir`）。
- [ ] `category_label` 映射沿用（row.category 已是 DB category 字符串）。

## Task 9：model_commands download 改造 + verify_model（v2）

**Files：** `crates/desktop/src/model_commands.rs`

- [ ] 抽 `bootstrap_manifest(dir: &Path) -> Result<String>`：遍历目录常规文件（递归，跳过隐藏），follow symlink，算 sha256 + 相对路径 + size，序列化为 `{"files":[...]}` JSON 字符串。
- [ ] 抽 `verify_against_manifest(dir, json) -> Result<VerifyResult>`：解析 JSON（失败→`NeedBootstrap`）；逐文件算 sha256 比对，返回 `{ok, broken_files: Vec<String>}`。
- [ ] `download_model` 改造：先 `resolve_model_dir(&repo)`：
  - 命中 → `bootstrap_manifest` → `set_model_secret_key` + `set_model_enabled(true)` + `reload_models_config()` + emit `download-done{repo, already_ready:true}`，**不下载**。
  - 未命中 → 现有 resolve_tasks + 逐文件下载（emit progress/file）→ 完成后 `bootstrap_manifest` + `set_model_secret_key` + `set_model_enabled(true)` + `reload_models_config()` + emit `download-done{repo, already_ready:false}`。
- [ ] 新增 `verify_model(model_name, repo)`：`resolve_model_dir` → 读 DB `secret_key`（经 `list_all_local_asr_models` 查该行）→ 空/解析失败→自举+置 true；非空→`verify_against_manifest`，全 ok→确保 true；有损坏→`set_model_enabled(false)` + reload，返回 `{ok, broken_files}`。
- [ ] 写 DB 后均 `octopus_asr::config::reload_models_config()`。
- [ ] 单测：`bootstrap_manifest`（临时目录造文件，断言 JSON 含正确 sha256）；`verify_against_manifest`（篡改文件后 broken_files 非空）。

## Task 10：main.rs 接线 verify_model（v2）

**Files：** `crates/desktop/src/main.rs`

- [ ] invoke_handler 增加 `model_commands::verify_model`。

## Task 11：models.js 前端（v2）

**Files：** `crates/desktop/dist/settings/models.js`

- [ ] 卡片：`is_enabled` → 「✓ 已就绪」+「重新校验」按钮；否则「下载」按钮。
- [ ] 下载按钮：`invoke('download_model', {repo})` + listen `download-file`/`download-progress`/`download-done`（done 时按 `already_ready` toast「已就绪」/「下载完成」，刷新列表）。
- [ ] 重新校验按钮：`invoke('verify_model', {model_name, repo})` → toast `ok`/损坏清单 + 刷新。
- [ ] 下载中禁用按钮（防连点）。

## Task 12：验证 + 文档同步（v2）

- [ ] `cargo check --workspace --all-targets` 通过、零新 warning。
- [ ] `cargo clippy -p octopus-desktop -p octopus-asr -p octopus-infra` 零新 warning。
- [ ] `cargo test -p octopus-infra list_all / set_model`、`-p octopus-asr reload`、`-p octopus-desktop model_commands`。
- [ ] architecture.md 更新（is_enabled 就绪语义 / verify_model / secret_key 校验 / RUNTIME_CONFIG 可刷新）。
- [ ] memory `parallel-workstreams` 更新本轮迭代。
