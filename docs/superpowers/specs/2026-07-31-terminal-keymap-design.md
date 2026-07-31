# 终端标准控制键 + macOS 增强键

> 内嵌终端键映射增强。补全 macOS Option/Cmd 组合键 + IME 兼容 + Shift+Enter。

**日期**：2026-07-31
**蓝本**：Terax `keymap.ts` + `rendererPool.ts::attachCustomKeyEventHandler`
**关联**：[内嵌终端 spec](2026-07-31-embedded-terminal-design.md)

## 背景

xterm.js 默认处理所有 ASCII 控制键（Ctrl+A/B/C/D/...，转成 `\x01-\x1f` 给 onData），这些 octopus 已支持（用户确认 Ctrl+C 中断正常）。

但 macOS 的 **Option 组合键**（词导航/词删除）和 **Cmd 组合键**（行首行尾）在 WKWebView 里浏览器默认不产生 readline 期望的转义序列——Option+← 不是 `\x1bb`。必须用 `attachCustomKeyEventHandler` 显式映射。同时 **IME 输入法组合**（中文拼音）期间的原生 keydown 必须拦截，否则会漏字/重复。

## 范围

- ✅ macOS Option+←/→ 词导航（`\x1bb`/`\x1bf`）
- ✅ macOS Option+Backspace 删词（`\x17`）
- ✅ macOS Cmd+←/→ 行首行尾（`\x01`/`\x05`）
- ✅ macOS Cmd+Backspace 删到行首（`\x15`）
- ✅ IME 兼容（isComposing / keyCode 229 拦截）
- ✅ Shift+Enter → `\x1b\r`（部分 TUI 用）
- ✅ alternate screen（TUI 全屏模式，如 vim/htop）禁用 readline 序列——交给应用自己处理
- ❌ 自定义键位映射（YAGNI，Terax 也没做）
- ❌ Linux/Windows Ctrl+Backspace 删词（macOS-only 优先，Terax 的 `terminalDeleteSequence` 有非 Mac 分支但 octopus 暂只 Mac）

## 架构

### keymap.ts（纯函数，可 TDD）

提取 3 个映射函数 + 1 个聚合函数，对齐 Terax keymap.ts：

```typescript
type TerminalKeyEvent = Pick<KeyboardEvent, "altKey" | "ctrlKey" | "metaKey" | "key" | "code">;

/** Option+←/→ 词导航（macOS）。纯 Option，无 Ctrl/Meta 干扰。 */
function wordNavigationSequence(event): string | null {
  // Option+Left → \x1bb, Option+Right → \x1bf
}

/** Cmd+←/→ 行首行尾（macOS only）。 */
function lineNavigationSequence(event, { isMac }): string | null {
  // Cmd+Left → \x01, Cmd+Right → \x05
}

/** Cmd/Option+Backspace 删除（macOS）。
 *  Cmd+Backspace → \x15 (删到行首), Option+Backspace → \x17 (删词) */
function deleteSequence(event, { isMac }): string | null

/** 聚合：alternate screen 模式返回 null（交给 TUI 应用）。
 *  否则尝试 line/word/delete 序列。 */
function readlineSequence(event, { isMac, isAlternateScreen }): string | null
```

### attachCustomKeyEventHandler 接线（useTerminalSession.ts）

在 xterm Terminal 创建后注册 handler：

```typescript
term.attachCustomKeyEventHandler((event) => {
  // 1. IME 组合中——拦截原生 keydown（含提交候选的 Enter），
  //    xterm 通过 compositionend 收最终字符串
  if (event.isComposing || event.keyCode === 229) return false;

  // 2. readline 序列（Option/Cmd 导航+删除）
  const seq = readlineSequence(event, { isMac: IS_MAC, isAlternateScreen });
  if (seq) {
    event.preventDefault();
    if (event.type === "keydown") pty.write(seq);
    return false;  // xterm 不再处理
  }

  // 3. Shift+Enter → \x1b\r
  if (isShiftEnter(event)) {
    event.preventDefault();
    if (event.type === "keydown") pty.write("\x1b\r");
    return false;
  }

  // 4. 其余（含 Ctrl+A/C/...、Cmd+C/V 复制粘贴）交给 xterm 默认
  return true;
});
```

### 辅助判定函数

```typescript
const IS_MAC = /Mac|iPhone|iPad/.test(navigator.userAgent);

function isShiftEnter(e): boolean {
  return e.key === "Enter" && e.shiftKey && !e.altKey && !e.ctrlKey && !e.metaKey;
}
```

## 不变量

1. `attachCustomKeyEventHandler` 返回 `false` = xterm 不处理该键（我们接管）；`true` = xterm 默认处理
2. IME 组合中（isComposing / keyCode 229）必须返回 `false`——否则中文输入会吞字/重复
3. alternate screen（vim/htop 全屏）禁用 readline 序列——TUI 应用自己管理光标
4. readline 序列匹配后必须 `preventDefault` + `return false`，避免浏览器默认行为（如 Cmd+← 浏览器后退）

## 测试策略

**keymap.ts 纯函数（TDD 核心）**：
- wordNavigationSequence：Option+← `\x1bb` / Option+→ `\x1bf` / 无 Option 返回 null
- lineNavigationSequence：Mac Cmd+← `\x01` / Cmd+→ `\x05` / 非 Mac 返回 null / 有 Ctrl 返回 null
- deleteSequence：Mac Cmd+Backspace `\x15` / Option+Backspace `\x17` / 无修饰 null
- readlineSequence：alternate screen 返回 null / 聚合三个子函数

**handler 接线**：依赖真实 DOM/KeyboardEvent，靠 e2e 冒烟（IME 中文输入、Option 词导航、Cmd 行首）。

## 风险

1. **WKWebView Option 键行为**：macOS 的 Option 在浏览器里可能被当作 Alt 产生特殊字符（如 Option+L = `¬`）。xterm 默认 `macOptionIsMeta: false`，我们的 handler 在 keydown 阶段拦截（before 字符产生），应正确。若实测 Option 组合仍异常，加 `macOptionIsMeta: true`（但会影响普通 Option+字母输入）。
2. **alternate screen 检测**：`term.buffer.active.type === "alternate"` 在 vim/less 进入全屏时为 true。需确保 handler 读的是实时 buffer 状态。
