# 桌面应用集成

> `octopus-desktop`——基于 Tauri 2 的桌面应用，系统集成核心。管理 5 类窗口、全局快捷键、系统托盘、macOS Dock 显隐策略、跨平台贴图窗口、平台特性适配。

源文件：`crates/desktop/src/`。

---

## 1. 窗口管理

| 窗口 | 用途 | 属性 |
|------|------|------|
| `result_window` | 识别结果展示 | 透明、无边框、置顶、可拖拽、多行滚动。720×480 物理固定，前端 CSS 切可见容器尺寸（默认 520×116 精简态 / 工具栏放大 720×480） |
| `settings_window` | 系统设置 | 原生标题栏、圆角、可调大小。五页面侧边栏：系统设置 / 识别记录 / 剪贴板 / 模型管理 / 提示词。窗口位置记忆 |
| `compact_editor_window` | 统一内容查看器 | 原生标题栏、880×620 可调 + 记忆、居中、min 400×320。多 tab（文本/图片/语音） |
| `clipboard_window` | 剪贴板历史浮窗 | 300×600，无边框圆角透明置顶，`clipboard_shortcut`（默认 CmdOrCtrl+Shift+D）唤起 |
| ~~`notepad_window`~~ | （已移除 2026-07-03） | 随 `octopus-notepad` crate 一并删除 |
| ~~`image_preview_window`~~ | （已移除 2026-07-04） | 合并入 CompactEditor 图片 tab |

**`open_settings` 支持初始页面参数**（`PENDING_PAGE` 暂存 + `get_initial_page` 拉取 + `settings://navigate` 事件），剪贴板浮窗「管理」按钮直接跳转剪贴板 tab。

---

## 2. macOS 动态激活策略（Dock 图标显隐）

应用启动即 `Accessory` 模式（无 Dock 图标，纯托盘应用）。

两个**常规窗口**（`settings_window` / `compact_editor_window`）任一打开时切 `Regular`。关窗 `Destroyed` 经 `activation.rs::restore_accessory_if_no_regular_window` 协调——**仅当常规窗口全无存活才切回 `Accessory`**，否则保持 `Regular`（避免 app 降级时 macOS 连带收掉其余常规窗口）。

设置窗口另经 `set_dock_icon()` 用 `objc2` 手动 `setApplicationIconImage`——release 裸二进制无 .app bundle，Tauri 仅 debug 自动设图标。

`#[cfg(target_os = "macos")]` 条件编译，Windows / Linux 无此逻辑。

---

## 3. 全局快捷键

| 配置键 | 默认 | 功能 |
|--------|------|------|
| `asr_shortcut` | — | 开始/停止录音（Toggle） |
| `clipboard_shortcut` | CmdOrCtrl+Shift+D | 唤起剪贴板浮窗（toggle 按焦点判断） |
| `screenshot_shortcut` | Alt+S | 截图 / 滚动截图模式 |
| `edit_shortcut` | CmdOrCtrl+Enter | 结果窗内 toggle 编辑（非全局，不在设置页管理） |
| `edit_global_shortcut` | CmdOrCtrl+Shift+E | 全局唤起结果窗 + toggle 编辑 |
| `polish_global_shortcut` | CmdOrCtrl+Shift+S | 全局唤起结果窗（不聚焦）+ 立即润色 |

**注册策略**：先注册后持久化——`unregister` 旧的 + `register` 新的，注册成功才写共享 `AppConfig` + `save_app_config`。**任一失败则恢复旧快捷键并返回 Err**（前端 toast 报冲突）。

**clipboard_window 唤起 toggle 逻辑**：失焦状态按快捷键直接 `show`+`set_focus` 激活，仅「可见且有焦点」才收起——避免 always-on-top 窗口失焦后仍 visible 导致需按两次。

---

## 4. 系统托盘

托盘菜单含「开始/停止」+「引擎: <model_name> (<mode>)」项 + 各功能入口。

- `tray::update_tray_engine_label` 实时刷新引擎项（`TRAY_ITEMS` 缓存 `engine_info` MenuItem handle，`set_text` 更新而非重建，规避 `MenuItem::with_id` 重复 ID panic）
- 识别状态变化时更新托盘图标/文字

---

## 5. clipboard_window（剪贴板历史浮窗）

- **300×600**，无边框圆角透明置顶
- 顶部标题栏：X + 「剪贴板」 + 右侧两 toggle（监听开关 CircleCheck/CircleX + Pin）+ `data-tauri-drag-region="deep"` + `cursor-grab`
- 搜索框 + 6 类过滤（全部/语音/文本/图片/文件/收藏，纯图标 tooltip）
- 列表：hairline 分隔线，ASR 条目左侧 voice 色条，图片条目内联 WebP 缩略图
- hover 显示操作按钮（编辑/预览/保存/OCR/打开/删除 + 收藏置末）
- **左侧类型图标单击即复制**（触效：icon 放大回弹 + 闪绿 + 「已复制」气泡 1.5s）
- 单击选中不关闭，双击写剪贴板 → 隐藏浮窗 → 模拟 Cmd+V 自动粘贴
- **操作栏容器 `onDoubleClick` 阻止冒泡**——连续快速点操作按钮的 dblclick 不冒泡到条目
- 窗口位置记忆

---

## 6. 一次性迁移

**`image_migration`**：`~/.octopus/clipboard_images/` → DB `image_data` BLOB。幂等（已存在的 hash 跳过），迁移成功后删除目录。启动时 `main.rs` setup 阶段调用。

---

## 7. 平台特性

### macOS

- **激活策略**：Accessory（无 Dock 图标）默认；Regular 当 settings/compact-editor 窗口打开（见 §2）
- **键盘模拟线程安全**：`paste_via_clipboard` 的 V 键用 `Key::Other(9)`（`kVK_ANSI_V`）而非 `Key::Unicode('v')`——enigo 0.6.1 的 `Key::Unicode` 走非线程安全的 Carbon `TIS*`/`UCKeyTranslate` API，在 `spawn_blocking` 非主线程执行会触发 SIGTRAP
- **贴图窗口**：自定义 `PinNSWindow`（`define_class!`）+ `NSTrackingArea` 检测 hover（见 [screenshot.md](./screenshot.md) §8）
- **屏幕录制权限**：`cargo run` 时授终端应用权限；打包 .app 后绑 octopus
- **窗口串行创建**（150ms 间隔）：WKWebView 同时创建多个全屏窗口会 segfault

### Windows

- **贴图窗口**：Win32 `WS_EX_TOPMOST|LAYERED|TOOLWINDOW` + `UpdateLayeredWindow`（见 [screenshot.md](./screenshot.md) §8）
- **剪贴板**：`ClipboardContext` 全局单例防锁竞争

### Linux

- **贴图窗口**：GTK3 Toplevel + Cairo 自绘（见 [screenshot.md](./screenshot.md) §8）
- **剪贴板**：X11 XFixes 事件驱动 / Wayland 两级轮询（MIME + text，500ms）

---

## 8. 设置窗口子系统

`settings_commands` + `settings_window`——独立 Tauri 窗口（原生标题栏、800×600 可调）。

**`set_config(key, value)`**（通用字段写入器）：
- `apply_config_value` 做字段类型/范围校验
- 快捷键先注册后持久化（见 §3）
- `edit_shortcut` / `hide_toolbar` / `clipboard_enabled` 改动发 `config-changed` 事件让结果窗 `refreshActive` 刷新 + 设置页/浮窗 `clipboard_enabled` toggle 双向同步

**系统设置页**（GeneralPanel.tsx）：6 张卡片——交互 / 模型选择 / 快捷键 / 降噪 / 粘贴 / OCR。

**模型管理页**（page 3）：4 个命令（`list_downloadable_models` / `download_model` / `verify_model` / `set_download_mirror`）。详见 [db-and-config.md](./db-and-config.md)。

**提示词页**（page 5）：6 个命令（`list_prompts` / `get_active_prompt` / `set_active_prompt` / `create_prompt` / `update_prompt` / `delete_prompt`），切换即时生效。

---

## 9. 引擎接入模式

支持三种引擎接入模式（cargo feature）：

| 模式 | feature | 说明 |
|------|---------|------|
| **embedded**（默认） | `embedded` | 内嵌 octopus-asr-local，本地推理 |
| **remote-ws** | `remote-ws` | 通过 WebSocket 连接远程 octopus-server |
| **remote-grpc** | `remote-grpc` | 通过 gRPC 连接远程推理服务 |
| **云引擎** | `cloud` | Aliyun/ByteDance/Tencent/Baidu WS 流式 |

**远程超时保护**：`WsRemoteEngine` / `GrpcRemoteEngine` / `AliyunEngine` 的 `transcribe` 均以 `tokio::time::timeout(8s)` 包裹（连接 + 收发全程），`health_check` 同样 `timeout(3s)`。超时返回 `Err`，经序列空洞修复的空串占位分支保证 `completed_seq` 连续推进。

`run-octopus.sh` 默认启用 `--features "embedded aliyun"`，否则云端引擎不可用。

---

## 10. 构建命令

```bash
# 构建桌面应用（embedded 模式，默认）
cargo run --release -p octopus-desktop --features embedded

# 构建桌面应用（含云端 ASR：阿里云/字节跳动/腾讯/百度）
cargo run --release -p octopus-desktop --features embedded,cloud

# 构建桌面应用（WebSocket 远程模式）
cargo run --release -p octopus-desktop --features remote-ws

# 构建桌面应用（gRPC 远程模式）
cargo run --release -p octopus-desktop --features remote-grpc

# 推荐（会清 WebView 缓存）
./run-octopus.sh
```

---

## 11. DB schema 变更策略

开发期简化：以 db.sql 为唯一真相，无历史迁移链。schema 变更直接改 db.sql + 升 user_version，删库重初始化。

`init_schema` 仅 `user_version < 18` 时执行 db.sql 建表+seed+yaml 导入（v18 跳过），v17→v18 跑 FTS5 backfill。

手编 `models` 表 / `app_config` 表需重启进程生效（`OnceLock` 缓存，运行中不可热更新；运行时修改走 `RuntimeConfig` + `persist_*`）。
