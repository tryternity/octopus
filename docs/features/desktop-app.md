# 桌面应用集成

> `octopus-desktop`——基于 Tauri 2 的桌面应用，系统集成核心。管理 5 类窗口、全局快捷键、系统托盘、macOS Dock 显隐策略、跨平台贴图窗口、平台特性适配。

源文件：`crates/desktop/src/`。

---

## 1. 窗口管理

| 窗口 | 用途 | 属性 |
|------|------|------|
| `result_window` | 识别结果展示 | 透明、无边框、置顶、可拖拽、多行滚动。720×480 物理固定，前端 CSS 切可见容器尺寸（默认 520×116 精简态 / 工具栏放大 720×480） |
| `settings_window` | 系统设置 | 原生标题栏、圆角、可调大小。五页面侧边栏：系统设置 / 剪贴管理 / 模型管理 / 提示词 / 系统状态。窗口位置记忆 |
| `compact_editor_window` | 统一内容查看器 | 原生标题栏、880×620 可调 + 记忆、居中、min 400×320。多 tab（文本/图片/语音） |
| `clipboard_window` | 剪贴板历史浮窗 | 300×600，无边框圆角透明置顶，`clipboard_shortcut`（默认 Alt+C）唤起 |
| ~~`notepad_window`~~ | （已移除 2026-07-03） | 随 `octopus-notepad` crate 一并删除 |
| ~~`image_preview_window`~~ | （已移除 2026-07-04） | 合并入 CompactEditor 图片 tab |

**`open_settings` 支持初始页面参数**（`PENDING_PAGE` 暂存 + `get_initial_page` 拉取 + `settings://navigate` 事件），剪贴板浮窗「管理」按钮直接跳转剪贴板 tab。

### 1.1 窗口位置 / 最大化 / 多显示器记忆

两套并行实现（`window_position.rs` + `compact_editor_window.rs::WindowState`），都走 `save_config_key`/`load_config_key`（`category='system'`，与业务配置隔离）：

| 实现 | 服务窗口 | 记忆内容 | DB key |
|------|----------|----------|--------|
| **轻量位置**（`window_position.rs`） | `clipboard_window` / `result_window` | 仅 x,y | `window_pos.{label}` = `"x,y"` |
| **全状态**（`WindowState`） | `compact_editor_window` | w/h/x/y + **maximized** + 最后非最大化位置 | `compact_editor_window_state`（JSON）+ `compact_editor_last_normal_pos` |

轻量位置接口：`save_window_position` / `load_window_position` / `is_position_visible`（50px 容差判点）/ `restore_window_position(window, label, fallback)` / `save_current_position`（`outer_position()/scale`，event handler 用）。

**六大不变量**（十二次迭代确立，违反即窗口错位/丢失）：
1. **scale 转换**：`Monitor::position()` 与 `size()` 均为**物理像素**，越界检测时 position 和 size 都必须 `÷ scale_factor` 统一逻辑（旧版只除 size 不除 position → Retina 副屏坐标误判进主屏，`c3efb0c`）。
2. **50px 容差越界检测**（`is_position_visible`）：副屏拔接后保存的绝对坐标失效时走 fallback 居中/默认，避免窗口「消失」到不存在的屏。
3. **`builder.maximized(true)` 在 WRY 不生效**——必须 `visible(false)` 建**接近全屏的大窗体**（屏尺寸减 margin 80）→ `show()` → `maximize()`；不能直接用主屏尺寸创建（`is_maximized=false` 会让关窗保存错误状态）。
4. **最大化保存真实位置**：关窗时先 `unmaximize()` → 读 `inner_position()`+`inner_size()`（真实非最大化位置）→ `re-maximize()` → 存 `last_normal_pos`；不能直接读最大化时的 `inner_position()`（返回全屏位置可能跨屏到主屏原点）。
5. **副屏未连接三层 fallback**：按 `last_normal_pos` 匹配已连接显示器 → 命中则该屏大窗体+maximize；未命中→ `primary_monitor` 大窗体+maximize；连主屏都拿不到→默认 880×620 居中。
6. **`inner_position` + `inner_size` 对称**（均内容区坐标、不含标题栏），均 `÷ scale` 存逻辑像素；`scale_factor` 获取失败 `unwrap_or(1.0)` 兜底。

> 全状态实现的窗口尺寸/最大化也记录于 [compact-editor.md](./compact-editor.md) §1。

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
| `asr_shortcut` | OptRight | 单键三模式触发（长按=PTT / 双击=toggle / 短按=hands-free） |
| `clipboard_shortcut` | Alt+C | 唤起剪贴板浮窗（toggle 按焦点判断） |
| `screenshot_shortcut` | CmdOrCtrl+Shift+X | 截图 / 滚动截图模式 |
| `edit_shortcut` | CmdOrCtrl+Enter | 结果窗内 toggle 编辑（非全局，不在设置页管理） |
| `edit_global_shortcut` | Alt+E | 全局唤起结果窗 + toggle 编辑 |

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
- 列表：hairline 分隔线，ASR 条目左侧 voice 色条，图片条目内联缩略图
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
- **粘贴前输入源切换**（`switch_input_source_on_paste` 默认 `true`）：CJK 输入法 composing 状态下模拟 Cmd+V 可能乱码/丢字符，三段式注入根治——(1) 切到 ABC → (2) 模拟 Cmd+V → (3) 恢复原输入源。实现 `input_source.rs` 用 `osascript -l JavaScript`（JXA）独立进程调 Carbon TIS API。
  - **v1→v2→v3 演进**：v1 直接 FFI → SIGTRAP；v2 GCD `dispatch_sync_f` → 仍 SIGTRAP（tokio `spawn_blocking` 与 libdispatch main queue 冲突）；**v3 独立 osascript 进程**（main thread 天然满足 TIS 要求）= 当前。
  - **RAII `InputSourceGuard`**：已是 ABC/US 时跳过（省 fork）；`Drop` 用存的 source **ID（String）**恢复（非 CFTypeRef，跨进程安全）。两条接入路径：ASR 粘贴（`paste.rs`）+ 剪贴板双击粘贴（`focus_tracker.rs`）。
- **自动粘贴焦点追踪（已暂缓）**：`focus_tracker.rs` 尝试在剪贴板浮窗失焦后自动将选中文本粘贴到前一个前台 app——7 个已记录的 macOS 陷阱（Accessory `hide()` 不还焦点 / enigo 非主线程静默失败 / `activate` by name 不可靠 `-1728` / WeChat 阻止 AppleScript）。最终结论：无单一方案覆盖所有 app，**已回退**为复制到剪贴板 + 手动 Cmd+V。重启条件：NSPanel（tauri-nspanel）或 CGEvent 直接注入。

### Windows

- **贴图窗口**：Win32 `WS_EX_TOPMOST|LAYERED|TOOLWINDOW` + `UpdateLayeredWindow`（见 [screenshot.md](./screenshot.md) §8）
- **剪贴板**：`ClipboardContext` 全局单例防锁竞争

### Linux

- **贴图窗口**：GTK3 Toplevel + Cairo 自绘（见 [screenshot.md](./screenshot.md) §8）
- **剪贴板**：X11 XFixes 事件驱动 / Wayland 两级轮询（MIME + text，500ms）

---

## 8. 设置窗口子系统

`settings_commands` + `settings_window`——独立 Tauri 窗口（原生标题栏、960×600 可调）。

**`set_config(key, value)`**（通用字段写入器）：
- `apply_config_value` 做字段类型/范围校验
- 快捷键先注册后持久化（见 §3）
- `edit_shortcut` / `hide_toolbar` / `clipboard_enabled` 改动发 `config-changed` 事件让结果窗 `refreshActive` 刷新 + 设置页/浮窗 `clipboard_enabled` toggle 双向同步

**系统设置页**（GeneralPanel.tsx）：6 张卡片——交互 / 模型选择 / 快捷键 / 降噪 / 粘贴 / OCR。

**模型管理页**（page 3）：4 个命令（`list_downloadable_models` / `download_model` / `verify_model` / `set_download_mirror`）。详见 [db-and-config.md](./db-and-config.md)。

**提示词页**（page 5）：6 个命令（`list_prompts` / `get_active_prompt` / `set_active_prompt` / `create_prompt` / `update_prompt` / `delete_prompt`），切换即时生效。

**剪贴管理页**（page 2）：剪贴板历史的完整管理视图（FilterTabs + 多选 + 全选 sticky header + `ClipboardRow` 两行布局），与浮窗共用 `types/clipboard.ts` 工具（`detectUrl` / `fileMeta` / `metaParts`）。详见 [clipboard.md](./clipboard.md)。

**系统状态页**（page 6）：实时进程内存/CPU + 各本地模型估算内存 + sparkline 趋势。详见 §13。

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
# 构建桌面应用（embedded 模式，默认）—— 生产构建用 --profile optimize
cargo run --profile optimize -p octopus-desktop --features embedded

# 构建桌面应用（含云端 ASR：阿里云/字节跳动/腾讯/百度）
cargo run --profile optimize -p octopus-desktop --features embedded,cloud

# 构建桌面应用（WebSocket 远程模式）
cargo run --profile optimize -p octopus-desktop --features remote-ws

# 构建桌面应用（gRPC 远程模式）
cargo run --profile optimize -p octopus-desktop --features remote-grpc

# 推荐（会清 WebView 缓存）
./run-octopus.sh
```

### 10.1 macOS DMG 打包

首次建立打包链路（2026-07-23）。产出未签名 `.dmg` + `.app`（自用/内测）。

```bash
# 生产级 DMG（LTO+strip，体积小）
./scripts/build-macos-dmg.sh

# 调试打包流程（无 LTO，链接快）
./scripts/build-macos-dmg.sh --no-lto

# 构建完冒烟测试
./scripts/build-macos-dmg.sh --open
```

**产物**：
- `.app`：`target/<profile>/bundle/macos/octopus.app`
- `.dmg`：`target/<profile>/bundle/dmg/octopus_<version>_<arch>.dmg`（UDBZ bzip2，~40MB）

**feature 组合**：`embedded,cloud,vault,custom-protocol`（`custom-protocol` 生产 build 必须启用，走 `frontendDist` 嵌入 dist 而非 `devUrl`）。

**打包意义**：此前是「裸二进制 cargo run」，系统权限（屏幕录制/辅助功能/麦克风）绑定 Terminal；打包后绑定 octopus 本身，用户授权更清晰。关键决策（dmg 不走 Tauri create-dmg fork / beforeBuildCommand 设 null / resources 用对象形式 / `seeds_dir()` 三路解析含 .app bundle）详见 [architecture.md §打包/分发](../architecture.md) + [plan](../superpowers/plans/archived/2026-07-23-macos-dmg-packaging.md)。

---

## 11. DB schema 变更策略

开发期简化：以 db.sql 为唯一真相，无历史迁移链。schema 变更直接改 db.sql + 升 user_version，删库重初始化。

`init_schema` 仅 `user_version < 18` 时执行 db.sql 建表+seed+yaml 导入（v18 跳过），v17→v18 跑 FTS5 backfill。

手编 `models` 表 / `app_config` 表需重启进程生效（`OnceLock` 缓存，运行中不可热更新；运行时修改走 `RuntimeConfig` + `persist_*`）。

---

## 12. AI 命令面板（action_bar_window）

> 选中文本 → 热键 → 模拟 Cmd+C → 弹出迷你浮窗 → AI/搜索/翻译/网页 → CompactEditor 展示结果。源文件：`crates/desktop/src/action_bar_commands.rs`、`crates/desktop/frontend/src/pages/ActionBar/`。**仅 macOS**（依赖 CGEvent 鼠标坐标 + osascript 模拟 Cmd+C），非 macOS `trigger_action_bar` 直接 return + warn。

**窗口**：`action_bar_window` 透明浮窗，定位鼠标正上方 X 居中；两级菜单（主菜单 + 子菜单）+ 键盘导航（↑↓ 切行、←→/Enter 进子菜单）；高度 82px（主菜单 38 + 子菜单 38 + 边框），缩小透明点击区。

**关键约束（反复踩坑）**：
- **物理/逻辑坐标**：`CGEvent::location()` 返回**逻辑坐标（points）**，Tauri `LogicalPosition` 是逻辑像素，两者一致**不除 scale**；`Monitor::position()/size()` 返回物理像素**需除 scale**。曾误把 CGEvent 当物理坐标除 scale → 浮窗偏到无关位置。
- **trigger 后台线程**：模拟 Cmd+C 后 200ms sleep 不能在主线程（阻塞事件循环 → 窗口无焦点 → Esc/按钮无响应），必须 `std::thread::spawn`。
- **NSWindow 操作必须在主线程**：`show_action_bar_window` 用 `app.run_on_main_thread()`，不能用 `tauri::async_runtime::spawn`（tokio worker 线程）。
- **capabilities 白名单**：`action_bar_window` 必须在 `capabilities/default.json` 的 `windows` 数组里，否则 listen/invoke 全被 ACL 静默拦。
- **mousedown capture 陷阱**：外部点击检测用 `click` 事件冒泡阶段（`false`），capture 模式会在 onClick 前触发拦截按钮点击。
- **system_prompt 全局污染**：`run_ai_action` 不用 `set_system_prompt`/`polish`（会污染并发 ASR 润色），改用 `octopus_llm::chat_text_with_prompt(system, user, config)` 参数注入。
- **剪贴板生命周期**：trigger 阶段 suppress_next → Cmd+C → 读选中 → 立即 write_text 恢复（选中文本不入库），存 `PENDING_CONTEXT`。
- **trigger 重入 guard**：`TRIGGER_IN_PROGRESS: AtomicBool` 防热键连按，`finalize_action_bar` 统一收口。
- **AI 结果不做 Run And Paste**（浏览器安全策略阻止模拟粘贴），改 CompactEditor isTemp 临时 tab 展示（不写 DB）。AI 超时：翻译 5s + 润色/摘要/解释 10s，前端 `timedOutRef` 丢弃超时后到达的结果。

---

## 13. 系统状态页（model_probe 依赖反转）

> 设置窗「系统状态」tab，实时展示 octopus 进程内存/CPU + 各本地模型估算内存 + 短时趋势（sparkline）。源文件：`crates/infra/src/model_probe.rs`、`crates/desktop/src/system_status_commands.rs`、`crates/desktop/frontend/src/pages/Settings/SystemPanel.tsx`。

**依赖反转架构**：infra 不依赖 sysinfo/desktop，只持有闭包。
- `infra/model_probe.rs`：全局 `set_probe(ProbeFn)` + `probe(LoadPhase, id)`（`LoadPhase::Before/After/Unload`），未注入时 no-op。
- 加载点埋点：asr-local（`load_engine_into_cache` / `SileroVad::new`）、ocr（`OcrEngine::instance`）、流式（`StreamingSessionManager::switch_model`）在加载前后调 `probe`，id 形如 `asr:<name>` / `ocr:<name>` / `vad:silero`。
- desktop 启动注入 probe 闭包：Before 存 RSS、After 优先复用 `estimated` 首次值、否则算 RSS 差写 `ModelMemoryRegistry`；Unload 清 active 条目。

**`SystemStatusSampler`**：tokio 后台每 2s（`SAMPLE_INTERVAL_SECS`）用 sysinfo 采样 octopus 进程 RSS/CPU + 系统级内存/CPU，写入容量 60 的 ring buffer（2 分钟窗口），`emit("system-status", snapshot)`。`get_system_status` 命令返回当前完整快照（前端首屏 invoke）。

**关键踩坑**：
- **RSS vs phys_footprint 双指标**：RSS（sysinfo `resident_size`）含 mmap 的 file-backed 模型权重，比活动监视器「内存」（`phys_footprint`）长期高 ~450M。macOS 用 `proc_pid_rusage` FFI 读 `ri_phys_footprint`（flavor `RUSAGE_INFO_V0=0` 非 16；字节偏移 72）→ `ProcessStats.real_bytes`；非 macOS 返回 None 退 RSS。
- **模型内存「估算」**：同进程 ort 无法 OS 级 per-model 拆分，用加载前后 RSS 差值近似；**仅首次记录不覆盖**（ort arena 复用致后续差值偏低/为负）。`estimated` 首次值持久缓存，跨 unload/reload 保留。
- **probe race ThreadId 隔离**：多线程并发加载同一未缓存模型时 `before_map` key 用 `(ThreadId, String)`，Before/After 同线程配对。
- **probe 持锁调用户闭包**：clone `Option<ProbeFn>`（Arc+1）释放锁后再调 f，避免 sysinfo 扫全部进程慢阻塞其他线程。
- **OCR idle 60s 自动释放联动**：OCR `probe(Unload)` → `registry.remove(id)`，状态页 OCR 条目消失。释放后进程内存数值不立即下降（macOS libmalloc 不主动 munmap），真实收益是「下次重载复用 free list」——详见 [ocr.md](./ocr.md) §3.1-§3.2。

---

## 14. 命令面板菜单（action_bar_items DB）

> AI 命令面板菜单项存储在 `action_bar_items` 表（自引用 `parent_id` 两级菜单，5 种 `action_type`：`submenu`/`ai`/`url`/`script`/`copy`）。

**关键约束**：
- `#[serde(rename_all = "camelCase")]` **必须**——`parent_id`→`parentId` 不匹配会导致菜单渲染完全失败（曾踩坑）。
- 系统内置 seed 用 `INSERT OR IGNORE` + 固定 id；新增无固定 id 的 seed 须放在所有固定 id seed 之后，避免 AUTOINCREMENT 抢占 id。
- 无固定 id 的 seed 用 `WHERE NOT EXISTS` 按 title 去重。

**脚本执行设计**：
- `run_script` 拆为 `spawn_script` + `wait_with_timeout` 共享逻辑；同步模式 60s 强制 kill，异步模式无超时。
- stdout/stderr 截断 64KB（`chars().take(65536)`）防 DB 膨胀。
- `write_output_to_clipboard` 仅同步模式可用；异步 UI 强制 false。
- Electron app（如豆包）用 `do shell script "open -a"` 启动，`activate`/`launch` 不可靠。

**扩展包格式**：
- `.octopusext` 文件夹 = package；`config.yaml` 声明元数据 + action + skill。
- `action_data` 存脚本**绝对路径**（前导 `/` 区分内联脚本）；运行时设 `OCTOPUS_PACKAGE_DIR` 环境变量。
- 同名文件夹覆盖升级（不新建 DB 记录）；元数据实时从 config.yaml 读，不存 DB。

**环境变量模板**：
- 3 个内置变量（`huggingface`/`modelscope`/`github`，key 不可改、值可改）+ 用户自定义。
- DB `app_config` 表 `category='env'`，与普通 config 同表隔离。
- 模型下载 URL 中 `{huggingface}` 等占位符运行时替换为实际值（仅 ASR 模型下载，LLM/OCR source URL 不替换）。

**App-aware 菜单绑定**（2026-07-23，v49）：
- `action_bar_items` 加 `app_bundle_ids` 列（JSON 数组，如 `["com.tencent.xinWeChat"]`）；`launcher_index` 加 `bundle_id` 列。
- 语义：`app_bundle_ids` 为空 = 全局项（所有 app 都显示）；非空 = 专属项（仅前台 app 的 bundle_id 在数组中才显示）。前端 `isItemVisibleForApp` 按 AND 匹配。
- AppPicker 组件：`list_all_apps` 返回 `AppBrief { name, bundle_id, icon }`，多选器勾选绑定。详见 [spec](../superpowers/specs/archived/2026-07-23-actionbar-app-aware.md)。

**Prompt 外部文件引用**（2026-07-23）：
- agent/ai 类型菜单的 `action_data` 支持 `@文件名` 语法引用 `~/.octopus/.sync/prompts/command/<文件名>.md`（运行时 `resolve_prompt_reference` 展开）。
- 前端 PromptEditor 组件：Segmented 切换「内联」/「引用文件」模式；引用模式用可编辑 input + datalist（可选已有 + 自由输入新名）+ Plus 创建 + hover 浮层预览。详见 [spec](../superpowers/specs/archived/2026-07-23-prompt-file-reference.md)。

---

## 15. 窗口焦点协调（FLOAT_DEPTH 引用计数）

> 全局快捷键不得将 settings/compact_editor 带到前台。macOS `set_focus` 触发 `NSApp.activate` 会把所有可见 Regular 窗口带到前台。

**策略**：
- show 前：记录前台 app + app 非活跃时临时隐藏 Regular 窗口（`WINDOWS_TO_HIDE_ON_FLOAT`）。
- `set_focus` 激活浮窗获得键盘焦点。
- hide 时：先 `activate` 原前台 app 交还焦点，再 `show` 恢复被隐藏的 Regular 窗口。
- `FLOAT_DEPTH` 引用计数支持多浮窗嵌套——只有最外层 depth==1 才记录/交还焦点。
- `action_bar_show_result` 调 `after_floating_window_hide_keep_active`（跳过 deactivate 避免 CompactEditor 被压后台）。
- 剪贴板失焦 = 虚拟关闭：`float_depth_decrement_and_is_zero` 扣减，depth>0 时直接 return。
- `TRIGGER_IN_PROGRESS: AtomicBool` 重入 guard 防重复触发。

**NSWindow 主线程约束**：
- `setIgnoresMouseEvents` / `set_position` / `set_size` / `show` 等 NSWindow 操作必须在主线程（`run_on_main_thread`）。
- `setSize` 在 `transparent + decorations(false)` 悬浮窗上被 NSWindow 拒绝——改用 CSS 伪装 + 点击穿透。
