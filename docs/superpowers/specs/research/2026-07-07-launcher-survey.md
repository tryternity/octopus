# 启动器功能特性调研 — Wox / Raycast / Alfred

> 调研对象：[Wox](https://github.com/Wox-launcher/Wox)（本地：`/Users/wudarui/workspace/agent/wox`，v2.3.0）、[Raycast](https://www.raycast.com/)（Pro 页 + 官网）、[Alfred](https://www.alfredapp.com/)（官网 + 帮助文档，v5.7.3）
> 调研日期：2026-07-07
> 调研目的：为 octopus（Rust + Tauri 桌面应用）引入启动器 / 命令面板能力做技术选型参考，产出一份客观的三者功能矩阵
> 文档深度：功能清单级（列特性 + 简述，不做架构级深挖）

---

## 1. 三者定位

| | Wox | Raycast | Alfred |
|---|---|---|---|
| **一句话定位** | 开源跨平台启动器，单可执行文件 | macOS 原生、键盘优先的可扩展启动器 | macOS 老牌启动器，Workflows 可视化编排见长 |
| **平台** | macOS / Linux / Windows | macOS（iOS 有 app、Windows 开发中、Browser Extension） | 仅 macOS 10.14+ |
| **开源** | 是（GPLv3） | 否（扩展生态开源） | 否 |
| **技术栈** | Go 后端 + Flutter UI（WebSocket/HTTP 通信）+ Python/Node.js 插件宿主 | 原生 Swift/SwiftUI | 原生（Swift） |
| **授权模式** | 完全免费 | 免费 + Pro（$8/月起）+ Advanced AI add-on + Team | 免费 + Powerpack（Single £34 / Mega £59 终身升级） |
| **分发** | 单可执行文件，无强制安装流程 | Mac App Store + 官网下载 | 官网下载 |
| **架构亮点** | core/host 分离，UI 嵌入，多语言插件进程隔离 | 原生性能，React + TS 扩展 SDK | 可视化 Workflow 画布 |

---

## 2. 核心 Launcher 功能

| 功能 | Wox | Raycast | Alfred |
|------|-----|---------|--------|
| **应用启动** | ✅ 全平台索引（macOS .app、Win .lnk/.url/UWP、Linux .desktop），本地化名、图标缓存 | ✅ | ✅ 缩写匹配（gc→Chrome） |
| **文件搜索** | ✅ 原生数据库索引（扫描/wildcard/增量 changefeed/跨平台 provider），不依赖 Everything | ✅ File Search | ✅ 免费基础；Powerpack 含 File Filter / Tags / File Buffer |
| **计算器** | ✅ 含历史、千分位、`^` 幂运算 | ✅ Calculator | ✅ 标准 + 高级（`= ` 前缀，三角/对数） |
| **剪贴板历史** | ✅ 文本/图片/文件路径、收藏、链接识别 | ✅ Pro 无限 | ✅ Powerpack（文本/图片/文件/颜色，保留期可配，密码应用忽略） |
| **拼写检查** | ❌ 无原生 | ❌ 无原生 | ✅ `spell` |
| **词典** | ❌ 无原生 | ❌ 无原生 | ✅ `define` + 同义词 |
| **系统命令** | ✅ 关机/重启/休眠/音量/Task View/copy-version | ✅ System | ✅ 锁屏/睡眠/关机/重启/清回收站/弹出磁盘等 |
| **URL 处理** | ✅ 动态网站图标 | ✅ | ✅ 识别并打开 |
| **Web 搜索** | ✅ 自定义浏览器 | ✅（Quicklinks） | ✅ 内置 20+ 预置 + 自定义 |
| **浏览器书签** | ✅ Safari/Chrome/Edge/Firefox | ✅（扩展） | ✅ Safari/Chrome（含阅读列表） |
| **联系人** | ❌ 无原生 | ❌ 无原生 | ✅ 通讯录搜索；Powerpack 含邮件/拨号/地址操作 |
| **Shell/终端** | ✅ 终端预览、搜索全屏、后台执行、history | ✅（扩展） | ✅ `>` 执行；Powerpack 含自定义终端、Browse in Terminal |
| **单位/货币换算** | ✅ Converter（长度/重量/温度/存储/时间/货币） | ✅（Convert） | ✅（基础） |

---

## 3. 窗口管理

| | Wox | Raycast | Alfred |
|---|-----|---------|--------|
| **能力** | ✅ Window Manager 插件：移动/调整/最小/最大/恢复/跨屏；**workspace layouts**（跨显示器保存 app 布局、恢复、启动缺失 app、浏览器 URL 去重） | ✅ Window Management（Pro）：自定义命令定位/调整窗口 | ❌ 无原生（靠 Workflow） |

---

## 4. AI 功能

| 功能 | Wox | Raycast | Alfred |
|------|-----|---------|--------|
| **AI Chat** | ✅ | ✅ | ❌ 无内置（靠 Workflow + Text View） |
| **AI Commands** | ✅ 模板（翻译/摘要）、silent query hotkey、Run And Paste（选中文本→AI 处理→原位替换） | ✅ AI Commands（自动化重复任务） | ❌（社区 OpenAI Workflow） |
| **Quick AI / 内联** | ✅（AI Command silent 模式） | ✅ Quick AI（结合 web 回答任意问题） | ❌ |
| **模型 Provider** | OpenAI / DeepSeek / Google / Groq / MiniMax / Ollama / OpenRouter / SiliconFlow（8 个，含本地 Ollama） | OpenAI / Anthropic / Perplexity / Meta / Mistral / Google / xAI / DeepSeek / Moonshot / Qwen / MiniMax / Z.ai / Groq / Together / Replicate / Baseten（数十个模型，含 GPT-5.x、Claude 4.x、Gemini 3.x、Grok 4.x） | 无（外部 API Key 自备） |
| **BYOK（自带 Key）** | ✅（provider 自配 host/key） | ✅ OpenAI / Anthropic / Google | N/A |
| **MCP 集成** | ✅ MCP client（STDIO + StreamableHTTP）；另内置 MCP server 用于插件开发 | ✅（支持） | ❌ |
| **工具调用（Tool Use）** | ✅ tool_registry + 内置工具（bash/edit/read_file/write_file/web_fetch/web_search/read_skill） | ✅（AI 模型原生） | ❌ |
| **Skill 系统** | ✅ discovery/registry/remote/runtime（可被 chat 引用） | ❌ | ❌ |
| **AI 主题生成** | ✅（ai-command store + 主题生成） | ❌ | ❌ |
| **Emoji AI 搜索** | ✅ | ❌ | ❌ |
| **隐私** | 本地/自配 provider | 不记录输入、不训练模型；Cloud Sync 时加密存储 | N/A |
| **付费门槛** | 免费 | Pro 起；Advanced AI add-on 解锁顶级模型 | N/A |

---

## 5. 插件 / 扩展系统

| | Wox | Raycast | Alfred |
|---|-----|---------|--------|
| **扩展 SDK 语言** | Python（`wox.plugin.python`）+ Node.js（`wox.plugin.nodejs`）+ Script Plugin + **WebSocket Plugin**（任意语言） | React + TypeScript | Bash/PHP/Python 3/Swift/Ruby/Perl/AppleScript/JS(Node) |
| **运行方式** | 独立宿主进程隔离（host_python / host_nodejs），JSON-RPC over WebSocket | 进程内扩展 | Workflow 脚本节点 |
| **商店** | ✅ 3 个：插件商店、主题商店、**AI 命令商店** | ✅ Raycast Store（数千扩展） | ✅ Alfred Gallery + Forum + GitHub（数千 Workflow） |
| **包管理** | ✅ WPM（可创建 script plugin，安装/升级/版本检查） | ✅ Store 内一键 | ✅ `.alfredworkflow` 导入导出 |
| **可视化编排** | ❌（代码式） | ❌（代码式） | ✅ **Workflow 画布编辑器**（拖拽、Prefabs 预制件、Automation Tasks、User Configuration、条件分支、变量系统） |
| **预制动作库** | ❌ | ❌ | ✅ Automation Tasks（无需写码：调图片大小、切换暗色等） |
| **Workflow 触发器** | Query Hotkey + 关键词 | Hotkey + 关键词 | Hotkey/Keyword/Universal Action/External/Snippet/Contact/File/URL Scheme/Fallback |
| **Rich UI Views** | Preview + WebView + Overlay（preserve-position/max-height/follow-scroll） | List / Detail / Grid（扩展内） | ✅ Text View（Markdown）/ Grid / Image / PDF View（5.5+） |

---

## 6. Snippets 片段

| | Wox | Raycast | Alfred |
|---|-----|---------|--------|
| **支持** | ❌ 无独立 Snippet 系统 | ✅ 关键词展开、动态变量 | ✅ Powerpack：集合、富文本、自动展开、`{date}/{time}/{clipboard}/{cursor}` 占位符、安全字段保护、导出分享 |
| **Snippet 触发 Workflow** | ❌ | ❌ | ✅（Snip Trigger） |

---

## 7. Quicklinks / 快速链接

| | Wox | Raycast | Alfred |
|---|-----|---------|--------|
| **能力** | 间接：Query Hotkey（per-hotkey 配置 position/query/toolbar/width）+ WebView（一键打开常用站）+ Web Search | ✅ **核心功能**：带变量的 Quicklinks（`{query}` 占位） | ✅ 自定义 Web Search（关键词+模板 URL） |

---

## 8. 主题与外观

| | Wox | Raycast | Alfred |
|---|-----|---------|--------|
| **主题编辑器** | ✅ Theme Editor（live preview、颜色控制、save-as、平台特定变体、wallpaper-aware 预览） | ✅ Custom Themes（Pro） | ✅ Powerpack：颜色/字体/间距/边框/透明度/模糊全自定义 |
| **主题商店** | ✅ theme store（169KB JSON，大量主题） | ✅ 社区数百主题 | ✅ 官网分享 |
| **跟随系统** | ✅ Auto theme（light/dark） | ✅ | ✅ Modern Dark/Light |
| **默认特色** | Glass dark（亚克力模糊、透明面板） | 原生质感 | 经典礼帽图标（可隐藏） |

---

## 9. 云同步 / 多设备

| | Wox | Raycast | Alfred |
|---|-----|---------|--------|
| **能力** | ✅ Cloud Sync（v2.3 新）：AES-256-GCM 加密、Argon2id recovery code、设备管理（join/revoke）、LWW 冲突、同步 settings/plugins/themes | ✅ Cloud Sync（Pro）：跨 Mac 同步工作流 | ✅ Powerpack：经 Dropbox/Google Drive/iCloud/OneDrive 同步偏好（部分设置按 Mac 独立） |
| **账号体系** | ✅ account + 付费 plan + 设备 | ✅ Raycast 账号 | ❌（无账号，纯文件同步） |

---

## 10. 团队 / 共享

| | Wox | Raycast | Alfred |
|---|-----|---------|--------|
| **团队功能** | ❌（个人） | ✅ Team plan：私有共享 Extensions / Snippets / Quicklinks | ❌（靠文件分享 Workflow） |

---

## 11. 预览能力

| | Wox | Raycast | Alfred |
|---|-----|---------|--------|
| **文件预览** | ✅ 超广：code/exe/image/markdown/PDF/shortcut/video/zip/Office/audio/font/calendar-contact/delimited data/RDP/folder/media + macOS Quick Look + 按需加载 | ✅（扩展内 Detail 视图） | ✅ Quick Look（Shift）+ 侧边栏预览（⌘⇧I） |
| **WebView 预览** | ✅ 嵌入网站预览（导航/工具栏/缓存/系统浏览器打开/清除会话） | ❌ | ❌ |
| **图片 Overlay** | ✅ 轻量图片覆盖预览 | ❌ | ✅ Image View（5.5+） |
| **可选中文字** | ✅（OCR/截图/剪贴板/富预览可选中文复制） | ✅ | ✅ Text View |

---

## 12. 截图 / 标注

| | Wox | Raycast | Alfred |
|---|-----|---------|--------|
| **能力** | ✅ Screenshot 插件：标注、历史、导出路径、剪贴板交接、多屏、**滚动截图**、**钉图覆盖**、插件 API、历史保留 | ❌ 无原生（扩展） | ❌ 无原生（Workflow） |

---

## 13. 其他独特功能

| 功能 | Wox | Raycast | Alfred |
|------|-----|---------|--------|
| **Glance（查询框实时信息）** | ✅ 时间/日期/电池/CPU/内存，插件可提供 | ❌ | ❌ |
| **Attention（通知中心）** | ✅ 持久 follow-up 任务、unread badge、inbox | ❌ | ❌ |
| **Notes** | ❌ | ✅ Raycast Notes（Pro，floating 笔记） | ❌ |
| **Focus（专注模式）** | ❌ | ✅ Raycast Focus（屏蔽干扰） | ❌ |
| **Translator** | ✅（AI Command 模板） | ✅ Translator（Pro，发音/听写） | ❌（社区 Workflow） |
| **Media Player** | ✅ Windows media session 集成（曲目/封面/播放控制） | ❌ | ✅ Music Mini Player（Music.app，含评分/播放列表） |
| **Selection Quick Look** | ✅ Space 预览（Windows File Explorer/open-save dialog） | ❌ | ✅ Quick Look（Shift） |
| **Explorer（对话框路径）** | ✅ open/save dialog 路径切换 + type-to-search | ❌ | ❌ |
| **Color** | ✅ Color 插件（名称/hex，多格式复制） | ❌（扩展） | ❌ |
| **Folder 收藏** | ✅ | ❌ | ❌ |
| **Result Drag 导出** | ✅ 原生文件拖拽到文件夹/其他 app | ❌ | ❌ |
| **Query Refinement** | ✅ 插件可暴露过滤/排序（文件类型/时间/大小） | ❌ | ❌ |
| **Tray Query** | ✅ 托盘自定义查询菜单 | ❌ | ❌ |
| **Hotkey Overview** | ✅ 已注册快捷键预览 | ✅ | ✅（冲突提示） |
| **Caps Lock 组合键** | ✅（非 Linux） | ❌ | ❌ |
| **Search（模糊/拼音）** | ✅ fzf 算法、拼音、短文本优化 | ✅ | ✅ 缩写/模糊 |
| **1Password 集成** | ❌ | ✅（扩展） | ✅ Powerpack（深度集成） |
| **Universal Actions** | ❌ | ✅ | ✅ Powerpack（60+ 操作，智能过滤） |
| **macOS Shortcuts** | ❌ | ✅（扩展） | ✅ Powerpack（Workflow 内运行） |
| **Onboarding** | ✅ 首次引导 | ✅ | ❌ |
| **Doctor 诊断** | ✅（severity 分级） | ❌ | ❌ |
| **Updater 渠道** | ✅ stable/beta 切换 | ✅ | ✅ |
| **Backup/Restore** | ✅ 全数据目录 | ✅ | ✅（Backup Workflow） |
| **i18n** | ✅ 多语言 | ✅ | ✅ |
| **iOS 伴侣** | ❌ | ✅ iOS app | ✅ Alfred Remote（iPhone/iPad 遥控） |
| **Usage Stats** | ✅（daily、X 分享） | ❌ | ✅（28 天图表、Twitter 分享） |
| **Telemetry** | ✅ 可选匿名 | ❌ | ❌ |
| **Pomodoro** | ❌ | ✅（扩展） | ✅（社区 Workflow） |

---

## 14. 免费 vs 付费功能边界

| | 完全免费 | Freemium |
|---|---|---|
| **Wox** | **全部功能免费**（开源） | — |
| **Raycast** | Launcher 基础、Store 扩展、有限 Clipboard、File Search、Calculator、Snippets、Quicklinks、Emoji、System、Calendar | **Pro**：AI、Cloud Sync、Custom Themes、Translator、无限 Clipboard、Custom Window Management、Notes、Focus；**Advanced AI** add-on：顶级模型 |
| **Alfred** | 应用启动、基础文件搜索、计算器、拼写/词典、系统命令、默认 Web 搜索、书签、Quick Look、Large Type、Terminal 基础、联系人基础 | **Powerpack**：Clipboard History、Snippets、Workflows、自定义 Hotkeys、主题编辑、1Password、Music Mini Player、Universal Actions、配置同步、Rich Views、Automation Tasks、macOS Shortcuts、File Buffer、最近文档 |

---

## 15. 对 octopus 的启示（技术选型参考）

> octopus 当前是 Rust + Tauri 2 桌面应用（ASR 工具集，含 CLI/Server/Desktop）。若引入启动器 / 命令面板能力，三者各有一部分可借鉴。

### 15.1 架构契合度

| 维度 | octopus 现状 | 最值得借鉴 |
|------|-------------|-----------|
| 后端语言 | Rust | 三者均非 Rust，但 **Wox 的 core/host 进程分离 + JSON-RPC** 模式语言无关，可直接映射到 Rust core + 插件宿主 |
| UI | Tauri WebView（HTML/JS） | Wox 用 Flutter、Raycast/Alfred 用 SwiftUI——三者 UI 思路不可直接移植，但 Tauri WebView 反而比 Flutter 更适合做插件 UI 渲染（天然支持 WebView 预览） |
| 跨平台目标 | macOS/Win/Linux（Tauri 原生支持） | **Wox 是唯一真正三平台对等的开源参考**，其 Wayland/Hyprland、UWP、.desktop 的跨平台踩坑经验（CHANGELOG 可见）极具参考价值 |

### 15.2 功能优先级建议（若 octopus 引入启动器能力）

按"复用 octopus 现有能力 + 用户感知价值"排序：

1. **AI 命令面板（最高价值）**——octopus 已有 LLM 润色能力（`octopus-llm`），可借鉴 **Raycast 的 AI Commands + Wox 的 AI Command（silent query hotkey + Run And Paste）**：选中文本→热键→AI 处理→原位替换。这是最小闭环、最高频场景。
2. **Query Hotkey + 多实例**——借鉴 Wox 的 per-hotkey 配置（position/width/toolbar），一个热键唤起一个命令面板。
3. **Glance / 实时信息**——octopus 可在面板展示 ASR 状态、转写历史、模型加载状态等（Wox Glance 模式）。
4. **Clipboard + 转写联动**——octopus 的剪贴板历史可直接关联"对该段音频/文本转写"动作（Universal Action 模式，借鉴 Alfred）。
5. **插件系统（可选高级）**——若开放扩展，Wox 的 WebSocket Plugin（任意语言）+ Store 模式比 Alfred Workflow 画布更轻、比 Raycast React 限定更灵活；MCP client 集成可直接复用 octopus 现有 MCP 能力。

### 15.3 应避免 / 暂缓

- **Alfred 式可视化 Workflow 画布**：实现成本极高，对 ASR 工具收益有限，建议远期。
- **独立 Snippet 自动展开系统**：与 octopus 核心场景（语音/转写）关联弱，除非定位升级为通用生产力工具。
- **Raycast 式闭源 Pro 模式**：octopus 是开源项目，商业化路径不同。

### 15.4 关键差异点（octopus 的潜在差异化）

octopus 若做启动器，其独特优势是 **ASR（语音输入）**：三者均无"语音唤起 / 语音命令"的原生能力。Wox/Raycast/Alfred 都是纯键盘驱动。octopus 可做"语音→命令"的差异化入口（说一句话触发转写/润色/搜索），这是三者都没有覆盖的蓝海。

---

## 附：信息来源

- **Wox**：本地 `CHANGELOG.md`（v2.0.0-beta.4 ~ v2.3.0）、`README.md`、`AGENTS.md`、`wox.core/`（ai/plugin/setting/ui 模块源码）、`store-plugin.json`
- **Raycast**：`raycast.com/`（主页）、`raycast.com/pro`（Pro + AI 模型清单 + Core Features 导航）、changelog
- **Alfred**：`alfredapp.com/`（首页 / Powerpack / What's New / help/features 各功能页）
