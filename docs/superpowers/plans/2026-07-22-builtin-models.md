# 内置模型开箱即用实施计划

> **Spec:** `docs/superpowers/specs/2026-07-22-builtin-models.md`
> **状态**：✅ Step 1/2/3 代码全部完成（2026-07-22），e2e 待用户验证
> **分 3 步，每步 e2e 通过后再做下一步。**

---

## Step 1：VAD 内嵌（include_bytes! + commit_from_memory）✅ 已完成 + e2e 通过

### Task 1.1：模型文件放入工程
- [x] `crates/asr-local/models/silero_vad_v4.onnx`（1.7MB）

### Task 1.2：VadSource enum + find_silero_vad 改造
**文件**: `crates/asr-local/src/config.rs`
- [x] 新增 `pub enum VadSource { File(PathBuf), Builtin }`
- [x] `find_silero_vad() -> Result<VadSource>`：磁盘存在 → File，否则 → Builtin
- [x] 新增 `create_silero_vad()` 便捷函数（find + 构造一步到位，调用方无需 match）

### Task 1.3：SileroVad::new_builtin()
**文件**: `crates/asr-local/src/vad.rs`
- [x] `pub fn new_builtin() -> Result<Self>`：`include_bytes!` + `commit_from_memory` + 共享 VAD_SESSIONS 缓存

### Task 1.4：12 个调用点改造
- [x] desktop: coordinator.rs / main.rs / pipeline.rs(×2)
- [x] server: main.rs
- [x] cli: main.rs(×2)
- [x] asr-local: audio.rs(×2) / vad.rs / streaming_runner.rs / pipeline.rs

### Task 1.5：验证
- [x] cargo check 0 error 0 warning（asr-local + desktop + cli + server）
- [x] cargo test 8 个 VAD 测试全过
- [x] e2e：删 `~/.octopus/models/silero_vad_v4.onnx` → 启动 → 说话 → VAD 从内嵌加载 → 分段正常

---

## Step 2：source_type 重命名（is_local → source_type）✅ 已完成

### Task 2.1：DB schema 迁移 v47→v48 ✅
**文件**: `crates/infra/src/db.sql` + `crates/infra/src/db.rs`
- [x] db.sql：models 表 `is_local` 列改名 `source_type`（DEFAULT 1），注释更新（0=builtin/1=local/2=cloud）
- [x] db.sql：seed INSERT 的列名改 source_type（local 模型值保持 1 不变）
- [x] db.rs init_schema：v47→v48 迁移提取为 `migrate_v47_to_v48()` helper（RENAME COLUMN + UPDATE CASE 0→2/1→1）
  - **偏差**：原计划用单一 `if v == 47` 段，实际发现 v17-v46 老库走完 v46→v47 段后 return，到不了 v48。
    改为 helper 在 3 处调用（v==47 库 / v==46 段末尾 / v17-v46 段末尾），幂等保护已加。
  - zipformer builtin seed 行推迟到 Step 3 Task 3.1（本 task 只改列名不增 seed）
- [x] fill_manifests SQL：`is_local=1` → `source_type IN (0,1)`（builtin + local 都需 manifest）
- [x] 测试：新增 `migration_v47_to_v48_renames_is_local_to_source_type`（建 v47 库 → 验证列改名 + 数据 0→2/1→1）
- [x] 全新库分支 + 5 处老迁移测试的 user_version 断言 47→48

### Task 2.2：Rust struct + SQL 改造 ✅
**文件**: `crates/infra/src/db.rs`（7 个 struct）
- [x] 7 个 struct 字段 `is_local: bool` → `source_type: i64`：ModelEntry / CompatibleLlmConfig / AsrEngineRow / ModelRow / LlmModelInfo / OcrModelInfo / LocalAsrModelRow（后者无 is_local 字段，确认无需改）
- [x] ModelEntry 加 4 helper：`is_builtin()` / `is_local()` / `is_cloud()` / `is_local_or_builtin()`
- [x] serde default = `default_local_source_type()`（= 1），向后兼容旧 YAML/JSON
- [x] 所有 SQL row mapping：`r.get::<_, i32>(N)? != 0` → `r.get::<_, i64>(N)?`（source_type 直接 i64）
- [x] SQL WHERE：`is_local = 1` → `source_type IN (0,1)`；`is_local = 0` → `source_type = 2`
- [x] SQL ORDER BY：`is_local DESC` → `source_type ASC`（builtin<local<cloud，语义一致且更细）
- [x] 密钥迁移守卫 SQL：`is_local = 0` → `source_type = 2`

### Task 2.3：业务逻辑改造 ✅
- [x] `asr-local/config.rs`：EngineInfo 字段 + 排序（bool 降序 → i64 升序）+ fallback 字面量（true → 0 builtin）+ 测试
- [x] `asr-local/{zipformer,streaming_zipformer}.rs`：测试 fixture 字面量
- [x] `desktop/runtime_config.rs`：3 struct（EngineOption/LlmOption/OcrOption）+ engine_label 参数 + 测试 helper
- [x] `desktop/config.rs`：LLM polish 热路径 `if is_local` → `if is_local_or_builtin()`（语义不变）
- [x] `desktop/{translation_commands,action_bar_commands}.rs`：对称的 translate 策略分支
- [x] `desktop/settings_commands.rs`：连接测试分支
- [x] `desktop/vault_commands.rs`：测试 helper INSERT SQL
- [x] `translation/cloud.rs` / `llm/examples/test_polish.rs` / `vault/migrate.rs`：构造点 + 测试
- **偏差**：原计划「desktop/vault_*.rs 15 处」实际只有 config.rs 1 处真实分支 + vault_commands 测试 SQL。
  `try_decrypt_secret_global` 本身不查 is_local（靠前缀判定），vault_secret_access.rs 仅注释引用

### Task 2.4：前端改造（18 处）✅
- [x] 6 个文件的 TypeScript interface：`is_local: boolean` → `source_type: number`
  - ModelRow.tsx / AsrTab.tsx / OcrTab.tsx / LlmTab.tsx / Settings/index.tsx（5 个 interface）
- [x] 条件判断：`model.is_local` → `model.source_type !== 2`（本地+builtin）；`!model.is_local` → `model.source_type === 2`（cloud）
- [x] 硬编码值：本地行 `is_local: true` → `source_type: 1`；云端行 `is_local: false` → `source_type: 2`

### Task 2.5：验证 ✅
- [x] cargo build --workspace 0 error 0 warning
- [x] cargo test -p octopus-infra：159 pass（+1 新迁移测试）
- [x] cargo test -p octopus-asr-local：124 pass + 4 fail（paraformer real_model 测试，模型文件缺失，与本次改动无关）
- [x] cargo test -p octopus-vault migrate：8 pass
- [x] cargo test -p octopus-desktop --bins：394 pass
- [x] tsc 0 error + vite build 成功
- [x] e2e：模型管理页正常显示 + ASR 正常 + 云端模型正常（用户验证通过 2026-07-22）

---

## Step 3：zipformer builtin 入 DB + 自动下载页 ✅ 已完成

### Task 3.1：zipformer builtin 入 DB ✅
**文件**: `crates/infra/src/db.sql` + `model_manifests.rs` + `db.rs`
- [x] db.sql seed 加 zipformer-small 行（source_type=0, source='asr/zipformer-small'）
- [x] model_manifests.rs 加 ZIPFORMER_SMALL_CTC 常量（3 文件：bbpe.model + model.int8.onnx + tokens.txt，27M）
  - HF repo: `csukuangfj/sherpa-onnx-streaming-zipformer-small-zh-int8-2025-04-01`
  - sha256 + size 由本地 ~/.octopus/asr/zipformer-small/ 实际文件计算
- [x] asr_manifest match 加 "zipformer-small" 分支
- [x] fill_manifests 已覆盖 source_type IN (0,1)（Step 2 改的 SQL，builtin 自动包含）
- [x] migrate_v47_to_v48 内 INSERT builtin seed + fill_manifests
- [x] **ensure_builtin_seed() 兜底**（ensure_db 每次启动跑 INSERT OR IGNORE）——
  修复「迁移时代码不完整导致漏注入」的历史库幂等性问题

### Task 3.2：缺失检测 + 下载命令 ✅
**文件**: `crates/infra/src/db.rs`（list_builtin_models）+ `crates/desktop/src/builtin_models.rs`（新建）
- [x] infra: `list_builtin_models()` 查 source_type=0（跨 domain）
- [x] desktop: `check_builtin_models_missing()` — 查 DB + resolve_model_dir 检测缺失
- [x] `check_builtin_models` Tauri 命令 — 返回 BuiltinModelInfo[] 给下载页
- [x] **下载复用 model_commands::download_model**（不新建独立下载逻辑——builtin 模型已在 DB，
  download_model 按 source 查 manifest → 下载 → set_model_available）

### Task 3.3：下载页 UI ✅
**文件**: 前端新建 download.html + entries/download-main.tsx + pages/Download/index.tsx
- [x] vite.config.ts input 加 "download": "download.html"
- [x] download_window.rs（新建）：WebviewWindowBuilder 520×460，单例管理，close_download_window 命令
- [x] capabilities/default.json windows 数组加 "download_window"
- [x] DownloadPage 组件（frontend-design skill 指导，克制功能性风格）：
  - 标题区：说明 + 27M 提示
  - ModelCard：状态色条（pending/downloading/done/error）+ 进度条（复用 ModelRow 样式）
  - 底部：「稍后下载」+「下载并进入系统」+ 下载中显示「进入系统」
  - listen download-progress / download-done 更新卡片状态

### Task 3.4：desktop setup 集成 ✅
**文件**: `crates/desktop/src/main.rs`
- [x] setup 钩子里 action_bar_window 创建之前调 check_builtin_models_missing
- [x] 缺失 → create_download_window（不阻断主窗口创建，并存模式）
- [x] generate_handler! 注册 check_builtin_models + close_download_window
- [x] mod 声明加 builtin_models + download_window

### Task 3.5：验证 ✅
- [x] cargo build --workspace 0 error 0 warning
- [x] cargo test -p octopus-infra：159 pass（含 v48 迁移 + builtin seed 测试）
- [x] cargo test -p octopus-desktop --bins：394 pass
- [x] tsc 0 error + vite build 成功（download.html 产物生成）
- [x] DB 验证：cli 触发 ensure_db → builtin seed 注入（source_type=0, manifest 填充）
- [x] e2e：删 zipformer 目录 → 启动 → 下载窗弹出 → 下载 → ASR 可用（用户验证通过 2026-07-22）

---

## 代码审查修复（2 轮，2026-07-22）

### 第 1 轮（6 个问题）
- [x] **完整性校验**：sync_builtin_models_availability 从 stat 目录改为逐文件 sha256（与 download_model 对齐）
- [x] **注释修正**：download_model 注释「置 is_enabled=true」→「置 is_available=true」
- [x] **增量下载**：known_broken 集合传到下载循环，完好文件跳过不重复 sha256
- [x] **文案如实**：下载页「后台下载」→「下载并进入系统」（前台串行，非后台）
- [x] **建窗日志**：download_window build() 错误从 `let _ =` 吞没改为 `log::error!`
- [x] **合并查询**：sync + check 合并为 check_and_sync_builtins 共享一次 DB 查询

### 第 2 轮（3 个问题）
- [x] **OnceLock 缓存**：sync 算一次并缓存，check 读缓存避免重复 sha256（~54MB IO/冷启动）
- [x] **DB 失败前端显示错误**：check_and_sync_builtins 返回 Result，前端 loadError 状态显示错误（不误报「已就绪」）
- [x] **注释残留**：download_window.rs 注释「后台下载」→「下载并进入系统」

### 额外修复（e2e 发现）
- [x] **模型名统一**：zipformer-small-ctc → zipformer-small，source models/zipformer → asr/zipformer-small
- [x] **source_type 透传**：LocalAsrModelRow + DownloadableModel 加 source_type 字段，前端不再硬编码 1
- [x] **busy 按行隔离**：busy={!!busyRepo} → busy={busyRepo === m.repo}
- [x] **builtin 禁删**：删除按钮 source_type===0 时 disabled（灰掉占位，图标对齐）
- [x] **激活前校验**：onActivate 先 verify_model，损坏/缺失自动 download_model 修复
- [x] **校验失败自动下载**：onVerify ok=false 时自动触发 onDownloadInternal
- [x] **CLI download 删除**：resolve_tasks HF API 路径无需求，删除 CLI download 子命令 + hf/ 模块

---

## Step 4：下载增强（多文件并发 + DownloadPopover 浮层，2026-07-23）

### Task 4.1：后端多文件并发下载
- [x] download_model 串行 for 改为 JoinSet + Semaphore(4) 并发
- [x] Downloader 用 Arc 共享（&self 方法跨 task 安全）
- [x] download-progress 事件加 file 字段（并发后按文件区分进度）
- [x] download-file 事件改语义 {repo, file, status}（start/done/error/skip）
- [x] 清理重复 dest 定义 + create_dir_all（L234-237 bug）

### Task 4.2：DownloadPopover 组件（文件级进度浮层）
- [x] 新建 components/DownloadPopover.tsx：文件列表 + 文件级进度条
- [x] 复用 SaveImagePopover 的 outside-click + absolute 定位骨架
- [x] 新增 list_model_files Tauri 命令（manifest 文件 + sha256 校验 exists）
- [x] fmtBytes 提取到 lib/utils（4 处重复定义统一）

### Task 4.3：ModelRow 集成 hover 浮层
- [x] 本地+builtin 模型行加 FileDown 图标按钮
- [x] hover/click 弹出 DownloadPopover 展示文件列表 + 进度

### Task 4.4：验证
- [x] cargo build 0 error 0 warning + desktop 394 pass
- [x] tsc OK + vite build OK
- [ ] e2e（待用户验证）：hover 模型行看文件列表 + 并发下载 + 文件级进度

---

## Step 5：download crate 代码审查修复（4 轮，2026-07-23）

多轮代码审查发现的 download crate（`crates/download/src/core/downloader.rs`）严重 bug 修复。

### 第 1 轮：3 个严重 bug
- [x] **hash 校验重试空转**：只重算同一文件 hash（确定性失败）→ 改为失败→删 .part→重下整个文件→再校验
- [x] **200 全文段错位**：多段下载遇 200 时非首段写入全文前 N 字节（错位）→ 先跳过 seg.begin 字节再写段数据
- [x] **stream 无超时**：body 流读取无 timeout（TCP stall 永久挂起）→ stream.next() 包裹 tokio::time::timeout

### 第 2 轮：消除重复实现
- [x] **方法/自由函数分叉**：修复打在仅测试调用的方法版（死代码），生产路径走自由函数（旧 buggy 逻辑）→ 方法版改为一行委托自由函数，删 130 行重复实现

### 第 3 轮：流提前结束 + 守护测试
- [x] **流提前结束静默成功**：Ok(None) break 后 written < expected 但仍返回 Ok → 加尾部校验返回 transient
- [x] **空 chunk 过度反应**：write_len == 0 break 改为 continue
- [x] **守护测试**：补 download_segment_short_stream_returns_transient（mock 短 body 断言 Transient）

### 第 4 轮：counter 回滚 + hash 重下 + 其他
- [x] **counter Transient 不回滚**：段级重试后 counter 虚高 >100% → RAII CounterGuard 统一兜底（drop 时未 commit 自动 fetch_sub）
- [x] **hash 重下进度泵已停**：retry_counter 独立、pump 已 abort → 复用主 counter + 重启 pump
- [x] **hash 重下失败跳过 sidecar 清理**：`?` 提前返回 → 显式错误处理 + 清理
- [x] **skip 循环无 cancel**：加 cancel.is_cancelled() 检查
- [x] **sem.acquire_owned unwrap**：改 map_err

### 不修（设计取舍）
- skip 字节不计 counter（属于其他段区间）
- list 不做运行期探针（严格换实时性，verify/activate 兜底）
- 416 归 Fatal（实际命中罕见）
- counter 回滚后进度条短暂倒退（比 >100% 可接受）

---

## 文档同步
- [x] architecture.md：models 表 source_type 描述 + builtin 模型机制
- [x] plan 文档：Step 2/3 全部 Task 标记完成 + 实际偏差记录
- [x] AGENTS.md：schema v46→v48 + zipformer 运行时布局描述更新
- [x] features/db-and-config.md：models 表 is_local → source_type 字段 + builtin 兜底引擎描述
- [x] features/asr-engine.md：resolve_model_dir + 兜底引擎描述更新（随包 → 首次下载）
- [x] e2e 验证（用户验证通过 2026-07-22）

---

## Step 6：校验性能优化（sidecar 缓存 + 分层校验，2026-07-23）

### 问题
hover 浮层 / 激活 / 启动 sync 每次都读整个文件算 SHA256（26MB ~百毫秒），明显卡顿。

### 方案：`.verified.json` sidecar 缓存 + 分层校验

| 场景 | 校验方式 | 耗时 |
|------|----------|------|
| 启动 sync | stat 快检（size+mtime 匹配缓存） | 微秒级 |
| hover 浮层 | stat 快检 | 微秒级 |
| 激活前自动校验 | stat 快检（verify_model full=false） | 微秒级 |
| 手动校验按钮 | 完整 SHA256（verify_model full=true） | ~百毫秒 |
| 下载完成后 | 直接写 `.verified.json`（Downloader 已校验 hash） | 微秒级 |

### Task 6.1：sidecar 缓存
- [x] VerifiedCache / VerifiedEntry struct（serde，存 `.verified.json`）
- [x] check_file_with_cache：stat 快检 → 缓存命中跳过 SHA256 → 不匹配才算 + 更新缓存
- [x] list_model_files 改 async + spawn_blocking（消除 hover 卡顿）
- [x] verify_model_inner 也用缓存（激活不再卡顿）

### Task 6.2：分层校验
- [x] verify_model 加 `full` 参数（手动校验 full=true 强制 SHA256，激活 full=false stat 快检）
- [x] check_builtin_ready 改为 stat 快检
- [x] download_model 成功后直接写 `.verified.json`（不重新校验）
- [x] VerifiedCache/Entry + check_file_with_cache 改 pub(crate) 供 builtin_models 复用

### Task 6.3：UX
- [x] loading 浮层显示标题 + 「正在校验文件…」提示（不再空白转圈）
- [x] 测试补 counter 回滚守护断言（成功 + 失败路径）

### Task 6.4：验证
- [x] cargo build 0 error + desktop 394 pass + tsc OK
- [ ] e2e（待用户验证）：hover/激活秒开，手动校验才读文件
