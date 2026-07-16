# 2026-07-16 设置系统 Raycast 风格改造设计

## 背景

设置窗口原有 3 套主题（light / glass-dark / nord），8 个面板各自本地定义样式组件（Card/Row/Toggle/inputClass 等），导致：
1. **样式组件重复**：Card 在 3 个面板各自定义、Toggle 有 3 种尺寸、inputClass 有 4 套 focus ring 不一致
2. **按钮变体 7 套各自为政**：primary 竟有两套（bg-foreground vs bg-voice）
3. **状态色硬编码**：Models 用 `emerald-500`/`rose-300`，不随主题切换
4. **Tab 三套并存**：反色填充 / 下划线 / voice 淡填充
5. **现成基础设施闲置**：项目已装 cva + @radix-ui/react-slot，还有孤儿 `components/ui/button.tsx` 无人引用

## 目标

基于 DESIGN.md（Raycast 设计系统），新增 raycast 主题 + 提取共享组件库收敛 8 个面板样式。**不改架构、不改数据逻辑、不改 Tauri 命令**，只收敛样式层。

## 设计决策

### 主题策略：新增而非替换
保留 light/glass-dark/nord 不动，新增第 4 套 `raycast` 主题。用户在「外观」可选。改动最小、不破坏现有体验、可回退。

### voice 色：应用图标浅蓝 `#6EB5FF`
不照搬 DESIGN.md 的 Raycast Red（#FF6363），改用 octopus 应用图标的浅蓝（≈ Raycast Blue `hsl(210,100%,71%)`）。理由：贴合 octopus 自己的品牌，落在 DESIGN.md 的交互蓝区间，比红色更适合作为常用 UI 强调色（红色在大量操作按钮中会显得警示感过强）。

### 字体：保持系统字体栈（放弃 Inter/GeistMono）
初版按 DESIGN.md 加载了 Inter + GeistMono，但**主要对象是中文**——Inter/GeistMono 不覆盖中文字符，中文 UI 文字本就 fallback 到系统字体（PingFang SC）。加载它们只影响英文/数字部分，收益小且增加 ~160KB 包体积。决定去掉，保持系统字体栈（`-apple-system`）。Raycast 风靠配色/阴影/状态色 token 实现，不依赖字体。

### 状态色 token 化（关键修复）
在 `index.css` `@theme` + 各 `[data-theme]` 注册 `--color-success`/`info`/`warning`/`destructive`（+ `-foreground`），每套主题都给值。让 Tailwind v4 生成 `text-success`/`bg-destructive` 等工具类。**修复 Models 的 emerald/rose、Agent 的 emerald/red/sky、各处 red-500 硬编码不随主题切换的问题**。

类型编码色（ActionBar TYPE_META 的 violet/sky/emerald/amber/rose/cyan）保留硬编码——它们是跨主题一致的类型辨识语义，不应随主题变。

### 共享组件库：激活闲置的 shadcn 基础设施
重写孤儿 `button.tsx`（cva 变体贴合现有用法），新增 input/card/row/toggle/badge/tabs。所有面板删本地重复定义，改用共享组件。

### Raycast 阴影（仅 raycast 主题显著）
`.raycast-ring`（双环容器）、`.raycast-btn-shadow`（按钮压感）、`.raycast-key`（键帽，仅 `[data-theme="raycast"]` 下生效）。其他主题这些 class 安全无效果。

## 不变量

1. **Tauri 命令签名不变**——所有 `invoke()` 调用、参数名、返回值结构零改动
2. **主题切换链路不变**——`get_theme_id` → `applyThemeById` → `[data-theme]` → `window_bg_hex`
3. **现有 3 套主题视觉不变**——light/glass-dark/nord 的 token 值不动，只新增状态色字段
4. **窗口尺寸不变**——保持 800×600，不动 settings_window.rs

## 降级路径

- 自定义主题 JSON 没有状态色字段 → 继承 `@theme` 默认值（light 的状态色），可接受
- 非 raycast 主题下 `.raycast-key` 等无效果 → 键帽用 `bg-surface` token 色，正常显示
- 字体加载失败 → fallback 到系统字体栈（`-apple-system` 等）

## 影响面

- 后端：仅 `theme.rs` 加 1 个 builtin ThemeInfo
- 前端：`index.css`（主题 token + 状态色 + 阴影 + 字体）、`index.html`/`theme.ts`（白名单加 raycast）、`main.tsx`（字体 import）、`components/ui/*`（共享组件库）、8 个设置面板 + ShortcutButton
- 其他窗口（Result/Clipboard/ActionBar 等）：自动继承新 token（状态色、字体），但不逐一精修
