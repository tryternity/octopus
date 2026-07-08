# AI 命令面板 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 新建 `action_bar_window` 迷你浮窗，用户选中文本→热键→模拟 Cmd+C→弹出动作栏→AI/搜索/翻译/网页→Run And Paste 替换原文。

**Architecture:** 新建独立 Tauri 窗口 + Rust 命令层（模拟 Cmd+C/V、LLM 调用、URL 检测）+ React 前端（两级菜单 + 键盘导航）。复用现有 focus_tracker / clipboard / llm / theme 基础设施。

**Tech Stack:** Rust + Tauri 2 + React 19 + TypeScript + Tailwind v4

## Global Constraints

- **前置条件**：feature 分支基于 main 最新提交
- **测试命令**：`cd crates/desktop/frontend && npm test`（vitest）；`cargo test -p octopus-desktop`（Rust）
- **类型检查**：`npx tsc --noEmit`
- **配置注册**：serde 自动 load/save（3 处：config.rs struct + apply_config_value + db.sql seed）
- **热键注册模式**：与 `register_clipboard_shortcut` 一致（`GlobalShortcutExt::on_shortcut` + `ShortcutState::Pressed`）
- **浮窗模式**：与 `clipboard_window` 一致（transparent + decorations(false) + always_on_top + skip_taskbar）
- **背景色注入**：`window_bg_hex("action_bar_window")` 返回 None（透明窗口不注入）
- **macOS 焦点**：window hide 后 macOS 自动还焦点（不需显式 restore_focus，同 clipboard_window paste 模式）

---

## File Structure

| 文件 | 职责 | 改动类型 |
|------|------|---------|
| `crates/desktop/src/action_bar_window.rs` | **新建** — 窗口创建/显示/隐藏 + 位置计算 | Create |
| `crates/desktop/src/action_bar_commands.rs` | **新建** — 触发/Cmd+C/AI/粘贴/打开URL 命令 | Create |
| `crates/desktop/src/focus_tracker.rs` | 新增 `simulate_copy` | Modify |
| `crates/desktop/src/main.rs` | 注册模块 + 热键 + 命令 | Modify |
| `crates/desktop/src/settings_commands.rs` | `apply_config_value` 加 action_bar 配置 | Modify |
| `crates/infra/src/config.rs` | AppConfig 加 2 个字段 | Modify |
| `crates/infra/src/db.sql` | seed 加 2 行 | Modify |
| `crates/desktop/frontend/src/pages/ActionBar/index.tsx` | **新建** — 浮窗前端 | Create |
| `crates/desktop/frontend/src/pages/ActionBar/urlDetect.ts` | **新建** — URL 宽松检测纯函数 | Create |
| `crates/desktop/frontend/src/pages/ActionBar/urlDetect.test.ts` | **新建** — 单元测试 | Create |
| `crates/desktop/frontend/src/App.tsx` | 路由加 action_bar_window | Modify |
| `crates/desktop/frontend/src/pages/Settings/GeneralPanel.tsx` | 快捷键卡片 + 搜索引擎下拉 | Modify |
| `crates/desktop/frontend/index.html` | 无改动（action_bar_window 是透明窗口，不注入 bg） | — |
| `crates/desktop/capabilities/default.json` | 无改动（已有 show/hide/set_position 权限） | — |

---

### Task 1: URL 宽松检测纯函数 + TDD

**Files:**
- Create: `crates/desktop/frontend/src/pages/ActionBar/urlDetect.ts`
- Test: `crates/desktop/frontend/src/pages/ActionBar/urlDetect.test.ts`

**Interfaces:**
- Produces: `detectActionUrl(text: string): { isUrl: boolean; url: string }` — 宽松 URL 检测

- [ ] **Step 1: 写失败测试**

```typescript
import { describe, it, expect } from "vitest";
import { detectActionUrl } from "./urlDetect";

describe("detectActionUrl", () => {
  const cases: Array<{ input: string; isUrl: boolean; url?: string; note?: string }> = [
    // 域名格式
    { input: "apple.com", isUrl: true, url: "https://apple.com" },
    { input: "github.com/octopus", isUrl: true, url: "https://github.com/octopus" },
    { input: "a.b/c", isUrl: true, url: "https://a.b/c" },
    { input: "foo.com.cn/bar", isUrl: true, url: "https://foo.com.cn/bar" },
    // IP 地址
    { input: "192.168.1.100", isUrl: true, url: "http://192.168.1.100" },
    { input: "127.0.0.1:3000", isUrl: true, url: "http://127.0.0.1:3000" },
    // localhost
    { input: "localhost", isUrl: true, url: "http://localhost" },
    { input: "localhost:8080/api", isUrl: true, url: "http://localhost:8080/api" },
    // 否定
    { input: "hello world", isUrl: false, note: "有空格" },
    { input: "123.456", isUrl: false, note: "纯数字不像域名也不是IP" },
    { input: ".hidden", isUrl: false, note: "以.开头" },
    { input: "end.", isUrl: false, note: "以.结尾" },
    { input: "", isUrl: false },
    { input: "你好世界", isUrl: false, note: "无.无localhost" },
  ];
  for (const c of cases) {
    it(`${c.note ?? c.input}: isUrl=${c.isUrl}`, () => {
      const result = detectActionUrl(c.input);
      expect(result.isUrl).toBe(c.isUrl);
      if (c.url) expect(result.url).toBe(c.url);
    });
  }
});
```

- [ ] **Step 2: 运行测试确认失败**

```bash
cd crates/desktop/frontend && npm test -- src/pages/ActionBar/urlDetect.test.ts
```
Expected: FAIL — 模块不存在

- [ ] **Step 3: 实现**

```typescript
const IPV4_RE = /^\d{1,3}\.\d{1,3}\.\d{1,3}\.\d{1,3}(:\d+)?(\/.*)?$/;
const LOCALHOST_RE = /^localhost(:\d+)?(\/.*)?$/i;

/**
 * 宽松 URL 检测——比剪贴板 detectUrl 更宽松。
 * 三条路径：域名格式 / IP 地址 / localhost。
 */
export function detectActionUrl(text: string): { isUrl: boolean; url: string } {
  const t = text.trim();
  if (!t || t.includes(" ")) return { isUrl: false, url: "" };

  // localhost（无 .）
  if (LOCALHOST_RE.test(t)) {
    return { isUrl: true, url: `http://${t}` };
  }

  // IP 地址
  if (IPV4_RE.test(t)) {
    return { isUrl: true, url: `http://${t}` };
  }

  // 域名格式：含 . 且不以 . 开头/结尾 且 . 两侧至少一侧含字母
  if (t.includes(".") && !t.startsWith(".") && !t.endsWith(".")) {
    const dotIdx = t.indexOf(".");
    const before = t.substring(0, dotIdx);
    const after = t.substring(dotIdx + 1);
    if (/[a-zA-Z]/.test(before) || /[a-zA-Z]/.test(after)) {
      return { isUrl: true, url: `https://${t}` };
    }
  }

  return { isUrl: false, url: "" };
}
```

- [ ] **Step 4: 运行测试确认通过**

```bash
cd crates/desktop/frontend && npm test -- src/pages/ActionBar/urlDetect.test.ts
```
Expected: PASS — 全部 case 通过

- [ ] **Step 5: 提交**

```bash
git add crates/desktop/frontend/src/pages/ActionBar/urlDetect.ts crates/desktop/frontend/src/pages/ActionBar/urlDetect.test.ts
git commit -m "feat(action-bar): URL 宽松检测纯函数（域名/IP/localhost）+ TDD"
```

---

### Task 2: focus_tracker 新增 simulate_copy

**Files:**
- Modify: `crates/desktop/src/focus_tracker.rs`

**Interfaces:**
- Produces: `FocusTracker::simulate_copy()` — 模拟 Cmd+C（macOS 用 osascript，同 simulate_paste 模式）

- [ ] **Step 1: 加 simulate_copy 公开方法**

在 `FocusTracker` impl 块中，`simulate_paste` 下方加：

```rust
    /// 模拟复制按键（Cmd+C / Ctrl+C）。
    pub fn simulate_copy(&self) {
        simulate_copy_platform();
    }
```

- [ ] **Step 2: macOS 实现**

在 `simulate_paste_platform` 函数下方加：

```rust
#[cfg(target_os = "macos")]
fn simulate_copy_platform() {
    use std::process::Command;
    let script = r#"tell application "System Events"
        set p to first process whose frontmost is true
        set frontmost of p to true
        delay 0.15
        keystroke "c" using command down
    end tell"#;
    log::info!("simulate_copy: osascript frontmost + keystroke");
    match Command::new("osascript").args(["-e", script]).output() {
        Ok(out) => {
            if !out.status.success() {
                log::warn!("simulate_copy failed: {}", String::from_utf8_lossy(&out.stderr));
            }
        }
        Err(e) => log::warn!("simulate_copy: {}", e),
    }
}
```

- [ ] **Step 3: Windows/Linux/fallback 空实现**

```rust
#[cfg(target_os = "windows")]
fn simulate_copy_platform() {}

#[cfg(target_os = "linux")]
fn simulate_copy_platform() {}

#[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
fn simulate_copy_platform() {}
```

- [ ] **Step 4: 编译检查**

```bash
cargo check -p octopus-desktop
```
Expected: 无错误

- [ ] **Step 5: 提交**

```bash
git add crates/desktop/src/focus_tracker.rs
git commit -m "feat(action-bar): focus_tracker 新增 simulate_copy（Cmd+C）"
```

---

### Task 3: 配置项注册

**Files:**
- Modify: `crates/infra/src/config.rs`
- Modify: `crates/infra/src/db.sql`
- Modify: `crates/desktop/src/settings_commands.rs`

- [ ] **Step 1: config.rs 加字段 + default**

在 `clipboard_theme` 字段后加：

```rust
    /// AI 命令面板全局热键。默认 CmdOrCtrl+Shift+Space。
    #[serde(default = "default_action_bar_shortcut")]
    pub action_bar_shortcut: String,

    /// AI 命令面板搜索引擎。默认 google。
    #[serde(default = "default_action_bar_search_engine")]
    pub action_bar_search_engine: String,
```

加 default 函数：

```rust
fn default_action_bar_shortcut() -> String {
    "CmdOrCtrl+Shift+Space".into()
}

fn default_action_bar_search_engine() -> String {
    "google".into()
}
```

在 `Default` impl 的 `clipboard_theme` 行后加：

```rust
            action_bar_shortcut: default_action_bar_shortcut(),
            action_bar_search_engine: default_action_bar_search_engine(),
```

- [ ] **Step 2: db.sql seed**

在 `clipboard_theme` 行后加：

```sql
    ('action_bar_shortcut',   'CmdOrCtrl+Shift+Space', 'AI 命令面板快捷键'),
    ('action_bar_search_engine', 'google', 'AI 命令面板搜索引擎'),
```

- [ ] **Step 3: apply_config_value 加校验**

在 `clipboard_theme` 分支后加：

```rust
        "action_bar_shortcut" => {
            cfg.action_bar_shortcut = value.as_str().ok_or("action_bar_shortcut 需要字符串")?.to_string();
        }
        "action_bar_search_engine" => {
            cfg.action_bar_search_engine = value.as_str().ok_or("action_bar_search_engine 需要字符串")?.to_string();
        }
```

- [ ] **Step 4: set_config 加热重载**

在 `set_config` 函数中（`screenshot_shortcut` 热重载块之后），加 `action_bar_shortcut` 热重载：

```rust
    if key == "action_bar_shortcut" && cfg.action_bar_shortcut != old_action_bar_sc {
        use tauri_plugin_global_shortcut::GlobalShortcutExt;
        if let Ok(old) = old_action_bar_sc.parse::<tauri_plugin_global_shortcut::Shortcut>() {
            let _ = app_handle.global_shortcut().unregister(old);
        }
        if let Err(e) = crate::action_bar_window::register_action_bar_shortcut(&app_handle, &cfg.action_bar_shortcut) {
            let _ = crate::action_bar_window::register_action_bar_shortcut(&app_handle, &old_action_bar_sc);
            return Err(format!("快捷键注册失败，配置未更改: {}", e));
        }
    }
```

同时在函数开头的解构里加 `old_action_bar_sc`：

```rust
    let (old_asr_sc, old_clipboard_sc, old_edit_global, old_polish_global, old_screenshot_sc, old_action_bar_sc, mut cfg) = {
        let g = rc.read();
        (g.asr_shortcut.clone(), g.clipboard_shortcut.clone(), g.edit_global_shortcut.clone(), g.polish_global_shortcut.clone(), g.screenshot_shortcut.clone(), g.action_bar_shortcut.clone(), g.clone())
    };
```

- [ ] **Step 5: 编译检查（会有 action_bar_window 未定义错误——预期，Task 4 创建）**

```bash
cargo check -p octopus-infra
```
Expected: infra 通过

- [ ] **Step 6: 提交**

```bash
git add crates/infra/src/config.rs crates/infra/src/db.sql crates/desktop/src/settings_commands.rs
git commit -m "feat(action-bar): 配置项注册（action_bar_shortcut + search_engine）"
```

---

### Task 4: action_bar_window.rs — 窗口创建 + 热键注册

**Files:**
- Create: `crates/desktop/src/action_bar_window.rs`
- Modify: `crates/desktop/src/main.rs`

**Interfaces:**
- Produces: `create_action_bar_window(app)` / `show_action_bar_window(app, x, y)` / `register_action_bar_shortcut(app, shortcut)` / `ACTION_BAR_WINDOW_LABEL`

- [ ] **Step 1: 创建 action_bar_window.rs**

```rust
//! AI 命令面板迷你浮窗——选中文本后热键触发，鼠标上方弹出。
//! 透明无边框 always_on_top，单例 show/hide toggle。

use tauri::{AppHandle, Manager, WebviewUrl, WebviewWindowBuilder};

pub const WINDOW_LABEL: &str = "action_bar_window";

/// 创建窗口（应用启动时调用，visible=false）。
pub fn create_action_bar_window(app: &AppHandle) {
    if app.get_webview_window(WINDOW_LABEL).is_some() {
        return;
    }
    let _ = WebviewWindowBuilder::new(
        app,
        WINDOW_LABEL,
        WebviewUrl::default(),
    )
    .title("")
    .inner_size(240.0, 50.0)
    .decorations(false)
    .always_on_top(true)
    .transparent(true)
    .skip_taskbar(true)
    .resizable(false)
    .shadow(false)
    .visible(false)
    .build();
}

/// 在指定坐标显示浮窗（鼠标上方）。
pub fn show_action_bar_window(app: &AppHandle, x: f64, y: f64) {
    if let Some(win) = app.get_webview_window(WINDOW_LABEL) {
        let _ = win.set_position(tauri::Position::Logical(
            tauri::LogicalPosition::new(x, y),
        ));
        let _ = win.show();
        let _ = win.set_focus();
    }
}

/// 隐藏浮窗。
pub fn hide_action_bar_window(app: &AppHandle) {
    if let Some(win) = app.get_webview_window(WINDOW_LABEL) {
        let _ = win.hide();
    }
}

/// 注册全局热键。与 register_clipboard_shortcut 范式一致。
pub fn register_action_bar_shortcut(
    app: &tauri::AppHandle,
    shortcut_str: &str,
) -> Result<(), String> {
    use tauri_plugin_global_shortcut::{GlobalShortcutExt, Shortcut, ShortcutState};
    let shortcut: Shortcut = shortcut_str
        .parse()
        .map_err(|e| format!("Failed to parse shortcut '{}': {}", shortcut_str, e))?;
    let app_handle = app.clone();
    app.global_shortcut()
        .on_shortcut(shortcut, move |_app, _scut, event| {
            if event.state() == ShortcutState::Pressed {
                crate::action_bar_commands::trigger_action_bar(&app_handle);
            }
        })
        .map_err(|e| format!("Failed to register action bar shortcut '{}': {}", shortcut_str, e))?;
    Ok(())
}
```

- [ ] **Step 2: main.rs 注册模块 + 窗口 + 热键**

在模块声明区加：

```rust
mod action_bar_window;
mod action_bar_commands;
```

在 `generate_handler!` 中加（末尾 `theme::get_theme_id` 后）：

```rust
            theme::get_theme_id,
            action_bar_commands::trigger_action_bar,
            action_bar_commands::run_ai_action,
            action_bar_commands::action_bar_paste_result,
            action_bar_commands::action_bar_open_url,
            action_bar_commands::action_bar_get_context,
```

在 setup 中（`create_clipboard_window` 后）加：

```rust
            crate::action_bar_window::create_action_bar_window(app.handle());
```

在热键注册区（`register_clipboard_shortcut` 后）加：

```rust
        if let Err(e) = action_bar_window::register_action_bar_shortcut(app.handle(), &config.action_bar_shortcut) {
            log::error!("Failed to register action bar shortcut: {}", e);
        }
```

- [ ] **Step 3: 编译检查**

```bash
cargo check -p octopus-desktop
```
Expected: 有 `action_bar_commands` 未定义错误——预期，Task 5 创建

- [ ] **Step 4: 提交**

```bash
git add crates/desktop/src/action_bar_window.rs crates/desktop/src/main.rs
git commit -m "feat(action-bar): 窗口创建 + 热键注册 + 模块/命令注册"
```

---

### Task 5: action_bar_commands.rs — 后端命令层

**Files:**
- Create: `crates/desktop/src/action_bar_commands.rs`

**Interfaces:**
- Produces: `trigger_action_bar` / `run_ai_action` / `action_bar_paste_result` / `action_bar_open_url` / `action_bar_get_context`

- [ ] **Step 1: 创建命令文件**

```rust
//! AI 命令面板后端命令——模拟 Cmd+C / LLM 调用 / 模拟 Cmd+V / 打开 URL。

use std::sync::Mutex;
use tauri::{AppHandle, Emitter, Manager};

use crate::action_bar_window::{hide_action_bar_window, show_action_bar_window, WINDOW_LABEL};
use crate::focus_tracker::FocusTracker;

/// 暂存选中文本 + 上下文（trigger 时写入，前端 mount 时 take）。
#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ActionBarContext {
    pub text: String,
    pub has_url: bool,
    pub url: String,
}

static PENDING_CONTEXT: Mutex<Option<ActionBarContext>> = Mutex::new(None);

/// 热键触发：模拟 Cmd+C → 读剪贴板 → 获取鼠标位置 → 显示浮窗。
#[tauri::command]
pub fn trigger_action_bar(app: AppHandle) {
    use std::process::Command;

    // 1. 记录触发前的剪贴板内容（判断 Cmd+C 是否成功）
    let clipboard_before = read_clipboard_text(&app);

    // 2. 模拟 Cmd+C
    let focus = FocusTracker;
    focus.simulate_copy();

    // 3. 等待 200ms 让系统完成复制
    std::thread::sleep(std::time::Duration::from_millis(200));

    // 4. 读剪贴板
    let clipboard_after = read_clipboard_text(&app);
    let text = match (&clipboard_before, &clipboard_after) {
        (Some(before), Some(after)) if before != after => after.clone(),
        (None, Some(after)) => after.clone(),
        _ => {
            log::warn!("[action-bar] Cmd+C didn't change clipboard — no selection?");
            return;
        }
    };

    if text.trim().is_empty() {
        log::warn!("[action-bar] Selected text is empty");
        return;
    }

    // 5. 前端 URL 检测（宽松——前端也有同逻辑，这里给前端省一次计算）
    // 实际检测在前端做更灵活（可用纯函数测试），这里只传 text。
    let ctx = ActionBarContext {
        text: text.clone(),
        has_url: false, // 前端自行检测
        url: String::new(),
    };
    *PENDING_CONTEXT.lock().unwrap() = Some(ctx);

    // 6. 获取鼠标位置（macOS 用 CGEvent）
    let (mx, my) = get_mouse_position();
    // 浮窗在鼠标上方弹出（y - 60 留出浮窗高度）
    let win_y = (my - 60.0).max(0.0);

    // 7. 显示浮窗
    show_action_bar_window(&app, mx, win_y);
}

/// 前端 mount 时拉取上下文。
#[tauri::command]
pub fn action_bar_get_context() -> Option<ActionBarContext> {
    PENDING_CONTEXT.lock().unwrap().take()
}

/// 执行 AI 动作（润色/摘要/解释/翻译）。
#[tauri::command]
pub async fn run_ai_action(action: String, text: String) -> Result<String, String> {
    let config = octopus_infra::config::load_config().map_err(|e| e.to_string())?;
    let llm_config = crate::config::llm_config(&config);

    // 构造 system prompt（临时切换）
    let (prompt, is_translate) = match action.as_str() {
        "polish" => ("请对以下文本进行润色，使其更加流畅、专业。保持原意不变。".to_string(), false),
        "summarize" => ("请用简洁的中文总结以下内容的要点，不超过 3 句话。".to_string(), false),
        "explain" => ("请用简洁的中文解释以下内容的含义。".to_string(), false),
        "translate" => {
            // CJK 检测：含 CJK 字符 → 翻译成英文；否则翻译成中文
            let has_cjk = text.chars().any(|c| {
                matches!(c as u32,
                    0x4e00..=0x9fff | 0x3040..=0x30ff | 0xac00..=0xd7af
                )
            });
            if has_cjk {
                ("Please translate the following text into English. Only output the translation.".to_string(), false)
            } else {
                ("请将以下文本翻译成中文。只输出翻译结果。".to_string(), false)
            }
        }
        _ => return Err(format!("未知动作: {}", action)),
    };

    // 临时切换 system prompt
    let old_prompt = octopus_llm::system_prompt();
    octopus_llm::set_system_prompt(&prompt);

    let result = octopus_llm::polish(None, &text, &llm_config);

    // 恢复原 system prompt
    octopus_llm::set_system_prompt(&old_prompt);

    result.map_err(|e| e.to_string())
}

/// 写结果到剪贴板 + 恢复焦点 + 模拟 Cmd+V + 隐藏浮窗。
#[tauri::command]
pub fn action_bar_paste_result(result: String, app: AppHandle) {
    // 1. 隐藏浮窗
    hide_action_bar_window(&app);

    // 2. 写剪贴板
    write_clipboard_text(&app, &result);

    // 3. 恢复焦点 + 模拟粘贴（同 paste_clipboard_item 模式）
    let focus = FocusTracker;
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(200));
        focus.restore_focus();
        std::thread::sleep(std::time::Duration::from_millis(100));
        focus.simulate_paste();
    });
}

/// 用系统浏览器打开 URL + 隐藏浮窗。
#[tauri::command]
pub fn action_bar_open_url(url: String, app: AppHandle) {
    hide_action_bar_window(&app);
    let _ = open::that(&url);
}

// ── 辅助函数 ──

fn read_clipboard_text(app: &AppHandle) -> Option<String> {
    use std::sync::Arc;
    let handle = app.state::<std::sync::Arc<octopus_clipboard::ClipboardHandle>>();
    handle.read_text().ok().flatten()
}

fn write_clipboard_text(app: &AppHandle, text: &str) {
    let handle = app.state::<std::sync::Arc<octopus_clipboard::ClipboardHandle>>();
    let _ = handle.write_text(text);
}

#[cfg(target_os = "macos")]
fn get_mouse_position() -> (f64, f64) {
    use core_graphics::event::CGEvent;
    use core_graphics::event_source::{CGEventSource, CGEventSourceStateID};
    let source = CGEventSource::new(CGEventSourceStateID::HIDSystemState).ok();
    let event = CGEvent::new(source.as_ref()).ok();
    if let Some(event) = event {
        let point = event.location();
        return (point.x, point.y);
    }
    (100.0, 100.0) // fallback
}

#[cfg(not(target_os = "macos"))]
fn get_mouse_position() -> (f64, f64) {
    (100.0, 100.0) // TODO: Windows/Linux
}
```

- [ ] **Step 2: 编译检查**

需要 `core-graphics` 依赖。检查 Cargo.toml：

```bash
grep "core-graphics" crates/desktop/Cargo.toml
```

如果没有，加到 `[target.'cfg(target_os = "macos")'.dependencies]`：

```toml
core-graphics = "0.24"
```

- [ ] **Step 3: 编译检查**

```bash
cargo check -p octopus-desktop
```
Expected: 无错误

- [ ] **Step 4: 提交**

```bash
git add crates/desktop/src/action_bar_commands.rs crates/desktop/Cargo.toml
git commit -m "feat(action-bar): 后端命令层（trigger/AI/paste/openURL）"
```

---

### Task 6: 前端浮窗组件

**Files:**
- Create: `crates/desktop/frontend/src/pages/ActionBar/index.tsx`
- Modify: `crates/desktop/frontend/src/App.tsx`

- [ ] **Step 1: App.tsx 加路由**

在 `switch (label)` 中加：

```tsx
          case "action_bar_window":
            return <ActionBar />;
```

import 区加：

```tsx
import ActionBar from "@/pages/ActionBar";
```

- [ ] **Step 2: 创建 ActionBar 组件**

完整组件（两级菜单 + 键盘导航 + loading + URL 检测）：

```tsx
// 见 spec §6 的组件结构——实际实现时从 spec 参照
// 包含：
// - mount → invoke("action_bar_get_context") 拿选中文本
// - 第一级菜单：AI / 翻译 / 搜索 / 网页（网页仅在 URL 检测命中时显示）
// - AI 子菜单：润色 / 摘要 / 解释
// - 键盘导航：←→↑↓ + Enter + Cmd+数字 + Esc 两级退出
// - loading 状态：AI 处理中转圈
// - 错误状态：失败提示 + "已复制到剪贴板"
// - 点击外部消失（listen window blur）
```

- [ ] **Step 3: 类型检查 + 测试**

```bash
cd crates/desktop/frontend && npx tsc --noEmit && npm test
```
Expected: 无错误，所有测试通过

- [ ] **Step 4: 提交**

```bash
git add crates/desktop/frontend/src/pages/ActionBar/index.tsx crates/desktop/frontend/src/App.tsx
git commit -m "feat(action-bar): 前端浮窗组件（两级菜单+键盘导航+loading）"
```

---

### Task 7: 设置页快捷键 + 搜索引擎配置

**Files:**
- Modify: `crates/desktop/frontend/src/pages/Settings/GeneralPanel.tsx`

- [ ] **Step 1: 快捷键卡片新增一行**

在"剪贴TAB切换"行后加：

```tsx
        <Row label="AI面板" effect="立即">
          <ShortcutButton shortcut={cfg.action_bar_shortcut as string} capturing={capturingKey === "action_bar_shortcut"} onClick={() => startShortcutCapture("action_bar_shortcut")} />
        </Row>
```

- [ ] **Step 2: 外观卡片后加搜索引擎下拉（或放快捷键卡片下方）**

```tsx
        <Row label="面板搜索" effect="立即" hint="AI面板搜索用的引擎">
          <select className={selectClass} value={(cfg.action_bar_search_engine as string) || "google"} onChange={(e) => setVal("action_bar_search_engine", e.target.value)}>
            <option value="google">Google</option>
            <option value="baidu">百度</option>
            <option value="bing">Bing</option>
          </select>
        </Row>
```

- [ ] **Step 3: 类型检查**

```bash
cd crates/desktop/frontend && npx tsc --noEmit
```
Expected: 无错误

- [ ] **Step 4: 提交**

```bash
git add crates/desktop/frontend/src/pages/Settings/GeneralPanel.tsx
git commit -m "feat(action-bar): 设置页快捷键 + 搜索引擎配置"
```

---

## 验收总清单

全部完成后逐项确认：

- [ ] 1. 选中文本 → 按热键 → 浮窗在鼠标上方弹出
- [ ] 2. 第一级菜单显示 AI/翻译/搜索/网页（网页仅 URL 命中时）
- [ ] 3. 点 AI → 展开子菜单润色/摘要/解释
- [ ] 4. 选 AI 动作 → loading → 结果原位替换选中（Run And Paste）
- [ ] 5. 替换失败 → 剪贴板已有结果（手动 Cmd+V）
- [ ] 6. 点翻译 → AI 翻译（CJK→英文，非 CJK→中文）→ 替换
- [ ] 7. 点搜索 → 浏览器打开搜索引擎
- [ ] 8. 点网页 → 浏览器打开 URL（自动补 scheme）
- [ ] 9. 键盘：←→移动 + ↑↓子菜单 + Enter 执行 + Cmd+数字 + Esc 两级退出
- [ ] 10. 点击外部 → 浮窗消失
- [ ] 11. 设置页可配置热键 + 搜索引擎
- [ ] 12. 主题切换 → 浮窗跟随主题

最终全量检查：
```bash
cd crates/desktop/frontend && npm test && npx tsc --noEmit
cargo test -p octopus-desktop
```
