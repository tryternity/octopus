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
