# 终端 tab 改名 + 布局切换（顶部 tabs ↔ 左侧 list）

> 内嵌终端增强（Task 7 后续）。相对原 plan 的 Task 7 是增量行为变更。

## 背景

Task 7 实现的终端窗口只有顶部 tab 栏，tab 标题固定为 `terminal.title` 或 agent 名。
用户需要：① 给 tab 自定义名字（多 agent 并行时区分会话）；② 切换为左侧 list 布局
（tab 多时顶部挤，左侧 list 纵向扩展更易扫视，对齐 VS Code 终端面板的常见形态）。

## 功能

### 1. Tab 改名

- **触发**：双击 tab 标题（tab bar 模式）或 list item 标题（sidebar 模式）→ 内联编辑。
- **确认**：Enter / 失焦 → 保存；Esc → 取消（恢复原值）。空字符串 → 回退默认标题。
- **显示优先级**：`customName`（用户改名）> `agentName`（OSC 检测）> `t("terminal.title")`。
- **持久化**：不持久化——tab 是临时会话，改名只在窗口生命周期内有效（与 cwd/pendingCommand 同生命周期）。

### 2. 布局切换（tabs ↔ sidebar）

- **切换器**：tab 栏右上角（tabs 模式）或 sidebar 底部（sidebar 模式）的图标按钮。
  - tabs 模式显示 `LayoutPanelLeft`（点击切到 sidebar）
  - sidebar 模式显示 `LayoutPanelTop`（点击切回 tabs）
- **持久化**：`localStorage("octopus-terminal-layout")` = `"tabs" | "sidebar"`，默认 `"tabs"`。
- **sidebar 布局**：
  - 左侧固定宽度 200px（可后续加拖拽调宽，Phase 1 固定）
  - 每个 item：agent 徽章 + 标题（可双击改名）+ 关闭按钮（hover 显示）
  - 激活 item 左侧 2px 强调条（比 border 更有存在感）
  - 底部：「+ 新建」按钮 + 布局切换图标
- **tabs 布局**：保持现有（顶部水平 tab 栏 + 右侧 + 按钮 + 切换图标）。

## 不变量

- 两种布局共享同一个 `Tab` 数据结构 + 同一套 TerminalPane 渲染（visibility 保活不变）。
- 布局模式不影响 PTY 生命周期 / agent 状态 / ActionBar 联动。
- 改名是纯前端状态（Tab.customName），不涉及 Rust。

## 降级

- 布局切换纯 CSS + React 条件渲染，无外部依赖，无降级路径。
- 改名 input 失败（极少）→ 浏览器原生 input 行为兜底。
