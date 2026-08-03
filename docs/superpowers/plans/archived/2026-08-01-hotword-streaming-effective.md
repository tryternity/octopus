# 实施计划：热词在流式听写中生效

> **对应 spec**：[2026-08-01-hotword-streaming-effective.md](../specs/2026-08-01-hotword-streaming-effective.md)
> **分支**：`bugfix/pr-0801`（worktree `.worktrees/bugfix_pr_0801`）
> **状态**：✅ 已完成

## 任务分解 + 实施记录

### Task 1：写失败测试（TDD 红）✅

文件：`crates/asr-local/src/streaming/streaming_runner.rs` 的 `#[cfg(test)]`

3 个测试（复用 `FakeStreamingEngine` + corrector 全局单例）：
- `streaming_runner_correct_applied_when_enabled` —— correct=true，Partial 被纠错
- `streaming_runner_no_correct_when_disabled` —— correct=false，原样返回（守护）
- `streaming_runner_finish_applies_correct_when_enabled` —— correct=true，finish 的 Final 被纠错

辅助：`serial()`（corrector 串行）+ `load_hotwords` + `runner_with_correct`。

**验证**：finish 测试失败（`left: "我们以经到了"` ≠ `right: "我们已经到了"`），其余通过 ✓

### Task 2：finish 加 corrector 注入 ✅

文件：`crates/asr-local/src/streaming/streaming_runner.rs`

- `finish()` 加 `if self.correct { corrector.correct(&text) }` 分支（在 ITN 前）
- `correct` 字段注释更新（去掉「默认 false」，改为「调用方按 asr_correct && language != en 传入」）

**验证**：3 个测试全绿 ✓

### Task 3：coordinator 传 correct + drain/bump ✅

- `session.rs:232` + `lifecycle.rs:299`：`from_session(streaming_engine, correct)`，`correct = config.asr_correct && !language.eq_ignore_ascii_case("en")`
- `lifecycle.rs` finish 后 `apply_engine_full` 之后：`drain_hits()` + `bump_hotword_hit_by_word` 循环

### Task 4：asr_correct 默认改 true ✅

- `config.rs::default_asr_correct()` → `true`（带注释说明）
- `db.sql` seed `'false'` → `'true'`
- `config.rs::app_config_default_values` 测试 `assert!(!cfg.asr_correct)` → `assert!(cfg.asr_correct)`
- `db/config.rs:216` 测试哨兵值 `cfg.asr_correct = false` **不改**（roundtrip 哨兵，与默认不同才有效）

### Task 4.5：corrector 测试串行锁跨模块共享（TDD 中发现）✅

**问题**：`streaming_runner_correct_applied_when_enabled` 单独跑过，全 crate 跑失败——corrector 全局单例被 corrector tests 模块的测试污染（两模块各自 `serial()` 用不同锁，跨模块不互斥）。

**修复**：corrector.rs 顶层新增 `#[cfg(test)] pub(crate) CORRECTOR_TEST_LOCK` + `test_serial()`，corrector tests 内的 `serial()` 和 streaming_runner tests 内的 `serial()` 都复用 `crate::text::corrector::test_serial()`。删除 corrector tests 内冗余的 `static TEST_LOCK` + `use std::sync::{Mutex, OnceLock}`。

**验证**：全 crate `cargo test -p octopus-asr-local --lib` 149 passed（此前 148 + 1 failed → 修复后 149 + 0 failed）✓

### Task 5：编译 + 测试 + 影响面 ✅

| 命令 | 结果 |
|---|---|
| `cargo build -p octopus-asr-local -p octopus-infra -p octopus-vault` | 0 error 0 warning |
| `cargo build -p octopus-desktop`（需 `crates/desktop/binaries/octopus-sck-helper` 占位） | 0 error 0 warning |
| `cargo test -p octopus-asr-local --lib` | 149 passed |
| `cargo test -p octopus-infra --lib` | 154 passed |
| `cargo test -p octopus-desktop` | 488 passed |
| `cargo build --release -p octopus-server -p octopus-cli` | 0 error 0 warning |
| `cargo test`（全 workspace） | 0 failed |

影响面 grep：
- `from_session(streaming_engine` 两处都传 `correct` 变量 ✓
- `asr_correct` 默认 true（config.rs + db.sql + assert）✓
- 流式 drain/bump（lifecycle.rs）✓

**⚠️ worktree 编译注意**：`cargo` 工作目录必须在 worktree（`.worktrees/bugfix_pr_0801`）而非主仓库——否则编译的是 main 代码。tauri externalBin 检查需要 `crates/desktop/binaries/octopus-sck-helper`（+ target-triple 后缀）占位文件存在，否则 build.rs 失败（pre-existing 打包要求，与本次改动无关）。

### Task 6：文档同步✅

- 新 spec：`docs/superpowers/specs/2026-08-01-hotword-streaming-effective.md`
- 本 plan：`docs/superpowers/plans/2026-08-01-hotword-streaming-effective.md`
- `docs/features/asr-engine.md` §注入点：补「流式热词纠错（2026-08-01 激活）」段落（激活 correct + 命中入库 + 门控位置 + 默认 true）
- `docs/architecture.md` §拼音纠错与热词校正：`asr_correct` 默认 false → true；作用范围补流式路径激活描述
- `docs/pr/0801.md`：标记问题 2 完成

## 与计划的偏差

- **Task 4.5（测试串行锁）是计划外新增**：TDD 过程中发现 corrector 全局单例跨模块测试污染，新增跨模块共享锁。这是实施中发现的必要修复，已回写到本 plan。
- 其余无偏差。

## 不在本次范围

- cloud 流式路径热词纠错（spec §5）
- 流式引擎 `skip_corrector` trait 方法（spec §5）
- 问题 1（已完成）、问题 3（另起 plan）
