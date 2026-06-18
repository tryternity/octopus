# 设置窗口设计（settings window）

- 日期：2026-06-17
- 状态：✅ 已实现（合并 main，见 plan Task 1–11 全部完成）
- 相关代码：`crates/desktop/src/settings_window.rs`（窗口 + `open_settings` + macOS Dock 图标）、`crates/desktop/src/settings_commands.rs`（`get_config` / `set_config` / `get_history` / `delete_history` / `check_shortcut`）、`crates/desktop/src/runtime_config.rs`（扩展）、`crates/desktop/dist/settings/index.html`（前端）
- 参考界面风格：用户提供 1.png / 2.png（左侧固定侧边栏 + 右侧主内容区，浅色主题，卡片式分块，非原生系统风格的自定义 Web 界面）

---

## 1. 背景与动机

当前 octopus desktop 的配置完全依赖手编 `~/.octopus/config.yaml`（25 字段）+ DB `models` 表。结果窗工具栏已支持运行时快捷切换（ASR 引擎 / 降噪 / 润色模式 / 润色模型 / 立即润色），但系统设置的第一个工具栏按钮仍是占位状态。用户需要一个完整的 GUI 设置界面，替代手编配置文件，降低使用门槛。

参考两张设计图（语音输入类工具的设置界面）：共同风格为**左侧固定侧边栏 + 右侧主内容区**，浅色主题，卡片式分块，图标+文字导航。octopus 的设置窗口将沿用此风格。

---

## 2. 功能范围

三个页面：

| # | 页面 | 内容 | 本轮状态 |
|---|---|---|---|
| 1 | 识别记录 | 当前识别实时文本（录音中）+ 历史记录浏览（transcriptions 表）：工具栏（全选 + 删除）、checkbox 批量选择、润色优先显示、单条拷贝 | ✅ 实现 |
| 2 | 系统设置 | config.yaml 19 个可配置字段的 GUI 编辑（分组卡片 + 实时保存） | ✅ 实现 |
| 3 | 模型管理 | 外部模型 API 配置 + 本地模型下载 | 🔴 占位（本轮不实现） |

---

## 3. 已确认决策（brainstorming 结论）

1. **前端技术**：纯 vanilla HTML（单 `index.html`，内联 CSS/JS），与 result_window 一致，无构建步骤、无 npm 依赖。
2. **入口**：工具栏设置按钮（第一个图标，现有占位）+ 系统托盘菜单新增"设置..."项，两者均调 `open_settings` 命令。
3. **窗口属性**：独立 Tauri 窗口，原生标题栏（`decorations: true`），默认 `800×600`，最小 `640×480`，可调整大小，非置顶，单例（已打开则 `set_focus`）。macOS 下采用 **动态激活策略**：启动/无设置窗时 `Accessory`（仅托盘，无 Dock 图标）；打开设置窗时切 `Regular`（Dock 图标出现）；关闭设置窗时切回 `Accessory`。
4. **保存语义**：实时保存——每个控件改动即时写 `config.yaml` + RuntimeConfig（如适用），无确认按钮。
5. **生效时机标注**：每个控件旁标注"立即"/"下次录音"/"重启"生效标签。
6. **跨平台**：macOS / Windows / Linux 三端 UI 一致。macOS 额外有动态激活策略（Dock 图标显隐），`#[cfg(target_os = "macos")]` 条件编译。
7. **后端命令**：通用读写命令（方案 A）——`get_config` + `set_config(key, value)` + `get_history` + `delete_history(ids)` + `open_settings`，最少样板代码。
8. **隐藏字段**：`denoise_enabled`（废弃，忽略不改代码）、`paste_method`/`write_to_clipboard`/`overlay_position`/`remote_url`/`grpc_endpoint`（暂未定，后续再加）。

---

## 4. 架构

### 4.1 文件布局

```
crates/desktop/
├── src/
│   ├── settings_window.rs   # 新建：窗口创建 + open_settings + macOS Dock 图标 / on_settings_closed
│   ├── settings_commands.rs # 新建：get_config / set_config / get_history / delete_history / check_shortcut
│   ├── runtime_config.rs    # 扩展：RuntimeConfig 新增字段 + 暴露 build_*_options_public 供 settings 复用
│   ├── tray.rs              # 修改：托盘菜单加"设置..."项
│   └── main.rs              # 修改：注册 4 个新命令 + 托盘事件
├── dist/
│   ├── result/index.html    # 现有（不动）
│   └── settings/index.html  # 新建：3 页面 vanilla HTML（单文件内联 CSS/JS + 图标）
```

### 4.2 窗口创建与 macOS 激活策略（`settings_window.rs`）

```rust
// 单例：已存在 → set_focus；不存在 → 创建
pub fn open_settings(app_handle: tauri::AppHandle) {
    if let Some(window) = app_handle.get_webview_window("settings_window") {
        let _ = window.set_focus();
        return;
    }
    // macOS: 打开设置窗口 → Dock 显示图标
    #[cfg(target_os = "macos")]
    app_handle.set_activation_policy(tauri::ActivationPolicy::Regular);

    let _ = WebviewWindowBuilder::new(&app_handle, "settings_window", ...)
        .title("Octopus 设置")
        .inner_size(800.0, 600.0)
        .min_inner_size(640.0, 480.0)
        .decorations(true)
        .visible(true)
        .build();
}

// macOS: 设置窗口关闭后回调 — 切回仅托盘模式
#[cfg(target_os = "macos")]
pub fn on_settings_closed(app_handle: &tauri::AppHandle) {
    app_handle.set_activation_policy(tauri::ActivationPolicy::Accessory);
}
```

`main.rs` 的 `app.run()` 回调监听 `RunEvent::WindowEvent { event: Destroyed, label: "settings_window" }`，触发 `on_settings_closed`。启动时 `main.rs` 直接 `app.set_activation_policy(Accessory)`。macOS 下 `open_settings` 还调 `set_dock_icon()`——release 裸二进制无 .app bundle，Tauri 仅 debug 自动设图标，故需用 `objc2` 手动 `setApplicationIconImage`（`include_bytes!` 内嵌 `icons/icon.png`）。

### 4.3 Tauri 命令

| 命令 | 签名 | 说明 |
|---|---|---|
| `open_settings` | `() -> ()` | 创建/聚焦设置窗口（单例） |
| `get_config` | `() -> Value` | 返回全量 AppConfig（19 个展示字段 JSON）+ ASR/LLM 模型列表（DB `models` 表）+ 系统麦克风设备列表（cpal 跨平台枚举） |
| `set_config` | `(key: String, value: Value) -> Result<(), String>` | 通用写：match key → 校验类型/范围 → 写 AppConfig + RuntimeConfig（如适用）+ 持久化 config.yaml。非法值返回 `Err` |
| `get_history` | `(limit: u32, offset: u32) -> Vec<TranscriptionRecord>` | 分页读 transcriptions 表（倒序） |
| `delete_history` | `(ids: Vec<i64>) -> Result<usize, String>` | 批量删除 transcriptions（IN 子句），返回删除行数 |
| `check_shortcut` | `(shortcut: String) -> Result<(), String>` | 检测快捷键是否被占用：尝试 `on_shortcut` 注册 → 立即 `unregister`，仅做检测不持久化 |

**`get_config` 返回结构：**
```json
{
  "config": {
    "asr_engine": "local:qwen3-asr:qwen3-asr-0.6B",
    "language": "auto",
    "asr_shortcut": "CmdOrCtrl+Shift+Space",
    "segment_silence": 400.0,
    "polish_mode": 0,
    "polish_interval": 5.0,
    "pause_polish_threshold_ms": 600,
    "polish_llm": "bigmodel:glm:glm-4-flashx",
    "asr_hardware_accelerated": false,
    "asr_correct": false,
    "output_simplified": true,
    "hide_toolbar": true,
    "denoise_mode": 1,
    "engine_mode": "embedded",
    "microphone": ""
  },
  "asr_engines": [
    {"name": "zipformer-small-ctc", "label": "本地:zipformer:zipformer-small-ctc", "current": false},
    ...
  ],
  "llm_models": [
    {"name": "glm-4-flashx", "label": "bigmodel:glm:glm-4-flashx", "current": true},
    ...
  ],
  "microphones": ["MacBook Pro 麦克风", "外接 USB 麦克风", ...]
}
```

**`set_config` 类型校验：**
```
match key {
    // 字符串枚举
    "engine_mode" => as_str() ∈ {"embedded","websocket","grpc"}
    "language" => as_str() ∈ {"auto","zh","en","ja","ko"}
    // u8 枚举
    "polish_mode" => as_u64() ∈ {0,1,2}
    "denoise_mode" => as_u64() ∈ {0,1,2}
    // bool
    "asr_hardware_accelerated" / "asr_correct" / "output_simplified" / "hide_toolbar" => as_bool()
    // f64 正数
    "segment_silence" / "polish_interval" => as_f64() > 0.0
    "pause_polish_threshold_ms" => as_f64() >= 600.0
    // string（自由）
    "asr_shortcut" / "edit_shortcut" / "microphone" => as_str()
    // string（裸 model_name → 构造 3-part spec）
    "asr_engine" => build_asr_engine_spec(as_str())
    "polish_llm" => build_polish_llm_spec(as_str())
    // 非法 key
    _ => Err("未知配置字段: {key}")
}
```

**RuntimeConfig 写入：** `set_config` 成功后，如字段属于 RuntimeConfig 镜像范围（asr_engine / polish_mode / polish_llm / denoise_mode / asr_correct / output_simplified / hide_toolbar），同步更新 RuntimeConfig。

### 4.4 生效时机分类

| 时机 | 字段 | 机制 |
|---|---|---|
| **立即** | polish_mode, denoise_mode, asr_correct, output_simplified, hide_toolbar, **asr_shortcut**（热重载：注销旧 + 注册新）, **edit_shortcut**（发 `config-changed` 事件，结果窗 `refreshActive` 重读 `toolbar_state.edit_shortcut`）, **polish_llm**（2026-06-18 改进：通过 `Command::UpdateRuntime` 同步到 coordinator config 快照，录音中改也立即生效） | 写 RuntimeConfig / 热重载 / `update_runtime`，即时生效 |
| **下次录音** | asr_engine, microphone, language, asr_hardware_accelerated, segment_silence, polish_interval, pause_polish_threshold_ms | 写 AppConfig 缓存，Coordinator Toggle 进入 Idle 时重读（asr_engine 需重建引擎实例） |
| **重启** | engine_mode | 需重启进程（引擎初始化等） |

---

## 5. 前端设计（`dist/settings/index.html`）

### 5.1 整体布局

```
┌─────────────┬──────────────────────────────────┐
│  Octopus    │                                  │
│             │                                  │
│  📋 识别记录 │         主内容区                  │
│  ⚙  系统设置 │    （随侧边栏切换）                 │
│  📦 模型管理 │                                  │
│             │                                  │
└─────────────┴──────────────────────────────────┘
  侧边栏 180px           剩余自适应
```

- **侧边栏**：固定 180px 宽，浅灰背景（`#f5f5f7`），三个导航项（图标 + 文字），当前项高亮蓝色。
- **主内容区**：白色背景，左侧边栏右有 1px 分割线，内容区可垂直滚动。
- **字体栈**：`-apple-system, "Segoe UI", "Noto Sans", sans-serif`（三端系统字体）。
- **配色**：主背景白 / 侧边栏浅灰 / 强调色蓝（`#007aff`）/ 文字深灰（`#1d1d1f`）/ 次要灰（`#86868b`）。

### 5.2 页面 1 — 识别记录

- **顶部区域**：当前正在识别的实时文本（若在录音中）。listen `update-result` 事件，显示当前 display_text。
- **工具栏**：全选 checkbox + 已选计数（"已选 N 项"）+ 删除按钮（红色边框，无选中时禁用）。全选 checkbox 支持 indeterminate 状态（部分选中时）。
- **历史列表**（倒序，最新在前）：每条记录：
  - 左侧 checkbox（选中后可批量删除）
  - 时间戳（`2026-06-17 14:30:25`）
  - **润色 text 优先显示**（`polished_text`，黑色主文本）；无润色则显示 `raw_text`
  - **原始 text 折叠隐藏**（`raw_text`，灰色次要文本）；点击"展开/折叠原始"切换
  - 元数据行：引擎名 + 润色状态 + 时长（`qwen3-asr · 已润色 · 3.2s`）
  - 右侧「拷贝」按钮：拷贝最终 text（润色优先，无润色拷贝原始）
- **删除流程**：选中记录 → 点击删除 → `confirm()` 确认 → `invoke('delete_history', { ids })` → 刷新列表（重置 offset）。
- **滚动加载**：初始加载 20 条（`get_history(20, 0)`），滚到底部加载下一页（`offset += 20`），空结果停止。

### 5.3 页面 2 — 系统设置

卡片顺序：交互 → 识别 → 润色 → 降噪 → 引擎模式。**全部无标题**（仅保留行内容）。每行控件后无独立 badge，生效时间作为灰色小字跟在 label 后面，加括号如「(立即)」「(下次录音)」「(重启)」。

**卡片「交互」（首位，无标题）：**
| 控件 | 类型 | 字段 | 生效 |
|---|---|---|---|
| 激活/关闭快捷键 | 快捷键捕获按钮（点击后捕获键盘组合，含冲突检测 `check_shortcut`） | `asr_shortcut` | 立即 |
| 编辑快捷键 | 快捷键捕获按钮（无冲突检测，仅结果窗内 keydown 判定） | `edit_shortcut` | 立即 |
| 工具栏自动隐藏 | toggle switch | `hide_toolbar` | 立即 |
| 麦克风设备 | 下拉（microphones 列表） | `microphone` | 下次录音 |

**卡片「识别」（无标题）：**
| 控件 | 类型 | 字段 | 生效 |
|---|---|---|---|
| 语言识别 | 下拉（auto/zh/en） | `language` | 下次录音 |
| ASR 引擎 | 下拉（asr_engines 列表） | `asr_engine` | 下次录音 |
| 硬件加速 | toggle switch | `asr_hardware_accelerated` | 下次录音 |
| ASR 纠错 | toggle switch | `asr_correct` | 立即 |
| 简繁输出 | toggle switch（true=简体） | `output_simplified` | 立即 |
| 句间停顿 | select（300/400/500/600ms） | `segment_silence` | 下次录音 |

**卡片「润色」（无标题）：**
| 控件 | 类型 | 字段 | 生效 |
|---|---|---|---|
| 润色模式 | 下拉（关闭/仅最终/中间+最终） | `polish_mode` | 立即 |
| 润色模型 | 下拉（llm_models 列表） | `polish_llm` | 立即（2026-06-18 改进，原「下次录音」） |
| 润色间隔 | 下拉（仅最后=0/每3~8秒） | `polish_interval` | 下次录音 |
| 润色停顿阈值 | 下拉（600/700/800/900/1000ms，>= 600） | `pause_polish_threshold_ms` | 下次录音 |

**卡片「降噪」（无标题）：**
| 控件 | 类型 | 字段 | 生效 |
|---|---|---|---|
| 降噪模式 | 下拉（无/轻度/深度） | `denoise_mode` | 立即 |

**卡片「引擎模式」（无标题）：**
| 控件 | 类型 | 字段 | 生效 |
|---|---|---|---|
| 引擎接入模式 | 下拉（embedded/websocket/grpc） | `engine_mode` | 重启 |

**控件交互：**
- 改动即调 `invoke('set_config', { key, value })`。
- 成功：控件保持新值，无额外提示（静默保存）。
- 失败：toast 提示错误信息，控件回退到旧值。
- 生效时间标签：跟在 label 文字后面的灰色小字括号，如「语言识别 (下次录音)」。
- **快捷键捕获**：点击按钮 → 显示「按下快捷键…（Esc 取消）」→ 捕获组合键（`Cmd` / `Ctrl` / `Alt` / `Shift` + 主键）→ 先调 `check_shortcut` 检测冲突 → 成功保存 + 热重载（注销旧快捷键 + 注册新快捷键），失败 toast 提示。

### 5.4 页面 3 — 模型管理

占位页面：居中显示"功能开发中，敬请期待"图标 + 文字。

---

## 6. 数据流

### 6.1 打开设置窗口
```
click[工具栏设置] 或 click[托盘"设置..."]
  → invoke('open_settings')
  → Rust: get_webview_window("settings_window")
     → 已存在: set_focus()
     → 不存在: WebviewWindowBuilder 创建
```

### 6.2 前端初始化
```
settings/index.html 加载完成
  → invoke('get_config') → 渲染系统设置页面的全部控件 + 填充当前值
  → invoke('get_history', {limit:20, offset:0}) → 渲染识别记录列表
```

### 6.3 修改配置
```
用户改动控件
  → invoke('set_config', {key, value})
  → Rust: 类型校验 → 写 AppConfig 字段 → 写 RuntimeConfig（如适用）
           → write_config_yaml() 写 config.yaml
           → 如为 asr_shortcut 字段：注销旧快捷键 + 注册新快捷键（热重载）
  → 成功: 控件保持新值
  → 失败: toast 错误 + 控件回退
```

**快捷键专用流程：**
```
用户点击快捷键按钮 → 进入捕获模式
  → 按下组合键（修饰键 + 主键）
  → invoke('check_shortcut', {shortcut})
     → Rust: 尝试 on_shortcut 注册 → 立即 unregister → 仅检测
     → 成功: 继续保存
     → 失败: toast「快捷键注册失败，可能被其他应用占用」+ 恢复原值
  → invoke('set_config', {key:'asr_shortcut', value})
     → Rust: 注销旧快捷键 + register_shortcut(新的)
```

### 6.4 历史记录翻页
```
历史列表滚到底部
  → invoke('get_history', {limit:20, offset: 当前数量})
  → 追加到列表尾部；空结果 → 标记已加载完，不再请求
```

---

## 7. 错误处理

| 场景 | 处理 |
|---|---|
| `set_config` 类型错误（如 bool 字段传字符串） | `Err("字段 X 需要 bool 类型")`，前端 toast |
| `set_config` 值越界（如 segment_silence ≤ 0） | `Err("segment_silence 必须大于 0")`，前端 toast |
| `set_config` 未知 key | `Err("未知配置字段: {key}")`，前端 toast |
| `pause_polish_threshold_ms` < 600 | `Err("pause_polish_threshold_ms 必须 >= 600（需大于句间停顿最大值）")`，前端 toast |
| config.yaml 写失败 | `warn` log + `Err("保存失败，本次仍生效，重启后回退")` |
| `get_history` DB 错误 | 返回空数组 + `warn` log |
| 设置窗口已打开再次 `open_settings` | `set_focus` 聚焦已有窗口，不重复创建 |

---

## 8. 跨平台

- 窗口创建：`WebviewWindowBuilder` 标准 API，三端一致（`decorations:true` 各平台自动渲染原生标题栏）。
- **macOS 动态激活策略**：启动时 `Accessory`（无 Dock 图标）；`open_settings` 切 `Regular`（Dock 图标出现）；窗口 Destroyed 事件触发 `on_settings_closed` 切回 `Accessory`。`#[cfg(target_os = "macos")]` 条件编译，Windows/Linux 无此逻辑。
- 麦克风列表：后端复用现有 infra 代码（cpal 跨平台枚举设备）。
- 字体栈：`-apple-system, "Segoe UI", "Noto Sans", sans-serif`。
- 图标：拷贝按钮内联 SVG（`copy.svg`），侧边栏导航用 CSS mask。

---

## 9. 测试

### Rust 单测
- `set_config` 类型校验：合法值通过 / 非法值返回 Err（覆盖 bool / f64 / u8 / 枚举 / string 各类型）。
- `set_config` 写盘：改单字段后其他字段保留（复用 `persist_config_override` 现有测试模式）。
- `set_config` 范围校验：`pause_polish_threshold_ms >= 600`、`segment_silence > 0`。
- `get_history` 分页：limit/offset 正确切片，offset 越界返回空。
- `delete_transcriptions` 批量删除：指定 id 删除 + 空 id 列表不报错（内部函数可直连 Connection 测试）。

### 手动 e2e
- 工具栏设置按钮 / 托盘菜单均能打开设置窗。
- 设置窗单例：重复打开不创建多窗口。
- 三个页面切换正常。
- 系统设置：改 polish_mode 立即生效（边录音边看润色行为变化）。
- 系统设置：改 asr_engine 后下次录音生效。
- 系统设置：非法值（如 pause_polish_threshold_ms=100）弹出 toast 错误。
- 识别记录：历史列表正确加载、滚动翻页。
- 识别记录：润色 text 在前、原始 text 在后（折叠）。
- 识别记录：checkbox 选择 + 全选 + 删除流程。
- 识别记录：拷贝按钮拷贝最终 text。
- 识别记录：录音中实时显示当前文本。
- 模型管理：显示占位页面。
- 跨平台验证（macOS / Windows / Linux）。

---

## 10. 非目标（YAGNI）

- **模型管理页面（页面 3）**：本轮仅占位，外部模型 API 配置 + 本地模型下载后续开发。
- **隐藏字段**（`denoise_enabled` / `paste_method` / `write_to_clipboard` / `overlay_position` / `remote_url` / `grpc_endpoint`）：不在设置界面展示，后续想清楚再加。
- **config.yaml 注释保留**：`serde_yaml` 整体序列化丢注释（与 result_window toolbar 的 `persist_config_override` 一致）。
- **设置搜索**：不做设置项搜索功能。
- **多语言**：仅中文 UI。
- **识别记录搜索/过滤**：不做，仅时间倒序浏览 + 分页。
- **识别记录批量导出**：仅支持删除，不支持批量导出。

---

## 11. 相关文件清单

| 文件 | 改动 |
|---|---|
| `crates/desktop/src/settings_window.rs` | **新建**：窗口创建 + `open_settings` 命令 + macOS `set_dock_icon` / `on_settings_closed` |
| `crates/desktop/src/settings_commands.rs` | **新建**：`get_config` / `set_config`（含 `apply_config_value` 类型校验 + `sync_runtime_config` + `write_config_yaml` + shortcut 热重载）/ `get_history` / `delete_history` / `check_shortcut` 命令（独立文件避免 `runtime_config.rs` 膨胀） |
| `crates/desktop/src/runtime_config.rs` | **修改**：RuntimeConfig 新增字段（asr_correct / output_simplified / hide_toolbar）+ 暴露 `build_asr_options_public` / `build_llm_options_public` 供 settings 复用 |
| `crates/desktop/src/tray.rs` | **修改**：托盘菜单新增"设置..."项 |
| `crates/desktop/src/main.rs` | **修改**：注册 6 个命令（`open_settings` / `get_config` / `set_config` / `get_history` / `delete_history` / `check_shortcut`）+ 设置窗口模块声明 + `Destroyed` 事件回调 |
| `crates/desktop/dist/settings/index.html` | **新建**：3 页面 vanilla HTML（单文件内联 CSS/JS + 图标） |
| `crates/desktop/tauri.conf.json` | **修改**：`frontendDist` 需包含 `settings/` 目录（或确认相对路径解析） |
| `docs/architecture.md` | 补充设置窗口子系统说明 |
| `docs/configuration.md` | 补注：config.yaml 字段现可经设置界面 GUI 编辑 |
