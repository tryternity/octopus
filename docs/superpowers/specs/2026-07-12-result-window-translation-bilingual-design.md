# Result 浮窗翻译双语视图设计

> **状态**：设计确认，待实现
> **日期**：2026-07-12
> **分支**：`worktree-translation-pane`
> **scope**：在语音识别 **Result 浮窗**（`pages/Result/`）内新增「翻译」入口——点击后自动进大窗，文本区从单语变**上下**两栏（上原文、下译文），最终提交产物为**译文**。
> **前置文档**：
> - [`2026-07-12-translation-bilingual-view-design.md`](./2026-07-12-translation-bilingual-view-design.md)（CompactEditor **左右**对照视图，独立模块，本设计不复用其组件但复用其后端翻译命令）
> - [`2026-07-12-local-translation-engine-design.md`](./2026-07-12-local-translation-engine-design.md)（`detect_translate_direction` + `resolve_translate_strategy` + `translate_text` 流式命令）

---

## 1. 背景与动机

### 1.1 现状

| 能力 | 落点 | 布局 | 触发 |
|------|------|------|------|
| 翻译对照（已实现） | CompactEditor（独立富文本编辑器） | **左右**对照 | action bar 选中已有文本翻译 / 普通文本 tab 工具栏翻译 |
| Result 浮窗识别 | `pages/Result/`（识别结果小窗） | 单语 | 全局热键触发实时 ASR |

**Result 浮窗**是实时语音识别的主界面：全局热键唤起 → 流式识别 → 工具栏（关闭/降噪/润色/润色此刻/展开/保存）→ 「保存」=`commit_edit` 把文本打到光标处（粘贴）。浮窗有迷你态（720×116）和大窗态（720×480，点展开）。

**Result 浮窗目前没有翻译入口。**

### 1.2 需求

用户希望在 Result 浮窗识别时，加一个「翻译」按钮：

1. **点击** → 自动进大窗，单语文本区变**上下**两栏（上原文、下译文），立即首翻一次
2. 翻译按钮变**下拉**，四选一：手动 / 自动 8s / 自动 12s / 自动 15s（自动档 = 节流定时器，原文有变化才重翻）
3. 「立即翻译」按钮 + `Cmd+T` 快捷键：无视档位立即加翻
4. **保存/提交** → 终翻一次最新全文 → **译文**提交到光标处 → 自动退出翻译模式回单语

### 1.3 设计约束（已确认）

| 决策点 | 选择 |
|--------|------|
| 翻译触发时机 | **手工触发**进入双语模式；进入即首翻一次；可重复重翻；保存时终翻一次 |
| ASR 仍在识别时译文行为 | **节流（throttle）**：每 N 秒检查原文是否变化，变化才重翻；翻译中跳过本次（不排队） |
| 自动档间隔 | **8 / 12 / 15 秒**三档；默认选中**手动**（首次），用户改选后**记忆到 DB** |
| 命名 | "手动"（vs 自动 8/12/15s） |
| 布局 | **上下**分栏（上原文、下译文），仅大窗态（480px 高） |
| 迷你窗 | **不支持**翻译模式（空间不足）；进翻译模式自动转大窗 |
| 手动翻译入口 | **独立按钮 + 快捷键**双通道（下拉纯选择器不触发动作） |
| 产物 | **译文**（保存提交译文到光标） |
| 档位记忆 | 写 **DB app_config 表**（`translate_mode` 键），与 `denoise_mode` 一致——**非** config.yaml |
| 保存后 | **自动退出**翻译模式，回单语大窗 |
| 工具栏 | **移除** settings 按钮（改经托盘菜单进入设置） |
| 实现路径 | **Approach 1**：Result 浮窗内新建轻量双语视图，改动封闭在 Result 模块 |

### 1.4 不做什么

- **不复用** `TranslationContrastPane`（CompactEditor 左右对照组件）——强耦合 CompactEditor 上下文（fontSize/onSave/savedFlash/promoteTempTab…），引入大量不需要的状态
- **不做左右布局**——Result 浮窗是窄长形（720px 宽），上下分栏更自然
- **不做实时字幕式同传**——流式 ASR 文本高频抖动，增量翻译 + 双栏同步滚动复杂度高、译文抖动严重，实用性存疑
- **不持久化原文**——与 CompactEditor contrast 一致，原文是脚手架，DB 只存译文

---

## 2. 数据模型

### 2.1 前端 Result 状态扩展

`Result/index.tsx` 新增：

```ts
type TranslateMode = 'off' | 'manual' | '8s' | '12s' | '15s';

// 新增 state
const [translateMode, setTranslateMode] = useState<TranslateMode>('off');
const [translatedText, setTranslatedText] = useState("");
const [translating, setTranslating] = useState(false);
const lastTranslatedRef = useRef<string>("");   // 上次翻译的原文快照（节流用）
const translatingRef = useRef(false);            // 防"翻译中"重复触发
const throttleTimerRef = useRef<ReturnType<typeof setInterval> | null>(null);
```

### 2.2 DB app_config 键

新增键 `translate_mode`（string），取值：`"manual"` / `"8s"` / `"12s"` / `"15s"`。

- 首次进入翻译模式：DB 无此键 → 默认 `"manual"`
- 用户切换档位 → `save_config_key("translate_mode", ...)` 写 DB
- 下次进入：读 DB，用记忆的档位
- 值 `"off"` **不入库**（退出翻译模式不覆盖记忆档）——`translateMode` 的 `'off'` 仅是前端态

### 2.3 ToolbarState 扩展

`runtime_config.rs::ToolbarState` 新增字段：

```rust
pub struct ToolbarState {
    // ... 现有字段 ...
    /// 翻译自动档（记忆档位）："manual" / "8s" / "12s" / "15s"
    pub translate_mode: String,
}
```

`toolbar_state` 命令读 DB `translate_mode` 填充（DB 无值 → 默认 `"manual"`）。

### 2.4 规则

- `translateMode === 'off'`：渲染单语 AsrEditor（现有行为），隐藏「立即翻译」按钮
- `translateMode !== 'off'`：强制 `expanded=true`，渲染上下分栏（原文 AsrEditor + 译文 TranslationPane）
- 翻译模式退出 → `translateMode='off'`，清空 `translatedText`，节流定时器清除

---

## 3. 翻译触发与数据流

### 3.1 翻译执行

复用现有 `translate_text` 命令（`action_bar_commands.rs:451`，fire-and-forget）：

- 前端 `invoke("translate_text", { text })` 立即返回
- 后端 `std::thread::spawn` → `do_translate_streaming`（按换行切段逐段翻译）
- emit `translate-progress`（每段完成，累积译文）→ 前端 `setTranslatedText`
- emit `translate-done`（全部完成）→ 前端 `setTranslating(false)`

### 3.2 三种翻译触发路径

```
                    ┌─ 首翻（进入翻译模式）──────────┐
                    │  立即 invoke translate_text     │
                    │  setTranslating(true)            │
                    └──────────────────────────────────┘
进翻译模式 ─────────┤
                    ┌─ 自动档节流 ───────────────────┐
                    │  setInterval(N 秒):              │
                    │    if 原文≠lastTranslated &&     │
                    │       !translating: doTranslate  │
                    │    // else 跳过                   │
                    └──────────────────────────────────┘
                    │
                    ┌─ 手动「立即翻译」/ Cmd+T ───────┐
                    │  无视档位立即 doTranslate        │
                    │  （仍受 translating 单飞约束）   │
                    └──────────────────────────────────┘

保存/提交（翻译态）─ 终翻（同步等待）:
  1. asrEditorRef.commit()              // 拿最新原文
  2. 终翻：invoke translate_text + await translate-done
  3. commit_edit 提交【译文】
  4. 退出翻译模式（translateMode='off'）
```

**首翻**、**自动档**、**手动** 三个路径走 `doTranslate()` 公共函数（fire-and-forget）；**终翻**单独处理（同步等待 `translate-done`，确保提交最终译文）。

### 3.3 doTranslate() 公共函数

```ts
const doTranslate = useCallback(async () => {
  const source = asrEditorRef.current?.getText() ?? text;
  if (!source.trim()) return;
  if (translatingRef.current) return;          // 单飞
  translatingRef.current = true;
  setTranslating(true);
  lastTranslatedRef.current = source;
  try {
    await invoke("translate_text", { text: source });
    // translate-progress/done 事件驱动 setTranslatedText / setTranslating
  } catch (e) {
    translatingRef.current = false;
    setTranslating(false);
    showToast(ti18n("result.translateFail") + ": " + String(e));
  }
}, [text, showToast]);
```

> **注**：`AsrEditorHandle` 需新增 `getText()` 方法（当前只有 `commit()`），用于节流比较和终翻取原文——commit 会清空编辑态副作用，节流路径不应触发。

### 3.4 终翻的同步等待

终翻需要同步拿最终译文，但 `translate_text` 是 fire-and-forget。实现：终翻 invoke 后，`listen("translate-done")` 一次性 Promise resolve：

```ts
const finalTranslate = async (): Promise<string> => {
  const source = asrEditorRef.current?.getText() ?? text;
  if (!source.trim()) return "";
  await invoke("translate_text", { text: source });
  return new Promise<string>((resolve) => {
    const unlisten = listen("translate-done", (e) => {
      unlisten();
      resolve(e.payload as string);
    });
  });
};
```

### 3.5 translate-progress / done 监听

翻译模式下 listen 两个事件：

```ts
useEffect(() => {
  if (translateMode === 'off') return;
  const fnProgress = listen("translate-progress", (e) => setTranslatedText(e.payload as string));
  const fnDone = listen("translate-done", (e) => {
    setTranslatedText(e.payload as string);
    translatingRef.current = false;
    setTranslating(false);
  });
  return () => { fnProgress.then(f => f()); fnDone.then(f => f()); };
}, [translateMode]);
```

### 3.6 节流定时器生命周期

```ts
useEffect(() => {
  if (translateMode === 'off' || translateMode === 'manual') {
    if (throttleTimerRef.current) { clearInterval(throttleTimerRef.current); throttleTimerRef.current = null; }
    return;
  }
  const secs = parseInt(translateMode);              // '8s' → 8
  throttleTimerRef.current = setInterval(() => {
    const current = asrEditorRef.current?.getText() ?? text;
    if (current !== lastTranslatedRef.current && !translatingRef.current) {
      doTranslate();
    }
  }, secs * 1000);
  return () => { if (throttleTimerRef.current) clearInterval(throttleTimerRef.current); };
}, [translateMode, text, doTranslate]);
```

### 3.7 保存语义（翻译态）

```ts
const onSave = useCallback(async () => {
  if (translateMode === 'off') {
    // 单语态：现有行为——AsrEditor commit → commit_edit 提交原文
    asrEditorRef.current?.commit();
    return;
  }
  // 翻译态：终翻 → 提交译文 → 退出
  asrEditorRef.current?.commit();          // 先提交原文编辑（同步拿最新原文）
  const finalText = await finalTranslate();   // 终翻（同步等待 translate-done）
  const submitText = finalText.startsWith("❌") ? translatedText : finalText;
  invoke("commit_edit", {               // 提交译文到光标
    text: submitText,
    dirtyRanges: [],
    hasEdited: false,
    caret: null,
    selection: null,
  });
  setTranslateMode('off');                 // 退出翻译模式
  setTranslatedText("");
}, [translateMode, translatedText]);
```

> **注**：翻译态下 `asrEditorRef.current?.commit()` 不会触发原文的 `commit_edit`——因为 AsrEditor 的 `onCommit` 回调是**调用方决定的**。现有 `onCommit` 直接 invoke `commit_edit`，翻译态需改为：翻译态下 `onCommit` 只更新 `text` state（不 invoke `commit_edit`），由 `onSave` 统一处理提交。实现细节：
>
> ```ts
> onCommit={(payload) => {
>   setText(payload.text);              // 翻译态/单语态都更新 text
>   if (translateMode === 'off') {      // 仅单语态走原 commit_edit 路径
>     invoke("commit_edit", { ...payload });
>   }
> }}
> ```
>
> 但注意闭包陷阱：`onCommit` 是 AsrEditor 内部 CM6 updateListener 触发的，需读 `translateModeRef.current`（ref 镜像）而非闭包捕获的 `translateMode`。

---

## 4. 组件设计

### 4.1 新建 TranslationPane.tsx

`crates/desktop/frontend/src/pages/Result/TranslationPane.tsx`：

```tsx
interface TranslationPaneProps {
  text: string;              // 译文（来自 translate-progress/done）
  translating: boolean;      // loading 态
  onChange?: (s: string) => void;  // 用户手动编辑译文
}
```

- 极简 CM6 实例：复用 AsrEditor 的主题/字体配置，**无** dirtyRanges / caret / commit / enter_edit_mode 逻辑
- 纯展示 + 可编辑（用户可手动改译文，改完作为终翻的兜底——若终翻失败用最后一次译文）
- `translating=true` 时顶部显示 `⏳ 翻译中…` 文案
- 不接收 `update-result`（只有原文区接收 ASR 流式）

### 4.2 Result/index.tsx 渲染分流

```tsx
{/* 文本区 */}
<div className="flex-1 px-3.5 pt-1 pb-2 overflow-hidden relative">
  {translateMode === 'off' ? (
    /* 现有：单语 AsrEditor */
    <div className="relative h-full">
      <AsrEditor ... />
    </div>
  ) : (
    /* 新增：上下分栏 */
    <div className="flex flex-col h-full gap-1">
      {/* 原文区（上）—— 现有 AsrEditor，继续接收 update-result */}
      <div className="flex-1 min-h-0 border-b border-black/[0.06] overflow-hidden">
        <AsrEditor ... />
      </div>
      {/* 译文区（下）—— 新建 TranslationPane */}
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

- `min-h-0` + `flex-1` 保证两栏各自可滚动（[架构文档 gotcha](../../architecture.md)：CM6 滚动条需要所有 flexbox 祖先链 `min-h-0`）
- 分割线 `border-b border-black/[0.06]`（轻量，不抢视觉）

### 4.3 布局示意

```
单语大窗（现有）             翻译模式（新增）
┌────────────────────┐      ┌────────────────────┐
│ [工具栏]            │      │ [工具栏]+翻译下拉  │
├────────────────────┤      ├────────────────────┤
│                    │      │ 原文区（AsrEditor） │
│  AsrEditor         │      │  ──────────────    │
│  （单语）           │      │ 译文区（新组件）    │
│                    │      │                    │
└────────────────────┘      └────────────────────┘
720×480                     720×480（复用大窗尺寸）
```

两栏各约 230px 高，够阅读。

---

## 5. 工具栏改动

### 5.1 现有 vs 改后

```
现有：[close] [settings] [denoise] [polish] [polish-now] [toggle-size] [save]
改后：[close] [denoise] [polish] [polish-now] [翻译模式▼] [立即翻译] [toggle-size] [save]
```

- **移除** settings（改经托盘菜单进入设置）
- **新增** 翻译模式下拉（languages 图标 + ▼ 指示）
- **新增** 立即翻译按钮（rotate/refresh 图标）

### 5.2 翻译模式下拉

- 单语态（`translateMode==='off'`）：点击 = 进入翻译模式（`setTranslateMode(记忆档位 || 'manual')` + 自动 `setExpanded(true)` + 首翻一次）
- 翻译态（`translateMode!=='off'`）：点击 = 展开下拉菜单
  ```
  ● 手动
  ○ 自动 8s
  ○ 自动 12s
  ○ 自动 15s
  ```
  - 当前档位带 ●
  - 点击某项 → `setTranslateMode(新档位)` + `invoke("set_translate_mode", { mode })` 写 DB + 重置节流定时器

### 5.3 立即翻译按钮

- `translateMode!=='off'` 时可见可点，`translateMode==='off'` 时隐藏（或 `display:none`，保持工具栏紧凑）
- 点击 → `doTranslate()`（受 `translating` 单飞约束）
- `translating=true` 时置灰禁用

### 5.4 快捷键 Cmd+T

`Result/index.tsx` 的 keydown handler 新增：

```ts
if (e.metaKey && e.key === 't') {
  e.preventDefault();
  if (translateMode !== 'off') doTranslate();
}
```

- 单语态 `Cmd+T` 无效（不自动进翻译模式，保持"显式点按钮进双语"的语义——避免误触进翻译模式后用户得手动退出）

---

## 6. 后端命令改动

### 6.1 新增 `set_translate_mode` 命令

`runtime_config.rs`：

```rust
#[tauri::command]
pub fn set_translate_mode(mode: String) -> Result<(), String> {
    let valid = matches!(mode.as_str(), "manual" | "8s" | "12s" | "15s");
    if !valid {
        return Err(format!("translate_mode='{}' 非法（应为 manual/8s/12s/15s）", mode));
    }
    octopus_infra::db::save_config_key("translate_mode", &mode).map_err(|e| e.to_string())
}
```

- 纯持久化，无运行时状态（翻译节流完全在前端）
- 与 `set_denoise_mode` 一致走 `save_config_key`

### 6.2 `toolbar_state` 命令补 `translate_mode`

```rust
pub fn toolbar_state(rc: State<'_, SharedRuntimeConfig>) -> Result<ToolbarState, String> {
    // ... 现有逻辑 ...
    let translate_mode = octopus_infra::db::get_config_key("translate_mode")
        .unwrap_or("manual".to_string());
    Ok(ToolbarState {
        // ... 现有字段 ...
        translate_mode,
    })
}
```

### 6.3 main.rs 注册

`main.rs` 的 invoke_handler 新增 `set_translate_mode`。

### 6.4 笔误清理（附带）

`runtime_config.rs:350` 日志文案 "写回 config.yaml 失败（polish_mode={}）" 改为 "写回 DB 失败（polish_mode={}）"——实际写 DB（`persist_polish_mode` 调 `db::save_config_key`），文案遗留笔误。

---

## 7. i18n 文案

`zh-CN.yaml` / `en.yaml` 新增：

| key | zh-CN | en |
|-----|-------|-----|
| `result.translate` | 翻译 | Translate |
| `result.translate.manual` | 手动 | Manual |
| `result.translate.auto8` | 自动 8s | Auto 8s |
| `result.translate.auto12` | 自动 12s | Auto 12s |
| `result.translate.auto15` | 自动 15s | Auto 15s |
| `result.translateNow` | 立即翻译 | Translate now |
| `result.translating` | 翻译中… | Translating… |
| `result.translateFail` | 翻译失败 | Translation failed |

---

## 8. 边界与降级

### 8.1 翻译引擎未配置

`resolve_translate_strategy` 返回无可用引擎时，`do_translate_streaming` emit `translate-done` 携带 `❌ 翻译失败: ...`。译文区显示错误文案，用户可：

- 手动改原文后重试
- 退出翻译模式（点下拉选"手动"后再次点按钮退出 / 保存提交当前译文）

### 8.2 终翻失败

终翻失败时（`translate-done` 携带 `❌` 前缀），提交**最后一次成功的译文**（`translatedText` 当前值）；若从未成功翻译过，提交空串 + toast 提示。不阻塞用户保存流程。

### 8.3 识别中途进翻译模式

- 原文区继续接收 `update-result`，ASR 不中断
- 自动档节流正常工作（每 N 秒检查原文变化）
- 用户可继续说话，译文定期跟进

### 8.4 翻译中退出翻译模式

- 清除节流定时器
- 正在进行的 `do_translate_streaming` 后台继续跑完（无害，emit 事件因前端已卸载 listener 被丢弃）
- 译文栏清空

### 8.5 迷你窗限制

- 进翻译模式强制 `setExpanded(true)`（大窗）
- 翻译模式下「展开/收起」按钮禁用或隐藏（防止收成迷你窗后双语栏挤崩）

---

## 9. 测试要点

### 9.1 前端单测

- `doTranslate` 单飞：`translating=true` 时二次调用不触发 invoke
- 节流定时器：档位切换正确启动/清除 interval；`translateMode==='manual'`/`'off'` 无 interval
- 终翻 Promise：`translate-done` resolve 正确 payload
- 渲染分流：`translateMode==='off'` 渲染单 AsrEditor；否则渲染上下分栏

### 9.2 后端单测

- `set_translate_mode`：合法值写 DB 成功；非法值返回 Err
- `toolbar_state`：DB 有 `translate_mode` 键时正确返回；无键时默认 `"manual"`

### 9.3 手动验证

1. 单语态点翻译 → 自动进大窗 + 首翻 + 默认"手动"
2. 切"自动 8s" → 持续说话 → 译文每 ~8s 跟进
3. 点「立即翻译」→ 立即重翻
4. Cmd+T → 等同立即翻译
5. 保存 → 终翻 + 提交译文 + 退出翻译模式
6. 重启 → 进翻译模式 → 默认用上次记忆的档位
7. 译文引擎未配置 → 错误文案显示

---

## 10. 影响范围

| 文件 | 改动 |
|------|------|
| `crates/desktop/frontend/src/pages/Result/TranslationPane.tsx` | **新建** 译文区组件 |
| `crates/desktop/frontend/src/pages/Result/index.tsx` | **修改** 翻译模式 state + 渲染分流 + 工具栏 + 节流 + 终翻 + 移除 settings 按钮 |
| `crates/desktop/frontend/src/pages/Result/AsrEditor.tsx` | **修改** AsrEditorHandle 新增 `getText()` |
| `crates/desktop/frontend/src/locales/zh-CN.yaml` | **修改** 新增翻译相关 key |
| `crates/desktop/frontend/src/locales/en.yaml` | **修改** 新增翻译相关 key |
| `crates/desktop/src/runtime_config.rs` | **修改** 新增 `set_translate_mode` 命令 + `ToolbarState.translate_mode` + 笔误清理 |
| `crates/desktop/src/main.rs` | **修改** 注册 `set_translate_mode` |
| `docs/architecture.md` | **修改** 补 Result 浮窗翻译双语视图说明 |

**无后端翻译逻辑改动**——完全复用现有 `translate_text` / `do_translate_streaming` / `detect_translate_direction`。
