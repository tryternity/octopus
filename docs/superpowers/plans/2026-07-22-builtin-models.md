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

## Step 2：source_type 重命名（is_local → source_type）

### Task 2.1：DB schema 迁移 v47→v48
**文件**: `crates/infra/src/db.sql` + `crates/infra/src/db.rs`
- [ ] db.sql：models 表 `is_local` 列改名 `source_type`，注释更新（0=builtin/1=local/2=cloud）
- [ ] db.sql：seed INSERT 的 is_local 值改为 source_type（local 模型=1）
- [ ] db.rs init_schema：v47→v48 迁移（RENAME COLUMN + UPDATE CASE 0→2/1→1 + INSERT zipformer builtin 行）
- [ ] 测试 assert v47→v48

### Task 2.2：Rust struct + SQL 改造
**文件**: `crates/infra/src/db.rs`（65 处）+ 其他 crate
- [ ] `Model` struct：`is_local: bool` → `source_type: i64`
- [ ] 加 helper: `is_builtin()` / `is_local()` / `is_cloud()` / `is_local_or_builtin()`
- [ ] 所有 SQL 语句 is_local → source_type
- [ ] row mapping 改

### Task 2.3：业务逻辑改造（~87 处）
- [ ] `asr-local/config.rs`（22 处）：is_local 判断改为 source_type helper
- [ ] `desktop/runtime_config.rs`（21 处）
- [ ] `desktop/vault_*.rs`（15 处）：try_decrypt_secret_global 的 is_local 判断
- [ ] 其他（~29 处）

### Task 2.4：前端改造（18 处）
- [ ] TypeScript interface: is_local → source_type
- [ ] 条件判断改为 source_type helper

### Task 2.5：验证
- [ ] cargo check 0 error / tsc 0 error
- [ ] cargo test -p octopus-infra → 迁移测试
- [ ] e2e：模型管理页正常显示 + ASR 正常 + 云端模型正常

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
