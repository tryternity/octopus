/**
 * VaultPicker 错误分类——把后端 `vault_detect_and_match` 抛出的错误字符串归类到
 * UI 应处理的三种状态：
 *
 *   - "locked"     vault 已初始化但当前会话未解锁 → 显示解锁表单
 *   - "uninit"     vault 未初始化（首次运行） → 显示初始化提示
 *   - 字符串原样   其他错误 → 直接显示给用户
 *
 * 与后端契约（见 crates/desktop/src/vault_commands.rs / crates/vault/src/unlock.rs）：
 *   - require_user_vault_key 返回 `"vault 未解锁"`
 *   - octopus_vault::unlock 内部错误含 `"vault 未初始化"`
 *   - 未来若后端改文案，这里需要同步（grep 关键字 "vault 未解锁" / "vault 未初始化"）
 *
 * 抽成纯函数方便单测（见 classifyError.test.ts）——前端无法对窗口全流程跑 e2e。
 */
export type ClassifiedError =
  | { kind: "locked" }
  | { kind: "uninit" }
  | { kind: "error"; message: string };

export function classifyError(raw: unknown): ClassifiedError {
  const msg = String(raw);
  // 关键字刻意宽松（"未解锁" / "未初始化" 不要求 "vault " 前缀）：
  // 后端错误链可能拼接上下文（anyhow context），固定前缀匹配过于脆弱。
  // locked 优先于 uninit——这是更紧迫的修复路径（用户当前会话失效）。
  if (msg.includes("未解锁") || msg.includes("locked")) {
    return { kind: "locked" };
  }
  if (msg.includes("未初始化") || msg.includes("not initialized") || msg.includes("uninitialized")) {
    return { kind: "uninit" };
  }
  return { kind: "error", message: msg };
}
