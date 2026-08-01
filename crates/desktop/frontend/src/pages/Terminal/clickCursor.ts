/**
 * 终端点击定位光标的纯函数（spec 2026-08-01-terminal-click-cursor）。
 *
 * 像素坐标 → 列号 → 门控 → 偏移转义序列。三个函数严格分工，可独立测。
 */

/** 像素 clientX → xterm 列号（clamp 到 [0, cols-1]）。 */
export function pixelToCol(
  clientX: number,
  rectLeft: number,
  rectWidth: number,
  cols: number,
): number {
  const cellWidth = rectWidth / cols;
  const col = Math.floor((clientX - rectLeft) / cellWidth);
  return Math.max(0, Math.min(cols - 1, col));
}

/** 门控：是否应该响应点击移动光标（三条件全满足）。 */
export function shouldMoveCursor(state: {
  inCommand: boolean;
  bufferType: "normal" | "alternate";
  clickRow: number;
  cursorY: number;
}): boolean {
  return (
    !state.inCommand &&
    state.bufferType === "normal" &&
    state.clickRow === state.cursorY
  );
}

/** 偏移量 → ANSI 转义序列（CUF 右移 / CUB 左移）。delta=0 返回空字符串。 */
export function buildCursorMoveSequence(delta: number): string {
  if (delta === 0) return "";
  if (delta > 0) return `\x1b[${delta}C`;
  return `\x1b[${-delta}D`;
}
