/**
 * 终端键映射纯函数——macOS Option/Cmd 组合键 → readline 转义序列。
 *
 * 对齐 Terax keymap.ts。Ctrl 组合键（Ctrl+A/C/...）由 xterm.js 默认处理，
 * 这里只管浏览器默认不产生正确字符的 macOS 增强键：
 * - Option+←/→ 词导航
 * - Cmd+←/→ 行首行尾
 * - Cmd/Option+Backspace 删除
 *
 * 所有函数纯：输入 KeyboardEvent 的关键字段，输出转义序列字符串或 null。
 */

/** handler 需要的 KeyboardEvent 字段。 */
export type TerminalKeyEvent = Pick<
  KeyboardEvent,
  "altKey" | "ctrlKey" | "metaKey" | "shiftKey" | "key" | "code"
>;

export type PlatformOpts = { isMac: boolean };

/**
 * Option+←/→ 词导航（跨平台——Option 在 macOS = Alt）。
 * - Option+Left  → `\x1bb`（ESC+b，readline backward-word）
 * - Option+Right → `\x1bf`（ESC+f，readline forward-word）
 *
 * 纯 Option，无 Ctrl/Meta 干扰。
 */
export function wordNavigationSequence(event: TerminalKeyEvent): string | null {
  if (!event.altKey || event.ctrlKey || event.metaKey) return null;
  if (event.key === "ArrowLeft" || event.code === "ArrowLeft") return "\x1bb";
  if (event.key === "ArrowRight" || event.code === "ArrowRight") return "\x1bf";
  return null;
}

/**
 * Cmd+←/→ 行首行尾（macOS only——Cmd 在其他平台不存在作导航修饰）。
 * - Cmd+Left  → `\x01`（Ctrl+A，readline beginning-of-line）
 * - Cmd+Right → `\x05`（Ctrl+E，readline end-of-line）
 */
export function lineNavigationSequence(
  event: TerminalKeyEvent,
  opts: PlatformOpts,
): string | null {
  if (!opts.isMac) return null;
  if (!event.metaKey || event.altKey || event.ctrlKey) return null;
  if (event.key === "ArrowLeft" || event.code === "ArrowLeft") return "\x01";
  if (event.key === "ArrowRight" || event.code === "ArrowRight") return "\x05";
  return null;
}

/**
 * Cmd/Option+Backspace 删除（macOS）。
 * - Cmd+Backspace    → `\x15`（Ctrl+U，readline unix-line-discard，删到行首）
 * - Option+Backspace → `\x17`（Ctrl+W，readline unix-word-rubout，删一个词）
 */
export function deleteSequence(
  event: TerminalKeyEvent,
  opts: PlatformOpts,
): string | null {
  if (event.key !== "Backspace" && event.code !== "Backspace") return null;
  if (opts.isMac) {
    if (event.metaKey && !event.altKey && !event.ctrlKey) return "\x15";
    if (event.altKey && !event.metaKey && !event.ctrlKey) return "\x17";
    return null;
  }
  return null;
}

/**
 * 聚合 readline 序列。alternate screen（vim/htop 全屏）模式返回 null——
 * TUI 应用自己管理光标，readline 序列会干扰。
 *
 * 尝试顺序：行导航 → 词导航 → 删除（返回首个非 null）。
 */
export function readlineSequence(
  event: TerminalKeyEvent,
  opts: PlatformOpts & { isAlternateScreen: boolean },
): string | null {
  if (opts.isAlternateScreen) return null;
  return (
    lineNavigationSequence(event, opts) ??
    wordNavigationSequence(event) ??
    deleteSequence(event, opts)
  );
}

/** Shift+Enter（无其他修饰）→ `\x1b\r`（部分 TUI / 多行输入用）。 */
export function isShiftEnter(event: TerminalKeyEvent): boolean {
  return (
    event.key === "Enter" &&
    event.shiftKey &&
    !event.altKey &&
    !event.ctrlKey &&
    !event.metaKey
  );
}
