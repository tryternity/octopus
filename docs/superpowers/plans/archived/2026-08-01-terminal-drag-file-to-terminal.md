# 终端文件拖拽 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 文件树拖文件/文件夹到终端内容区，插入相对当前 cwd 的、shell 转义的路径（不回车）+ 自动聚焦终端。

**Architecture:** 三个新单元：`relPath` 纯函数（相对路径计算）+ `shellEscape` 纯函数（shell 转义）+ 拖拽接线（FileTreePanel 拖拽源 + TerminalPane drop 目标）。相对路径基准是 `session.cwd`（OSC 7 实时 trackedCwd，无需改 props）。

**Tech Stack:** TypeScript + React + vitest，零新依赖。

**Spec:** `docs/superpowers/specs/2026-08-01-terminal-drag-file-to-terminal-design.md`

## Global Constraints

- **仅单拖**：本次不支持多选拖拽（文件树单选机制不变）
- **相对路径基准是 session.cwd**（OSC 7 实时 trackedCwd，`useTerminalSession.ts:134`）
- **插入不回车**：只 `session.write(text)`，不自动 `\n`
- **drop 后自动聚焦终端**：`term.focus()`
- **relPath / shellEscape 严格分工**：relPath 只管路径关系不转义，shellEscape 只转义不管路径关系
- **外部/父目录文件回退绝对路径**：避免 `../../` 难看相对路径
- **shellEscape 安全字符集**：`[a-zA-Z0-9_./@:-]` 是安全字符，含其他字符则单引号包裹；单引号转义用 `'"'"'`（POSIX 标准，对齐后端 `shell_escape_single` 的 `agent_adapter.rs:205`）

## File Structure

| 文件 | 责任 | 动作 |
|---|---|---|
| `crates/desktop/frontend/src/pages/Terminal/relPath.ts` | 纯函数：relPath(fullPath, cwd) → 相对路径或回退绝对 | 新建 |
| `crates/desktop/frontend/src/pages/Terminal/relPath.test.ts` | relPath 测试（~8 case） | 新建 |
| `crates/desktop/frontend/src/pages/Terminal/shellEscape.ts` | 纯函数：shellEscape(s) → shell 转义 | 新建 |
| `crates/desktop/frontend/src/pages/Terminal/shellEscape.test.ts` | shellEscape 测试（~6 case） | 新建 |
| `crates/desktop/frontend/src/pages/Terminal/FileTreePanel.tsx` | `renderNode`（~line 162）加 `draggable` + `onDragStart` | 修改 |
| `crates/desktop/frontend/src/pages/Terminal/TerminalPane.tsx` | `.terminal-pane-canvas`（~line 155）加 `onDragOver` + `onDrop` | 修改 |

**Decomposition 理由**：Task 1（relPath）+ Task 2（shellEscape）是独立纯函数，可并行 TDD；Task 3（接线）依赖前两者。每个 Task 产出可独立验证的单元。

---

### Task 1: relPath 纯函数（TDD）

**Files:**
- Create: `crates/desktop/frontend/src/pages/Terminal/relPath.ts`
- Test: `crates/desktop/frontend/src/pages/Terminal/relPath.test.ts`

**Interfaces:**
- Produces: `relPath(fullPath: string, cwd: string): string` —— Task 3 的 drop handler 调用。

- [x] **Step 1: Write the failing test**

Create `crates/desktop/frontend/src/pages/Terminal/relPath.test.ts`:

```typescript
import { describe, it, expect } from "vitest";
import { relPath } from "./relPath";

describe("relPath", () => {
  it("子树内：去 cwd 前缀得相对路径", () => {
    expect(relPath("/proj/src/a.ts", "/proj")).toBe("src/a.ts");
  });

  it("多层子树", () => {
    expect(relPath("/proj/src/sub/b.ts", "/proj")).toBe("src/sub/b.ts");
  });

  it("等于 cwd 本身 → '.'", () => {
    expect(relPath("/proj", "/proj")).toBe(".");
  });

  it("外部文件 → 回退绝对路径", () => {
    expect(relPath("/other/file", "/proj")).toBe("/other/file");
  });

  it("父目录 → 回退绝对路径（避免 ../../）", () => {
    expect(relPath("/proj", "/other")).toBe("/proj");
  });

  it("cwd 尾斜杠规范化后匹配", () => {
    expect(relPath("/proj/src/a.ts", "/proj/")).toBe("src/a.ts");
  });

  it("cwd 多个尾斜杠规范化", () => {
    expect(relPath("/proj/src/a.ts", "/proj//")).toBe("src/a.ts");
  });

  it("cwd 为空 → 回退 fullPath", () => {
    expect(relPath("/proj/file", "")).toBe("/proj/file");
  });

  it("含空格的子树路径：relPath 不转义（留给 shellEscape）", () => {
    expect(relPath("/proj/my dir/a.ts", "/proj")).toBe("my dir/a.ts");
  });
});
```

- [x] **Step 2: Run test to verify it fails**

Run: `cd crates/desktop/frontend && npx vitest run src/pages/Terminal/relPath.test.ts`
Expected: FAIL，`Failed to resolve import "./relPath"`。

- [x] **Step 3: Write minimal implementation**

Create `crates/desktop/frontend/src/pages/Terminal/relPath.ts`:

```typescript
/**
 * 计算 fullPath 相对 cwd 的路径。
 *
 * - cwd 子树内（fullPath 以 cwd + "/" 开头）：去前缀得相对路径
 * - 等于 cwd：返回 "."
 * - 外部/父目录：回退绝对路径（避免 ../../ 难看相对路径）
 *
 * 只管路径关系，不做 shell 转义（空格等留给 shellEscape）。
 */
export function relPath(fullPath: string, cwd: string): string {
  // 规范化：去 cwd 尾部斜杠（防 /proj/ vs /proj 不匹配）
  const normalizedCwd = cwd.replace(/\/+$/, "");
  if (!normalizedCwd) return fullPath;
  if (fullPath === normalizedCwd) return ".";
  const prefix = normalizedCwd + "/";
  if (fullPath.startsWith(prefix)) {
    return fullPath.slice(prefix.length);
  }
  return fullPath;
}
```

- [x] **Step 4: Run test to verify it passes**

Run: `cd crates/desktop/frontend && npx vitest run src/pages/Terminal/relPath.test.ts`
Expected: PASS（9 个 it 全过）。

- [x] **Step 5: Commit**

```bash
git add crates/desktop/frontend/src/pages/Terminal/relPath.ts crates/desktop/frontend/src/pages/Terminal/relPath.test.ts
git commit -m "feat(terminal): relPath 纯函数——相对路径计算（拖拽 TDD 入口）"
```

---

### Task 2: shellEscape 纯函数（TDD）

**Files:**
- Create: `crates/desktop/frontend/src/pages/Terminal/shellEscape.ts`
- Test: `crates/desktop/frontend/src/pages/Terminal/shellEscape.test.ts`

**Interfaces:**
- Produces: `shellEscape(s: string): string` —— Task 3 的 drop handler 调用。

**Context:**
- 对齐后端 `shell_escape_single`（`agent_adapter.rs:205-207`）的安全级别。
- 后端实现：`format!("'{}'", s.replace('\'', "'\"'\"'"))` —— 始终单引号包裹 + 单引号转义为 `'"'"'`。
- 本前端版采用「条件包裹」（无特殊字符不包裹，更可读），但单引号转义用同样的 `'"'"'`（POSIX 标准）。安全级别与后端一致。

- [x] **Step 1: Write the failing test**

Create `crates/desktop/frontend/src/pages/Terminal/shellEscape.test.ts`:

```typescript
import { describe, it, expect } from "vitest";
import { shellEscape } from "./shellEscape";

describe("shellEscape", () => {
  it("无特殊字符 → 原样（更可读）", () => {
    expect(shellEscape("file.txt")).toBe("file.txt");
    expect(shellEscape("src/a.ts")).toBe("src/a.ts");
  });

  it("含空格 → 单引号包裹", () => {
    expect(shellEscape("my file.txt")).toBe("'my file.txt'");
  });

  it("含单引号 → 单引号包裹 + 单引号转义（POSIX '\"'\"' 法）", () => {
    expect(shellEscape("it's.txt")).toBe("'it'\"'\"'s.txt'");
  });

  it("含 $ → 单引号包裹防变量展开", () => {
    expect(shellEscape("a$b.txt")).toBe("'a$b.txt'");
  });

  it("路径分隔符 / 是安全字符 → 原样", () => {
    expect(shellEscape("path/to/file")).toBe("path/to/file");
  });

  it("空字符串 → 原样（空无需转义）", () => {
    expect(shellEscape("")).toBe("");
  });

  it("安全字符集（字母数字 _ . / @ : -）→ 原样", () => {
    expect(shellEscape("a.b-c_d@e:f/g")).toBe("a.b-c_d@e:f/g");
  });
});
```

- [x] **Step 2: Run test to verify it fails**

Run: `cd crates/desktop/frontend && npx vitest run src/pages/Terminal/shellEscape.test.ts`
Expected: FAIL，`Failed to resolve import "./shellEscape"`。

- [x] **Step 3: Write minimal implementation**

Create `crates/desktop/frontend/src/pages/Terminal/shellEscape.ts`:

```typescript
/**
 * shell 转义——单引号包裹含特殊字符的字符串。
 *
 * 安全级别对齐后端 shell_escape_single（agent_adapter.rs:205）。差异：后端始终
 * 包裹，本前端版「条件包裹」（无特殊字符不包裹，更可读）。
 *
 * - 含任一非安全字符（[^a-zA-Z0-9_./@:-]）→ 单引号包裹
 * - 含单引号 → POSIX 标准转义：'"'"'（闭引号 + 双引号包裹单引号 + 开引号）
 * - 无特殊字符 → 原样
 */
const SAFE_CHARS = /^[a-zA-Z0-9_./@:-]*$/;

export function shellEscape(s: string): string {
  if (s === "") return "";
  if (SAFE_CHARS.test(s)) return s;
  // 单引号转义：' → '"'"'（POSIX 标准双引号法，对齐后端 shell_escape_single）
  return `'${s.replace(/'/g, "'\"'\"'")}'`;
}
```

- [x] **Step 4: Run test to verify it passes**

Run: `cd crates/desktop/frontend && npx vitest run src/pages/Terminal/shellEscape.test.ts`
Expected: PASS（7 个 it 全过）。

- [x] **Step 5: Commit**

```bash
git add crates/desktop/frontend/src/pages/Terminal/shellEscape.ts crates/desktop/frontend/src/pages/Terminal/shellEscape.test.ts
git commit -m "feat(terminal): shellEscape 纯函数——shell 转义（对齐后端安全级别）"
```

---

### Task 3: 拖拽接线（FileTreePanel + TerminalPane）

**Files:**
- Modify: `crates/desktop/frontend/src/pages/Terminal/FileTreePanel.tsx`（renderNode ~line 162）
- Modify: `crates/desktop/frontend/src/pages/Terminal/TerminalPane.tsx`（canvas ~line 155）

**Interfaces:**
- Consumes: `relPath` from Task 1, `shellEscape` from Task 2。

**Context:**
- FileTreePanel `renderNode(name, fullPath, kind, depth)`（~line 154）的行 div（~line 162）是拖拽起点，fullPath 已可用。
- TerminalPane `.terminal-pane-canvas`（~line 155，`containerRef`）是 drop 目标。
- TerminalPane 内部已有 `sessionRef`（~line 128，稳定化模式对齐 paste-text listener，AGENTS.md gotcha）——drop handler 用 `sessionRef.current` 拿最新 session（含实时 cwd + write/focus）。
- `useTerminalSession` 暴露 `session.cwd`（OSC 7 实时 trackedCwd）+ `session.write` + `session.focus`（需确认 focus 是否在 session 上，可能在 term 实例上）。

- [x] **Step 1: FileTreePanel renderNode 加拖拽源**

Modify `crates/desktop/frontend/src/pages/Terminal/FileTreePanel.tsx`。

在 `renderNode` 的行 div（~line 162-187）加 `draggable` + `onDragStart`：

```typescript
        <div
          className={`file-tree-row ${isSelected ? "file-tree-row-selected" : ""}`}
          style={{ paddingLeft: `${depth * 12 + 4}px` }}
          draggable
          onDragStart={(e) => {
            e.dataTransfer.setData("text/plain", fullPath);
            e.dataTransfer.effectAllowed = "copy";
          }}
          onClick={() => {
            if (isDir) toggleDir(fullPath);
            else setSelected(fullPath);
          }}
          onContextMenu={(e) => openNodeMenu(e, name, fullPath)}
        >
```

- [x] **Step 2: TerminalPane canvas 加 drop 目标**

Modify `crates/desktop/frontend/src/pages/Terminal/TerminalPane.tsx`。

顶部 import 加：
```typescript
import { relPath } from "./relPath";
import { shellEscape } from "./shellEscape";
```

修改 `.terminal-pane-canvas` div（~line 155），加 `onDragOver` + `onDrop`：

```typescript
      <div
        ref={containerRef}
        className="terminal-pane-canvas"
        onContextMenu={openContextMenu}
        onDragOver={(e) => {
          // 允许 drop（否则浏览器/WKWebView 拒绝）
          if (e.dataTransfer.types.includes("text/plain")) {
            e.preventDefault();
            e.dataTransfer.dropEffect = "copy";
          }
        }}
        onDrop={(e) => {
          const fullPath = e.dataTransfer.getData("text/plain");
          if (!fullPath) return;
          e.preventDefault();
          const s = sessionRef.current;
          const rel = relPath(fullPath, s.cwd ?? "");
          const escaped = shellEscape(rel);
          s.write(escaped);  // 插入光标位置，不回车
          s.focus();         // 自动聚焦终端
        }}
      />
```

**注意**：需确认 `session.focus()` 是否存在——`useTerminalSession` 返回的 session 对象是否有 `focus` 方法。若没有，可能要用 `containerRef.current?.querySelector("textarea")?.focus()` 或暴露 term 实例。实现时核实 session 接口。

- [x] **Step 3: session 接口已确认（无需额外核实）**

`useTerminalSession` 返回的 session 已暴露 `write`（写 PTY）+ `focus`（聚焦 xterm）+ `cwd`（OSC 7 实时）——均已确认（useTerminalSession.ts:343-348）。Step 2 的 `s.write(escaped)` + `s.focus()` + `s.cwd` 直接可用，无需改 session 接口。此步无代码改动。

- [x] **Step 4: tsc + vitest**

Run:
```bash
cd crates/desktop/frontend
npx tsc --noEmit
npx vitest run
```
Expected: tsc 0 error；vitest 全过（原 442 + Task 1/2 的 16 = 458）。

- [x] **Step 5: Commit**

```bash
git add crates/desktop/frontend/src/pages/Terminal/FileTreePanel.tsx crates/desktop/frontend/src/pages/Terminal/TerminalPane.tsx
git commit -m "feat(terminal): 文件树拖文件到终端——相对路径插入（不回车）+ 自动聚焦"
```

---

### Task 4: e2e 验证 + 文档同步

**Files:**
- 无代码改动，验证 + 文档

- [x] **Step 1: 构建 + 手动 e2e**

Run:
```bash
./scripts/build-macos-dmg.sh --no-lto --open
```

手动测试清单：
1. ✅ 拖文件树文件到终端 → 插入相对路径（如 cwd `/proj`，拖 `src/a.ts` → 插入 `src/a.ts`）
2. ✅ 拖含空格的文件 → 插入转义路径（`my dir/a.ts` → `'my dir/a.ts'`）
3. ✅ 拖文件夹 → 插入相对目录路径（如 `src`）
4. ✅ cd 后拖文件 → 相对路径跟随新 cwd（OSC 7 实时性）
5. ✅ 拖 cwd 外部文件 → 回退绝对路径
6. ✅ 拖入后光标在终端（自动聚焦），可继续输入（如先输 `vim ` 再拖文件 → `vim src/a.ts`）
7. ✅ 拖入不回车（路径插入但命令不执行）
8. ⚠️ WKWebView HTML5 拖拽是否工作（主要风险）——若不工作，记录现象

- [x] **Step 2: 更新 architecture.md + research**

Modify `docs/architecture.md` 终端章节，补「文件树拖文件到终端（相对路径插入）」。
Modify `docs/research/2026-07-30-embedded-terminal-agent-analysis.md`「octopus 独有」或「已实现」表，标记「文件拖放进终端」从 P2 → ✅ 已实现。

- [x] **Step 3: Commit**

```bash
git add docs/
git commit -m "docs(sync): 终端文件拖拽——architecture + research 同步"
```

- [x] **Step 4: Review plan（强制——回看偏差）**

实现完成后回到本 plan，把实际偏差（如 session.focus 接口、xterm drop 事件是否被吞、WKWebView 拖拽兼容性）回写。

---

## Self-Review 记录

**Spec 覆盖检查**：
- ✅ 三个新单元（relPath + shellEscape + 接线）→ Task 1/2/3
- ✅ 相对路径基准 session.cwd（OSC 7 实时）→ Task 3 Step 2（用 sessionRef.current.cwd）
- ✅ 子树内相对/外部回退绝对 → Task 1（relPath 算法）
- ✅ shellEscape 对齐后端 → Task 2（SAFE_CHARS + `'"'"'` 转义）
- ✅ 插入不回车 + 自动聚焦 → Task 3 Step 2
- ✅ 仅单拖 → Task 3（FileTreePanel 单节点 draggable）
- ✅ relPath/shellEscape 严格分工 → Task 1/2 独立纯函数

**类型一致性**：
- `relPath(fullPath: string, cwd: string): string` —— Task 1 定义、Task 3 调用一致
- `shellEscape(s: string): string` —— Task 2 定义、Task 3 调用一致
- `sessionRef.current` 拿 session（含 cwd + write + focus）—— Task 3 复用 TerminalPane 现有 ref 模式

**已知实现注意**（非占位符）：
- ~~Task 3 Step 2 的 `session.focus()`：需确认 session 是否暴露 focus~~ —— 已确认 session 暴露 `write`/`focus`/`cwd`（useTerminalSession.ts:343-348），直接可用。
- Task 4 的 xterm drop 事件风险：xterm.js 可能拦截 drop。若 `.terminal-pane-canvas` 收不到 onDrop，备选是在容器父 div 监听。这是主要不确定性，e2e 验证。
- WKWebView HTML5 拖拽是最大风险——若完全不工作，需改用 pointer events 模拟（spec 风险 #2）。实现时先验证 HTML5 拖拽能否工作。

## 实施记录（Review plan 回写，2026-08-01）

4 个 task 全部实现完成 + e2e 通过。但 **Task 3 严重偏离 plan**——HTML5 DnD 方案在 WKWebView 完全失败，演进成双入口方案。

### 关键偏离：HTML5 DnD 失败 → 双入口方案（Task 3）

plan 原写的 Task 3 是 HTML5 DnD 接线（`draggable` + `onDragStart` + `onDragOver` + `onDrop`）。实测：
1. **onDrop（bubble 阶段）不触发** —— xterm canvas 内部元素拦截，事件不冒泡到 `.terminal-pane-canvas`
2. **onDropCapture（capture 阶段）也不触发** —— 改 capture 仍不行
3. **结论**：WKWebView + xterm canvas 下 HTML5 DnD 完全不可靠

**演进过程**（5 个 fix commit）：
1. `8ba3d259` 初始 HTML5 DnD 接线（按 plan）→ e2e 失败（drop 不触发）
2. `5337ba8a` 改 capture 阶段 → 仍失败
3. `b294d8f7` **切换 pointer events 方案**（dragStore + mousedown/mouseup hit-test）→ 功能通但无 ghost
4. `65c65b63` **加 Finder OS 拖入**（Tauri `onDragDropEvent`，照搬 Terax）→ OS 原生 ghost 体验
5. `c2a08016` **文件树拖拽自定义 ghost**（dragStore.startDrag 创建跟随鼠标 div）→ 内部拖拽也有视觉反馈

**最终架构**（双入口，详见 spec）：
- Finder OS 拖入 → `onDragDropEvent` + `formatDroppedPaths`（绝对路径）+ paste
- 文件树内部拖拽 → pointer events + dragStore ghost + relPath（相对路径）+ paste

### 其他偏离

1. **写入方式 write → paste**（`80ee9a6b`）：参考 Terax，拖文件=用户粘贴语义，bracketed paste mode 让 Claude Code 等程序正确识别。plan 原写 `session.write`。
2. **新增 formatDroppedPaths**（`65c65b63`）：shellEscape.ts 加多文件格式化函数（照搬 Terax，OS 拖入用）。
3. **dragStore.ts 演进**：最初只有 setDragPath/takeDragPath（`b294d8f7`），后扩展 startDrag（含 ghost 管理，`c2a08016`）。废弃 setDragPath/clearDragPath。

### 教训

**WKWebView 的 HTML5 DnD 不可靠是硬限制**——遇到拖拽需求时，优先考虑 Tauri 原生 `onDragDropEvent`（OS 文件）+ pointer events（内部 DOM），不要先尝试 HTML5 DnD。已记入本次 spec 的「为什么不用 HTML5 DnD」段。
