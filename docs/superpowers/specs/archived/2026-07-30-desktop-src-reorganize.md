# crates/desktop/src 目录重组 spec（desktop crate 大文件重构 #6）

> **Status: ✅ 已实现**（2026-07-30，分支 `daily_refactor_record`）
> **前置**：大文件拆分 #1-#5 已完成（coordinator / action_bar / vault / screenshot / record）

## 背景

前五轮拆分把 5 个超大文件（共 9618 行）拆成了目录模块。但 `crates/desktop/src/` 下仍有 **70 个平铺 .rs 文件**（共 21488 行），只有 8 个子目录 mod。文件平铺导致：查找文件靠记忆而非目录导航；功能相关的文件散落各处（如 `vault_error.rs` 远离 `vault_commands/`）。

本轮把 70 个平铺文件全部归入功能域 mod 目录，`desktop/src/` 下除 `main.rs` 外不再有散落文件。

## 设计决策（brainstorming 确认）

1. **全部 70 个文件归组**——不留平铺
2. **多级 mod 组织**——功能域为顶级 mod，内部可有子 mod
3. **可重组现有 mod**——现有 8 个 mod 按需改名/扩展
4. **窗口跟随功能域**——`record_*_window.rs` 归 record/、`password_generator_window.rs` 归 vault/，不集中放 ui/
5. **ASR 全栈合并 engine/**——engine trait + 6 引擎实现 + cloud_pipeline + pipeline/transcript/audio + coordinator/
6. **独立命令文件统一 commands/**——model/settings/system_status/hotword/search/compact_editor/translation + builtin_models + model_migrate + compact_editor_window
7. **core/ 只放启动+基础设施**——不含功能模块
8. **剩余文件归已有域**——extensions→core/、subtitle_polish→record/、shortcut→core/

## 目标目录结构

```
crates/desktop/src/
├── main.rs                  # 仅入口（234 行，mod 声明改为 11 个顶级 mod）
│
├── core/                    # 启动 + 基础设施（10 文件）
│   ├── mod.rs
│   ├── setup.rs             # AppSetup（run() 拆分产物）
│   ├── bootstrap.rs         # bootstrap() -> AppConfig
│   ├── config.rs            # LLM 配置解析 / is_streaming_engine
│   ├── runtime_config.rs    # SharedRuntimeConfig + 工具栏命令
│   ├── db_queue.rs          # DB 写入 actor
│   ├── invoke_handler.rs    # generate_handler! 宏
│   ├── error_util.rs        # e2s / e2s_ctx
│   ├── perf_log.rs          # 性能打点
│   ├── file_watcher.rs      # notify-rs 文件监听
│   ├── shortcut.rs          # 全局快捷键注册（ASR toggle）
│   └── extensions.rs        # 扩展系统（导入/安装/刷新）
│
├── engine/                  # ASR 全栈（10 文件 + coordinator/ 子目录）
│   ├── mod.rs
│   ├── engine.rs            # TranscriptionEngine trait
│   ├── engine_embedded.rs   # 本地引擎
│   ├── engine_ws.rs         # WebSocket 远程
│   ├── engine_grpc.rs       # gRPC 远程
│   ├── engine_dispatch.rs   # cloud 路由分发
│   ├── engine_aliyun.rs     # 阿里云
│   ├── cloud_pipeline.rs    # 云端 ASR 流水线
│   ├── pipeline.rs          # ASR 流水线（VAD 分段 + 流式）
│   ├── transcript.rs        # 文本段模型
│   ├── audio.rs             # 音频采集
│   └── coordinator/         # 录音生命周期协调器（已有 mod.rs + 8 子模块）
│
├── record/                  # 录屏 + 截图（8 文件 + 2 子目录）
│   ├── mod.rs
│   ├── record_commands/     # 已有 mod.rs + 4 子模块
│   ├── screenshot_commands/ # 已有 mod.rs + 3 子模块
│   ├── record_area_picker.rs
│   ├── record_audio_probe.rs
│   ├── record_hotkey.rs
│   ├── record_window.rs
│   ├── record_annotation_window.rs
│   ├── record_control_window.rs
│   ├── screenshot_geometry.rs
│   └── subtitle_polish.rs   # 字幕 LLM 润色（与 postprocess 相关）
│
├── vault/                   # 密码库（5 文件 + 2 子目录）
│   ├── mod.rs
│   ├── vault_commands/      # 已有 mod.rs + 5 子模块
│   ├── autotype/            # 已有 4 文件
│   ├── vault_error.rs
│   ├── vault_state.rs
│   ├── vault_secret_access.rs
│   ├── vault_sync_commands.rs
│   └── password_generator_window.rs
│
├── clipboard/               # 剪贴板（4 文件）
│   ├── mod.rs
│   ├── clipboard_commands.rs
│   ├── clipboard_window.rs
│   ├── clipboard_queue.rs
│   └── clipboard_dock.rs
│
├── action_bar/              # 命令面板（4 文件 + 1 子目录）
│   ├── mod.rs
│   ├── action_bar_commands/ # 已有 mod.rs + 7 子模块
│   ├── action_bar_window.rs
│   ├── action_hotkey.rs
│   ├── agent_adapter.rs
│   └── terminal_launcher.rs
│
├── platform/                # 平台/输入辅助（7 文件 + 1 子目录）
│   ├── mod.rs
│   ├── app_context/         # 已有 5 文件（macos_ax/linux_atspi/windows_uia/ffi/sublime_plugin）
│   ├── input_source.rs
│   ├── finder_selection.rs
│   ├── keystroke.rs
│   ├── paste.rs
│   ├── sys_open.rs
│   ├── activation.rs
│   └── focus_tracker.rs
│
├── commands/                # 独立命令文件（10 文件）
│   ├── mod.rs
│   ├── model_commands.rs
│   ├── settings_commands.rs
│   ├── system_status_commands.rs
│   ├── hotword_commands.rs
│   ├── search_commands.rs
│   ├── compact_editor_commands.rs
│   ├── compact_editor_window.rs  # 跟随 compact_editor_commands
│   ├── translation_commands.rs
│   ├── builtin_models.rs
│   └── model_migrate.rs
│
└── ui/                      # 通用窗口 + UI 工具（11 文件）
    ├── mod.rs
    ├── result_window.rs
    ├── pin_window.rs
    ├── settings_window.rs
    ├── onboarding_window.rs
    ├── overlay_window.rs
    ├── download_window.rs
    ├── tray.rs
    ├── i18n.rs
    ├── theme.rs
    ├── window_factory.rs
    └── window_position.rs
```

**统计**：11 个顶级 mod + main.rs，70 个平铺文件全部归组。

## 拆分约束（不变量）

### 1. glob re-export 保持路径不变（核心）

每个 mod 的 `mod.rs` 用 `pub use submodule::*;` 或 `pub use <file_module>::*;` 全量 re-export。

**但这里有个关键差异**：前五轮拆分是把单文件拆成多文件再 re-export。这轮是把**已有的独立 mod 文件**（如 `vault_error.rs` 本身就是 `mod vault_error`）搬进目录。搬运后：

- 原路径 `crate::vault_error::xxx` → 新路径 `crate::vault::vault_error::xxx`
- 靠 `vault/mod.rs` 的 `pub use vault_error::*;` 保持 `crate::vault::xxx` 可访问
- 但原来的 `crate::vault_error::xxx` 路径会失效！

**应对**：main.rs 原来的 `mod vault_error;` 改为 `mod vault;`（vault/mod.rs 内 `mod vault_error; pub use vault_error::*;`）。所有 `crate::vault_error::xxx` 引用要改成 `crate::vault::xxx`（或 `crate::vault::vault_error::xxx`）。

这意味着**外部引用路径需要改**——与前五轮 glob re-export 零改动的模式不同。需要全局 grep 替换。

### 2. 现有子目录整体搬入

`coordinator/`、`record_commands/`、`screenshot_commands/`、`vault_commands/`、`autotype/`、`action_bar_commands/`、`app_context/` 这 7 个已有子目录整体 `git mv` 进对应的新顶级 mod，内部结构不变。

### 3. main.rs mod 声明重写

main.rs 的 ~97 行 mod 声明改为 11 行顶级 mod：
```rust
mod core;
mod engine;
mod record;
mod vault;
mod clipboard;
mod action_bar;
mod platform;
mod commands;
mod ui;
// feature-gated 保留：
#[cfg(feature = "vault")] mod vault;  // vault 整体 gate？
```

### 4. feature gate 处理

vault 域的 feature gate 需要特殊处理——`vault_commands/`、`vault_error.rs`、`vault_state.rs` 等都是 `#[cfg(feature = "vault")]`。搬进 `vault/` 后，gate 移到 `vault/mod.rs` 的子 mod 声明上。

cloud 域类似（`engine_aliyun.rs` / `cloud_pipeline.rs` 是 `#[cfg(feature = "cloud")]`）。

### 5. 逻辑完全不变

纯文件搬家 + 路径调整。不改函数逻辑/签名。

## 风险

| 风险 | 等级 | 应对 |
|---|---|---|
| 引用路径断裂（`crate::vault_error::` → `crate::vault::`） | 中 | 全局 grep 替换 + 编译验证 |
| feature gate 跨目录后不匹配 | 中 | 每步 build 验证 cloud/vault feature |
| mod 声明冲突（如 core 是 Rust 保留名？） | 低 | `core` 不是 Rust 2018 保留 mod 名，可用 |
| 搬运量大（70 文件 + 7 子目录） | 中 | 分域渐进，每域独立 commit + build |

### 路径迁移影响面量化

引用最多的平铺文件（搬运后需全局改 `crate::xxx::` → `crate::<domain>::xxx::`）：

| 被引用文件 | 引用文件数 | 迁移后路径 |
|---|---|---|
| error_util | 24 | `crate::core::error_util::` |
| config | 20 | `crate::core::config::` |
| tray | 12 | `crate::ui::tray::` |
| result_window | 12 | `crate::ui::result_window::` |
| runtime_config | 11 | `crate::core::runtime_config::` |
| transcript | 10 | `crate::engine::transcript::` |
| engine | 10 | `crate::engine::engine::`（或 `crate::engine::` 经 re-export） |
| activation | 10 | `crate::platform::activation::` |
| vault_state | 8 | `crate::vault::vault_state::` |
| window_factory | 7 | `crate::ui::window_factory::` |

**缓解策略**：每个域的 mod.rs 用 glob re-export（`pub use error_util::*`），这样引用方可选用 `crate::core::error_util::e2s`（精确）或 `crate::core::e2s`（经 re-export，更简洁）。推荐统一改成 `crate::core::e2s` 形式减少嵌套。

**替代方案（降低风险）**：如果路径迁移工作量/风险过大，可考虑只在每个域 mod.rs 加 `pub use <file>::*;`，然后 main.rs 额外加一层 `pub use core::*;` 等——但这样 main.rs 会膨胀，且 `crate::error_util::` 这种顶层路径仍需保留 alias。不推荐。

## 不做

- 不改函数逻辑/签名
- 不改 invoke_handler.rs 的命令注册（命令路径靠 re-export 保持，但 mod 路径要改）
- 不拆已有子目录内部结构
