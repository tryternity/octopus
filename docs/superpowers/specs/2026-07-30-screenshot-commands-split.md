# screenshot_commands.rs 拆分 spec（desktop crate 大文件重构 #4）

> **Status: ✅ 已实现**（2026-07-29，分支 `daily_bugfix_0729`）

## 背景

`crates/desktop/src/screenshot_commands.rs` 1632 行（无测试模块），是 desktop crate 当前第 2 大功能域文件。承载截图全功能：区域截图（选区/确认/保存/OCR/二维码/pin）+ 滚动截图（长截图拼接）。

前三个大文件拆分已完成（coordinator / action_bar_commands / vault_commands）。这是第 4 个。

## 现状结构分析

### 同 glob re-export 模式

15 个 invoke_handler 命令 + 11 处外部引用 → glob re-export 保持路径不变。

### 两大功能域 + 共享平台 helper

| 域 | 行号范围 | 内容 |
|---|---|---|
| **区域截图** | L1-754 | FFI（cg_event_source）/ 截图窗口 / 选区确认 / 保存到历史 / OCR / 二维码扫描 / pin 浮窗 / shortcut 注册 |
| **滚动截图** | L755-1632 | ScrollStopMode / Guard / SendApp / Cocoa helper（save_frontmost_app / activate_prev_app / get_window_pid_at_point / activate_app_by_pid / get_window_number / set_window_ignores_mouse_events / set_app_active_on_main）+ `start_scroll_recording`（580 行）+ stop |

**关键**：`start_scroll_recording` 是 580 行巨函数，内部逻辑紧密耦合（选区定位→显示器检测→滚动循环→拼接），本次**纯搬家不拆函数体**。

### 共享 helper（被外部引用的 `pub(crate)`）

`get_primary_screen_height` / `get_window_cocoa_frame` / `active_display_for_point` 被 pin_window.rs 等外部用 → glob re-export 保路径。

## 职责聚类（3 子模块 + mod.rs）

| 子模块 | 行数 | 内容 |
|---|---|---|
| `mod.rs`（留） | ~40 | mod 声明 + glob re-export |
| `area.rs` | ~755 | 区域截图全流程：FFI / 窗口 / 选区 / 保存 / OCR / QR / pin / shortcut 注册 + `ScreenCaptureClone` struct |
| `scroll.rs` | ~880 | 滚动截图：`ScrollStopMode` / `ScrollRecordingGuard`+Drop / `SendApp` / `InteractiveRect` + 全部 Cocoa helper + `start_scroll_recording` + stop |
| `shared.rs` | ~50 | 跨域平台 helper：`get_primary_screen_height` / `get_window_cocoa_frame` / `active_display_for_point` / `format_file_size`（被 area + scroll + 外部共用） |

注：`shared.rs` 仅放真正跨 area/scroll 共用的；area 专属（`close_all_screenshot_windows`）留 area.rs，scroll 专属留 scroll.rs。

## 目标目录结构

```
crates/desktop/src/screenshot_commands/
├── mod.rs      # ~40 行：mod 声明 + glob re-export
├── shared.rs   # 跨域平台 helper
├── area.rs     # 区域截图
└── scroll.rs   # 滚动截图（含 start_scroll_recording 580 行巨函数）
```

## 拆分约束（不变量）

1. **glob re-export**：`pub use submodule::*`，路径不变
2. **不拆 `start_scroll_recording` 函数体**：580 行原样搬到 scroll.rs
3. **cfg gate 跟着搬**：macOS 专属函数（`get_window_number` / `set_window_ignores_mouse_events` 有双 cfg 版本）原样保留
4. **逻辑完全不变**：纯代码搬家
5. **共享 helper 判定**：被 area + scroll 都用的放 shared.rs；仅一方用的留各自模块

## 风险
低。同模式。唯一注意点是 `start_scroll_recording` 巨函数搬家时确保 import 完整（它引用大量 scroll helper + shared helper）。

## 不做
- 不拆 `start_scroll_recording` 内部逻辑（留给后续优化）
- 不改函数签名/逻辑

## 实施记录（2026-07-29）

四个 Task 全部完成，零 warning、441 测试全过。最终结构：

| 文件 | 行数 | 内容 |
|---|---|---|
| `mod.rs` | 15 | 仅子模块声明 + `pub(crate) use submodule::*` glob re-export |
| `shared.rs` | 57 | `format_file_size` + 3 个 `pub(crate)` macOS 平台 helper |
| `scroll.rs` | 905 | 滚动截图全部（含 `start_scroll_recording` 580 行巨函数原样搬） |
| `area.rs` | 721 | 区域截图全部 + 6 个截图静态量 + cg_event_source_ffi |

### 与原 plan 的偏差（已修正）

1. **`register_scroll_esc` / `unregister_scroll_esc` 归属**：原 plan 列在 area.rs（L100-135，
   仅按行号范围机械划分）。实际它们引用 `SCROLL_STOP_MODE` / `SCROLL_RECORDING` /
   `ScrollStopMode`（scroll 专属静态量），且只被 `start_scroll_recording` /
   `ScrollRecordingGuard` 调用——纯 scroll 域。归入 scroll.rs 消除跨模块静态依赖，
   比机械按行划分更内聚。

2. **re-export 可见性用 `pub(crate) use` 而非 `pub use`**：原 plan 写 `pub use submodule::*`。
   screenshot_commands 的 invoke_handler 命令是 `pub fn`（可 `pub use`），但跨子模块共享的
   平台 helper（`get_primary_screen_height` 等）是 `pub(crate)`——`pub use` 的 glob 不重导出
   `pub(crate)` 项会触发 "glob doesn't reexport anything" warning。改用 `pub(crate) use`：
   `pub` 命令与 `pub(crate)` helper 都能在 crate 内经 `crate::screenshot_commands::xxx` 访问
   （所有调用方都在 desktop crate 内）。

3. **跨子模块共享符号提升为 `pub(crate)`**（均为 crate 内可见，未泄露到 crate 外）：
   - `shared.rs`：`format_file_size`（原 `fn`，被 area + scroll 都用）→ `pub(crate)`
   - `scroll.rs`：`close_all_screenshot_windows`（area 的 confirm/cancel/pin/save 都调）、
     `save_frontmost_app`（area 的 start_screenshot 调）→ `pub(crate)`
   - `area.rs`：`right_mouse_button_down`（scroll 的右键取消轮询调）、
     `TOTAL_WINDOWS`（scroll 的 close_all_screenshot_windows 复位）→ `pub(crate)`

### 验证

- `cargo build -p octopus-desktop --features embedded` — 0 error 0 warning
- `cargo build -p octopus-desktop --features embedded,cloud,vault` — 0 error 0 warning
- `cargo test -p octopus-desktop` — 441 passed, 0 failed, 1 ignored
