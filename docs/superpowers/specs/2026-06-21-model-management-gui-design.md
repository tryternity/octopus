# 模型管理 GUI 接入设计（desktop 设置窗口页面 3）

> 2026-06-21。把阶段1 的 download 能力接到 desktop 设置窗口的「模型管理」页（页面 3，此前是占位）。
> 相关：download crate spec `2026-06-21-model-download-design.md`、阶段1 接入 spec `2026-06-21-download-model-integration-design.md`。
> worktree：`model-mgmt-ui`（分支 `worktree-model-mgmt-ui`，从 main `cdf5a70` 开出，已含阶段1）。

## 1. 背景与定位

阶段1（merge `f6f02bb`）已交付下载能力的「后端三层」：cli `download` 子命令、`resolve_model_dir` 第3级 `~/.octopus/models/<source>`、`AppConfig.download_mirror`。但 GUI 仍是占位——用户无法在桌面点按钮下模型。

本阶段做 GUI 接入：设置窗口「模型管理」页列出可下载的本地 ASR 模型，点按钮即下载到 `~/.octopus/models/<repo>/`，实时进度推前端。

**与 `setting-ui2` 分支的关系**：setting-ui2 在做设置窗口其他 UI（非模型管理页），也会改 `dist/settings/index.html`。本工作的前端 JS 隔离到独立 `models.js`，使对 `index.html` 的改动压缩到两处局部编辑，合并冲突最小化。

## 2. 现状（已探明）

### 2.1 设置窗口结构
- Tauri webview，加载 `dist/settings/index.html`（**手写单文件**，738 行，无前端构建步骤）。
- 侧边栏 3 页：`history`（识别记录）/ `settings`（系统设置，JS 动态渲染）/ `models`（模型管理，**占位** "📦 功能开发中"）。
- `switchPage(name)` 切 `.active`；前端用 `window.__TAURI__.core.invoke` 调命令、`window.__TAURI__.event.listen` 听事件。

### 2.2 后端命令（`settings_commands.rs`）
`get_config` / `set_config`（含 `download_mirror` 字段，阶段1 已加）/ `get_history` / `test_llm_connection` / `test_asr_connection` 等。**无下载相关命令**。

### 2.3 引擎 DB = 天然下载目录（关键利好）
`infra/src/db.sql` 的 models 表 seed：每个本地 ASR 引擎的 `source` 即 HF repo：
- `zipformer-multi` → `k2-fsa/sherpa-onnx-streaming-zipformer-ctc-multi-zh-hans-int8-2023-12-13`
- `paraformer-streaming` → `csukuangfj/sherpa-onnx-streaming-paraformer-zh`
- `whisper-small` → `onnx-community/whisper-small.en`
- `moonshine-base-en` → `csukuangfj/sherpa-onnx-moonshine-base-en-int8`
- ……（兜底 `zipformer-small-ctc` → `models/zipformer`，随包打包，**不可下载**）

`source` ↔ cli `download <repo>` ↔ `resolve_model_dir` 第3级，三者完全对齐。`EngineInfo`（`list_engines()` 返回）不含 `source`，但 `resolve_engine_in_config(&cfg, 裸名)` 接 `NameOnly` 分支遍历所有 section 返回 `entry`，`entry.source` 即 repo。

### 2.4 download crate 公开 API（复用，不改）
```rust
HfRequest { repo, include, exclude, source_url: Option<String>, target_dir: PathBuf }
resolve_tasks(&reqwest::Client, HfRequest) -> Result<Vec<DownloadTask>>
Downloader::new(DownloadConfig) -> Result<Downloader>          // .client() 借内部 reqwest::Client
Downloader::download(&DownloadTask, mpsc::Sender<Progress>, None) -> Result
Progress { downloaded_bytes: u64, total_bytes: Option<u64>, speed_bps: Option<f64> }
```

## 3. 设计

### 3.1 后端新模块 `crates/desktop/src/model_commands.rs`
独立模块（不动 `settings_commands.rs`，降低与 setting-ui2 冲突面）。两个命令：

**`list_downloadable_models()` → `Vec<DownloadableModel>`**
- `load_config()` + `list_engines()`
- 遍历引擎：仅 `is_local`，且 `entry.source` 是 HF repo 形态（`is_hf_repo`：含 `/`、非 `models/` 前缀、非 `http`/`wss`、非绝对路径）。
- `downloaded = resolve_model_dir(&source).is_ok()`（任一级命中即「已就绪」）。
- 返回 `{ name, repo, category, description, downloaded }`。

**`download_model(repo, rc, app_handle) async`**
- 镜像：`rc.read().download_mirror`（空 = 官方源）。
- `target_dir = octopus_config_home().join("models")`。
- 构 `HfRequest` → `resolve_tasks`（用 `Downloader::client()`）。
- mpsc 进度转发 task：`rx.recv()` → `app_handle.emit("download-progress", {repo, downloaded, total, speed})`。
- 多文件逐一下载，每文件 emit `download-file {repo, index, total, file}`；`download` 复用同一 `tx.clone()`。
- 全部完成 drop(tx) → 转发 task 自然退出 → 返回 Ok；失败透传 `DownloadError`。

### 3.2 前端独立 `dist/settings/models.js`
- `renderModels()`：`invoke('list_downloadable_models')` → 渲染卡片列表（名称 / category / description 含尺寸 / 已下载✓ 或 下载按钮）。
- 下载按钮：`invoke('download_model', {repo})`；同时 `listen('download-file')` 显示「文件 i/total」+ `listen('download-progress')` 更新当前文件进度条（字节数 / 百分比 / MB/s）。
- 镜像输入框：读 `get_config().config.download_mirror` 回填；`change` → `invoke('set_config', {key:'download_mirror', value})`。
- 在 `window` 上挂 `initModelsPage()`，由页面切换或加载时调用。

### 3.3 `index.html` 两处局部改动（最小化冲突）
1. `#page-models` 占位 → 空容器 `<div id="models-list"></div>`（由 `models.js` 动态填充，对齐 page-settings 模式）。
2. `</body>` 前加 `<script src="models.js"></script>`。
3. `switchPage` 切到 `models` 时调用 `initModelsPage()`（一处 1 行分支）。

### 3.4 接线
- `Cargo.toml`：加 `octopus-download = { path = "../download" }`（**非 optional**，模型管理始终可用）。
- `main.rs`：`mod model_commands;` + invoke_handler 注册 `list_downloadable_models` / `download_model`。

## 4. 接口契约

| 接口 | 变化 |
|---|---|
| download crate | **不改**（复用 `HfRequest`/`resolve_tasks`/`Downloader`/`Progress`） |
| `resolve_model_dir` | **不改**（阶段1 已加 models 级，此处只读判定） |
| desktop `invoke_handler` | 新增 `list_downloadable_models` / `download_model` |
| 前端 | 新增 `models.js`；`index.html` 两处局部编辑 |
| Tauri 事件 | 新增 `download-progress`（字节级）/ `download-file`（文件级） |

## 5. 数据流

**列模型**：`models.js renderModels` → `list_downloadable_models` → `list_engines` + resolve 取 source + `resolve_model_dir` 判定 → 卡片列表。

**下载**：点按钮 → `download_model(repo)` → `resolve_tasks` → 逐文件 `Downloader::download` → mpsc → 转发 task emit `download-progress`/`download-file` → 前端进度条 → 完成 → 重新 `renderModels`（downloaded=✓）。

## 6. 错误处理

| 场景 | 行为 |
|---|---|
| `list_engines`/`load_config` 失败 | 命令返回 Err 字符串，前端 toast |
| `resolve_tasks` 失败（仓库不存在/网络） | `download_model` 返回 Err，前端 toast |
| 单文件下载失败 | 透传 `DownloadError`，镜像 fallback 由 download crate 处理 |
| 并发下载同一模型 | v1 不防（invoke 串行；用户连点由前端禁用按钮兜底） |

## 7. 范围边界（本阶段不做）

- **不做删除模型 / 在文件夹中显示**（YAGNI）。
- **不做下载取消**（download crate 暂无取消 API；大模型下载用户等待）。
- **不做并发多模型下载**（一次一个）。
- **不做 DB models 表 UI 管理**（引擎目录来自 seed + 手编 DB，本页只读列表）。

## 8. 测试策略

- **`is_hf_repo` 单测**（model_commands.rs `#[cfg(test)]`）：随包 `models/zipformer`→false、绝对路径→false、`wss://`→false、`k2-fsa/x`→true、空串→false。
- **`list_downloadable_models` 逻辑**：mock 引擎列表 + source，断言兜底被排除、downloaded 判定正确（纯逻辑，可抽函数测）。
- 前端 / Tauri 集成无自动化（webview + 网络），靠 `cargo check --workspace --all-targets` + clippy + 手动。
