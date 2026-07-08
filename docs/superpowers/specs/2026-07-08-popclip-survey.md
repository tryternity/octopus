# PopClip 功能调研报告

> 调研对象：[PopClip](https://www.popclip.app/)（macOS 工具，v2025.9.2）
> 调研日期：2026-07-08
> 调研目的：了解 PopClip 的功能点、使用方式、交互范式、扩展机制，为 octopus 可能的产品方向提供参考
> 文档深度：功能清单级

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

## 8. 信息来源

- [popclip.app/](https://www.popclip.app/)（首页）
- [popclip.app/guide/](https://www.popclip.app/guide/)（欢迎页 + FAQ）
- [popclip.app/guide/basics](https://www.popclip.app/guide/basics)（基本操作）
- [popclip.app/guide/actions](https://www.popclip.app/guide/actions)（内置动作）
- [popclip.app/guide/settings](https://www.popclip.app/guide/settings)（设置系统）
- [popclip.app/guide/extensions](https://www.popclip.app/guide/extensions)（扩展系统）
- [popclip.app/kb/troubleshooting](https://www.popclip.app/kb/troubleshooting)（故障排除 + 兼容性）
