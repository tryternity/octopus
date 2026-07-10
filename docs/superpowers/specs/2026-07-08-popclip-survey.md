# PopClip 功能调研报告

> 调研对象：[PopClip](https://www.popclip.app/)（macOS 工具，v2025.9.2）
> 调研日期：2026-07-08
> 调研目的：了解 PopClip 的功能点、使用方式、交互范式、扩展机制，为 octopus 可能的产品方向提供参考
> 文档深度：功能清单级

> 📚 **延伸调研（2026-07-09）**：本文档聚焦 PopClip/SnipDo/OnText/Click to Do 四款"选中文本即操作"工具。后续从 `~/.tolaria/` 知识库补充扫描了 11 款相关工具（VoxFlow / eSearch / KoBar / Snow Shot / MarkerOn / Paster / Moly / tauri-nspanel / DropPoint / Wox-Raycast-Alfred / PopClip 补充），完整可借鉴点汇总见 [`2026-07-09-action-bar-related-tools-survey.md`](./2026-07-09-action-bar-related-tools-survey.md)。

---

## 1. 产品定位

**PopClip 是 macOS 上的即时文本操作工具**——选中任意文本时，自动弹出一个包含常用操作的小工具栏。

- **仅 macOS**（无 Windows / Linux / iOS 版本）
- 付费应用，有免费试用（Standalone 版独立售卖，也曾有 Setapp 版和 Mac App Store 版）
- 个人开发者（Nick Moore / Pilotmoon Software），2011 年至今持续维护
- 最新版本 2025.9.2，支持 macOS 26 Tahoe

---

## 2. 核心交互范式

### 2.1 触发方式

| 触发方式 | 说明 |
|----------|------|
| **选中文本自动出现** | 鼠标/触控板拖选文本后自动弹出（可关闭） |
| **长按出现** | 不选中文本，长按鼠标/触控板 0.5 秒触发（用于粘贴场景） |
| **键盘快捷键** | 自定义全局热键唤出（`⌃⌘P` 等），唤出后进入键盘控制模式 |
| **AppleScript** | 可被其他工具通过 AppleScript 唤出 |

### 2.2 键盘控制模式

键盘快捷键唤出后支持全键盘操作：

| 按键 | 动作 |
|------|------|
| `←→` / `↑↓` | 切换高亮动作 |
| `Enter` / `Space` | 执行高亮动作 |
| `Esc` | 关闭 PopClip |

### 2.3 消失方式

- 点击 PopClip 外部任意位置
- 按任意键（按键同时发送给下层 app）
- 鼠标移开 PopClip 区域
- 滚动

### 2.4 抑制自动出现

- 设置中完全关闭自动出现
- 设置中排除特定 app / 网站
- 选中文本时按住 `⌘` 键临时抑制

---

## 3. 内置动作（Built-in Actions）

动作是**上下文感知的**——只显示当前文本适用的动作，减少视觉杂乱。

| 动作 | 触发条件 | 修饰键行为 | 备注 |
|------|---------|-----------|------|
| **Cut / Copy / Paste** | Copy 需选中文本；Cut 需选中文本 + 可编辑区域；Paste 需剪贴板有内容 + 可编辑区域 | `⇧` = 纯文本模式（去格式） | 可设为图标或文字显示 |
| **Search** | 始终可用（有最大长度限制） | `⇧` = 后台标签页打开；`⌥` = 用备用搜索引擎 | 默认 Google（中国区默认百度）；可自定义 Search URL（`***` 占位符） |
| **Open Link** | 选中文本含 URL（`http:` / `https:` 及多种 scheme） | `⇧` = 后台打开；`⌥` = 复制 URL 列表 | 支持 `bluesky:` `craftdocs:` `evernote:` `ftp:` `hook:` `message:` `omnifocus:` `spotify:` 等 scheme；URL 缺省时自动补 `https:` |
| **Dictionary** | 选中文本是字典中的词 | `⇧` = 复制定义而非打开 | 调用 macOS Dictionary app |
| **Reveal in Finder** | 选中文本是文件/文件夹路径 | — | 如 `~/Documents` `/Applications/` |
| **Spelling** | 单词拼写错误 + 可编辑区域 | `⇧` = 复制建议而非替换 | 以子菜单形式出现；最多检查两种语言；使用 macOS 系统拼写检查器 |

---

## 4. 设置系统

### 4.1 菜单栏图标 + 状态菜单

点击菜单栏图标弹出三项菜单（On/Off 切换 + 打开设置）。

### 4.2 设置窗口三个 Tab

**General Tab**：
- Appear automatically（开关）
- Apps / Websites 排除规则
- 键盘快捷键设置（必须含 `⌃` `⌥` `⌘` 之一）
- 外观：Size（滑块）、Colour（Light/Dark/Auto）、Position（Above/Below）

**Actions Tab**：
- 列出所有内置动作 + 已安装扩展
- 每个动作可开/关（checkbox）
- 每个动作有设置按钮（齿轮图标）
- 可拖拽重排序（Spelling 固定在顶部不可移动）
- "Get Extensions" 按钮打开扩展目录

**App Tab**：
- 版本号 + 版本类型（Standalone / Setapp）
- Software Update（检查更新 / 自动更新 / Beta 版本）
- License 信息（试用倒计时 / 购买入口）
- Start at login 开关
- Show in menu bar 开关

---

## 5. 扩展系统（Extensions）

### 5.1 安装方式

| 方式 | 格式 | 说明 |
|------|------|------|
| **下载安装** | `.popclipextz` / `.popcliptxt` | 打开文件即安装；未签名扩展有安全警告 |
| **Snippet 安装** | 纯文本 | 以 `#popclip` 开头的特殊标记文本，选中后 PopClip 自动识别并提示安装 |

### 5.2 Snippet 格式示例

```
#popclip extension to search Emojipedia
name: Emojipedia
icon: search filled E
url: https://emojipedia.org/search/?q=***
```

特性：
- 纯文本，可分享在任何地方（邮件、论坛、网页）
- `#popclip` 标记是 PopClip 识别 snippet 的特殊标记
- 支持 YAML 格式定义动作

### 5.3 扩展管理

- 在 Actions Tab 配置、重排序、删除
- 扩展**不自动更新**——需手动下载新版本
- 未签名扩展运行有文件/网络访问权限的代码，需用户确认

### 5.4 开发者参考

有完整的 Extensions Developer Reference 文档，支持创建 snippet 和可下载扩展。

---

## 6. 已知兼容性问题

### 6.1 完全不兼容的 app（Can't fix）

大量 app 无法使用 PopClip，包括：
- **Adobe 全系**（Acrobat / Dreamweaver / Illustrator / InDesign）
- **JetBrains 全系**（IntelliJ / PyCharm / WebStorm 等）——不会自动出现，但键盘快捷键可唤出
- **终端类**（Alacritty / emacs / vim / MacVIM）
- **虚拟化**（Parallels Desktop / VMWare / Crossover）
- **其他**（Apple Books / Citrix / Microsoft Word 有特殊问题 / Pixelmator / Unity3D 等）

### 6.2 常见问题

- macOS 升级后 Copy 失效 → 需删除并重新添加 Accessibility 权限
- 3-finger drag 模式下出现延迟（macOS 限制）
- 自定义鼠标指针（MouseScape / Cursor Pro）会导致不可靠
- Alfred / LaunchBar 的 Clipboard Merging 功能会触发"咔哒"声（需关闭 "Fast append selected text" / "Enable ClipMerge"）
- **必须从 `/Applications` 文件夹运行**（不能在 Desktop / Downloads）

---

## 7. 与 octopus 的对比

| 维度 | PopClip | octopus |
|------|---------|---------|
| **平台** | 仅 macOS | macOS / Windows / Linux（Tauri 2） |
| **触发** | 选中文本自动弹出 | 全局热键唤起浮窗 |
| **核心场景** | 通用文本操作（剪切/复制/搜索/拼写/字典） | 语音输入 + AI 文本处理（ASR / LLM 润色 / OCR） |
| **扩展系统** | Snippet（纯文本）+ `.popclipextz`（包） | ~/.octopus/themes/*.json（主题）；AI 命令/插件（规划中） |
| **AI** | 无内置（靠扩展） | 内置 LLM（润色/翻译/摘要模板） |
| **语音** | 无 | 核心 ASR（流式/离线/云端） |
| **交互范式** | 小工具栏（鼠标驱动，选中文本触发） | 浮窗（热键驱动，剪贴板/结果窗） |
| **定价** | 付费（一次性 £29.99） | 开源免费 |

### 可借鉴点

1. **选中文本触发动作**——PopClip 的核心范式。octopus 可以考虑：用户选中文本时，在当前光标位置附近弹出迷你动作栏（翻译/润色/摘要），而非要求用户先唤起浮窗再操作。这比全局热键更自然。

2. **Snippet 纯文本扩展格式**——`#popclip` 开头的 YAML 块即可定义一个动作。极低门槛（无需开发 SDK/编译/打包）。octopus 的 AI 命令模板可以用类似格式：`#octopus\nname: 翻译\nprompt: 翻译成中文\ninput: selection`。

3. **上下文感知**——动作只显示当前适用的。octopus 的命令面板可以借鉴：选中 URL → 显示"打开/复制链接"；选中图片 → 显示"OCR/保存"；选中文本 → 显示"润色/翻译/摘要"。

4. **排除特定 app**——某些 app（Adobe/JetBrains/终端）不兼容。octopus 如果做选中文本触发，也需要排除规则。

5. **键盘控制模式**——热键唤出后全键盘操作（方向键 + Enter）。octopus 剪贴板浮窗的键盘导航已实现类似体验。

---

## 8. 同类产品对比

### 8.1 Click to Do（Windows 11 Copilot+ PC）

微软官方 2025 年推出的系统级功能，**AI 驱动**的屏幕内容操作工具。

| 维度 | 说明 |
|------|------|
| **平台** | 仅 Windows 11 Copilot+ PC（需 40 TOPS NPU + 16GB RAM） |
| **触发** | `Win + 鼠标点击` / `Win + Q` / 触摸右滑 / 截图工具入口 |
| **工作原理** | **截图 + 本地 OCR** 识别屏幕上的文本和图片（不依赖应用 API）→ 用户选择 → 弹出操作菜单 |
| **隐私** | 分析完全本地执行（Phi Silica 本地小模型 + NPU）；只有用户选择联网动作（搜索/打开网站）才发送数据 |
| **文本动作** | Copy / Open with / Search the web / Send email / Open website |
| **智能文本动作**（需英文 + ≥10 词 + MS 账号）| **Summarize** / **Create bulleted list** / **Rewrite (Casual/Formal/Refine)** / Draft with Copilot in Word / Practice in Reading Coach / Read with Immersive Reader |
| **图片动作** | Copy / Save as / Share / Open with / Visual search with Bing / Blur background / Erase objects / Remove background |
| **AI 集成** | **Ask Copilot**——选中文本/图片后直接发给 Copilot，可加自定义 prompt |
| **地区差异** | EEA/中国区部分功能受限（Summarize/Rewrite/Copilot 等在中国不可用） |

**关键差异化**：Click to Do 用**截图 + OCR** 而非 Accessibility API——所以它在**任何 app 都能工作**（包括禁用复制、不兼容 Accessibility 的 app）。但需要 Copilot+ PC 硬件（NPU）。

### 8.2 SnipDo（Windows，原 Pantherbar）

Windows 上受 PopClip 启发的免费工具。

| 维度 | 说明 |
|------|------|
| **平台** | 仅 Windows |
| **触发** | 选中文本自动弹出（与 PopClip 相同范式） |
| **内置动作** | Copy / Paste / Search / Spelling / Dictionary |
| **扩展系统** | 支持自建扩展 + 免费扩展商店 |
| **排除规则** | 可排除特定 app |
| **兼容性** | 部分应用因 Windows 限制不工作 |
| **定价** | 免费 |

### 8.3 OnText（macOS，PopClip 键盘优先替代品）

2024 年新出的 macOS 原生 Swift 应用，主打**键盘优先 + 内置 AI**。

| 维度 | 说明 |
|------|------|
| **平台** | macOS 13+（仅 Apple Silicon），原生 Swift |
| **触发** | **全局快捷键**（默认 F2），选中→按键→弹出面板。**不支持自动弹出**（开发者认为误触多） |
| **文本获取** | **Accessibility API 直读**（`{text}`，快速无痕迹）或 **模拟 Cmd+C**（`{textWithCopy}`，格式保留更好） |
| **内置动作** | Search / Copy / Translate / Character Count / Large Type + 7 种命名风格转换（camelCase/PascalCase/snake_case/kebab_case/CONSTANT_CASE/dot.case） |
| **自定义动作** | 6 种类型：URL / Shell Script / AppleScript / macOS Shortcut / Builtin / Folder（分组） |
| **占位符** | `{text}` `{textWithCopy}` `{clipboard}` `{paste}` `{prompt}` `{date}` `{time}` `{datetime}` `{weekday}` |
| **正则上下文** | ✅ 动作可设正则规则——仅当选中文本匹配时显示 |
| **动作快捷键** | ✅ 每个动作可分配快捷键，面板内键盘导航（字母/数字直接触发） |
| **Inline AI** | **内置**：ChatGPT / Gemini / Claude / Ollama。选中文本→F2→输入指令→AI 处理。`⌘R` 替换选中文本 / `⌘C` 智能复制。支持 Prompt Presets / Temperature / Max Tokens / System Prompts |
| **Action Library** | 内置预配置动作集：AI Services / Translation / Developer Tools / Productivity / Search |
| **定价** | 免费版 + Pro 版（1 许可证 3 台 Mac） |

**与 PopClip 核心差异**：
- **键盘优先** vs 鼠标优先（OnText 不自动弹出）
- **内置 AI 面板**（OnText 可在面板内直接 AI 对话+替换文本）vs PopClip 靠扩展发到外部网页
- **动作快捷键**（OnText 每个动作可设快捷键）vs PopClip 纯鼠标点击
- **扩展生态**：PopClip 218+ 扩展远超 OnText

### 8.4 四产品横向对比

| 维度 | PopClip | SnipDo | Click to Do | OnText |
|------|---------|--------|-------------|--------|
| **平台** | macOS | Windows | Win 11 Copilot+ | macOS |
| **触发** | 选中文本自动弹出 | 选中文本自动弹出 | Win+点击 / Win+Q | 全局快捷键 |
| **文本获取** | Accessibility API | Windows API | 截图+OCR（本地 NPU） | Accessibility API 或 Cmd+C |
| **AI** | 无内置（靠扩展） | 无 | **内置 Phi Silica**（本地 NPU） | **内置**（ChatGPT/Gemini/Claude/Ollama） |
| **扩展系统** | Snippet + .popclipextz（218+） | 扩展商店 | 无（系统内置） | 6 种自定义动作类型 |
| **定价** | 付费 £30 | 免费 | 系统内置（需 Copilot+ PC 硬件） | 免费 + Pro |
| **兼容性** | 30+ app 不兼容 | 部分 Windows app 限制 | **全 app 兼容**（截图+OCR 不依赖 app API） | 与 PopClip 类似 |

### 8.5 对 octopus 的额外启示

1. **Accessibility API 直读 vs 模拟 Cmd+C**——OnText 提供了两种占位符：`{text}`（Accessibility 直读，快速无痕迹但可能丢格式）和 `{textWithCopy}`（模拟 Cmd+C，格式保留但留剪贴板痕迹）。octopus 可以先做 `Cmd+C` 方案，后续 macOS 专属版加 Accessibility 直读。

2. **Click to Do 的截图+OCR 方案**——完全绕过 app 兼容性问题（任何 app 都能工作）。octopus 已有截图+OCR 能力，可以作为"选中文本失败时的 fallback"。

3. **OnText 的正则上下文**——动作可设正则规则，仅当选中文本匹配时显示。如选中 URL→显示"打开链接"；选中邮箱→显示"发邮件"。比 PopClip 的固定规则更灵活。

4. **OnText 不做自动弹出的理由**——误触（Cmd+A 全选时意外弹出）、输入冲突、需频繁 Esc 关闭。这验证了 octopus "热键触发"决策的正确性。

5. **Inline AI 替换文本**——OnText 的 `⌘R` 直接用 AI 结果替换应用中的选中文本（模拟 Cmd+V）。这就是之前讨论的 Run And Paste 方案——已有 `simulate_paste` 基础设施。

---

## 9. PopClip 扩展生态全景（15 大类，218 个扩展）

PopClip 的 218 个扩展覆盖了 15 个大类。扩展的本质模式是：**选中文本 → 通过 URL scheme / AppleScript / Shell Script / macOS Shortcut 发送到第三方应用或服务**。大部分扩展本质上就是"把选中文本塞进一个 URL 或命令"。

### 9.1 AI 工具（最新热门）

| 扩展 | 作用 |
|------|------|
| **OpenAI Chat** | 选中文本发送到 ChatGPT API，返回结果替换原文 |
| **Claude** | 选中文本发给 Anthropic Claude |
| **Grok** | 发给 xAI Grok |
| **Perplexity** | 发给 Perplexity AI 搜索 |
| **Ollama** | 发给本地 Ollama 模型 |

### 9.2 翻译与语言

| 扩展 | 作用 |
|------|------|
| **Instant Translate** | 选中文本后即时翻译（不打开浏览器，面板内直接显示，Microsoft Translator API） |
| **DeepL** | 发给 DeepL 翻译 |
| **Google Translate** | 发给 Google 翻译 |
| **Pinyin** | 中文字符转拼音 |
| **Eudic** | 欧路词典查询 |

### 9.3 笔记与知识管理

| 扩展 | 作用 |
|------|------|
| **Notion** | 选中文本创建 Notion 页面或追加内容 |
| **Obsidian** | 发送到 Obsidian vault |
| **Apple Notes** | 新建笔记或追加到已有笔记 |
| **Evernote** | 创建 Evernote 笔记 |
| **Drafts** | 发送到 Drafts app |
| **Bear / Craft / Logseq** | 发送到对应笔记应用 |
| **Stickies** | 创建桌面便利贴 |
| **Tot** | 追加到 Tot 的某个页面 |
| **Freeform** | 发送到 Freeform 白板 |

### 9.4 文本编辑

| 扩展 | 作用 |
|------|------|
| **Delete** | 删除选中的文本 |
| **Select All** | 全选后重新触发 PopClip |
| **Highlight** | 高亮选中文本（仅限 Pages/Preview/Notes/Obsidian 等支持的应用） |

### 9.5 文本格式转换

| 扩展 | 作用 |
|------|------|
| **Uppercase / Lowercase** | 全大写/全小写 |
| **Title Case** | 英文标题大写规范 |
| **Coding Cases** | camelCase / PascalCase / snake_case / kebab-case / CONSTANT_CASE |
| **Slugify** | URL 友好格式（空格→连字符，去特殊字符，小写） |
| **Quotes** | 给选中文本加引号（多种引号风格可选） |
| **Brackets** | 加括号 `()` `[]` `{}` `<>` `//` |
| **Hyphenate / Underscore** | 空格→连字符/下划线互转 |
| **Full/Half Width** | 全角/半角转换（中日韩字符） |
| **ROT13** | ROT13 加密 |
| **Alphagram** | 字母排序或打乱 |

### 9.6 开发者工具

| 扩展 | 作用 |
|------|------|
| **Base64** | Base64 编码/解码 |
| **URL Encode** | URL 百分号编码/解码 |
| **Terminal** | 选中文本作为终端命令执行（支持 Terminal/iTerm2/Warp/Ghostty/kitty） |
| **Dash** | 在 Dash 中搜索开发者文档 |
| **MDN** | 在 MDN Web Docs 搜索 |
| **GitHub** | 在 GitHub 搜索选中文本 |
| **Stack Overflow** | 搜索 Stack Overflow |
| **Unicode Lens** | 查看选中文本的 Unicode 码点 |
| **Unix Time** | Unix 时间戳 ↔ UTC 互转 |
| **HTML Encode** | HTML 实体编码/解码 |
| **Name Color** | 给十六进制颜色值一个描述性名称 |

### 9.7 搜索引擎与网站

| 扩展 | 作用 |
|------|------|
| **Google** | Google 搜索（可选国家站） |
| **Amazon** | 搜索 Amazon 商品 |
| **YouTube** | 搜索 YouTube 视频 |
| **IMDb** | 搜索电影信息 |
| **Wikipedia** | 维基百科搜索 |
| **Douban** | 豆瓣搜索（书/影/音） |
| **eBay / Etsy** | 电商平台搜索 |
| **Goodreads** | 图书搜索 |
| **DOI** | 解析学术 DOI 编号 |

### 9.8 待办与任务管理

| 扩展 | 作用 |
|------|------|
| **Things 3** | 创建 Things 任务 |
| **Todoist** | 创建 Todoist 任务 |
| **OmniFocus** | 发送到 OmniFocus 收件箱 |
| **Reminders** | 创建 macOS 提醒事项 |
| **ClickUp** | 发送到 ClickUp |
| **Due** | 创建 Due 提醒 |

### 9.9 链接处理

| 扩展 | 作用 |
|------|------|
| **Shorten** | URL 短链（支持多个短链服务） |
| **Save Link** | 保存链接到 Pocket/Instapaper/Readability 等 |
| **Open in Browser** | 在指定浏览器打开 URL（支持 Safari/Chrome/Firefox/Arc/Brave/Edge/Vivaldi 等 20+ 浏览器） |
| **IINA** | 在 IINA 播放器中打开视频 URL |
| **Downie** | 下载链接页面的视频 |
| **Leech** | 用 Leech 下载 URL 指向的文件 |

### 9.10 Markdown

| 扩展 | 作用 |
|------|------|
| **Markdown** | 给选中文本加 Markdown 格式（粗体/斜体/代码块等） |
| **HTML to Markdown** | HTML 转 Markdown |

### 9.11 实用工具

| 扩展 | 作用 |
|------|------|
| **Calculate** | 选中文本作为数学表达式求值 |
| **Convert** | 单位换算（公制↔英制：lb↔kg, °F↔°C, miles↔km 等） |
| **Large Type** | 大字显示选中文本 |
| **Word Count / Line Count** | 统计字数/行数 |
| **Say** | 语音朗读选中文本（macOS TTS） |
| **Print** | 打印选中文本 |

### 9.12 其他

- **日历**（Fantastical / Apple Calendar）
- **联系人**（搜索通讯录）
- **地图**（Apple Maps / Google Maps / OpenStreetMap）
- **社交**（LinkedIn 搜索）
- **音乐**（Spotify 搜索）
- **电话**（Call——用 iPhone 拨号）
- **剪贴板工具**（Paste / PastePal 等剪贴板管理器集成）
- **拖拽工具**（Drag & Drop 到 Dropzone / Yoink 等）

### 9.13 对 octopus 的启示

扩展的本质是**"选中文本塞进 URL/命令"**——这正是 PopClip Snippet 格式的威力（4 行 YAML 定义一个扩展）。

octopus 的 AI 命令模板可以采用同样的模式：
```
#octopus
name: 翻译成中文
prompt: 请将以下文本翻译成中文
input: selection
output: replace
```

用户用纯文本就能创建自定义 AI 动作，无需开发 SDK/编译/打包。这与 PopClip 的 Snippet 和 OnText 的 Custom Actions 理念一致——**极低门槛的扩展系统**。

---

## 10. 信息来源

- [popclip.app/](https://www.popclip.app/)（首页）
- [popclip.app/guide/](https://www.popclip.app/guide/)（欢迎页 + FAQ）
- [popclip.app/guide/basics](https://www.popclip.app/guide/basics)（基本操作）
- [popclip.app/guide/actions](https://www.popclip.app/guide/actions)（内置动作）
- [popclip.app/guide/settings](https://www.popclip.app/guide/settings)（设置系统）
- [popclip.app/guide/extensions](https://www.popclip.app/guide/extensions)（扩展系统）
- [popclip.app/kb/troubleshooting](https://www.popclip.app/kb/troubleshooting)（故障排除 + 兼容性）
- [support.microsoft.com - Click to Do](https://support.microsoft.com/en-us/windows/ai/ai-features/click-to-do-do-more-with-what-s-on-your-screen)（微软 Click to Do 官方文档）
- [snipdo-app.com](https://snipdo-app.com/)（SnipDo 官网）
- [gityeop.gumroad.com/l/ontext](https://gityeop.gumroad.com/l/ontext)（OnText Gumroad 页面）
- [gityeop.github.io/OnText](https://gityeop.github.io/OnText/docs/intro)（OnText 完整文档）
