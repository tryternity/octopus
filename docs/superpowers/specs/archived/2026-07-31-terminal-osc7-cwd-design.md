# OSC 7 cwd 追踪（新 tab 继承目录 + 标题显示路径）

> 内嵌终端增强。追踪 shell 当前工作目录，用于新 tab 继承 + 标题显示。

**日期**：2026-07-31
**蓝本**：Terax `osc-handlers.ts` + `zshrc.zsh`
**关联**：[功能差距对比](../../research/2026-07-30-embedded-terminal-agent-analysis.md) P2

## 目标

终端追踪 shell 当前工作目录（cwd），实现：
1. **新 tab 继承当前目录**——在 `~/projects/foo` 里开新 tab，新 tab 也在 `~/projects/foo`
2. **标题显示目录名**——tab/sidebar item 标题显示 cwd 的 basename（如 `foo` 而非「终端」）

## 范围

- ✅ Shell（zsh/bash）precmd hook 发 OSC 7（`file://host/<urlencoded_pwd>`）
- ✅ 前端 registerOscHandler(7) 解析 + 安全过滤（命令执行期间忽略）
- ✅ 新 tab 继承当前活跃 tab 的 cwd
- ✅ 标题显示目录 basename（优先级：customName > 目录名 > agentName > 默认）
- ❌ ActionBar agent 用终端 cwd（跨模块改 Rust，P2 后续）

## 架构

### 数据流

```
Shell precmd → OSC 7;file://host/path → xterm parser → parseOsc7 → cwd 更新
                                                                     ↓
                                           新 tab 继承 ← addTab(currentCwd)
                                           标题显示 ← displayLabel(加 cwd)
```

### Shell 脚本改动（`scripts/zshrc.zsh` / `bashrc.bash`）

在 precmd hook 里加 OSC 7 发射（与 OSC 133;A 同一处）：
```bash
# zsh
_octopus_precmd() {
  ...
  printf '\e]7;file://%s%s\e\\' "${HOST}" "$(_octopus_urlencode "$PWD")"
  printf '\e]133;A\e\\'
}
```

需要 `_octopus_urlencode` 函数（byte-wise percent-encode，多字节路径安全）——参考 Terax zshrc。

### 前端 `osc-handlers.ts`（新文件）

```typescript
/** 从 OSC 7 payload 解析 cwd（pure，可单测）。 */
function parseOsc7(data: string): string | null {
  // file://host/path → path（percent-decode）
  const m = data.match(/^file:\/\/[^/]*(\/.*)$/);
  if (!m) return null;
  try { return decodeURIComponent(m[1]); } catch { return m[1]; }
}

/** Shell 集成状态——追踪是否在命令执行中（安全过滤用）。 */
type ShellIntegrationState = { inCommand: boolean };

/** 注册 OSC 7 cwd handler + 安全过滤。 */
function registerCwdHandler(term, onCwd, state?): () => void {
  return term.parser.registerOscHandler(7, (data) => {
    if (state?.inCommand) return true; // 命令执行中忽略（防伪造）
    const cwd = parseOsc7(data);
    if (cwd) onCwd(cwd);
    return true;
  });
}

/** 注册 OSC 133 prompt tracker——更新 inCommand 状态。 */
function registerPromptTracker(term, state?): () => void {
  return term.parser.registerOscHandler(133, (data) => {
    if (data.startsWith("A") || data.startsWith("D")) state.inCommand = false;
    else if (data.startsWith("B") || data.startsWith("C")) state.inCommand = true;
    return true;
  });
}
```

### cwd 状态管理（`useTerminalSession`）

- session 返回 `cwd: string | null`
- PTY 连接后注册 registerCwdHandler + registerPromptTracker
- onCwd 回调更新 cwd（通过 state）

### 新 tab 继承（`index.tsx`）

```typescript
const addTab = useCallback((cwd?: string, command?: string) => {
  // cwd 未指定时继承当前活跃 tab 的 cwd
  const effectiveCwd = cwd ?? tabs.find(t => t.id === activeId)?.cwd;
  ...
}, [tabs, activeId]);
```

### 标题显示（`displayLabel` 优先级更新）

```typescript
// agent-activity.ts displayLabel 扩展
function displayLabel(customName, agentName, cwdBasename, fallback): string {
  if (customName?.trim()) return customName;
  if (cwdBasename) return cwdBasename;     // 新增：目录名优先于 agentName
  if (agentName) return agentName;
  return fallback;
}
```

从 cwd 提取 basename：`cwd?.split("/").filter(Boolean).pop() ?? null`

## 安全过滤

**核心不变量**：命令执行期间（OSC 133 B→C→D/A）忽略 OSC 7。

为什么：命令 stdout/stderr 不可信——SSH 远程 shell、`cat` 恶意文件、`echo` 都能发 OSC 7 伪造 cwd。只有 shell 自己的 precmd hook（命令之间）发的 OSC 7 才可信。

octopus 已有 OSC 133 在 shell_init 发射（zsh precmd/preexec），前端 registerPromptTracker 复用这些标记更新 inCommand。

## 测试策略

**parseOsc7 纯函数（TDD）**：
- 正常 `file://host/path` → `/path`
- percent-encode（中文/空格）→ decode
- 无效格式（非 file://、无 path）→ null

**ShellIntegrationState 状态机（TDD）**：
- OSC 133 A/D → inCommand=false（允许 OSC 7）
- OSC 133 B/C → inCommand=true（忽略 OSC 7）

**registerCwdHandler 集成**：依赖真实 xterm parser，靠 e2e（cd 后开新 tab 验证继承）。

## 风险

1. **OSC 7 + OSC 133 时序**：shell 先发 OSC 133;D（命令结束）再发 OSC 7（cwd）再发 OSC 133;A（新 prompt）。inCommand 在 D 时变 false，OSC 7 通过。正确。
2. **bash OSC 133**：bash 用 PROMPT_COMMAND + PS0，OSC 133 B/C 可能不如 zsh 精确——bash 的 inCommand 可能不准，但 precmd 的 OSC 7 仍会在命令间发，安全过滤是兜底。
3. **无 OSC 7 的 shell**（fish/sh）：cwd 保持 null，新 tab 回退 home，标题回退默认——优雅降级。
