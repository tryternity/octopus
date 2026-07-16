# Run And Paste 全局快捷键 + Silent 执行设计

> 2026-07-16 · 借鉴 Wox 的 Query Hotkey + IsSilentExecution 机制，为 auto_paste 菜单项提供全局快捷键入口

## 1. 设计目标

用户在任何应用里选中文本，直接按**全局快捷键**触发动作（翻译/AI/脚本），结果**自动粘贴回光标位置**——全程不弹出 ActionBar 浮窗，不需要手动选择菜单项。

核心体验：**选中 → 热键 → 自动翻译替换**，无 UI 干扰。

## 2. 与现有机制的关系

| 现有机制 | 角色 |
|---------|------|
| `action_bar_items.auto_paste` | 标记一个菜单项是 Run And Paste 模式（已有） |
| `action_bar_items.shortcut` | 浮窗内局部快捷键（Cmd+字母，已有） |
| **`action_bar_items.global_shortcut`** | **全局快捷键（新增）——silent 执行 + 粘贴的入口** |
| `detect_selection` | 读选中文本（复用，含 Sublime 插件分支） |
| `execute_action_bar_inner` | 执行动作（翻译/AI/脚本，复用） |
| `paste::paste` | 写剪贴板 + 模拟 ⌘V + 恢复原剪贴板（复用） |

## 3. 数据模型

### 3.1 DB v34→v35

```sql
ALTER TABLE action_bar_items ADD COLUMN global_shortcut TEXT NOT NULL DEFAULT '';
```

- 存储格式：Tauri shortcut 字符串，如 `"CmdOrCtrl+Shift+T"`
- 空字符串 = 无全局快捷键
- 仅 `auto_paste=1 AND is_enabled=1` 的项参与全局快捷键注册

### 3.2 设置页 UI

菜单项编辑器，当 `auto_paste=true` 时显示"全局快捷键"输入框（按键录制组件）：
- 字段：`globalShortcut`
- 创建/编辑时通过 `set_global_shortcut(id, global_shortcut)` 命令更新
- 快捷键冲突检测：录入时检查是否与已有全局快捷键（ActionBar 热键 / 剪贴板热键 / 其他菜单项热键）冲突

## 4. 全局快捷键注册

### 4.1 注册时机

- **应用启动**：`main.rs` setup 闭包内，`register_action_hotkeys(app)`
- **设置变更后**：设置页保存菜单项后，重新注册（先全部注销再重注册，或增量更新）

### 4.2 注册逻辑

```rust
pub fn register_action_hotkeys(app: &AppHandle) -> Result<(), String> {
    // 1. 先注销所有已注册的 action hotkey（用 label 前缀 "action_hotkey_" 追踪）
    // 2. 查 DB：SELECT * FROM action_bar_items WHERE global_shortcut != '' AND auto_paste = 1 AND is_enabled = 1
    // 3. 对每个项注册 tauri_plugin_global_shortcut
    //    label = format!("action_hotkey_{}", item.id)
    //    回调 → spawn 线程执行 silent_run_and_paste(item_id, app)
}
```

### 4.3 快捷键冲突处理

注册前 probe 检测：`tauri_plugin_global_shortcut` 的 `register` 如果快捷键已被系统或其他应用占用会返回 Err。逐个注册时失败的跳过 + log warn（不阻断其他快捷键注册）。

## 5. Silent 执行链路

### 5.1 完整触发流程

```
全局快捷键按下（主线程回调）
  → spawn worker 线程（不阻塞热键回调）
  → ① 同步捕获源窗口 PID
  → ② detect_selection（读选中文本）
  → ③ 无选中 → overlay 显示"请先选中文本" 2s → 结束
  → ④ 有选中：
       隐藏 ActionBar 浮窗（如可见）
       overlay 显示"正在执行 {动作名}..."
       execute_action_bar_inner(item_id, selected_text)
       成功 → 隐藏 overlay
              ActivateWindowByPid(源窗口 PID)
              sleep 150ms（焦点稳定）
              paste（写剪贴板 + ⌘V + 恢复原剪贴板）
       失败 → overlay 显示错误 3s → 隐藏 overlay
```

### 5.2 源窗口 PID 捕获（①）

在 worker 线程最开始（overlay 还没显示、ActionBar 还没隐藏）同步读：

**macOS**：
```rust
use objc2_app_kit::NSWorkspace;
let pid = NSWorkspace::sharedWorkspace()
    .frontmostApplication()
    .map(|app| app.processIdentifier());
```

此时 frontmost 是源应用（全局热键不改 frontmost）。PID 存入 worker 局部变量，后续 ActivateWindowByPid 用。

**Windows/Linux**：暂不支持（detect_selection 本身只有 macOS 完整实现）。

### 5.3 无选中处理（③）

不弹任何窗口，只显示 overlay 窗口的 toast 模式：
- overlay 窗口 show + emit `overlay://toast { message: "请先选中文本", type: "warn", duration: 2000 }`
- 前端显示黄色 toast，2s 后自动 emit hide
- 不获取键盘焦点

### 5.4 执行 + 粘贴（④）

**execute_action_bar_inner**：复用现有逻辑。但 silent 模式下不弹 CompactEditor——`auto_paste=true` 时本就走 `action_bar_run_and_paste` 路径（L1332/1354/1421），只需在 `action_bar_run_and_paste` 里加**焦点恢复 + 延迟 paste**。

**action_bar_run_and_paste 改动**：
```rust
pub fn action_bar_run_and_paste(result: String, app: AppHandle, source_pid: Option<i32>) {
    // 隐藏 overlay
    hide_overlay_window(&app);

    // 激活源窗口（如有 PID）
    if let Some(pid) = source_pid {
        #[cfg(target_os = "macos")]
        { crate::activation::activate_window_by_pid(pid); }
    }

    // 写剪贴板 + 延迟 paste
    write_clipboard_text(&app, &result);
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(150)); // 等焦点稳定
        let config = load_config().unwrap_or_default();
        let handle = app.state::<Arc<ClipboardHandle>>();
        if let Err(e) = crate::paste::paste(&result, &handle, &config) {
            log::warn!("[run-and-paste] paste 失败: {}", e);
            // paste 失败 → overlay 显示错误
            show_overlay_error(&app, &e.to_string());
        }
    });
}
```

**注意**：`paste::paste` 内部已有剪贴板备份/恢复（`backup_clipboard` + `restore_clipboard`），`write_to_clipboard=false` 时粘贴后恢复原内容。Run And Paste 应传 `write_to_clipboard=false`（用户配置的语义）——结果粘贴到光标但不留在剪贴板。

但当前 `action_bar_run_and_paste` 先 `write_clipboard_text` 再调 `paste::paste`——双写。修正：不预写剪贴板，让 `paste::paste` 统一处理（它内部会写）。

## 6. Overlay 窗口

### 6.1 窗口属性

```rust
WebviewWindowBuilder::new(app, "overlay_window", WebviewUrl::default())
    .title("")
    .inner_size(300.0, 48.0)
    .decorations(false)
    .always_on_top(true)
    .transparent(true)
    .skip_taskbar(true)
    .resizable(false)
    .shadow(false)
    .visible(false)
    .build();
```

### 6.2 位置

鼠标附近（与 ActionBar 鼠标位置弹出一致），用 `get_mouse_position` + 碰撞检测。

### 6.3 前端状态

三种模式，由 `overlay://show` 事件的 payload 决定：

```ts
type OverlayPayload = {
  mode: "loading" | "toast";
  message: string;
  type?: "warn" | "error";  // toast 模式的颜色
  duration?: number;         // toast 模式的自动关闭 ms
  actionName?: string;       // loading 模式显示的动作名
};
```

- **loading 模式**：spinner + "正在执行 {actionName}..."，等待 `overlay://hide`
- **toast 模式**：图标 + {message}，{duration} ms 后自动 `overlay://hide`

### 6.4 焦点策略

overlay 窗口**不调 `set_focus`**——避免抢源应用键盘焦点。macOS 上 `show()` 即可见但不成为 key window。用 `before_floating_window_show` / `after_floating_window_hide` 协调 FLOAT_DEPTH（与其他浮窗一致）。

## 7. ActivateWindowByPid

### 7.1 macOS 实现（AX API）

```rust
#[cfg(target_os = "macos")]
pub fn activate_window_by_pid(pid: i32) -> bool {
    use objc2_app_kit::{NSWorkspace, NSRunningApplication};
    // 方案 A（优先）：NSRunningApplication.setActivationPolicy + activate
    let apps = NSWorkspace::sharedWorkspace().runningApplications();
    for app in apps.iter() {
        if app.processIdentifier() == pid {
            app.activateWithOptions(
                NSApplicationActivationOptions::NSApplicationActivateAllWindows
            );
            return true;
        }
    }
    false
}
```

比 osascript 更可靠——直接用 NSRunningApplication API 激活指定 PID 的进程。

### 7.2 Windows/Linux

暂不支持（`#[cfg(target_os = "macos")]`）。

## 8. 设置页改动

### 8.1 ActionBarPanel 菜单编辑器

当 `type` 为 `ai`/`script` 且 `autoPaste=true` 时，显示"全局快捷键"字段：

```tsx
{(type === "ai" || type === "script") && form.autoPaste && (
  <FormField label="全局快捷键（silent 执行 + 粘贴）">
    <ShortcutRecorder
      value={form.globalShortcut ?? ""}
      onChange={(v) => onChange({ ...form, globalShortcut: v })}
      placeholder="按组合键录入（留空 = 不设全局快捷键）"
    />
  </FormField>
)}
```

### 8.2 快捷键录入组件

复用已有的快捷键录入 UI（设置页已有 ActionBar/clipboard/result 等快捷键录入组件）。录入时校验冲突。

### 8.3 DB 命令

新增 Tauri 命令：
- `set_global_shortcut(id: i64, global_shortcut: String)` → 更新单个菜单项的 global_shortcut
- `list_action_hotkeys()` → 返回所有已注册的全局快捷键（设置页冲突检测用）

## 9. 不变量

1. `global_shortcut` 仅对 `auto_paste=1 AND is_enabled=1` 的项生效
2. silent 模式不显示 ActionBar 浮窗（仅 overlay）
3. overlay 窗口不获取键盘焦点
4. 粘贴前必须 ActivateWindowByPid（确保焦点在源应用）
5. 粘贴后恢复原剪贴板（`write_to_clipboard=false` 语义）
6. 全局快捷键注册失败（系统占用）不阻断其他快捷键注册

## 10. 降级

- 无选中 → overlay toast 提示，不执行动作
- `detect_selection` 失败 → overlay toast 提示
- 动作执行失败（LLM 超时 / 脚本报错） → overlay toast 显示错误
- `activate_window_by_pid` 失败 → log warn，仍尝试 paste（可能粘到错误窗口）
- `paste` 失败 → overlay toast 显示错误
- 全局快捷键被系统占用 → 跳过该项，log warn

## 11. 与现有 auto_paste 路径的关系

现有 auto_paste 有两条触发路径：
1. **ActionBar 浮窗内**（已有）：弹出浮窗 → 搜索/选中菜单项 → 执行 → `action_bar_run_and_paste`。这条路径**不经过全局快捷键**，结果粘贴逻辑也走 `paste::paste`。
2. **全局快捷键 silent**（新增）：全局热键 → silent 执行 → `action_bar_run_and_paste`。

两条路径共用 `action_bar_run_and_paste`，但 silent 路径额外传入 `source_pid`（用于 ActivateWindowByPid），浮窗路径不传（浮窗 hide 后系统自动还焦）。

`action_bar_run_and_paste` 签名改为：
```rust
pub fn action_bar_run_and_paste(result: String, app: AppHandle, source_pid: Option<i32>)
```
浮窗路径传 `None`，silent 路径传 `Some(pid)`。

## 12. 性能预算

| 操作 | 预算 |
|------|------|
| 热键 → worker 线程启动 | <10ms |
| 源窗口 PID 捕获 | <5ms |
| detect_selection | ~200-500ms（含 Cmd+C 模拟） |
| overlay 显示 | <16ms（窗口已预创建，只 show + emit） |
| 动作执行（LLM） | 1-10s（取决于模型） |
| ActivateWindowByPid + 焦点稳定 | ~150ms |
| paste（写剪贴板 + ⌘V + 恢复） | ~300ms |

总预算：选中 → 粘贴 ≈ 2-12s（主要是 LLM 延迟，overlay 全程可见）。
