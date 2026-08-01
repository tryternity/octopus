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
