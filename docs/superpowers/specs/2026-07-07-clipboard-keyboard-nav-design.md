# 剪贴板浮窗键盘导航设计 — 2026-07-07

> **状态**：已与用户确认设计，待写实施计划
> **日期**：2026-07-07
> **scope**：一期仅改造剪贴板浮窗的操作体验，不含 AI 命令面板 / Glance / 转写联动
> **背景调研**：[`2026-07-07-launcher-survey.md`](./2026-07-07-launcher-survey.md)（Wox 剪贴板交互范式对照）

---

## 1. 背景与动机

### 1.1 现状痛点

剪贴板浮窗唤出后（全局热键 `CmdOrCtrl+Shift+D`，`clipboard_window.rs:51`），**所有操作只能靠鼠标**：

| 操作 | 当前方式 | 痛点 |
|------|---------|------|
| 选中条目 | 鼠标点击行 | 无法用 `↑↓` 键盘移动选中 |
| 切换过滤类型（全部/语音/文本/OCR/图片/文件/收藏） | 鼠标点击 tab | 无法用键盘切换 |
| 粘贴到原应用 | 鼠标双击行 | 选中后无法 `Enter` 执行 |
| 次级动作（复制/编辑/删除/收藏/打开） | 鼠标 hover 出现按钮后点击 | 完全无键盘可达路径 |

代码现状（`crates/desktop/frontend/src/pages/Clipboard/`）：
- `index.tsx:20` 有 `selectedId` 状态，但**只由鼠标 `onSelect` 驱动**（`ClipboardItem.tsx:75` `handleClick`），无全局键盘监听
- `FilterTabs.tsx` 纯 `onClick`，无键盘处理
- `ClipboardItem.tsx:83` `handleDoubleClick` 走 `paste_clipboard_item`，但无键盘触发路径
- `SearchBar.tsx` 普通 `<input>`，方向键/Tab 均无特殊处理

### 1.2 对标

Wox（`clipboard.go:127` trigger keyword `cb`）和 Raycast 都是**全键盘驱动**：搜索框持焦，输入即过滤，方向键移动选中，回车执行默认动作，Tab 切类型。这是启动器/浮窗类应用的标准心智模型，键盘用户的基本预期。

### 1.3 一期目标

让剪贴板浮窗**完全脱离鼠标可用**，对齐 Wox/Raycast 的键盘范式。不做其他功能（AI 命令、Glance、联动均不在本期）。

---

## 2. 设计决策

### 2.1 键盘交互模型

采用**搜索框持焦模型**（Wox/Raycast 范式）：搜索框始终持焦，输入即过滤；方向键等按键通过全局监听拦截后作用于列表/tab，但焦点不离开搜索框。

### 2.2 按键映射

| 按键 | 搜索框为空 | 搜索框有内容 |
|------|-----------|-------------|
| `↑` / `↓` | 移动列表选中（上/下） | 同左 |
| `←` / `→` | 切换过滤 tab（左/右） | 归输入框光标移动（**不拦截**） |
| `Tab` / `Shift+Tab` | 切换过滤 tab（右/左） | 同左（始终可用） |
| `Enter` | 对选中条目执行默认动作 | 同左 |
| `<修饰键>+1` .. `<修饰键>+7` | 跳到第 N 个过滤 tab | 同左 |
| `Esc` | 清空搜索内容；已空则隐藏浮窗 | 清空搜索内容 |

> **修饰键可配置**：用户可在设置页「快捷键」卡片选择 `⌘ Command` / `⌃ Control` / `⌥ Option`（配置字段 `clipboard_tab_modifier`，默认 `ctrl`）。macOS Accessory 激活策略下 Cmd 可能被前一 app 菜单栏拦截；Option+数字用 `e.code`（物理键位）匹配而非 `e.key`（Option 会产生特殊字符如 `¡`）。

**不变量**：
- `Tab/Shift+Tab` **始终**切过滤 tab，与搜索框内容无关（用户提到的"兜底恒定可用"）。**含义是"仅在 7 个过滤 tab 间循环"**，不是浏览器默认的全浮窗焦点遍历——需 `preventDefault` 拦截后手动切 tab，否则焦点会跑到关闭/置顶/列表行/footer 按钮上。
- `←/→` 行为随搜索框内容动态切换：空=切 tab（与 Tab 同逻辑，方便快捷），有内容=光标移动（不拦截，让出给文字编辑）
- `↑/↓` 无条件移动选中，即使在搜索框里也拦截（用户在搜索框打字时仍可用方向键选条目）

### 2.3 Enter 默认动作

`Enter` 复用现有 `paste_clipboard_item` 命令（`clipboard_commands.rs:162`）。该命令**已是双保险**：
1. `write_item_to_clipboard`（`:191`）—— 先把条目内容写入系统剪贴板
2. `simulate_paste`（`:197`）—— 再模拟 `Cmd+V` 粘贴到原焦点应用

因此"粘贴 + 复制双保险"**后端逻辑已存在**，前端 Enter 只需调 `invoke("paste_clipboard_item", { id })`，与现有双击行为完全一致。

### 2.4 选中态与列表边界

- 列表为空时 `↑↓` 无动作
- 选中到达列表首/末条时，再按 `↑`/`↓` 停在边界（不循环）
- **过滤切换/搜索变化后**：选中态重置为**第一条**（若列表非空），否则 `null`
- 选中态需滚动跟随（选中条目滚出可视区时自动滚入，用 `scrollIntoView({ block: "nearest" })`）

### 2.5 过滤 tab 顺序与序号映射

`FilterTabs.tsx:5` 的 `TABS` 数组顺序即 `Ctrl+N` 序号映射（收藏提到第 2 位——除"全部"外最高频操作）：

> 不用 `Cmd+N`：octopus 激活策略为 `Accessory`，剪贴板浮窗显示时不切 `Regular`，前一 app 的菜单栏 key equivalent 会拦截 `Cmd+digit`。`Ctrl` 不产生特殊字符、非标准 menu equivalent、跨平台一致。

| 序号 | value | label |
|------|-------|-------|
| 1 | all | 全部 |
| 2 | favorite | 收藏 |
| 3 | asr | 语音 |
| 4 | text | 文本 |
| 5 | ocr | OCR |
| 6 | image | 图片 |
| 7 | file | 文件 |

`←/→` 和 `Tab/Shift+Tab` 按此顺序循环移动（末尾右移回首个，首个左移到末尾）。

---

## 3. 改动范围

**前端为主（4 文件）+ 配置系统（Rust + DB）**：

| 文件 | 改动 |
|------|------|
| `pages/Clipboard/index.tsx` | 全局 keydown 监听；`selectedIndex` 状态；处理全部按键；`tabModifierRef` 读配置动态选修饰键；`config-changed` 监听热更新 |
| `pages/Clipboard/SearchBar.tsx` | 无改动（搜索框持焦靠 index.tsx 统一调度）|
| `pages/Clipboard/FilterTabs.tsx` | **调整 TABS 顺序**：收藏提到第 2 位；tab 按钮加 `data-tab-index` |
| `pages/Clipboard/ClipboardItem.tsx` | 行根 div 加 `data-clip-index`；`onSelect` 改为传 index |
| `pages/Settings/GeneralPanel.tsx` | 快捷键卡片新增"剪贴TAB切换"行（修饰键下拉 + `+ 1..7` 提示）|
| `crates/infra/src/config.rs` | `AppConfig` 新增 `clipboard_tab_modifier` 字段 + default |
| `crates/infra/src/db.sql` | `app_config` seed 新增 `clipboard_tab_modifier` |
| `crates/desktop/src/settings_commands.rs` | `apply_config_value` 加 `clipboard_tab_modifier` 校验；`config-changed` emit 改为无条件 |

**配置系统 serde 重构**（实施过程中发现并根治的系统性问题，见 architecture.md）：

| 文件 | 改动 |
|------|------|
| `crates/infra/src/db.rs` | `load/save_app_config_at` 从手动逐字段枚举改为 serde 自动遍历 |
| `crates/infra/src/db.rs` | 新增 `app_config_roundtrip_all_fields` 回归测试 |

**主题系统**（借鉴 Wox，ui-ux-pro-max + frontend-design skill 设计）：

| 文件 | 改动 |
|------|------|
| `crates/desktop/src/theme.rs` | **新建**：3 套内置主题 + `list_themes` 命令 + `~/.octopus/themes/*.json` 扩展 |
| `crates/desktop/frontend/src/lib/theme.ts` | **新建**：`applyTheme` 写 CSS 变量到 `:root`，`applyThemeFromConfig` 读配置应用 |
| `crates/desktop/frontend/src/index.css` | `.theme-blur` 类（仅 clipboard_window 的 body）|
| `crates/desktop/frontend/src/main.tsx` | render 前异步加载主题防闪烁 |
| `crates/desktop/frontend/src/App.tsx` | 每窗口 mount 应用主题 + 监听 config-changed 同步 |
| `crates/desktop/frontend/src/pages/Settings/GeneralPanel.tsx` | 外观卡片 + 主题下拉（即时预览）|
| `crates/infra/src/config.rs` | `clipboard_theme` 字段 |
| `crates/infra/src/db.sql` | `clipboard_theme` seed |
| `crates/desktop/src/settings_commands.rs` | `clipboard_theme` 校验 |
| `crates/desktop/frontend/src/pages/Result/index.tsx` | `surface`/`tool-icon` 替代硬编码黑色 |
| `crates/desktop/frontend/src/pages/CompactEditor/index.tsx` | stone 硬编码改为语义 token |
| `crates/desktop/frontend/src/pages/Settings/ClipboardPanel.tsx` | stone 硬编码改为语义 token；SVG 图标加 icon-filter |
| `crates/desktop/frontend/src/pages/Settings/HistoryPanel.tsx` | stone 硬编码改为语义 token |
| `crates/desktop/frontend/src/pages/Clipboard/ClipboardItem.tsx` | SVG 预览图标加 icon-filter |
| `crates/desktop/frontend/src/pages/Screenshot/*` | 工具栏/弹窗背景+图标 filter 跟随主题 |
| `crates/desktop/frontend/src/pages/Screenshot/ScrollPreview.tsx` | 保存按钮 #3b82f6→var(--color-voice) |
| `crates/desktop/frontend/src/pages/ImagePreview/Toolbar.tsx` | 工具栏卡片/ToolButton/弹窗全面适配主题（Lucide+SVG） |

**不改**：后端命令、数据模型、`useClipboardHistory` 请求逻辑、浮窗创建/隐藏/焦点恢复链路、截图遮罩层（`rgba(0,0,0,0.5)` 功能需要）。

---

## 4. 实现要点

### 4.1 选中索引管理

`index.tsx` 当前用 `selectedId: number | null`（item.id）。键盘导航需要的是**数组索引**（才能 `items[index ± 1]`）。两种方案：

- **A. 新增 `selectedIndex: number | null` 状态**，与 `selectedId` 并存。`items` 变化时（过滤/搜索/刷新）重置为 0 或 null；渲染时 `selectedIndex === index` 传给行的 `isSelected`。
- **B. 用 `useMemo` 从 `selectedId` 反查索引**。

推荐 **A**：键盘导航以索引为第一性 citizen，`selectedId` 可在执行动作时从 `items[selectedIndex].id` 取。避免每次按键都反查。

### 4.2 全局 keydown 监听位置

在 `index.tsx` 用 `useEffect` 注册 `window.addEventListener("keydown", handler)`。handler 内根据 `search` 是否为空、`selectedIndex`、`items.length` 决定动作。

**为何不放在 SearchBar**：`←→`（空时切 tab）、`<修饰键>+N`、`Esc` 都需要作用在搜索框持焦之外的上下文；放 `index.tsx` 统一调度最清晰，且避免 SearchBar 与 index 之间的状态透传复杂度。

### 4.3 SearchBar 的方向键处理

SearchBar 内 `<input>` 的 `onKeyDown`：
- `←/→`：仅当 `value === ""` 时 `e.preventDefault()` + 通知父组件切 tab（通过 `onArrowLeft/Right` 回调或直接调 `onChange` 配合 tab 索引）
- 有内容时：**不拦截**，让浏览器默认光标移动生效
- `Tab`：**拦截**（`preventDefault`），由父组件统一处理切 tab。注意：若不拦截，浏览器默认 Tab 会在浮窗所有可聚焦元素（关闭/置顶/列表行/footer 按钮）间遍历，不符合"仅切过滤 tab"的设计。因此 Tab 也归 `index.tsx` 的全局 keydown 处理（或 SearchBar 拦截后回调父组件），不让浏览器接管。

**注意**：浮窗根 `<div>` 有 `data-tauri-drag-region`（`index.tsx:65`），需确保 drag region 不拦截键盘事件（它只影响鼠标拖拽，键盘不受影响——预期无需特殊处理）。

### 4.4 滚动跟随

选中变化时，对应行 DOM 元素 `scrollIntoView({ block: "nearest" })`。实现方式：`ClipboardItem.tsx` 加 `ref` 转发，或用 `document.querySelector` 按 data 属性定位。推荐给行加 `data-clip-index={index}`，父组件在 `selectedIndex` 变化的 `useEffect` 里 `querySelector([data-clip-index="N"]).scrollIntoView(...)`。

### 4.5 过滤切换后选中重置

`useClipboardHistory` 返回的 `items` 在 filter/search 变化后异步更新。需在 `items` 变化的 `useEffect` 里：若 `selectedIndex !== null && selectedIndex >= items.length`，重置为 `items.length > 0 ? 0 : null`。简单做法：filter/search 变化时直接 `setSelectedIndex(0)`（在 keydown handler 和 SearchBar 回调里），items 更新后若越界再夹紧。

---

## 5. 边界与降级

| 场景 | 行为 |
|------|------|
| 列表为空（无搜索结果或无历史） | `↑↓Enter` 无动作；`←→Tab <修饰键>+N Esc` 正常 |
| 选中条目后该条目被异步删除（如他处删除触发 `clipboard://changed`） | `items` 刷新后 selectedIndex 越界 → 夹紧到 0 或 null |
| 粘贴失败（`paste_clipboard_item` 返回 Err 或模拟粘贴未生效） | 后端已写剪贴板，用户可手动 `Cmd+V`；前端 catch 错误后不崩溃（与双击现有行为一致） |
| 搜索框聚焦丢失（如用户点了列表里某行） | 次级动作仍需鼠标；一期接受"次级动作只能鼠标"的现状。可选轻量优化：列表点击后自动把焦点拉回搜索框（`searchRef.current?.focus()`），但不强制 |

---

## 6. 验收标准

### 键盘导航
1. 浮窗唤出后，**不触碰鼠标**即可完成：搜索 → `↑↓` 选条目 → `Enter` 粘贴到原应用
2. 搜索框为空时 `←→` 可循环切换 7 个过滤 tab；`<修饰键>+1..7` 可直接跳转
3. 搜索框有内容时 `←→` 只移动光标不切 tab；`Tab/Shift+Tab` 仍可切 tab
4. `Esc` 在有搜索内容时清空搜索，已空时隐藏浮窗
5. `↑↓` 选中会自动滚动跟随，选中条目始终可见
6. 过滤/搜索切换后选中态重置为第一条
7. 鼠标交互（点击/双击/hover 按钮）全部保持原有行为不回归

### 主题系统
8. 设置页"外观"卡片可切换 3 套内置主题（Warm Paper / Obsidian Glass / Nord Aurora）
9. 切换后**所有窗口**（剪贴板浮窗 / 识别结果窗 / 设置页 / 编辑器 / 截图工具栏）即时跟随
10. 暗色主题下文字、图标、工具栏清晰可读（对比度 ≥4.5:1）
11. `~/.octopus/themes/*.json` 可新增自定义主题，重启后出现在下拉列表

---

## 6.5 主题系统设计

### 设计依据
- **Wox 调研**：Glass Dark 主题用半透明背景 + 原生窗口模糊（NSVisualEffectView）
- **ui-ux-pro-max §6**：文字对比度 ≥4.5:1（AA）、暗色用去饱和提亮、语义 token
- **frontend-design**："spend boldness in one place"——每套有一个辨识点

### 3 套内置主题

| 主题 | id | 设计意图 | blur | 关键色 |
|------|-----|---------|:----:|--------|
| Warm Paper | `light` | 纸质感暖灰——工具的温度感 | ❌ | bg `#fafaf9` / fg `#292524`（12.3:1）|
| Obsidian Glass | `glass-dark` | 黑曜石——致密深色 | ❌ | bg `#121216` / fg `#f5f5f7` |
| Nord Aurora | `nord` | 北极极光——冷蓝 + 极光青 | ❌ | bg `#2e3440` / fg `#e5e9f0` |

> **半透明取舍**：Wox 的半透明依赖原生窗口模糊（均匀无亮斑）。CSS `backdrop-filter` 在 Tauri WebView 下做不到均匀模糊——任何 α<1 都会透出背后白色。因此暗色主题用**纯不透明实色**，视觉差异通过颜色（暖/深/冷）而非透明度实现。

### 主题 token 体系

| token | 用途 | 亮色 | 暗色 |
|-------|------|------|------|
| `background` | 窗口主背景 | `#fafaf9` | `#121216` / `#2e3440` |
| `foreground` | 主文字 | `#292524` | `#f5f5f7` / `#e5e9f0` |
| `muted` | 次要背景（搜索框/hover）| `#f5f4f0` | `rgba(255,255,255,0.05)` |
| `muted-foreground` | 次要文字 | `#78716c` | `#9ca3af` / `#81a1c1` |
| `accent` | 选中态背景 | `#e7e5e0` | `rgba(255,255,255,0.16)` |
| `border` | 边框 | `#e7e5e0` | `rgba(255,255,255,0.08)` |
| `voice` | 强调色（语音/选中条/确认按钮）| `#d97706` | `#f59e0b` / `#88c0d0` |
| `surface` | 不透明表面（result_window/截图工具栏）| `#fafaf9` | `#1a1a1e` / `#2e3440` |
| `tool-icon` | result_window 工具栏图标色 | `rgba(0,0,0,0.55)` | `rgba(255,255,255,0.55)` |
| `icon-filter` | 截图工具栏图标 CSS filter | `none` | `brightness(0) invert(1)` |

### 应用机制

- `applyTheme(theme)`：遍历 `theme.colors` 写 `--color-xxx` 到 `:root`；`icon-filter` 单独写 `--icon-filter`（非颜色）
- Tailwind v4 的 `bg-background` / `text-foreground` 等类自动读 CSS 变量
- `App.tsx` 每窗口 mount 时 `applyThemeFromConfig()` + 监听 `config-changed` 重新应用
- `backdrop-blur` 只在 `clipboard_window` 的 body 应用（按窗口 label 判断）

### 用户扩展

`~/.octopus/themes/*.json` 格式：
```json
{
  "id": "my-theme",
  "name": "My Theme",
  "blur": false,
  "colors": {
    "background": "#...",
    "foreground": "#...",
    "muted": "...",
    "muted-foreground": "...",
    "accent": "...",
    "accent-foreground": "...",
    "border": "...",
    "voice": "...",
    "primary": "...",
    "primary-foreground": "...",
    "surface": "...",
    "tool-icon": "...",
    "icon-filter": "none"
  }
}
```
同 id 用户主题覆盖内置。

---

## 7. 不在本次范围

明确排除，避免 scope 蔓延：
- AI 命令面板（模板、silent hotkey、Run And Paste）
- Glance 实时信息条
- Clipboard 与转写/OCR/LLM 的联动（需改造 `compact_editor_window` 查看器，二期）
- 剪贴板条目的次级动作键盘快捷键（复制/编辑/删除/收藏仍鼠标驱动）
- 分组展示（今天/昨天/收藏分组）、来源应用图标、别名编辑
- 富文本/HTML/RTF 支持、剪贴板合并、密码应用忽略
- 主题编辑器（实时预览 + 颜色拾取器）、主题商店

---

## 8. 后续演进（留档，不在本期）

本期建立"搜索框持焦 + 键盘导航"基线后，后续可增量叠加：
- **二期**：查看器（`compact_editor_window`）内对图片 OCR、对音频转写、对长文 AI 摘要——联动入口在查看器侧
- **AI 命令面板**：新建独立浮窗窗口（仿 `clipboard_window`），聚合 octopus 现有 ~70 个 Tauri 命令为可发现动作，预定义 AI 模板（翻译/润色/总结）
- **Glance**：查询框实时信息条（ASR 状态/转写历史/模型加载状态）
- **上下文感知**：捕获焦点应用 + 选中文本，为 AI 命令提供上下文输入
- 应用索引、文件搜索（更远期）

架构上，本期剪贴板的键盘导航模式（全局 keydown + 搜索框持焦 + 索引管理）可直接复用到未来的命令面板浮窗。
