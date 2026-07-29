# screenshot_commands.rs 拆分 plan（desktop crate 大文件重构 #4）

> **对应 spec**: `docs/superpowers/specs/2026-07-30-screenshot-commands-split.md`
> **状态**: ✅ 全部完成（2026-07-29，分支 `daily_bugfix_0729`）

## 阶段 0：目录化

### Task 0.1 — screenshot_commands.rs → screenshot_commands/mod.rs
- `mkdir -p crates/desktop/src/screenshot_commands && git mv ... mod.rs`
- 验证：build（embedded）+ test

---

## 阶段 1：子模块提取

### Task 1.1 — shared.rs（~50 行）
搬出（被 area + scroll + 外部 record_area_picker 共用）：
- `fn format_file_size` (L32)
- `pub(crate) fn get_primary_screen_height` (L939)
- `pub(crate) fn get_window_cocoa_frame` (L950)
- `pub(crate) fn active_display_for_point` (L965)

### Task 1.2 — scroll.rs（~880 行）
搬出（L755-1035 helper + L1028-1632 核心）：
- `fn close_all_screenshot_windows` (L755) — 滚动停止时用
- `enum ScrollStopMode` (L776)
- `struct ScrollRecordingGuard` + `impl Drop` (L797-810)
- `struct SendApp` (L811)
- `fn save_frontmost_app` / `fn activate_prev_app` (L819-859)
- `fn get_window_pid_at_point` (L860)
- `fn activate_app_by_pid` (L910)
- `fn get_window_number`（双 cfg 版本 L928/L936）
- `fn set_window_ignores_mouse_events`（双 cfg 版本 L982/L997）
- `fn set_app_active_on_main` (L1006)
- `pub struct InteractiveRect` (L1028)
- `pub async fn start_scroll_recording` (L1036，580 行巨函数，原样搬)
- `pub fn stop_scroll_recording` / `pub fn stop_scroll_recording_with_mode` (L1618-1632)

### Task 1.3 — area.rs（~700 行）
搬出 mod.rs 剩余全部（区域截图）：
- `mod cg_event_source_ffi` (L17) + `fn right_mouse_button_down` (L26)
- `struct ScreenCaptureClone` (L42)
- `pub fn register_screenshot_shortcut` (L69)
- `fn register_scroll_esc` / `fn unregister_scroll_esc` (L100-135)
- `pub async fn start_screenshot` (L136)
- `fn save_screenshot_to_history` (L301)
- `pub async fn ocr_screenshot` (L355)
- `pub async fn scan_qrcode_screenshot` (L453)
- `pub fn get_last_screenshot_ocr` (L475)
- `pub fn show_screenshot_window` / `fn show_all_screenshot_windows` (L491-521)
- `pub async fn save_screenshot_dialog` (L522)
- `pub async fn confirm_screenshot_with_data` (L562)
- `pub fn get_screenshot_image` / `pub fn get_screenshot_image_size` (L591-619)
- `pub async fn confirm_screenshot` (L620)
- `pub async fn cancel_screenshot` (L672)
- `pub async fn pin_screenshot` (L681)

---

## 阶段 2：收尾

### Task 2.1 — 文档同步 + 全量验证
- spec status → ✅ + architecture.md（补 screenshot_commands 条目）
- 全量验证：embedded / cloud,vault / remote-ws / remote-grpc + test

---

## 验证 checklist
- [x] `cargo build -p octopus-desktop --features embedded` — 0 error 0 warning
- [x] `cargo build -p octopus-desktop --features embedded,cloud,vault` — 0 error 0 warning
- [x] `cargo test -p octopus-desktop` — 441 passed

## 回滚
每个 Task 独立 commit。失败 `git reset --hard HEAD~1`。
