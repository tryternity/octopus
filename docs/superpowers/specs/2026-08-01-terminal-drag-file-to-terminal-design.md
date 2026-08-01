# 终端文件拖拽——文件树拖文件/文件夹到终端，相对路径插入

> **日期**：2026-08-01
> **关联**：[文件树设计](2026-07-31-terminal-file-tree-design.md)、[内嵌终端设计](2026-07-31-embedded-terminal-design.md)、research（Terax `useTerminalFileDrop.ts` + `quoteShellPath.ts` 先例）

## 目标

从文件树侧栏拖文件/文件夹到终端内容区，插入**相对当前 cwd 的、shell 转义的路径**到光标位置（不回车），终端自动聚焦。

## 范围

### 包含
- FileTreePanel 节点设为可拖拽源（`draggable` + `onDragStart` 写 fullPath）
- TerminalPane 终端区设为 drop 目标（`onDragOver` + `onDrop`）
- 相对路径计算（cwd 子树内相对，外部回退绝对）
- shell 转义（空格/特殊字符单引号包裹）
- 插入不回车 + 自动聚焦终端

### 不包含
- **多选拖拽**：本次仅单拖（文件树现为单选）。多选多拖留后续（需先做多选机制）
- **Finder 等外部拖入**：仅文件树内拖拽（外部拖入的路径语义不同，后续）
- **拖到非终端目标**（如编辑器）：仅终端内容区

## 架构

### 三个新单元

1. **`relPath.ts`**（纯函数）：`relPath(fullPath, cwd)` → 相对路径或回退绝对路径
2. **`shellEscape.ts`**（纯函数）：`shellEscape(s)` → shell 安全转义（对齐后端 `shell_escape_single` 安全级别）
3. **拖拽接线**：FileTreePanel `renderNode` 加拖拽源 + TerminalPane canvas 加 drop 目标

### 数据流

```
用户拖 FileTreePanel 节点
  → onDragStart: e.dataTransfer.setData("text/plain", fullPath)
  → 拖到 .terminal-pane-canvas
  → onDragOver: e.preventDefault()（允许 drop，否则浏览器/WKWebView 拒绝）
  → onDrop:
      const fullPath = e.dataTransfer.getData("text/plain")
      const cwd = session.cwd  // useTerminalSession 内部的实时 trackedCwd（OSC 7）
      const rel = relPath(fullPath, cwd ?? "")
      const escaped = shellEscape(rel)
      session.write(escaped)    // 插入光标位置，不回车
      term.focus()              // 自动聚焦，用户可立即继续输入
```

**trackedCwd 来源**：`useTerminalSession` 内部已有 `trackedCwd`（OSC 7 实时追踪，`useTerminalSession.ts:134`），通过 `session.cwd` 暴露。TerminalPane 的 drop handler 直接读 `session.cwd`，**无需改 props 接线**——cd 后相对路径基准自动跟随。

## relPath 算法

核心：fullPath 以 `cwd + "/"` 开头 → 去前缀得相对路径；否则回退绝对路径。

| fullPath | cwd | 结果 | 说明 |
|---|---|---|---|
| `/proj/src/a.ts` | `/proj` | `src/a.ts` | 子树内，去前缀 |
| `/proj` | `/proj` | `.` | 等于 cwd 本身 |
| `/other/file` | `/proj` | `/other/file` | 外部，回退绝对 |
| `/proj` | `/other` | `/proj` | 父级，回退绝对 |
| `/proj/src/a.ts` | `/proj/` | `src/a.ts` | cwd 尾斜杠规范化后匹配 |

**实现要点**：
- 规范化：`cwd = cwd.replace(/\/+$/, "")` 去尾部斜杠
- 判定：
  - `fullPath === cwd` → `"."`
  - `fullPath.startsWith(cwd + "/")` → `fullPath.slice(cwd.length + 1)`
  - 否则 → `fullPath`（绝对路径，不转义——shellEscape 仍会处理其特殊字符）

**不变量**：relPath 只管路径关系，**不做 shell 转义**（空格等留给 shellEscape）。即子树内的 `my dir/b.ts` 返回 `my dir/b.ts`（带空格），由 shellEscape 包成 `'my dir/b.ts'`。

## shellEscape 规则

对齐后端 `shell_escape_single`（`agent_adapter.rs:205`）的安全级别——单引号包裹 + 单引号转义。

| 输入 | 输出 | 说明 |
|---|---|---|
| `file.txt` | `file.txt` | 无特殊字符，原样（更可读） |
| `my file.txt` | `'my file.txt'` | 含空格，单引号包裹 |
| `it's.txt` | `'it'\''s.txt'` | 含单引号，标准转义（闭引号 + `\'` + 开引号） |
| `src/a.ts` | `src/a.ts` | 路径分隔符 `/` 是安全字符，原样 |
| `a$b.txt` | `'a$b.txt'` | 含 `$`，单引号包裹防变量展开 |

**判定「需要转义」**：检查是否含 `[^a-zA-Z0-9_./@:-]` 之外的字符（字母数字下划线 + `.`/`/`/`@`/`:`/`-` 是 shell 安全字符集）。含任一非安全字符 → 单引号包裹。

**单引号转义**：单引号字符串内无法直接含单引号，用 `'\''` 序列（闭引号 + 反斜杠转义单引号 + 开引号）拆分。

**不变量**：shellEscape 不做路径关系判断——输入是什么就转义什么。relPath 和 shellEscape 严格分工，可独立测。

## 接线位置

| 文件 | 改动 |
|---|---|
| `crates/desktop/frontend/src/pages/Terminal/FileTreePanel.tsx` | `renderNode`（~line 162）的行 div 加 `draggable` + `onDragStart`（setData fullPath） |
| `crates/desktop/frontend/src/pages/Terminal/TerminalPane.tsx` | `.terminal-pane-canvas`（~line 153）加 `onDragOver`（preventDefault）+ `onDrop`（读 dataTransfer → relPath → shellEscape → session.write + term.focus） |

## 不变量

1. **仅单拖**：本次不支持多选拖拽（文件树单选机制不变）
2. **相对路径基准是实时 trackedCwd**（OSC 7），不是文件树 cwd 或初始 cwd
3. **插入不回车**：只 `session.write(text)`，不自动 `\n`
4. **drop 后自动聚焦终端**：`term.focus()`
5. **relPath / shellEscape 严格分工**：relPath 只管路径关系不转义，shellEscape 只转义不管路径关系
6. **外部/父目录文件回退绝对路径**：避免 `../../` 难看相对路径

## 测试策略

| 单元 | 覆盖 | 方式 |
|---|---|---|
| `relPath` | 子树内/等于 cwd/外部/父级/尾斜杠/cwd 为空 | vitest 纯函数（~8 case） |
| `shellEscape` | 无特殊/空格/单引号/`$`/路径分隔符/空字符串 | vitest 纯函数（~6 case） |
| 拖拽接线 | dragStart 写 dataTransfer / drop 读 + write / 不回车 / 聚焦 | e2e 手动（HTML5 拖拽在 WKWebView 难单测） |

## 风险

1. **xterm drop 事件被吞**：xterm.js 可能拦截 drop 事件到其内部 textarea。若 `.terminal-pane-canvas` 收不到 onDrop，需在容器父 div 监听，或用 `ondragover`/`ondrop` 直接挂 canvas 父元素。实现时验证事件能否到达。
2. **WKWebView HTML5 拖拽**：WKWebView 对 HTML5 drag/drop 的支持需 e2e 验证。这是主要不确定性——若不工作，备选方案是用 pointer events 模拟（mousedown 跟踪 + 判定 canvas 区域）。
3. **session.cwd 时序**：drop 时 `session.cwd` 必须是最新值。useTerminalSession 的 session 对象每次渲染新引用，drop handler 若闭包捕获旧 session 会拿到 stale cwd——用 ref 持有最新 session（对齐 paste-text listener 的稳定化模式，AGENTS.md gotcha）。
