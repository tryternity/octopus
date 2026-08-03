# 终端文件拖拽——文件树 + Finder 拖文件到终端

> **状态：✅ 实现完成（已 e2e 验证）**
> **日期**：2026-08-01
> **关联**：[文件树设计](2026-07-31-terminal-file-tree-design.md)、[内嵌终端设计](2026-07-31-embedded-terminal-design.md)、research（Terax `useTerminalFileDrop.ts` + `quoteShellPath.ts` 先例）

## 目标

把文件拖到终端内容区，插入 shell 转义路径到光标位置（不回车）+ 自动聚焦。支持两个入口：
1. **文件树内部拖拽**（webview DOM）：插入相对当前 cwd 的路径
2. **Finder OS 文件拖入**：插入绝对路径（OS 文件不一定在 cwd 子树）

## 范围

### 包含
- FileTreePanel 节点 mousedown 发起拖拽（pointer events 方案 + 自定义 ghost）
- Finder/OS 文件拖入（Tauri `onDragDropEvent`，OS 原生 ghost）
- TerminalPane canvas 接收 drop（document mouseup hit-test / onDragDropEvent）
- 相对路径计算（文件树拖拽用，cwd 子树内相对，外部回退绝对）
- shell 转义（空格/特殊字符单引号包裹）
- 插入不回车（`session.paste`）+ 自动聚焦终端

### 不包含
- **多选拖拽**：本次仅单拖（文件树现为单选）。多选多拖留后续
- **拖到非终端目标**（如编辑器）：仅终端内容区

## 架构

### 为什么不用 HTML5 DnD（重要决策）

**实测**：HTML5 Drag and Drop（`draggable`/`dataTransfer`）在 WKWebView + xterm canvas 下**完全不可靠**——`onDrop`（bubble 阶段）不触发，`onDropCapture`（capture 阶段）也不触发。这是 WKWebView 的已知限制。

因此采用**双入口分治**：
- **Finder OS 拖入**用 Tauri 原生 `onDragDropEvent`（OS 层事件，绕开 HTML5 DnD）
- **文件树内部拖拽**用 pointer events 模拟（mousedown/mouseup，绕开 HTML5 DnD）

### 五个单元

1. **`relPath.ts`**（纯函数）：`relPath(fullPath, cwd)` → 相对路径或回退绝对路径（仅文件树拖拽用）
2. **`shellEscape.ts`**（纯函数）：`shellEscape(s)` shell 转义 + `formatDroppedPaths(paths)` 多文件格式化
3. **`dragStore.ts`**（模块级状态 + ghost）：`startDrag(path, label)` 创建 ghost + 启动 mousemove 跟踪；`takeDragPath()` 取路径 + 清除 ghost
4. **文件树拖拽源**：FileTreePanel `renderNode` 的 `onMouseDown` 调 `startDrag(fullPath, name)`
5. **终端 drop 目标**：
   - 文件树拖拽：TerminalPane `document mouseup` + containerRef hit-test → `takeDragPath` → relPath + shellEscape + `session.paste`
   - OS 拖拽：TerminalPane `getCurrentWebview().onDragDropEvent` → `formatDroppedPaths(p.paths)` + `session.paste`（只活跃 pane 挂）

### 数据流

**文件树内部拖拽**（pointer events）：
```
用户在文件树节点 mousedown
  → startDrag(fullPath, name)：dragStore 记录路径 + 创建 ghost + 启动 mousemove 跟踪
  → 拖动（mousemove）：ghost 跟随鼠标（显示文件名）+ body.terminal-file-dragging 触发 CSS（节点半透明 + canvas hover 高亮）
  → 拖到终端 canvas 松开（document mouseup）：
      const path = takeDragPath()  // 取路径 + 清除 ghost/body class
      hit-test：鼠标是否在 containerRef 矩形内
      是 → const rel = relPath(path, sessionRef.current.cwd)
           const escaped = shellEscape(rel)
           session.paste(escaped) + session.focus()
      否 → 忽略（path 已被 take 清除）
```

**Finder OS 文件拖入**（Tauri onDragDropEvent）：
```
用户从 Finder 拖文件到窗口（OS 原生 ghost）
  → getCurrentWebview().onDragDropEvent payload.type === "drop"
  → p.paths（OS 文件路径数组，绝对路径）
  → session.paste(formatDroppedPaths(p.paths)) + session.focus()
```

### 写入方式：paste 而非 write（参考 Terax）

拖文件本质是「用户粘贴路径」，用 `session.paste`（bracketed paste mode）而非 `session.write`（字面写 PTY）。终端跑 Claude Code 等开启 bracketed paste 的程序时，paste 让它正确识别为一次完整输入（而非逐字符）；普通 shell 两者行为一致。

## relPath 算法

核心：fullPath 以 `cwd + "/"` 开头 → 去前缀得相对路径；否则回退绝对路径。

| fullPath | cwd | 结果 | 说明 |
|---|---|---|---|
| `/proj/src/a.ts` | `/proj` | `src/a.ts` | 子树内，去前缀 |
| `/proj` | `/proj` | `.` | 等于 cwd 本身 |
| `/other/file` | `/proj` | `/other/file` | 外部，回退绝对 |
| `/proj/src/a.ts` | `/proj/` | `src/a.ts` | cwd 尾斜杠规范化 |

**仅文件树拖拽用**（OS 拖入用绝对路径，不经 relPath——Finder 文件不一定在 cwd 子树）。

## shellEscape 规则

对齐后端 `shell_escape_single`（`agent_adapter.rs:205`）安全级别。

- 含任一非安全字符（`[^a-zA-Z0-9_./@:-]`）→ 单引号包裹
- 含单引号 → POSIX 标准转义 `'"'"'`
- 无特殊字符 → 原样
- `formatDroppedPaths(paths)`：多文件各转义 + 空格连接 + 末尾空格（OS 拖入用，照搬 Terax）

## 不变量

1. **双入口独立**：文件树拖拽（pointer events）和 OS 拖入（onDragDropEvent）互不干扰
2. **文件树拖拽用相对路径**（relPath）；**OS 拖入用绝对路径**（formatDroppedPaths）
3. **插入不回车**：只 `session.paste(text)`，不自动 `\n`
4. **drop 后自动聚焦终端**：`session.focus()`
5. **OS 拖入只活跃 pane 响应**：非活跃 tab 隐藏，listener 只在 active 时挂
6. **relPath/shellEscape 严格分工**：relPath 只管路径关系不转义，shellEscape 只转义不管路径关系

## 测试策略

| 单元 | 覆盖 | 方式 |
|---|---|---|
| `relPath` | 子树内/等于 cwd/外部/父级/尾斜杠/cwd 为空 | vitest 纯函数（9 case） |
| `shellEscape` | 无特殊/空格/单引号/`$`/路径分隔符/空字符串 | vitest 纯函数（7 case） |
| 拖拽接线 | mousedown 设状态 / mouseup hit-test + paste / OS onDragDropEvent / ghost 创建移除 | e2e 手动（已验证通过） |

## 风险（已解决）

1. ~~xterm drop 事件被吞~~ → **已确认 HTML5 DnD 在 WKWebView 完全不可靠**，改用 pointer events + OS 拖入绕开
2. ~~WKWebView HTML5 拖拽~~ → **改用 Tauri onDragDropEvent（OS）+ pointer events（内部）**
3. **OS 拖入绝对路径**：Finder 文件用绝对路径不经 relPath——可能较长，但 OS 文件不在 cwd 子树时相对路径会是 `../../`（更难看），绝对路径更合理

## 关键文件

- `crates/desktop/frontend/src/pages/Terminal/relPath.ts`（纯函数 + 9 测试）
- `crates/desktop/frontend/src/pages/Terminal/shellEscape.ts`（纯函数 + formatDroppedPaths + 7 测试）
- `crates/desktop/frontend/src/pages/Terminal/dragStore.ts`（模块级状态 + ghost 管理）
- `crates/desktop/frontend/src/pages/Terminal/FileTreePanel.tsx`（renderNode onMouseDown → startDrag）
- `crates/desktop/frontend/src/pages/Terminal/TerminalPane.tsx`（document mouseup hit-test + onDragDropEvent listener）
- 参考实现：Terax `useTerminalFileDrop.ts` + `quoteShellPath.ts`
