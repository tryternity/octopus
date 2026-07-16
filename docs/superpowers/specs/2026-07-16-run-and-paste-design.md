# Quick Execute 全局快捷键设计

> 2026-07-16 · 全局快捷键跳过 ActionBar 浮窗直接执行菜单项，结果展示在 CompactEditor
>
> **实现完成** — auto_paste 已清理，由 global_shortcut 字段决定是否启用快速执行

## 1. 设计目标

用户在任何应用里选中文本，直接按**全局快捷键**触发动作（翻译/AI/脚本/复制等），跳过"弹出 ActionBar 浮窗 + 手动选菜单"两步。结果展示在 CompactEditor（与 ActionBar 路径完全一致）。

核心体验：**选中 → 热键 → CompactEditor 展示结果**，省去浮窗交互。

与原"Run And Paste"（粘贴替换）的区别：不直接粘贴替换原文本（浏览器/PDF 等不支持替换的场景不友好），改为展示在 CompactEditor 让用户自取。

## 2. 数据模型

### 2.1 DB v36：action_bar_items 加 global_shortcut 列

```sql
ALTER TABLE action_bar_items ADD COLUMN global_shortcut TEXT NOT NULL DEFAULT '';
```

- 存储格式：Tauri shortcut 字符串，如 `"CmdOrCtrl+Shift+T"`
- 空字符串 = 无全局快捷键
- 有值 = 该菜单项可全局快捷键触发（Quick Execute）
- **不再需要 auto_paste**（已从代码层清理，DB 列保留兼容旧库）

### 2.2 所有叶子命令（非 submenu）均可设

设置页菜单编辑器对所有 `type !== "submenu"` 的项显示"全局快捷键"字段。用 `ShortcutButton` 组件录入（参照系统快捷键设置 Tab），支持 Esc 退出录制、Backspace 清除。

## 3. 快捷键注册

### 3.1 注册时机

- **应用启动**：`main.rs` setup 闭包调 `register_action_hotkeys(app)`
- **设置变更后**：`set_global_shortcut` 命令保存后触发重注册

### 3.2 注册逻辑

```rust
pub fn register_action_hotkeys(app: &AppHandle) {
    // 1. 先注销所有已注册的快捷键（用 DB 里的值精确注销）
    // 2. 查 DB：WHERE global_shortcut != '' AND is_enabled = 1
    // 3. 逐个 register + on_shortcut 回调
    //    回调 → spawn worker → quick_execute(item_id, app)
}
```

## 4. Quick Execute 链路

```
全局快捷键按下（主线程回调）
  → spawn worker 线程
  → baseline 隔离（保存/恢复 CHANGE_COUNT_BASELINE）
  → detect_selection（读选中文本）
  → 无选中 → fallback 到 ActionBar 浮窗（trigger_action_bar）
  → 有选中：
       隐藏 ActionBar（如可见）
       execute_action_bar_inner(item_id, text, is_silent=false)
       → 结果展示在 CompactEditor（与 ActionBar 路径一致）
```

### 4.1 与 ActionBar 路径的关系

| 维度 | ActionBar 路径 | Quick Execute 路径 |
|------|---------------|-------------------|
| 入口 | 全局热键弹出浮窗 → 手动选菜单 | 全局热键直接执行 |
| detect_selection | trigger_action_bar worker | action_hotkey worker（baseline 隔离） |
| 执行 | execute_action_bar_inner | execute_action_bar_inner（完全相同） |
| 结果 | CompactEditor | CompactEditor（完全相同） |
| FLOAT_DEPTH | 有（浮窗 show/hide） | 无（不弹浮窗） |

### 4.2 baseline 隔离

`detect_selection` 内部写 `CHANGE_COUNT_BASELINE`（全局静态量）。Quick Execute 的 detect 与 ActionBar 的 detect 共享此 baseline 会互相污染。解法：detect 前后 save/restore baseline。

## 5. LLM 超时可配

`chat_text_with_prompt` 新增 `timeout_secs: Option<u64>` 参数：
- `None`：全局默认 120s
- `Some(N)`：per-request 超时（通过 `reqwest::blocking::RequestBuilder::timeout()` 覆盖）

## 6. 设置页 UI

- **ShortcutButton 组件**（`components/ShortcutButton.tsx`）：从 GeneralPanel 抽出的共享组件，点击进入录制模式
- **录入流程**：点击按钮 → 按组合键 → `check_shortcut` 冲突检测 → 保存
- **清除**：Backspace/Delete 清空快捷键
- **保存**：`invoke("set_global_shortcut", { id, globalShortcut })` → 触发重注册

## 7. 不变量

1. `global_shortcut` 仅对 `is_enabled=1` 的项生效
2. Quick Execute 不弹 ActionBar 浮窗（仅 CompactEditor）
3. Quick Execute 的 detect 不污染 ActionBar 的 CHANGE_COUNT_BASELINE
4. 无选中时 fallback 到 ActionBar 浮窗（而非报错）
5. 全局快捷键注册失败（系统占用）不阻断其他快捷键

## 8. 降级

- 无选中 → fallback 到 ActionBar 浮窗
- detect_selection 失败 → fallback 到 ActionBar 浮窗
- 动作执行失败 → ActionBar 浮窗（execute_action_bar_inner 的错误处理）
- 全局快捷键被系统占用 → 跳过该项，log warn
