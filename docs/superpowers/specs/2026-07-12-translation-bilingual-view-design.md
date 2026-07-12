# 翻译结果左右对比展示设计

> **状态**：已实现
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
| 普通文本 tab 翻译原文取法 | **有选区翻选区，无选区翻全文**（实现简化为翻全文） |
| 保存语义 | **只存译文（右半）**，原文是脚手架不持久化 |
| 组件结构 | **新增独立 `TranslationContrastPane` 组件**，不拼两个 MarkdownPane |
| 视图切换 | **只原文 / 对照 / 只译文 三态**，正交于每列的编辑/预览 |
| 编辑/预览切换 | **单 toggle 按钮**（点击切换两态，显示目标模式图标），非两个按钮 |
| splitter | **可拖拽 splitter** 调整左右比例，复用 MarkdownPane 拖拽模式，比例持久化 localStorage |
| 翻译执行 | **流式**：立即开编辑器，后台逐段翻译 emit `translate-progress`/`translate-done` |
| 引擎选择 | **Opus-MT 优先**（轻量 30M），其次 m2m100（418M），其次 LLM |

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
- 后端 `translate_text` 命令：fire-and-forget（立即返回，后台 spawn 线程翻译，结果通过 `emit("translate-progress")` / `emit("translate-done")` 事件返回）
- 前端立即切该 tab `mode='contrast'`（译文区显示 loading），listen 事件更新 `translatedText`
- `translate_text` 返回 `Result<(), String>`（不返回译文）

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
[字号-] [15] [字号+]  |  [只原文][对照][只译文]  |  左:[toggle编辑/预览]  右:[toggle编辑/预览]  |  [翻译] [保存]
```

- **左段**：字号 ±（共享）
- **中段**：视图布局切换组（`PanelLeft` / `Columns2` / `PanelRight` 图标）
- **中右段**：左列 / 右列各自一个 **toggle 按钮**（当前编辑态显示 Eye 图标→点切预览，当前预览态显示 FileText 图标→点切编辑），非两个独立按钮
- **右段**：[翻译]（`Languages` 图标）→ `onTranslate` / [保存] → `onSave`

### 4.5 视图布局切换 + Splitter

- 三个列容器始终挂载（保持 CM6 状态），用 `display: none` 切换可见性——与 MarkdownPane 现有 split/preview 切换手法一致，不丢光标/撤销栈
- **contrast 模式下中间有可拖拽 splitter**：grid 布局（`${splitRatio * 100}% 1px ${(1 - splitRatio) * 100}%`），拖拽 PointerEvent 实时更新比例（20%-80%），释放时持久化到 `localStorage`（key `contrast-split-ratio`），复用 MarkdownPane 的 splitter 模式
- **splitter 颜色** `bg-muted-foreground/30`（比 `bg-border` 深一档，与 CM6 行号线视觉区分，避免混淆）；hover 变 voice 色
- 仅 contrast 模式显示 splitter，left / right 模式隐藏

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
pub fn translate_text(text: String, app: AppHandle) -> Result<(), String>;
```

- fire-and-forget：立即返回 `Ok(())`，后台 `std::thread::spawn` 翻译，结果通过 `emit("translate-progress", accumulated)` / `emit("translate-done", final)` 事件返回
- 新增 `do_translate_streaming(text, app)` 公共函数：按换行切分段落，逐段调 `do_translate`，每段完成 emit 增量结果
- `do_translate(text, config)` 对 opus-mt 走 `load_opus_mt(source, target)` 按方向加载引擎
- 前端 listen `translate-progress`（实时更新译文区）+ `translate-done`（清 loading 状态）

### 5.3 action bar 翻译路径改 contrast

- `execute_action_bar_inner` 的 local 分支：`display` 从 `format!("【翻译】\n{}", translated)` 改为 `TempTabPayload { mode:"contrast", original_text:text, translated_text:translated }`
- LLM 分支：`action_bar_show_result` 加 `original_text` 参数，内部改调 contrast 版 `open_temp`

### 5.4 无 DB schema 改动

contrast 的原文不持久化，保存只写译文（=普通 clipboard 条目）。

---

## 6. 边界情况与降级

### 6.1 翻译失败



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
| `frontend/src/pages/CompactEditor/index.tsx` | `Tab` 加 mode/originalText/translatedText 字段；`pendingToTab` / `OpenTabPayload` / `listen` temp 分支携带新字段；渲染区按 `mode` 分流 MarkdownPane vs TranslationContrastPane；`doSave` 对 contrast 取 translatedText；普通文本 tab 工具栏加翻译入口；listen translate-progress/done 事件流式更新 |
| `crates/desktop/src/compact_editor_commands.rs` | `open_temp_compact_editor` 改收 `TempTabPayload`；`PendingTabFull` 加 3 字段；事件 payload 携带对照参数 |
| `crates/desktop/src/action_bar_commands.rs` | `execute_action_bar_inner` local/LLM 翻译分支改流式 contrast（立即开 tab + 后台 emit）；`action_bar_show_result` 加 `original_text` 参数；新增 `translate_text` 命令（fire-and-forget）+ `do_translate_streaming` + `do_translate` |
| `crates/desktop/src/tray.rs` | `open_temp_compact_editor` 调用点包成 `TempTabPayload { text:"", .. }` |
| `crates/desktop/src/main.rs` | 注册 `translate_text` 命令 |
| `crates/translation/src/engine.rs` | 缓存改 HashMap 按 spec+方向分 key；新增 `load_opus_mt(source, target)` |
| `crates/translation/src/discovery.rs` | KNOWN_MODELS 加 opus-mt；`check_opus_mt()` 检查 zh-en + en-zh 子目录 |
| `crates/translation/Cargo.toml` | 加 `serde_json` 依赖 |

### 7.3 新增（翻译引擎）

| 文件 | 职责 |
|------|------|
| `crates/translation/src/opus_mt.rs` | Opus-MT MarianMT 引擎：encoder-decoder greedy，按方向加载 zh-en/en-zh 子目录，从 generation_config 读 token IDs，tokenizer precompiled_charsmap=null 修复 |

### 7.4 不变

- DB schema（无新表、无新列）
- `CodeMirrorEditor` / `MarkdownPreview` 组件（contrast 复用）
- 现有 single 模式的 MarkdownPane（不动）
- 窗口尺寸 / 位置记忆 / 透明策略

---

## 9. 翻译引擎：Opus-MT 接入

### 9.1 模型信息

| 属性 | 值 |
|------|-----|
| 架构 | MarianMT（encoder-decoder，FairSeq 系） |
| 参数量 | ~30M/方向（vs m2m100 418M） |
| ONNX | int8 量化（encoder_model_int8.onnx + decoder_model_int8.onnx） |
| d_model | 512（vs m2m100 1024） |
| layers | 6+6（vs m2m100 12+12） |
| vocab_size | 65001 |
| 方向 | 1对1（需 zh-en + en-zh 两个子目录） |
| tokenizer | HF tokenizers（tokenizer.json） |
| decoder_start_id | 65000（从 generation_config.json 读） |
| eos_id | 0 |

### 9.2 目录结构

```
~/.octopus/models/translate/opus-mt/
├── zh-en/   → Xenova/opus-mt-zh-en (HF cache symlink)
│   ├── onnx/encoder_model_int8.onnx
│   ├── onnx/decoder_model_int8.onnx
│   ├── tokenizer.json
│   ├── config.json
│   └── generation_config.json
└── en-zh/   → Xenova/opus-mt-en-zh (HF cache symlink)
    └── (同上)
```

一组模型在设置页算一个（两个方向需都存在才算 downloaded）。

### 9.3 tokenizer precompiled_charsmap=null 修复

Xenova 导出的 tokenizer.json 中 `normalizer.precompiled_charsmap` 为 `null`，`tokenizers` crate 0.21.4 遇到 null 直接 panic（`Precompiled: Error("invalid type: null")`）。修复：加载时解析 JSON，删除整个 `normalizer` 字段（MarianMT 不需要 normalization）。

### 9.4 引擎选择优先级（自动模式）

1. **opus-mt**（轻量 30M，中英互译）—— 已下载则优先
2. **m2m100-418M**（多语言 100+）—— opus-mt 未下载时 fallback
3. **LLM**（远程）—— 本地引擎均未下载时

用户可在设置页手动选择 `local:opus-mt` / `local:m2m100` / `自动` / `LLM`。

### 9.5 引擎缓存

`engine.rs` 全局缓存改为 `HashMap<String, Arc<dyn TranslationEngine>>`：
- m2m100：按 spec key 缓存（如 `local:m2m100-418M`）
- opus-mt：按 spec+方向 key 缓存（如 `local:opus-mt-zh-en`），因为不同方向加载不同子目录

---

## 10. 不在本次范围

- **截图翻译触发 UI**（OCR → 翻译编排命令）——数据通路已支持，UI 后续独立任务
- **对照模式的原文持久化**——原文是脚手架，关 tab 即丢，不进 DB
- **左右滚动同步**——原文译文长短不同，强制同步反而不便
- **翻译历史 / 多版本对比**——一期仅当前一次翻译结果
- **自动翻译（输入原文实时翻译）**——手动触发翻译按钮
