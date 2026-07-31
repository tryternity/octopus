/**
 * OSC 序列处理——OSC 7 cwd 追踪 + OSC 133 prompt tracker（安全过滤）。
 *
 * 参考 Terax osc-handlers.ts，macOS-only（路径解析不做 Windows drive 映射）。
 *
 * OSC 7：shell precmd hook 发 `file://host/<urlencoded_pwd>`，
 * 前端解析出 cwd 用于新 tab 继承 + 标题显示。
 *
 * 安全过滤：命令执行期间（OSC 133 B→D/A）忽略 OSC 7——命令 stdout/stderr
 * 不可信（SSH/恶意文件可伪造 OSC 7），只有 shell precmd（命令间）发的才可信。
 */

import type { Terminal } from "@xterm/xterm";

/**
 * 从 OSC 7 payload 解析 cwd（pure，可单测）。
 *
 * 格式：`file://host/path` → `/path`（percent-decode）。
 * 无效格式返回 null。
 */
export function parseOsc7(data: string): string | null {
  const m = data.match(/^file:\/\/[^/]*(\/.*)$/);
  if (!m) return null;
  try {
    return decodeURIComponent(m[1]);
  } catch {
    return m[1];
  }
}

/** 从 cwd 提取 basename（目录名，用于标题显示）。pure。 */
export function cwdBasename(cwd: string | null): string | null {
  if (!cwd) return null;
  const parts = cwd.split("/").filter(Boolean);
  return parts.length > 0 ? parts[parts.length - 1] : null;
}

/** Shell 集成状态——追踪是否在命令执行中（安全过滤用）。 */
export type ShellIntegrationState = { inCommand: boolean };

export function createShellIntegrationState(): ShellIntegrationState {
  return { inCommand: false };
}

/**
 * 更新 ShellIntegrationState（pure，可单测）。
 *
 * OSC 133 标记语义：
 * - A（prompt 开始）/ D（命令结束）→ inCommand = false（允许 OSC 7）
 * - B（命令开始）/ C（pre-exec）→ inCommand = true（忽略 OSC 7）
 */
export function updateShellIntegration(
  state: ShellIntegrationState,
  osc133Data: string,
): void {
  if (osc133Data.startsWith("A") || osc133Data.startsWith("D")) {
    state.inCommand = false;
  } else if (osc133Data.startsWith("B") || osc133Data.startsWith("C")) {
    state.inCommand = true;
  }
}

/**
 * 注册 OSC 7 cwd handler。
 *
 * 命令执行期间（state.inCommand）忽略 OSC 7——防 SSH/恶意文件伪造 cwd。
 *
 * @returns dispose 函数（取消注册）
 */
export function registerCwdHandler(
  term: Terminal,
  onCwd: (cwd: string) => void,
  state?: ShellIntegrationState,
): () => void {
  const d = term.parser.registerOscHandler(7, (data) => {
    if (state?.inCommand) return true;
    const cwd = parseOsc7(data);
    if (cwd) onCwd(cwd);
    return true;
  });
  return () => d.dispose();
}

/**
 * 注册 OSC 133 prompt tracker——更新 inCommand 状态。
 *
 * @returns dispose 函数
 */
export function registerPromptTracker(
  term: Terminal,
  state: ShellIntegrationState,
): () => void {
  const d = term.parser.registerOscHandler(133, (data) => {
    updateShellIntegration(state, data);
    return true;
  });
  return () => d.dispose();
}
