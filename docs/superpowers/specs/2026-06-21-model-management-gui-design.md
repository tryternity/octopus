# 模型管理 GUI 接入设计（desktop 设置窗口页面 3）

> 2026-06-21 初版（已合并 main `7fd0682`）。2026-06-22 **就绪逻辑重构 v2**（见 §9）：`is_enabled` 改为「就绪」语义、点下载先探查、`secret_key` 存文件清单 + sha256 自举、新增完整性复核。
> 相关：download crate spec `2026-06-21-model-download-design.md`、阶段1 接入 spec `2026-06-21-download-model-integration-design.md`。
> worktree：`model-mgmt-ui`（分支 `worktree-model-mgmt-ui`）。

## 1. 背景与定位

阶段1（merge `f6f02bb`）已交付下载能力的「后端三层」：cli `download` 子命令、`resolve_model_dir` 第3级 `~/.octopus/models/<source>`、`AppConfig.download_mirror`。v1（merge `7fd0682`）把下载接到了 GUI：设置窗口「模型管理」页列出可下载的本地 ASR 模型，点按钮即下载到 `~/.octopus/models/<repo>/`，实时进度推前端。

**v2 重构动机**：v1 的「已就绪」= `resolve_model_dir().is_ok()`（文件在任意路径即算），且用 `list_engines()` 取列表——但 `list_engines` 走 `load_config`，后者在 DB 加载层硬过滤 `is_enabled=1`，导致 seed 里 `is_enabled=0` 的可下载模型**根本列不出来**。v2 改为：直读 DB 列全部本地模型，`is_enabled` 作为「就绪」标志，点下载先探查文件（命中即就绪、置 true，不重下），并对已就绪模型做 sha256 完整性复核（损坏置 false）。

**与 `setting-ui2` 分支的关系**：setting-ui2 在做设置窗口其他 UI（非模型管理页），也会改 `dist/settings/index.html`。本工作的前端 JS 隔离到独立 `models.js`，使对 `index.html` 的改动压缩到两处局部编辑，合并冲突最小化。

## 2. 现状（已探明）

### 2.1 设置窗口结构
- Tauri webview，加载 `dist/settings/index.html`（**手写单文件**，无前端构建步骤）。
- 侧边栏 4 页：`history`（识别记录）/ `settings`（系统设置）/ `models`（模型管理）/ `prompts`（提示词，setting-ui2 的）。
- `switchPage(name)` 切 `.active`；前端用 `window.__TAURI__.core.invoke` 调命令、`window.__TAURI__.event.listen` 听事件。
- 模型管理页 `#page-models` 顶部第一张卡片即「下载镜像」（镜像输入），第二张「ASR 模型」（`#models-list` 由 `models.js` 填充）。

### 2.2 后端命令
`get_config` / `set_config`（含 `download_mirror` 字段）/ `get_history` / `test_llm_connection` / `test_asr_connection` 等。模型管理命令在独立模块 `model_commands.rs`（v1）：`list_downloadable_models` / `download_model` / `set_download_mirror`。v2 新增 `verify_model`。

### 2.3 引擎 DB = 天然下载目录（关键利好）
`infra/src/db.sql` 的 models 表 seed：每个本地 ASR 引擎的 `source` 即 HF repo（如 `csukuangfj/sherpa-onnx-streaming-paraformer-zh`）；兜底 `zipformer-small-ctc` → `models/zipformer`（随包打包，不可下载）。`source` ↔ cli `download <repo>` ↔ `resolve_model_dir`，三者对齐。

### 2.4 `is_enabled` 过滤发生在 DB 加载层（v2 关键发现）
`load_models_at`（`infra/src/db.rs:396`）SQL 硬编码 `WHERE domain='asr' AND is_enabled = 1`——**`is_enabled=0` 的模型不进 `AsrConfig`、不进 `list_engines()`**。seed 里 local 可下载模型 `is_enabled` 全 0，所以 v1 用 `list_engines()` 列表时只能看到用户手编置 1 的。v2 因此改为**直读 DB（不过滤 is_enabled）**列全部。`ModelEntry` 已含 `is_enabled` 字段。

### 2.5 `RUNTIME_CONFIG` 不可刷新（v2 改造点）
`asr/config.rs:16` `static RUNTIME_CONFIG: OnceLock<AsrConfig>`，注释「手编 DB models 表后需重启进程生效」——`OnceLock` 不能 reset。v2 改为可刷新的 `RwLock<Option<Arc<AsrConfig>>>` + `reload_models_config()`（对齐既有 `reload_app_config` / `APP_CONFIG` 模式，审查二1 已验证），让「改 `is_enabled` 后引擎下拉立即更新」。

### 2.6 download crate 公开 API（复用，不改）
```rust
HfRequest { repo, include, exclude, source_url: Option<String>, target_dir: PathBuf }
resolve_tasks(&reqwest::Client, HfRequest) -> Result<Vec<DownloadTask>>
Downloader::new(DownloadConfig) -> Result<Downloader>          // .client() 借内部 reqwest::Client
Downloader::download(&DownloadTask, mpsc::Sender<Progress>, None) -> Result
Progress { downloaded_bytes: u64, total_bytes: Option<u64>, speed_bps: Option<f64> }
```
download crate 下载时已做服务端 sha256 校验（阶段1），下载成功的文件即可信——v2 自举算 sha256 以此为信任基础。

## 3. 设计

### 3.1 后端模块 `crates/desktop/src/model_commands.rs`
独立模块（不动 `settings_commands.rs`，降低与 setting-ui2 冲突面）。命令：`list_downloadable_models` / `download_model` / `verify_model` / `set_download_mirror`。

**`list_downloadable_models()` → `Vec<DownloadableModel>`**
- 直读 `list_all_local_asr_models()`（infra 新函数，`domain='asr' AND is_local=1`，**不过滤 is_enabled**），不再走 `list_engines()`/`load_config`。
- `is_hf_repo(&source)` 过滤（排除随包 `models/`、绝对路径、云端 `http`/`wss`）。
- 返回 `{ name, repo, category, description, is_enabled }`（v2：`downloaded` 字段 → `is_enabled`）。

**`download_model(repo, rc, app_handle) async`**（v2：先探查）
1. `resolve_model_dir(&repo)` 探查三个路径（`~/.octopus/<source>`、`~/.octopus/models/<source>`、HF cache）。
2. **命中**（文件已就绪，如用户 hf-cli 下过的在 cache）→ 自举：遍历目录常规文件算 sha256 写 `secret_key`（`set_model_secret_key`）+ 置 `is_enabled=true`（`set_model_enabled`）+ `reload_models_config()`；emit `download-done {repo, already_ready:true}`；**不重下**。
3. **未命中** → 镜像 `rc.download_mirror`（空=官方源）+ `target_dir=~/.octopus/models` + `HfRequest` + `resolve_tasks`（用 `Downloader::client()`）；mpsc 进度转发 task emit `download-progress`/`download-file`；逐文件 `Downloader::download`；全部完成 → 自举算 sha256 写 `secret_key` + 置 `is_enabled=true` + `reload_models_config()`；emit `download-done {repo, already_ready:false}`。
4. 失败透传 `DownloadError`。

**`verify_model(model_name, repo)`**（v2 新增，完整性复核）
1. 读 `secret_key` JSON 清单。
2. **清单空** → 自举生成（遍历 `resolve_model_dir` 目录文件算 sha256 写回）+ 确保 `is_enabled=true` + reload；返回「已生成清单，就绪」。
3. **清单非空** → 逐文件算当前 sha256 比对（缺文件/hash 不符即损坏）。
   - 全匹配 → 确保 `is_enabled=true`；返回「校验通过」。
   - 任一不符 → 置 `is_enabled=false` + `reload_models_config()`；返回损坏文件清单。

**`set_download_mirror(value, rc)`**：v1 不变（独立命令，写 rc + `save_app_config`）。

### 3.2 前端独立 `dist/settings/models.js`
- `renderModels()`：`invoke('list_downloadable_models')` → 卡片列表。`is_enabled=true` → 「✓ 已就绪」+「重新校验」按钮；`false` → 「下载」按钮。
- 下载按钮：`invoke('download_model', {repo})`；`listen('download-file')` 显示「文件 i/total」+ `listen('download-progress')` 更新当前文件进度条；`listen('download-done')` → toast（已就绪/下载完成）+ 重新 `renderModels`。
- 「重新校验」按钮：`invoke('verify_model', {model_name, repo})` → toast 结果 + `renderModels`。
- 镜像输入框（顶部卡片）：读 `get_config().config.download_mirror` 回填；`change` → `invoke('set_download_mirror', {value})`。
- `window.initModelsPage` 挂全局，由导航点击或页面加载调用。

### 3.3 `index.html` 两处局部改动（v1 已落地，v2 不动结构）
1. `#page-models` 占位 → 顶部「下载镜像」卡片 + 「ASR 模型」卡片（`<div id="models-list">`）。
2. `</body>` 前加 `<script src="models.js"></script>`。

### 3.4 接线
- `Cargo.toml`：`octopus-download = { path = "../download" }`（v1 已加）。
- `main.rs`：`mod model_commands;` + invoke_handler 注册 `list_downloadable_models` / `download_model` / `verify_model` / `set_download_mirror`。

## 4. 接口契约

| 接口 | 变化 |
|---|---|
| download crate | **不改** |
| `resolve_model_dir` | **不改**（只读探查） |
| `infra::db` | **新增** `list_all_local_asr_models` / `set_model_enabled` / `set_model_secret_key`（+ `_at` 变体） |
| `asr::config` | **新增** `reload_models_config`；`RUNTIME_CONFIG` OnceLock→RwLock |
| desktop `invoke_handler` | 新增 `verify_model`（list/download/mirror v1 已注册） |
| Tauri 事件 | 新增 `download-done`（v2）；保留 `download-progress`/`download-file` |
| `DownloadableModel` DTO | `downloaded: bool` → `is_enabled: bool` |

## 5. 数据流

**列模型**：`models.js renderModels` → `list_downloadable_models` → `list_all_local_asr_models`（直读 DB，不过滤）+ `is_hf_repo` 过滤 → 卡片（按 `is_enabled` 显示就绪/下载）。

**下载**：点按钮 → `download_model(repo)` → 探查 `resolve_model_dir`：
- 命中 → 自举 sha256 写 `secret_key` + 置 `is_enabled=true` + reload → `download-done{already_ready:true}` → 刷新（就绪）。
- 未命中 → `resolve_tasks` → 逐文件 `Downloader::download` → mpsc → emit `download-progress`/`download-file` → 完成 → 自举 + 置 true + reload → `download-done{already_ready:false}` → 刷新（就绪）。

**重新校验**：点按钮 → `verify_model` → 读 `secret_key` 清单 → 复核 sha256 → 通过(确保 true)/损坏(置 false + reload) → toast + 刷新。

## 6. 错误处理

| 场景 | 行为 |
|---|---|
| `list_all_local_asr_models`/读 DB 失败 | 命令返回 Err，前端 toast |
| 探查命中但自举算 sha256 失败（目录权限/IO） | `download_model` 返回 Err，toast；不置 is_enabled |
| `resolve_tasks` 失败（仓库不存在/网络） | `download_model` 返回 Err，toast |
| 单文件下载失败 | 透传 `DownloadError`，镜像 fallback 由 download crate 处理 |
| `verify_model` 发现损坏 | 置 `is_enabled=false` + reload，返回损坏清单，toast |
| `secret_key` JSON 损坏（无法解析） | 视为「清单空」→ 自举重新生成 |

## 7. 范围边界（不做）

- **不增删改模型条目**：本地 ASR 模型清单是应用限定的（开发适配过的），列表只读，只能下载/校验/切 `is_enabled`。
- **不删除模型文件 / 在文件夹中显示**（YAGNI）。
- **不下载取消**（download crate 暂无取消 API）。
- **不并发多模型下载**（一次一个）。
- **云端模型不在此页**：火山/腾讯/百度/阿里等云端 ASR 走「系统设置」填 key + 连接测试，另套管理，与本页无关。

## 8. 测试策略

- **`is_hf_repo` 单测**（v1 已有）：随包/绝对路径/云端/空/真实 repo。
- **`list_all_local_asr_models` 单测**（infra）：含 `is_enabled=0` 的也被列出；`secret_key`/`is_enabled` 字段正确。
- **`set_model_enabled_at` / `set_model_secret_key_at` 单测**（infra）：写入后重读生效。
- **`RUNTIME_CONFIG` reload 单测**（asr）：改 DB 后 `reload_models_config()` → `load_config()` 返回新值。
- **`verify_model` 清单比对逻辑**：纯逻辑抽函数测（清单 parse + sha256 比对判定），文件系统部分靠手动。
- 前端 / Tauri 集成无自动化（webview + 网络），靠 `cargo check --workspace --all-targets` + clippy + 手动。

## 9. 就绪逻辑重构 v2 详述（2026-06-22）

### 9.1 `is_enabled` 语义
`is_enabled` 统一表达「该模型文件是否就绪可用」：`true`=文件完备可被引擎加载，`false`=未就绪。引擎下拉（`list_engines`→`load_config`）只收 `is_enabled=1` 是 load 层既有行为，自然联动——**未就绪的模型不会出现在「系统设置」的引擎下拉**，避免选了下载不全的模型。seed 里 `is_enabled=0` 语义即「未下载」（用户：seed 不同步不管，以当前 DB 为准；用户初始只有兜底打包模型，其余靠本页下载）。

### 9.2 `RUNTIME_CONFIG` 可刷新化
- `static RUNTIME_CONFIG: OnceLock<AsrConfig>` → `static RUNTIME_CONFIG: RwLock<Option<Arc<AsrConfig>>>`（对齐 `APP_CONFIG`）。
- `load_config()`：读 `RwLock`；首次空则 `ensure_db` + `load_models` + 写入。13+ 调用点行为不变（读法从 `OnceLock::get` 换 `RwLock::read`，clone 成本不变）。
- `reload_models_config()`：从 DB 重读 `AsrConfig` 替换缓存（对齐 `reload_app_config`）。`download_model`/`verify_model` 改 `is_enabled` 后调用，让引擎下拉即时更新。

### 9.3 `secret_key` 复用与 JSON schema
local 模型 `secret_key`（DB 默认 `''`，原仅 api 模型用）重载为「文件清单 + sha256」JSON；api 模型（`is_local=0`）`secret_key` 仍是真 API key，**按 `is_local` 分支，不冲突**。schema（**path 为 key 的 map**，紧凑可读；`BTreeMap` 保证字母序、diff 友好）：
```json
{"model.onnx":{"sha256":"<hex>","size":12345}, "tokens.txt":{"sha256":"<hex>","size":75756}}
```
key 为相对模型目录根（`resolve_model_dir` 返回目录）的路径。读取时 JSON 解析失败→视为清单空→自举重建。manifest 逻辑（`bootstrap_manifest`/`verify_against_manifest`/`Manifest`）下沉到 `asr::manifest`，desktop（`download_model`/`verify_model`）与 cli（`sync-models`）共用。

**批量预填**：cli `octopus-cli sync-models` 扫描所有本地 ASR 模型，就绪的（`resolve_model_dir` 命中）自举清单写 `secret_key` + 置 `is_enabled=true`，未就绪置 `false`，末尾 `reload_models_config`——供首次填充或批量复核（GUI 的 `verify_model` 是单模型按需触发）。

### 9.4 自举（manifest 生成）
触发时机：① 下载完成；② 探查命中（已有文件，如 hf-cache）；③ `verify_model` 发现清单空。
做法：遍历 `resolve_model_dir` 返回目录下的**常规文件**（递归），算 sha256 + 相对路径 + 字节数，写 `secret_key`。HF cache snapshot 目录下是 symlink 到 blobs——按**实际文件内容**算 hash（follow link 读字节）。跳过隐藏/系统文件（`.DS_Store` 等）。

### 9.5 校验算法 = sha256
与 download crate（阶段1 服务端 sha256 校验）同一套，自举/复核一致，不引入 md5 第二套。用户原话「md5 对不上」理解为「校验码对不上」泛指。

### 9.6 校验时机（性能）
- **列表展示**：只按 `is_enabled` 显示，**不算 hash**（快，进页面即时）。
- **sha256 校验**：仅在「点下载探查时」（自举/复核）和「重新校验按钮」触发。大模型（1G+）算 sha256 几百 ms~秒级，不在列表每次跑。
- 「重新校验」按钮供用户怀疑文件损坏时手动触发全量复核。
