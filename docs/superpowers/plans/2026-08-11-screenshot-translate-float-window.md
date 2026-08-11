# 截图翻译——只读译文浮窗 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 截图工具栏加「翻译」按钮，OCR + 流式翻译后在新只读浮窗 `translate_window` 展示译文。

**Architecture:** 新建 `translate_window` 只读浮窗（参考 `overlay_window` 建窗范式）+ 新命令 `translate_screenshot`（复制 `ocr_screenshot` 骨架，尾部换成 show 浮窗 + 翻译）+ `TranslateEmitTarget::Float` 分支（`emit_to` 定向，复用 `do_translate_streaming`）+ ready 机制（照搬 `result_window`，防 emit 早于 React mount）。

**Tech Stack:** Rust + Tauri 2 + React 19 + TypeScript + Tailwind v4

## Global Constraints

- **工作目录**：`.worktrees/research-tolaria-comparison`（分支 `research/tolaria-comparison`），不 cd 到主干
- **casing 规范**：Tauri 命令返回值 + 事件 payload 统一 camelCase（`#[serde(rename_all = "camelCase")]`）
- **事件名规范**：`translate-window://<event>`（kebab-case 域名 + camelCase payload）
- **图标资源**：在 `crates/desktop/frontend/public/icons/`（非 `src/icons/`），已有 `action-translate.svg` 可复用
- **前端改动后需** `touch crates/desktop/src/main.rs` 强制 cargo 重新嵌入 dist
- **不改 `ocr_screenshot` / `translate_text` 签名**（spec §6 不变量 1、2）

---

## File Structure

**新建文件：**
| 文件 | 职责 |
|---|---|
| `crates/desktop/src/ui/translate_window.rs` | 浮窗建窗 + show_at_mouse + ready 机制（WINDOW_READY + PENDING_TEXT）+ reset 事件 |
| `crates/desktop/frontend/translate.html` | 浮窗 HTML 入口（复制 overlay.html 改 entry） |
| `crates/desktop/frontend/src/entries/translate-main.tsx` | 浮窗 React 入口（12 行，同 screenshot-main） |
| `crates/desktop/frontend/src/pages/Translate/index.tsx` | 只读浮窗页面（listener + 渲染 + 复制/Esc/拖拽） |

**修改文件：**
| 文件 | 改动 |
|---|---|
| `crates/desktop/src/ui/mod.rs` | 加 `pub mod translate_window;` |
| `crates/desktop/src/core/setup.rs:440` | create_windows 加预创建 |
| `crates/desktop/src/core/invoke_handler.rs` | 注册 3 个新命令 |
| `crates/desktop/capabilities/default.json:4` | windows 数组加 `"translate_window"` |
| `crates/desktop/src/action_bar/action_bar_commands/translate.rs:150` | TranslateEmitTarget 加 Float 分支 |
| `crates/desktop/src/record/screenshot_commands/area.rs` | 新增 translate_screenshot 命令 |
| `crates/desktop/frontend/vite.config.ts:51` | input 加 translate entry |
| `crates/desktop/frontend/src/pages/Screenshot/index.tsx` | 加翻译按钮 + doTranslate |
| `crates/desktop/frontend/src/locales/zh-CN.yaml` + `en.yaml` | 加 screenshot.tool.translate 文案 |

---

## Task 1: 后端——`TranslateEmitTarget::Float` 分支

**Files:**
- Modify: `crates/desktop/src/action_bar/action_bar_commands/translate.rs:150-189`

**Interfaces:**
- Consumes: `do_translate_streaming(text, app, target)`（行 279，已有）
- Produces: `TranslateEmitTarget::Float` 变体；`emit_progress`/`emit_done` 的 Float 分支调 `translate_window` 模块的 ready-gated emit 函数

**注意**：Float 分支的 emit 不能直接 `app.emit_to(...)`——必须经 `translate_window` 模块的 ready 机制（`WINDOW_READY` 为 false 时存 PENDING_TEXT）。所以本 task 先加 enum 变体 + emit 分支调用 `translate_window::emit_float_progress/done`（这些函数在 Task 2 实现）。**本 task 编译会失败**（依赖 Task 2 的函数），但这是有意——Task 2 紧接着补齐。

- [ ] **Step 1: 加 Float 变体到 enum**

修改 `crates/desktop/src/action_bar/action_bar_commands/translate.rs:150-153`：

```rust
#[derive(Clone)]
pub(crate) enum TranslateEmitTarget {
    Result,
    CompactEditor { session_id: String },
    /// 截图翻译只读浮窗（emit_to translate_window，走 ready 机制防丢事件）。
    Float,
}
```

- [ ] **Step 2: emit_progress 加 Float 分支**

修改 `emit_progress`（行 157-172），在 CompactEditor 分支后加：

```rust
            TranslateEmitTarget::Float => {
                crate::ui::translate_window::emit_float_progress(app, text);
            }
```

- [ ] **Step 3: emit_done 加 Float 分支**

修改 `emit_done`（行 175-188），在 CompactEditor 分支后加：

```rust
            TranslateEmitTarget::Float => {
                crate::ui::translate_window::emit_float_done(app, text);
            }
```

- [ ] **Step 4: 暂不编译（依赖 Task 2 的 translate_window 模块）**

记下：`translate_window::emit_float_progress` / `emit_float_done` 在 Task 2 实现。Task 1 + Task 2 一起编译验证。

---

## Task 2: 后端——`translate_window.rs` 模块（建窗 + ready 机制）

**Files:**
- Create: `crates/desktop/src/ui/translate_window.rs`
- Modify: `crates/desktop/src/ui/mod.rs:13`

**Interfaces:**
- Consumes: `window_factory::build_float_window` + `FloatWindowSpec`（`window_factory.rs:35`）；`get_mouse_position`（`action_bar_commands`，overlay_window.rs:47 同模式调用）
- Produces:
  - `pub const WINDOW_LABEL: &str = "translate_window"`
  - `pub fn create_translate_window(app: &AppHandle)` — 启动期预建 visible=false
  - `pub fn show_at_mouse(app: &AppHandle)` — 定位 + show + emit reset
  - `pub fn emit_float_progress(app: &AppHandle, text: &str)` — ready-gated emit（供 Task 1 的 Float 分支调）
  - `pub fn emit_float_done(app: &AppHandle, text: &str)` — ready-gated emit
  - `#[tauri::command] pub fn set_translate_window_ready(app: AppHandle)` — 前端 mount 后调

- [ ] **Step 1: 创建 `translate_window.rs`**

创建 `crates/desktop/src/ui/translate_window.rs`：

```rust
//! 截图翻译只读译文浮窗——显示流式翻译结果，不获取键盘焦点。
//!
//! 复用 overlay_window 的最简建窗范式 + result_window 的 ready 机制（防 emit 早于
//! React mount 丢事件）。职责单一：只读展示译文 + 复制 + Esc/外击关闭 + 可拖拽。

use std::sync::atomic::{AtomicBool, Ordering};
use parking_lot::Mutex;
use tauri::{AppHandle, Emitter, Manager};

use crate::ui::window_factory::{build_float_window, FloatWindowSpec};

pub const WINDOW_LABEL: &str = "translate_window";

const WIN_W: f64 = 400.0;
const WIN_H: f64 = 300.0;

/// 前端 React mount 完成 + listener 注册后置 true。emit 早于 mount 时存 PENDING。
static WINDOW_READY: AtomicBool = AtomicBool::new(false);
/// 未 ready 时暂存最新译文（progress 覆盖，done 终态）。
static PENDING_TEXT: Mutex<Option<(String, bool)>> = Mutex::new(None); // (text, is_done)

/// 创建浮窗（启动期调用，visible=false）。
pub fn create_translate_window(app: &AppHandle) {
    if app.get_webview_window(WINDOW_LABEL).is_some() {
        return;
    }
    let _ = build_float_window(app, FloatWindowSpec {
        label: WINDOW_LABEL,
        url: "translate.html",
        title: "",
        inner_size: (WIN_W, WIN_H),
        visible: false,
        resizable: true,
        position: None,
        focused: Some(false),           // 不抢键盘焦点（同 result_window）
        accept_first_mouse: Some(true), // 非激活窗首次点击可靠（同 result_window）
    });
}

/// 在鼠标附近 show 窗口 + emit reset 清空上次译文。
pub fn show_at_mouse(app: &AppHandle) {
    if let Some(win) = app.get_webview_window(WINDOW_LABEL) {
        let (win_x, win_y) = match crate::action_bar::action_bar_commands::get_mouse_position(app) {
            Some((mx, my)) => (mx - WIN_W / 2.0, my - WIN_H - 20.0), // 鼠标上方居中
            None => {
                // fallback：主屏中心偏上（同 overlay_window 范式）
                app.primary_monitor()
                    .ok()
                    .flatten()
                    .map(|m| {
                        let scale = m.scale_factor();
                        let pos = m.position();
                        let sz = m.size();
                        ((pos.x as f64 / scale + sz.width as f64 / scale / 2.0) - WIN_W / 2.0,
                         (pos.y as f64 / scale + sz.height as f64 / scale / 3.0) - WIN_H / 2.0)
                    })
                    .unwrap_or((400.0, 300.0))
            }
        };
        let _ = win.set_position(tauri::Position::Logical(
            tauri::LogicalPosition::new(win_x, win_y),
        ));
        // 重置 ready（防上次 hide 后 ready 残留 true，新翻译 emit 早于 listener 重注册）
        WINDOW_READY.store(false, Ordering::SeqCst);
        *PENDING_TEXT.lock() = None;
        let _ = win.show();
        // 通知前端清空上次译文（listener 已注册，reset 不参与 ready 机制）
        let _ = app.emit_to(WINDOW_LABEL, "translate-window://reset", ());
    }
}

/// ready-gated emit progress（供 TranslateEmitTarget::Float 调）。
pub fn emit_float_progress(app: &AppHandle, text: &str) {
    if WINDOW_READY.load(Ordering::SeqCst) {
        let _ = app.emit_to(WINDOW_LABEL, "translate-window://progress", text);
    } else {
        *PENDING_TEXT.lock() = Some((text.to_string(), false));
    }
}

/// ready-gated emit done（供 TranslateEmitTarget::Float 调）。
pub fn emit_float_done(app: &AppHandle, text: &str) {
    if WINDOW_READY.load(Ordering::SeqCst) {
        let _ = app.emit_to(WINDOW_LABEL, "translate-window://done", text);
    } else {
        *PENDING_TEXT.lock() = Some((text.to_string(), true));
    }
}

/// 前端 mount 完成 + listener 注册后调用：flush pending + 标记 ready。
#[tauri::command]
pub fn set_translate_window_ready(app: AppHandle) {
    WINDOW_READY.store(true, Ordering::SeqCst);
    let pending = PENDING_TEXT.lock().take();
    if let Some((text, is_done)) = pending {
        if is_done {
            let _ = app.emit_to(WINDOW_LABEL, "translate-window://done", &text);
        } else {
            let _ = app.emit_to(WINDOW_LABEL, "translate-window://progress", &text);
        }
    }
}
```

- [ ] **Step 2: 注册模块**

修改 `crates/desktop/src/ui/mod.rs`，在 `pub mod overlay_window;`（行 7）后加：

```rust
pub mod translate_window;
```

- [ ] **Step 3: 注册 ready 命令到 invoke_handler**

修改 `crates/desktop/src/core/invoke_handler.rs`，找到 `result_window_ready` 注册行（行 42 附近），在附近加：

```rust
crate::ui::translate_window::set_translate_window_ready,
```

- [ ] **Step 4: 编译验证**

```bash
cd /Users/wudarui/workspace/agent/octopus/.worktrees/research-tolaria-comparison
cargo build -p octopus-desktop 2>&1 | tail -30
```

Expected: 0 error（`emit_float_progress`/`emit_float_done` 已在 Task 2 Step 1 实现，Task 1 的引用可解析）。可能有 unused warning（`translate_screenshot` 命令还没写），记录但继续。

- [ ] **Step 5: Commit**

```bash
git add crates/desktop/src/ui/translate_window.rs crates/desktop/src/ui/mod.rs crates/desktop/src/action_bar/action_bar_commands/translate.rs crates/desktop/src/core/invoke_handler.rs
git commit -m "feat(translate): translate_window 浮窗模块 + TranslateEmitTarget::Float 分支

新建 translate_window.rs（建窗 + show_at_mouse + ready 机制防 emit 早于 mount 丢失）。
TranslateEmitTarget 加 Float 变体，emit 走 ready-gated emit_to 定向。"
```

---

## Task 3: 后端——`translate_screenshot` 命令 + setup 预创建 + capability

**Files:**
- Modify: `crates/desktop/src/record/screenshot_commands/area.rs`（在 `ocr_screenshot` 后新增命令）
- Modify: `crates/desktop/src/core/setup.rs:440`
- Modify: `crates/desktop/src/core/invoke_handler.rs`
- Modify: `crates/desktop/capabilities/default.json:4`

**Interfaces:**
- Consumes: `ocr_screenshot` 同套（`OcrLockGuard` + `save_screenshot_to_history` + `OcrEngine` + `close_all_screenshot_windows`）；`translate_window::show_at_mouse`；`do_translate_streaming` + `TranslateEmitTarget::Float`
- Produces: `#[tauri::command] pub async fn translate_screenshot(request, app_handle) -> Result<(), String>`

- [ ] **Step 1: 在 area.rs 加 translate_screenshot 命令**

修改 `crates/desktop/src/record/screenshot_commands/area.rs`，在 `ocr_screenshot` 函数（行 414 `Ok(())` 结束）后、`scan_qrcode_screenshot`（行 420）前插入：

```rust
/// 截图翻译：合成选区 → 图片入库 → OCR 识别 → 关截图窗 + show translate_window → 流式翻译。
/// 与 ocr_screenshot 同 Raw body 协议 + OcrLockGuard 互斥，尾部换成浮窗 + 翻译。
/// OCR 空文本时 show 浮窗并 emit done 显示错误提示，不调翻译。
#[tauri::command]
pub async fn translate_screenshot(
    request: tauri::ipc::Request<'_>,
    app_handle: tauri::AppHandle,
) -> Result<(), String> {
    let _ocr_lock = octopus_ocr::engine::OcrLockGuard::try_acquire()
        .ok_or_else(|| "前一个 OCR 还未完成，请稍后".to_string())?;
    let tauri::ipc::InvokeBody::Raw(png_bytes) = request.body() else {
        return Err("expected raw binary body".into());
    };
    let png_bytes = png_bytes.clone();

    ALL_CAPTURES.lock().clear();
    PENDING_IMAGES.lock().clear();

    let (ocr_engine, ocr_model) = crate::clipboard::clipboard_commands::current_ocr_meta();

    let (image_id, text) = tokio::task::spawn_blocking(move || {
        let img = ::image::load_from_memory(&png_bytes)
            .map_err(|e| e2s_ctx("解码失败: {:?}", e))?;
        let image_id = save_screenshot_to_history(&png_bytes, Some(&img))?;
        let engine = octopus_ocr::engine::OcrEngine::instance()
            .map_err(e2s)?;
        let (text, _blocks) = engine.recognize_with_blocks_from_image(&img).map_err(e2s)?;
        // OCR 文本独立入库（同 ocr_screenshot，便于后续在 CompactEditor 回看）
        if !text.trim().is_empty() {
            let _ocr_id = octopus_infra::db::with_db(|conn| {
                octopus_clipboard::store::insert_ocr_item(conn, &text, &ocr_engine, &ocr_model)
            }).map_err(e2s)?;
        }
        Ok::<_, String>((image_id, text))
    })
    .await
    .map_err(e2s)??;
    let _ = image_id; // 保留 image_id 以备后续回溯（本 spec 不消费）

    let _ = app_handle.emit("clipboard://changed", ());

    let text_empty = text.trim().is_empty();
    let ah = app_handle.clone();
    let _ = app_handle.run_on_main_thread(move || {
        close_all_screenshot_windows(&ah);
        crate::ui::translate_window::show_at_mouse(&ah);
    });

    if text_empty {
        // OCR 空文本：show 浮窗后等 ready，直接 emit done 显示错误（不调翻译）
        crate::ui::translate_window::emit_float_done(&app_handle, "❌ 未识别到文本");
    } else {
        // 流式翻译：worker 线程跑 do_translate_streaming，事件经 ready 机制 emit_to 浮窗
        let app_clone = app_handle.clone();
        std::thread::spawn(move || {
            crate::action_bar::action_bar_commands::do_translate_streaming(
                &text,
                &app_clone,
                crate::action_bar::action_bar_commands::TranslateEmitTarget::Float,
            );
        });
    }

    Ok(())
}
```

**路径可见性说明**：`do_translate_streaming`（`translate.rs:279`）和 `TranslateEmitTarget`（`translate.rs:150`）都是 `pub(crate)`，经 `action_bar_commands/mod.rs:19` 的 `pub use translate::*;` re-export 到 `action_bar_commands` 命名空间。但 glob re-export 对 `pub(crate)` 项的处理取决于编译器——**先编译验证**。若 `crate::action_bar::action_bar_commands::do_translate_streaming` 报 not found，改用完整路径 `crate::action_bar::action_bar_commands::translate::do_translate_streaming`（`mod translate;` 虽私有，但 area.rs 可经 `crate::action_bar::action_bar_commands::translate::` 访问——需确认，或最稳妥改为在 `translate.rs` 里把这俩项显式 `pub(crate) use` 或在 mod.rs 加 `pub(crate) use translate::{do_translate_streaming, TranslateEmitTarget};`）。

**推荐**：实施时先按上面代码（短路径）编译，报错则去 `action_bar_commands/mod.rs:19` 的 `pub use translate::*;` 旁加一行：
```rust
pub(crate) use translate::{do_translate_streaming, TranslateEmitTarget};
```
显式 re-export 这俩 `pub(crate)` 项，确保 `crate::action_bar::action_bar_commands::do_translate_streaming` 可达。

- [ ] **Step 2: 注册 translate_screenshot 命令**

修改 `crates/desktop/src/core/invoke_handler.rs`，在 `ocr_screenshot` 注册行（行 163）后加：

```rust
crate::record::screenshot_commands::translate_screenshot,
```

- [ ] **Step 3: setup 预创建浮窗**

修改 `crates/desktop/src/core/setup.rs:440`，在 `create_overlay_window` 行后加：

```rust
crate::ui::translate_window::create_translate_window(self.app.handle());
```

- [ ] **Step 4: capability 加 translate_window**

修改 `crates/desktop/capabilities/default.json:4`，windows 数组在 `"overlay_window"` 后加 `"translate_window"`：

```json
"windows": ["main", "result_window", "settings_window", "clipboard_window", "compact_editor_window", "terminal_*", "action_bar_window", "overlay_window", "translate_window", "screenshot_*", "vault_picker_window", "password_generator_window", "download_window", "record_config_window", "record_history_window", "record_area_picker_*", "record_annotation_window", "record_control_window", "onboarding_window"],
```

- [ ] **Step 5: 编译验证**

```bash
cargo build -p octopus-desktop 2>&1 | tail -30
```

Expected: 0 error。若 `do_translate_streaming` 可见性问题，按 Step 1 注意调整路径。

- [ ] **Step 6: Commit**

```bash
git add crates/desktop/src/record/screenshot_commands/area.rs crates/desktop/src/core/setup.rs crates/desktop/src/core/invoke_handler.rs crates/desktop/capabilities/default.json
git commit -m "feat(translate): translate_screenshot 命令 + setup 预创建浮窗 + capability

新命令复制 ocr_screenshot 骨架（同 Raw body + OcrLockGuard），尾部换 show_at_mouse +
do_translate_streaming(Float)。setup 预建 translate_window visible=false。"
```

---

## Task 4: 前端——translate.html + entry + 页面骨架

**Files:**
- Create: `crates/desktop/frontend/translate.html`
- Create: `crates/desktop/frontend/src/entries/translate-main.tsx`
- Create: `crates/desktop/frontend/src/pages/Translate/index.tsx`
- Modify: `crates/desktop/frontend/vite.config.ts:51`

**Interfaces:**
- Consumes: `mountApp`（`lib/mountApp`）；`listen`/`invoke`/`getCurrent`（`@tauri-apps/api`）；`useT`（`lib/i18n`）
- Listens: `translate-window://progress` | `translate-window://done` | `translate-window://reset`
- Invokes: `set_translate_window_ready`；clipboard `writeText`

- [ ] **Step 1: 创建 translate.html（复制 overlay.html 改 entry）**

创建 `crates/desktop/frontend/translate.html`（内容同 `overlay.html`，仅 title + script src 改）：

```html
<!doctype html>
<html lang="zh-CN">
  <head>
    <meta charset="UTF-8" />
    <meta name="viewport" content="width=device-width, initial-scale=1.0" />
    <title>octopus-translate</title>
    <script>
      // 阻断式主题恢复（透明窗口，无 bg 参数）。
      try {
        var themeId = localStorage.getItem("octopus-theme-id");
        if (themeId) {
          document.documentElement.setAttribute("data-theme", themeId);
          var builtin = ["light", "glass-dark", "nord", "raycast"];
          if (builtin.indexOf(themeId) === -1) {
            var css = localStorage.getItem("octopus-custom-theme-css");
            if (css) {
              var s = document.createElement("style");
              s.id = "octopus-custom-theme";
              s.textContent = css;
              document.head.appendChild(s);
            }
          }
        }
      } catch (e) {}
    </script>
  </head>
  <body>
    <div id="root"></div>
    <script type="module" src="/src/entries/translate-main.tsx"></script>
  </body>
</html>
```

- [ ] **Step 2: 创建 translate-main.tsx（复制 screenshot-main）**

创建 `crates/desktop/frontend/src/entries/translate-main.tsx`：

```tsx
// 截图翻译译文浮窗独立入口。
// 与其他窗口 entry 共用：lib/mountApp（启动逻辑）+ lib/theme + lib/i18n + index.css。
import "@/index.css";
import { mountApp } from "@/lib/mountApp";
import Translate from "@/pages/Translate";

mountApp(<Translate />);
```

- [ ] **Step 3: 创建 Translate 页面**

创建 `crates/desktop/frontend/src/pages/Translate/index.tsx`：

```tsx
import { useEffect, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { invoke, getCurrent } from "@tauri-apps/api/core";
import { writeText } from "@tauri-apps/plugin-clipboard-manager";
import { useT } from "@/lib/i18n";

export default function Translate() {
  const [text, setText] = useState("");
  const [done, setDone] = useState(false);
  const [copied, setCopied] = useState(false);
  const t = useT();

  useEffect(() => {
    // 1. 先注册 listener（防 ready flush 时 pending emit 丢失）
    const unlistenProgress = listen<string>("translate-window://progress", (e) => {
      setText(e.payload);
      setCopied(false);
    });
    const unlistenDone = listen<string>("translate-window://done", (e) => {
      setText(e.payload);
      setDone(true);
      setCopied(false);
    });
    const unlistenReset = listen("translate-window://reset", () => {
      setText("");
      setDone(false);
      setCopied(false);
    });
    // 2. 通知后端 ready（触发 pending 文本一次性 emit）
    invoke("set_translate_window_ready").catch((e) => console.error("ready failed:", e));
    return () => {
      unlistenProgress.then((u) => u());
      unlistenDone.then((u) => u());
      unlistenReset.then((u) => u());
    };
  }, []);

  // Esc 关闭
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") getCurrent().hide();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);

  // 浮窗外点击关闭：监听窗口 blur（失焦 = 点击了其他窗口/桌面）。
  // 不用 DOM mousedown capture——translate_window 是独立窗口，DOM mousedown 只在窗口内
  // 触发，capture 阶段会误关内部按钮点击。blur 是窗口级事件，点浮窗外才触发。
  useEffect(() => {
    const win = getCurrent();
    let enabled = false;
    const enableTimer = setTimeout(() => { enabled = true; }, 200);
    const unlistenPromise = win.onFocusChanged(({ payload: focused }) => {
      if (enabled && !focused) win.hide();
    });
    return () => {
      clearTimeout(enableTimer);
      unlistenPromise.then((u) => u());
    };
  }, []);

  const handleCopy = async () => {
    if (!text) return;
    try {
      await writeText(text);
      setCopied(true);
      setTimeout(() => setCopied(false), 1500);
    } catch (e) {
      console.error("copy failed:", e);
    }
  };

  return (
    <div
      data-tauri-drag-region
      className="w-screen h-screen flex flex-col bg-[var(--color-bg)] text-[var(--color-text)] rounded-lg overflow-hidden select-none"
    >
      <header data-tauri-drag-region className="flex items-center justify-between px-3 py-1.5 border-b border-[var(--color-border)] cursor-move">
        <span className="text-xs opacity-60">
          {done ? t("screenshot.translate.done") : t("screenshot.translate.translating")}
        </span>
        <button
          onClick={(e) => { e.stopPropagation(); getCurrent().hide(); }}
          className="opacity-50 hover:opacity-100 text-xs"
          title={t("common.close")}
        >✕</button>
      </header>
      <main className="flex-1 overflow-auto p-3 text-sm leading-relaxed whitespace-pre-wrap break-words select-text">
        {text || <span className="opacity-50">⏳ {t("screenshot.translate.translating")}</span>}
      </main>
      <footer className="flex items-center justify-end gap-2 px-3 py-2 border-t border-[var(--color-border)]">
        <button
          onClick={handleCopy}
          disabled={!text}
          className="px-3 py-1 text-xs rounded bg-[var(--color-accent)] text-white disabled:opacity-40 hover:opacity-90"
        >
          {copied ? t("common.copied") : t("common.copy")}
        </button>
      </footer>
    </div>
  );
}
```

- [ ] **Step 4: vite.config.ts 加 entry**

修改 `crates/desktop/frontend/vite.config.ts:51`，在 `onboarding: "onboarding.html",` 后加：

```ts
        translate: "translate.html",
```

（加在 input 对象内，逗号 + 缩进对齐）

- [ ] **Step 5: 前端构建验证**

```bash
cd /Users/wudarui/workspace/agent/octopus/.worktrees/research-tolaria-comparison/crates/desktop/frontend
npx tsc --noEmit 2>&1 | tail -20
npm run build 2>&1 | tail -20
```

Expected: 0 error。若 tsc 报 `@tauri-apps/plugin-clipboard-manager` 找不到，检查 `package.json` 是否已装（result_window 的 TranslationPane 已用，应该已装）。

- [ ] **Step 6: Commit**

```bash
cd /Users/wudarui/workspace/agent/octopus/.worktrees/research-tolaria-comparison
git add crates/desktop/frontend/translate.html crates/desktop/frontend/src/entries/translate-main.tsx crates/desktop/frontend/src/pages/Translate/index.tsx crates/desktop/frontend/vite.config.ts
git commit -m "feat(translate): translate.html + entry + 只读译文浮窗页面

浮窗 listen translate-window://progress|done|reset，流式渲染译文。
复制/Esc/外击关闭/可拖拽。ready 机制防 emit 早于 mount 丢失。"
```

---

## Task 5: 前端——截图工具栏翻译按钮 + i18n

**Files:**
- Modify: `crates/desktop/frontend/src/pages/Screenshot/index.tsx:745-759`（加 doTranslate）+ `:991-996`（加按钮）
- Modify: `crates/desktop/frontend/src/locales/zh-CN.yaml`（screenshot.tool.translate + screenshot.translate.*）
- Modify: `crates/desktop/frontend/src/locales/en.yaml`

**Interfaces:**
- Consumes: `composeAndCropBytes`（已有）+ `invoke` + `setOcrWarn`/`ocrWarnTimerRef`（已有）+ `t`

- [ ] **Step 1: i18n 加文案**

修改 `crates/desktop/frontend/src/locales/zh-CN.yaml`，在 `screenshot.tool` 段 `watermark: 水印`（约行 1048）后加：

```yaml
    translate: 翻译
```

并在 `screenshot` 段内（`ocrBusy:` 附近）加 `translate` 子段：

```yaml
  translate:
    translating: 翻译中...
    done: 翻译完成
```

修改 `crates/desktop/frontend/src/locales/en.yaml`，同样位置加：

```yaml
    translate: Translate
```

和：

```yaml
  translate:
    translating: Translating...
    done: Translation complete
```

**注意**：`common.copy` 和 `common.copied` 应已存在（result_window 等用），确认 `common.copied` 存在，不存在则补 `copied: 已复制 / Copied`。

- [ ] **Step 2: 加 doTranslate 函数**

修改 `crates/desktop/frontend/src/pages/Screenshot/index.tsx`，在 `doOcr` 函数（行 745-759）后加：

```tsx
  function doTranslate() {
    composeAndCropBytes().then((bytes) => {
      if (!bytes) return;
      invoke("translate_screenshot", bytes as unknown as Record<string, unknown>).catch((e) => {
        const msg = String(e);
        if (msg.includes("还未完成")) {
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

- [ ] **Step 3: 加翻译按钮到工具栏**

修改 `crates/desktop/frontend/src/pages/Screenshot/index.tsx:993`（OCR 按钮 ToolButton 闭合 `/>` 后、QR 按钮 `<ToolButton onClick={doQrScan}` 前）加：

```tsx
          <ToolButton onClick={doTranslate} label={t("screenshot.tool.translate")} icon={
            <img src="icons/action-translate.svg" alt={t("screenshot.tool.translate")} className="w-[18px] h-[18px]" style={{ filter: "var(--icon-filter)" }} />
          } />
```

- [ ] **Step 4: 前端构建验证**

```bash
cd /Users/wudarui/workspace/agent/octopus/.worktrees/research-tolaria-comparison/crates/desktop/frontend
npx tsc --noEmit 2>&1 | tail -20
npm run build 2>&1 | tail -20
```

Expected: 0 error。

- [ ] **Step 5: Commit**

```bash
cd /Users/wudarui/workspace/agent/octopus/.worktrees/research-tolaria-comparison
git add crates/desktop/frontend/src/pages/Screenshot/index.tsx crates/desktop/frontend/src/locales/zh-CN.yaml crates/desktop/frontend/src/locales/en.yaml
git commit -m "feat(translate): 截图工具栏翻译按钮 + i18n 文案

OCR 按钮旁加翻译按钮（action-translate.svg），复用 composeAndCropBytes +
OcrLockGuard 互斥。新文案 screenshot.tool.translate + translate.translating/done。"
```

---

## Task 6: 联调 + 文档同步

**Files:**
- Verify: 全链路手动 e2e
- Modify: `docs/features/screenshot.md`（加截图翻译章节）
- Modify: `docs/architecture.md`（compact_editor_window 行更新 + translate_window 新增）
- Modify: `docs/superpowers/specs/2026-08-11-screenshot-translate-float-window-design.md`（§9 实现注记）
- Modify: 本 plan 文件（实施状态表）

- [ ] **Step 1: 全量构建**

```bash
cd /Users/wudarui/workspace/agent/octopus/.worktrees/research-tolaria-comparison
# 前端改动后需 touch main.rs 强制 cargo 重新嵌入 dist
touch crates/desktop/src/main.rs
cargo build -p octopus-desktop 2>&1 | tail -30
```

Expected: 0 error 0 warning（unused warning 须清掉）。

- [ ] **Step 2: 手动 e2e 验证**

启动 app，测试完整流程：

```bash
./run-octopus.sh
```

验证清单：
- [ ] 截图选区 → 工具栏出现「翻译」按钮（OCR 旁）
- [ ] 点翻译按钮 → 截图窗关闭，鼠标上方弹出 translate_window
- [ ] 浮窗显示「⏳ 翻译中...」→ 译文流式更新
- [ ] 翻译完成 → 头部状态变「翻译完成」
- [ ] 点「复制」→ 按钮变「已复制」→ 粘贴验证译文进剪贴板
- [ ] Esc → 浮窗 hide
- [ ] 再次截图翻译 → 浮窗复用，上次译文清空，新译文流式
- [ ] OCR 空文本（截空白区）→ 浮窗显示「❌ 未识别到文本」
- [ ] 快速连点翻译 → 第二次触发 ocrWarn「前一个 OCR 还未完成」

- [ ] **Step 3: 文档同步——screenshot.md**

在 `docs/features/screenshot.md` 的 OCR 章节（§10）后加 §11 截图翻译章节，描述：
- 触发：工具栏翻译按钮
- 流程：OCR → translate_window 浮窗流式展示
- 交互：复制 / Esc / 外击关闭 / 可拖拽
- 与 OCR 按钮的区别：OCR 开 CompactEditor，翻译开只读浮窗
- 互斥：与 OCR 共用 OcrLockGuard

- [ ] **Step 4: 文档同步——architecture.md**

修改 `docs/architecture.md`：
- `compact_editor_window` 行的「截图翻译（数据通路已支持，UI 后续）」改为「截图翻译已实现（translate_window 只读浮窗，CompactEditor contrast 模式另供纯文本翻译）」
- 窗口列表加 `translate_window` 行（只读译文浮窗，截图翻译触发）

- [ ] **Step 5: spec 实现注记 + plan 实施状态表**

在 `docs/superpowers/specs/2026-08-11-screenshot-translate-float-window-design.md` §9 补实际偏差（如有）。

在本 plan 文件末尾加实施状态表：

```markdown
## 实施状态

| Task | 状态 | 偏差/注记 |
|---|---|---|
| 1. TranslateEmitTarget::Float | ✅ | — |
| 2. translate_window 模块 | ✅ | — |
| 3. translate_screenshot 命令 | ✅ | — |
| 4. 前端浮窗页面 | ✅ | — |
| 5. 工具栏按钮 + i18n | ✅ | — |
| 6. 联调 + 文档 | ✅ | — |
```

- [ ] **Step 6: 最终 commit**

```bash
git add docs/features/screenshot.md docs/architecture.md docs/superpowers/specs/2026-08-11-screenshot-translate-float-window-design.md docs/superpowers/plans/2026-08-11-screenshot-translate-float-window.md
git commit -m "docs: 截图翻译功能文档同步

screenshot.md 加 §11 截图翻译章节；architecture.md translate_window 行 +
compact_editor_window 截图翻译状态更新；spec/plan 实施注记 + 状态表。"
```

---

## Self-Review 清单（实施完成后回看）

1. **Spec 覆盖**：spec §2 数据流（Task 3）、§3.1 translate_screenshot（Task 3）、§3.2 Float 分支（Task 1）、§3.3 translate_window + ready（Task 2）、§3.4 注册清单（Task 2-3）、§4.1 工具栏按钮（Task 5）、§4.2 浮窗页面（Task 4）、§4.3 vite（Task 4）、§4.4 i18n（Task 5）、§5 错误处理（Task 3 text_empty 分支 + do_translate_streaming 已有）、§7 测试（Task 6 e2e）
2. **Placeholder**：无 TBD/TODO，所有代码块完整
3. **类型一致**：`emit_float_progress`/`emit_float_done`（Task 1 调用 ↔ Task 2 定义）；`translate-window://progress|done|reset`（Task 2 emit ↔ Task 4 listen）；`set_translate_window_ready`（Task 2 定义 ↔ Task 4 invoke ↔ Task 3 注册）；`translate_screenshot`（Task 3 定义 ↔ Task 5 invoke ↔ Task 3 注册）

---

## 实施状态

实施于 2026-08-11 完成（Task 1-6）。Step 2 手动 e2e（GUI 启动 app + 点击验证）由用户后续手动执行——subagent 无法做 GUI e2e。

| Task | 状态 | 偏差/注记 |
|---|---|---|
| 1. TranslateEmitTarget::Float | ✅ | 补 `translate_text` 的 `Float => String::new()` 穷举分支（§9.1） |
| 2. translate_window 模块 | ✅ | TOCTOU 修复：ready 判定 + PENDING 写入放同一锁（§9.2，对齐 result_window.rs:256-264） |
| 3. translate_screenshot 命令 | ✅ | `do_translate_streaming` / `TranslateEmitTarget` 经 glob `pub use translate::*;` 短路径可达（§9.6），无需显式 re-export |
| 4. 前端浮窗页面 | ✅ | (a) clipboard 改 `navigator.clipboard.writeText`（plugin 未装，§9.3）；(b) `getCurrentWindow` 从 `@tauri-apps/api/window`（§9.4）；(c) CSS var 名修正 `bg-background/90 text-foreground ... bg-primary text-primary-foreground`（§9.5） |
| 5. 工具栏按钮 + i18n | ✅ | — |
| 6. 联调 + 文档 | ✅ | Step 1 全量构建 0 error 0 warning；Step 2 GUI e2e 待用户手动；Step 3-5 文档已同步（screenshot.md §11、architecture.md translate_window 行 + compact_editor 截图翻译状态、spec §9 实现注记、本表） |

**构建验证**（2026-08-11）：`touch crates/desktop/src/main.rs && cargo build -p octopus-desktop` → 0 error 0 warning。

**Step 2 e2e 验证清单**（用户后续手动跑）：
- [ ] 截图选区 → 工具栏出现「翻译」按钮（OCR 旁）
- [ ] 点翻译按钮 → 截图窗关闭，鼠标上方弹出 translate_window
- [ ] 浮窗显示「⏳ 翻译中...」→ 译文流式更新
- [ ] 翻译完成 → 头部状态变「翻译完成」
- [ ] 点「复制」→ 按钮变「已复制」→ 粘贴验证译文进剪贴板
- [ ] Esc → 浮窗 hide
- [ ] 再次截图翻译 → 浮窗复用，上次译文清空，新译文流式
- [ ] OCR 空文本（截空白区）→ 浮窗显示「❌ 未识别到文本」
- [ ] 快速连点翻译 → 第二次触发 ocrWarn「前一个 OCR 还未完成」
