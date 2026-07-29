# record_commands.rs 拆分 plan（desktop crate 大文件重构 #5）

> **对应 spec**: `docs/superpowers/specs/2026-07-30-record-commands-split.md`
> **分支**: `daily_refactor_record`

## 阶段 0：目录化

### Task 0.1 — record_commands.rs → record_commands/mod.rs
- `mkdir -p crates/desktop/src/record_commands && git mv ... mod.rs`
- 验证：build + test

---

## 阶段 1：子模块提取（按行数从小到大）

### Task 1.1 — permission.rs（~165 行）
搬出（grep 重新定位行号）：
- `pub async fn check_record_permission` / `request_screen_record_permission`
- `pub async fn open_privacy_settings`
- `fn probe_microphone_permission`
- `pub async fn check_microphone_permission` / `request_microphone_permission`
- `pub async fn check_accessibility_permission` / `request_accessibility_permission`

### Task 1.2 — library.rs（~250 行）
搬出：
- `pub struct ListRecordingsParams` + `impl From`
- `pub async fn list_recordings` / `get_recording` / `get_recording_thumbnail` / `rename_recording` / `toggle_recording_favorite`
- `pub async fn delete_recording` / `open_recording_file` / `reveal_recording`
- `pub struct RecordStatus` + `pub async fn get_record_status`

### Task 1.3 — postprocess.rs（~620 行）
搬出：
- ffmpeg helpers：`pub(crate) fn probe_ffmpeg` / `async fn find_ffmpeg` / `fn ffmpeg_missing_hint` / `pub async fn check_ffmpeg`
- `pub async fn export_gif`
- `pub struct MergeResult` + `pub enum RecordTaskEvent` + `pub async fn merge_audio_tracks`
- `pub async fn generate_subtitle` / `async fn generate_subtitle_inner`
- `pub async fn read_subtitle` / `reveal_subtitle`
- `pub struct LlmOption` + `pub async fn list_subtitle_llms` + `fn capitalize`

### Task 1.4 — control.rs（~510 行，含测试）
搬出：
- `pub struct RecordConfig`
- `pub async fn record_start` / `record_start_default` / `record_pause` / `record_resume` / `record_stop` / `record_kill`
- `pub(crate) async fn build_default_config` / `start_with_config`
- `fn parse_bool_config` / `resolve_mic_device_name`
- `pub(crate) async fn stop_and_store` / `fn derive_fields_from_request` / `pub(crate) struct MetaFields` / `async fn stop_and_store_inner`
- 测试：3 个 `resolve_mic_device_name_*`

---

## 阶段 2：收尾

### Task 2.1 — 文档同步 + 全量验证
- spec status → ✅ + architecture.md（补 record_commands 条目）
- 全量验证：embedded / cloud,vault + test

---

## 验证 checklist
- [ ] `cargo build -p octopus-desktop --features embedded,cloud,vault` — 0 error 0 warning
- [ ] `cargo test -p octopus-desktop` — 441 passed

## 回滚
每个 Task 独立 commit。失败 `git reset --hard HEAD~1`。
