/** 序号 → 显示标签：1-9 显示数字，10-35 显示 a-z
 * @param index 0-based 序号（第 1 项 = 0）
 */
export function indexLabel(index: number): string {
  if (index <= 8) return String(index + 1);
  return String.fromCharCode(88 + index); // 9→'a'(97), 10→'b'(98), ... 34→'z'(122)
}

/** 显示标签 → 序号（0-based）。无效返回 -1 */
export function labelToIndex(key: string): number {
  if (/^[1-9]$/.test(key)) return parseInt(key, 10) - 1;
  if (/^[a-z]$/.test(key)) return key.charCodeAt(0) - 88; // 'a'→9, 'b'→10, ... 'z'→34
  return -1;
}
