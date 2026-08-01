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

/**
 * 偏移量 → 方向键序列（readline 可识别的输入）。
 *
 * 用方向键（`\x1b[C` 右 / `\x1b[D` 左）重复 delta 次，而非 CUF/CUB（`\x1b[nC`）。
 * 原因：CUF/CUB 是终端显示控制序列，shell readline 不认（字面输出 C/D）；
 * 方向键是 readline 的输入序列，shell 正确识别为光标移动。
 */
export function buildCursorMoveSequence(delta: number): string {
  if (delta === 0) return "";
  if (delta > 0) return "\x1b[C".repeat(delta);
  return "\x1b[D".repeat(-delta);
}
