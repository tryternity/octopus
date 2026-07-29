# 删除 vault pull_from_files 死代码 spec

> **Status: ✅ 已完成**（2026-07-29，分支 `daily_bugfix_0729`）

## 1. 背景

`pull_from_files`（engine.rs:859-1038，179 行）是**生产死代码**：
- `sync_now`（生产同步入口）走 `merge_vault`
- `clone_initial`（首次 clone）走 `store::import_all_from_files`
- 无任何生产代码调用 `pull_from_files`
- 仅 11 个测试调用它
- 函数注释（engine.rs:1220）自述「clone_initial 仍用 pull_from_files」，**与事实矛盾**

## 2. 策略：先迁移测试 → 再删死代码

pull_from_files 的 11 个测试守护了 merge_vault **未覆盖**的关键安全场景：
- stamp 校验拒绝/通过（INV-S9）
- meta 缺失异常态处理
- weak KDF 参数拒绝
- **软删 is_deleted 保留**（H2 不变量——防软删密码复活）
- 损坏文件容错
- folder rename 捕获
- sync_enc 不一致清空 local_enc

这些逻辑在 merge_vault 里**都有对应实现**（阶段 A stamp 校验、build_cipher_input_from_file 保留 is_deleted 等），但 merge_vault 的 7 个测试没覆盖这些场景。

迁移 = 把测试调用从 `pull_from_files()` 改为 `merge_vault()`，既删死代码又补 merge_vault 测试缺口。

## 3. 迁移要点

- 返回类型变化：`pull_from_files() -> Result<(usize, usize)>` → `merge_vault() -> Result<MergeReport{pulled,pushed,conflicts,skipped}>`
- merge_vault 比 pull 多 push 阶段——某些测试的 "pulled=N" 断言可能需适配 MergeReport 字段
- 错误类型一致（都返回 `SyncError`），错误断言（如 `MasterPasswordMismatch`）无需改
- 软删测试（H2 不变量）是安全关键——迁移后必须验证仍守护

## 4. 风险
- 若某测试迁移后失败（说明 merge_vault 与 pull_from_files 行为有细微差异），需调查而非强行改断言
- 软删 is_deleted 保留是数据安全关键，迁移后验证不复活
