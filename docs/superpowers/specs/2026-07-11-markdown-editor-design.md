# CompactEditor Markdown 改造设计

> 日期：2026-07-11
> 分支：`feature/markdown-editor`
> 状态：设计已确认，待编写实施计划

## 一、背景

当前 CompactEditor（`crates/desktop/frontend/src/pages/CompactEditor/index.tsx`，565 行）是纯 textarea 编辑器，支持文本编辑/语音只读/图片预览三种 tab。核心问题：

- 纯文本无 markdown 语法支持（无高亮、无预览）
- 撤销/重做走 `document.execCommand`（WKWebView 踩坑方案，不稳定）
- 手写查找替换 ~180 行，维护成本高
- 无 i18n 基础设施（全前端 ~15 文件、上百中文硬编码）

本次改造将文本/语音 tab 的 textarea 替换为 CodeMirror 6 编辑器 + markdown-it 实时预览，并搭建 i18n 基础设施。

## 二、需求决策

| # | 决策项 | 选择 | 理由 |
|---|--------|------|------|
| 1 | 视图模式 | 智能默认（可编辑→分屏、只读→预览），可手动切换编辑/分屏/预览 | 不同 source 需求不同 |
| 2 | Tab 类型 | 图片 tab 不变；文本/语音 tab 升级 CM6 | 图片预览与 markdown 编辑是独立需求 |
| 3 | 代码块高亮 | 纯 `<pre><code>` 无高亮，highlight 回调预留埋点 | 剪贴板/OCR 场景代码块频率低 |
| 4 | 渲染时机 | debounce 150ms | marka.md 验证过，预览无感知延迟 |
| 5 | 查找替换 | CM6 原生 `search()` + 工具栏保留撤销/重做按钮 | 原生成熟稳定，免维护 |
| 6 | 依赖 | 删 marked，加 markdown-it + CM6 套件 | markdown-it 扩展性更强 |
| 7 | Shiki/Mermaid/PlantUML | 仅埋点（highlight 回调 + mermaid 占位 class） | 当前不需要，预留扩展 |
| 8 | i18n 范围 | 搭基础设施 + 仅 CompactEditor 使用 | 其余页面后续独立迁移 |
| 9 | i18n 库 | 轻量自建（~60 行 t() + JSON locale） | 桌面应用无需 i18next 的复杂特性 |
| 10 | UI 语言切换 | 后端 config 新增 `ui_language` + 设置面板切换 | 与现有设置管道一致 |

## 三、整体架构

```
CompactEditor (index.tsx)  ← 外壳保留：tab 栏、后端命令、窗口管理
  ├── 图片 tab → ImagePreview（不变）
  ├── 文本 tab → MarkdownPane（新）          ← 替换 textarea
  └── 语音 tab → MarkdownPane（新, readOnly） ← 替换只读 textarea

MarkdownPane（新组件）
  ├── 工具栏（撤销/重做/字号/视图切换/清空/保存 + 字数统计）
  ├── Splitter（可拖拽分屏）
  │    ├── CodeMirrorEditor（CM6 编辑器）
  │    └── MarkdownPreview（markdown-it 渲染）
  └── 滚动同步（useSyncScroll）

i18n 基础设施（新）
  ├── lib/i18n.ts（t() 函数 + locale 加载 + React hook useT()）
  ├── locales/zh-CN.json
  └── locales/en.json
```

## 四、新增文件清单

| 文件 | 职责 | 行数估算 |
|------|------|----------|
| `lib/markdown.ts` | markdown-it 实例 + `renderMarkdown()` + highlight 埋点 | ~60 |
| `lib/i18n.ts` | `t(key)` + locale 加载 + React hook `useT()` + `initI18n()` | ~60 |
| `locales/zh-CN.json` | 中文字典（CompactEditor + 设置面板界面语言项） | ~30 |
| `locales/en.json` | 英文字典 | ~30 |
| `pages/CompactEditor/MarkdownPane.tsx` | tab 内容：工具栏 + CM6 + Preview + 内联 Splitter | ~190 |
| `pages/CompactEditor/CodeMirrorEditor.tsx` | CM6 实例封装（extensions + theme + value 同步） | ~190 |
| `pages/CompactEditor/MarkdownPreview.tsx` | 预览面板（debounce + innerHTML + 代码块复制按钮 + 链接拦截） | ~100 |
| ~~`pages/CompactEditor/Splitter.tsx`~~ | 合并入 MarkdownPane（独立组件导致视图切换时 CM6 卸载重建，改为内联 grid + display:none 切换） |
| `hooks/useSyncScroll.ts` | 双向比例滚动同步（rAF 节流 + echo 计数防回环） | ~65 |

CSS（prose 排版 + CM6 面板适配）合入现有 `index.css`。

## 五、后端改动

### 5.1 config.rs — 新增 ui_language 字段

```rust
#[serde(default = "default_ui_language")]
pub ui_language: String,

fn default_ui_language() -> String { "zh-CN".into() }
```

### 5.2 settings_commands.rs — 校验 ui_language

```rust
"ui_language" => {
    let v = value.as_str().ok_or("ui_language 需要字符串")?;
    if !["zh-CN", "en"].contains(&v) {
        return Err(format!("ui_language 非法值 '{}'（应为 zh-CN/en）", v));
    }
    cfg.ui_language = v.to_string();
}
```

### 5.3 GeneralPanel.tsx — 新增「界面语言」下拉

与现有主题选择下拉并列，选项：中文 / English。切换时 `updateConfig("ui_language", v)` → `setLocale(v)` 热更新。

### 5.4 compact_editor_window.rs — 窗口默认尺寸

- 默认尺寸 `880×620` → `1100×680`（分屏需要更宽）
- `MIN_WIDTH` `480` → `600`
- 窗口记忆逻辑不变（已有保存状态的旧用户不受影响）

## 六、模块设计

### 6.1 MarkdownPane 组件

```tsx
interface MarkdownPaneProps {
  text: string;
  readOnly: boolean;              // 语音 tab = true
  fontSize: number;               // 字号（外壳状态，跨 tab 共享）
  onFontSizeChange: (n: number) => void;  // 字号变更回调（外壳更新状态）
  onChange: (next: string) => void;       // 文本变更回调（外壳更新 tab.text）
  onClear: () => void;            // 清空回调（外壳更新 tab.text）
  onSave: () => void;             // 保存回调（外壳 doSave）
  disableSave?: boolean;          // 临时 tab 灰掉保存按钮
  savedFlash: boolean;            // 保存成功闪烁态（外壳控制，传入驱动按钮样式）
}
```

> **字号状态管理**：fontSize 存在外壳 `useState`（跨 tab 共享 + localStorage 记忆），通过 `fontSize` / `onFontSizeChange` props 下传 MarkdownPane 工具栏。MarkdownPane 卸载/重建不丢失字号。

**视图模式状态**：

```tsx
type ViewMode = 'split' | 'editor' | 'preview';
// 默认值由 readOnly 决定：可编辑 → 'split'，只读 → 'preview'
const [viewMode, setViewMode] = useState<ViewMode>(readOnly ? 'preview' : 'split');
```

**关键设计：CM6 + Preview 始终挂载，display:none 切换可见性**

视图模式切换不卸载/重建组件——用 CSS grid + `display:none` 切换 CM6 编辑器和 Preview 的可见性。CM6 实例一旦 mount 就持久存活，避免卸载重建导致的 flexbox 高度归零 + 光标丢失问题。Splitter 拖拽逻辑内联到 MarkdownPane（grid 模板列在 split/editor/preview 间动态切换）。

**布局**：

```
┌────────────────────────────────────────────────┐
│ 工具栏（flex-shrink-0）                            │
│ [撤销][重做] | [A⁻ 15 A⁺] | [清空]  flex-1  12字 │
│     | [编辑][分屏][预览] | [保存 ⌘↵]            │
├──────────────────┬─────────────────────────────┤
│   CodeMirror     │     MarkdownPreview         │
│   (CM6 编辑器)    │     (markdown-it 渲染)       │
└──────────────────┴─────────────────────────────┘
         ↑ grid 分隔线（内联 Splitter）↑
```

> **工具栏布局**：左侧为编辑操作组（撤销/重做/字号/清空），右侧为视图模式组（编辑/分屏/预览）+ 保存按钮，用 `flex-1` + 分隔线视觉隔开。

- `editor` 模式：隐藏 Preview，Editor 占满
- `split` 模式：Splitter 分屏（默认比例 0.5，localStorage 记忆）
- `preview` 模式：隐藏 Editor，Preview 占满
- 只读 tab 默认 `preview`，可手动切换查看 CM6 源码高亮
- 只读 tab 不显示 Clear 按钮、Save 按钮灰掉（`disableSave = isTemp || readOnly`），Cmd+S/Cmd+Enter 早返回——防止只读转写记录被误删或覆盖系统剪贴板

### 6.2 CodeMirrorEditor 组件

```tsx
interface CodeMirrorEditorProps {
  value: string;
  readOnly: boolean;
  fontSize: number;
  onChange: (next: string) => void;
  viewRef?: React.RefObject<EditorView | null>;
}
```

**Extensions**：

- `lineNumbers()` — 行号
- `history()` — 撤销/重做（CM6 原生，替代 execCommand）
- `drawSelection()` / `highlightActiveLine()` / `bracketMatching()`
- `syntaxHighlighting(mdHighlight)` — markdown 语法着色（HeadingStyle 定义）
- `markdown()` — `@codemirror/lang-markdown`
- `EditorView.lineWrapping` — 自动换行
- `search({ top: true })` — 原生查找/替换面板（Cmd+F 唤起）
- `keymap.of([...defaultKeymap, ...historyKeymap, ...searchKeymap, indentWithTab])`
- `EditorState.readOnly.of(readOnly)` — 只读模式
- `buildTheme(fontSize)` — 主题（CSS 变量映射到 octopus `--color-*`）

**value 同步**（借鉴 marka.md）：

- `onChangeRef` 模式：避免每次输入重建 CM 实例
- mount 时 `EditorState.create({ doc: value, ... })`，仅 mount 一次
- 外部 value 变化时，若 `current !== value` 则 dispatch changes

**撤销/重做**：工具栏按钮通过 `viewRef` 调用 CM6 `undo()`/`redo()` command（`@codemirror/commands`）。

**主题映射**：

```tsx
EditorView.theme({
  "&": { color: "var(--color-foreground)", fontSize: `${fontSize}px` },
  ".cm-scroller": { lineHeight: "1.6" },
  ".cm-gutters": { backgroundColor: "transparent", color: "var(--color-muted-foreground)", border: "none" },
  ".cm-cursor": { borderLeftColor: "var(--color-voice, #d97706)" },
  "&.cm-focused .cm-selectionBackground": { backgroundColor: "color-mix(in srgb, var(--color-voice) 22%, transparent)" },
  ".cm-panels": { backgroundColor: "var(--color-muted)", borderColor: "var(--color-border)" },
  // ... 完整映射见实现
}, { dark: false });
```

**markdown 语法高亮（HighlightStyle）**：

```tsx
const mdHighlight = HighlightStyle.define([
  { tag: t.heading1, fontSize: "1.35em", fontWeight: "600" },
  { tag: t.heading2, fontSize: "1.18em", fontWeight: "600" },
  { tag: t.strong, fontWeight: "600" },
  { tag: t.emphasis, fontStyle: "italic" },
  { tag: t.link, color: "var(--color-voice, #d97706)" },
  { tag: t.monospace, color: "var(--color-voice, #d97706)" },
  { tag: t.quote, color: "var(--color-muted-foreground)", fontStyle: "italic" },
  // ... 完整定义见实现
]);
```

### 6.3 MarkdownPreview 组件

```tsx
interface MarkdownPreviewProps {
  source: string;
  viewMode: ViewMode;  // editor 模式不渲染省 CPU
}
```

**渲染管线**：

```
source → useDebouncedValue(source, 150ms) → renderMarkdown() → articleRef.innerHTML = html → decorateCodeBlocks() → 链接拦截
```

- `renderMarkdown` 是同步函数（无 Shiki 异步加载）
- 命令式 `innerHTML`（非 `dangerouslySetInnerHTML`）——避免 React 重渲染擦除代码块复制按钮 DOM
- `decorateCodeBlocks`：给 `<pre><code>` 包裹 `.md-codeblock` div + 添加复制按钮（hover 显示）
- 链接拦截：`#anchor` 平滑滚动 / `http(s)` 走 `openUrl`（`@tauri-apps/plugin-opener`）/ 其余协议 `preventDefault` 阻止 webview 导航离开应用

### 6.4 lib/markdown.ts — markdown-it 配置

```tsx
const md = new MarkdownIt({
  html: false,
  linkify: true,
  typographer: true,
  breaks: false,
  highlight: (code, lang) => {
    // 埋点：mermaid 占位（后续渲染 SVG 的入口）
    if (lang === "mermaid") {
      return `<pre class="md-mermaid-pending"><code>${escapeHtml(code)}</code></pre>`;
    }
    // 埋点：其他语言高亮的入口（后续接 Shiki 在此返回 html）
    return "";  // 空 → markdown-it 走默认 <pre><code>
  },
});

md.use(taskLists, { enabled: false, label: true });
md.use(mark);

// GitHub 风格 heading slug（锚点跳转）
md.renderer.rules.heading_open = (tokens, idx, options, _env, self) => {
  const inline = tokens[idx + 1];
  if (inline?.type === "inline") {
    const id = slugify(inline.content);
    if (id) tokens[idx].attrSet("id", id);
  }
  return self.renderToken(tokens, idx, options);
};

export function renderMarkdown(src: string): string {
  return md.render(src);
}
```

### 6.5 Splitter 组件

```tsx
interface SplitterProps {
  left: ReactNode;
  right: ReactNode;
  ratio: number;           // 0~1
  onRatioChange: (r: number) => void;
  showRight: boolean;      // editor 模式隐藏右侧
}
```

CSS Grid 布局（`grid-template-columns: left% 1px right%`），Pointer Events + `setPointerCapture` 拖拽。比例记忆存 `localStorage`（key `compact-editor-split-ratio`）。

### 6.6 useSyncScroll hook

双向比例同步——选择器 `.md-cm-editor .cm-scroller` ↔ `.md-preview`。rAF 节流 + echo 计数防回环。`rebindKey` 为 `viewMode`（视图切换后重新绑定）。

### 6.7 i18n 基础设施（lib/i18n.ts）

**核心 API**：

- `initI18n()` — 从后端 config 读 `ui_language`，设 locale（`main.tsx` 启动时调用）
- `setLocale(locale)` — 切换语言，通知所有 `useT()` 订阅者重渲染
- `useT()` — React hook，返回 `t(key, params?)` 函数，订阅 locale 变化
- `t(key, params?)` — 非 React 上下文用（如 `decorateCodeBlocks` 内部）
- 插值：`${name}` 语法，如 `t("editor.charCount", { n: 12 })` → `"12 字"`

**字典加载**：静态 import（`zh-CN.json` / `en.json`），Tauri 离线无网络依赖。

**前端初始化时序**：

```
main.tsx → initI18n()（读 config 设 locale）→ ReactDOM.render()（首次渲染 locale 已就绪）
```

**切换语言**：`updateConfig("ui_language", v)` → 成功后 `setLocale(v)` → 所有 `useT()` 重渲染。

### 6.8 i18n key 清单

| key | zh-CN | en |
|-----|-------|-----|
| `editor.undo` | 撤销 | Undo |
| `editor.redo` | 重做 | Redo |
| `editor.fontSize` | 字号 | Font Size |
| `editor.view.split` | 分屏 | Split |
| `editor.view.editor` | 编辑 | Editor |
| `editor.view.preview` | 预览 | Preview |
| `editor.clear` | 清空 | Clear |
| `editor.clearConfirm` | 再按确认清空 | Press again to confirm |
| `editor.save` | 保存 | Save |
| `editor.saved` | 已保存 | Saved |
| `editor.charCount` | `${n} 字` | `${n} chars` |
| `editor.copyCode` | 复制 | Copy |
| `editor.copied` | 已复制 | Copied |
| `editor.previewEmpty` | 开始输入即可看到预览 | Start typing to see preview |
| `editor.switchHint` | 切换到此标签编辑 | Switch to this tab to edit |
| `editor.imageTabHint` | 切换到此标签加载图片 | Switch to this tab to load image |
| `editor.noTabs` | 没有打开的条目 | No open items |
| `tab.image` | 图片 | Image |
| `tab.empty` | 空 | Empty |
| `tab.close` | 关闭 | Close |
| `settings.uiLanguage` | 界面语言 | Interface Language |
| `settings.uiLanguage.zhCN` | 中文 | 中文 |
| `settings.uiLanguage.en` | English | English |

## 七、CompactEditor index.tsx 改动

### 7.1 移除（~180 行）

| 移除内容 | 替代方案 |
|---------|---------|
| `collectMatches` / `runFind` / `gotoMatch` / `selectRange` / `replaceOne` / `replaceAll` | CM6 `search()` |
| 查找替换状态（`showFind` / `findQuery` / `replaceQuery` / `matchIdx` / `matches`） | CM6 `search()` |
| `undo()` / `redo()`（execCommand） | MarkdownPane 内 CM6 command |
| `clearAll` / `clearPending` | 移入 MarkdownPane |
| textarea `onKeyDown`（Enter 跳转） | 移除 |
| 查找/替换 UI 条 | 移除 |
| textarea 渲染 | MarkdownPane |

### 7.2 保留（外壳核心）

- tab 管理（`loadAndAddTab` / `closeTab` / `pendingToTab` / `readInitialTabFromUrl`）
- `doSave` + `doSaveRef` + Cmd+S/Cmd+Enter 快捷键（**readOnly/isTemp tab 早返回**，不触发保存）
- 字号状态 `fontSize`（传入 MarkdownPane + `onFontSizeChange` 回调）
- `savedFlash` 保存反馈（传入 MarkdownPane 驱动按钮样式）
- mount effect（`get_pending_compact_tabs` + `listen("compact-editor://open-tab")`）

### 7.3 内容区渲染变更

```tsx
// 文本/语音 tab：仅活跃 tab 挂载 MarkdownPane（与图片 tab 懒加载策略一致）
{i === activeIdx ? (
  <MarkdownPane
    text={tab.text || ''}
    readOnly={tab.source === 'transcription'}
    fontSize={fontSize}
    onFontSizeChange={setFontSize}
    onChange={(next) => updateActiveTextAt(next, i)}
    onClear={() => updateActiveTextAt('', i)}
    onSave={doSave}
    disableSave={active?.isTemp || tab.source === 'transcription'}
    savedFlash={savedFlash}
  />
) : (
  <div className="flex-1 flex items-center justify-center text-xs text-muted-foreground">
    {t("editor.switchHint")}
  </div>
)}
```

### 7.4 CM6 实例生命周期

仅活跃 tab 挂载 CM6（与图片 tab 懒加载对称）。tab 切换时卸载旧 CM6 实例（`view.destroy()` 同步清理）、挂载新实例。text 状态由父级 `tab.text` 保持，CM6 从 `doc: text` 初始化。不在所有 tab hidden 挂载 CM6——多实例常驻内存浪费大，用户一次只编辑一个 tab。

## 八、依赖变更

### 移除

```jsonc
// 从 package.json 删除
"marked": "^18.0.5",
"@types/marked": "^5.0.2",
```

### 新增

```jsonc
// CodeMirror 6 套件
"@codemirror/commands": "^6",
"@codemirror/lang-markdown": "^6",
"@codemirror/language": "^6",
"@codemirror/search": "^6",
"@codemirror/state": "^6",
"@codemirror/view": "^6",
"codemirror": "^6",
"@lezer/highlight": "^1",  // HighlightStyle 依赖

// markdown-it 套件
"markdown-it": "^14",
"@types/markdown-it": "^14",
"markdown-it-mark": "^4",
"markdown-it-task-lists": "^2",
```

## 九、不引入的功能（YAGNI）

| 功能 | 理由 | 扩展路径 |
|------|------|---------|
| Shiki 代码高亮 | 剪贴板/OCR 场景代码块频率低 | markdown-it `highlight` 回调加 shiki 逻辑 |
| Mermaid 渲染 | 当前不需要 | highlight 回调已埋 `md-mermaid-pending` class，Preview useEffect `querySelectorAll` 渲染 |
| PlantUML | 当前不需要 | 同上模式 |
| Vim 模式 | `@replit/codemirror-vim` ~200KB，当前不需要 | CM6 Compartment 动态加载 |
| CSV 预览 | 剪贴板场景不需要 | — |
| 主题选择器 | octopus 已有主题系统 | — |
| 命令面板 / 文件树 / TOC / PDF 导出 | 与 octopus 场景不符 | — |
| 全局 i18n 迁移 | 本次仅 CompactEditor，其余页面后续独立任务 | 逐步替换硬编码为 `t()` 调用 |
| 纯预览查找 | ReadingFind 组件，当前不需要 | 后续参考 marka.md `reading-find.tsx` |

## 十、错误处理与边界情况

### 10.1 保存时序

CM6 debounce 仅影响预览渲染，不影响数据源。保存走 `onChange` 更新的 `tab.text`（即时），`doSave` 读 `active.text`。CM6 `updateListener` 每次变更同步调 `onChange`（无 debounce）。

### 10.2 只读 tab 视图切换

readOnly 由 `tab.source === 'transcription'` 固定决定，传入 CM6 `EditorState.readOnly.of(readOnly)`。视图模式可自由切换（看源码 vs 看渲染），但 readOnly 状态固定。

### 10.3 临时 tab（isTemp）

`disableSave` prop 传入 MarkdownPane，保存按钮 `disabled` + 灰样式。`doSave` 内部已有 `if (active.isTemp) return` 兜底。

### 10.4 空文本保存 = 删除

`doSave` 检测 `text.trim() === ""` → `delete_clipboard_item` → 关 tab 或关窗。此逻辑保留在外壳，不受改造影响。

### 10.5 查找替换与视图模式

- `editor` / `split` 模式：CM6 `search()` 正常工作（Cmd+F）
- `preview` 模式：CM6 不可见，Cmd+F 无目标，不触发

### 10.6 窗口尺寸迁移

用户已有保存的窗口状态用旧值（880×620），不受新默认值影响。首次使用（无记忆）的用户看到新默认值 1100×680。`MIN_WIDTH` 600 仅影响新窗口最小可拖拽宽度。

## 十一、测试策略

### 11.1 前端单元测试（vitest）

- `lib/markdown.ts`：`renderMarkdown()` 各语法元素（标题/粗斜体/链接/代码块/列表/引用/表格/task-list/mermaid 占位）
- `lib/i18n.ts`：`t()` 翻译 + 插值 + 缺 key fallback + locale 切换通知

### 11.2 前端组件测试

- `MarkdownPreview`：给定 source → 渲染正确 HTML + 代码块复制按钮存在
- `Splitter`：拖拽改变 ratio + clamping

### 11.3 后端测试

- `settings_commands.rs`：`ui_language` 校验（合法值 / 非法值）

### 11.4 手动验证

- 文本 tab：编辑 markdown → 预览实时更新（debounce 150ms）
- 语音 tab：只读预览 → 切到编辑模式看 CM6 源码高亮
- 图片 tab：功能不变
- Cmd+F 查找替换面板正常工作
- Cmd+S 保存回写 DB
- 撤销/重做按钮 + Cmd+Z/Cmd+Shift+Z
- 分屏拖拽 + 滚动同步
- 语言切换：设置面板切 English → CompactEditor 文案变英文
- 多 tab 快速切换无崩溃
