# CompactEditor Markdown 改造实施计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 将 CompactEditor 的 textarea 替换为 CodeMirror 6 + markdown-it 实时预览，并搭建轻量 i18n 基础设施。

**Architecture:** CompactEditor 外壳保留（tab 管理、后端命令、窗口管理），文本/语音 tab 内核替换为 MarkdownPane 组件（CM6 编辑器 + Splitter + 预览面板）。i18n 为轻量自建（~60 行 t() + JSON locale），后端 config 新增 `ui_language` 字段。

**Tech Stack:** CodeMirror 6（`@codemirror/*`）、markdown-it 14、Tailwind v4 CSS、React 19、Tauri 2、vitest + jsdom

## Global Constraints

- **语言**：代码注释、commit message、文档用中文
- **测试框架**：前端 vitest + jsdom（`crates/desktop/frontend/`），后端 `cargo test`
- **前端路径**：`crates/desktop/frontend/src/`
- **后端路径**：`crates/desktop/src/` + `crates/infra/src/`
- **config 写入**：后端 `apply_config_value` 校验 + 前端 `setVal("key", value)` 调 `update_config` 命令
- **CSS 变量体系**：octopus 用 `--color-foreground` / `--color-muted` / `--color-border` / `--color-voice` / `--color-background`（见 `index.css`）
- **设置面板下拉样式**：复用 `selectClass`（`GeneralPanel.tsx:86`）
- **commit 格式**：`feat:`/`fix:`/`refactor:`/`chore:`/`test:` 前缀

---

## 文件结构总览

### 新建文件

| 文件 | 职责 |
|------|------|
| `crates/desktop/frontend/src/lib/markdown.ts` | markdown-it 实例 + `renderMarkdown()` + highlight 埋点 |
| `crates/desktop/frontend/src/lib/markdown.test.ts` | markdown 渲染单测 |
| `crates/desktop/frontend/src/lib/i18n.ts` | `t()` / `useT()` / `initI18n()` / `setLocale()` |
| `crates/desktop/frontend/src/lib/i18n.test.ts` | i18n 单测 |
| `crates/desktop/frontend/src/locales/zh-CN.json` | 中文字典 |
| `crates/desktop/frontend/src/locales/en.json` | 英文字典 |
| `crates/desktop/frontend/src/hooks/useSyncScroll.ts` | 双向比例滚动同步 |
| `crates/desktop/frontend/src/pages/CompactEditor/MarkdownPane.tsx` | tab 内容（工具栏 + CM6 + Splitter + Preview） |
| `crates/desktop/frontend/src/pages/CompactEditor/CodeMirrorEditor.tsx` | CM6 实例封装 |
| `crates/desktop/frontend/src/pages/CompactEditor/MarkdownPreview.tsx` | 预览面板 |
| `crates/desktop/frontend/src/pages/CompactEditor/Splitter.tsx` | 可拖拽分屏 |

### 修改文件

| 文件 | 改动 |
|------|------|
| `crates/desktop/frontend/package.json` | 删 marked、加 CM6 + markdown-it 套件 |
| `crates/desktop/frontend/src/index.css` | 追加 prose 排版 + CM6 面板适配 CSS |
| `crates/desktop/frontend/src/main.tsx` | 追加 `initI18n()` 调用 |
| `crates/desktop/frontend/src/pages/CompactEditor/index.tsx` | 移除 textarea + 手写查找替换，改用 MarkdownPane |
| `crates/desktop/frontend/src/pages/Settings/GeneralPanel.tsx` | 新增界面语言下拉 |
| `crates/desktop/src/compact_editor_window.rs` | 默认窗口尺寸 880×620 → 1100×680、MIN_WIDTH 400 → 600 |
| `crates/infra/src/config.rs` | AppConfig 新增 `ui_language` 字段 |
| `crates/desktop/src/settings_commands.rs` | `apply_config_value` 新增 `ui_language` 分支 |

---

## Task 1: 依赖更新 + 后端 ui_language 字段

**Files:**
- Modify: `crates/desktop/frontend/package.json`
- Modify: `crates/infra/src/config.rs`
- Modify: `crates/desktop/src/settings_commands.rs`

**Interfaces:**
- Produces: `AppConfig.ui_language: String`（默认 `"zh-CN"`），供 Task 5（i18n initI18n）和 Task 8（GeneralPanel 下拉）使用

- [ ] **Step 1: 更新前端 package.json 依赖**

在 `crates/desktop/frontend/` 下执行依赖增删：

```bash
cd crates/desktop/frontend
npm uninstall marked @types/marked
npm install \
  @codemirror/commands@^6 \
  @codemirror/lang-markdown@^6 \
  @codemirror/language@^6 \
  @codemirror/search@^6 \
  @codemirror/state@^6 \
  @codemirror/view@^6 \
  codemirror@^6 \
  @lezer/highlight@^1 \
  markdown-it@^14 \
  markdown-it-mark@^4 \
  markdown-it-task-lists@^2
npm install -D @types/markdown-it@^14
```

验证 `package.json` 中 `marked` 已删除，新依赖已加入。

- [ ] **Step 2: 后端 config.rs 新增 ui_language 字段**

在 `crates/infra/src/config.rs` 的 `AppConfig` struct 中新增字段。找到 `pub language: String,` 字段所在位置，在其后追加：

```rust
    #[serde(default = "default_ui_language")]
    pub ui_language: String,
```

在文件的 `default_*` 函数区（如 `default_language` 附近）追加：

```rust
fn default_ui_language() -> String {
    "zh-CN".into()
}
```

同时在默认值构造处（`Default for AppConfig` 或 `default()` 函数中 `language: default_language()` 附近）追加：

```rust
            ui_language: default_ui_language(),
```

> 注意：搜索 `language: default_language()` 出现的每处（通常 2-3 处：默认构造、测试构造），都要在其后加 `ui_language: default_ui_language(),`。

- [ ] **Step 3: settings_commands.rs 新增 ui_language 校验**

在 `crates/desktop/src/settings_commands.rs` 的 `apply_config_value` 函数中，找到 `"language"` 分支，在其后追加新分支：

```rust
        "ui_language" => {
            let v = value.as_str().ok_or("ui_language 需要字符串")?;
            if !["zh-CN", "en"].contains(&v) {
                return Err(format!("ui_language 非法值 '{}'（应为 zh-CN/en）", v));
            }
            cfg.ui_language = v.to_string();
        }
```

- [ ] **Step 4: 后端编译验证**

```bash
cargo build -p octopus-infra -p octopus-desktop 2>&1 | tail -5
```

Expected: 编译通过，无错误。如有 `ui_language` 字段缺失的编译错误，检查 Step 2 中所有构造点是否都补了字段。

- [ ] **Step 5: 后端测试**

```bash
cargo test -p octopus-infra -- config
cargo test -p octopus-desktop -- settings_commands
```

Expected: 全部通过。如 `apply_config_valid_language` 类测试失败，仿照其模式补一个 `apply_config_valid_ui_language` 测试。

- [ ] **Step 6: 前端构建验证**

```bash
cd crates/desktop/frontend && npx tsc --noEmit 2>&1 | tail -5
```

Expected: 无类型错误（marked 类型引用已移除，新包类型尚未使用但不报错）。

- [ ] **Step 7: Commit**

```bash
git add -A
git commit -m "chore: 依赖更新（删 marked 加 CM6+markdown-it）+ 后端 ui_language 字段"
```

---

## Task 2: markdown-it 渲染模块

**Files:**
- Create: `crates/desktop/frontend/src/lib/markdown.ts`
- Create: `crates/desktop/frontend/src/lib/markdown.test.ts`

**Interfaces:**
- Produces: `renderMarkdown(src: string): string` — 同步 markdown→HTML 渲染，供 Task 6（MarkdownPreview）调用

- [ ] **Step 1: 编写 markdown.test.ts 测试文件**

```typescript
import { describe, it, expect } from "vitest";
import { renderMarkdown } from "./markdown";

describe("renderMarkdown", () => {
  it("渲染标题", () => {
    expect(renderMarkdown("# Hello")).toContain("<h1");
    expect(renderMarkdown("## Sub")).toContain("<h2");
  });

  it("渲染粗体和斜体", () => {
    expect(renderMarkdown("**bold**")).toContain("<strong>");
    expect(renderMarkdown("*italic*")).toContain("<em>");
  });

  it("渲染代码块（无高亮）", () => {
    const html = renderMarkdown("```ts\nconst x = 1;\n```");
    expect(html).toContain("<pre");
    expect(html).toContain("<code");
    expect(html).toContain("const x = 1;");
  });

  it("渲染行内代码", () => {
    expect(renderMarkdown("`code`")).toContain("<code>");
  });

  it("渲染链接", () => {
    const html = renderMarkdown("[example](https://example.com)");
    expect(html).toContain('href="https://example.com"');
    expect(html).toContain("example");
  });

  it("渲染引用块", () => {
    expect(renderMarkdown("> quote")).toContain("<blockquote>");
  });

  it("渲染无序列表", () => {
    expect(renderMarkdown("- item1\n- item2")).toContain("<ul>");
  });

  it("渲染有序列表", () => {
    expect(renderMarkdown("1. first\n2. second")).toContain("<ol>");
  });

  it("渲染 task-list", () => {
    const html = renderMarkdown("- [x] done\n- [ ] todo");
    expect(html).toContain("task-list");
  });

  it("渲染 ==高亮==（markdown-it-mark）", () => {
    expect(renderMarkdown("==highlighted==")).toContain("<mark>");
  });

  it("渲染表格", () => {
    const html = renderMarkdown("| A | B |\n|---|---|\n| 1 | 2 |");
    expect(html).toContain("<table>");
  });

  it("mermaid 占位 class（埋点）", () => {
    const html = renderMarkdown("```mermaid\ngraph TD\nA-->B\n```");
    expect(html).toContain("md-mermaid-pending");
  });

  it("heading 锚点 id（slugify）", () => {
    const html = renderMarkdown("# Hello World");
    expect(html).toContain('id="hello-world"');
  });

  it("空字符串安全渲染", () => {
    expect(renderMarkdown("")).toBe("");
  });
});
```

- [ ] **Step 2: 运行测试验证失败**

```bash
cd crates/desktop/frontend && npx vitest run src/lib/markdown.test.ts 2>&1 | tail -10
```

Expected: FAIL — 模块不存在。

- [ ] **Step 3: 实现 markdown.ts**

```typescript
import MarkdownIt from "markdown-it";
import mark from "markdown-it-mark";
import taskLists from "markdown-it-task-lists";

function escapeHtml(value: string): string {
  return value
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;");
}

function slugify(text: string): string {
  return text
    .toLowerCase()
    .replace(/[^\p{L}\p{N}\s-]/gu, "")
    .replace(/ /g, "-")
    .replace(/^-|-$/g, "");
}

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
    // 当前返回空 → markdown-it 走默认 <pre><code> 无高亮
    return "";
  },
});

md.use(taskLists, { enabled: false, label: true });
md.use(mark);

// GitHub 风格 heading slug（锚点跳转用）
md.renderer.rules.heading_open = (tokens, idx, options, _env, self) => {
  const inline = tokens[idx + 1];
  if (inline?.type === "inline") {
    const id = slugify(inline.content);
    if (id) tokens[idx].attrSet("id", id);
  }
  return self.renderToken(tokens, idx, options);
};

/** 同步渲染 markdown → HTML（无 Shiki 异步加载） */
export function renderMarkdown(src: string): string {
  return md.render(src);
}
```

- [ ] **Step 4: 运行测试验证通过**

```bash
cd crates/desktop/frontend && npx vitest run src/lib/markdown.test.ts 2>&1 | tail -10
```

Expected: 全部 PASS。

- [ ] **Step 5: Commit**

```bash
git add crates/desktop/frontend/src/lib/markdown.ts crates/desktop/frontend/src/lib/markdown.test.ts
git commit -m "feat(frontend): markdown-it 渲染模块 + highlight 埋点"
```

---

## Task 3: i18n 基础设施

**Files:**
- Create: `crates/desktop/frontend/src/lib/i18n.ts`
- Create: `crates/desktop/frontend/src/lib/i18n.test.ts`
- Create: `crates/desktop/frontend/src/locales/zh-CN.json`
- Create: `crates/desktop/frontend/src/locales/en.json`
- Modify: `crates/desktop/frontend/src/main.tsx`

**Interfaces:**
- Produces:
  - `initI18n(): Promise<void>` — 从后端读 ui_language 初始化 locale
  - `setLocale(locale: "zh-CN" | "en"): void` — 切换语言，通知订阅者
  - `useT(): (key: string, params?) => string` — React hook，订阅 locale 变化
  - `t(key: string, params?): string` — 非 React 上下文用（如 decorateCodeBlocks 内部）
  - `getLocale(): "zh-CN" | "en"` — 读当前 locale

- [ ] **Step 1: 编写 locale 字典文件**

`crates/desktop/frontend/src/locales/zh-CN.json`:

```json
{
  "editor.undo": "撤销",
  "editor.redo": "重做",
  "editor.fontSize": "字号",
  "editor.view.split": "分屏",
  "editor.view.editor": "编辑",
  "editor.view.preview": "预览",
  "editor.clear": "清空",
  "editor.clearConfirm": "再按确认清空",
  "editor.save": "保存",
  "editor.saved": "已保存",
  "editor.charCount": "${n} 字",
  "editor.placeholder.edit": "在此编辑…",
  "editor.placeholder.readonly": "语音识别记录（只读）",
  "editor.copyCode": "复制",
  "editor.copied": "已复制",
  "editor.previewEmpty": "开始输入即可看到预览",
  "editor.switchHint": "切换到此标签编辑",
  "settings.uiLanguage": "界面语言",
  "settings.uiLanguage.zhCN": "中文",
  "settings.uiLanguage.en": "English"
}
```

`crates/desktop/frontend/src/locales/en.json`:

```json
{
  "editor.undo": "Undo",
  "editor.redo": "Redo",
  "editor.fontSize": "Font Size",
  "editor.view.split": "Split",
  "editor.view.editor": "Editor",
  "editor.view.preview": "Preview",
  "editor.clear": "Clear",
  "editor.clearConfirm": "Press again to confirm",
  "editor.save": "Save",
  "editor.saved": "Saved",
  "editor.charCount": "${n} chars",
  "editor.placeholder.edit": "Start typing…",
  "editor.placeholder.readonly": "Transcription (read-only)",
  "editor.copyCode": "Copy",
  "editor.copied": "Copied",
  "editor.previewEmpty": "Start typing to see preview",
  "editor.switchHint": "Switch to this tab to edit",
  "settings.uiLanguage": "Interface Language",
  "settings.uiLanguage.zhCN": "中文",
  "settings.uiLanguage.en": "English"
}
```

- [ ] **Step 2: 编写 i18n.test.ts 测试文件**

```typescript
import { describe, it, expect, beforeEach } from "vitest";
import { setLocale, getLocale, t } from "./i18n";

describe("i18n", () => {
  beforeEach(() => {
    setLocale("zh-CN");
  });

  it("中文翻译", () => {
    expect(t("editor.undo")).toBe("撤销");
    expect(t("editor.save")).toBe("保存");
  });

  it("英文翻译", () => {
    setLocale("en");
    expect(t("editor.undo")).toBe("Undo");
    expect(t("editor.save")).toBe("Save");
  });

  it("插值", () => {
    expect(t("editor.charCount", { n: 42 })).toBe("42 字");
    setLocale("en");
    expect(t("editor.charCount", { n: 42 })).toBe("42 chars");
  });

  it("缺 key fallback 返回 key 本身", () => {
    expect(t("nonexistent.key")).toBe("nonexistent.key");
  });

  it("getLocale 反映当前 locale", () => {
    setLocale("en");
    expect(getLocale()).toBe("en");
    setLocale("zh-CN");
    expect(getLocale()).toBe("zh-CN");
  });
});
```

- [ ] **Step 3: 运行测试验证失败**

```bash
cd crates/desktop/frontend && npx vitest run src/lib/i18n.test.ts 2>&1 | tail -10
```

Expected: FAIL — 模块不存在。

- [ ] **Step 4: 实现 i18n.ts**

```typescript
import { useState, useEffect, useCallback } from "react";
import { invoke } from "@/lib/tauri";
import zhCN from "@/locales/zh-CN.json";
import en from "@/locales/en.json";

type Locale = "zh-CN" | "en";

const DICTS: Record<Locale, Record<string, string>> = {
  "zh-CN": zhCN as Record<string, string>,
  "en": en as Record<string, string>,
};

let currentLocale: Locale = "zh-CN";
const listeners = new Set<() => void>();

function translate(key: string, params?: Record<string, string | number>): string {
  const dict = DICTS[currentLocale];
  let str = dict[key] ?? key;
  if (params) {
    for (const [k, v] of Object.entries(params)) {
      str = str.replace(new RegExp(`\\$\\{${k}\\}`, "g"), String(v));
    }
  }
  return str;
}

function localeFromConfig(v?: string): Locale {
  return v === "en" ? "en" : "zh-CN";
}

/** 从后端 config 读 ui_language，初始化 locale（main.tsx 启动时调用） */
export async function initI18n(): Promise<void> {
  try {
    const config = await invoke<Record<string, unknown>>("get_config");
    setLocale(localeFromConfig(config.ui_language as string | undefined));
  } catch {
    // 后端未就绪时用默认 zh-CN
  }
}

export function setLocale(locale: Locale): void {
  if (locale === currentLocale) return;
  currentLocale = locale;
  listeners.forEach((fn) => fn());
}

export function getLocale(): Locale {
  return currentLocale;
}

/** React hook：订阅 locale 变化，返回 t 函数 */
export function useT(): (key: string, params?: Record<string, string | number>) => string {
  const [, forceUpdate] = useState({});
  useEffect(() => {
    const fn = () => forceUpdate({});
    listeners.add(fn);
    return () => {
      listeners.delete(fn);
    };
  }, []);
  return useCallback(translate, []);
}

// 非 React 上下文使用（如 decorateCodeBlocks 内部）
export const t = translate;
```

- [ ] **Step 5: 运行测试验证通过**

```bash
cd crates/desktop/frontend && npx vitest run src/lib/i18n.test.ts 2>&1 | tail -10
```

Expected: 全部 PASS。

- [ ] **Step 6: main.tsx 追加 initI18n 调用**

将 `crates/desktop/frontend/src/main.tsx` 改为 async 初始化：

```typescript
import { createRoot } from 'react-dom/client'
import './index.css'
import App from './App.tsx'
import { restoreCachedTheme } from './lib/theme'
import { initI18n } from './lib/i18n'

// 从 localStorage 同步恢复主题（零 IPC，微秒级）
restoreCachedTheme()

// 初始化 i18n（从后端 config 读 ui_language），完成后渲染
initI18n().finally(() => {
  createRoot(document.getElementById('root')!).render(<App />)
})
```

> 注意：`initI18n` 即使失败（后端未就绪）也会 `.finally()` 继续 render（用默认 zh-CN）。

- [ ] **Step 7: 类型检查 + 全量前端测试**

```bash
cd crates/desktop/frontend && npx tsc --noEmit 2>&1 | tail -5 && npx vitest run 2>&1 | tail -5
```

Expected: 无类型错误，所有测试通过。

- [ ] **Step 8: Commit**

```bash
git add crates/desktop/frontend/src/lib/i18n.ts crates/desktop/frontend/src/lib/i18n.test.ts crates/desktop/frontend/src/locales/ crates/desktop/frontend/src/main.tsx
git commit -m "feat(frontend): 轻量 i18n 基础设施 + zh-CN/en 字典"
```

---

## Task 4: CodeMirrorEditor 组件

**Files:**
- Create: `crates/desktop/frontend/src/pages/CompactEditor/CodeMirrorEditor.tsx`

**Interfaces:**
- Consumes: `fontSize: number`（来自 MarkdownPane props 传递）
- Produces: `<CodeMirrorEditor value={} readOnly={} fontSize={} onChange={} viewRef={} />`
  - `viewRef` 暴露 `EditorView` 实例，供 MarkdownPane 调 undo/redo 和 useSyncScroll 使用

- [ ] **Step 1: 实现 CodeMirrorEditor.tsx**

```tsx
import { useEffect, useRef, type RefObject } from "react";
import { Compartment, EditorState } from "@codemirror/state";
import { EditorView, keymap, lineNumbers, highlightActiveLine, drawSelection } from "@codemirror/view";
import { defaultKeymap, history, historyKeymap, indentWithTab } from "@codemirror/commands";
import { HighlightStyle, syntaxHighlighting, bracketMatching } from "@codemirror/language";
import { search, searchKeymap } from "@codemirror/search";
import { markdown } from "@codemirror/lang-markdown";
import { tags as t } from "@lezer/highlight";

// markdown 语法高亮样式
const mdHighlight = HighlightStyle.define([
  { tag: t.heading1, fontSize: "1.35em", fontWeight: "600", color: "var(--color-foreground)" },
  { tag: t.heading2, fontSize: "1.18em", fontWeight: "600", color: "var(--color-foreground)" },
  { tag: t.heading3, fontSize: "1.08em", fontWeight: "600", color: "var(--color-foreground)" },
  { tag: [t.heading4, t.heading5, t.heading6], fontWeight: "600", color: "var(--color-foreground)" },
  { tag: t.strong, fontWeight: "600", color: "var(--color-foreground)" },
  { tag: t.emphasis, fontStyle: "italic", color: "var(--color-foreground)" },
  { tag: t.strikethrough, textDecoration: "line-through", color: "var(--color-muted-foreground)" },
  { tag: t.link, color: "var(--color-voice, #d97706)", textDecoration: "none" },
  { tag: t.url, color: "var(--color-voice, #d97706)" },
  { tag: t.quote, color: "var(--color-muted-foreground)", fontStyle: "italic" },
  { tag: t.monospace, color: "var(--color-voice, #d97706)" },
  { tag: t.list, color: "var(--color-voice, #d97706)" },
  { tag: t.contentSeparator, color: "var(--color-muted-foreground)" },
  { tag: t.meta, color: "var(--color-muted-foreground)" },
  { tag: t.processingInstruction, color: "var(--color-muted-foreground)" },
]);

function buildTheme(fontSize: number) {
  return EditorView.theme(
    {
      "&": {
        height: "100%",
        backgroundColor: "transparent",
        color: "var(--color-foreground)",
        fontFamily: "ui-monospace, SFMono-Regular, Menlo, monospace",
        fontSize: `${fontSize}px`,
      },
      ".cm-scroller": {
        fontFamily: "ui-monospace, SFMono-Regular, Menlo, monospace",
        lineHeight: "1.6",
        padding: "16px 20px 60px 0",
      },
      ".cm-content": {
        caretColor: "var(--color-voice, #d97706)",
        paddingLeft: "10px",
      },
      ".cm-cursor, .cm-dropCursor": {
        borderLeftColor: "var(--color-voice, #d97706)",
        borderLeftWidth: "1.5px",
      },
      ".cm-gutters": {
        backgroundColor: "transparent",
        color: "var(--color-muted-foreground)",
        border: "none",
        fontFamily: "ui-monospace, SFMono-Regular, Menlo, monospace",
        fontSize: "11px",
        margin: "0",
        padding: "0",
        boxShadow: "none",
      },
      ".cm-lineNumbers": {
        boxSizing: "border-box",
        borderRight: "1px solid var(--color-border)",
        minWidth: "36px",
        width: "36px",
      },
      ".cm-lineNumbers .cm-gutterElement": {
        boxSizing: "border-box",
        minWidth: "36px",
        padding: "0 8px 0 0",
        textAlign: "right",
      },
      ".cm-activeLine": { backgroundColor: "transparent" },
      ".cm-activeLineGutter": { backgroundColor: "transparent", color: "var(--color-foreground)" },
      "&.cm-focused": { outline: "none" },
      "&.cm-focused .cm-selectionBackground, ::selection, .cm-selectionBackground": {
        backgroundColor: "color-mix(in srgb, var(--color-voice, #d97706) 22%, transparent)",
      },
      ".cm-line": { padding: "0" },
      // CM6 search 面板样式适配
      ".cm-panels": {
        backgroundColor: "var(--color-muted)",
        color: "var(--color-foreground)",
        borderColor: "var(--color-border)",
      },
      ".cm-panels.cm-panels-top": { borderBottom: "1px solid var(--color-border)" },
      ".cm-textfield": {
        background: "var(--color-background)",
        color: "var(--color-foreground)",
        border: "1px solid var(--color-border)",
        borderRadius: "4px",
        padding: "3px 8px",
        fontFamily: "ui-monospace, monospace",
        fontSize: "12px",
        outline: "none",
      },
      ".cm-button": {
        background: "var(--color-background)",
        color: "var(--color-foreground)",
        border: "1px solid var(--color-border)",
        borderRadius: "4px",
        padding: "3px 8px",
        fontFamily: "inherit",
        fontSize: "11px",
        cursor: "pointer",
        backgroundImage: "none",
      },
      ".cm-searchMatch": {
        backgroundColor: "color-mix(in srgb, var(--color-voice, #d97706) 22%, transparent)",
        borderRadius: "2px",
      },
      ".cm-searchMatch-selected": {
        backgroundColor: "color-mix(in srgb, var(--color-voice, #d97706) 45%, transparent)",
      },
    },
    { dark: false },
  );
}

interface CodeMirrorEditorProps {
  value: string;
  readOnly: boolean;
  fontSize: number;
  onChange: (next: string) => void;
  viewRef?: RefObject<EditorView | null>;
}

export function CodeMirrorEditor({ value, readOnly, fontSize, onChange, viewRef: externalViewRef }: CodeMirrorEditorProps) {
  const hostRef = useRef<HTMLDivElement>(null);
  const viewRef = useRef<EditorView | null>(null);
  const onChangeRef = useRef(onChange);
  onChangeRef.current = onChange;
  // fontSize 动态切换用 Compartment，避免重建实例
  const themeCompartment = useRef(new Compartment());

  // mount 时创建 CM6 实例（仅一次）
  useEffect(() => {
    if (!hostRef.current) return;

    const state = EditorState.create({
      doc: value,
      extensions: [
        lineNumbers(),
        history(),
        drawSelection(),
        highlightActiveLine(),
        bracketMatching(),
        syntaxHighlighting(mdHighlight, { fallback: true }),
        markdown(),
        EditorView.lineWrapping,
        search({ top: true }),
        keymap.of([...defaultKeymap, ...historyKeymap, ...searchKeymap, indentWithTab]),
        EditorState.readOnly.of(readOnly),
        themeCompartment.current.of(buildTheme(fontSize)),
        EditorView.updateListener.of((update) => {
          if (update.docChanged) {
            onChangeRef.current(update.state.doc.toString());
          }
        }),
      ],
    });

    const view = new EditorView({ state, parent: hostRef.current });
    viewRef.current = view;
    if (externalViewRef) externalViewRef.current = view;

    return () => {
      view.destroy();
      viewRef.current = null;
      if (externalViewRef) externalViewRef.current = null;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  // 外部 value 变化时同步（仅当 != 内部值，防光标跳）
  useEffect(() => {
    const view = viewRef.current;
    if (!view) return;
    const current = view.state.doc.toString();
    if (current !== value) {
      view.dispatch({ changes: { from: 0, to: current.length, insert: value } });
    }
  }, [value]);

  // fontSize 变化时重配置主题（不重建实例）
  useEffect(() => {
    const view = viewRef.current;
    if (!view) return;
    view.dispatch({ effects: themeCompartment.current.reconfigure(buildTheme(fontSize)) });
  }, [fontSize]);

  return <div ref={hostRef} className="md-cm-editor" style={{ height: "100%", width: "100%", overflow: "hidden" }} />;
}
```

- [ ] **Step 2: 类型检查**

```bash
cd crates/desktop/frontend && npx tsc --noEmit 2>&1 | tail -5
```

Expected: 无类型错误。

- [ ] **Step 3: Commit**

```bash
git add crates/desktop/frontend/src/pages/CompactEditor/CodeMirrorEditor.tsx
git commit -m "feat(frontend): CodeMirror 6 编辑器封装 + markdown 语法高亮 + 主题映射"
```

---

## Task 5: MarkdownPreview + Splitter + useSyncScroll

**Files:**
- Create: `crates/desktop/frontend/src/pages/CompactEditor/MarkdownPreview.tsx`
- Create: `crates/desktop/frontend/src/pages/CompactEditor/Splitter.tsx`
- Create: `crates/desktop/frontend/src/hooks/useSyncScroll.ts`

**Interfaces:**
- Consumes: `renderMarkdown(src: string): string` from Task 2, `t(key)` from Task 3
- Produces:
  - `<MarkdownPreview source={} />`
  - `<Splitter left={} right={} ratio={} onRatioChange={} showRight={} />`
  - `useSyncScroll({ editorSelector, previewSelector, rebindKey })`

- [ ] **Step 1: 实现 useSyncScroll.ts**

```typescript
import { useEffect } from "react";

type Options = {
  editorSelector?: string;
  previewSelector?: string;
  rebindKey?: unknown;
};

/**
 * 比例双向滚动同步。rAF 节流 + echo 计数防回环。
 * 借鉴 marka.md use-sync-scroll.ts。
 */
export function useSyncScroll({
  editorSelector = ".md-cm-editor .cm-scroller",
  previewSelector = ".md-preview",
  rebindKey,
}: Options = {}): void {
  useEffect(() => {
    let editor: HTMLElement | null = null;
    let preview: HTMLElement | null = null;
    let rafId: number | undefined;
    const echo = { editor: 0, preview: 0 };

    const makeSync = (
      src: HTMLElement,
      dst: HTMLElement,
      srcKey: "editor" | "preview",
      dstKey: "editor" | "preview",
    ) => {
      let pending = false;
      return () => {
        if (echo[srcKey] > 0) {
          echo[srcKey] -= 1;
          return;
        }
        if (pending) return;
        pending = true;
        requestAnimationFrame(() => {
          pending = false;
          const srcRange = src.scrollHeight - src.clientHeight;
          const dstRange = dst.scrollHeight - dst.clientHeight;
          if (srcRange <= 0 || dstRange <= 0) return;
          const ratio = src.scrollTop / srcRange;
          const target = ratio * dstRange;
          if (Math.abs(dst.scrollTop - target) < 1) return;
          echo[dstKey] += 1;
          dst.scrollTop = target;
        });
      };
    };

    let onEditor: (() => void) | undefined;
    let onPreview: (() => void) | undefined;

    const tryAttach = () => {
      editor = document.querySelector<HTMLElement>(editorSelector);
      preview = document.querySelector<HTMLElement>(previewSelector);
      if (!editor || !preview) {
        rafId = requestAnimationFrame(tryAttach);
        return;
      }
      onEditor = makeSync(editor, preview, "editor", "preview");
      onPreview = makeSync(preview, editor, "preview", "editor");
      editor.addEventListener("scroll", onEditor, { passive: true });
      preview.addEventListener("scroll", onPreview, { passive: true });
    };

    tryAttach();

    return () => {
      if (rafId != null) cancelAnimationFrame(rafId);
      if (editor && onEditor) editor.removeEventListener("scroll", onEditor);
      if (preview && onPreview) preview.removeEventListener("scroll", onPreview);
    };
  }, [editorSelector, previewSelector, rebindKey]);
}
```

- [ ] **Step 2: 实现 Splitter.tsx**

```tsx
import { useCallback, useEffect, useRef, type ReactNode } from "react";

interface SplitterProps {
  left: ReactNode;
  right: ReactNode;
  ratio: number;
  onRatioChange: (r: number) => void;
  showRight: boolean;
}

const MIN_RATIO = 0.2;
const MAX_RATIO = 0.8;

export function Splitter({ left, right, ratio, onRatioChange, showRight }: SplitterProps) {
  const containerRef = useRef<HTMLDivElement>(null);
  const draggingRef = useRef(false);

  const onPointerDown = useCallback((e: React.PointerEvent<HTMLDivElement>) => {
    draggingRef.current = true;
    (e.target as HTMLElement).setPointerCapture(e.pointerId);
    document.body.style.cursor = "col-resize";
    document.body.style.userSelect = "none";
  }, []);

  const onPointerMove = useCallback(
    (e: React.PointerEvent<HTMLDivElement>) => {
      if (!draggingRef.current || !containerRef.current) return;
      const rect = containerRef.current.getBoundingClientRect();
      const next = (e.clientX - rect.left) / rect.width;
      const clamped = Math.min(MAX_RATIO, Math.max(MIN_RATIO, next));
      onRatioChange(clamped);
    },
    [onRatioChange],
  );

  const stopDrag = useCallback((e: React.PointerEvent<HTMLDivElement>) => {
    if (!draggingRef.current) return;
    draggingRef.current = false;
    try {
      (e.target as HTMLElement).releasePointerCapture(e.pointerId);
    } catch {
      // pointer may already be released
    }
    document.body.style.cursor = "";
    document.body.style.userSelect = "";
  }, []);

  useEffect(() => {
    return () => {
      document.body.style.cursor = "";
      document.body.style.userSelect = "";
    };
  }, []);

  if (!showRight) {
    return <div className="flex-1 flex flex-col min-h-0 min-w-0 overflow-hidden">{left}</div>;
  }

  const leftPct = `${ratio * 100}%`;
  const rightPct = `${(1 - ratio) * 100}%`;

  return (
    <div
      ref={containerRef}
      className="flex-1 grid min-h-0"
      style={{ gridTemplateColumns: `${leftPct} 1px ${rightPct}` }}
    >
      <div className="relative min-w-0 min-h-0 flex flex-col overflow-hidden">{left}</div>
      <div
        role="separator"
        aria-orientation="vertical"
        aria-valuenow={Math.round(ratio * 100)}
        onPointerDown={onPointerDown}
        onPointerMove={onPointerMove}
        onPointerUp={stopDrag}
        onPointerCancel={stopDrag}
        className="bg-border cursor-col-resize select-none hover:bg-voice transition-colors"
      >
        <div className="absolute inset-y-0 -inset-x-[5px]" />
      </div>
      <div className="relative min-w-0 min-h-0 flex flex-col overflow-hidden">{right}</div>
    </div>
  );
}
```

- [ ] **Step 3: 实现 MarkdownPreview.tsx**

```tsx
import { useEffect, useRef, useState } from "react";
import { renderMarkdown } from "@/lib/markdown";
import { t } from "@/lib/i18n";

interface MarkdownPreviewProps {
  source: string;
}

// debounce hook（内联，避免新文件）
function useDebounced<T>(value: T, delayMs: number): T {
  const [debounced, setDebounced] = useState(value);
  useEffect(() => {
    const id = window.setTimeout(() => setDebounced(value), delayMs);
    return () => window.clearTimeout(id);
  }, [value, delayMs]);
  return debounced;
}

/** 给代码块包裹容器 + 添加复制按钮 */
function decorateCodeBlocks(root: HTMLElement): () => void {
  const cleanups: Array<() => void> = [];
  const codeEls = root.querySelectorAll("pre > code");
  codeEls.forEach((code) => {
    const pre = code.parentElement;
    if (!pre || pre.parentElement?.classList.contains("md-codeblock")) return;

    const wrapper = document.createElement("div");
    wrapper.className = "md-codeblock";
    pre.parentNode?.insertBefore(wrapper, pre);
    wrapper.appendChild(pre);

    const btn = document.createElement("button");
    btn.type = "button";
    btn.className = "md-copy-btn";
    btn.textContent = t("editor.copyCode");
    const onClick = async () => {
      try {
        await navigator.clipboard.writeText(code.textContent ?? "");
      } catch {
        // WKWebView clipboard 可能受限，忽略
      }
      btn.textContent = t("editor.copied");
      window.setTimeout(() => { btn.textContent = t("editor.copyCode"); }, 1400);
    };
    btn.addEventListener("click", onClick);
    wrapper.appendChild(btn);
    cleanups.push(() => btn.removeEventListener("click", onClick));
  });
  return () => cleanups.forEach((fn) => fn());
}

export function MarkdownPreview({ source }: MarkdownPreviewProps) {
  const debouncedSource = useDebounced(source, 150);
  const [html, setHtml] = useState("");
  const articleRef = useRef<HTMLElement>(null);

  // 渲染 markdown → HTML
  useEffect(() => {
    setHtml(renderMarkdown(debouncedSource));
  }, [debouncedSource]);

  // 命令式 innerHTML（非 dangerouslySetInnerHTML，避免 React 重渲染擦除 DOM 装饰）
  useEffect(() => {
    if (!articleRef.current) return;
    articleRef.current.innerHTML = html;
    const cleanup = decorateCodeBlocks(articleRef.current);
    return cleanup;
  }, [html]);

  // 链接拦截：锚点滚动 / 外部链接阻止（Tauri 环境下无浏览器导航）
  useEffect(() => {
    const article = articleRef.current;
    if (!article) return;
    const onClick = (e: MouseEvent) => {
      const a = (e.target as HTMLElement).closest("a");
      if (!a) return;
      const href = a.getAttribute("href");
      if (!href) return;
      if (href.startsWith("#")) {
        e.preventDefault();
        const id = decodeURIComponent(href.slice(1));
        const target = article.querySelector(`[id="${CSS.escape(id)}"]`);
        target?.scrollIntoView({ behavior: "smooth", block: "start" });
      } else if (/^https?:\/\//.test(href)) {
        // 阻止 WebView 内导航——外部链接由用户自行复制或后续接入 opener
        e.preventDefault();
      }
    };
    article.addEventListener("click", onClick);
    return () => article.removeEventListener("click", onClick);
  }, [html]);

  if (source.trim().length === 0) {
    return (
      <div className="md-preview flex-1 flex items-center justify-center">
        <span className="text-sm text-muted-foreground">{t("editor.previewEmpty")}</span>
      </div>
    );
  }

  return (
    <div className="md-preview flex-1 overflow-auto p-5" style={{ userSelect: "text" }}>
      <article ref={articleRef} className="md-prose" />
    </div>
  );
}
```

- [ ] **Step 4: 类型检查**

```bash
cd crates/desktop/frontend && npx tsc --noEmit 2>&1 | tail -5
```

Expected: 无类型错误。

- [ ] **Step 5: Commit**

```bash
git add crates/desktop/frontend/src/pages/CompactEditor/MarkdownPreview.tsx crates/desktop/frontend/src/pages/CompactEditor/Splitter.tsx crates/desktop/frontend/src/hooks/useSyncScroll.ts
git commit -m "feat(frontend): MarkdownPreview + Splitter + useSyncScroll"
```

---

## Task 6: MarkdownPane 组件（工具栏 + CM6 + Splitter + Preview 组合）

**Files:**
- Create: `crates/desktop/frontend/src/pages/CompactEditor/MarkdownPane.tsx`

**Interfaces:**
- Consumes: `CodeMirrorEditor`（Task 4）、`MarkdownPreview`（Task 5）、`Splitter`（Task 5）、`useSyncScroll`（Task 5）、`useT()`（Task 3）
- Produces: `<MarkdownPane text={} readOnly={} fontSize={} onFontSizeChange={} onChange={} onClear={} onSave={} disableSave={} savedFlash={} />`

- [ ] **Step 1: 实现 MarkdownPane.tsx**

```tsx
import { useState, useRef, useCallback, useEffect } from "react";
import type { EditorView } from "@codemirror/view";
import { undo, redo } from "@codemirror/commands";
import { Undo2, Redo2, ZoomIn, ZoomOut, Eraser, Check, Save, Eye, Type, Columns2, FileText } from "lucide-react";
import { CodeMirrorEditor } from "./CodeMirrorEditor";
import { MarkdownPreview } from "./MarkdownPreview";
import { Splitter } from "./Splitter";
import { useSyncScroll } from "@/hooks/useSyncScroll";
import { useT } from "@/lib/i18n";

type ViewMode = "split" | "editor" | "preview";

const FONT_MIN = 12;
const FONT_MAX = 24;
const SPLIT_KEY = "compact-editor-split-ratio";

interface MarkdownPaneProps {
  text: string;
  readOnly: boolean;
  fontSize: number;
  onFontSizeChange: (n: number) => void;
  onChange: (next: string) => void;
  onClear: () => void;
  onSave: () => void;
  disableSave?: boolean;
  savedFlash: boolean;
}

const ToolBtn = ({ onClick, title, disabled, children }: {
  onClick: () => void; title: string; disabled?: boolean; children: React.ReactNode;
}) => (
  <button
    type="button"
    disabled={disabled}
    title={title}
    onClick={onClick}
    className="p-1.5 rounded-md text-muted-foreground hover:bg-accent hover:text-foreground disabled:opacity-30 disabled:hover:bg-transparent transition-colors"
  >{children}</button>
);

export function MarkdownPane({
  text, readOnly, fontSize, onFontSizeChange, onChange, onClear, onSave, disableSave, savedFlash,
}: MarkdownPaneProps) {
  const t = useT();
  const [viewMode, setViewMode] = useState<ViewMode>(readOnly ? "preview" : "split");
  const [clearPending, setClearPending] = useState(false);
  const [splitRatio, setSplitRatio] = useState(() => {
    const saved = Number(localStorage.getItem(SPLIT_KEY));
    return saved >= 0.2 && saved <= 0.8 ? saved : 0.5;
  });
  const viewRef = useRef<EditorView | null>(null);

  useEffect(() => {
    localStorage.setItem(SPLIT_KEY, String(splitRatio));
  }, [splitRatio]);

  // 滚动同步（仅 split 模式）
  useSyncScroll({ rebindKey: viewMode });

  const handleUndo = useCallback(() => {
    if (viewRef.current) undo(viewRef.current);
  }, []);
  const handleRedo = useCallback(() => {
    if (viewRef.current) redo(viewRef.current);
  }, []);

  const handleClear = () => {
    if (!clearPending) {
      setClearPending(true);
      window.setTimeout(() => setClearPending(false), 2000);
      return;
    }
    onClear();
    setClearPending(false);
  };

  const charCount = [...text].length;
  const showRight = viewMode !== "editor";
  const showLeft = viewMode !== "preview";

  return (
    <div className="flex flex-col flex-1 min-h-0">
      {/* 工具栏 */}
      <div className="flex-shrink-0 flex items-center gap-0.5 px-2 py-1.5 border-b border-border bg-muted">
        <ToolBtn onClick={handleUndo} title={t("editor.undo")}><Undo2 className="w-4 h-4" /></ToolBtn>
        <ToolBtn onClick={handleRedo} title={t("editor.redo")}><Redo2 className="w-4 h-4" /></ToolBtn>
        <span className="w-px h-4 bg-border mx-1" />
        <ToolBtn onClick={() => onFontSizeChange(Math.max(FONT_MIN, fontSize - 1))} title={t("editor.fontSize")} disabled={fontSize <= FONT_MIN}>
          <ZoomOut className="w-4 h-4" />
        </ToolBtn>
        <span className="text-[11px] text-muted-foreground w-7 text-center tabular-nums">{fontSize}</span>
        <ToolBtn onClick={() => onFontSizeChange(Math.min(FONT_MAX, fontSize + 1))} title={t("editor.fontSize")} disabled={fontSize >= FONT_MAX}>
          <ZoomIn className="w-4 h-4" />
        </ToolBtn>
        <span className="w-px h-4 bg-border mx-1" />
        {/* 视图模式切换 */}
        <ToolBtn onClick={() => setViewMode("editor")} title={t("editor.view.editor")} disabled={viewMode === "editor"}>
          <FileText className="w-4 h-4" />
        </ToolBtn>
        <ToolBtn onClick={() => setViewMode("split")} title={t("editor.view.split")} disabled={viewMode === "split"}>
          <Columns2 className="w-4 h-4" />
        </ToolBtn>
        <ToolBtn onClick={() => setViewMode("preview")} title={t("editor.view.preview")} disabled={viewMode === "preview"}>
          <Eye className="w-4 h-4" />
        </ToolBtn>
        <span className="w-px h-4 bg-border mx-1" />
        <ToolBtn onClick={handleClear} title={clearPending ? t("editor.clearConfirm") : t("editor.clear")}>
          {clearPending ? <Check className="w-4 h-4 text-red-500" /> : <Eraser className="w-4 h-4" />}
        </ToolBtn>
        <div className="flex-1" />
        <span className="text-[11px] text-muted-foreground mr-2 tabular-nums">
          {t("editor.charCount", { n: charCount })}
        </span>
        <button
          type="button"
          disabled={disableSave}
          onClick={onSave}
          className={`flex items-center gap-1 px-2.5 py-1 rounded-md text-xs transition-colors ${
            disableSave
              ? "bg-muted text-muted-foreground cursor-not-allowed"
              : savedFlash ? "bg-emerald-600 text-white" : "bg-[#007aff] hover:bg-[#0066d6] text-white"
          }`}
        >
          {savedFlash ? <Check className="w-3.5 h-3.5" /> : <Save className="w-3.5 h-3.5" />}
          {savedFlash ? t("editor.saved") : t("editor.save")}
          <span className="text-[10px] opacity-70">⌘↵</span>
        </button>
      </div>

      {/* 内容区 */}
      <div className="flex-1 flex min-h-0">
        {showLeft && showRight ? (
          // split 模式
          <Splitter
            left={<CodeMirrorEditor value={text} readOnly={readOnly} fontSize={fontSize} onChange={onChange} viewRef={viewRef} />}
            right={<MarkdownPreview source={text} />}
            ratio={splitRatio}
            onRatioChange={setSplitRatio}
            showRight={true}
          />
        ) : showLeft ? (
          // editor 模式
          <Splitter
            left={<CodeMirrorEditor value={text} readOnly={readOnly} fontSize={fontSize} onChange={onChange} viewRef={viewRef} />}
            right={null}
            ratio={1}
            onRatioChange={() => {}}
            showRight={false}
          />
        ) : (
          // preview 模式
          <MarkdownPreview source={text} />
        )}
      </div>
    </div>
  );
}
```

- [ ] **Step 2: 类型检查**

```bash
cd crates/desktop/frontend && npx tsc --noEmit 2>&1 | tail -5
```

Expected: 无类型错误。注意确认 `lucide-react` 导出了 `Columns2`、`FileText` 图标（如不存在换成同类图标）。

- [ ] **Step 3: Commit**

```bash
git add crates/desktop/frontend/src/pages/CompactEditor/MarkdownPane.tsx
git commit -m "feat(frontend): MarkdownPane 组件（工具栏 + 视图模式切换 + CM6/Preview 组合）"
```

---

## Task 7: CSS 样式（prose 排版 + CM6 滚动条）

**Files:**
- Modify: `crates/desktop/frontend/src/index.css`

- [ ] **Step 1: 在 index.css 末尾追加 markdown 预览 + CM6 样式**

在 `crates/desktop/frontend/src/index.css` 的末尾追加：

```css
/* ── Markdown 预览排版 ── */
.md-prose {
  max-width: 720px;
  margin: 0 auto;
  color: var(--color-foreground);
  font-size: 15px;
  line-height: 1.65;
}
.md-prose > * + * { margin-top: 1em; }
.md-prose h1, .md-prose h2, .md-prose h3, .md-prose h4 {
  font-weight: 600;
  letter-spacing: -0.015em;
  line-height: 1.25;
  margin-top: 1.6em;
}
.md-prose h1 {
  font-size: 1.8em;
  margin-top: 0;
  padding-bottom: 0.35em;
  border-bottom: 1px solid var(--color-border);
}
.md-prose h2 { font-size: 1.32em; }
.md-prose h3 { font-size: 1.1em; }
.md-prose h4 { font-size: 1em; color: var(--color-muted-foreground); }
.md-prose a {
  color: var(--color-voice, #d97706);
  text-decoration: none;
  border-bottom: 1px solid color-mix(in srgb, var(--color-voice, #d97706) 40%, transparent);
}
.md-prose a:hover { border-bottom-color: var(--color-voice, #d97706); }
.md-prose code {
  font-family: ui-monospace, SFMono-Regular, Menlo, monospace;
  font-size: 0.86em;
  padding: 1.5px 6px;
  border-radius: 4px;
  background: var(--color-muted);
  border: 1px solid var(--color-border);
}
.md-prose pre {
  font-family: ui-monospace, SFMono-Regular, Menlo, monospace;
  font-size: 0.85em;
  line-height: 1.6;
  padding: 14px 16px;
  border-radius: 8px;
  border: 1px solid var(--color-border);
  overflow-x: auto;
  white-space: pre-wrap;
  overflow-wrap: anywhere;
  background: var(--color-muted);
  margin: 0;
}
.md-prose pre code {
  background: transparent;
  border: 0;
  padding: 0;
  font-size: inherit;
  white-space: inherit;
}
.md-prose blockquote {
  border-left: 3px solid var(--color-voice, #d97706);
  padding-left: 1em;
  color: var(--color-muted-foreground);
  margin: 0;
}
.md-prose table { border-collapse: collapse; width: 100%; }
.md-prose th, .md-prose td { border: 1px solid var(--color-border); padding: 6px 12px; text-align: left; }
.md-prose img { max-width: 100%; border-radius: 8px; }

/* 代码块 + 复制按钮 */
.md-codeblock { position: relative; }
.md-copy-btn {
  position: absolute;
  top: 8px;
  right: 8px;
  display: inline-flex;
  align-items: center;
  font-size: 11px;
  padding: 3px 8px;
  border-radius: 4px;
  border: 1px solid var(--color-border);
  background: var(--color-background);
  color: var(--color-muted-foreground);
  opacity: 0;
  cursor: pointer;
  transition: opacity 120ms;
}
.md-codeblock:hover .md-copy-btn { opacity: 1; }
.md-copy-btn:hover { color: var(--color-foreground); border-color: var(--color-muted-foreground); }

/* mermaid 占位（埋点，当前仅显示原文） */
.md-mermaid-pending {
  font-family: ui-monospace, monospace;
  font-size: 0.85em;
  color: var(--color-muted-foreground);
}

/* CM6 编辑器滚动条 */
.md-cm-editor .cm-scroller {
  scrollbar-width: thin;
  scrollbar-color: rgba(128, 128, 128, 0.35) transparent;
}
.md-cm-editor .cm-scroller::-webkit-scrollbar { width: 8px; height: 8px; }
.md-cm-editor .cm-scroller::-webkit-scrollbar-track { background: transparent; }
.md-cm-editor .cm-scroller::-webkit-scrollbar-thumb { background: rgba(128, 128, 128, 0.35); border-radius: 4px; }
.md-cm-editor .cm-scroller::-webkit-scrollbar-thumb:hover { background: rgba(128, 128, 128, 0.6); }

/* 预览区滚动条 */
.md-preview {
  scrollbar-width: thin;
  scrollbar-color: rgba(128, 128, 128, 0.35) transparent;
}
.md-preview::-webkit-scrollbar { width: 8px; }
.md-preview::-webkit-scrollbar-track { background: transparent; }
.md-preview::-webkit-scrollbar-thumb { background: rgba(128, 128, 128, 0.35); border-radius: 4px; }
.md-preview::-webkit-scrollbar-thumb:hover { background: rgba(128, 128, 128, 0.6); }

/* CM6 编辑器面板占满父容器 */
.md-cm-editor .cm-editor { height: 100%; }
```

- [ ] **Step 2: 前端构建验证**

```bash
cd crates/desktop/frontend && npx vite build 2>&1 | tail -5
```

Expected: 构建成功，无 CSS 错误。

- [ ] **Step 3: Commit**

```bash
git add crates/desktop/frontend/src/index.css
git commit -m "style(frontend): Markdown prose 排版 + CM6 滚动条 + 代码块复制按钮"
```

---

## Task 8: CompactEditor index.tsx 改造（移除 textarea，接入 MarkdownPane）

**Files:**
- Modify: `crates/desktop/frontend/src/pages/CompactEditor/index.tsx`

**Interfaces:**
- Consumes: `MarkdownPane`（Task 6）、`useT()`（Task 3）

- [ ] **Step 1: 改造 index.tsx**

对 `crates/desktop/frontend/src/pages/CompactEditor/index.tsx` 做以下改动：

**1a. 更新 import**：

移除不再需要的 import（`Undo2`、`Redo2`、`ZoomIn`、`ZoomOut`、`Search`、`Eraser`、`Save`、`Check`、`ChevronUp`、`ChevronDown`、`Replace`、`Type`、`Eye` 不再在此文件使用——移到 MarkdownPane 了；但 `Mic`、`X`、`Type`、`Eye` 在 tab 栏 `tabIcon` 仍用，保留 `Mic`、`X`、`Type`、`Eye`）。

新增 import：

```tsx
import MarkdownPane from "./MarkdownPane";
import { useT } from "@/lib/i18n";
```

**1b. 移除手写查找替换逻辑**：

删除以下代码块（约 180 行）：
- `collectMatches` 函数（~14 行）
- `selectRange` 函数（~12 行）
- `runFind` useCallback（~8 行）
- findDebounce / prevFindQuery 相关 useEffect（~20 行）
- `gotoMatch` 函数（~14 行）
- `replaceOne` 函数（~14 行）
- `replaceAll` 函数（~10 行）
- `undo` / `redo` 函数（~2 行）
- `clearAll` / `clearPending` 状态（~5 行）
- 查找替换相关 state：`showFind`、`findQuery`、`replaceQuery`、`matchIdx`、`matches`（~5 行）

**1c. 保留并调整外壳逻辑**：

保留 tab 管理（`loadAndAddTab`、`closeTab`、`pendingToTab`、`readInitialTabFromUrl`）、`doSave` + `doSaveRef`、`fontSize` 状态、`savedFlash` 状态、mount effect、Cmd+S/Cmd+Enter 快捷键监听。

在组件内新增 `const t = useT();`。

**1d. 改造工具栏区域**：

移除原工具栏 `<div>`（撤销/重做/字号/查找替换/清空/保存）和查找替换条 `<div>`——这些都移入 MarkdownPane 了。tab 栏保留不变。

**1e. 改造内容区渲染**：

将原 textarea 渲染替换为 MarkdownPane。找到内容区 `tabs.map` 内的 textarea 分支：

```tsx
// 原来的 textarea 块替换为：
{i === activeIdx ? (
  <MarkdownPane
    text={tab.text || ''}
    readOnly={tab.source === 'transcription'}
    fontSize={fontSize}
    onFontSizeChange={setFontSize}
    onChange={(next) => updateActiveTextAt(next, i)}
    onClear={() => updateActiveTextAt('', i)}
    onSave={doSave}
    disableSave={active?.isTemp}
    savedFlash={savedFlash}
  />
) : (
  <div className="flex-1 flex items-center justify-center text-xs text-muted-foreground">
    {t("editor.switchHint")}
  </div>
)}
```

**1f. 快捷键监听调整**：

移除 keydown useEffect 中查找替换相关的快捷键拦截（`Cmd+F`）。保留 `Cmd+S` / `Cmd+Enter`（调 `doSaveRef`）。`Cmd+Z` / `Cmd+Shift+Z` 移除（CM6 原生处理）。`Escape` 关查找栏移除。

改造后的 keydown effect 简化为：

```tsx
useEffect(() => {
  const onKey = (e: KeyboardEvent) => {
    if (e.isComposing || e.keyCode === 229) return;
    const mod = e.metaKey || e.ctrlKey;
    if (mod && e.key === "Enter") { e.preventDefault(); if (!active?.itemType || active.itemType === 'text') doSaveRef.current(); return; }
    if (mod && e.key.toLowerCase() === "s") { e.preventDefault(); if (!active?.itemType || active.itemType === 'text') doSaveRef.current(); return; }
  };
  document.addEventListener("keydown", onKey);
  return () => document.removeEventListener("keydown", onKey);
}, []);
```

> 注意：此 effect 不再依赖 `showFind`，deps 改为 `[]`（doSaveRef 已稳定引用）。

- [ ] **Step 2: 类型检查**

```bash
cd crates/desktop/frontend && npx tsc --noEmit 2>&1 | tail -10
```

Expected: 无类型错误。如有未使用变量警告（如 `Type` 图标 import），清理掉。

- [ ] **Step 3: 前端构建**

```bash
cd crates/desktop/frontend && npx vite build 2>&1 | tail -5
```

Expected: 构建成功。

- [ ] **Step 4: Commit**

```bash
git add crates/desktop/frontend/src/pages/CompactEditor/index.tsx
git commit -m "refactor(frontend): CompactEditor 接入 MarkdownPane（移除 textarea + 手写查找替换 ~180 行）"
```

---

## Task 9: GeneralPanel 界面语言下拉 + 窗口尺寸调整

**Files:**
- Modify: `crates/desktop/frontend/src/pages/Settings/GeneralPanel.tsx`
- Modify: `crates/desktop/src/compact_editor_window.rs`

- [ ] **Step 1: GeneralPanel 新增界面语言下拉**

在 `crates/desktop/frontend/src/pages/Settings/GeneralPanel.tsx` 中，找到「外观」Card（`<Card icon={Palette} title="外观">`），在其 `<Row label="主题">` 之后新增一个 Row：

```tsx
<Row label={t("settings.uiLanguage")} effect="立即">
  <select
    className={selectClass}
    value={(cfg.ui_language as string) || "zh-CN"}
    onChange={(e) => setUiLanguage(e.target.value)}
  >
    <option value="zh-CN">{t("settings.uiLanguage.zhCN")}</option>
    <option value="en">{t("settings.uiLanguage.en")}</option>
  </select>
</Row>
```

在组件内（`setTheme` useCallback 附近）新增 `setUiLanguage`：

```tsx
const setUiLanguage = useCallback(async (lang: string) => {
  await setVal("ui_language", lang);
  setLocale(lang as "zh-CN" | "en");
}, [setVal]);
```

新增 import：

```tsx
import { useT, setLocale } from "@/lib/i18n";
```

并在组件内添加 `const t = useT();`。

> 注意：`setVal` 的签名是 `(key: string, value: unknown) => Promise<void>`，调后端 `update_config` 命令。`setLocale` 热更新前端 i18n，不需要刷新页面。

- [ ] **Step 2: compact_editor_window.rs 默认窗口尺寸**

在 `crates/desktop/src/compact_editor_window.rs` 中找到窗口尺寸常量定义：

```rust
// crates/desktop/src/compact_editor_window.rs 第 11-14 行
const WIDTH: f64 = 880.0;     // → 1100.0
const HEIGHT: f64 = 620.0;    // → 680.0
const MIN_WIDTH: f64 = 480.0; // → 600.0
const MIN_HEIGHT: f64 = 360.0; // 保持不变
```

- [ ] **Step 3: 编译验证**

```bash
cargo build -p octopus-desktop 2>&1 | tail -5
```

Expected: 编译通过。

- [ ] **Step 4: 前端类型检查**

```bash
cd crates/desktop/frontend && npx tsc --noEmit 2>&1 | tail -5
```

Expected: 无类型错误。

- [ ] **Step 5: Commit**

```bash
git add crates/desktop/frontend/src/pages/Settings/GeneralPanel.tsx crates/desktop/src/compact_editor_window.rs
git commit -m "feat: 设置面板界面语言下拉 + 编辑器窗口默认尺寸加宽"
```

---

## Task 10: 集成验证 + 文档同步

**Files:**
- Modify: `docs/architecture.md`

- [ ] **Step 1: 全量前端测试**

```bash
cd crates/desktop/frontend && npx vitest run 2>&1 | tail -10
```

Expected: 全部 PASS（含新增 markdown.test.ts + i18n.test.ts）。

- [ ] **Step 2: 全量后端测试**

```bash
cargo test -p octopus-infra -p octopus-desktop 2>&1 | tail -10
```

Expected: 全部 PASS。

- [ ] **Step 3: 前端构建**

```bash
cd crates/desktop/frontend && npx vite build 2>&1 | tail -5
```

Expected: 构建成功。

- [ ] **Step 4: 桌面应用构建**

```bash
cargo build --release -p octopus-desktop --features embedded 2>&1 | tail -5
```

Expected: 编译成功。

- [ ] **Step 5: 更新 architecture.md**

在 `docs/architecture.md` 的 CompactEditor 相关段落更新描述，追加 markdown 改造要点：

- CompactEditor 文本/语音 tab 已升级为 CodeMirror 6 + markdown-it 预览
- 视图模式（编辑/分屏/预览），可编辑默认分屏，只读默认预览
- 查找替换改为 CM6 原生 search()
- i18n 基础设施（lib/i18n.ts），ui_language config 字段
- 代码高亮/Mermaid/PlantUML 仅埋点（highlight 回调），后续可扩展

- [ ] **Step 6: Commit**

```bash
git add docs/architecture.md
git commit -m "docs: 更新 architecture.md——CompactEditor Markdown 改造"
```

---

## 手动验证清单（构建后执行）

- [ ] 文本 tab：编辑 markdown → 预览实时更新（debounce 150ms）
- [ ] 语音 tab：只读预览 → 切到编辑模式看 CM6 源码高亮
- [ ] 图片 tab：功能不变
- [ ] Cmd+F 查找替换面板正常工作
- [ ] Cmd+S / Cmd+Enter 保存回写 DB
- [ ] 撤销/重做按钮 + Cmd+Z / Cmd+Shift+Z
- [ ] 分屏拖拽 + 滚动同步
- [ ] 视图模式切换（编辑/分屏/预览）
- [ ] 语言切换：设置面板切 English → CompactEditor 文案变英文
- [ ] 多 tab 快速切换无崩溃
- [ ] 代码块复制按钮 hover 显示 + 点击复制
- [ ] 标题锚点跳转（预览中点 `#anchor` 链接滚动到对应标题）
