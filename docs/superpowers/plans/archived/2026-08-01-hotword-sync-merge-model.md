# 实施计划：热词同步升级到 merge 模型

> **对应 spec**：[2026-08-01-hotword-sync-merge-model.md](../specs/2026-08-01-hotword-sync-merge-model.md)
> **分支**：`bugfix/pr-0801`（worktree `.worktrees/bugfix_pr_0801`）
> **状态**：✅ 已完成

## 任务分解 + 实施记录

### Task 1：写失败测试（TDD 红）✅

文件：`crates/sync/src/hotword.rs` 的 `#[cfg(test)] mod tests`

新增 4 个 merge 测试：
- `merge_pulls_remote_newer_set` —— 远程 updated_ms 较新 → pull 覆盖 DB
- `merge_keeps_local_newer_set_not_overwritten` —— **核心回归**：本地新词不被旧 outline 覆盖
- `merge_pushes_db_only_set` —— DB 有 outline 无 → push 写文件
- `merge_db_wins_on_equal_timestamp_md5_conflict` —— 时间戳相等 + md5 冲突 → DB 赢

辅助函数 `write_remote_set(id, name, words, updated_ms)`：手写 outline + set 文件模拟远程状态。

**验证**：`cargo test -p octopus-sync --lib hotword --no-run` → 4 处 `cannot find function merge_hotwords` 编译失败 ✓

### Task 2：实现 `merge_hotwords`（TDD 绿）✅

文件：`crates/sync/src/hotword.rs`

新增：
- `HotwordMergeReport { pulled, pushed, conflicts, skipped }`（独立于 vault `MergeReport`，因 sync 不能依赖 vault）
- `merge_hotwords() -> Result<HotwordMergeReport>`：3-way merge，对称 `merge_vault`，去掉 stamp/meta 校验

**验证**：`cargo test -p octopus-sync --lib hotword` → 33 passed（含 4 个新 merge 测试）✓

### Task 3：`sync_now` 切换到 merge + 过时注释更新✅

文件：`crates/vault/src/sync/engine.rs`

- `sync_now`（line ~745）：pull/push 两步 → 单步 merge（`skip_pull` 时走 push_hotwords_to_files，其余走 merge_hotwords）
- 更新过时注释（line ~716-724）："热词已升级到 merge_hotwords（2026-08-01），对称于 merge_vault"
- `pull_hotwords_from_files` 文档注释加 ⚠️ 无方向感知警告 + 指向 merge_hotwords
- 既有测试 `pull_overwrites_local_new_data_when_outline_stale_documented_bug` 改名 `pull_function_direction_blind_by_design`，注释从「文档化 bug」改为「文档化设计契约」
- `push_exports_local_new_data_when_outline_stale` 注释更新（去掉「配合 UpToDate 跳过 pull」过时描述）

**验证**：`cargo build -p octopus-vault -p octopus-sync` → 0 error 0 warning ✓

### Task 4：全量测试 + 影响面追踪✅

| 命令 | 结果 |
|---|---|
| `cargo test -p octopus-sync --lib` | 110 passed, 0 failed |
| `cargo test -p octopus-vault --lib` | 258 passed, 0 failed |
| `cargo build --release -p octopus-server -p octopus-cli` | 0 error 0 warning |
| `cargo build -p octopus-desktop` | 0 error 0 warning（pre-existing helper binary 警告与本次无关） |
| `cargo test`（全 workspace 核心） | 0 failed |

影响面 grep 确认：
- `merge_hotwords` / `HotwordMergeReport`：仅 sync crate 定义 + engine.rs 映射 ✓
- `pull_hotwords_from_files`：生产路径无调用（仅测试 + 函数定义保留）✓
- `push_hotwords_to_files`：仍被 sync_now NoUpstream 分支 + enable_sync 测试用 ✓

### Task 5：文档同步✅

- 新 spec：`docs/superpowers/specs/2026-08-01-hotword-sync-merge-model.md`
- 旧 spec 注记：`docs/superpowers/specs/archived/2026-07-25-hotword-sync-overwrite-bug.md` 顶部加归档说明
- `docs/architecture.md` line 971：更新「UpToDate 仍执行 pull」→「UpToDate 仍执行 merge」+ 热词 merge 升级背景；更新热词 sync 模块描述（pull/push engine → merge engine）
- 源码注释：`hotword.rs` 顶部模块注释待补 merge 模型段落（见下文「待办」）

## 与计划的偏差

无重大偏差。原计划 Task 5 提到「更新 hotword.rs 顶部模块注释加 `## 同步模型` 段落」——实际改为在 `merge_hotwords` 函数文档注释 + `pull_hotwords_from_files` 文档注释中详述，模块顶部注释保持（避免重复）。这是更好的位置（紧贴代码）。

## 不在本次范围

- 热词软删 tombstone（跨设备删除复活）：记入 spec §5 已知问题，留作后续
- 问题 2（热词不生效）、问题 3（instant 实时文本）：另起 plan
