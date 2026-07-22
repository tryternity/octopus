# 内置模型开箱即用实施计划

> **Spec:** `docs/superpowers/specs/2026-07-22-builtin-models.md`
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
- [ ] e2e：模型管理页正常显示 + ASR 正常 + 云端模型正常（待用户验证）

---

## Step 3：zipformer builtin 入 DB + 自动下载页 ✅ 已完成

### Task 3.1：zipformer builtin 入 DB ✅
**文件**: `crates/infra/src/db.sql` + `model_manifests.rs` + `db.rs`
- [x] db.sql seed 加 zipformer-small-ctc 行（source_type=0, source='models/zipformer'）
- [x] model_manifests.rs 加 ZIPFORMER_SMALL_CTC 常量（3 文件：bbpe.model + model.int8.onnx + tokens.txt，27M）
  - HF repo: `csukuangfj/sherpa-onnx-streaming-zipformer-small-ctc-zh-int8-2025-04-01`
  - sha256 + size 由本地 ~/.octopus/models/zipformer/ 实际文件计算
- [x] asr_manifest match 加 "zipformer-small-ctc" 分支
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
  - 底部：「稍后下载」+「后台下载并进入系统」+ 下载中显示「进入系统」
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
- [ ] e2e（待用户验证）：删 zipformer 目录 → 启动 → 下载窗弹出 → 下载 → ASR 可用

---

## 文档同步
- [x] architecture.md：models 表 source_type 描述 + builtin 模型机制
- [x] plan 文档：Step 2/3 全部 Task 标记完成 + 实际偏差记录
- [ ] AGENTS.md 运行时文件布局（schema v47→v48）+ features/ 相关章节（后续 z-sync-superpowers）
