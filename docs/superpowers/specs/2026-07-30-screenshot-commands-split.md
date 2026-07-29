# screenshot_commands.rs 拆分 spec（desktop crate 大文件重构 #4）

> **Status: 🔨 待实现**（2026-07-30，分支 `daily_refactor_screenshot`）

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
