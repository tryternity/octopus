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

  // Esc 关闭
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => { if (e.key === "Escape") getCurrent().hide(); };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);

  // 浮窗外点击关闭（mousedown capture，同截图标注窗范式）
  useEffect(() => {
    const onDown = () => getCurrent().hide();
    window.addEventListener("mousedown", onDown, true);
    return () => window.removeEventListener("mousedown", onDown, true);
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
- ✅ Esc 关闭（`getCurrent().hide()`，不销毁，下次 show 复用）
- ✅ 浮窗外 mousedown 关闭（capture 阶段，同 AGENTS.md mousedown capture 范式）

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
5. Esc / 点浮窗外 → 浮窗 hide
6. 再次截图翻译 → 浮窗复用，上次译文清空，新译文流式更新

## 8. 文档同步

实现完成后同步：
- `docs/features/screenshot.md`：新增「截图翻译」章节（§11 或接 §10 OCR 后）
- `docs/architecture.md`：`compact_editor_window` 行的「截图翻译（数据通路已支持，UI 后续）」更新为已实现 + 新增 `translate_window` 行
- 本 spec 对应的 plan（`docs/superpowers/plans/2026-08-11-screenshot-translate-float-window.md`）

## 9. 实现注记（实施时补充）

> 实现过程中发现的偏差、新增决策、踩坑记录写在这里。plan 的实施状态表也需同步更新。
