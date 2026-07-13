# 翻译结果左右对比展示 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [x]`) syntax for tracking.

**Goal:** 为 CompactEditor 图文编辑器新增「翻译对照」视图模式，左原文 / 右译文双栏并排，两侧均可编辑（各自 markdown 编辑/预览），新增视图布局切换（只原文 / 对照 / 只译文）。

**Architecture:** Tab 新增 `mode/originalText/translatedText` 字段。`mode='contrast'` 时渲染新增的 `TranslationContrastPane` 组件（替代 `MarkdownPane`），内含左右两列，每列复用现有 `CodeMirrorEditor` + `MarkdownPreview`，各自独立的编辑/预览切换 + 视图布局切换。后端 `open_temp_compact_editor` 扩参支持对照 payload；新增 `translate_text` 命令供普通 tab 工具栏翻译。

**Tech Stack:** Rust + Tauri 2（后端命令）、React + TypeScript + CodeMirror 6（前端）、Vitest（单测）。

## Global Constraints

- **不改动 DB schema**——原文不持久化，保存只写译文
- **不动现有 single 模式**——MarkdownPane 的 editor/split/preview 三态保留
- **不改窗口尺寸**——复用现有 880×620 compact_editor_window
- **复用现有组件**——`CodeMirrorEditor`、`MarkdownPreview`、`ToolBtn` 模式、`clearPending` 双击确认模式
- **i18n 双语**——所有新增文案同步 `zh-CN.yaml` 和 `en.yaml`
- **不使用 em dash**——代码和文档中用逗号/句号/括号代替
- **测试命令**——`cd crates/desktop/frontend && npx vitest run`（前端单测）；`cargo build --release -p octopus-desktop --features embedded`（后端编译）

---

## File Structure

| 文件 | 操作 | 职责 |
|------|------|------|
| `crates/desktop/frontend/src/pages/CompactEditor/TranslationContrastPane.tsx` | **新建** | 对照视图组件（双栏 + 视图布局切换 + 每列编辑/预览 + 翻译/保存） |
| `crates/desktop/frontend/src/pages/CompactEditor/index.tsx` | **修改** | Tab 加 mode/originalText/translatedText；payload 携带；渲染分流；doSave 处理 contrast；普通文本 tab 工具栏翻译入口 |
| `crates/desktop/frontend/src/pages/CompactEditor/promoteTempTab.ts` | **修改** | contrast temp tab 升级时同步 mode 字段 |
| `crates/desktop/frontend/src/pages/CompactEditor/promoteTempTab.test.ts` | **修改** | 补 contrast temp 升级用例 |
| `crates/desktop/frontend/src/locales/zh-CN.yaml` | **修改** | 新增 contrast.* / editor.translate 等 key |
| `crates/desktop/frontend/src/locales/en.yaml` | **修改** | 同上英文 |
| `crates/desktop/src/compact_editor_commands.rs` | **修改** | `open_temp_compact_editor` 改收 payload struct；`PendingTabFull` 加字段；事件 payload 扩展 |
| `crates/desktop/src/action_bar_commands.rs` | **修改** | 翻译分支改调 contrast 版 open_temp；新增 `translate_text` 命令；提取公共翻译执行函数 |
| `crates/desktop/src/tray.rs` | **修改** | `open_temp_compact_editor` 调用点包成 payload |
| `crates/desktop/src/main.rs` | **修改** | 注册 `translate_text` 命令 |

---

### Task 1: 后端 contrast payload 结构

**Files:**
- Modify: `crates/desktop/src/compact_editor_commands.rs:31-43`（`PendingTabFull` 加字段）
- Modify: `crates/desktop/src/compact_editor_commands.rs:76-105`（`store_pending_temp_tab` + `open_temp_compact_editor`）

**Interfaces:**
- Produces: `TempTabPayload` struct、`open_temp_compact_editor(app, payload: &TempTabPayload)` 签名。后续 task 的 action_bar / tray 调用方依赖此签名。

**背景：** 当前 `open_temp_compact_editor(app, text: &str)` 只传单段文本。需扩展为携带 `mode`/`original_text`/`translated_text` 的 struct，兼容现有调用方（托盘、action bar 非翻译 AI）。

- [x] **Step 1: 新增 `TempTabPayload` struct 并扩展 `PendingTabFull`**

在 `compact_editor_commands.rs` 的 `PendingTabFull` struct 后新增 `TempTabPayload`，并给 `PendingTabFull` 加 3 个 `#[serde(default)]` 字段：

```rust
/// 临时 tab 打开参数（不写 DB）。mode=None 为单栏（现有行为），mode="contrast" 为翻译对照。
#[derive(Clone, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct TempTabPayload {
    /// 单栏文本（mode=None 时用）
    #[serde(default)]
    pub text: String,
    /// "contrast" | None
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<String>,
    /// 对照原文（mode=contrast 时用）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub original_text: Option<String>,
    /// 对照译文（mode=contrast 时用）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub translated_text: Option<String>,
}
```

在 `PendingTabFull` 的 `is_temp` 字段后追加：

```rust
    /// 对照模式（mode=contrast）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<String>,
    /// 对照原文
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub original_text: Option<String>,
    /// 对照译文
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub translated_text: Option<String>,
```

- [x] **Step 2: 改 `store_pending_temp_tab` 签名为收 `TempTabPayload`**

将现有 `store_pending_temp_tab(text: String, source: &str)` 替换为：

```rust
/// 存储临时 tab（不查 DB，payload 直接传入）。
pub fn store_pending_temp(payload: TempTabPayload, source: &str) {
    PENDING_TABS.lock().push(PendingTabFull {
        item_id: 0,
        source: source.to_string(),
        item_type: "text".into(),
        text: payload.text,
        img_width: 0,
        img_height: 0,
        is_temp: true,
        mode: payload.mode,
        original_text: payload.original_text,
        translated_text: payload.translated_text,
    });
}
```

- [x] **Step 3: 改 `open_temp_compact_editor` 签名**

将现有 `open_temp_compact_editor(app: &tauri::AppHandle, text: &str)` 替换为收 `TempTabPayload`：

```rust
/// 打开 CompactEditor 并定位到一个临时 tab（不写 DB）。
/// payload.mode=None 为单栏（现有行为）；payload.mode="contrast" 为翻译对照（左原文右译文）。
/// 窗口已存在 → emit 推送新 temp tab；窗口不存在 → store_pending_temp + 建窗。
pub fn open_temp_compact_editor(app: &tauri::AppHandle, payload: &TempTabPayload) {
    if let Some(window) = app.get_webview_window(WINDOW_LABEL) {
        let _ = window.emit("compact-editor://open-tab", serde_json::json!({
            "itemId": 0,
            "source": "temp",
            "text": payload.text,
            "isTemp": true,
            "mode": payload.mode,
            "originalText": payload.original_text,
            "translatedText": payload.translated_text,
        }));
        let _ = window.show();
        let _ = window.set_focus();
    } else {
        store_pending_temp(payload.clone(), "temp");
        create_compact_editor_window(app, None);
    }
}
```

注意：`TempTabPayload` 需 derive `Clone`（Step 1 已含）。

- [x] **Step 4: 编译验证**

```bash
cargo build --release -p octopus-desktop --features embedded 2>&1 | tail -20
```

Expected: 编译失败（调用方 `tray.rs` / `action_bar_commands.rs` 仍传 `&str`）——这是预期的，Task 2/3 会修复。记录报错文件列表确认是预期的调用方。

---

### Task 2: 修复 open_temp 调用方（tray + action bar 非翻译路径）

**Files:**
- Modify: `crates/desktop/src/tray.rs:127`
- Modify: `crates/desktop/src/action_bar_commands.rs:206-207`（`action_bar_show_result` 内 `open_temp_compact_editor` 调用）
- Modify: `crates/desktop/src/action_bar_commands.rs:176-208`（`action_bar_show_result` 签名加 `original_text`）

**Interfaces:**
- Consumes: Task 1 的 `TempTabPayload` + `open_temp_compact_editor(app, &TempTabPayload)`
- Produces: `action_bar_show_result(result, original_text, action, app, write_clipboard)` 新签名。Task 3 的翻译路径依赖此签名。

- [x] **Step 1: 修 `tray.rs` 调用点**

找到 `tray.rs` 中 `open_temp_compact_editor(app, "")` 调用（约 127 行），改为：

```rust
crate::compact_editor_commands::open_temp_compact_editor(app, &Default::default());
```

- [x] **Step 2: 修 `action_bar_show_result` 签名 + 非翻译路径**

将 `action_bar_show_result` 签名改为加 `original_text: String` 参数：

```rust
pub fn action_bar_show_result(result: String, original_text: String, action: String, app: AppHandle, write_clipboard: bool) {
```

函数体内 `open_temp_compact_editor` 调用改为构建 `TempTabPayload`。翻译 action（`action == "translate"`）用 contrast，其他用 single：

```rust
    let payload = if action == "translate" && !original_text.is_empty() {
        crate::compact_editor_commands::TempTabPayload {
            text: format!("【翻译】\n{}", result),
            mode: Some("contrast".into()),
            original_text: Some(original_text),
            translated_text: Some(result.clone()),
        }
    } else {
        crate::compact_editor_commands::TempTabPayload {
            text: display_text,
            ..Default::default()
        }
    };
    crate::compact_editor_commands::open_temp_compact_editor(&app, &payload);
```

注意：`display_text` 仍由现有 label 匹配逻辑生成（翻译/润色/摘要/解释）。

- [x] **Step 3: 更新 `action_bar_show_result` 的所有调用点**

搜索 `action_bar_show_result(` 的调用（约 3 处：润色/摘要/解释的 AI 路径 + LLM 翻译路径），每处加 `original_text` 参数。非翻译路径传 `String::new()`：

- 约 729 行 LLM 翻译分支：`action_bar_show_result(result, text, item.title, app.clone(), true)` —— 传 `text`（选中的原文）
- 约 740 行 非 translate AI：`action_bar_show_result(result, String::new(), item.title, app.clone(), true)`

用 `grep -n "action_bar_show_result" crates/desktop/src/action_bar_commands.rs` 确认全部调用点已更新。

- [x] **Step 4: 编译验证**

```bash
cargo build --release -p octopus-desktop --features embedded 2>&1 | tail -20
```

Expected: 编译成功（Task 1 的 `open_temp_compact_editor` 签名变更全部消化）。如有报错，检查遗漏的调用点。

- [x] **Step 5: Commit**

```bash
git add crates/desktop/src/tray.rs crates/desktop/src/action_bar_commands.rs crates/desktop/src/compact_editor_commands.rs
git commit -m "refactor(compact-editor): open_temp_compact_editor 改收 TempTabPayload 支持对照模式"
```

---

### Task 3: action bar 翻译路径改 contrast + 新增 translate_text 命令

**Files:**
- Modify: `crates/desktop/src/action_bar_commands.rs:696-732`（`execute_action_bar_inner` 翻译分支）
- Modify: `crates/desktop/src/action_bar_commands.rs:330-361`（提取公共 `do_translate` 函数）
- Modify: `crates/desktop/src/main.rs:294`（注册 `translate_text` 命令）

**Interfaces:**
- Produces: `do_translate(text, &config) -> Result<String, String>`（公共翻译执行）、`#[tauri::command] translate_text(text, app) -> Result<String, String>`（前端工具栏翻译用）

- [x] **Step 1: 提取公共翻译执行函数 `do_translate`**

在 `action_bar_commands.rs` 的 `detect_translate_direction` 函数后新增（约 362 行）：

```rust
/// 执行翻译（公共逻辑）：解析引擎策略 + 执行翻译。
/// 供 execute_action_bar_inner（action bar 入口）和 translate_text（工具栏入口）复用。
fn do_translate(text: &str, config: &octopus_infra::config::AppConfig) -> Result<String, String> {
    let (source_lang, target_lang) = detect_translate_direction(text);
    match resolve_translate_strategy(config) {
        TranslateStrategy::Local(spec) => {
            let manager = octopus_translation::TranslationManager::new(&spec);
            let engine = manager.engine()
                .map_err(|e| e.to_string())?
                .ok_or_else(|| "翻译引擎加载失败".to_string())?;
            engine.translate(text, source_lang, target_lang)
                .map_err(|e| e.to_string())
        }
        TranslateStrategy::Llm => {
            let llm_config = crate::config::llm_config_ignore_mode(config)
                .ok_or_else(|| "翻译引擎未配置，请在设置中配置本地翻译模型或 LLM".to_string())?;
            let prompt = auto_translate_prompt(text);
            octopus_llm::chat_text_with_prompt(prompt, text, &llm_config)
                .map_err(|e| e.to_string())
        }
    }
}
```

- [x] **Step 2: 新增 `translate_text` Tauri 命令**

在 `do_translate` 后新增：

```rust
/// 前端工具栏翻译按钮调用。返回纯译文字符串，前端据此切 contrast 模式。
#[tauri::command]
pub fn translate_text(text: String) -> Result<String, String> {
    let config = octopus_infra::config::load_config().map_err(|e| e.to_string())?;
    do_translate(&text, &config)
}
```

- [x] **Step 3: 注册 `translate_text` 命令到 `main.rs`**

在 `main.rs` 的 `generate_handler!` 列表中，`action_bar_commands::action_bar_show_result,` 行附近（约 295 行）追加：

```rust
            action_bar_commands::translate_text,
```

- [x] **Step 4: 改 `execute_action_bar_inner` 翻译分支用 contrast**

将 local 翻译分支（约 699-721 行）的 `display` 构建改为 contrast payload。替换整个 `TranslateStrategy::Local(spec)` 分支：

```rust
                    TranslateStrategy::Local(spec) => {
                        if let Some(win) = app.get_webview_window(crate::action_bar_window::WINDOW_LABEL) {
                            let _ = win.hide();
                        }
                        #[cfg(target_os = "macos")]
                        { crate::activation::after_floating_window_hide_keep_active(&app); }
                        finalize_action_bar(&app);

                        let app_clone = app.clone();
                        let original = text.clone();
                        std::thread::spawn(move || {
                            let config = match octopus_infra::config::load_config() {
                                Ok(c) => c,
                                Err(e) => {
                                    let p = crate::compact_editor_commands::TempTabPayload {
                                        text: format!("【翻译】\n❌ 配置加载失败: {}", e),
                                        mode: Some("contrast".into()),
                                        original_text: Some(original),
                                        translated_text: Some(format!("❌ 配置加载失败: {}", e)),
                                        ..Default::default()
                                    };
                                    crate::compact_editor_commands::open_temp_compact_editor(&app_clone, &p);
                                    return;
                                }
                            };
                            let manager = octopus_translation::TranslationManager::new(&spec);
                            let translated = match manager.engine() {
                                Ok(Some(engine)) => match engine.translate(&text, source_lang, target_lang) {
                                    Ok(t) => t,
                                    Err(e) => format!("❌ 翻译失败: {}", e),
                                },
                                _ => "❌ 引擎加载失败".into(),
                            };
                            let p = crate::compact_editor_commands::TempTabPayload {
                                text: format!("【翻译】\n{}", translated),
                                mode: Some("contrast".into()),
                                original_text: Some(original),
                                translated_text: Some(translated),
                                ..Default::default()
                            };
                            crate::compact_editor_commands::open_temp_compact_editor(&app_clone, &p);
                        });
                        return Ok(true);
                    }
```

注意：`source_lang` / `target_lang` 在 match 前已由 `detect_translate_direction` 计算，线程闭包需 move 它们——改为在闭包前用 `let` 绑定（已在 697 行 `let (source_lang, target_lang) = ...`）。

- [x] **Step 5: 编译验证**

```bash
cargo build --release -p octopus-desktop --features embedded 2>&1 | tail -20
```

Expected: 编译成功。如有 move/borrow 报错，检查 `text` / `source_lang` / `target_lang` 的所有权转移。

- [x] **Step 6: Commit**

```bash
git add crates/desktop/src/action_bar_commands.rs crates/desktop/src/main.rs
git commit -m "feat(translation): action bar 翻译走 contrast 模式 + 新增 translate_text 命令"
```

---

### Task 4: i18n 文案新增

**Files:**
- Modify: `crates/desktop/frontend/src/locales/zh-CN.yaml:2-27`
- Modify: `crates/desktop/frontend/src/locales/en.yaml`（对应 editor/tab 段）

- [x] **Step 1: zh-CN.yaml 新增 contrast + translate keys**

在 `editor:` 段（约 2-20 行）的 `previewEmpty:` 行前插入：

```yaml
  translate: 翻译
  translateFail: 翻译失败
  translateConfirm: 重新翻译会覆盖右侧译文，再按确认
  translating: 翻译中...
  contrast:
    original: 原文
    translated: 译文
    layoutOriginal: 只原文
    layoutContrast: 对照
    layoutTranslated: 只译文
```

- [x] **Step 2: en.yaml 同步英文**

在对应 `editor:` 段插入：

```yaml
  translate: Translate
  translateFail: Translation failed
  translateConfirm: Re-translate will overwrite the right pane, press again to confirm
  translating: Translating...
  contrast:
    original: Source
    translated: Translation
    layoutOriginal: Source only
    layoutContrast: Side by side
    layoutTranslated: Translation only
```

- [x] **Step 3: i18n 单测验证**

```bash
cd crates/desktop/frontend && npx vitest run src/lib/i18n.test.ts 2>&1 | tail -15
```

Expected: PASS（确认新 key 在两个 locale 文件中都存在，无 missing key 报错）。

- [x] **Step 4: Commit**

```bash
git add crates/desktop/frontend/src/locales/zh-CN.yaml crates/desktop/frontend/src/locales/en.yaml
git commit -m "i18n: 新增翻译对照视图文案 (contrast.*)"
```

---

### Task 5: 前端 Tab 数据模型扩展 + promoteTempTab 更新

**Files:**
- Modify: `crates/desktop/frontend/src/pages/CompactEditor/index.tsx:12-45`（Tab + OpenTabPayload + PendingTabFull + pendingToTab）
- Modify: `crates/desktop/frontend/src/pages/CompactEditor/index.tsx:163-174`（listen temp 分支）
- Modify: `crates/desktop/frontend/src/pages/CompactEditor/promoteTempTab.ts`
- Modify: `crates/desktop/frontend/src/pages/CompactEditor/promoteTempTab.test.ts`

**Interfaces:**
- Produces: `Tab.mode/originalText/translatedText` 字段、`OpenTabPayload` 扩展字段、`PendingTabFull` 扩展字段。Task 6/7 的组件依赖这些字段。

- [x] **Step 1: Tab 接口加字段**

在 `index.tsx` 的 `interface Tab`（约 12-21 行）末尾 `isTemp?: boolean;` 后追加：

```ts
  mode?: 'single' | 'contrast';
  originalText?: string;
  translatedText?: string;
```

- [x] **Step 2: OpenTabPayload 加字段**

在 `interface OpenTabPayload`（约 22-27 行）追加：

```ts
  mode?: string;
  originalText?: string;
  translatedText?: string;
```

- [x] **Step 3: PendingTabFull 加字段**

在 `interface PendingTabFull`（约 29-37 行）追加：

```ts
  mode?: string;
  originalText?: string;
  translatedText?: string;
```

- [x] **Step 4: pendingToTab 映射新字段**

在 `pendingToTab` 函数（约 38-45 行）的两个 return 分支中都追加 mode/originalText/translatedText：

文本 return（约 44 行）：
```ts
  return { key, source, itemId: p.itemId, itemType: 'text', text: p.text, isTemp: p.isTemp, mode: p.mode as Tab['mode'], originalText: p.originalText, translatedText: p.translatedText };
```

- [x] **Step 5: listen temp 分支处理 contrast**

在 `useEffect` 的 listen temp 分支（约 165-170 行），构建 tab 时携带新字段：

```ts
        if (p.source === 'temp') {
          const tempKey = `temp:${Date.now()}_${Math.random().toString(36).slice(2, 6)}`;
          const next = [...tabsRef.current, {
            key: tempKey,
            source: 'temp' as const,
            itemId: 0,
            itemType: 'text' as const,
            text: p.text,
            isTemp: true,
            mode: (p.mode === 'contrast' ? 'contrast' : 'single') as Tab['mode'],
            originalText: p.originalText,
            translatedText: p.translatedText,
          }];
          tabsRef.current = next;
          setTabs(next);
          setActiveIdx(next.length - 1);
        }
```

- [x] **Step 6: promoteTempTab 保留 contrast 字段语义**

contrast temp tab 升级为 clipboard 条目时，mode 应回退为 single（DB 只存译文），但保留 translatedText 到 text。修改 `promoteTempTab.ts`：

```ts
export function promoteTempTab(tabs: Tab[], idx: number, newId: number): Tab[] {
  return tabs.map((t, i) => {
    if (i !== idx) return t;
    // contrast temp 升级：text 已是译文（doSave 时设 tab.text = translatedText），
    // 升级为 single clipboard 条目，丢弃原文。
    const isContrast = t.mode === 'contrast';
    return {
      ...t,
      key: `clipboard:${newId}`,
      source: "clipboard",
      itemId: newId,
      itemType: "text",
      isTemp: false,
      mode: isContrast ? 'single' : t.mode,
      originalText: undefined,
      translatedText: undefined,
    };
  });
}
```

- [x] **Step 7: 补 promoteTempTab 单测**

在 `promoteTempTab.test.ts` 追加 contrast 升级用例：

```ts
import { promoteTempTab } from "./promoteTempTab";
import { describe, it, expect } from "vitest";

describe("promoteTempTab", () => {
  it("contrast temp 升级为 single clipboard，丢弃原文", () => {
    const tabs = [{
      key: "temp:1",
      source: "temp" as const,
      itemId: 0,
      itemType: "text" as const,
      text: "译文内容",
      isTemp: true,
      mode: "contrast" as const,
      originalText: "原文",
      translatedText: "译文内容",
    }];
    const result = promoteTempTab(tabs, 0, 42);
    expect(result[0].key).toBe("clipboard:42");
    expect(result[0].source).toBe("clipboard");
    expect(result[0].itemId).toBe(42);
    expect(result[0].isTemp).toBe(false);
    expect(result[0].mode).toBe("single");
    expect(result[0].originalText).toBeUndefined();
    expect(result[0].translatedText).toBeUndefined();
  });

  it("single temp 升级保持 mode=single", () => {
    const tabs = [{
      key: "temp:1",
      source: "temp" as const,
      itemId: 0,
      itemType: "text" as const,
      text: "内容",
      isTemp: true,
    }];
    const result = promoteTempTab(tabs, 0, 42);
    expect(result[0].mode).toBeUndefined();
    expect(result[0].isTemp).toBe(false);
  });
});
```

- [x] **Step 8: 运行单测**

```bash
cd crates/desktop/frontend && npx vitest run src/pages/CompactEditor/promoteTempTab.test.ts 2>&1 | tail -15
```

Expected: PASS。

- [x] **Step 9: Commit**

```bash
git add crates/desktop/frontend/src/pages/CompactEditor/index.tsx crates/desktop/frontend/src/pages/CompactEditor/promoteTempTab.ts crates/desktop/frontend/src/pages/CompactEditor/promoteTempTab.test.ts
git commit -m "feat(compact-editor): Tab 数据模型扩展 contrast 字段 + promoteTempTab 升级语义"
```

---

### Task 6: 新增 TranslationContrastPane 组件

**Files:**
- Create: `crates/desktop/frontend/src/pages/CompactEditor/TranslationContrastPane.tsx`

**Interfaces:**
- Consumes: `CodeMirrorEditor`（现有）、`MarkdownPreview`（现有）、`useT` / `t`（i18n）、Task 4 的 contrast.* / editor.translate keys
- Produces: `TranslationContrastPane` 组件，Task 7 的 index.tsx 渲染区分流依赖此组件 Props

- [x] **Step 1: 创建组件文件**

创建 `crates/desktop/frontend/src/pages/CompactEditor/TranslationContrastPane.tsx`：

```tsx
import { useState, useRef, useCallback, useEffect, useMemo } from "react";
import type { EditorView } from "@codemirror/view";
import { undo, redo } from "@codemirror/commands";
import {
  Undo2, Redo2, ZoomIn, ZoomOut, Check, Save, FileText, Eye,
  PanelLeft, Columns2, PanelRight, Languages, Loader2,
} from "lucide-react";
import { CodeMirrorEditor } from "./CodeMirrorEditor";
import { MarkdownPreview } from "./MarkdownPreview";
import { useT } from "@/lib/i18n";

type PaneMode = "editor" | "preview";
type ViewLayout = "left" | "contrast" | "right";

const FONT_MIN = 12;
const FONT_MAX = 24;

interface TranslationContrastPaneProps {
  originalText: string;
  translatedText: string;
  readOnly: boolean;
  fontSize: number;
  onFontSizeChange: (n: number) => void;
  onOriginalChange: (s: string) => void;
  onTranslatedChange: (s: string) => void;
  onTranslate: () => void;
  onSave: () => void;
  disableSave?: boolean;
  savedFlash: boolean;
  translating: boolean;
}

const ToolBtn = ({ onClick, title, disabled, active, children }: {
  onClick: () => void; title: string; disabled?: boolean; active?: boolean; children: React.ReactNode;
}) => (
  <button
    type="button"
    disabled={disabled}
    title={title}
    onClick={onClick}
    className={`p-1.5 rounded-md transition-colors disabled:opacity-30 disabled:hover:bg-transparent ${
      active ? "bg-accent text-foreground" : "text-muted-foreground hover:bg-accent hover:text-foreground"
    }`}
  >{children}</button>
);

export function TranslationContrastPane({
  originalText, translatedText, readOnly, fontSize, onFontSizeChange,
  onOriginalChange, onTranslatedChange, onTranslate, onSave, disableSave, savedFlash, translating,
}: TranslationContrastPaneProps) {
  const t = useT();
  const [leftMode, setLeftMode] = useState<PaneMode>("editor");
  const [rightMode, setRightMode] = useState<PaneMode>("editor");
  const [viewLayout, setViewLayout] = useState<ViewLayout>("contrast");
  const [translateConfirm, setTranslateConfirm] = useState(false);
  const dirtyTranslatedRef = useRef(false);
  const leftViewRef = useRef<EditorView | null>(null);
  const rightViewRef = useRef<EditorView | null>(null);

  // 跟踪译文是否被用户手动编辑（用于翻译覆盖确认）
  useEffect(() => { dirtyTranslatedRef.current = false; }, [translatedText]);
  const handleTranslatedChange = useCallback((next: string) => {
    dirtyTranslatedRef.current = true;
    onTranslatedChange(next);
  }, [onTranslatedChange]);

  const confirmTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  useEffect(() => () => { if (confirmTimerRef.current) clearTimeout(confirmTimerRef.current); }, []);

  const handleTranslateClick = useCallback(() => {
    // 译文已被手动编辑 → 双击确认（复用 clearPending 模式）
    if (dirtyTranslatedRef.current && !translateConfirm) {
      setTranslateConfirm(true);
      confirmTimerRef.current = setTimeout(() => { setTranslateConfirm(false); confirmTimerRef.current = null; }, 2000);
      return;
    }
    if (confirmTimerRef.current) { clearTimeout(confirmTimerRef.current); confirmTimerRef.current = null; }
    setTranslateConfirm(false);
    dirtyTranslatedRef.current = false;
    onTranslate();
  }, [dirtyTranslatedRef, translateConfirm, onTranslate]);

  const handleUndoLeft = useCallback(() => { if (leftViewRef.current) undo(leftViewRef.current); }, []);
  const handleRedoLeft = useCallback(() => { if (leftViewRef.current) redo(leftViewRef.current); }, []);
  const handleUndoRight = useCallback(() => { if (rightViewRef.current) undo(rightViewRef.current); }, []);
  const handleRedoRight = useCallback(() => { if (rightViewRef.current) redo(rightViewRef.current); }, []);

  // 双列布局的 flex 基础
  const leftFlex = viewLayout === "left" ? "1" : viewLayout === "right" ? "0" : "1";
  const rightFlex = viewLayout === "right" ? "1" : viewLayout === "left" ? "0" : "1";

  const origCharCount = useMemo(() => [...originalText].length, [originalText]);
  const transCharCount = useMemo(() => [...translatedText].length, [translatedText]);

  const renderPane = (
    label: string,
    charCount: number,
    paneMode: PaneMode,
    setPaneMode: (m: PaneMode) => void,
    text: string,
    onChange: (s: string) => void,
    viewRef: React.RefObject<EditorView | null>,
    onUndo: () => void,
    onRedo: () => void,
    visible: boolean,
    flexBasis: string,
  ) => (
    <div
      className={`flex flex-col min-h-0 min-w-0 ${viewLayout === "contrast" ? "border-r last:border-r-0 border-border" : ""}`}
      style={{ display: visible ? "flex" : "none", flex: flexBasis }}
    >
      {/* 列内小标题栏 */}
      <div className="flex-shrink-0 flex items-center gap-1 px-2 py-1 border-b border-border bg-muted/50">
        <Undo2 className="w-3.5 h-3.5 text-muted-foreground" />
        <button type="button" onClick={onUndo} title={t("editor.undo")} className="p-1 rounded hover:bg-accent text-muted-foreground hover:text-foreground">
          <Undo2 className="w-3.5 h-3.5" />
        </button>
        <button type="button" onClick={onRedo} title={t("editor.redo")} className="p-1 rounded hover:bg-accent text-muted-foreground hover:text-foreground">
          <Redo2 className="w-3.5 h-3.5" />
        </button>
        <span className="text-[11px] text-muted-foreground font-medium ml-1">{label}</span>
        <span className="text-[10px] text-muted-foreground tabular-nums">{t("editor.charCount", { n: charCount })}</span>
        <div className="flex-1" />
        <ToolBtn onClick={() => setPaneMode("editor")} title={t("editor.view.editor")} disabled={readOnly || paneMode === "editor"} active={paneMode === "editor"}>
          <FileText className="w-3.5 h-3.5" />
        </ToolBtn>
        <ToolBtn onClick={() => setPaneMode("preview")} title={t("editor.view.preview")} disabled={paneMode === "preview"} active={paneMode === "preview"}>
          <Eye className="w-3.5 h-3.5" />
        </ToolBtn>
      </div>
      {/* 内容区：CM6 + Preview 始终挂载，display 切换 */}
      <div className="flex-1 flex min-h-0">
        <div className="flex-1 min-h-0 min-w-0 flex flex-col overflow-hidden" style={{ display: paneMode === "preview" ? "none" : "flex" }}>
          <CodeMirrorEditor value={text} readOnly={readOnly} fontSize={fontSize} onChange={onChange} viewRef={viewRef} />
        </div>
        <div className="flex-1 min-h-0 min-w-0 flex flex-col overflow-hidden" style={{ display: paneMode === "editor" ? "none" : "flex" }}>
          <MarkdownPreview source={text} fontSize={fontSize} />
        </div>
      </div>
    </div>
  );

  return (
    <div className="flex flex-col flex-1 min-h-0">
      {/* 顶部主工具栏 */}
      <div className="flex-shrink-0 flex items-center gap-0.5 px-2 py-1.5 border-b border-border bg-muted">
        <ToolBtn onClick={() => onFontSizeChange(Math.max(FONT_MIN, fontSize - 1))} title={t("editor.fontSize")} disabled={fontSize <= FONT_MIN}>
          <ZoomOut className="w-4 h-4" />
        </ToolBtn>
        <span className="text-[11px] text-muted-foreground w-7 text-center tabular-nums">{fontSize}</span>
        <ToolBtn onClick={() => onFontSizeChange(Math.min(FONT_MAX, fontSize + 1))} title={t("editor.fontSize")} disabled={fontSize >= FONT_MAX}>
          <ZoomIn className="w-4 h-4" />
        </ToolBtn>
        <span className="w-px h-4 bg-border mx-1" />
        {/* 视图布局切换组 */}
        <ToolBtn onClick={() => setViewLayout("left")} title={t("editor.contrast.layoutOriginal")} active={viewLayout === "left"}>
          <PanelLeft className="w-4 h-4" />
        </ToolBtn>
        <ToolBtn onClick={() => setViewLayout("contrast")} title={t("editor.contrast.layoutContrast")} active={viewLayout === "contrast"}>
          <Columns2 className="w-4 h-4" />
        </ToolBtn>
        <ToolBtn onClick={() => setViewLayout("right")} title={t("editor.contrast.layoutTranslated")} active={viewLayout === "right"}>
          <PanelRight className="w-4 h-4" />
        </ToolBtn>
        <div className="flex-1" />
        {/* 翻译按钮 */}
        <button
          type="button"
          onClick={handleTranslateClick}
          disabled={translating}
          title={translateConfirm ? t("editor.translateConfirm") : t("editor.translate")}
          className={`flex items-center gap-1 px-2.5 py-1 rounded-md text-xs transition-colors ${
            translateConfirm
              ? "bg-red-500 text-white"
              : "bg-[#007aff] hover:bg-[#0066d6] text-white"
          } disabled:opacity-50 disabled:cursor-not-allowed`}
        >
          {translating ? <Loader2 className="w-3.5 h-3.5 animate-spin" /> : <Languages className="w-3.5 h-3.5" />}
          {translateConfirm ? t("editor.translateConfirm") : t("editor.translate")}
        </button>
        <span className="w-px h-4 bg-border mx-1" />
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

      {/* 双列内容区 */}
      <div className="flex-1 flex min-h-0">
        {renderPane(
          t("editor.contrast.original"), origCharCount, leftMode, setLeftMode,
          originalText, onOriginalChange, leftViewRef, handleUndoLeft, handleRedoLeft,
          viewLayout !== "right", leftFlex,
        )}
        {renderPane(
          t("editor.contrast.translated"), transCharCount, rightMode, setRightMode,
          translatedText, handleTranslatedChange, rightViewRef, handleUndoRight, handleRedoRight,
          viewLayout !== "left", rightFlex,
        )}
      </div>
    </div>
  );
}
```

- [x] **Step 2: TypeScript 编译验证**

```bash
cd crates/desktop/frontend && npx tsc --noEmit 2>&1 | tail -20
```

Expected: PASS（无类型错误）。如有 `lucide-react` 图标名错误，确认图标存在（`PanelLeft` / `PanelRight` / `Columns2` / `Languages` / `Loader2` 均为 lucide-react 已有图标）。

- [x] **Step 3: Commit**

```bash
git add crates/desktop/frontend/src/pages/CompactEditor/TranslationContrastPane.tsx
git commit -m "feat(compact-editor): 新增 TranslationContrastPane 对照视图组件"
```

---

### Task 7: index.tsx 渲染分流 + doSave contrast 语义 + 工具栏翻译入口

**Files:**
- Modify: `crates/desktop/frontend/src/pages/CompactEditor/index.tsx:199-246`（doSave）
- Modify: `crates/desktop/frontend/src/pages/CompactEditor/index.tsx:312-352`（渲染区）

**Interfaces:**
- Consumes: Task 6 的 `TranslationContrastPane`、Task 3 的 `translate_text` 命令、Task 5 的 Tab 字段

- [x] **Step 1: doSave 处理 contrast temp tab**

在 `doSave`（约 199 行）的 `if (active.isTemp)` 分支内，contrast tab 升级时须把 `translatedText` 作为入库文本。在 `const newId = await invoke...` 前加判断：

```tsx
      if (active.isTemp) {
        // contrast temp：入库的是译文（右半），非 text 字段
        const saveText = active.mode === 'contrast' ? (active.translatedText || "") : (active.text || "");
        if (saveText.trim() === "") {
          if (tabs.length <= 1) { invoke("close_compact_editor"); return; }
          const idx = activeIdx;
          const next = tabsRef.current.filter((_, i) => i !== idx);
          tabsRef.current = next;
          setTabs(next);
          setActiveIdx(Math.min(activeIdx, next.length - 1));
          return;
        }
        const newId = await invoke<number>("insert_clipboard_text_item", { text: saveText });
        // contrast 升级前把 text 设为译文（promoteTempTab 依赖 text 作为条目内容）
        const tabsWithText = tabsRef.current.map((t, i) =>
          i === activeIdx ? { ...t, text: saveText } : t
        );
        const next = promoteTempTab(tabsWithText, activeIdx, newId);
        tabsRef.current = next;
        setTabs(next);
        setSavedFlash(true);
        if (savedFlashTimer.current) clearTimeout(savedFlashTimer.current);
        savedFlashTimer.current = setTimeout(() => setSavedFlash(false), 1200);
        return;
      }
```

注意：这段替换原 `if (active.isTemp)` 的整个 if 体（约 206-224 行）。

- [x] **Step 2: 新增 contrast tab 保存（非 temp）路径**

在 `doSave` 的既有条目分支（约 225-238 行），contrast 已 promote 为 clipboard 后保存：

```tsx
      // contrast 已 promote 为 clipboard：保存译文
      if (active.mode === 'contrast') {
        const saveText = active.translatedText || "";
        if (saveText.trim() === "") {
          await invoke("delete_clipboard_item", { id: active.itemId });
          if (tabs.length <= 1) { invoke("close_compact_editor"); return; }
          const idx = activeIdx;
          const next = tabsRef.current.filter((_, i) => i !== idx);
          tabsRef.current = next;
          setTabs(next);
          setActiveIdx(idx === activeIdx ? Math.min(activeIdx, next.length - 1) : activeIdx > idx ? activeIdx - 1 : activeIdx);
          return;
        }
        await invoke("set_clipboard_item_text", { itemId: active.itemId, text: saveText });
        setSavedFlash(true);
        if (savedFlashTimer.current) clearTimeout(savedFlashTimer.current);
        savedFlashTimer.current = setTimeout(() => setSavedFlash(false), 1200);
        return;
      }
```

- [x] **Step 3: 新增翻译状态 + handleTranslate**

在 `CompactEditor` 组件内（约 93 行 `savedFlash` state 附近）新增：

```tsx
  const [translating, setTranslating] = useState(false);
```

在 `doSave` 定义后新增 `handleTranslateForTab`：

```tsx
  // 工具栏翻译按钮：有选区翻选区，无选区翻全文，成功后切 contrast
  const handleTranslateForTab = useCallback(async (idx: number) => {
    const tab = tabsRef.current[idx];
    if (!tab || tab.source === 'transcription') return;
    setTranslating(true);
    try {
      const sourceText = tab.text || "";
      const translated = await invoke<string>("translate_text", { text: sourceText });
      const next = tabsRef.current.map((t, i) =>
        i === idx
          ? { ...t, mode: 'contrast' as const, originalText: sourceText, translatedText: translated }
          : t
      );
      tabsRef.current = next;
      setTabs(next);
    } catch (e) {
      console.error("翻译失败:", e);
      // toast 简化：alert 替代（前端无统一 toast 系统）
      alert(t("editor.translateFail") + ": " + String(e));
    } finally {
      setTranslating(false);
    }
  }, [t]);
```

- [x] **Step 4: 渲染区分流 contrast vs single**

在内容区渲染（约 325-344 行），文本 tab 分支前加 contrast 判断：

```tsx
              // contrast tab：渲染 TranslationContrastPane
              i === activeIdx ? (
                tab.mode === 'contrast' ? (
                  <TranslationContrastPane
                    originalText={tab.originalText || ''}
                    translatedText={tab.translatedText || ''}
                    readOnly={tab.source === 'transcription'}
                    fontSize={fontSize}
                    onFontSizeChange={setFontSize}
                    onOriginalChange={(next) => updateActiveTextAt(next, i)}
                    onTranslatedChange={(next) => setTabs(prev => prev.map((t, j) => j === i ? { ...t, translatedText: next } : t))}
                    onTranslate={() => handleTranslateForTab(i)}
                    onSave={doSave}
                    disableSave={tab.source === 'transcription'}
                    savedFlash={savedFlash}
                    translating={translating}
                  />
                ) : (
                  <MarkdownPane
                    text={tab.text || ''}
                    readOnly={tab.source === 'transcription'}
                    fontSize={fontSize}
                    onFontSizeChange={setFontSize}
                    onChange={(next) => updateActiveTextAt(next, i)}
                    onClear={() => updateActiveTextAt('', i)}
                    onSave={doSave}
                    disableSave={tab.source === 'transcription'}
                    savedFlash={savedFlash}
                  />
                )
              ) : (
```

注意：上面 `i === activeIdx ? (` 替换原来的 `i === activeIdx ? (` 行（约 327 行），后续 `: (` 非活跃占位不变。

- [x] **Step 5: 引入 TranslationContrastPane**

在文件顶部 import 区（约 7 行）追加：

```tsx
import { TranslationContrastPane } from "./TranslationContrastPane";
```

- [x] **Step 6: TypeScript 编译验证**

```bash
cd crates/desktop/frontend && npx tsc --noEmit 2>&1 | tail -20
```

Expected: PASS。

- [x] **Step 7: 全量单测**

```bash
cd crates/desktop/frontend && npx vitest run 2>&1 | tail -20
```

Expected: 全部 PASS。

- [x] **Step 8: Commit**

```bash
git add crates/desktop/frontend/src/pages/CompactEditor/index.tsx
git commit -m "feat(compact-editor): 渲染分流 contrast + doSave 保存译文 + 工具栏翻译入口"
```

---

### Task 8: MarkdownPane 工具栏加翻译按钮（single 模式入口）

**Files:**
- Modify: `crates/desktop/frontend/src/pages/CompactEditor/MarkdownPane.tsx:18-28`（Props 加 onTranslate）
- Modify: `crates/desktop/frontend/src/pages/CompactEditor/MarkdownPane.tsx:124-176`（工具栏渲染）

**背景：** 普通文本 tab（single 模式）工具栏需要「翻译」按钮，点击后切 contrast。按钮由 index.tsx 传 `onTranslate` 回调。readOnly tab（语音记录）不显示。

- [x] **Step 1: MarkdownPane Props 加 onTranslate + translating**

在 `interface MarkdownPaneProps`（约 18-28 行）追加：

```tsx
  onTranslate?: () => void;
  translating?: boolean;
```

- [x] **Step 2: 函数签名解构新 props**

在 `export function MarkdownPane({...})` 参数列表（约 42-44 行）追加 `onTranslate, translating`：

```tsx
export function MarkdownPane({
  text, readOnly, fontSize, onFontSizeChange, onChange, onClear, onSave, disableSave, savedFlash, onTranslate, translating,
}: MarkdownPaneProps) {
```

- [x] **Step 3: 工具栏加翻译按钮**

在工具栏的视图模式组前（约 151 行 `<span className="w-px h-4 bg-border mx-1" />` 视图组前）插入翻译按钮。仅 `!readOnly && onTranslate` 时显示：

```tsx
        {!readOnly && onTranslate && (
          <>
            <button
              type="button"
              onClick={onTranslate}
              disabled={translating}
              title={t("editor.translate")}
              className="flex items-center gap-1 px-2 py-1 rounded-md text-xs bg-[#007aff] hover:bg-[#0066d6] text-white transition-colors disabled:opacity-50"
            >
              <Languages className="w-3.5 h-3.5" />
              {translating ? t("editor.translating") : t("editor.translate")}
            </button>
            <span className="w-px h-4 bg-border mx-1" />
          </>
        )}
```

注意：在 import 区加 `Languages` 到 lucide-react 导入（约第 4 行）。如果 `Loader2` 需要用于 translating spinner，一并加。

- [x] **Step 4: index.tsx 传 onTranslate 给 MarkdownPane**

在 Task 7 Step 4 的渲染区，MarkdownPane 调用处加 props：

```tsx
                  <MarkdownPane
                    text={tab.text || ''}
                    readOnly={tab.source === 'transcription'}
                    fontSize={fontSize}
                    onFontSizeChange={setFontSize}
                    onChange={(next) => updateActiveTextAt(next, i)}
                    onClear={() => updateActiveTextAt('', i)}
                    onSave={doSave}
                    disableSave={tab.source === 'transcription'}
                    savedFlash={savedFlash}
                    onTranslate={tab.source === 'transcription' ? undefined : () => handleTranslateForTab(i)}
                    translating={translating}
                  />
```

- [x] **Step 5: TypeScript 编译 + 单测**

```bash
cd crates/desktop/frontend && npx tsc --noEmit 2>&1 | tail -10 && npx vitest run 2>&1 | tail -10
```

Expected: PASS。

- [x] **Step 6: Commit**

```bash
git add crates/desktop/frontend/src/pages/CompactEditor/MarkdownPane.tsx crates/desktop/frontend/src/pages/CompactEditor/index.tsx
git commit -m "feat(compact-editor): MarkdownPane 工具栏加翻译按钮（single→contrast 入口）"
```

---

### Task 9: 端到端编译验证 + 文档同步

**Files:**
- Verify: 全项目编译
- Modify: `docs/architecture.md`（CompactEditor 段补 contrast 模式说明）

- [x] **Step 1: 后端全量编译**

```bash
cargo build --release -p octopus-desktop --features embedded 2>&1 | tail -10
```

Expected: 编译成功。

- [x] **Step 2: 前端全量编译 + 单测**

```bash
cd crates/desktop/frontend && npx tsc --noEmit 2>&1 | tail -5 && npx vitest run 2>&1 | tail -10
```

Expected: 全部 PASS。

- [x] **Step 3: architecture.md 文档同步**

在 `docs/architecture.md` 的 CompactEditor 段（约 181 行）末尾补一句 contrast 模式说明：

```
**翻译对照模式（2026-07-12）**：`Tab.mode='contrast'` 时渲染 `TranslationContrastPane`（替代 `MarkdownPane`），左原文/右译文双栏，各列独立 CM6 编辑器 + Markdown 预览（无 split，已外层分栏），新增视图布局切换（只原文/对照/只译文）。入口三条：(1) action bar 翻译（local/LLM 均走 contrast temp tab，携带 originalText+translatedText）；(2) 普通文本 tab 工具栏「翻译」按钮（invoke `translate_text`，成功后切 mode='contrast'）；(3) 截图翻译（数据通路已支持，UI 后续）。保存只写译文（`translatedText`），原文是脚手架不持久化；temp→clipboard 升级时 mode 回退 single。后端 `open_temp_compact_editor` 改收 `TempTabPayload { text, mode?, original_text?, translated_text? }`。详见 [spec](superpowers/specs/2026-07-12-translation-bilingual-view-design.md)。
```

- [x] **Step 4: Commit**

```bash
git add docs/architecture.md
git commit -m "docs(sync): architecture 补 translation-bilingual-view contrast 模式说明"
```

- [x] **Step 5: 最终 git log 确认**

```bash
git log --oneline -10
```

Expected: 看到 7-8 个 feat/refactor/docs 提交，对应 Task 1-9。

---

### Task 10: 流式翻译（实现偏差补录）

**背景：** 原 Task 3 设计为同步翻译后返回结果。用户反馈翻译太慢。改为流式：立即打开编辑器（译文 loading），后台逐段翻译 emit。

- [x] **Step 1:** 新增 `do_translate_streaming` 按换行切段逐段翻译 emit；`translate_text` 改 fire-and-forget（`Result<(), String>` + spawn 线程）
- [x] **Step 2:** action bar local 翻译改流式（立即打开 contrast tab + spawn 线程 emit）
- [x] **Step 3:** 前端 listen `translate-progress`/`translate-done` + `translatingTabKeyRef` 跟踪
- [x] **Step 4:** `handleTranslateForTab` 改 fire-and-forget（立即切 contrast + invoke 不等返回）

---

### Task 11: TranslationContrastPane splitter + toggle 按钮（实现偏差补录）

**背景：** 原 Task 6 用 flex 布局 + 两个独立编辑/预览按钮。用户反馈缺 splitter、两按钮不如一个 toggle。

- [x] **Step 1:** 加可拖拽 splitter（grid 布局 + PointerEvent + localStorage 持久化，复用 MarkdownPane 模式）
- [x] **Step 2:** 编辑/预览改单 toggle 按钮（点一下切换，图标显示目标模式）
- [x] **Step 3:** splitter 颜色调深（`bg-muted-foreground/30`，与行号线 `bg-border` 区分）

---

### Task 12: Opus-MT 轻量翻译引擎接入

**背景：** m2m100（418M）太慢。接入 Opus-MT（MarianMT 30M/方向），推理速度快数倍。

- [x] **Step 1:** `opus_mt.rs` MarianMT encoder-decoder greedy，按方向加载 zh-en/en-zh 子目录
- [x] **Step 2:** `engine.rs` 缓存改 HashMap + `load_opus_mt(source, target)`
- [x] **Step 3:** `discovery.rs` 加 opus-mt（一组模型，两个方向子目录）
- [x] **Step 4:** `resolve_translate_strategy` 自动模式优先 opus-mt
- [x] **Step 5:** `do_translate` 对 opus-mt 走 `load_opus_mt`

---

### Task 13: Opus-MT tokenizer precompiled_charsmap=null 修复

**背景：** Xenova 导出的 tokenizer.json 中 `precompiled_charsmap` 为 null，tokenizers 0.21.4 直接 panic。最终方案：解析 JSON 删除整个 `normalizer` 字段（MarianMT 不需要 normalization）。

- [x] **Step 1:** `load_opus_tokenizer` 函数：含 `precompiled_charsmap` 则删除 `normalizer` 字段后 from_bytes 加载

---

### Task 14: Opus-MT greedy 解码重复修复

**背景：** MarianMT 训练用 beam search（num_beams=6），greedy 解码陷入重复循环（preview preview list rows list rows）。旧重复检测只拦连续相同 token，拦不住模式重复。

- [x] **Step 1:** repetition_penalty=1.3（已出现 token logit / 1.3）
- [x] **Step 2:** no_repeat_ngram_size=3（禁止已出现 3-gram 后继 token）

---

### Task 15: 代码审查两轮修复（6+6 项）

**背景：** 两轮代码审查发现 P0 panic、P1 路径不一致、P2 前端状态不对称等问题。

**一轮（6b2cc53d）：**
- [x] **P0:** no-repeat-ngram 切片越界 panic（`>= n-1` → `>= n`，循环上界修正）
- [x] **P1:** discovery 路径统一为 `resolve_model_dir`（HF cache + `~/.octopus/models/<repo>`）
- [x] **P1:** downloadable 列表补 `Xenova/opus-mt-en-zh`，discovery dedup 为一行
- [x] **P1:** tokenizer encode 加 `truncate` 兜底，分段判断改用实测 token 数
- [x] **P1:** 段内拼接从 `\n` 改为 `""`（与 m2m100 一致，保持段落对齐）
- [x] **P3:** repetition penalty 排除 decoder_start_id

**二轮（fba52820）：**
- [x] **P0 守护:** penalty 逻辑抽为纯函数 `apply_penalties` + 6 单测（len=1/2/3/重复 n-gram/正负 logit）
- [x] **P2 前端:** TranslateTab 匹配从 `m.source===repo` → `m.name===name`
- [x] **P2 encoder eos:** `encode(text, false)` → `encode(text, true)` 补 `</s>`
- [x] **P3 spec 大小写:** 前端去掉 `.toLowerCase()`
- [x] **清理:** discovery dead code + 错别字 + unwrap_or 默认值

**e2e 测试（015ba9c7）：**
- [x] 补 `tests/opus_mt_test.rs`（中→英 + 英→中，`#[ignore]`，断言无 4+ 连续重复词）

**三轮清理（9ac1aa22 + 62c10f4c）：**
- [x] splitter 颜色 `bg-border` → `bg-muted-foreground/30`（与行号线区分）
- [x] opus_mt_test unused import 删除 + ignore message 改为引导 GUI 下载路径
