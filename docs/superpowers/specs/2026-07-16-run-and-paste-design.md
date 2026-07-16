# Quick Execute 全局快捷键设计

> 2026-07-16 · 全局快捷键跳过 ActionBar 浮窗直接执行菜单项，结果展示在 CompactEditor
>
> **实现完成** — auto_paste 已清理，由 global_shortcut 字段决定是否启用快速执行
>
> **2026-07-17 修订** — 修两个 bug：①注册清理从「按 DB 当前值逐个 unregister」改为 `unregister_all()` 全量清空，避免删除/清空快捷键后旧 handler 残留；②去掉"无选中 → fallback ActionBar 浮窗"分支，菜单项热键语义严格限定为「对选中文本执行动作」，无选中静默失败，不再劫持系统快捷键（详见 §3.3、§4、§7、§8）

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
    // 1. 先 unregister_all() 全量清空所有已注册的菜单项快捷键
    // 2. 查 DB：WHERE global_shortcut != '' AND is_enabled = 1
    // 3. 逐个 register + on_shortcut 回调
    //    回调 → spawn worker → quick_execute(item_id, app)
}
```

### 3.3 必须全量清空（2026-07-17 修订）

⚠️ **注销必须用 `unregister_all()` 全量清空**，不能只按 DB 当前值逐个 `unregister`。

旧实现遍历 `list_action_hotkeys()` 结果（已过滤 `global_shortcut != ''`）逐个注销，对**删除 / 清空**场景失效：

- 用户把菜单项快捷键从 `CmdOrCtrl+Shift+G` 清空 → DB 变 `''` → 查询结果集不含此项 → unregister 循环跳过 → 旧 handler 永久残留在进程内（直到下次重启）
- 残留 handler 仍会触发，不仅菜单项热键"删不掉"，还会劫持系统快捷键（Finder 的 `Cmd+Shift+G`「前往文件夹」被吞）

`unregister_all()` 是全量重建语义，对 action_bar / clipboard / asr / edit / polish / screenshot 各自独立注册的快捷键**无副作用**——它们走各自的 `register_*_shortcut` 路径，由各自调用方负责生命周期。但需注意：**`register_action_hotkeys` 调用时机**必须是「重建菜单项热键集合」而非「增量更新」，目前 `set_global_shortcut` 保存后全量重调一次，满足该约束。

## 4. Quick Execute 链路

```
全局快捷键按下（主线程回调）
  → spawn worker 线程
  → baseline 隔离（保存/恢复 CHANGE_COUNT_BASELINE）
  → detect_selection（读选中文本）
  → 非文本选中（None/File/Folder）→ log info + return（静默失败）
  → 文本选中：
       隐藏 ActionBar（如可见）
       execute_action_bar_inner(item_id, text, is_silent=false)
       → 结果展示在 CompactEditor（与 ActionBar 路径一致）
```

> **2026-07-17 修订**：原先"无选中 → fallback 到 ActionBar 浮窗"分支已删除。
> 菜单项热键的语义是**对这段文本执行动作**，没有选中文本就不该继续。
> fallback 弹浮窗会让用户在 Finder / 桌面等场景下按下菜单项热键时，
> 被静默切换到完全不同的交互（搜索浮窗），同时**劫持系统快捷键**
> （如 Finder `Cmd+Shift+G`「前往文件夹」被 octopus 吞）。
> 静默失败留给用户主动用 `action_bar_shortcut`（如 `Alt+A`）唤出浮窗。

### 4.1 与 ActionBar 路径的关系

| 维度 | ActionBar 路径 | Quick Execute 路径 |
|------|---------------|-------------------|
| 入口 | 全局热键弹出浮窗 → 手动选菜单 | 全局热键直接执行 |
| detect_selection | trigger_action_bar worker | action_hotkey worker（baseline 隔离） |
| 执行 | execute_action_bar_inner | execute_action_bar_inner（完全相同） |
| 结果 | CompactEditor | CompactEditor（完全相同） |
| FLOAT_DEPTH | 有（浮窗 show/hide） | 无（不弹浮窗） |
| 无选中行为 | 弹出浮窗（搜索模式） | **静默失败**（2026-07-17 修订） |

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
4. **非文本选中（None/File/Folder）时静默失败**——不 fallback 弹浮窗（2026-07-17 修订，详见 §4）
5. 全局快捷键注册失败（系统占用）不阻断其他快捷键
6. **菜单项热键集合的全量一致性**——`register_action_hotkeys` 调用后，进程内注册集合与当前 DB 中 `global_shortcut != '' AND is_enabled = 1` 的项严格相等（通过 `unregister_all()` + 全量重注册保证，2026-07-17 修订，详见 §3.3）

## 8. 降级

- 非文本选中 → log info + return（静默失败，不弹浮窗）
- detect_selection 失败（异常） → log warn + return（不弹浮窗）
- 动作执行失败 → execute_action_bar_inner 的错误处理（log warn，不阻断）
- 全局快捷键被系统占用 → 跳过该项，log warn
