/**
 * 列表选中索引移动。delta=-1 上移、+1 下移。
 * 边界夹紧（不循环）：到首/末停止。len=0 或 current 越界时夹紧到有效范围或 null。
 */
export function moveIndex(current: number | null, len: number, delta: number): number | null {
  if (len <= 0) return null;
  // null 初态：直接落到首/末边界，不再额外移动 delta
  if (current === null) {
    return delta > 0 ? 0 : len - 1;
  }
  // 越界夹紧到 [0, len-1] 后再移动
  const start = current >= len ? len - 1 : current < 0 ? 0 : current;
  const next = start + delta;
  return Math.max(0, Math.min(len - 1, next));
}

/**
 * tab 循环切换。delta=-1 左、+1 右。末尾右移绕回首，首位左移绕到末。
 */
export function moveTab(current: number, len: number, delta: number): number {
  return (current + delta + len) % len;
}
