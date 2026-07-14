# Launch 功能深度调研报告

> 2026-07-15 · 调研启动器类工具的 launch 功能，为 octopus 引入启动器/命令面板能力做技术参考
> 基于项目历史调研文档 + .tolaria 知识库 + 最新 web 资源

## 0. 已有调研基础

本项目已有两份相关调研：
- `docs/superpowers/specs/research/2026-07-07-launcher-survey.md` — Wox / Raycast / Alfred 三者功能矩阵（功能清单级）
- `docs/superpowers/specs/research/2026-07-09-action-bar-related-tools-survey.md` — 11 份工具笔记补充（VoxFlow / eSearch / KoBar / PopClip 等）
- `.tolaria/桌面工具/启动器功能特性调研-wox-raycast-alfred.md` — 同上第一份的 .tolaria 副本

本报告在以上基础上**聚焦 launch 功能本身**——即"用户如何唤起、触发、执行命令"，而非泛功能对比。

---

## 1. Launch 功能分类框架

从三个启动器 + 相关工具中提炼出 launch 功能的 7 个维度：

### 1.1 唤起方式（Invocation）

| 方式 | Wox | Raycast | Alfred | PopClip | octopus 现状 |
|------|:---:|:-------:|:------:|:-------:|:------------:|
| **全局热键** | ✅ Alt+Space（可配） | ✅ ⌥+Space（可配） | ✅ ⌘+Space 替代 Spotlight | ❌ | ✅ ⌘⇧Space |
| **选中文本触发** | ❌ | ❌ | ❌ | ✅ 选中文本后弹出 | ✅ action bar |
| **快捷键直达** | ✅ Query Hotkey（per-hotkey 配置 query/position/width） | ✅ 每个命令可绑独立热键 | ✅ Workflow Hotkey | ✅ 每个动作可配快捷键 | ✅ 已有 shortcut 字段 |
| **Silent Query**（不弹窗直接执行） | ✅ AI Command silent mode | ✅ Quick AI | ❌ | ❌ | ❌ |
| **Floating Hotkey**（按住说话） | ❌ | ❌ | ❌ | ❌ | ✅ VoxFlow 同源模式 |
| **Snippet 触发**（打字自动展开触发命令） | ❌ | ✅ Snippet keyword | ✅ Snippet Trigger | ❌ | ❌ |
| **Universal Action**（对选中文件/文本执行操作） | ❌ | ✅ | ✅ Powerpack 60+ | ✅ | ✅ action bar（kind=Text/Files + accepts=text/file/any） |
| **菜单栏图标点击** | ✅ Tray Query | ✅ | ✅ | ❌ | ✅ 托盘菜单 |
| **Deep Link / URL Scheme** | ✅ `wox://` | ❌ | ✅ `alfred://` | ❌ | ❌ |

### 1.2 查询输入（Query Input）

| 能力 | Wox | Raycast | Alfred |
|------|:---:|:-------:|:------:|
| **模糊搜索** | ✅ fzf 算法 + 拼音 + 短文本对齐 | ✅ | ✅ 缩写/模糊 |
| **自然语言计算器** | ✅ 表达式优先级 | ✅ | ✅ `=` 前缀 |
| **Inline 预览** | ✅ 输入时实时预览结果 | ✅ | ✅ |
| **多行输入** | ❌（单行） | ✅ AI Chat 多行 | ✅ Text View |
| **命令前缀路由** | ❌（全局搜索） | ❌（全局搜索） | ✅ 关键词前缀触发 Workflow |
| **Action Panel**（结果上按 Tab 展开二级操作） | ✅ | ✅ | ✅ |

### 1.3 结果渲染（Result Rendering）

| 视图 | Wox | Raycast | Alfred |
|------|:---:|:-------:|:------:|
| **List** | ✅ 主视图 | ✅ List | ✅ 列表 |
| **Detail / Preview** | ✅ 文件预览（code/image/PDF/Office...） | ✅ Detail 视图 | ✅ Quick Look（Shift） |
| **Grid** | ❌ | ✅ Grid | ✅ Grid（5.5+） |
| **Markdown Text** | ✅ WebView | ✅ | ✅ Text View（5.5+） |
| **WebView**（嵌入网页） | ✅ 导航/工具栏/缓存 | ❌ | ❌ |
| **Image Overlay** | ✅ | ❌ | ✅ Image View（5.5+） |
| **AI Chat** | ✅ 内置 Chat | ✅ 内置 Chat | ✅ ChatGPT/DALL-E Workflow（5.5+） |

### 1.4 执行动作（Action Execution）

| 动作类型 | Wox | Raycast | Alfred | octopus 可复用 |
|----------|:---:|:-------:|:------:|:--------------:|
| **打开应用/文件** | ✅ | ✅ | ✅ | ✅ 已有 |
| **复制到剪贴板** | ✅ | ✅ | ✅ | ✅ 已有 |
| **AI 处理（选中文本→处理→替换/插入）** | ✅ Run And Paste | ✅ AI Commands | ❌ | ✅ 已有润色 |
| **Web 搜索** | ✅ | ✅ Quicklinks | ✅ | ✅ 已有 |
| **Shell 命令执行** | ✅ 后台执行 | ✅ | ✅ `>` | ✅ 已有（script 类型） |
| **打开 URL** | ✅ | ✅ | ✅ | ✅ 已有 |
| **系统命令（关机/休眠等）** | ✅ | ✅ | ✅ | ❌ 可加 |
| **窗口管理** | ✅ Window Manager | ✅ Pro | ❌ | ❌ 远期 |
| **Workflow 链式编排** | ❌ | ❌ | ✅ 画布编辑器 | ❌ 远期 |
| **拖拽导出** | ✅ | ❌ | ❌ | ❌ |

### 1.5 命令注册与管理

| 维度 | Wox | Raycast | Alfred | octopus 现状 |
|------|:---:|:-------:|:------:|:------------:|
| **命令存储** | 插件 JSON 配置 + Store | Extension manifest | Workflow 文件 | DB `action_bar_items` 表 |
| **可视化编辑** | ❌（代码） | ❌（代码） | ✅ **画布拖拽** | ✅ 设置页 CRUD |
| **命令分组** | 插件名分组 | Extension 名分组 | Workflow 分组 | ✅ submenu 两级 |
| **命令商店** | ✅ 插件/主题/AI命令 3 商店 | ✅ 数千扩展 | ✅ Gallery | ❌ |
| **远程命令** | ✅ AI Command Store | ✅ Store | ✅ Gallery | ❌ |

### 1.6 上下文感知（Context Awareness）

| 能力 | Wox | Raycast | Alfred | VoxFlow | octopus |
|------|:---:|:-------:|:------:|:-------:|:-------:|
| **当前应用感知** | ✅ | ✅ | ✅ | ✅ | ✅ app_context |
| **选中文本感知** | ❌ | ✅ Universal Action | ✅ | ✅ | ✅ |
| **窗口标题感知** | ✅ | ❌ | ❌ | ✅ | ✅ |
| **文件选中感知** | ❌ | ✅ Universal Action | ✅ | ❌ | ✅ Finder |
| **AX 文本树读取** | ❌ | ❌ | ❌ | ✅ | ✅ macos_ax |
| **OCR 截图兜底** | ❌ | ❌ | ❌ | ✅ | ✅ |

### 1.7 AI 集成

| 能力 | Wox | Raycast | Alfred 5.5+ |
|------|:---:|:-------:|:-----------:|
| **AI Chat** | ✅ | ✅ | ✅ ChatGPT Workflow |
| **AI Commands（选中文本处理）** | ✅ silent + Run And Paste | ✅ | ✅ |
| **多模型 Provider** | ✅ 8 个 | ✅ 数十个 | ✅ OpenAI/自定义 |
| **MCP 集成** | ✅ Client + Server | ✅ | ❌ |
| **Tool Use（工具调用）** | ✅ tool_registry | ✅ | ❌ |
| **BYOK（自带 Key）** | ✅ | ✅ | ✅ |
| **AI 生成 Workflow** | ❌ | ❌ | ✅ 社区讨论中 |

---

## 2. Alfred（AiFred）Launch 功能深挖

> Alfred 5.5+（2026-07） — "AiFred" 是社区对 Alfred AI 化的昵称

### 2.1 Workflow 触发器（Launch 入口）

Alfred 的触发器最丰富：

| 触发器 | 说明 |
|--------|------|
| **Hotkey** | 全局快捷键，可配修饰键 + 参数（选中文本/文件/URL） |
| **Keyword** | 输入框中输入关键词前缀触发 |
| **Universal Action** | 对选中的文件/文本/URL 执行（60+ 内置操作） |
| **External Trigger** | 其他 Workflow 或脚本通过 URL Scheme/CLI 触发 |
| **Snippet Trigger** | 打字时自动匹配触发（如 `;date` 展开为日期） |
| **Contact Action** | 通讯录条目触发 |
| **File Action** | 文件操作触发 |
| **Fallback Search** | 搜索无结果时 fallback 到 Web 搜索 |
| **URL Scheme** | `alfred://navigateto/workflows>workflow>...` |

### 2.2 Alfred 5.5 新增 AI 相关

- **ChatGPT / DALL-E Workflow**：内置 AI 交互 Workflow，Markdown 渲染
- **Text View**：Markdown 富文本渲染，支持 AI 输出
- **Image View**：图片预览，支持 DALL-E 生成结果
- **Agentic Workflow 讨论**：社区讨论用 AI agent 自动构建 Workflow（未实现）

### 2.3 Alfred 独特的 Launch 能力

- **可视化 Workflow 画布**：拖拽连接节点，零代码编排——Wox/Raycast 都没有
- **Automation Tasks**：预制无代码操作（调图片大小、切换暗色等），无需写脚本
- **Prefabs**：Workflow 预制件，可复用的节点组合
- **User Configuration**：Workflow 内嵌用户配置面板（下拉/文本/复选框）
- **条件分支 + 变量系统**：Workflow 内支持 if/else、变量传递

---

## 3. Raycast Launch 功能深挖

### 3.1 核心启动能力

- **Root Search**：全局搜索（应用/文件/命令/计算器/Snippets 一体化）
- **Quicklinks**：带变量的快捷链接（`{query}` 占位），可绑热键
- **Snippets**：关键词展开文本（动态变量 `{date}/{clipboard}/{cursor}`）
- **AI Commands**：选中文本 → AI 处理 → 替换/插入/复制
- **Quick AI**：结合 Web 回答任意问题
- **Universal Actions**：对任意选中项执行操作

### 3.2 独特 Launch 能力

- **Hyper Key**：将 Caps Lock 重映射为 Hyper 键，全局可用
- **Dictate Anywhere**：系统级听写集成
- **Floating Notes**：浮动笔记窗口
- **Focus Mode**：专注模式（屏蔽干扰应用）
- **Calendar**：日历事件快速查看和加入会议
- **Window Management**（Pro）：快捷键控制窗口位置/大小

---

## 4. Wox Launch 功能深挖

### 4.1 独特 Launch 能力

- **Query Hotkey**：每个热键可独立配置 `position / query / toolbar / width`——不同热键唤起不同位置/不同初始查询的面板
- **Silent Query Hotkey**：不弹面板，直接执行 AI 命令（选中文本→处理→替换）
- **Run And Paste**：AI 命令的"处理选中文本→结果直接粘贴到光标位置"
- **Explorer 插件**：在系统 open/save 对话框中搜索切换路径
- **Glance**：查询框空状态时展示实时信息（时间/电池/CPU/内存）
- **Attention**：持久通知中心（follow-up 任务、unread badge）
- **Tray Query**：系统托盘菜单可配置自定义查询命令
- **Action Panel**：每个结果携带二级操作面板（Tab 键展开）
- **Result Drag**：结果项可拖拽到文件夹/其他应用
- **Deep Link**：`wox://` URL Scheme 深度链接

### 4.2 AI Command 生态

- **AI Command Store**：在线 AI 命令商店（社区分享 prompt 模板）
- **AI Theme Generation**：用 AI 生成主题配色
- **AI Emoji Search**：自然语言搜 Emoji
- **MCP Client + Server**：可作为 MCP client 调用外部工具，也内置 MCP server 供插件开发

---

## 5. 对 octopus 的 Launch 功能建议

### 5.1 octopus 当前 Launch 能力

| 已有 | 说明 |
|------|------|
| **全局热键唤起** | ⌘⇧Space 唤起 action bar |
| **选中文本浮窗** | action bar 浮窗（翻译/搜索/AI/脚本） |
| **菜单项快捷键** | `action_bar_items.shortcut` 字段 |
| **托盘菜单** | 系统托盘唤起设置/录音 |
| **上下文感知** | app_context（AX 文本树/OCR/窗口标题） |
| **Finder 文件选中** | Finder 选中文件触发 action bar |
| **Agent 语音联动** | 含 `{{task}}` 的菜单项联动语音录音 |

### 5.2 可引入的 Launch 功能（按价值排序）

#### ~~P0: Universal Actions 扩展~~（已实现）

octopus action bar **已经是 Universal Action 的等价实现**：

| 维度 | Raycast | Alfred | octopus |
|------|---------|--------|---------|
| 选中文本触发 | ✅ | ✅ Powerpack | ✅ `kind=Text` |
| 选中文件触发 | ✅ | ✅ Powerpack | ✅ `kind=Files`（Finder 选中） |
| 按上下文过滤动作 | ✅ | ✅ | ✅ `accepts=text/file/any` |
| 动作类型 | AI/搜索/复制/Note | 60+ 内置 | AI/搜索/网页/脚本/copy_path/agent |
| 触发方式 | 命令面板内选中 | 全局热键 | 全局热键 ⌘⇧Space |

**octopus 与两者的差异**：
- Alfred/Raycast 的 Universal Action 是"选中后在面板里选操作"，octopus 是"选中后浮窗直接选"——交互更轻
- octopus 的 `accepts` 字段按上下文过滤可见菜单项，与 Raycast/Alfred 逻辑一致
- octopus 独有 `agent` 类型（文件桥接到 Terminal.app 启动编码 agent），三者无等价

**可改进方向**：扩展 `kind=Files` 的文件类型识别（图片→OCR、代码文件→格式化、压缩包→预览），而非当前仅 agent 桥接。

#### P0（高价值，低成本）

1. **Silent Query Hotkey**——借鉴 Wox：不弹 action bar，直接用热键执行某个菜单项。用户选中文本 → 按 `⌘⇧T` → 直接翻译（不经过菜单选择）。已有 `shortcut` 字段，只需前端注册全局热键时判断是否 silent。

2. **Deep Link（`octopus://`）**——借鉴 Wox/Alfred：允许其他应用/脚本通过 URL Scheme 触发 octopus 命令（`octopus://translate?text=hello`）。Tauri 2 原生支持 deep link。

#### P1（中价值，中成本）

4. **Glance / 空状态面板**——借鉴 Wox：action bar 空状态时显示实时信息（ASR 状态/录音历史/模型加载/系统资源），而非空白。

5. **AI Command Store（远程命令）**——借鉴 Wox：在线命令模板库，用户一键安装（翻译/润色/摘要 prompt 模板）。octopus 已有 `action_bar_items` DB 表，只需加远程拉取 + 导入。

6. **Quicklinks**——借鉴 Raycast：带变量的快捷链接（`https://translate.google.com/?text={query}`），可绑热键，直接在浏览器打开。与 action bar 的 `web` 类型菜单项接近，但更轻量。

#### P2（高价值，高成本）

7. **可视化 Workflow 编排**——借鉴 Alfred 画布：拖拽连接节点（ASR → LLM → 剪贴板 → 粘贴），零代码。实现成本极高但差异化强。

8. **Snippet 自动展开**——借鉴 Raycast/Alfred：打字时自动匹配关键词展开（`;date` → 当前日期），全局键盘监听。

9. **Query Hotkey 多实例**——借鉴 Wox：不同热键唤起不同位置/不同初始查询的 action bar。

### 5.3 octopus 独有的差异化 Launch 能力

octopus 有三者都没有的核心能力：**语音**。

- **语音唤起**："嘿 octopus" → 录音 → 识别 → 执行命令（语音→命令路由）
- **语音命令**：说"翻译这段" → 自动选中文本 → 翻译 → 粘贴
- **语音 + 上下文联动**：语音输入意图 + AX/OCR 读窗口上下文 → LLM 生成 prompt → 执行

这是 Wox/Raycast/Alfred 都是纯键盘驱动的蓝海区域。

---

## 6. 关键启示

1. **Alfred 的护城河是 Workflow 画布**——零代码编排是最大差异化，但实现成本极高
2. **Raycast 的护城河是生态规模**——数千扩展 + Team 协作 + iOS 伴侣
3. **Wox 的护城河是开源全免费 + AI 原生**——MCP/Skill/Tool Use/AI Store 全面集成
4. **三者共同趋势**：AI 集成标配化、MCP 协议兴起、Universal Actions 扩展化
5. **octopus 的差异化**：语音驱动 + ASR 原生——三者都不具备的蓝海
