# 内置模型开箱即用实施计划

> **Spec:** `docs/superpowers/specs/2026-07-22-builtin-models.md`
> **分 3 步，每步 e2e 通过后再做下一步。**

---

## Step 1：VAD 内嵌（include_bytes! + commit_from_memory）

### Task 1.1：模型文件放入工程
- [x] `crates/asr-local/models/silero_vad_v4.onnx`（1.7MB）

### Task 1.2：VadSource enum + find_silero_vad 改造
**文件**: `crates/asr-local/src/config.rs`
- [ ] 新增 `pub enum VadSource { File(PathBuf), Builtin }`
- [ ] `find_silero_vad() -> Result<VadSource>`：磁盘存在 → File，否则 → Builtin

### Task 1.3：SileroVad::new_builtin()
**文件**: `crates/asr-local/src/vad.rs`
- [ ] `pub fn new_builtin() -> Result<Self>`：`include_bytes!` + `commit_from_memory` + 共享 VAD_SESSIONS 缓存

### Task 1.4：12 个调用点改造
- [ ] desktop: coordinator.rs / main.rs / pipeline.rs(×2)
- [ ] server: main.rs
- [ ] cli: main.rs(×2)
- [ ] asr-local: audio.rs(×2) / vad.rs / streaming_runner.rs / pipeline.rs

### Task 1.5：验证
- [ ] cargo check 0 error
- [ ] 删 `~/.octopus/models/silero_vad_v4.onnx` → e2e VAD 正常

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

## Step 3：zipformer builtin 入 DB + 自动下载页

### Task 3.1：zipformer builtin 入 DB
**文件**: `crates/infra/src/db.sql` + `model_manifests.rs`
- [ ] db.sql seed 加 zipformer-small-ctc 行（source_type=0）
- [ ] model_manifests.rs 加 zipformer-small-ctc manifest（HF repo + sha256）
- [ ] fill_manifests 覆盖 source_type=0

### Task 3.2：ensure_builtin_models() + 下载命令
**文件**: `crates/desktop/src/builtin_models.rs`（新建）
- [ ] 查 DB source_type=0 → 检查本地文件 → 返回缺失列表
- [ ] `download_builtin_model(name)` 命令：HfRequest + resolve_tasks + Downloader + 进度 emit
- [ ] `check_builtin_models()` 命令：返回缺失列表

### Task 3.3：下载页 UI
**文件**: 前端新建 `pages/BuiltinDownload/` 或主窗口启动路由
- [ ] 缺失 builtin 模型列表 + 大小
- [ ] 「后台下载并进入系统」/「稍后下载」按钮
- [ ] 下载进度（listen 后台 emit）
- [ ] 下载完成 → set_model_available + 通知

### Task 3.4：desktop setup 集成
**文件**: `crates/desktop/src/main.rs`
- [ ] setup 里 ensure_db 之后调 check_builtin_models
- [ ] 缺失 → 前端路由到下载页

### Task 3.5：验证
- [ ] 删 zipformer → 启动 → 下载页 → 点下载 → 进系统 → 后台下载完成 → ASR 可用
- [ ] 文件已存在 → 直接进系统
- [ ] 选「稍后下载」→ 进系统（ASR 无兜底）

---

## 文档同步
- [ ] AGENTS.md 运行时文件布局
- [ ] architecture.md source_type + builtin 模型说明
- [ ] features/ 相关章节更新
