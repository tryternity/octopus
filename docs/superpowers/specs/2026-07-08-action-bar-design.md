# AI 命令面板设计 — 选中文本→热键→AI 处理→替换

> **状态**：已与用户确认设计，待写实施计划
> **日期**：2026-07-08
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
| LLM 调用 | `octopus_llm::polish` / `octopus_llm::polish_regions` | 复用底层 |
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
Rust 后端：
  1. 记录当前焦点窗口（restore_focus 用）
  2. 模拟 Cmd+C（选中文本 → 剪贴板）
  3. 等待 200ms（让系统完成复制）
  4. 读剪贴板拿到选中文本
  5. 获取鼠标坐标
  6. 在鼠标上方创建/显示 action_bar_window（传入选中文本 + 坐标）
  ↓
前端浮窗：
  7. 显示第一级菜单（图标行）
  8. 用户选择动作（鼠标/键盘）
  ↓
根据动作类型分流（见 2.2）
```

### 2.2 动作分流

#### AI 动作（润色/摘要/解释/翻译）

```
用户选 AI 动作
  ↓
浮窗切换为 loading 状态（转圈）
  ↓
前端调 invoke("run_ai_action", { action, text })
  ↓
Rust 后端：
  1. 构造 prompt（按动作类型）
  2. 调 octopus_llm::polish 或 polish_regions
  3. 返回结果
  ↓
前端收到结果：
  1. invoke("action_bar_paste_result", { result })
  2. Rust 后端：
     a. 写结果到剪贴板（write_text）
     b. 恢复焦点到原 app（restore_focus）
     c. 等待 100ms
     d. 模拟 Cmd+V（simulate_paste）
     e. 隐藏 action_bar_window
  ↓
成功：选中文本被替换为 AI 结果
失败（模拟粘贴未生效）：用户手动 Cmd+V（剪贴板已有结果）
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

搜索引擎可配置（默认 Google，中国区 Baidu，可在设置页选 Google/Baidu/Bing/自定义 URL）。

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

| 按键 | 第一级状态 | AI 子菜单展开状态 |
|------|-----------|------------------|
| `← →` | 在第一级图标间移动高亮 | 在第一级图标间移动高亮（子菜单不收起） |
| `↑ ↓` | 无效 | 在子菜单项间移动高亮 |
| `Enter` / `Space` | 执行高亮动作（AI 则展开子菜单） | 执行高亮的子菜单动作 |
| `Cmd+1..4` | 直接触发第 N 个第一级图标 | — |
| `Cmd+1..3`（子菜单展开后） | — | 直接执行第 N 个 AI 子菜单动作（润色/摘要/解释） |
| `Esc` | 关闭浮窗 | **返回第一级**（收起子菜单）；再按 `Esc` 关闭浮窗 |

**Cmd+数字的两段式**：第一级按 `Cmd+1` 展开 AI 子菜单；子菜单展开后 `Cmd+1/2/3` 分别执行润色/摘要/解释。用户可以连续按 `Cmd+1` → `Cmd+1` 完成"AI→润色"（两次按键）。

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
| 尺寸 | ~220×44（单行）/ ~220×120（子菜单展开） | CSS 自适应 |
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
| `trigger_action_bar` | `()` | 热键触发：模拟 Cmd+C → 读剪贴板 → 获取鼠标位置 → show 窗口 → emit 选中文本 |
| `run_ai_action` | `async fn(action: String, text: String) -> Result<String, String>` | 调 LLM 处理（润色/摘要/解释/翻译） |
| `action_bar_paste_result` | `(result: String)` | 写剪贴板 + 恢复焦点 + 模拟 Cmd+V + 隐藏浮窗 |
| `action_bar_open_url` | `(url: String)` | 用系统浏览器打开 URL + 隐藏浮窗 |
| `action_bar_get_context` | `() -> ActionResult` | 前端 mount 时拉取选中文本 + 上下文 |

### 5.2 `simulate_copy` 实现

```rust
pub fn simulate_copy(&self) {
    #[cfg(target_os = "macos")]
    {
        // 发送 Cmd+C（同 simulate_paste 的 Cmd+V 模式）
        self.send_keycode(0x08, true);  // Cmd
        self.send_keycode(0x06, true);  // C
        self.send_keycode(0x06, false);
        self.send_keycode(0x08, false);
    }
    // Windows/Linux: Ctrl+C
}
```

### 5.3 鼠标位置获取

```rust
// Tauri 2 的 WebviewWindow 不直接提供全局鼠标位置，
// 用 CGEvent (macOS) / Win32 GetCursorPos (Windows) 获取。
// 或用 tauri-plugin-positioner 的 cursor position。
```

### 5.4 AI 动作 prompt 映射

| 动作 | 构造方式 |
|------|---------|
| 润色 | 复用 `octopus_llm::polish_regions`（现有润色逻辑） |
| 摘要 | `polish(None, text, config)` + system prompt 改为"请用简洁的中文总结以下内容的要点" |
| 解释 | `polish(None, text, config)` + system prompt 改为"请解释以下内容的含义" |
| 翻译 | `polish(None, text, config)` + system prompt 改为"翻译成中文/英文"（自动检测源语言方向） |

一期通过临时切换 system prompt 实现——不需要新的 LLM 调用接口，复用 `octopus_llm` 的 `set_system_prompt` + `polish`。

---

## 6. 前端组件

### 6.1 action_bar/index.tsx

```
ActionBarPage
  ├── 第一级图标行（4 个图标 + 高亮）
  ├── AI 子菜单（展开时显示，3 个动作）
  ├── Loading 状态（AI 处理中）
  └── 错误状态（失败 + "已复制到剪贴板"提示）
```

### 6.2 数据流

```
mount → invoke("action_bar_get_context") → 拿到 { text, hasUrl }
  ↓
渲染第一级菜单（根据 hasUrl 决定是否显示网页图标）
  ↓
用户选 AI → 展开子菜单
用户选搜索 → invoke("action_bar_open_url", { search_url + text })
用户选翻译 → invoke("run_ai_action", { action: "translate", text })
用户选网页 → invoke("action_bar_open_url", { url })
  ↓
AI 结果返回 → invoke("action_bar_paste_result", { result })
```

---

## 7. 配置项

| 配置字段 | 默认值 | 说明 |
|---------|--------|------|
| `action_bar_shortcut` | `Cmd+Shift+Space` | 唤起 AI 命令面板的全局热键 |
| `action_bar_search_engine` | `google` | 搜索引擎：google/baidu/bing/自定义 URL |

配置存 AppConfig（serde 自动 load/save），设置页 GeneralPanel 快捷键卡片新增一行。

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
