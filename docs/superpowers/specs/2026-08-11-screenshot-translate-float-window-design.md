# 截图翻译——只读译文浮窗

- 日期：2026-08-11
- 类型：功能增强（截图）
- 优先级：P0（调研 `2026-07-30-translation-landscape-analysis.md` §四 + `2026-08-03-octopus-vs-tolaria-9-modules.md` §2.5/§4.5 双双标为 P0）
- 依赖：现有 `translate_text` 流式翻译（`action_bar/translate.rs`）+ `ocr_screenshot` OCR 编排（`record/screenshot_commands/area.rs`）+ `window_factory::build_float_window` + 截图工具栏 children slot

## 1. 背景与动机

### 1.1 数据通路就绪，触发 UI 缺失

调研报告 `2026-08-03-octopus-vs-tolaria-9-modules.md` §2.1（L211）明确：

> **Translate-on-screenshot** 缺口。`docs/features/screenshot.md` 没有截图翻译条目；架构文档明确写「截图翻译：数据通路已支持，UI 后续」——后端 `translate_text` 命令和 CompactEditor 翻译对照视图已就绪，但截图工具栏没有触发入口。

已就绪的能力：

| 能力 | 位置 | 状态 |
|---|---|---|
| 流式翻译 `do_translate_streaming` | `action_bar/translate.rs:279` | ✅ 按段切分 + emit progress/done |
| 方向检测 + 策略解析 | `translate.rs:56-137` | ✅ CJK→en / 非CJK→zh，LocalModel/CloudModel/FallbackLlm |
| OCR 引擎 | `OcrEngine::recognize_with_blocks_from_image` | ✅ paddle-ocr |
| 截图 OCR 编排范式 | `ocr_screenshot`（`area.rs:322`） | ✅ Raw body PNG → save history → OCR → 开 tab |
| 透明浮窗建窗 helper | `window_factory::build_float_window`（`window_factory.rs:35`） | ✅ 8-10 个浮窗已共享 |
| 截图工具栏 children slot | `AnnotationToolbar`（`AnnotationToolbar.tsx:536`） | ✅ OCR/QR 等业务按钮已走此范式 |

唯一缺口：截图 OCR 后**没有「翻译」触发入口**，也没有独立的「只读译文浮窗」展示结果。

### 1.2 不复用 CompactEditor contrast——本 spec 的产品决策

调研建议复用 CompactEditor `mode="contrast"` 双栏（左原文右译文），但本 spec 的产品决策是：**不开 CompactEditor，而是新开一个独立的只读浮窗只展示译文**。

理由：
- 截图翻译场景的原文是「图片上的文字」，用户已从截图看到原文，不需要左栏回看
- CompactEditor 是编辑器（可写、多 tab、持久化），只读译文用编辑器过重
- 独立浮窗轻量、不抢焦点、可拖拽、随用随关，更贴合「快速看一眼译文」的场景
- 与竞品（Pot / STranslate / eSearch）的「截图翻译弹窗」体验一致

### 1.3 本 spec 范围

截图工具栏加「翻译」按钮 → 新建 `translate_window` 只读浮窗 → OCR + 流式翻译 → 译文流式展示。

**不在范围**（YAGNI，留后续）：
- Action Bar「截图翻译」action（调研 §10.2 C 线第二入口）
- Quick Access Overlay（调研 §2.5 #4，截图后右下角缩略卡片六动作）
- 图片翻译（矢量覆盖层在原图位置呈现译文，Stranslate 做法，工作量更大）
- 多引擎并行对比（P1 档位）

## 2. 架构（数据流）

```
[截图工具栏「翻译」按钮]
   ↓ invoke("translate_screenshot", <PNG bytes>)   ← Raw body 协议（同 ocr_screenshot）
[translate_screenshot 命令（area.rs，新）]
   ├─ OcrLockGuard::try_acquire()（复用现有互斥）
   ├─ spawn_blocking:
   │    ├─ 解码 PNG
   │    ├─ save_screenshot_to_history → image_id（可选，保留以备后续回溯）
   │    ├─ OcrEngine::recognize_with_blocks_from_image → text
   │    │    ├─ text 空 → emit "translate-window://done" payload="❌ 未识别到文本"，return
   │    │    └─ text 非空 → 继续
   │    └─ run_on_main_thread:
   │         ├─ close_all_screenshot_windows
   │         └─ translate_window::show_at_mouse(&app)   ← show 预建浮窗（鼠标位置）
   │
   └─ std::thread::spawn:
        do_translate_streaming(text, app, TranslateEmitTarget::Float)
            ↓ emit_to("translate_window", "translate-window://progress", accumulated)
            ↓ emit_to("translate_window", "translate-window://done", accumulated)
[translate_window（新浮窗，已预建 visible=false）]
   listen("translate-window://progress|done") → setText → 流式渲染
   done → 用户「复制」/ Esc / 点外关闭
```

### 2.1 三个关键架构决策

**决策 1：新建 `translate_window`，不复用 `result_window`**

`result_window`（`ui/result_window.rs`）是 ASR 录音的核心 UI，深度耦合：
- 常驻点击穿透轮询线程（`start_click_through_poller:139-225`，与 ASR 录音态绑定）
- 多屏跟随 / 按屏记忆位置（`reposition_to_mouse_monitor:319-341`，与 ASR 单键三模式绑定）
- CM6 编辑器 + instant/toggle 双视图
- 启动期已预创建为单例（`setup.rs:60`），无法按需多开

强行复用会拖入 ASR 全部副作用。新建一个职责单一的只读浮窗成本最低（`overlay_window.rs` 的 6 行建窗范式直接可抄）。

**决策 2：新命令 `translate_screenshot`，不改 `ocr_screenshot`**

`ocr_screenshot` 内部固定「关窗 + 开双 CompactEditor tab」逻辑（`area.rs:388-404`）。改它要加 flag 牵连（OCR-only vs OCR+translate 分支），且现有 OCR 按钮的行为不应变。新命令独立编排更干净——复制 `ocr_screenshot` 骨架（同 Raw body 协议、同 `OcrLockGuard`、同 save_history + OCR），尾部换成「show translate_window + translate」。

**决策 3：加 `TranslateEmitTarget::Float` 分支，不复用 Result 广播**

`TranslateEmitTarget` enum（`translate.rs:150`）专为扩展设计，注释写明「决定 emit 哪套事件名 + payload 结构」。加第三个 `Float` 分支用 `emit_to("translate_window", ...)` 定向 emit，不复用 `Result` 分支的全局广播 `emit`——避免译文泄漏到 result_window（若同时显示）。这符合 `translate.rs:146`「彻底隔离跨窗口泄漏」的既定方向。

## 3. 后端组件

### 3.1 `translate_screenshot` 新命令（`record/screenshot_commands/area.rs`）

复制 `ocr_screenshot`（行 322-414）骨架：

```rust
#[tauri::command]
pub async fn translate_screenshot(
    request: tauri::ipc::Request<'_>,   // Raw body PNG
    app_handle: tauri::AppHandle,
) -> Result<(), String>                 // fire-and-forget，同 ocr_screenshot
```

内部流程：
1. `OcrLockGuard::try_acquire()` — 拿不到锁返回中文错误（前端同 `ocrWarn` 提示）
2. `spawn_blocking` 内：解码 PNG → `save_screenshot_to_history`（拿 image_id，用于后续可能的回溯，但本 spec 不消费）→ `OcrEngine::recognize_with_blocks_from_image` → text
3. **text 空**：`run_on_main_thread` 内 `close_all_screenshot_windows` + `translate_window::show_at_mouse`，然后 `emit_to("translate_window", "translate-window://done", "❌ 未识别到文本")`
4. **text 非空**：`run_on_main_thread` 内 `close_all_screenshot_windows` + `translate_window::show_at_mouse`，然后 `std::thread::spawn` 跑 `do_translate_streaming(&text, &app, TranslateEmitTarget::Float)`

**注意 `emit_to` 与 ready 机制的时序**：show 浮窗后前端 React mount 需数百 ms，`do_translate_streaming` 的首段 progress 可能在 listener 注册前 emit。必须用 ready 机制兜底（见 3.3）。

### 3.2 `TranslateEmitTarget::Float` 新分支（`action_bar/translate.rs:150`）

```rust
pub(crate) enum TranslateEmitTarget {
    Result,
    CompactEditor { session_id: String },
    Float,  // 新增
}
```

`impl TranslateEmitTarget`（行 155-189）的 `emit_progress` / `emit_done` 加 `Float` 分支：

```rust
TranslateEmitTarget::Float => {
    let _ = app.emit_to("translate_window", "translate-window://progress", text);
}
// emit_done 同样 emit_to "translate-window://done"
```

**不缓存**：Float 路径不用 `TRANSLATE_RESULTS` 缓存（那是 CompactEditor session 路径的兜底）。Float 用独立的 ready 机制（见 3.3）。

入口命令 `translate_text`（行 326）的 `target_type` 不动——截图翻译不经过 `translate_text`，而是 `translate_screenshot` 内部直接调 `do_translate_streaming(text, app, TranslateEmitTarget::Float)`。`translate_text` 仅供前端 Result/CompactEditor 的纯文本翻译按钮用。

### 3.3 `translate_window.rs` 新模块（`ui/`）

参考 `overlay_window.rs` 全套范式：

```rust
pub const WINDOW_LABEL: &str = "translate_window";

/// 启动期预建 visible=false（setup.rs:413 create_windows 调用）。
pub fn create_translate_window(app: &AppHandle) {
    if app.get_webview_window(WINDOW_LABEL).is_some() { return; }
    let _ = build_float_window(app, FloatWindowSpec {
        label: WINDOW_LABEL,
        url: "translate.html",
        title: "",
        inner_size: (400.0, 300.0),
        visible: false,
        resizable: true,
        position: None,
        focused: Some(false),          // 不抢键盘焦点（同 result_window）
        accept_first_mouse: Some(true), // 非激活窗首次点击可靠（同 result_window）
    });
    // on_window_event: 监听 Destroyed → 重置 WINDOW_READY（下次重建后 ready 流程重走）
}

/// 在鼠标附近 show 窗口（不调 set_focus，不抢焦点）。
pub fn show_at_mouse(app: &AppHandle) { /* 复用 get_mouse_position + fallback 逻辑 */ }
```

**ready 机制（必须照搬 `result_window.rs:26-94`）**：

```rust
static WINDOW_READY: AtomicBool = AtomicBool::new(false);
static PENDING_TEXT: Mutex<String> = Mutex::new(String::new());

/// 前端 mount 完成后调用，通知后端可以 emit。
#[tauri::command]
pub fn set_translate_window_ready() {
    WINDOW_READY.store(true, Ordering::SeqCst);
    // 取走 pending 文本一次性 emit（防 emit 早于 mount 丢失）
    let pending = std::mem::take(&mut *PENDING_TEXT.lock());
    if !pending.is_empty() { /* emit_to progress/done */ }
}
```

`TranslateEmitTarget::Float` 的 emit 实现：
- `WINDOW_READY == true` → 直接 `emit_to`
- `WINDOW_READY == false` → 进度累积到 `PENDING_TEXT`（仅保留最新），ready 时一次性 emit

**这是已踩过的坑**（`docs/features/result-window.md §9`）：Tauri v2 对未注册 listener 不缓存事件（fire-and-forget），新窗口 React mount 需数百 ms，emit 早于 mount 会丢。

### 3.4 注册清单（4 处小改）

| 文件 | 改动 |
|---|---|
| `ui/mod.rs` | 加 `pub mod translate_window;` |
| `core/setup.rs:413 create_windows()` | 加 `translate_window::create_translate_window(&app.handle())` |
| `capabilities/default.json:4` | windows 数组加 `"translate_window"` |
| `core/invoke_handler.rs` | 注册 `translate_screenshot` + `set_translate_window_ready` 命令 |

## 4. 前端组件

### 4.1 截图工具栏「翻译」按钮（`pages/Screenshot/index.tsx:989`）

OCR 按钮后加：

```tsx
<ToolButton onClick={doTranslate} label={t("screenshot.tool.translate")}
  icon={<img src="icons/translate.svg" alt="Translate"
    className="w-[18px] h-[18px]" style={{ filter: "var(--icon-filter)" }} />} />
```

`doTranslate` 复制 `doOcr`（行 745-759），仅改 invoke 命令名：

```tsx
function doTranslate() {
  composeAndCropBytes().then((bytes) => {
    if (!bytes) return;
    invoke("translate_screenshot", bytes as unknown as Record<string, unknown>).catch((e) => {
      const msg = String(e);
      if (msg.includes("还未完成")) {  // 同 OcrLockGuard 互斥
        setOcrWarn(true);
        if (ocrWarnTimerRef.current) clearTimeout(ocrWarnTimerRef.current);
        ocrWarnTimerRef.current = setTimeout(() => setOcrWarn(false), 1800);
      } else {
        console.error(e);
      }
    });
  });
}
```

### 4.2 `translate.html` + `entries/translate-main.tsx` + `pages/Translate/index.tsx`（新）

**`translate.html`**（同 `overlay.html` / `result.html` 范式，最小骨架）。

**`entries/translate-main.tsx`**（12 行，同 `screenshot-main.tsx`）：
```tsx
import "@/index.css";
import { mountApp } from "@/lib/mountApp";
import Translate from "@/pages/Translate";
mountApp(<Translate />);
```

**`pages/Translate/index.tsx`**（只读浮窗）：

```tsx
export default function Translate() {
  const [text, setText] = useState("");
  const [done, setDone] = useState(false);
  const t = useT();

  useEffect(() => {
    // 1. 注册 listener（必须在 ready 前，防 ready 时 pending emit 丢失）
    let p = listen<string>("translate-window://progress", e => setText(e.payload));
    let d = listen<string>("translate-window://done", e => { setText(e.payload); setDone(true); });
    // 2. 通知后端 ready（触发 pending 文本一次性 emit）
    invoke("set_translate_window_ready");
    return () => { p.then(u => u()); d.then(u => u()); };
  }, []);

  // Esc 关闭（不监听 blur——浮窗置顶，用户需一边看译文一边操作其他窗口）
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => { if (e.key === "Escape") getCurrentWindow().hide(); };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);

  return (
    <div data-tauri-drag-region className="...">
      <header data-tauri-drag-region>  {/* 拖拽区 */}
        <span className="text-xs opacity-60">{/* 语言对或 "翻译中..." */}</span>
      </header>
      <main className="select-text">{text || "⏳ 翻译中..."}</main>
      <footer>
        <button onClick={() => writeText(text)}>{t("common.copy")}</button>
      </footer>
    </div>
  );
}
```

**交互清单**：
- ✅ 流式渲染（progress 事件实时 setText）
- ✅ 复制译文（`@tauri-apps/plugin-clipboard-manager` writeText）
- ✅ 可拖拽（`data-tauri-drag-region`，透明浮窗标配）
- ✅ Esc 关闭（`getCurrentWindow().hide()`，不销毁，下次 show 复用）
- ❌ 不监听 blur 自动关闭——浮窗 `always_on_top` 置顶，用户可一边看译文一边操作其他窗口（对照原文/编辑器），失焦就消失会打断工作流。仅 Esc / ✕ 按钮关闭（用户主动操作）

**生命周期注意**：hide 不销毁窗口（同 overlay_window），下次 `show_at_mouse` 复用单例。React mount 只发生一次，ready 只调一次——但 `text` state 需在每次 show 时重置。

**方案**：后端 `show_at_mouse` 内 show 后立即 `emit_to("translate_window", "translate-window://reset", ())`，前端 listen 此事件清空 text + done state（`setText("")` + `setDone(false)`）。这样：
- 首次翻译：mount → ready → progress/done 正常流
- 再次翻译：show → reset 清屏 → progress/done 正常流
- reset 事件不参与 ready 机制（show 时前端 listener 已注册，reset 不会早于 mount）

`pages/Translate/index.tsx` 的 listener 三件套变四件套：progress / done / **reset**。

### 4.3 vite.config.ts

`frontend/vite.config.ts:34-51` rollupOptions.input 加：
```ts
translate: resolve(__dirname, "translate.html"),
```

### 4.4 i18n

`locales/zh-CN.yaml` + `en.yaml` 的 `screenshot.tool` 段（zh-CN.yaml:1047 / en.yaml:1038 附近，`watermark` 后）加：
```yaml
# zh-CN
translate: 翻译
# en
translate: Translate
```

### 4.5 图标

`frontend/src/icons/translate.svg`——参考 action_bar 已有的翻译图标（若有），或用 lucide `languages` / `translate` 图标。与 `ocr-ai.svg` / `qr-code.svg` 同目录同尺寸规范。

## 5. 错误处理 / 降级

| 场景 | 处理 |
|---|---|
| OCR 空文本 | 浮窗 show + emit `done` payload="❌ 未识别到文本"，不调翻译 |
| OCR 互斥（前一个未完成） | 命令返回中文错误，前端 `setOcrWarn`（同现有 OCR 按钮） |
| 翻译失败（引擎错误） | `do_translate_streaming` 已有 `❌ 翻译失败: {e}` 走 done 事件，浮窗直接显示 |
| 翻译引擎未配置 | `do_translate` FallbackLlm 路径报 "翻译 fallback LLM 未配置"，浮窗显示 |
| emit 早于 listener 注册 | ready 机制（`PENDING_TEXT` + `set_translate_window_ready`）兜底 |
| 浮窗已显示时再次翻译 | 后端 `show_at_mouse` 内 show 后 emit `translate-window://reset`，前端清空 text + done（见 4.2） |

## 6. 不变量

1. **`ocr_screenshot` 行为不变**——现有 OCR 按钮仍开双 CompactEditor tab，新翻译按钮独立命令
2. **`translate_text` 签名不变**——Result/CompactEditor 的纯文本翻译路径不受影响，Float 路径不经此命令
3. **`result_window` 不受影响**——Float 路径用 `emit_to` 定向，不广播到 result_window
4. **浮窗 hide 不销毁**——复用单例，避免反复 mount 的 ready 竞态
5. **OCR 互斥共享**——`translate_screenshot` 与 `ocr_screenshot` 共用 `OcrLockGuard`，不可并发

## 7. 测试

### 7.1 后端单测（`#[cfg(test)] mod tests`）

- `TranslateEmitTarget::Float` 的 emit 定向：mock AppHandle 验证 `emit_to("translate_window", ...)` 不泄漏到其他窗口 label
- ready 机制：`WINDOW_READY == false` 时 emit 进 `PENDING_TEXT`，`set_translate_window_ready` 后取走

### 7.2 前端

- `pages/Translate/index.tsx` 的 listener 注册 → setText 渲染
- ready 调用时序（listener 先于 ready 注册）
- Esc / 外部 click / 复制按钮交互

### 7.3 手动 e2e（核心验证）

1. 截图选区 → 工具栏点「翻译」按钮
2. 截图窗关闭，鼠标位置弹出 translate_window
3. 浮窗显示「⏳ 翻译中...」→ 译文流式更新 → done
4. 点「复制」→ 译文进剪贴板
5. Esc / ✕ 按钮 → 浮窗 hide（不监听 blur——浮窗置顶，允许一边看译文一边操作其他窗口）
6. 再次截图翻译 → 浮窗复用，上次译文清空，新译文流式更新

## 8. 文档同步

实现完成后同步：
- `docs/features/screenshot.md`：新增「截图翻译」章节（§11 或接 §10 OCR 后）
- `docs/architecture.md`：`compact_editor_window` 行的「截图翻译（数据通路已支持，UI 后续）」更新为已实现 + 新增 `translate_window` 行
- 本 spec 对应的 plan（`docs/superpowers/plans/2026-08-11-screenshot-translate-float-window.md`）

## 9. 实现注记（实施时补充）

> 实现过程中发现的偏差、新增决策、踩坑记录写在这里。plan 的实施状态表也需同步更新。

实现于 2026-08-11 完成（Task 1-6）。以下为实施过程中的偏差、新增决策、踩坑记录。

### 9.1 `translate_text` 的 `Float => String::new()` 分支补全（Task 1+2，brief 未提及）

`translate.rs::translate_text`（行 342 附近）的 match arms 按 `TranslateEmitTarget` 穷举 `session_id` 字段（`Result => String::new()`、`CompactEditor { session_id } => session_id.clone()`）。加 `Float` 变体（无字段）后编译器报 non-exhaustive。补 `Float => String::new()`——Float 路径不经 `translate_text` 命令（直接调 `do_translate_streaming`），此分支理论不会执行，仅为满足穷举。

### 9.2 TOCTOU 修复（Task 2，brief 描述的 ready 机制有竞态）

Brief 里 ready 机制描述：`emit_float_progress` 读 `WINDOW_READY` 后写 `PENDING_TEXT`——load + store 非原子，若 `set_translate_window_ready` 在两者之间执行（load 时未 ready → store 时已 ready → flush 空 PENDING → 下次 store 覆盖空），事件会丢。

修复（对齐 `result_window.rs:256-264` 的同款修复范式）：`emit_float_progress` / `emit_float_done` 内「判 ready + 写 PENDING」放同一把 `PENDING_TEXT` 锁。ready 时直接 emit（不持锁太久）；未 ready 时持锁写 PENDING，`set_translate_window_ready` 也持同一锁取 PENDING——互斥消除竞态。

### 9.3 `@tauri-apps/plugin-clipboard-manager` 未安装（Task 4，改用 `navigator.clipboard`）

Brief 假设 `package.json` 已装此插件（result_window 的 TranslationPane 已用）——实际 desktop 项目未安装。10+ 前端组件（TerminalPane、MarkdownPane、action bar 等）统一用浏览器原生 `navigator.clipboard.writeText(text)`（项目通用 pattern）。

改动：`import { writeText } from "@tauri-apps/plugin-clipboard-manager"` → 改用 `await navigator.clipboard.writeText(text)`。功能等价（都是写系统剪贴板），不增依赖。

### 9.4 `getCurrent` API 路径修正（Task 4，brief 的 import 错误）

Brief 写 `import { invoke, getCurrent } from "@tauri-apps/api/core"`——`getCurrent` 在 Tauri v2 不在 `core` 导出。

修正：`getCurrentWindow` 从 `@tauri-apps/api/window` 导入（Tauri v2 标准 API）。所有调用 `getCurrentWindow().hide()` / `getCurrentWindow().onFocusChanged(...)`。

### 9.5 CSS 颜色变量名修正（Task 4，brief 的 var 名 + accent 语义角色错误）

Brief 写 `bg-[var(--color-bg)] text-[var(--color-text)] ... bg-[var(--color-accent)] text-white`。实际项目 Tailwind v4 主题系统的 CSS var 名与语义角色不同：
- `--color-bg` / `--color-text` 不存在（项目无此 var）
- `--color-accent` 不存在；按钮主色应用 `bg-primary text-primary-foreground`（语义角色：primary = 品牌主色 / 强调按钮背景，accent 在项目里是 hover 强调色不是按钮主色）

修正（对齐项目惯例，参考 `pages/Onboarding/` / `pages/Clipboard/` 等组件）：
```tsx
className="w-screen h-screen flex flex-col bg-background/90 text-foreground backdrop-blur-2xl border border-border rounded-lg overflow-hidden select-none"
// 复制按钮：
className="px-3 py-1 text-xs rounded bg-primary text-primary-foreground disabled:opacity-40 hover:opacity-90"
```
- `bg-background/90` + `backdrop-blur-2xl`（透明浮窗磨砂效果，90% 不透明度避免完全遮挡）
- `text-foreground` / `border-border`（标准语义色）
- `bg-primary text-primary-foreground`（主按钮标准配色）

### 9.6 `do_translate_streaming` / `TranslateEmitTarget` 跨模块路径（Task 3，brief 担心的可见性无问题）

Brief Step 1「路径可见性说明」担心 glob re-export 对 `pub(crate)` 项可能不可达，预留 fallback（在 `mod.rs` 加 `pub(crate) use translate::{do_translate_streaming, TranslateEmitTarget};`）。

实测：`action_bar_commands/mod.rs:19` 的 `pub use translate::*;` glob 对 `pub(crate)` 项同样生效——`crate::action_bar::action_bar_commands::do_translate_streaming` 和 `crate::action_bar::action_bar_commands::TranslateEmitTarget::Float` 均可达。**无需显式 re-export**，brief 的 fallback 没用上。

### 9.7 Self-review 补充：未踩坑的点

- ~~**hide 复用单例竞态**：每次 show 前 `WINDOW_READY.store(false)` + 清 PENDING + emit reset——ready 重走，listener 已在前次 mount 注册（除非窗口销毁，但 hide 不销毁），reset 事件不丢。~~ **〔2026-08-11 更正，见 §9.8 C1〕此判断错误**：`set_translate_window_ready` 只在 React mount 时调一次，hide≠destroy 不重 mount → reset 后 ready 永不回 true → 所有后续 emit 进 PENDING 永不 flush。该 reset 必须删除。
- **`close_all_screenshot_windows` 时序**：`run_on_main_thread` 内执行（同 ocr_screenshot），与 show translate_window 同一闭包，顺序执行无竞态。
- **OCR `recognize_with_blocks_from_image`**：与 ocr_screenshot 共用同一 `DynamicImage`（已在 ocr_screenshot 双重解码消除优化中实现），截图翻译天然继承此优化。

### 9.8 全分支 review 修复（2026-08-11）

合并前最终 review 发现两处缺陷，已修复（commit `9cbae2dc` 代码 + `18602bfc` 本文档）。

#### C1（Critical）：`show_at_mouse` 的 ready reset 导致后续翻译永远卡在「⏳ 翻译中...」

**症状**：每次 `show_at_mouse` 都 `WINDOW_READY.store(false, Ordering::SeqCst)` + 清 PENDING。但 `set_translate_window_ready`（唯一把 ready 设回 true 的入口）只在 `pages/Translate/index.tsx` 的 `useEffect(..., [])` 里调一次——React mount 时。浮窗启动期 hidden 创建，hide≠destroy，React 只 mount 一次。故 `show_at_mouse` 把 ready 翻 false 后，**没有任何路径把它翻回 true**。后续 `emit_float_progress` / `emit_float_done` 全部命中 `WINDOW_READY == false` 分支写 PENDING，而 PENDING 只在 `set_translate_window_ready`（永不再次调用）里 flush——译文永久滞留，用户看空「⏳ 翻译中...」。

**根因**：误以为 listener 在每次 show 时重注册。实际 `listen()` 是 React 的 `useEffect` 副作用，依赖 `[]` → 只注册一次。hide≠destroy，React 不 unmount，listener 跨 show 复用。所以 ready 一旦 initial mount 设 true，就不应该再 reset——对齐 `result_window.rs`（grep `WINDOW_READY.store` 只在 `set_*_ready` 命令里有 `store(true)`，无任何 `store(false)`）。

**修复**（`crates/desktop/src/ui/translate_window.rs::show_at_mouse`）：删除两行
```rust
WINDOW_READY.store(false, Ordering::SeqCst);
*PENDING_TEXT.lock() = None;
```
保留 `set_position` / `win.show()` / `emit reset`。reset 事件单独负责清空前端 UI 文本（listener 已注册，收到 reset 即清 state）。

**对照范式**：`result_window.rs` 从不 reset WINDOW_READY——其 `show_at_mouse` 等价函数（result_window 无此函数，但 `show_result_window` 类逻辑）直接 emit，ready 机制靠 initial mount 单次 set true + reset 事件清空 UI。translate_window 现与之一致。

#### I1（Important）：OCR 空文本 `emit_float_done` 与 `show_at_mouse` 竞态

**症状**：`translate_screenshot` 里空文本路径：
```rust
let _ = app_handle.run_on_main_thread(move || {
    close_all_screenshot_windows(&ah);
    crate::ui::translate_window::show_at_mouse(&ah);   // 异步排队到 main thread
});
if text_empty {
    crate::ui::translate_window::emit_float_done(&app_handle, "❌ 未识别到文本");   // 立即执行
}
```
`run_on_main_thread` 是异步排队——emit_float_done 在调用方线程**立即**执行，可能在 show_at_mouse 跑之前就到达前端 listener。若到达时刻 listener 尚未因 show 重置（reset 事件还没 emit），错误文本先写入 state；随后 show_at_mouse 的 reset 事件清空它 → 用户看到空窗。

**修复**（`crates/desktop/src/record/screenshot_commands/area.rs::translate_screenshot`）：把空文本的 `emit_float_done` 移进 `run_on_main_thread` 闭包内、`show_at_mouse` **之后**：
```rust
let text_empty = text.trim().is_empty();
let ah = app_handle.clone();
let _ = app_handle.run_on_main_thread(move || {
    close_all_screenshot_windows(&ah);
    crate::ui::translate_window::show_at_mouse(&ah);
    if text_empty {
        crate::ui::translate_window::emit_float_done(&ah, "❌ 未识别到文本");
    }
});
if !text_empty {
    let app_clone = app_handle.clone();
    std::thread::spawn(move || { /* do_translate_streaming */ });
}
```

**正确性论证（与 C1 修复联动）**：
1. 闭包按序执行：`show_at_mouse` 先跑（含 emit reset），然后 emit_float_done 跑。
2. C1 修复后 `WINDOW_READY` 不再被 show_at_mouse reset → ready 仍为 initial mount 设的 true。
3. 前端 listener 收到事件顺序：`translate-window://reset`（清空 UI）→ `translate-window://done`（写错误文本）。
4. 顺序与预期一致（先清后写），用户看到「❌ 未识别到文本」。✅

**为什么要 move 进闭包而非保留外面**：`run_on_main_thread` 闭包在 main thread 上同步执行，闭包内调用顺序即执行顺序——无任何异步竞态。闭包外的 `if text_empty { emit }` 与排队中的 show_at_mouse 之间是跨线程时序，非确定。
