# Rust-Patterns 专项审查设计

- 日期：2026-07-05
- 分支：`rust-review-2026-07-05`
- 审查报告：`docs/rust-review-2026-07-05.md`
- 关联：与 `2026-07-05-code-review-remediation-design.md`（深度审查 P0/P1/P2）独立，本 spec 只覆盖 rust-patterns skill 六大领域的专项审查

## 1. 背景与目标

用 `rust-patterns` skill 对 main（6ce7b36）做专项审查，聚焦六大领域：所有权/借用、错误处理、枚举/模式匹配、Trait/泛型、并发、模块可见性。

**总览结论**：clippy 零警告，269 测试全绿，代码质量较高。发现 3 个 P1 + 3 个 P2 问题。

## 2. P1 修复（已完成）

### P1-1: Mutex lock().unwrap() poisoned panic 风险

**位置**：`cli/main.rs`(10 处)、`download/downloader.rs`(1 处)

**方案**：`.lock().unwrap()` → `.lock().unwrap_or_else(|e| e.into_inner())`，poisoned mutex 不再 panic 传播，恢复数据继续执行。

### P1-2: HeaderValue parse unwrap

**位置**：`desktop/src/settings_commands.rs:449`

**方案**：`.parse().unwrap()` → `.parse().map_err(...)?`，secret_key 含非法 HTTP header 字符时返回错误而非 panic。

### P1-3: ndarray as_slice().unwrap()

**位置**：`asr-local/src/streaming_paraformer.rs`(3 处：encoder 输出 2 处 + decoder cache 1 处)

**方案**：`.as_slice().unwrap()` → `.as_slice().ok_or_else(|| anyhow!(...))?`，非连续内存布局时返回错误而非 panic。

## 3. P2 修复（已完成）

### P2-1: pub(crate) 收窄

将 14 个零外部调用的 `pub fn` 收窄为 `pub(crate)`：

| Crate | 函数 | 数量 |
|---|---|---|
| clipboard/store.rs | `insert_asr_item`、`update_segments`、`count_all` | 3 |
| download | `classify_status`、`dest_hash`、`sidecar_path`、`new_state`、`part_path`、`plan_segments`、`should_download`、`fnmatch` | 8 |
| llm/prompt.rs | `build_system_prompt`、`user_prompt`、`regions_prompt` | 3 |

### P2-3: paste.rs thread::sleep

**结论**：`paste` 已在 `spawn_blocking` 内调用（`coordinator.rs:1259-1260`），`thread::sleep` 不阻塞 async runtime。无风险，无需改动。

### P2-2: cloud_pipeline block_on

**结论**：`tauri::async_runtime::block_on` 在 coordinator 主线程调用 `open_cloud_session`。注释说明 open 只 spawn task 立即返回不阻塞。消除需架构级重构（改 channel / spawn 接管），暂不动。

## 4. 不需要改动的部分

| 领域 | 状态 |
|---|---|
| 测试中 unwrap() | 正常——测试 panic 是期望行为 |
| ONNX session.run().unwrap() 在测试 | 正常——依赖模型文件存在 |
| stitch.rs / zipformer.rs unwrap | 均在 `!is_empty()` 守护下 |
| unsafe 全部有 SAFETY 文档 | denoise.rs、screenshot_commands.rs 等 |
| download error.rs | thiserror 最佳实践典范 |
| 通配符匹配 | 仅 2 处 UI 边界，降级语义正确 |
