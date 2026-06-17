# 设置窗口设计（settings window）

- 日期：2026-06-17
- 状态：📝 设计已确认，待实施
- 相关代码：`crates/desktop/src/settings_window.rs`（新建）、`crates/desktop/src/runtime_config.rs`（扩展）、`crates/desktop/dist/settings/index.html`（新建）
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
| 1 | 识别记录 | 当前识别实时文本（录音中）+ 历史记录浏览（transcriptions 表） | ✅ 实现 |
| 2 | 系统设置 | config.yaml 19 个可配置字段的 GUI 编辑（分组卡片 + 实时保存） | ✅ 实现 |
| 3 | 模型管理 | 外部模型 API 配置 + 本地模型下载 | 🔴 占位（本轮不实现） |

---

## 3. 已确认决策（brainstorming 结论）

1. **前端技术**：纯 vanilla HTML（单 `index.html`，内联 CSS/JS），与 result_window 一致，无构建步骤、无 npm 依赖。
2. **入口**：工具栏设置按钮（第一个图标，现有占位）+ 系统托盘菜单新增"设置..."项，两者均调 `open_settings` 命令。
3. **窗口属性**：独立 Tauri 窗口，原生标题栏（`decorations: true`），默认 `800×600`，最小 `640×480`，可调整大小，非置顶，单例（已打开则 `set_focus`）。
4. **保存语义**：实时保存——每个控件改动即时写 `config.yaml` + RuntimeConfig（如适用），无确认按钮。
5. **生效时机标注**：每个控件旁标注"立即"/"下次录音"/"重启"生效标签。
6. **跨平台**：macOS / Windows / Linux 三端一致，无平台条件编译。
7. **后端命令**：通用读写命令（方案 A）——`get_config` + `set_config(key, value)` + `get_history` + `open_settings`，最少样板代码。
8. **隐藏字段**：`denoise_enabled`（废弃，忽略不改代码）、`paste_method`/`write_to_clipboard`/`overlay_position`/`remote_url`/`grpc_endpoint`（暂未定，后续再加）。

---

## 4. 架构

### 4.1 文件布局

```
crates/desktop/
├── src/
│   ├── settings_window.rs   # 新建：窗口创建 + open/close/ready 机制（参考 result_window.rs）
│   ├── runtime_config.rs    # 扩展：set_config 通用写 + RuntimeConfig 新增字段
│   ├── tray.rs              # 修改：托盘菜单加"设置..."项
│   └── main.rs              # 修改：注册 4 个新命令 + 托盘事件
├── dist/
│   ├── result/index.html    # 现有（不动）
│   └── settings/index.html  # 新建：3 页面 vanilla HTML（单文件内联 CSS/JS + 图标）
```

### 4.2 窗口创建（`settings_window.rs`）

```rust
// 参考 result_window.rs 的 ready 机制（WINDOW_READY + PENDING_TEXT），
// 但设置窗无需 pending 暂存——打开时前端主动 invoke('get_config') 拉数据。
pub fn open_settings(app: &tauri::AppHandle) {
    // 单例：已存在 → set_focus；不存在 → 创建
    if let Some(window) = app.get_webview_window("settings_window") {
        let _ = window.set_focus();
        return;
    }
    let _ = tauri::WebviewWindowBuilder::new(
        app,
        "settings_window",
        tauri::WebviewUrl::App("settings/index.html".into()),
    )
    .title("Octopus 设置")
    .inner_size(800.0, 600.0)
    .min_inner_size(640.0, 480.0)
    .decorations(true)       // 原生标题栏
    .visible(true)
    .build();
}
```

### 4.3 Tauri 命令

| 命令 | 签名 | 说明 |
|---|---|---|
| `open_settings` | `() -> ()` | 创建/聚焦设置窗口（单例） |
| `get_config` | `() -> Value` | 返回全量 AppConfig（19 个展示字段 JSON）+ ASR/LLM 模型列表（DB `models` 表）+ 系统麦克风设备列表（cpal 跨平台枚举） |
| `set_config` | `(key: String, value: Value) -> Result<(), String>` | 通用写：match key → 校验类型/范围 → 写 AppConfig + RuntimeConfig（如适用）+ 持久化 config.yaml。非法值返回 `Err` |
| `get_history` | `(limit: u32, offset: u32) -> Vec<HistoryItem>` | 分页读 transcriptions 表 |

**`get_config` 返回结构：**
```json
{
  "config": {
    "asr_engine": "local:qwen3-asr-0.6B",
    "language": "auto",
    "shortcut": "CmdOrCtrl+Shift+Space",
    "segment_duration": 5.0,
    "segment_silence": 500.0,
    "segment_overlap": 200.0,
    "polish_mode": 0,
    "polish_interval": 5.0,
    "pause_polish_threshold_ms": 600,
    "polish_llm": "bigmodel:glm-4-flashx",
    "asr_hardware_accelerated": false,
    "asr_correct": false,
    "output_simplified": true,
    "hide_toolbar": true,
    "denoise_mode": 1,
    "engine_mode": "embedded",
    "microphone": ""
  },
  "asr_engines": [
    {"name": "zipformer-small-ctc", "label": "本地-zipformer-small-ctc", "current": false},
    ...
  ],
  "llm_models": [
    {"name": "glm-4-flashx", "label": "bigmodel-glm-4-flashx", "current": true},
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
    "segment_duration" / "segment_silence" / "segment_overlap" / "polish_interval" => as_f64() > 0.0
    "pause_polish_threshold_ms" => as_f64() > 500.0
    // string（自由）
    "shortcut" / "microphone" / "asr_engine" / "polish_llm" => as_str()
    // 非法 key
    _ => Err("未知配置字段: {key}")
}
```

**RuntimeConfig 写入：** `set_config` 成功后，如字段属于 RuntimeConfig 镜像范围（asr_engine / polish_mode / polish_llm / denoise_mode / asr_correct / output_simplified / hide_toolbar），同步更新 RuntimeConfig。

### 4.4 生效时机分类

| 时机 | 字段 | 机制 |
|---|---|---|
| **立即** | polish_mode, denoise_mode, asr_correct, output_simplified, hide_toolbar | 写 RuntimeConfig，Coordinator 下次读取时生效 |
| **下次录音** | asr_engine, polish_llm, microphone, language, asr_hardware_accelerated, segment_duration, segment_silence, segment_overlap, polish_interval, pause_polish_threshold_ms | 写 AppConfig 缓存，Coordinator Toggle 进入 Idle 时重读 |
| **重启** | shortcut, engine_mode | 需重启进程（全局快捷键注册等） |

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
- **历史列表**：按日期分组（如"6月17日"、"6月16日"），每条记录：
  - 时间戳（`14:30:25`）
  - 原文（`raw_text`）— 默认显示
  - 润色版（`polished_text`）— 点击展开/折叠
  - 元数据行：引擎名 + 润色状态 + 时长（`qwen3-asr · 已润色 · 3.2s`）
- **滚动加载**：初始加载 20 条（`get_history(20, 0)`），滚到底部加载下一页（`offset += 20`），空结果停止。

### 5.3 页面 2 — 系统设置

按功能分组为卡片，每张卡片有标题 + 若干控件行：

**卡片「识别」：**
| 控件 | 类型 | 字段 | 生效 |
|---|---|---|---|
| 语言 | 下拉（auto/zh/en/ja/ko） | `language` | 下次录音 |
| ASR 引擎 | 下拉（asr_engines 列表） | `asr_engine` | 下次录音 |
| 硬件加速 | toggle switch | `asr_hardware_accelerated` | 下次录音 |
| ASR 纠错 | toggle switch | `asr_correct` | 立即 |
| 简繁输出 | toggle switch（true=简体） | `output_simplified` | 立即 |

**卡片「润色」：**
| 控件 | 类型 | 字段 | 生效 |
|---|---|---|---|
| 润色模式 | 下拉（关闭/仅最终/中间+最终） | `polish_mode` | 立即 |
| 润色模型 | 下拉（llm_models 列表） | `polish_llm` | 下次录音 |
| 润色间隔 | number input（秒） | `polish_interval` | 下次录音 |
| 停顿润色阈值 | number input（毫秒，须>500） | `pause_polish_threshold_ms` | 下次录音 |

**卡片「降噪」：**
| 控件 | 类型 | 字段 | 生效 |
|---|---|---|---|
| 降噪模式 | 下拉（无/轻度/深度） | `denoise_mode` | 立即 |

**卡片「VAD 分段」：**
| 控件 | 类型 | 字段 | 生效 |
|---|---|---|---|
| 分段时长 | number input（秒） | `segment_duration` | 下次录音 |
| 静音阈值 | number input（毫秒） | `segment_silence` | 下次录音 |
| 分段重叠 | number input（毫秒） | `segment_overlap` | 下次录音 |

**卡片「音频」：**
| 控件 | 类型 | 字段 | 生效 |
|---|---|---|---|
| 麦克风设备 | 下拉（microphones 列表） | `microphone` | 下次录音 |

**卡片「交互」：**
| 控件 | 类型 | 字段 | 生效 |
|---|---|---|---|
| 全局快捷键 | text input | `shortcut` | 重启 |
| 工具栏自动隐藏 | toggle switch | `hide_toolbar` | 立即 |

**卡片「引擎模式」：**
| 控件 | 类型 | 字段 | 生效 |
|---|---|---|---|
| 引擎接入模式 | 下拉（embedded/websocket/grpc） | `engine_mode` | 重启 |

**控件交互：**
- 改动即调 `invoke('set_config', { key, value })`。
- 成功：控件保持新值，无额外提示（静默保存）。
- 失败：toast 提示错误信息，控件回退到旧值。
- 生效标签：每个控件右侧灰色小字标注"立即"/"下次录音"/"重启"。

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
           → persist_config_override(key, value) 写 config.yaml
  → 成功: 控件保持新值
  → 失败: toast 错误 + 控件回退
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
| `set_config` 值越界（如 segment_duration ≤ 0） | `Err("segment_duration 必须大于 0")`，前端 toast |
| `set_config` 未知 key | `Err("未知配置字段: {key}")`，前端 toast |
| `pause_polish_threshold_ms` ≤ 500 | `Err("须 > 500（Active Flush 阈值）")`，前端 toast |
| config.yaml 写失败 | `warn` log + `Err("保存失败，本次仍生效，重启后回退")` |
| `get_history` DB 错误 | 返回空数组 + `warn` log |
| 设置窗口已打开再次 `open_settings` | `set_focus` 聚焦已有窗口，不重复创建 |

---

## 8. 跨平台

- 窗口创建：`WebviewWindowBuilder` 标准 API，三端一致（`decorations:true` 各平台自动渲染原生标题栏）。
- 麦克风列表：后端复用现有 infra 代码（cpal 跨平台枚举设备）。
- 字体栈：`-apple-system, "Segoe UI", "Noto Sans", sans-serif`。
- 无平台条件编译（设置窗是纯 UI + Tauri 命令，不涉及平台 API）。
- 图标：复用 CSS mask 方式（同 result_window toolbar），Font Awesome SVG。

---

## 9. 测试

### Rust 单测
- `set_config` 类型校验：合法值通过 / 非法值返回 Err（覆盖 bool / f64 / u8 / 枚举 / string 各类型）。
- `set_config` 写盘：改单字段后其他字段保留（复用 `persist_config_override` 现有测试模式）。
- `set_config` 范围校验：`pause_polish_threshold_ms > 500`、`segment_duration > 0`。
- `get_history` 分页：limit/offset 正确切片，offset 越界返回空。

### 手动 e2e
- 工具栏设置按钮 / 托盘菜单均能打开设置窗。
- 设置窗单例：重复打开不创建多窗口。
- 三个页面切换正常。
- 系统设置：改 polish_mode 立即生效（边录音边看润色行为变化）。
- 系统设置：改 asr_engine 后下次录音生效。
- 系统设置：非法值（如 pause_polish_threshold_ms=100）弹出 toast 错误。
- 识别记录：历史列表正确加载、滚动翻页。
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
- **识别记录搜索/过滤**：本轮不做，仅时间倒序浏览 + 分页。

---

## 11. 相关文件清单

| 文件 | 改动 |
|---|---|
| `crates/desktop/src/settings_window.rs` | **新建**：窗口创建 + `open_settings` 命令 |
| `crates/desktop/src/runtime_config.rs` | **修改**：`set_config` 通用写命令 + `get_config` + `get_history` + RuntimeConfig 新增字段（asr_correct / output_simplified / hide_toolbar） |
| `crates/desktop/src/tray.rs` | **修改**：托盘菜单新增"设置..."项 |
| `crates/desktop/src/main.rs` | **修改**：注册 `open_settings` / `get_config` / `set_config` / `get_history` 命令 + 托盘事件处理 |
| `crates/desktop/dist/settings/index.html` | **新建**：3 页面 vanilla HTML（单文件内联 CSS/JS + 图标） |
| `crates/desktop/tauri.conf.json` | **修改**：`frontendDist` 需包含 `settings/` 目录（或确认相对路径解析） |
| `docs/architecture.md` | 补充设置窗口子系统说明 |
| `docs/configuration.md` | 补注：config.yaml 字段现可经设置界面 GUI 编辑 |
