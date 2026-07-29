# desktop 重复代码清理 spec：e2s + create_window + reveal/open

> **Status: 🔨 实施中**（2026-07-29，分支 `daily_bugfix_0729`）
>
> **背景**：rust-patterns 扫描发现 desktop crate 三类重复代码。本 spec 是第一梯队 DRY 重构，纯机械去重，风险极低。

## 1. 范围

| 项 | 收益 | 风险 |
|---|---|---|
| **A. reveal/open 去重** | 7 处复制粘贴 → 统一 `sys_open` helper；**顺带修 3 处 macOS-only 硬编码的跨平台 bug** | 低（macOS 行为等价） |
| **B. e2s 错误转换推广** | ~178 处 `.map_err(\|e\| e.to_string())?` → 统一 `e2s`/`e2s_ctx` helper | 低（逐文件人眼判断） |
| **C. create_window 抽象** | 8-10 个透明浮动窗口的 5 参数 builder 链重复 → 统一 `build_float_window` | 中（保留 build 后副作用在调用方） |

## 2. 设计

### 2.1 项 A：reveal/open 去重（新建 `sys_open.rs`）

**现状**：`search_commands::reveal_path` 已实现跨平台三分支（macOS `open -R` / Windows `explorer /select,` / Linux `xdg-open parent`），但 6 处重复造轮子，其中 3 处是 macOS-only 硬编码（Windows/Linux 不可用）。

**新模块 `sys_open.rs`**（`pub(crate)`）：
- `reveal_path(path: impl AsRef<Path>) -> Result<(), String>`——从 search_commands 提取核心逻辑
- `reveal_path_lossy(path)`——失败仅 log 不返 Err（给 stop_and_store_inner 的「reveal 失败不影响录制」场景）
- `open_with_default(target: &str) -> Result<(), String>`——macOS `open` / Windows `cmd /c start ""` / Linux `xdg-open`，覆盖 open_url + open_path

**调用点改动（7 处）**：
| 调用点 | 改为 | 跨平台修复 |
|---|---|---|
| clipboard_commands::reveal_in_file_manager | reveal_path | — |
| record_commands::reveal_recording | reveal_path | ✅ Win/Linux |
| record_commands::reveal_subtitle | reveal_path | ✅ Win/Linux |
| record_commands::stop_and_store_inner | reveal_path_lossy | — |
| record_commands::open_recording_file | open_with_default | ✅ Win/Linux |
| action_bar_commands 内联 URL | open_with_default | — |
| clipboard_commands::open_file_item | open_with_default（抽 IO 部分） | — |

**search_commands 的 macOS-only `open_url`/`open_file`/`launch_app`**：保留薄包装（Tauri 命令签名不变），内部改调 sys_open。

### 2.2 项 B：e2s 推广（新建 `error_util.rs`）

**现状**：`record_commands.rs:25` 已有 `e2s` helper（泛型 `Display + Debug`，带 log），但只在本文件用（16 处）。其余 178 处 `.map_err(|e| e.to_string())?` 散布全 crate。

**新模块 `error_util.rs`**（`pub(crate)`）：
- `e2s<E: Display + Debug>(e: E) -> String`——保留 log 副作用，去 `[record]` 硬编码前缀
- `e2s_ctx<E: Display>(ctx: &str, e: E) -> String`——带上下文案（处理 40+ 处 `format!("xxx: {e}")`）

**替换规则（每处人眼判断）**：
- 纯 `|e| e.to_string()` → `e2s`（带 ? 的 `map_err(e2s)?`，tail expression 的 `map_err(e2s)`）
- `format!("上下文: {e}")` → `e2s_ctx("上下文", e)`
- **不动**：`ok_or_else`（构造错误，非转换）、`unwrap_or_else`（fallback 默认值）、字符串内容匹配（重试逻辑）

### 2.3 项 C：create_window 抽象（新建 `window_factory.rs`）

**现状**：8-10 个透明浮动窗口（action_bar/overlay/password_generator/record_config/result/clipboard/record_control/record_annotation/screenshot/record_area_picker）共享 5 参数 builder 链：`transparent(true)+decorations(false)+always_on_top(true)+skip_taskbar(true)+shadow(false)`。

**新模块 `window_factory.rs`**：
- `FloatWindowSpec` struct：label / url / title / inner_size / visible / resizable / position
- `build_float_window(app, spec) -> tauri::Result<WebviewWindow>`——封装 5 参数默认 + spec 参数，返回 WebviewWindow

**边界（只抽 builder 链）**：
- ✅ 抽：5 参数透明浮动默认 + label/url/title/尺寸/可见/位置
- ❌ 不抽：build 后副作用（on_window_event / poller / 激活策略 / dock 检测）——保留在各窗口的 create 函数，在 `build_float_window` 返回后接
- ❌ 不抽单例策略（3 种变体：return / destroy-rebuild / focus-return）——调用方预检查后再调 helper
- ❌ 不抽 decorations=true 窗口（compact_editor/download/settings，共性少）
- 平台中立（不用 cfg，record_* 的模块级 `#![cfg(macos)]` 不受影响）

## 3. 不变量
- 行为不变（除修复的 3 处跨平台 bug 变好）
- 所有 `#[tauri::command]` 签名不变（前端契约）
- capabilities/default.json 的 label 不变
- macOS 行为等价（reveal/open 在 macOS 路径不变）

## 4. 风险与降级
- **A**：reveal/open macOS 等价（`open -R`/`open` 不变），Win/Linux 新增支持（原不可用），无降级需要
- **B**：e2s 逐文件改，每批 cargo check 验证，出错回退该文件
- **C**：build 后副作用保留调用方，helper 只管 builder 链；手动冒烟验证窗口能创建
