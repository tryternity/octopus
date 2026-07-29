# record_commands.rs 拆分 spec（desktop crate 大文件重构 #5）

> **Status: ✅ 已实现**（2026-07-30，分支 `daily_refactor_record`，5 commit）

## 背景

`crates/desktop/src/record_commands.rs` 1628 行（含 37 行测试），是 desktop crate 当前第 2 大功能域文件。承载屏幕录制全功能：权限检测、录屏控制、录制库管理、ffmpeg 工具、后处理（GIF/音轨合并/字幕）。

前四个大文件拆分已完成（coordinator / action_bar_commands / vault_commands / screenshot_commands）。这是第 5 个。

## 现状结构分析

### 同 glob re-export 模式

32 个 invoke_handler 命令 + 8 处外部引用 → glob re-export 保持路径不变。

### macOS 独占

整个模块仅 macOS 编译（main.rs `#[cfg(target_os = "macos")] mod record_commands;`，模块内 `#![cfg(target_os = "macos")]`）。子模块继承这个 gate。

### 职责聚类（5 组 → 5 子模块 + mod.rs）

| 子模块 | 行数 | 内容 |
|---|---|---|
| `mod.rs`（留） | ~80 | mod 声明 + glob re-export + 共享 helper（`provider` / `now_iso` / `with_db_blocking`）+ list displays/windows/microphones |
| `permission.rs` | ~165 | check/request screen_record/microphone/accessibility permission + open_privacy_settings + probe_microphone_permission |
| `control.rs` | ~510 | RecordConfig + record_start/start_default/start_with_config/pause/resume/stop/kill + stop_and_store + build_default_config + parse_bool_config + resolve_mic_device_name + MetaFields + derive_fields_from_request（含 3 个测试） |
| `library.rs` | ~250 | ListRecordingsParams + list/get/thumbnail/rename/favorite/delete/open/reveal recordings + RecordStatus + get_record_status |
| `postprocess.rs` | ~620 | export_gif + MergeResult + RecordTaskEvent + merge_audio_tracks + generate_subtitle + read/reveal_subtitle + LlmOption + list_subtitle_llms + capitalize + ffmpeg helpers（probe/find/hint/check） |

## 目标目录结构

```
crates/desktop/src/record_commands/
├── mod.rs          # ~80 行：mod 声明 + glob re-export + 共享 helper + list_displays/windows/microphones
├── permission.rs   # 权限检测/请求
├── control.rs      # 录屏控制（start/stop/pause/resume/kill + config）
├── library.rs      # 录制库管理（CRUD + status）
└── postprocess.rs  # 后处理（GIF/合并/字幕 + ffmpeg 工具）
```

## 拆分约束（不变量）

1. **glob re-export**：`pub use submodule::*`，路径不变
2. **共享 helper 留 mod.rs**：`provider` / `now_iso` / `with_db_blocking` 被多个子模块用
3. **`#![cfg(target_os = "macos")]`**：每个子模块顶部加这个 gate（继承原模块的独占性）
4. **测试**：3 个 `resolve_mic_device_name` 测试搬到 control.rs
5. **逻辑完全不变**：纯代码搬家

## 风险
低。同模式。注意 ffmpeg helpers（probe_ffmpeg 被 setup.rs 外部引用 `pub(crate)`）经 glob re-export 保路径。

## 不做
- 不改函数逻辑/签名
- 不改 invoke_handler.rs / 外部引用路径

---

## 实施记录（2026-07-30）

### 最终目录结构

```
crates/desktop/src/record_commands/
├── mod.rs          # 100 行（共享 helper + 3 个 list_* + 4 个 glob re-export）
├── permission.rs   # 154 行（7 个权限命令 + probe_microphone_permission 私有）
├── library.rs      # 163 行（10 个录制库 CRUD + RecordStatus）
├── postprocess.rs  # 717 行（ffmpeg helpers + GIF + 音轨合并 + 字幕 + LLM 下拉）
└── control.rs      # 562 行（控制 + 配置 + 入库 + 3 个测试）
```

合计 1696 行（原 1628 行，增加来自各子模块头注释 + 共享 helper `use super::` 导入）。

### 关键约束兑现

- **glob re-export**：`pub use <module>::*`，32 个命令 + 8 处外部引用路径 `crate::record_commands::xxx` 全部不变（invoke_handler.rs / setup.rs / record_audio_probe.rs 等零改动）。
- **`#![cfg(target_os = "macos")]`**：每个子模块顶部都加了 gate（含 mod.rs 保留）。
- **共享 helper 留 mod.rs**：`provider` / `now_iso` / `with_db_blocking` 留在 mod.rs，4 个子模块用 `use super::{...}` 精确引用（不用 `use super::*` 避免 unused warning）。
- **测试**：3 个 `resolve_mic_device_name_*` 搬到 control.rs，路径变为 `record_commands::control::tests::*`。

### 偏差

- spec 预估 postprocess ~620 行，实际 717 行——原因是 ffmpeg helpers + LlmOption + capitalize 全部归 postprocess（spec 表格列了，估算偏紧）。无逻辑偏差。
- spec 预估 control ~510 行，实际 562 行——含共享 helper 导入注释。无逻辑偏差。
- RecordStatus / get_record_status 原在 ffmpeg helpers 与 export_gif 之间（mod.rs 物理位置），spec 归 library.rs。拆分时按 spec 归类（与录制状态查询而非后处理更贴合）。
- control.rs 多了一处 `use octopus_record::platform::HelperProvider` 显式 trait 导入——`provider().resolve_helper_path()` / `provider().list_displays()` 等方法调用需要 trait 在 scope（原 mod.rs 因顶部 use octopus_record 已带入，control.rs 单独 import）。

### 验证结果

- `cargo build -p octopus-desktop --features embedded,cloud,vault` — 0 error 0 warning ✅
- `cargo test -p octopus-desktop` — 441 passed / 0 failed / 1 ignored ✅
- 3 个 `resolve_mic_device_name_*` 测试在新路径 `record_commands::control::tests` 下运行正常（2 passed + 1 ignored）。
