# Result 浮窗翻译双语视图 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 在语音识别 Result 浮窗新增「翻译」入口——点击后自动进大窗，文本区从单语变上下两栏（上原文、下译文），支持手动/自动节流重翻，保存时终翻并提交译文到光标处。

**Architecture:** 前端在 `pages/Result/index.tsx` 新增翻译模式状态机（`TranslateMode: off/manual/8s/12s/15s`），`off` 时渲染现有单语 AsrEditor，非 `off` 时渲染上下分栏（原文 AsrEditor + 新建 TranslationPane）。翻译执行复用现有 `translate_text` fire-and-forget 命令 + `translate-progress`/`translate-done` 事件。档位偏好写 DB app_config 表（`translate_mode` 键），与 `denoise_mode` 一致。后端仅新增一个 `set_translate_mode` 命令 + `ToolbarState` 扩展一个字段。

**Tech Stack:** Rust（Tauri 2 commands）、React + TypeScript（CodeMirror 6 编辑器）、Vitest（前端单测）、`octopus_infra::db`（SQLite app_config 表）。

**Spec:** [`docs/superpowers/specs/2026-07-12-result-window-translation-bilingual-design.md`](../specs/2026-07-12-result-window-translation-bilingual-design.md)

## Global Constraints

- 所有 UI 文案走 i18n（`zh-CN.yaml` + `en.yaml`），key 前缀 `result.translate*`
- 配置持久化写 **DB app_config 表**（`octopus_infra::db::save_config_key`），不写 config.yaml
- 翻译后端零改动——复用现有 `translate_text` / `do_translate_streaming` / `detect_translate_direction`
- 所有工作在 worktree `.worktrees/worktree-translation-pane`（分支 `worktree-translation-pane`）内完成
- 测试为内联 `#[cfg(test)] mod tests {}`（Rust）和 `*.test.ts`（前端），无独立 tests 目录
- 前端测试只测纯逻辑函数（跟随项目现有模式——无 React 组件渲染测试）

## File Structure

| 文件 | 职责 | 改动类型 |
|------|------|---------|
| `crates/desktop/src/runtime_config.rs` | 新增 `set_translate_mode` 命令 + `ToolbarState.translate_mode` + 笔误清理 | 修改 |
| `crates/desktop/src/main.rs` | 注册 `set_translate_mode` | 修改 |
| `crates/desktop/frontend/src/components/SvgIcon.tsx` | 注册 `translate` + `redo` 图标 | 修改 |
| `crates/desktop/frontend/src/pages/Result/AsrEditor.tsx` | `AsrEditorHandle` 新增 `getText()` | 修改 |
| `crates/desktop/frontend/src/pages/Result/TranslationPane.tsx` | 译文区组件（极简 CM6） | **新建** |
| `crates/desktop/frontend/src/pages/Result/index.tsx` | 翻译模式状态 + 渲染分流 + 工具栏 + 翻译逻辑 + 保存语义 | 修改 |
| `crates/desktop/frontend/src/locales/zh-CN.yaml` | 翻译相关 i18n key | 修改 |
| `crates/desktop/frontend/src/locales/en.yaml` | 翻译相关 i18n key | 修改 |
| `docs/architecture.md` | Result 浮窗翻译双语视图说明 | 修改 |

---

### Task 1: 后端 `set_translate_mode` 命令 + `ToolbarState` 扩展 + 笔误清理

**Files:**
- Modify: `crates/desktop/src/runtime_config.rs:127-140`（ToolbarState struct）、`:243-265`（toolbar_state 命令）、`:341-356`（set_polish_mode 笔误）、新增 `set_translate_mode` 命令
- Modify: `crates/desktop/src/main.rs:205`（invoke_handler 注册）
- Test: `crates/desktop/src/runtime_config.rs`（内联 `#[cfg(test)] mod tests`）

**Interfaces:**
- Consumes: `octopus_infra::db::save_config_key`（`crates/infra/src/db.rs:483`）、`octopus_infra::db::load_config_key`（`crates/infra/src/db.rs:496`）
- Produces:
  - `pub fn set_translate_mode(mode: String) -> Result<(), String>`——Tauri command，校验 + 写 DB
  - `ToolbarState.translate_mode: String`——新增字段，`toolbar_state` 命令填充

- [ ] **Step 1: ToolbarState 新增 `translate_mode` 字段**

在 `crates/desktop/src/runtime_config.rs` 的 `ToolbarState` struct（约 127 行）末尾（`edit_shortcut` 字段后）新增：

```rust
    /// 翻译自动档（记忆档位）："manual" / "8s" / "12s" / "15s"。DB 无值时默认 "manual"。
    pub translate_mode: String,
```

定位精确文本——在 `edit_shortcut` 字段后追加（约 139 行）：

```rust
    /// 结果展示区编辑 toggle 快捷键（Tauri Accelerator 字符串，默认 "Cmd+Enter"，进入/保存同键）。
    /// 仅结果窗聚焦时生效。
    pub edit_shortcut: String,
    /// 翻译自动档（记忆档位）："manual" / "8s" / "12s" / "15s"。DB 无值时默认 "manual"。
    pub translate_mode: String,
}
```

- [ ] **Step 2: `toolbar_state` 命令填充 `translate_mode`**

在 `toolbar_state` 命令（约 244 行）的 `ToolbarState { ... }` 构造中新增字段。读取 DB：

```rust
    let translate_mode = octopus_infra::db::load_config_key("translate_mode")
        .ok()
        .flatten()
        .filter(|s| matches!(s.as_str(), "manual" | "8s" | "12s" | "15s"))
        .unwrap_or_else(|| "manual".to_string());
```

在 `ToolbarState { ... }` 构造体（约 257 行）末尾追加：

```rust
        edit_shortcut,
        translate_mode,
    }
```

- [ ] **Step 3: 新增 `set_translate_mode` 命令**

在 `set_denoise_mode` 命令（约 358 行）之后新增：

```rust
/// 设置翻译自动档位（manual/8s/12s/15s）。纯持久化到 DB，翻译节流逻辑在前端。
#[tauri::command]
pub fn set_translate_mode(mode: String) -> Result<(), String> {
    let valid = matches!(mode.as_str(), "manual" | "8s" | "12s" | "15s");
    if !valid {
        return Err(format!("translate_mode='{}' 非法（应为 manual/8s/12s/15s）", mode));
    }
    octopus_infra::db::save_config_key("translate_mode", &mode).map_err(|e| e.to_string())
}
```

- [ ] **Step 4: 笔误清理——`set_polish_mode` 日志文案**

在 `set_polish_mode`（约 348 行），将日志文案从 "写回 config.yaml 失败" 改为 "写回 DB 失败"（实际写 DB app_config 表）：

```rust
    if let Err(e) = persist_polish_mode(mode) {
        log::warn!(
            "写回 DB 失败（polish_mode={}）：{} —— 本次仍生效，重启后回退",
            mode,
            e
        );
    }
```

- [ ] **Step 5: main.rs 注册 `set_translate_mode`**

在 `crates/desktop/src/main.rs` 的 `invoke_handler`（约 205 行 `runtime_config::set_denoise_mode,` 之后）新增：

```rust
            runtime_config::set_denoise_mode,
            runtime_config::set_translate_mode,
```

- [ ] **Step 6: 编译验证**

Run: `cargo build -p octopus-desktop --features embedded 2>&1 | tail -5`
Expected: 编译通过，无 error。

- [ ] **Step 7: Commit**

```bash
git add crates/desktop/src/runtime_config.rs crates/desktop/src/main.rs
git commit -m "feat(desktop): 新增 set_translate_mode 命令 + ToolbarState.translate_mode + 笔误清理"
```

---

### Task 2: i18n 文案

**Files:**
- Modify: `crates/desktop/frontend/src/locales/zh-CN.yaml:452-472`（result 段）
- Modify: `crates/desktop/frontend/src/locales/en.yaml:452-472`（result 段）

**Interfaces:**
- Produces: `result.translate` / `result.translateManual` / `result.translateAuto8` / `result.translateAuto12` / `result.translateAuto15` / `result.translateNow` / `result.translating` / `result.translateFail`

- [ ] **Step 1: zh-CN.yaml 新增翻译 key**

在 `crates/desktop/frontend/src/locales/zh-CN.yaml` 的 `result:` 段（约 469 行 `denoise:` 段之后、`# ════════ Screenshot` 之前）新增：

```yaml
  translate: 翻译
  translateManual: 手动
  translateAuto8: 自动 8s
  translateAuto12: 自动 12s
  translateAuto15: 自动 15s
  translateNow: 立即翻译
  translating: 翻译中…
  translateFail: "翻译失败："
```

- [ ] **Step 2: en.yaml 新增翻译 key**

在 `crates/desktop/frontend/src/locales/en.yaml` 的 `result:` 段对应位置新增：

```yaml
  translate: Translate
  translateManual: Manual
  translateAuto8: Auto 8s
  translateAuto12: Auto 12s
  translateAuto15: Auto 15s
  translateNow: Translate Now
  translating: Translating…
  translateFail: "Translation failed:"
```

- [ ] **Step 3: Commit**

```bash
git add crates/desktop/frontend/src/locales/zh-CN.yaml crates/desktop/frontend/src/locales/en.yaml
git commit -m "i18n: 新增 Result 浮窗翻译双语视图文案"
```

---

### Task 3: SvgIcon 注册 translate + redo 图标

**Files:**
- Modify: `crates/desktop/frontend/src/components/SvgIcon.tsx:3-23`（ICONS map）

**Interfaces:**
- Produces: `IconName` 新增 `"translate"` 和 `"redo"`（指向已有的 `/icons/action-translate.svg` 和 `/icons/redo.svg`）

- [ ] **Step 1: 注册图标**

在 `SvgIcon.tsx` 的 `ICONS` 对象中（约 16 行 `"minimize"` 之后）新增：

```typescript
  "minimize": "/icons/minimize.svg",
  "translate": "/icons/action-translate.svg",
  "redo": "/icons/redo.svg",
```

- [ ] **Step 2: 类型检查**

Run: `cd crates/desktop/frontend && npx tsc --noEmit 2>&1 | tail -5`
Expected: 无 error。

- [ ] **Step 3: Commit**

```bash
git add crates/desktop/frontend/src/components/SvgIcon.tsx
git commit -m "feat(frontend): SvgIcon 注册 translate + redo 图标"
```

---

### Task 4: AsrEditorHandle 新增 `getText()` 方法

**Files:**
- Modify: `crates/desktop/frontend/src/pages/Result/AsrEditor.tsx:18-20`（AsrEditorHandle 接口）、`:153`（useImperativeHandle）

**Interfaces:**
- Consumes: 现有 `viewRef`（`EditorView | null`）
- Produces: `AsrEditorHandle.getText(): string`——返回当前 CM6 文档全文（不触发 commit 副作用）

**背景：** 翻译节流和终翻需要读取当前原文，但不能调 `commit()`（commit 会清空编辑态、触发 `onCommit` 回调、产生 IPC 副作用）。新增只读 `getText()` 方法。

- [ ] **Step 1: 扩展 AsrEditorHandle 接口**

在 `AsrEditor.tsx:18-20`：

```typescript
export interface AsrEditorHandle {
  commit: () => void;
  getText: () => string;
}
```

- [ ] **Step 2: 实现 getText**

在 `AsrEditor.tsx:153` 的 `useImperativeHandle`：

```typescript
  useImperativeHandle(ref, () => ({
    commit: doCommit,
    getText: () => viewRef.current?.state.doc.toString() ?? "",
  }));
```

- [ ] **Step 3: 类型检查**

Run: `cd crates/desktop/frontend && npx tsc --noEmit 2>&1 | tail -5`
Expected: 无 error。

- [ ] **Step 4: Commit**

```bash
git add crates/desktop/frontend/src/pages/Result/AsrEditor.tsx
git commit -m "feat(result): AsrEditorHandle 新增 getText() 只读方法"
```

---

### Task 5: 新建 TranslationPane 组件

**Files:**
- Create: `crates/desktop/frontend/src/pages/Result/TranslationPane.tsx`

**Interfaces:**
- Consumes: CodeMirror 6（`@codemirror/state` + `@codemirror/view`）、`useT` / `t`（i18n）
- Produces: `TranslationPane` 组件（Props: `text` / `translating` / `onChange`）

**设计：** 极简 CM6 实例，复用 AsrEditor 的主题配置（字体/颜色/行高），但**无** dirtyRanges / caret / commit / enter_edit_mode / 流式追加逻辑——纯展示 + 可编辑。

- [ ] **Step 1: 创建 TranslationPane.tsx**

创建 `crates/desktop/frontend/src/pages/Result/TranslationPane.tsx`：

```tsx
import { useEffect, useRef } from "react";
import { EditorState } from "@codemirror/state";
import { EditorView, keymap, drawSelection } from "@codemirror/view";
import { defaultKeymap, history, historyKeymap, indentWithTab } from "@codemirror/commands";
import { useT } from "@/lib/i18n";

interface TranslationPaneProps {
  text: string;
  translating: boolean;
  onChange?: (s: string) => void;
}

function buildTheme() {
  return EditorView.theme({
    "&": {
      height: "100%",
      backgroundColor: "transparent",
      color: "var(--color-foreground)",
      fontFamily: "-apple-system, BlinkMacSystemFont, sans-serif",
      fontSize: "14px",
    },
    ".cm-scroller": {
      lineHeight: "1.6",
      padding: "4px 14px 8px",
      fontFamily: "inherit",
    },
    ".cm-gutters": { display: "none" },
    ".cm-activeLine": { backgroundColor: "transparent" },
    ".cm-activeLineGutter": { backgroundColor: "transparent" },
    "&.cm-focused": { outline: "none" },
    "&.cm-focused .cm-selectionBackground, ::selection, .cm-selectionBackground": {
      backgroundColor: "color-mix(in srgb, var(--color-voice, #d97706) 22%, transparent)",
    },
    ".cm-cursor, .cm-dropCursor": {
      borderLeftColor: "var(--color-voice, #d97706)",
      borderLeftWidth: "1.5px",
    },
    ".cm-line": { padding: "0" },
  }, { dark: false });
}

export function TranslationPane({ text, translating, onChange }: TranslationPaneProps) {
  const t = useT();
  const hostRef = useRef<HTMLDivElement>(null);
  const viewRef = useRef<EditorView | null>(null);
  const onChangeRef = useRef(onChange);
  onChangeRef.current = onChange;
  const lastExternalTextRef = useRef(text);

  useEffect(() => {
    if (!hostRef.current) return;

    const state = EditorState.create({
      doc: text,
      extensions: [
        history(),
        drawSelection(),
        EditorView.lineWrapping,
        keymap.of([...defaultKeymap, ...historyKeymap, indentWithTab]),
        buildTheme(),
        EditorView.updateListener.of((update) => {
          if (update.docChanged) {
            onChangeRef.current?.(update.state.doc.toString());
          }
        }),
      ],
    });

    const view = new EditorView({ state, parent: hostRef.current });
    viewRef.current = view;

    return () => {
      view.destroy();
      viewRef.current = null;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  // 外部 text 变化时更新文档（translate-progress/done 驱动），但用户编辑中不打断
  useEffect(() => {
    const view = viewRef.current;
    if (!view) return;
    const current = view.state.doc.toString();
    if (text === current) return;
    lastExternalTextRef.current = text;
    const len = view.state.doc.length;
    view.dispatch({ changes: { from: 0, to: len, insert: text } });
  }, [text]);

  return (
    <div className="relative h-full flex flex-col">
      {translating && (
        <div className="flex-shrink-0 px-3.5 pt-1 text-[11px]" style={{ color: "var(--color-tool-icon)", opacity: 0.5 }}>
          {t("result.translating")}
        </div>
      )}
      <div ref={hostRef} className="flex-1 overflow-hidden" style={{ minHeight: 0 }} />
    </div>
  );
}
```

- [ ] **Step 2: 类型检查**

Run: `cd crates/desktop/frontend && npx tsc --noEmit 2>&1 | tail -5`
Expected: 无 error。

- [ ] **Step 3: Commit**

```bash
git add crates/desktop/frontend/src/pages/Result/TranslationPane.tsx
git commit -m "feat(result): 新建 TranslationPane 译文区组件"
```

---

### Task 6: Result/index.tsx 翻译模式 UI 骨架（状态 + 渲染分流 + 工具栏）

**Files:**
- Modify: `crates/desktop/frontend/src/pages/Result/index.tsx`

**Interfaces:**
- Consumes: Task 2 的 i18n key、Task 3 的 `translate`/`redo` 图标、Task 5 的 `TranslationPane`、现有 `toolbarState`（含 Task 1 的 `translate_mode`）
- Produces: Result 组件支持翻译模式渲染分流 + 工具栏交互（但翻译执行逻辑在 Task 7 接入）

**背景：** 本任务搭好 UI 骨架——翻译模式下自动进大窗、上下分栏渲染、工具栏翻译下拉 + 立即翻译按钮、移除 settings。翻译执行逻辑（doTranslate / 节流 / 终翻 / 事件监听）在 Task 7 接入。本任务结束后，UI 可切换翻译模式（但翻译按钮暂为 no-op）。

- [ ] **Step 1: 新增 translateMode 等状态**

在 `Result/index.tsx` 的 state 声明区（约 46 行 `polishLoading` 之后）新增：

```tsx
  const [translateMode, setTranslateMode] = useState<TranslateMode>('off');
  const [translatedText, setTranslatedText] = useState("");
  const [translating, setTranslating] = useState(false);
```

在文件顶部（约 12 行 `const DENOISE_MODES` 之后）新增类型定义和辅助函数：

```tsx
type TranslateMode = 'off' | 'manual' | '8s' | '12s' | '15s';

const TRANSLATE_MODES: TranslateMode[] = ['manual', '8s', '12s', '15s'];
```

在 state 区之后新增 refs：

```tsx
  const lastTranslatedRef = useRef<string>("");
  const translatingRef = useRef(false);
  const translateModeRef = useRef<TranslateMode>('off');
  useEffect(() => { translateModeRef.current = translateMode; }, [translateMode]);
```

- [ ] **Step 2: 修改 `PopupType` 支持翻译菜单**

将 `PopupType` 类型（约 29 行）扩展：

```tsx
type PopupType = "polish" | "denoise" | "asr" | "llm" | "translate" | null;
```

- [ ] **Step 3: 进翻译模式函数**

在 `polishNow` 回调之后（约 192 行）新增：

```tsx
  const enterTranslateMode = useCallback(() => {
    const remembered = toolbarState.translate_mode;
    const mode: TranslateMode = TRANSLATE_MODES.includes(remembered as TranslateMode)
      ? remembered as TranslateMode
      : 'manual';
    setTranslateMode(mode);
    if (!expanded) {
      setExpanded(true);
      invoke("set_result_click_through", { expanded: true });
    }
  }, [toolbarState.translate_mode, expanded]);

  const exitTranslateMode = useCallback(() => {
    setTranslateMode('off');
    setTranslatedText("");
    translatingRef.current = false;
    setTranslating(false);
  }, []);

  const openTranslatePopup = async () => {
    if (translateMode === 'off') {
      enterTranslateMode();
      return;
    }
    if (popupType === "translate") { setPopupType(null); return; }
    setPopupItems(TRANSLATE_MODES.map(m => ({
      label: m === 'manual' ? t("result.translateManual")
        : m === '8s' ? t("result.translateAuto8")
        : m === '12s' ? t("result.translateAuto12")
        : t("result.translateAuto15"),
      current: m === translateMode,
      name: m,
    })));
    setPopupType("translate");
  };

  const handleTranslateModeSelect = async (item: PopupItem) => {
    const mode = item.name as TranslateMode;
    setTranslateMode(mode);
    setPopupType(null);
    try {
      await invoke("set_translate_mode", { mode });
    } catch (e) {
      showToast(ti18n("result.switchFailed") + e);
    }
  };
```

- [ ] **Step 4: handlePopupSelect 分发翻译菜单**

在 `handlePopupSelect`（约 261 行）新增 translate 分支。在 `else if (popupType === "denoise"` 之后：

```tsx
      } else if (popupType === "translate" && item.name) {
        await handleTranslateModeSelect(item);
        return;
      }
```

- [ ] **Step 5: 工具栏——移除 settings，新增翻译下拉 + 立即翻译**

将 `tools` 数组（约 278-286 行）改为：

```tsx
  const tools: { id: string; icon: IconName; label: string; active?: boolean; disabled?: boolean; onClick: () => void }[] = [
    { id: "close", icon: "close", label: t("result.close"), onClick: () => invoke("discard_recording") },
    { id: "denoise", icon: "denoise", label: t("result.denoiseMode"), active: toolbarState.denoise_mode !== 0, onClick: openDenoisePopup },
    { id: "polish", icon: "polish", label: t("result.polishMode"), active: toolbarState.polish_mode !== 0, onClick: openPolishPopup },
    { id: "polish-now", icon: "polish-now", label: t("result.polishNow"), disabled: polishLoading, onClick: polishNow },
    { id: "translate", icon: "translate", label: translateMode === 'off' ? t("result.translate") : t("result.translate"), active: translateMode !== 'off', onClick: openTranslatePopup },
    { id: "translate-now", icon: "redo", label: t("result.translateNow"), disabled: translating || translateMode === 'off', onClick: () => {} },
    { id: "toggle-size", icon: (expanded ? "minimize" : "expand-edit") as IconName, label: expanded ? t("result.zoomOut") : t("result.zoomIn"), disabled: translateMode !== 'off', onClick: toggleExpand },
    { id: "save", icon: "save" as IconName, label: t("result.save"), onClick: () => asrEditorRef.current?.commit() },
  ];
```

> **注：** `translate-now` 的 `onClick: () => {}` 是 Task 6 的占位——Task 7 接入 `doTranslate`。`settings` 按钮已移除。翻译模式下 `toggle-size` 禁用（spec §8.5——防止收成迷你窗后双语栏挤崩）。

- [ ] **Step 6: 翻译模式——渲染分流（上下分栏）**

将文本区渲染（约 351-371 行）改为条件渲染：

```tsx
      {/* Text display */}
      <div className="flex-1 px-3.5 pt-1 pb-2 overflow-hidden relative">
        {translateMode === 'off' ? (
          <div className="relative h-full">
            <AsrEditor
              key={asrEditorResetKey}
              ref={asrEditorRef}
              text={text}
              caret={caretRef.current}
              expanded={expanded}
              onCommit={(payload) => {
                setText(payload.text);
                if (translateModeRef.current === 'off') {
                  invoke("commit_edit", {
                    text: payload.text,
                    dirtyRanges: payload.dirtyRanges,
                    hasEdited: payload.hasEdited,
                    caret: payload.caret ?? null,
                    selection: payload.selection ?? null,
                  });
                }
              }}
            />
          </div>
        ) : (
          <div className="flex flex-col h-full gap-1">
            {/* 原文区（上） */}
            <div className="flex-1 min-h-0 border-b border-black/[0.06] overflow-hidden">
              <AsrEditor
                key={asrEditorResetKey}
                ref={asrEditorRef}
                text={text}
                caret={caretRef.current}
                expanded={true}
                onCommit={(payload) => {
                  setText(payload.text);
                }}
              />
            </div>
            {/* 译文区（下） */}
            <div className="flex-1 min-h-0 overflow-hidden">
              <TranslationPane
                text={translatedText}
                translating={translating}
                onChange={setTranslatedText}
              />
            </div>
          </div>
        )}
      </div>
```

> **关键：** `onCommit` 回调现在检查 `translateModeRef.current`——翻译态下只更新 `text` state，**不** invoke `commit_edit`（由 Task 7 的 `onSave` 统一处理提交译文）。单语态走原路径。

- [ ] **Step 7: 导入 TranslationPane**

在文件顶部 import 区（约 8 行）新增：

```tsx
import { TranslationPane } from "./TranslationPane";
```

- [ ] **Step 8: 类型检查**

Run: `cd crates/desktop/frontend && npx tsc --noEmit 2>&1 | tail -5`
Expected: 无 error。

- [ ] **Step 9: Commit**

```bash
git add crates/desktop/frontend/src/pages/Result/index.tsx
git commit -m "feat(result): 翻译模式 UI 骨架——渲染分流 + 工具栏翻译下拉 + 移除 settings"
```

---

### Task 7: Result/index.tsx 翻译执行逻辑（doTranslate + 节流 + 终翻 + 事件监听 + Cmd+T）

**Files:**
- Modify: `crates/desktop/frontend/src/pages/Result/index.tsx`

**Interfaces:**
- Consumes: Task 6 的 `translateMode` state / `translateModeRef` / `translatedText` state、现有 `translate_text` Tauri 命令（`crates/desktop/src/action_bar_commands.rs:451`）、`translate-progress` / `translate-done` 事件
- Produces: `doTranslate` / `finalTranslate` / 节流定时器 / 事件监听 / `Cmd+T` 快捷键 / 保存语义

**背景：** Task 6 搭好了 UI 骨架，本任务接入实际翻译逻辑。翻译执行复用现有 `translate_text`（fire-and-forget）+ 事件驱动更新。

- [ ] **Step 1: doTranslate 公共函数**

在 `enterTranslateMode` 之后新增：

```tsx
  const doTranslate = useCallback(async () => {
    const source = asrEditorRef.current?.getText() ?? text;
    if (!source.trim()) return;
    if (translatingRef.current) return;
    translatingRef.current = true;
    setTranslating(true);
    lastTranslatedRef.current = source;
    try {
      await invoke("translate_text", { text: source });
    } catch (e) {
      translatingRef.current = false;
      setTranslating(false);
      showToast(ti18n("result.translateFail") + String(e));
    }
  }, [text, showToast]);
```

- [ ] **Step 2: finalTranslate 同步等待函数**

在 `doTranslate` 之后新增：

```tsx
  const finalTranslate = useCallback(async (): Promise<string> => {
    const source = asrEditorRef.current?.getText() ?? text;
    if (!source.trim()) return "";
    return new Promise<string>((resolve) => {
      let resolved = false;
      const unlistenPromise = listen("translate-done", (e) => {
        if (resolved) return;
        resolved = true;
        unlistenPromise.then(f => f());
        resolve(e.payload as string);
      });
      invoke("translate_text", { text: source }).catch(() => {
        if (resolved) return;
        resolved = true;
        resolve("");
      });
    });
  }, [text]);
```

- [ ] **Step 3: translate-progress / done 事件监听**

在 Tauri events 的 `useEffect`（约 120 行）的 handlers 数组中新增两个事件，**或**在翻译模式专用 `useEffect` 中处理。推荐后者（翻译模式退出时自动卸载）：

在 `enterTranslateMode` / `doTranslate` 定义之后新增：

```tsx
  // 翻译事件监听——仅翻译模式下生效
  useEffect(() => {
    if (translateMode === 'off') return;
    let unlistens: UnlistenFn[] = [];
    let cancelled = false;

    (async () => {
      const fnProgress = await listen("translate-progress", (e) => {
        setTranslatedText(e.payload as string);
      });
      if (cancelled) { fnProgress(); return; }
      unlistens.push(fnProgress);

      const fnDone = await listen("translate-done", (e) => {
        setTranslatedText(e.payload as string);
        translatingRef.current = false;
        setTranslating(false);
      });
      if (cancelled) { fnDone(); return; }
      unlistens.push(fnDone);
    })();

    return () => {
      cancelled = true;
      unlistens.forEach(f => f());
    };
  }, [translateMode]);
```

- [ ] **Step 4: 节流定时器**

在事件监听 `useEffect` 之后新增：

```tsx
  // 自动档节流——translateMode 为 8s/12s/15s 时启动定时器
  useEffect(() => {
    if (translateMode === 'off' || translateMode === 'manual') return;
    const secs = parseInt(translateMode, 10);
    if (isNaN(secs)) return;

    const timer = setInterval(() => {
      const current = asrEditorRef.current?.getText() ?? "";
      if (current !== lastTranslatedRef.current && !translatingRef.current && current.trim()) {
        doTranslate();
      }
    }, secs * 1000);

    return () => clearInterval(timer);
  }, [translateMode, doTranslate]);
```

- [ ] **Step 5: 进翻译模式时首翻**

修改 `enterTranslateMode`（Task 6 Step 3 定义的），在 `setTranslateMode(mode)` 之后加首翻。改为：

```tsx
  const enterTranslateMode = useCallback(() => {
    const remembered = toolbarState.translate_mode;
    const mode: TranslateMode = TRANSLATE_MODES.includes(remembered as TranslateMode)
      ? remembered as TranslateMode
      : 'manual';
    setTranslateMode(mode);
    if (!expanded) {
      setExpanded(true);
      invoke("set_result_click_through", { expanded: true });
    }
    // 首翻一次
    setTimeout(() => doTranslateRef.current(), 100);
  }, [toolbarState.translate_mode, expanded]);
```

新增 `doTranslateRef`（防闭包陈旧）：

```tsx
  const doTranslateRef = useRef<() => void>(() => {});
  useEffect(() => { doTranslateRef.current = doTranslate; }, [doTranslate]);
```

> **100ms 延迟：** 等 `translateMode` state 提交 + 事件监听 `useEffect` 挂载完成后再首翻，否则 `translate-progress` 事件可能没人监听。

- [ ] **Step 6: translate-now 按钮接入 doTranslate**

将 Task 6 Step 5 中 `translate-now` 工具的 `onClick` 从 `() => {}` 改为：

```tsx
    { id: "translate-now", icon: "redo", label: t("result.translateNow"), disabled: translating || translateMode === 'off', onClick: doTranslate },
```

- [ ] **Step 7: 保存语义——终翻 + 提交译文 + 退出翻译模式**

在 `tools` 数组之前新增 `onSave` 回调：

```tsx
  const onSave = useCallback(async () => {
    if (translateModeRef.current === 'off') {
      asrEditorRef.current?.commit();
      return;
    }
    // 翻译态：先提交原文编辑（拿最新原文），再终翻，最后提交译文
    asrEditorRef.current?.commit();
    const finalText = await finalTranslate();
    const submitText = finalText && !finalText.startsWith("❌")
      ? finalText
      : translatedTextRef.current;
    invoke("commit_edit", {
      text: submitText,
      dirtyRanges: [],
      hasEdited: false,
      caret: null,
      selection: null,
    });
    exitTranslateMode();
  }, [finalTranslate, exitTranslateMode]);
```

新增 `translatedTextRef`（onSave 闭包防陈旧）：

```tsx
  const translatedTextRef = useRef("");
  useEffect(() => { translatedTextRef.current = translatedText; }, [translatedText]);
```

将 `tools` 数组中 `save` 按钮的 `onClick` 从 `() => asrEditorRef.current?.commit()` 改为 `onSave`：

```tsx
    { id: "save", icon: "save" as IconName, label: t("result.save"), onClick: onSave },
```

- [ ] **Step 8: Cmd+T 快捷键**

在 keydown handler `useEffect`（约 212 行的 `onKeyDown`）中，`if (e.key === "Escape")` 之后、`const sc = parseShortcut(...)` 之前新增：

```tsx
      if (e.metaKey && e.key === 't') {
        e.preventDefault();
        if (translateModeRef.current !== 'off') doTranslateRef.current();
        return;
      }
```

- [ ] **Step 9: 全量编译 + 类型检查**

Run: `cd crates/desktop/frontend && npx tsc --noEmit 2>&1 | tail -5 && npx vitest run 2>&1 | tail -10`
Expected: tsc 无 error，vitest 全部 PASS。

- [ ] **Step 10: Rust 编译验证**

Run: `cargo build -p octopus-desktop --features embedded 2>&1 | tail -5`
Expected: 编译通过。

- [ ] **Step 11: Commit**

```bash
git add crates/desktop/frontend/src/pages/Result/index.tsx
git commit -m "feat(result): 翻译执行逻辑——doTranslate + 节流 + 终翻 + Cmd+T + 保存提交译文"
```

---

### Task 8: architecture.md 文档同步

**Files:**
- Modify: `docs/architecture.md:207`（result_window 行）

- [ ] **Step 1: 补充翻译双语视图说明**

在 `docs/architecture.md` 的 `result_window` 行（约 207 行），在现有 CM6 改造说明之后追加翻译模式说明。找到该行末尾的 CM6 说明文本，在其后追加：

```
**翻译双语视图（2026-07-12）**：工具栏翻译按钮（languages 图标）→ 自动进大窗 + 文本区从单语 AsrEditor 变上下分栏（上原文 AsrEditor + 下译文 TranslationPane）。翻译模式 `TranslateMode: off/manual/8s/12s/15s`——`off` 为单语（默认），其余为翻译态。进入即首翻一次（`translate_text` fire-and-forget），自动档（8/12/15s）每 N 秒节流检查原文变化后重翻（翻译中跳过），手动档靠「立即翻译」按钮 + `Cmd+T` 快捷键。保存时终翻一次最新全文 → 提交**译文**到光标 → 自动退出翻译模式。档位偏好写 DB app_config 表（`translate_mode` 键，默认 `manual`）。移除工具栏 settings 按钮（改经托盘菜单进入）。详见 [spec](superpowers/specs/2026-07-12-result-window-translation-bilingual-design.md)。
```

- [ ] **Step 2: Commit**

```bash
git add docs/architecture.md
git commit -m "docs(sync): architecture 补 Result 浮窗翻译双语视图说明"
```

---

### Task 9: 最终验证

- [ ] **Step 1: 前端全量编译 + 单测**

```bash
cd crates/desktop/frontend && npx tsc --noEmit 2>&1 | tail -5 && npx vitest run 2>&1 | tail -10
```

Expected: 全部 PASS。

- [ ] **Step 2: Rust 全量编译**

```bash
cargo build -p octopus-desktop --features embedded 2>&1 | tail -5
```

Expected: 编译通过。

- [ ] **Step 3: 最终 git log 确认**

```bash
git log --oneline -10
```

Expected: 看到 7-8 个 feat/docs 提交，对应 Task 1-8。

- [ ] **Step 4: 手动验证清单**

启动桌面应用 `cargo run --release -p octopus-desktop --features embedded`，逐项验证：

1. 全局热键唤起 Result 浮窗 → 工具栏无 settings 按钮
2. 点翻译按钮 → 自动进大窗 + 上下分栏 + 首翻一次
3. 下拉切"自动 8s" → 持续说话 → 译文每 ~8s 跟进
4. 点「立即翻译」→ 立即重翻
5. `Cmd+T` → 等同立即翻译
6. 保存（Cmd+Enter 或工具栏保存）→ 终翻 + 提交译文 + 退出翻译模式回单语
7. 重启应用 → 进翻译模式 → 默认用上次记忆的档位
8. 翻译引擎未配置 → 译文区显示错误文案
