# 翻译结果左右对比展示设计

> **状态**：设计已确认，待编写实施计划
> **日期**：2026-07-12
> **分支**：`translation-bilingual-view`
> **scope**：在 CompactEditor 图文编辑器新增「翻译对照」视图模式，左原文 / 右译文双栏并排，两侧均可编辑（各自 markdown 编辑/预览），新增视图布局切换（只原文 / 对照 / 只译文）
> **前置文档**：
> - [`2026-07-12-local-translation-engine-design.md`](./2026-07-12-local-translation-engine-design.md)（本地翻译引擎，contrast 模式复用其 `detect_translate_direction` + `resolve_translate_strategy`）
> - [`2026-07-11-result-editor-design.md`](./2026-07-11-result-editor-design.md)（CompactEditor CM6 架构基础）

---

## 1. 背景与动机

当前翻译流程：action bar 选中文本 → 翻译 → 结果以 `【翻译】\n{译文}` 形式开一个 CompactEditor **临时 tab**，**原文被丢弃**，用户只能看到译文，无法对照原文校译。

本次改造为 CompactEditor 新增「翻译对照」视图模式：

- 左原文 / 右译文双栏并排
- 两侧均可编辑，各自独立 markdown 编辑/预览
- 新增视图布局切换：只原文 / 对照 / 只译文
- 现有 single 模式（editor / split / preview）完全保留，不动

### 设计约束（已确认）

| 决策点 | 选择 |
|--------|------|
| 改造方式 | **新增**对照视图模式，不替换现有 editor/split/preview |
| 两栏可编辑性 | **两侧都可编辑**，各自只有「编辑 / 预览」两态（无内部分栏） |
| 触发方式 | **C：翻译自动进对照 + 普通文本 tab 点翻译切对照** |
| 普通文本 tab 翻译原文取法 | **有选区翻选区，无选区翻全文** |
| 保存语义 | **只存译文（右半）**，原文是脚手架不持久化 |
| 组件结构 | **新增独立 `TranslationContrastPane` 组件**，不拼两个 MarkdownPane |
| 视图切换 | **只原文 / 对照 / 只译文 三态**，正交于每列的编辑/预览 |

---

## 2. 数据模型

### 2.1 Tab 接口扩展

`Tab` 接口新增 3 个可选字段（兼容现有 single tab）：

```ts
export interface Tab {
  // ... 现有字段 ...
  mode?: 'single' | 'contrast';        // 默认 'single'
  originalText?: string;                 // 对照模式左半（原文）
  translatedText?: string;               // 对照模式右半（译文）
}
```

### 2.2 规则

- `mode='single'`（默认）：走现有 `MarkdownPane`，忽略 originalText/translatedText
- `mode='contrast'`：渲染 `TranslationContrastPane`，读 originalText/translatedText 作为两列初值，组件内部 `useState` 维护编辑态
- **single→contrast 不可逆**：切回 single 时 `tab.text = translatedText`（丢弃原文），`mode='single'`，清空 originalText/translatedText
- **关 tab 重开恢复 single**：contrast tab 关闭后从 DB 重开（已 promote 为 clipboard 条目）→ 读到的是 single 模式（DB 只有译文文本），mode 默认 single。符合"原文是脚手架，不持久化"

### 2.3 入口

1. **action bar 翻译** → 后端 `open_temp_compact_editor` 携带 `originalText` + `translatedText` + `mode='contrast'` → temp tab
2. **普通文本 tab 工具栏「翻译」按钮** → 前端调用翻译命令 → 切该 tab `mode='contrast'`，`originalText = tab.text`（或选区），`translatedText = 译文`

---

## 3. 翻译触发与数据流

### 3.1 入口 1：action bar 翻译（选中文本）

改现有 `execute_action_bar_inner`（`action_bar_commands.rs`）：

- **本地引擎路径**：`std::thread::spawn` 翻译后，调 `open_temp_compact_editor` 携带 `mode="contrast"` + `original_text` + `translated_text`（当前只传译文）
- **LLM 路径**：`action_bar_show_result` 新增 `original_text` 参数透传，同样以 contrast temp tab 打开

翻译方向复用现有 `detect_translate_direction`（CJK→en，否则→zh）。

### 3.2 入口 2：普通 tab 工具栏翻译（新增前端命令）

- 用户点工具栏「翻译」按钮 → 前端读选区（CM6 有 API 取）或全文 → `invoke("translate_text", { text })`
- 后端 `translate_text` 命令：解析引擎策略（复用 `resolve_translate_strategy`），执行翻译返回译文字符串
- 前端拿到译文 → 切该 tab `mode='contrast'`，`originalText = 选中或全文`，`translatedText = 译文`
- 翻译前先清空选区光标到末尾（避免对照模式下选区残留）

### 3.3 入口 3：截图翻译（OCR → 翻译 → 对照）—— 本次仅预留架构

截图翻译是待实现组合动作（OCR → 翻译 → CompactEditor 对照），见 [`action-bar-related-tools-survey.md` §6.1.4](./2026-07-09-action-bar-related-tools-survey.md)。

- **数据通路本次打通**：contrast 数据流支持 OCR markdown 文本作为 `originalText`
- **触发 UI 本次不实现**：截图翻译涉及截图工具栏改动 + OCR 等待编排，scope 较大，作为独立后续任务
- 后续 `ocr_screenshot_translate` 命令：OCR markdown → translate → `open_temp_compact_editor(contrast)`，originalText=OCR 文本，translatedText=译文。架构零改动，仅新增编排命令

### 3.4 temp contrast tab 保存

- `mode='contrast'` 的 temp tab 升级时，DB 存 `translatedText`（译文），原文丢弃——与 §1 保存语义一致
- 走现有 `promoteTempTab`，仅 `set_clipboard_item_text(itemId, translatedText)`

---

## 4. TranslationContrastPane 组件设计

### 4.1 Props 接口

```tsx
interface TranslationContrastPaneProps {
  originalText: string;
  translatedText: string;
  readOnly: boolean;
  fontSize: number;
  onFontSizeChange: (n: number) => void;
  onOriginalChange: (s: string) => void;
  onTranslatedChange: (s: string) => void;
  onTranslate: () => void;        // 触发重新翻译（原文→右半）
  onSave: () => void;             // 存译文
  disableSave?: boolean;
  savedFlash: boolean;
}
```

### 4.2 布局

全宽 flex，左右各 50%（contrast 布局时）：

```
┌─────────────────────────────────────────┐
│ 工具栏（字号 / 视图布局 / 左编辑·预览    │
│   右编辑·预览 / 翻译 / 保存）             │
├──────────────────┬──────────────────────┤
│  原文区            │  译文区               │
│  ┌─[编辑][预览]─┐  │  ┌─[编辑][预览]──┐   │
│  │ CodeMirror   │  │  │ CodeMirror    │   │
│  │ 或 Preview   │  │  │ 或 Preview    │   │
│  └──────────────┘  │  └───────────────┘   │
└──────────────────┴──────────────────────┘
```

### 4.3 内部状态

**每列内部状态**（互不影响）：

- `leftMode: 'editor' | 'preview'`
- `rightMode: 'editor' | 'preview'`
- 各自独立的 CM6 实例（复用 `CodeMirrorEditor`）+ `MarkdownPreview`
- 各列内只有「编辑 / 预览」两态按钮，**不再有 split**（外层已分栏）

**视图布局状态**：

- `viewLayout: 'left' | 'contrast' | 'right'`（默认 `contrast`）
- `left`：只显示左列（原文），占 100% 宽
- `contrast`：左右各 50%（默认）
- `right`：只显示右列（译文），占 100% 宽

**正交关系**：视图布局管"显示哪几列"，每列的 `editor/preview` 管"该列怎么显示"。组合出 3×2×2 种状态。

### 4.4 工具栏分组

```
[字号-] [15] [字号+]  |  [只原文][对照][只译文]  |  左:[编辑][预览]  右:[编辑][预览]  |  [翻译] [保存]
```

- **左段**：字号 ±（共享）
- **中段**：视图布局切换组（`PanelLeft` / `Columns2` / `PanelRight` 图标）
- **中右段**：左列 [编辑][预览] · 右列 [编辑][预览]
- **右段**：[翻译]（`Languages` 图标）→ `onTranslate` / [保存] → `onSave`

### 4.5 视图布局切换实现

三个列容器始终挂载（保持 CM6 状态），用 `display: none` 切换可见性——与 MarkdownPane 现有 split/preview 切换手法一致，不丢光标/撤销栈。

### 4.6 滚动

左右各自独立滚动（**非同步**）——原文与译文长短不同、语义段落未必对齐，强制同步反而别扭。

### 4.7 翻译按钮语义

- 点击 = 拿当前 `originalText` 重新翻译，结果**替换** `translatedText`（覆盖右半）
- 若用户已手动编辑译文（`dirtyTranslated` 标记），弹 2 秒确认气泡防误覆盖（复用 MarkdownPane 的 `clearPending` 双击确认模式）

### 4.8 尺寸

窗口现有 880×620（compact_editor_window 默认），对比模式两列各 ~430px，够用。不新建窗口、不改窗口大小记忆。

---

## 5. 后端命令改动

### 5.1 `open_temp_compact_editor` 扩参（`compact_editor_commands.rs`）

当前签名：`(app, text: &str)`。改为携带对照参数：

```rust
pub struct TempTabPayload {
    pub text: String,                     // 单栏文本（mode=single 时用）
    pub mode: Option<String>,             // "contrast" | None
    pub original_text: Option<String>,    // 对照原文
    pub translated_text: Option<String>,  // 对照译文
}

pub fn open_temp_compact_editor(app: &AppHandle, payload: TempTabPayload);
```

- 现有调用点（托盘「图文编辑」传 `text=""`，action bar 非翻译 AI 传 `text=显示文本`）改为包成 `TempTabPayload { text, mode:None, .. }`，行为不变
- 翻译调用点传 `mode="contrast"` + original + translated

**事件 payload 扩展**：`compact-editor://open-tab` 的 temp 分支同理携带 mode/original/translated；`PendingTabFull` 加对应 3 字段。

### 5.2 新增 `translate_text` 命令

位置：`action_bar_commands.rs`（或 `compact_editor_commands.rs`，与翻译策略逻辑同 crate）。

```rust
#[tauri::command]
pub fn translate_text(text: String, app: AppHandle) -> Result<String, String>;
```

- 复用 `detect_translate_direction` + `resolve_translate_strategy`
- 本地引擎：同步执行（短文本 <2s，可接受）返回译文
- LLM：同步调用 `chat_text_with_prompt`
- 供普通 tab 工具栏翻译按钮调用，返回纯译文字符串

### 5.3 action bar 翻译路径改 contrast

- `execute_action_bar_inner` 的 local 分支：`display` 从 `format!("【翻译】\n{}", translated)` 改为 `TempTabPayload { mode:"contrast", original_text:text, translated_text:translated }`
- LLM 分支：`action_bar_show_result` 加 `original_text` 参数，内部改调 contrast 版 `open_temp`

### 5.4 无 DB schema 改动

contrast 的原文不持久化，保存只写译文（=普通 clipboard 条目）。

---

## 6. 边界情况与降级

### 6.1 翻译失败

- **入口 1（action bar）**：译文填 `format!("❌ 翻译失败：{}", e)`，仍以 contrast 打开（原文正常显示，右半是错误信息，用户可手动编辑覆盖）
- **入口 2（普通 tab 工具栏）**：`translate_text` 返回 Err → 前端 toast 提示「翻译失败」+ 不切 contrast（保留 single 模式原 tab 不动）

### 6.2 引擎未配置 / 未下载

`resolve_translate_strategy` 返回 Llm 但 LLM 也未配置 → `translate_text` 返回 `Err("翻译引擎未配置，请在设置中配置本地翻译模型或 LLM")`，前端同上 toast。

### 6.3 temp contrast tab 保存为空译文

用户清空右半译文 → `translatedText.trim() == ""` → 复用现有 temp 逻辑：关 tab（仅一个）或移除该 tab（多个）。原文一并丢弃，不单独保存。

### 6.4 single→contrast 不可逆但可中途关 tab 重开

contrast tab 关闭后从 DB 重开（已 promote 为 clipboard 条目）→ 读到的是 single 模式（DB 只有译文文本），mode 默认 single。符合"原文是脚手架，不持久化"。

### 6.5 readOnly tab（transcription 语音记录）不提供翻译按钮

工具栏翻译按钮在 `readOnly=true` 时隐藏——语音 tab 只读，不触发翻译转 contrast。action bar 已能对任意选中文本翻译。

### 6.6 翻译中二次点击翻译按钮

`translating` 布尔 state 锁定按钮（disabled + spinner 图标），翻译完成解锁。防重复请求。

### 6.7 截图翻译数据通路（本次不实现 UI，但 contrast 数据流已支持）

后续 `ocr_screenshot_translate` 命令：OCR markdown → translate → `open_temp_compact_editor(contrast)`，originalText=OCR 文本，translatedText=译文。架构零改动，仅新增编排命令。

### 6.8 原文超长

m2m100 greedy `max_length=200` tokens，超长原文会截断翻译。不特殊处理——CM6 正常显示长原文，译文显示引擎返回的部分，用户可手动补。

---

## 7. 新增/修改文件

### 7.1 新增

| 文件 | 职责 | 行数估算 |
|------|------|----------|
| `frontend/src/pages/CompactEditor/TranslationContrastPane.tsx` | 对照视图组件（双栏 + 视图布局切换 + 每列编辑/预览 + 翻译/保存） | ~280 |

### 7.2 修改

| 文件 | 改动 |
|------|------|
| `frontend/src/pages/CompactEditor/index.tsx` | `Tab` 加 mode/originalText/translatedText 字段；`pendingToTab` / `OpenTabPayload` / `listen` temp 分支携带新字段；渲染区按 `mode` 分流 MarkdownPane vs TranslationContrastPane；`doSave` 对 contrast 取 translatedText；普通文本 tab 工具栏加翻译入口（挂给 MarkdownPane 或独立按钮） |
| `crates/desktop/src/compact_editor_commands.rs` | `open_temp_compact_editor` 改收 `TempTabPayload`；`PendingTabFull` 加 3 字段；事件 payload 携带对照参数 |
| `crates/desktop/src/action_bar_commands.rs` | `execute_action_bar_inner` local/LLM 翻译分支改调 contrast 版 open_temp；`action_bar_show_result` 加 `original_text` 参数；新增 `translate_text` 命令 |
| `crates/desktop/src/tray.rs` | `open_temp_compact_editor` 调用点包成 `TempTabPayload { text:"", .. }` |
| `crates/desktop/src/lib.rs` | 注册 `translate_text` 命令 |

### 7.3 不变

- DB schema（无新表、无新列）
- `CodeMirrorEditor` / `MarkdownPreview` 组件（contrast 复用）
- 现有 single 模式的 MarkdownPane（不动）
- 窗口尺寸 / 位置记忆 / 透明策略

---

## 8. 不在本次范围

- **截图翻译触发 UI**（OCR → 翻译编排命令）——数据通路已支持，UI 后续独立任务
- **对照模式的原文持久化**——原文是脚手架，关 tab 即丢，不进 DB
- **左右滚动同步**——原文译文长短不同，强制同步反而不便
- **翻译历史 / 多版本对比**——一期仅当前一次翻译结果
- **自动翻译（输入原文实时翻译）**——手动触发翻译按钮
