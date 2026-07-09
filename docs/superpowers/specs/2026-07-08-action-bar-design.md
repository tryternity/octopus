# AI 命令面板设计 — 选中文本→热键→AI 处理→替换

> **状态**：已实现，审查修复 9 项（P0/P1/P2）已完成
> **日期**：2026-07-08（审查修复：2026-07-09）
> **scope**：新建 `action_bar_window` 迷你浮窗，选中→热键→AI/搜索/翻译/打开网页
> **调研依据**：[`2026-07-08-popclip-survey.md`](./2026-07-08-popclip-survey.md)（PopClip/SnipDo/OnText/Click to Do 调研）

---

## 1. 背景与动机

### 1.1 核心场景

用户在任意 app（浏览器/编辑器/邮件/文档）中选中文本后，按全局热键 → octopus 模拟 `Cmd+C` 拿到选中文本 → 弹出迷你动作栏（类似 PopClip）→ 选 AI 动作 → LLM 处理 → 结果**原位替换**选中文本（Run And Paste）。

### 1.2 对标

| 产品 | 触发 | AI | 替换文本 |
|------|------|-----|---------|
| PopClip | 自动弹出（鼠标选中文本） | 靠扩展 | 无（复制到剪贴板） |
| OnText | 热键（F2） | 内置 ChatGPT/Claude/Ollama | `⌘R` 替换 |
| Click to Do | Win+点击 | 内置 Phi Silica（NPU） | 无（发送到 Copilot） |
| **octopus** | **热键** | **内置 LLM（OpenAI-compatible）** | **模拟 Cmd+V 原位替换** |

octopus 的差异化：**热键触发（无兼容性问题）+ 内置 AI（不依赖外部 App）+ Run And Paste（原位替换）**。

### 1.3 已有基础设施

| 能力 | 位置 | 复用 |
|------|------|------|
| 全局热键注册 | `tauri-plugin-global-shortcut` + `shortcut.rs` | 新注册一个热键 |
| 模拟键盘 `Cmd+C` | `focus_tracker.rs` `simulate_paste` 同模式 | 新增 `simulate_copy` |
| 模拟键盘 `Cmd+V` | `focus_tracker.rs` `simulate_paste` | 直接复用 |
| 焦点恢复 | `focus_tracker.rs` `restore_focus` | 直接复用 |
| 剪贴板读取 | `ClipboardHandle::read_text` | 直接复用 |
| LLM 调用 | `octopus_llm::chat_text_with_prompt`（action bar）/ `polish_regions`（ASR 润色） | action bar 不碰全局 prompt |
| 透明无边框浮窗 | `clipboard_window.rs` / `result_window.rs` | 参考模式 |
| 主题感知 | URL bg hex 注入 + `[data-theme]` CSS | 直接复用 |
| 键盘导航 | 剪贴板浮窗 keydown handler | 参考模式 |

---

## 2. 交互流程

### 2.1 完整流程

```
用户选中文本（任意 app）
  ↓
按全局热键（默认 Cmd+Shift+Space，可配置）
  ↓
Rust 后端（std::thread::spawn 后台线程）：
  1. TRIGGER_IN_PROGRESS 重入 guard（防热键连按）
  2. 隐藏常规窗口（settings/compact_editor）
  3. 备份剪贴板 → 模拟 Cmd+C → 等待 200ms → 读剪贴板
  4. 获取鼠标坐标（CGEvent::location()）
  5. run_on_main_thread → show_action_bar_window（鼠标上方）
  ↓
前端浮窗：
  6. 显示第一级菜单（图标行）
  7. 用户选择动作（鼠标/键盘）
  ↓
根据动作类型分流（见 2.2）
```

### 2.2 动作分流

#### AI 动作（润色/摘要/解释/翻译）

```
用户选 AI 动作
  ↓
浮窗切换为 loading 状态（转圈）
前端设 5s（翻译）/ 10s（其他）超时 + timedOutRef
  ↓
前端调 invoke("run_ai_action", { action, text })
  ↓
Rust 后端：
  1. 按 action 类型构造 system + user prompt
  2. 调 octopus_llm::chat_text_with_prompt（不碰全局 SYSTEM_PROMPT）
  3. 返回结果
  ↓
前端收到结果（先判 timedOutRef，已超时则丢弃）：
  invoke("action_bar_show_result", { result, originalText, action })
  ↓
Rust 后端：
     a. 写结果到剪贴板（结果留给用户，不恢复原始剪贴板）
     b. finalize_action_bar（恢复常规窗口 + 重置 guard）
     c. 用临时 tab 打开 CompactEditor 展示结果（不写 DB）
  ↓
超时场景：前端 setView("error")，后台 LLM 返回后丢弃
```

#### 翻译动作

与 AI 动作相同流程——翻译本质就是一个 prompt 为"翻译成中文/英文"的 AI 动作。一期翻译方向固定（自动检测→中译英/英译中），不弹子菜单。判断逻辑：选中文本含 CJK 字符（Unicode 范围 `\u4e00-\u9fff\u3040-\u30ff\uac00-\ud7af`）→ 翻译成英文；不含 CJK → 翻译成中文。

#### 搜索动作

```
用户点搜索图标
  ↓
前端调 invoke("action_bar_open_url", { url: search_url + encoded_text })
  ↓
Rust 后端：用系统默认浏览器打开搜索 URL
  ↓
隐藏浮窗
```

搜索引擎通过搜索子菜单选择（Google/百度/Bing），`action_bar_search_engine` 配置项控制子菜单默认高亮项。

#### 打开网页动作

```
选中文本符合 URL 格式（宽松：a.b 或 a.b/c...）
  ↓
用户点网页图标
  ↓
前端调 invoke("action_bar_open_url", { url: normalized_url })
  ↓
Rust 后端：打开 URL（自动补 https:// 如果缺 scheme）
  ↓
隐藏浮窗
```

URL 格式宽松检测：包含 `.` 且无空格 → 视为 URL。比剪贴板的 `detectUrl` 更宽松（不要求常见域名后缀）。

### 2.3 浮窗消失条件

| 事件 | 行为 |
|------|------|
| 选了 AI 动作 | 切换 loading，等结果返回后消失 |
| 选了搜索/网页 | 立即消失 |
| 点击浮窗外部 | 消失（不执行动作） |
| 按 Esc | 消失（不执行动作） |
| 按任意非导航键 | 消失（按键透传给下层 app） |

---

## 3. 动作列表与菜单结构

### 3.1 两级菜单

**第一级**（始终显示，图标行）：

| 图标 | 动作 | 触发条件 |
|------|------|---------|
| ✨ AI | 展开子菜单 | 始终 |
| 🌐 翻译 | 直接执行翻译 | 始终 |
| 🔍 搜索 | 打开搜索引擎 | 始终 |
| 🔗 网页 | 打开 URL | 选中文本符合 URL 格式（宽松检测） |

**第二级（AI 子菜单）**：

| 图标 | 动作 | Prompt 示意 |
|------|------|------------|
| ✏️ 润色 | 文本润色 | 复用现有 `polish_regions` 逻辑 |
| 📋 摘要 | 总结要点 | "请用简洁的中文总结以下内容的要点" |
| 💡 解释 | 解释含义 | "请解释以下内容的含义" |

### 3.2 URL 宽松检测

比剪贴板的 `detectUrl` 更宽松。满足以下任一条件即为 URL：

**路径 A：域名格式**（含 `.`）
```
条件：无空格 且 包含 "." 且 不以 "." 开头/结尾 且 "." 两侧至少一侧含字母
```

**路径 B：IP 地址**（无 `.` 不适用，但 IP 本身含 `.`——归入路径 A 的数字变体）
```
条件：匹配 IPv4 格式 \d{1,3}\.\d{1,3}\.\d{1,3}\.\d{1,3}（可选 :端口）
```

**路径 C：localhost**（无 `.`）
```
条件：匹配 localhost（可选 :端口、/路径）
```

示例：
- `apple.com` → ✅ 补 `https://apple.com`
- `github.com/octopus` → ✅
- `a.b/c` → ✅
- `192.168.1.100` → ✅ 补 `http://192.168.1.100`
- `127.0.0.1:3000` → ✅ 补 `http://127.0.0.1:3000`
- `localhost` → ✅ 补 `http://localhost`
- `localhost:8080/api` → ✅ 补 `http://localhost:8080/api`
- `hello world` → ❌（有空格）
- `123.456` → ❌（不像域名——`.` 两侧无字母且非 IP 格式）

### 3.3 键盘导航

> ⚠️ 以下为初始设计，**实际实现已有重大变更**。权威键盘导航逻辑详见 [`2026-07-09-action-bar-menu-db-design.md` §6.1](./2026-07-09-action-bar-menu-db-design.md#61-浮窗-actionbarindextsx)。

**实际实现的键盘导航**（菜单 DB 化后）：

| 按键 | 行为 |
|------|------|
| `↑ ↓` | **切换焦点层**（main↔sub），不展开/收起子菜单 |
| `← →` | **当前行移动**：主菜单中移到 submenu 项→展开子菜单预览，移到非 submenu 项→收起；子菜单中在子项间移动 |
| `Enter` / `Space` | 执行当前焦点高亮项 |
| `Cmd+数字` | 直接触发第 N 项（主菜单或子菜单） |
| `Esc` | **直接关闭浮窗**（一次） |

~~原始设计（已废弃）~~：

| 按键 | 第一级状态 | AI 子菜单展开状态 |
|------|-----------|------------------|
| ~~`← →`~~ | ~~在第一级图标间移动高亮~~ | ~~在第一级图标间移动高亮（子菜单不收起）~~ |
| ~~`↑ ↓`~~ | ~~无效~~ | ~~在子菜单项间移动高亮~~ |
| ~~`Esc`~~ | ~~关闭浮窗~~ | ~~返回第一级（收起子菜单）；再按 Esc 关闭浮窗~~ |

---

## 4. 窗口设计

### 4.1 action_bar_window

| 属性 | 值 | 说明 |
|------|-----|------|
| label | `action_bar_window` | 唯一标识 |
| decorations | false | 无标题栏 |
| transparent | true | 透明背景 |
| always_on_top | true | 悬浮在其他窗口上 |
| skip_taskbar | true | 不出现在任务栏 |
| resizable | false | 固定尺寸 |
| visible | false | 创建时隐藏，show 时显示 |
| 尺寸 | 260×82（主菜单 38px + 子菜单 38px + 边框） | CSS 自适应 |
| 位置 | 鼠标光标上方 | Rust 侧 `set_position` |

### 4.2 窗口生命周期

```
热键触发 → show + set_position（鼠标上方）
  ↓
动作完成 / Esc / 点击外部 → hide
  ↓
下次热键触发 → show（复用窗口，不销毁）
```

单例模式——首次创建，后续 show/hide toggle。和 `clipboard_window` 一致。

### 4.3 主题感知

- Rust 建窗时注入 URL `?bg=hex`（同 CompactEditor/Settings）
- 前端 index.html `<head>` 脚本同步设背景色
- `data-theme` 属性 + `[data-theme="xxx"]` CSS 规则
- icon-filter（暗色主题 SVG 图标反色）

### 4.4 macOS 激活策略

action_bar_window 是透明悬浮窗——**不切 Regular**（和 clipboard_window/result_window 一致），保持 Accessory 模式。

---

## 5. 后端命令

### 5.1 新增 Tauri 命令

| 命令 | 签名 | 说明 |
|------|------|------|
| `trigger_action_bar` | `(app: AppHandle)` | 热键触发：重入 guard → 隐藏常规窗口 → 备份剪贴板 → Cmd+C → 读剪贴板 → show 窗口。仅 macOS。 |
| `run_ai_action` | `async fn(action, text) -> Result<String, String>` | 调 `chat_text_with_prompt`（润色/摘要/解释/翻译），不碰全局 SYSTEM_PROMPT |
| `action_bar_show_result` | `(result, original_text, action, app)` | 写结果到剪贴板（留给用户）+ finalize + CompactEditor 临时 tab 展示 |
| `action_bar_dismiss` | `(app)` | 隐藏浮窗 + finalize（恢复剪贴板 + 恢复窗口 + 重置 guard） |
| `action_bar_open_url` | `(url, app)` | 打开 URL + finalize（恢复剪贴板 + 恢复窗口 + 重置 guard） |
| `action_bar_get_context` | `() -> Option<ActionBarContext>` | 前端 mount 时 take 选中文本 |

### 5.2 `simulate_copy` 实现

macOS 用 osascript 发送 Cmd+C 按键事件（与 simulate_paste 的 Cmd+V 模式一致）。Windows/Linux 为空实现（action bar 仅 macOS 可用）。

### 5.3 鼠标位置获取

macOS 用 `CGEvent::location()` 获取 Quartz 全局逻辑坐标（points）。⚠️ 此值为**逻辑坐标**，不除 scale_factor——与 Tauri `LogicalPosition` 一致。`Monitor::position()/size()` 才是物理像素需除 scale。详见 AGENTS.md 坐标踩坑章节。

### 5.4 AI 动作 prompt 映射

| 动作 | 构造方式 |
|------|---------|
| 润色 | `chat_text_with_prompt(system="润色...", user=text)` |
| 摘要 | `chat_text_with_prompt(system="总结...", user=text)` |
| 解释 | `chat_text_with_prompt(system="解释...", user=text)` |
| 翻译 | `chat_text_with_prompt(system="翻译成中文/英文...", user=text)`（自动检测源语言方向） |

~~一期通过临时切换 system prompt 实现——复用 `set_system_prompt` + `polish`。~~（已废弃）

**改为 `chat_text_with_prompt`**（审查修复 P0-1）：原方案临时切换全局 `SYSTEM_PROMPT` 会污染并发的 ASR 实时润色（`polish_regions` 读同一全局）。新增 `octopus_llm::chat_text_with_prompt(system, user, config)` 接受自定义 system + user prompt，不碰全局状态。同时修正了原方案 `polish()` 生成 "请润色以下语音识别文本" user prompt 对翻译/摘要/解释场景不匹配的问题。

---

## 6. 前端组件

### 6.1 action_bar/index.tsx

```
ActionBarPage
  ├── 第一级图标行（AI/翻译/搜索 + URL 检测到时显示网页图标）
  ├── 子菜单（展开时显示；AI: 润色/摘要/解释；搜索: Google/百度/Bing）
  ├── Loading 状态（AI 处理中）
  └── 错误状态（失败/超时信息 + 关闭按钮）
```

### 6.2 数据流

```
mount → invoke("action_bar_get_context") → 拿到 { text }
  → invoke("get_config") → 读取 searchEngine 配置
  ↓
渲染第一级菜单（detectActionUrl 判 URL 决定是否显示网页图标）
  ↓
用户选 AI → 展开子菜单（润色/摘要/解释）
用户选搜索 → 展开子菜单（Google/百度/Bing，默认高亮配置引擎）
用户选翻译 → executeAiAction("translate")
用户选网页 → invoke("action_bar_open_url", { url })
  ↓
AI 结果返回（未超时）→ invoke("action_bar_show_result", { result, originalText, action })
```

---

## 7. 配置项

| 配置字段 | 默认值 | 说明 |
|---------|--------|------|
| `action_bar_shortcut` | `Cmd+Shift+Space` | 唤起 AI 命令面板的全局热键 |
| `action_bar_search_engine` | `google` | 搜索引擎：google/baidu/bing（控制搜索子菜单默认高亮项） |

`action_bar_shortcut` 存 AppConfig，设置页 GeneralPanel 快捷键卡片配置。`action_bar_search_engine` 影响搜索子菜单默认高亮项（而非独立设置 UI）。

---

## 8. 不在本次范围

- 外部 App 集成（发送到豆包/Claude/ChatGPT——二期）
- Snippet 纯文本自定义动作（二期）
- 截图+OCR fallback（二期）——选中不到文本时（禁制复制页面/PDF）自动弹截图框 → OCR → 拿文本作为输入。已有截图+OCR 全链路能力（`screenshot_commands` + `paddle_ocr`），只需串联
- 上下文增强（二期）——传入当前 App 名 + 窗口标题作为 LLM system context（`focus_tracker` 已有获取活动窗口能力）；可加剪贴板最近 N 条历史
- 上下文感知（检测选中文本类型自动推荐动作——二期）
- Accessibility API 直读（替代 Cmd+C——macOS 专属增强，二期）
- 自动弹出（选中文本自动触发——不做，OnText 验证误触多）

---

## 9. 后续演进

- **二期**：外部 App 集成（通用"发送到 App + 粘贴"动作，用户配置 bundle ID）
- **Snippet 自定义动作**：`#octopus\nname: 翻译\nprompt: ...\ninput: selection` 纯文本格式
- **正则上下文**：动作可设正则规则，仅当选中文本匹配时显示
- **Accessibility 直读**：macOS `AXSelectedText` 替代 Cmd+C（禁用复制页面也能工作）
- **语音输入到面板**：语音说"翻译" → 面板自动选中翻译动作
