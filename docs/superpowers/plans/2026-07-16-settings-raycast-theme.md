# 2026-07-16 设置系统 Raycast 风格改造实施计划

> spec：[2026-07-16-settings-raycast-theme-design.md](../specs/2026-07-16-settings-raycast-theme-design.md)

## 实施记录（反映实际实现）

### Phase A：设计基础层 ✅

- [x] **A1 字体**：~~加载 Inter + GeistMono~~ **（2026-07-17 放弃）**。初版按 DESIGN.md 加载了 `@fontsource-variable/inter` + `geist-mono`，但主要对象是中文，这两个字体不覆盖中文字符（中文本就 fallback 到 PingFang SC），加载它们只影响英文/数字部分、收益小且增加 ~160KB 包体积。已回滚：`main.tsx` 删 import、`index.css` 还原系统字体栈（删 `--font-sans`/`--font-mono`/OpenType/正字距）、`vite-env.d.ts` 删 module 声明、`npm uninstall` 两个包。CSS 从 76.91KB → 73.37KB，dist 不再有 13 个 woff2（~312KB 磁盘）。
- [x] **A2 raycast 主题**：`theme.rs` builtin_themes() 加第 4 套（`#07080a` 底 / `#6EB5FF` voice / `#101111` surface）；`index.css` 加 `[data-theme="raycast"]`；`index.html` builtin 数组加 `"raycast"`；`theme.ts` BUILTIN_IDS 加 `"raycast"`。
- [x] **A3 状态色 token**：`@theme` + 4 套 `[data-theme]` 各加 `--color-success`/`info`/`warning`/`destructive`（+ `-foreground`）。raycast 用 DESIGN.md HSL 值（success `#5fc992` / info `#55b3ff` / warning `#ffbc33` / destructive `#FF6363`）。
- [x] **A4 Raycast 阴影**：`.raycast-ring`（双环容器）、`.raycast-btn-shadow`（按钮压感）、`.raycast-key`（键帽，**仅 `[data-theme="raycast"]` 下生效**——修复初版无条件应用导致浅色主题键帽变黑的 bug）。

### Phase A 关键决策（实施中发现）

- **`.raycast-key` 作用域**：初版写成无条件 `.raycast-key { background: linear-gradient... }`，会让 light 主题键帽变黑。改为 `[data-theme="raycast"] .raycast-key`，其他主题键帽用元素自身 `bg-surface` token。
- **ThemeColors struct 不加状态色字段**：权衡后保持后端 schema 不动。内置 raycast 状态色硬编码在 CSS；自定义主题 JSON 继承 `@theme` 默认值（light）。避免所有现有自定义主题反序列化走 default 路径的复杂度。

### Phase B：设置窗口外壳 ✅

- [x] Sidebar 176px，nav 项选中态改为 Raycast list 签名：左侧 voice 竖条（`w-[2px] bg-voice` 绝对定位）+ `bg-accent` 填充。
- [x] **降级**：原计划加 sidebar 搜索框（Raycast 灵魂元素），但设置窗口 nav 是 8 个固定项，搜索意义不大且引入过滤逻辑 + i18n。降级为不加，专注 nav 选中态视觉。
- [x] Toast：`bg-background/95 backdrop-blur border` 替代 `bg-black/80`。

### Phase C：共享组件库 ✅

- [x] `components/ui/button.tsx` 重写：cva variant（primary/voice/outline/ghost/destructive/destructive-ghost/success/**voice-soft**）+ size（sm/default/lg/icon/icon-sm）。`voice-soft`（`bg-voice/10 text-voice hover:bg-voice/20`）是实施中为 AsrTab/LlmTab 添加模型小按钮新增的变体。
- [x] `input.tsx`：Input/Select/Textarea + inputVariants（default/mono/bare）+ size（default/full/sm）。统一 4 套 inputClass 的 focus ring 规格（`focus:border-voice/50 focus:ring-2 focus:ring-voice/15`）。
- [x] `card.tsx`：Card/CardHeader/CardTitle/CardContent。
- [x] `row.tsx`：Row（label+hint+effect+children，统一 General/Hotword 两版）。
- [x] `toggle.tsx`：Toggle（统一 3 种尺寸为 w-10 h-[22px]，带 role=switch + aria-checked）。
- [x] `badge.tsx`：Badge（variant: muted/voice/success/destructive/outline）。
- [x] `tabs.tsx`：PillTabs/UnderlineTabs/Segmented。

### Phase C 面板迁移（8 个）✅

按 通用→复杂 顺序，每个面板「删本地组件 → import 共享 → 替换内联按钮 → 状态色换 token」：

- [x] **C1 GeneralPanel**：删本地 Card/Row/Toggle/selectClass → 共享；UnderlineTabs 替换手写下划线 Tab。**修复隐藏 bug**：原 `themes.map((t) => ...)` 的 `t` 遮蔽 i18n 的 `t`，改为 `(th)`。
- [x] **C2 SystemPanel**：本地 Card → StatCard 包装（共享 Card + 宽松 Content）；sparkline 颜色 `#6ab0f3`/`#f3a96a` → `var(--color-info)`/`var(--color-warning)`；kind badge → 共享 Badge。
- [x] **C3 PromptsPanel**：三态（列表/编辑/查看）的 X 按钮 → Button ghost icon-sm；主按钮 → Button primary；badge → 共享 Badge；textarea → 共享 Textarea bare variant。
- [x] **C4 HotwordPanel**：删本地 Card/Row/Toggle/selectClass → 共享（SectionCard 包装）；词卡加 `.raycast-ring`；按钮全换 Button（voice/outline/destructive-ghost）；`hover:text-red-500` → `hover:text-destructive`。
- [x] **C5 Models 系列**：ModelRow `border-l-emerald-500` → `border-l-success`、`bg-emerald-50/40` → `bg-success/10`、激活按钮 → Button success、CurrentBanner emerald → success；CloudModelForm 遮罩 `bg-black/30` → `bg-background/80 backdrop-blur-sm` + `.raycast-ring`、测试结果 emerald → success、inputClass/labelClass → 共享；AsrTab/LlmTab 添加按钮 → Button voice-soft；EnvironmentTab delete → Button destructive-ghost、新增行 input → 共享 Input mono；ModelsPanel Pill Tab → 共享 PillTabs。
- [x] **C6 AgentPanel**：删 inputClass → 共享 Input；Pill Tab → 共享 PillTabs；状态色 `bg-emerald-500`/`bg-red-500`/`bg-sky-500` → `bg-success`/`bg-destructive`/`bg-info`；按钮全换 Button。
- [x] **C7 ClipboardPanel**：批量删除 `bg-red-600`/`border-red-300 text-red-500` → Button destructive/destructive-ghost；加载更多 → Button outline；ClipboardRow 删除态 `bg-red-50/10`/`bg-red-500`/`text-red-600` → `bg-destructive/*` token；copied `text-emerald-500` → `text-success`；链接 `text-blue-500` → `text-info`。类型色条（amber/teal/indigo/emerald 渐变）保留（签名元素）；收藏星标 amber-400 保留（金色通用语义）。
- [x] **C8 ActionBarPanel**（最大，1222 行）：ToolBtn（ghost/solid/outline）→ Button（ghost/voice/outline）；Toggle 本地包装转发到 UIToggle；保留 inputBase（ActionBar 表单用稍大 padding px-3 py-2，focus 规格与共享对齐）；3 处清除按钮 + TreeNode 删除确认 + ScriptRunsList 状态色 + stderr 文字 → destructive/warning/success token；EditForm Cancel/Save → Button。TYPE_META 类型色（violet/sky/emerald/amber/rose/cyan）保留（跨主题一致的类型辨识）。

### Phase C 关键决策（实施中调整）

- **ActionBar 保留 inputBase**：原计划全换共享 Input，但 ActionBar 表单用 `px-3 py-2`（比共享默认 `px-2.5 py-1.5` 宽松），逐个替换风险高。改为保留 inputBase 常量但 focus 规格与共享对齐。
- **TYPE_META 类型色保留硬编码**：每种 action 类型一个色（violet=ai/sky=url/emerald=script 等），是用户辨识类型的签名元素，应跨主题一致。只 token 化「状态语义色」（成功/失败/警告），不动「类型编码色」。
- **Button 新增 voice-soft 变体**：AsrTab/LlmTab 的"添加云端模型"小按钮原是 `bg-voice/10 text-voice`（淡填充），不对应任何现有变体。新增 voice-soft 而非用 ghost+手动 className。

### Phase D：验证 ✅

- [x] `tsc -b --noEmit` → 0 error
- [x] `oxlint` → 0 error（45 warnings 全是既有非本次改动，在 ActionBar/index.tsx 等其他窗口）
- [x] `vite build` → 成功（字体按 unicode-range 分片，latin 48KB + latin-ext 85KB 等）
- [x] `vitest run` → 228 tests passed（含 systemStatusMath 26 tests）
- [x] `cargo build -p octopus-desktop` → 0 error 0 warning
- [x] 影响面追踪：`rg "light.*glass-dark.*nord"` 确认只有 theme.ts 一处主题列表（已加 raycast），index.html builtin 数组已同步

### Phase E：Azure Mist 主题 + 面板布局调整（2026-07-17 追加）✅

后续在 `feature/ui-refinement` 分支的增量改动，沿用本 spec 的主题/组件体系：

- [x] **E1 Azure Mist 主题**（第 5 套，参照 DESIGN.md「HashiCorp」明亮浅蓝）：`theme.rs builtin_themes()` 加 `azure`（background `#f6f8fb` / primary `#1563a8` 深靛蓝 / voice `#2b89ff` / muted-foreground `#656a76` DESIGN.md Dark Gray）；`index.css` 加 `[data-theme="azure"]`（状态色沿用 light）；`theme.ts BUILTIN_IDS` 加 `"azure"`。
- [x] **E2 面板去 max-w 约束**：9 个面板根容器原锁 `max-w-[640px]`/`[560px]`，窗口拉大右侧空白。移除约束让内容随 `flex-1 min-w-0` 自适应（General/Hotword/Prompts×3/System + Models 5 Tab）。
- [x] **E3 窗口默认宽度 800→960**：`settings_window.rs SETTINGS_WIDTH`，方便提示词查看。同步 architecture.md / desktop-app.md 尺寸描述。
- [x] **E4 命名调整**（zh-CN，en 不动）：nav.agent「Agent 管理」→「智能体管理」、agentPanel.title/tasksTitle 同步、nav.prompts「提示词」→「提示配方」。
- [x] **E5 去冗余页面 title**：ActionBarPanel 删页头签名色条+mono 标签（操作组左右分布：辅助操作左、新增主操作右）；PromptsPanel 列表视图删 header（保留查看/编辑上下文标题）；清理无引用死 i18n key（actionBar.editMenuItem/scriptRecords/menuManage、prompts.header、hotword.title/header/intro）。
- [x] **E6 HotwordPanel 重写为左右分栏**：原 3-Card 垂直堆叠 → 左栏 220px 场景列表（选中 voice 竖条 + toggle + 重命名 + 导出/删除 + 底部新建/导入）+ 右栏上下分区（右上 方言模糊+新增热词操作行 / 右下 搜索+排序+词卡网格+挖掘面板）。删本地 SectionCard 组件及未用 imports。
- [x] **验证**：`tsc --noEmit` 0 error；`vite build` 成功；`cargo build -p octopus-desktop --features embedded` 0 error 0 warning；`cargo test --bin octopus-desktop` 311 passed。

## 不做的事（明确边界）

- 不动任何 Tauri 命令签名、后端数据逻辑、DB schema
- 不动其他窗口（Result/Clipboard/ActionBar/Overlay/CompactEditor/Screenshot）——自动继承新 token 但不逐一精修
- 不删除 HistoryPanel.tsx 孤儿文件（超出范围）
- 不给后端 ThemeColors struct 加状态色字段（保持 schema 稳定）
