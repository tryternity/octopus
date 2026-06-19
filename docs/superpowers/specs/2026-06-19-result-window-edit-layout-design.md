# 结果窗编辑布局调整设计（编辑态文字不动 + 保存按钮移 toolbar）

> Date: 2026-06-19
> 状态：设计阶段（brainstorming 产出，待实现）

## 1. 背景

editable-result 功能已实现（`edit_shortcut` 进入编辑 + ✏️ 按钮进入；快捷键后已统一为 **toggle**——进入/保存同键，见 §4.5）。当前编辑态有两个布局体验问题：

1. **进入编辑时文字水平重排**：编辑态 CSS 给 `#result-text` 加 `padding-right: 90px`（给浮在文本区右上的「完成编辑」按钮让位），文字内容区从 520px 变 430px → 换行位置改变 → 视觉上文字"动了"。
2. **保存按钮位置**：「完成编辑」按钮浮在文本区右上，用户希望移到 toolbar（顶栏工具区）。

## 2. 目标

- 进入/退出编辑时，**文字水平位置不变**（不重排）——主要诉求。
- 「保存编辑」入口移到 toolbar。
- ✏️ 按钮复用 toggle（进入 ↔ 保存），编辑态图标切为 💾（`save.svg`）。
- 垂直跳动（Cmd+E 进入时 toolbar 出现）可接受（次要）。

## 3. 现状（关键事实）

文件：`crates/desktop/dist/result/index.html`（单文件前端，无构建）。

- 编辑态 CSS（L200-209）：`#container.editing #text-wrapper` 淡蓝边框；`#container.editing #result-text { padding: 1px 90px 7px 13px }`（**重排根因**）。
- `#edit-done` 按钮（L184-198 CSS + L241 DOM）：浮文本区右上，编辑态显示，点它 `commitEdit()`。
- ✏️ 按钮（`#tool-edit`，L234）：点 `enterEdit()`；编辑态加 `.active`。
- 窗口尺寸驱动 toolbar 显隐（L257-275）：`HIDDEN_H=100` / `TOOLBAR_H=132`；`showToolbar()` 加 `toolbar-visible` + `setSize(132)`；`hideToolbar()` 有 `editing` 拦截（编辑中不隐藏）。
- **文字区宽度恒 520px（`WIN_W`），不受 toolbar 显隐影响**——水平换行只由 `#result-text` 的 padding 决定。
- `enterEdit()`（L428）：contenteditable=true + `editing` class + 显示 edit-done + focus + 光标置末尾 + `invoke('enter_edit_mode')`。
- `commitEdit()`（L447）+ `edit_shortcut` toggle 再按一次（keydown L468-480）。
- 编辑 toggle 快捷键 `edit_shortcut`（默认 Cmd+E）：进入与保存（退出）都用此键。

## 4. 设计

### 4.1 删除文本区「完成编辑」按钮

- 删 DOM `<button id="edit-done">`（L241）。
- 删 CSS `#edit-done` 全部规则（L184-198）。
- 删 JS `btnEditDone` 引用与事件绑定（L426、L433、L454、L482-483、L497）。

### 4.2 移除编辑态 padding（文字不重排核心）

- 删 `#container.editing #result-text { padding: 1px 90px 7px 13px }`（L206-208）。
- 编辑态 `#result-text` 沿用非编辑态默认 padding → 宽度恒 520px → **水平不重排**。
- 保留 `#container.editing #text-wrapper` 淡蓝边框（编辑中视觉提示）。
- 保留 `#container.editing #result-text:focus { background: transparent }`。

### 4.3 ✏️ 按钮复用 toggle + 图标切换

- CSS 新增（编辑态 `#tool-edit` 图标换 `save.svg`）：
  ```css
  #container.editing #tool-edit .icon {
    -webkit-mask-image: url(icons/save.svg?v=1);
    mask-image: url(icons/save.svg?v=1);
  }
  ```
  非编辑态沿用 `edit.svg`（L103）。`save.svg` 已就位（`icons/save.svg`，Font Awesome 软盘，单色 mask 源）。
- JS `tool-edit` click 改 toggle 语义：
  ```js
  btnEdit.addEventListener('click', (e) => {
    e.preventDefault();
    editing ? commitEdit() : enterEdit();
  });
  ```
- `title`/`aria-label` 编辑态切为「保存编辑」（可访问性，可选）。
- 保留 `btnEdit.classList.add/remove('active')`（编辑态高亮提示）。

### 4.4 编辑态 toolbar 强制常驻（方案 X）

- `enterEdit()` 末尾调 `showToolbar()`：保证编辑态 toolbar 可见，保存按钮（💾）恒可见。
  - 点 ✏️ 进入：toolbar 已 visible（鼠标在按钮上），`showToolbar()` 内 `if (toolbarVisible) return` no-op → **无跳动**。
  - Cmd+E 进入：toolbar 可能 hidden → `showToolbar()` → 窗口 100→132、文字顶部 8→32px（下移 24px，**用户已确认可接受**）。
- `hideToolbar()` 已有 `editing` 拦截（L270），编辑中不隐藏。✓ 无需改。
- `commitEdit()` 后不主动 `hideToolbar()`：toolbar 保持 visible，下次 `mouseleave` 才隐藏（恢复正常 hover 行为，避免退出编辑立即跳变）。

### 4.5 不变项

- 进入方式：`edit_shortcut`（Cmd+E）+ ✏️ 点击。
- 保存（退出）：`edit_shortcut` toggle 再按一次（与进入同键）。
- 后端命令 `enter_edit_mode` / `commit_edit` / `update_edit_buffer` 不变。
- 编辑态硬暂停 ASR（coordinator `editing` 标志）不变。

## 5. 交互流

```
非编辑态（✏️ edit.svg）:
  点 ✏️ 或 Cmd+E → enterEdit():
    contenteditable=true, .editing class, focus 光标置末尾
    图标 edit.svg → save.svg, .active 高亮
    showToolbar()（若此前 hidden：窗口增高、文字下移 24px）
    invoke('enter_edit_mode')

编辑态（💾 save.svg）:
  点 💾 或再按 `edit_shortcut` → commitEdit():
    contenteditable=false, 移除 .editing class
    图标 save.svg → edit.svg, 移除 .active
    invoke('commit_edit', {text})
    （toolbar 保持 visible，mouseleave 后隐藏）
```

## 6. 边界

- **编辑中结果窗隐藏**：新录音触发 `edit-force-exit` 事件清理——需确保清理时同步恢复图标（save.svg → edit.svg）+ 移除 `.active`（当前 `edit-force-exit` 处理 L491-500 已移除 editing class/contenteditable，需补图标恢复）。
- **`edit_shortcut` 在编辑态**：keydown 统一 toggle（L468-470），编辑态再按一次触发保存。同键 toggle，不冲突。
- **save.svg 加载失败**：mask 源缺失 → 图标空白（按钮仍在、可点）。降级可接受。

## 7. 测试

手动 e2e（GUI，需本地 `cargo run`）：

1. 识别出文字 → ✏️ 进入编辑 → **文字水平位置不变（不重排）** ✓
2. 编辑态图标为 💾 → 点 💾 保存 → 退出，图标回 ✏️ ✓
3. `edit_shortcut` 进入 → 再按 `edit_shortcut` 保存（toggle）✓
4. Cmd+E 进入（toolbar 此前 hidden）→ toolbar 出现（窗口增高）→ 💾 可见可点 ✓
5. 编辑中 mouseleave 窗口 → toolbar 不隐藏（editing 拦截）✓
6. 编辑中触发新录音（结果窗 hide）→ `edit-force-exit` → 图标恢复 ✏️、退出编辑态 ✓

无单元测试（纯前端 HTML/CSS/JS 改动，后端命令不变）。

## 8. 影响范围

- 仅 `crates/desktop/dist/result/index.html`（前端单文件）+ 新增 `icons/save.svg`（已就位）。
- 不改 Rust（coordinator / runtime_config / commands 不变）。
- 不改 config（`edit_shortcut` 不变）。
- 不改 editable-result spec（本设计是其布局调整，机制不变）。
