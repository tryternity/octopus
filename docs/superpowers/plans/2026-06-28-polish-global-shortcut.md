# 全局立即润色快捷键（polish_global_shortcut）实施计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 新增全局快捷键 `polish_global_shortcut`（默认 `CmdOrCtrl+Shift+L`），任意应用聚焦时对当前识别结果立即润色（show 窗口不聚焦），复刻 `edit_global_shortcut` 模式。

**Architecture:** config 字段 + result_window handler（show+emit，不 set_focus）+ 前端 listen 复用 `polishNow`（从 polish-now 按钮抽出）+ settings 热重载 + 设置 UI 行。纯复刻 edit_global，零新机制。

**Tech Stack:** Rust（tauri-plugin-global-shortcut）/ TypeScript（React + Tauri event）/ SQLite app_config。

**关联 spec:** [2026-06-28-polish-global-shortcut-design.md](../specs/2026-06-28-polish-global-shortcut-design.md)

---

## File Structure

| 文件 | 责任 | 改动 |
|------|------|------|
| `crates/infra/src/config.rs` | AppConfig 字段定义 | +`polish_global_shortcut` 字段 + default fn + Default impl + 单测 |
| `crates/infra/src/db.sql` | app_config seed | +seed 行 |
| `crates/infra/src/db.rs` | DB load/save | load +分支 / save +字段（26→27）|
| `crates/desktop/src/result_window.rs` | 窗口管理 + 全局键 handler | +`trigger_global_polish` +`register_polish_global_shortcut` |
| `crates/desktop/src/main.rs` | setup 注册 | +注册调用 |
| `crates/desktop/src/settings_commands.rs` | set_config 热重载 + 校验 | apply_config_value +分支 / set_config +热重载块（old_polish_global）|
| `crates/desktop/frontend/src/pages/Result/index.tsx` | 结果窗前端 | 抽 `polishNow` + 按钮 onClick + listen useEffect |
| `crates/desktop/frontend/src/pages/Settings/GeneralPanel.tsx` | 设置页 | 快捷键卡片 +「立即润色」行 |
| `docs/architecture.md` | 架构文档 | 同步卡片清单 + handler 描述 |

---

## Task 1: 配置层 `polish_global_shortcut` 字段

**Files:**
- Modify: `crates/infra/src/config.rs`（字段 L153-154 区、default fn L227-229 区、Default impl L271 区、单测 L320 区）
- Modify: `crates/infra/src/db.sql`（seed L188 区）
- Modify: `crates/infra/src/db.rs`（load L324 区、save L369/393 区）

- [ ] **Step 1.1: config.rs 加字段定义**

在 `edit_global_shortcut` 字段定义之后（L154 `pub edit_global_shortcut: String,` 之后）插入：

```rust
    /// 全局立即润色快捷键（跨应用，show 结果窗不聚焦 + 触发 polish_now）。
    /// 默认 CmdOrCtrl+Shift+L。
    #[serde(default = "default_polish_global_shortcut")]
    pub polish_global_shortcut: String,
```

- [ ] **Step 1.2: config.rs 加 default 函数**

在 `default_edit_global_shortcut`（L227-229）之后插入：

```rust
fn default_polish_global_shortcut() -> String {
    "CmdOrCtrl+Shift+L".into()
}
```

- [ ] **Step 1.3: config.rs Default impl 加初始化**

在 Default impl 的 `edit_global_shortcut: default_edit_global_shortcut(),`（L271）之后插入：

```rust
            polish_global_shortcut: default_polish_global_shortcut(),
```

- [ ] **Step 1.4: config.rs 单测加断言**

在单测 `assert_eq!(cfg.edit_global_shortcut, "CmdOrCtrl+Shift+E");`（L320）之后插入：

```rust
        assert_eq!(cfg.polish_global_shortcut, "CmdOrCtrl+Shift+L");
```

- [ ] **Step 1.5: db.sql seed 加行**

在 `edit_global_shortcut` seed 行（L188）之后插入（注意对齐 + category 吃列 DEFAULT='setting'）：

```sql
    ('polish_global_shortcut',   'CmdOrCtrl+Shift+L',                    '全局立即润色快捷键（跨应用 show 结果窗不聚焦 + 触发 polish_now）'),
```

- [ ] **Step 1.6: db.rs load 加分支**

在 `load_app_config_at` 的 `"edit_global_shortcut" => cfg.edit_global_shortcut = value,`（L324）之后插入：

```rust
            "polish_global_shortcut" => cfg.polish_global_shortcut = value,
```

- [ ] **Step 1.7: db.rs save 加字段**

`save_app_config_at`：
- 数组长度 `let fields: [(&str, String); 26]` → `27`
- 在 `("edit_global_shortcut", cfg.edit_global_shortcut.clone()),`（L393）之后插入：

```rust
        ("polish_global_shortcut", cfg.polish_global_shortcut.clone()),
```

- [ ] **Step 1.8: 验证 config 编译 + 单测**

Run: `cargo test -p octopus-infra config::tests -- --nocapture`（或含默认值断言的测试名）
Expected: PASS，含 `polish_global_shortcut == "CmdOrCtrl+Shift+L"` 断言通过。

- [ ] **Step 1.9: Commit**

```bash
git add crates/infra/src/config.rs crates/infra/src/db.sql crates/infra/src/db.rs
git commit -m "feat(infra): polish_global_shortcut 配置字段 + db load/save（27 字段）"
```

---

## Task 2: 后端 handler + 注册

**Files:**
- Modify: `crates/desktop/src/result_window.rs`（L180 `register_edit_global_shortcut` 之后）
- Modify: `crates/desktop/src/main.rs`（L389 注册块之后）

- [ ] **Step 2.1: result_window.rs 加 trigger_global_polish + register**

在 `register_edit_global_shortcut` 函数（L162-180）之后插入。**关键区别：trigger 只 `show` 不 `set_focus`**（润色不需窗口接收键盘）：

```rust
/// 全局立即润色快捷键被按下：show 结果窗（不 set_focus，润色不需窗口聚焦接收键盘）
/// 并通知前端触发 polish_now。前端 polishNow 内部判空（无结果静默）+ polishLoading
/// 门控（幂等）。与 trigger_global_edit 的区别仅在此处不 set_focus。
pub fn trigger_global_polish(app: &tauri::AppHandle) {
    if let Some(window) = app.get_webview_window(WINDOW_LABEL) {
        let _ = window.show();
        let _ = window.emit("global-polish-trigger", ());
    }
}

/// 注册全局立即润色快捷键。与 register_edit_global_shortcut 的区别：handler 调
/// trigger_global_polish。set_config 热重载时复用此函数。
pub fn register_polish_global_shortcut(
    app: &tauri::AppHandle,
    shortcut_str: &str,
) -> Result<(), String> {
    use tauri_plugin_global_shortcut::{GlobalShortcutExt, Shortcut, ShortcutState};
    let shortcut: Shortcut = shortcut_str
        .parse()
        .map_err(|e| format!("Failed to parse shortcut '{}': {}", shortcut_str, e))?;
    let app_handle = app.clone();
    app.global_shortcut()
        .on_shortcut(shortcut, move |_ah, _scut, event| {
            if event.state() == ShortcutState::Pressed {
                trigger_global_polish(&app_handle);
            }
        })
        .map_err(|e| format!("Failed to register shortcut '{}': {}", shortcut_str, e))?;
    debug!("Registered global polish shortcut: {}", shortcut_str);
    Ok(())
}
```

- [ ] **Step 2.2: main.rs setup 加注册**

在 `register_edit_global_shortcut` 注册块（L386-389）之后插入：

```rust
            // 6.2 Register global polish shortcut（跨应用 show 结果窗 + 立即润色）
            if let Err(e) = result_window::register_polish_global_shortcut(app.handle(), &config.polish_global_shortcut) {
                log::error!("Failed to register global polish shortcut: {}", e);
            }
```

- [ ] **Step 2.3: 验证 desktop 编译**

Run: `cargo check -p octopus-desktop`
Expected: 0 error（可能有 pre-existing dead_code warning，无关）。

- [ ] **Step 2.4: Commit**

```bash
git add crates/desktop/src/result_window.rs crates/desktop/src/main.rs
git commit -m "feat(desktop): trigger_global_polish + register（show 不聚焦）+ main 注册"
```

---

## Task 3: 热重载 + 校验

**Files:**
- Modify: `crates/desktop/src/settings_commands.rs`（L87-89 old 拆分、L107-118 热重载块、L245-247 apply 分支）

- [ ] **Step 3.1: set_config old 拆分加 old_polish_global**

L87-89：
```rust
    let (old_asr_sc, old_clipboard_sc, old_edit_global, mut cfg) = {
        let g = rc.read().unwrap();
        (g.asr_shortcut.clone(), g.clipboard_shortcut.clone(), g.edit_global_shortcut.clone(), g.clone())
    };
```
改为（加 `old_polish_global`）：
```rust
    let (old_asr_sc, old_clipboard_sc, old_edit_global, old_polish_global, mut cfg) = {
        let g = rc.read().unwrap();
        (g.asr_shortcut.clone(), g.clipboard_shortcut.clone(), g.edit_global_shortcut.clone(), g.polish_global_shortcut.clone(), g.clone())
    };
```

- [ ] **Step 3.2: set_config 加 polish_global 热重载块**

在 `edit_global_shortcut` 热重载块（L107-118）之后、`clipboard_shortcut` 块（L120）之前插入：

```rust
    // polish_global_shortcut 热重载：注册成功后才持久化（同 asr/edit_global 审查 Issue 3）。
    if key == "polish_global_shortcut" && cfg.polish_global_shortcut != old_polish_global {
        use tauri_plugin_global_shortcut::GlobalShortcutExt;
        if let Ok(old) = old_polish_global.parse::<tauri_plugin_global_shortcut::Shortcut>() {
            let _ = app_handle.global_shortcut().unregister(old);
        }
        if let Err(e) = crate::result_window::register_polish_global_shortcut(&app_handle, &cfg.polish_global_shortcut) {
            let _ = crate::result_window::register_polish_global_shortcut(&app_handle, &old_polish_global);
            return Err(format!("快捷键注册失败，配置未更改: {}", e));
        }
    }
```

- [ ] **Step 3.3: apply_config_value 加 polish_global 分支**

在 `"edit_global_shortcut" =>` 分支（L245-247）之后插入：

```rust
        "polish_global_shortcut" => {
            cfg.polish_global_shortcut = value.as_str().ok_or("polish_global_shortcut 需要字符串")?.to_string();
        }
```

- [ ] **Step 3.4: 验证 desktop 编译 + 单测**

Run: `cargo test -p octopus-desktop settings_commands`
Expected: PASS（既有 apply_config_value 单测不受影响；新分支字符串校验同 edit_global）。

- [ ] **Step 3.5: Commit**

```bash
git add crates/desktop/src/settings_commands.rs
git commit -m "feat(desktop): polish_global_shortcut 热重载 + apply_config_value 分支"
```

---

## Task 4: 前端

**Files:**
- Modify: `crates/desktop/frontend/src/pages/Result/index.tsx`（polish-now 按钮 onClick L348-352、新增 polishNow useCallback + listen useEffect）
- Modify: `crates/desktop/frontend/src/pages/Settings/GeneralPanel.tsx`（快捷键卡片 L154-156 语音编辑行后）

- [ ] **Step 4.1: Result/index.tsx 抽 polishNow + 按钮 onClick 复用**

把 polish-now 按钮（L348-352）内联的 onClick 逻辑抽成 `polishNow` useCallback（加 polishLoading 门控 + trim 判空）。在 `toggleEdit` 声明区附近（global-edit-toggle useEffect 之前）加：

```ts
  // 立即润色：工具栏按钮 + 全局 polish_global_shortcut 共用。
  // polishLoading 门控（幂等，与按钮 disabled 一致）+ 空文本判空（无结果静默）。
  const polishNow = useCallback(async () => {
    if (polishLoading) return;
    if (!displayedRef.current.trim()) return;
    setPolishLoading(true);
    try { await invoke("polish_now"); showToast("润色中…"); }
    catch (e) { setPolishLoading(false); showToast("润色失败：" + e); }
  }, [polishLoading, showToast]);
```

polish-now 按钮 onClick 改为 `onClick: polishNow`（去掉内联 async）。

- [ ] **Step 4.2: Result/index.tsx 加 global-polish-trigger listen**

在 `global-edit-toggle` useEffect（L254-262）之后加独立 useEffect（规避 TDZ，同 global-edit-toggle）：

```ts
  // 全局立即润色快捷键（polish_global_shortcut）：后端 show 结果窗（不聚焦）后 emit 此事件，
  // 复用 polishNow——空文本静默、进行中幂等，与工具栏「立即润色」按钮同语义。
  // 独立 useEffect（同 global-edit-toggle）：polishNow 在此声明，前置使用触发 TS2448。
  useEffect(() => {
    let unlisten: UnlistenFn | undefined;
    let cancelled = false;
    listen("global-polish-trigger", () => polishNow()).then((fn) => {
      if (cancelled) fn();
      else unlisten = fn;
    });
    return () => { cancelled = true; unlisten?.(); };
  }, [polishNow]);
```

- [ ] **Step 4.3: GeneralPanel.tsx 加「立即润色」行**

在「语音编辑」行（L154-156）之后插入（快捷键卡片内，`</Card>` 之前）：

```tsx
        <Row label="立即润色" effect="立即" hint="对当前识别结果立即润色">
          <ShortcutButton shortcut={cfg.polish_global_shortcut as string} capturing={capturingKey === "polish_global_shortcut"} onClick={() => startShortcutCapture("polish_global_shortcut")} />
        </Row>
```

- [ ] **Step 4.4: 验证前端 build**

Run: `npm --prefix crates/desktop/frontend run build`
Expected: tsc + vite 通过，新 bundle 生成（含 polishNow + listen + 立即润色行）。

- [ ] **Step 4.5: Commit**

```bash
git add crates/desktop/frontend/src/pages/Result/index.tsx crates/desktop/frontend/src/pages/Settings/GeneralPanel.tsx crates/desktop/dist
git commit -m "feat(desktop): 前端 polishNow 抽函数 + global-polish-trigger listen + 设置页立即润色行"
```

---

## Task 5: 文档同步 + 全量验证

**Files:**
- Modify: `docs/architecture.md`（L152 result_window 描述、L298 设置卡片清单 + handler 描述）
- Modify: 本 plan（checkbox 全勾）

- [ ] **Step 5.1: architecture.md result_window 描述加全局润色入口**

L152 `result_window` 行的工具栏/编辑入口描述里，在全局 `edit_global_shortcut` 之后补全局润色入口：

> + 全局 `polish_global_shortcut` 默认 CmdOrCtrl+Shift+L（任意应用聚焦时 show 结果窗**不聚焦** + 触发 `polish_now` 立即润色，复用前端 `polishNow`：空文本静默、polishLoading 幂等）

- [ ] **Step 5.2: architecture.md 设置卡片清单 + handler 描述**

L298：
- 快捷键卡片清单「语音识别/语音编辑/剪贴板浮窗」→「语音识别/语音编辑/立即润色/剪贴板浮窗」
- `set_config` 热重载快捷键列表 `asr_shortcut / clipboard_shortcut / edit_global_shortcut` → 加 `/ polish_global_shortcut`；handler 描述补 `register_polish_global_shortcut`（handler 调 `trigger_global_polish`：show 结果窗不聚焦 + emit `global-polish-trigger` → 前端 `polishNow`）；save 字段 `26 字段` → `27 字段`。

- [ ] **Step 5.3: 全量编译 + 测试**

Run: `cargo check -p octopus-desktop -p octopus-infra && cargo test -p octopus-infra -p octopus-desktop`
Expected: 0 error，单测全绿。

- [ ] **Step 5.4: 前端最终 build**

Run: `npm --prefix crates/desktop/frontend run build`
Expected: 通过。

- [ ] **Step 5.5: 本 plan checkbox 全勾 + Commit 文档**

```bash
git add docs/architecture.md docs/superpowers/plans/2026-06-28-polish-global-shortcut.md
git commit -m "docs: polish_global_shortcut 同步 architecture + plan checkbox"
```

---

## 验证清单（e2e，待用户桌面环境确认）

1. 按默认 `CmdOrCtrl+Shift+L`：结果窗 show（不抢焦点）+ 当前识别结果立即润色（toast「润色中…」→ 润色文本）。
2. 无识别结果时按：结果窗 show（透明）但不润色（前端判空）。
3. 润色进行中再按：幂等忽略（polishLoading 门控）。
4. 结果窗当前隐藏时按：show 后润色，`update-result` 显示润色文本。
5. 设置 → 快捷键 → 立即润色：键盘捕获改键，热重载即时生效 + 设置页显示新值（DB 持久化）；冲突键报错恢复。
6. 重启应用：配置持久化，全局润色键仍生效（验证 DB 存取）。
7. 不抢焦点验证：在别的工作应用输入时按润色键，当前应用键盘焦点不丢失。

## 不改动

- 工具栏「立即润色」按钮功能（仅 onClick 改 polishNow，行为零差异）。
- `polish_now` 后端命令、`Command::PolishNow`、coordinator 润色逻辑。
- `polish_mode`（自动润色）独立不受影响。
