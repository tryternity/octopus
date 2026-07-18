/**
 * VaultPicker 错误分类——把后端 `vault_*` 抛出的错误归类到 UI 应处理的状态：
 *
 *   - "locked"     vault 已初始化但当前会话未解锁 → 显示解锁表单
 *   - "uninit"     vault 未初始化（首次运行） → 显示初始化提示
 *   - 字符串原样   其他错误 → 直接显示给用户
 *
 * 与后端契约（见 crates/desktop/src/vault_error.rs + vault_commands.rs）：
 *
 *   - follow-up #9 起后端统一返回 JSON 字符串：`{ code, message }`
 *     - code 是稳定的枚举值（"locked" / "not_initialized" / ...）
 *     - message 是用户安全的中文文案（不含内部细节）
 *   - 旧版本（pre-#9）返回裸错误字符串，含中文关键字（"未解锁" / "未初始化"）。
 *
 * 本模块先尝试 JSON parse（新格式），失败则退回字符串关键字匹配（向后兼容）。
 * 抽成纯函数方便单测（见 classifyError.test.ts）——前端无法对窗口全流程跑 e2e。
 */
export type ClassifiedError =
  | { kind: "locked" }
  | { kind: "uninit" }
  | { kind: "error"; message: string };

/** 后端 VaultError.code（见 crates/desktop/src/vault_error.rs） */
type VaultErrorCode =
  | "not_initialized"
  | "locked"
  | "invalid_master_password"
  | "cipher_not_found"
  | "invalid_input"
  | "totp_invalid"
  | "import_failed"
  | "keychain_unavailable"
  | "clipboard_error"
  | "autotype_failed"
  | "internal_error";

/**
 * 把后端 VaultError.code 映射到前端 ViewState。
 *
 * locked 包含两类：
 *   - "locked"（vault 已初始化但未解锁）
 *   - "keychain_unavailable"（系统密钥串不可用——用户可改用主密码解锁）
 *   - "invalid_master_password"（unlock 表单中输错密码——保持 locked 视图让用户重输）
 *
 * 其它 code 一律当 generic error，把后端的稳定 message 直接展示。
 */
function mapCode(code: string, message: string): ClassifiedError {
  switch (code as VaultErrorCode) {
    case "not_initialized":
      return { kind: "uninit" };
    case "locked":
    case "keychain_unavailable":
    case "invalid_master_password":
      // 三种都让用户走解锁表单路径（keychain 不可用 → 用主密码；密码错 → 重输）
      return { kind: "locked" };
    default:
      // 其它 code（cipher_not_found / totp_invalid / import_failed / clipboard_error /
      // autotype_failed / invalid_input / internal_error / 未知 code）→ 显示稳定 message
      return { kind: "error", message: message || code };
  }
}

export function classifyError(raw: unknown): ClassifiedError {
  const str = String(raw);

  // 1) 新契约：JSON `{ code, message }`
  //    Tauri 命令 reject 时 err 通常是 string，但也可能是 Error 对象（其 toString()
  //    含 "Error: " 前缀）——JSON.parse 容忍前缀空白但不容忍前导文字，故先 trim。
  const trimmed = str.trim();
  if (trimmed.startsWith("{")) {
    try {
      const parsed = JSON.parse(trimmed) as { code?: unknown; message?: unknown };
      if (
        typeof parsed.code === "string" &&
        (typeof parsed.message === "string" || parsed.message === undefined)
      ) {
        return mapCode(parsed.code, typeof parsed.message === "string" ? parsed.message : "");
      }
    } catch {
      // 不是合法 JSON → 落到 legacy 字符串匹配
    }
  }

  // 2) Legacy（pre-#9）：字符串关键字匹配
  //    关键字刻意宽松（"未解锁" / "未初始化" 不要求 "vault " 前缀）：
  //    后端错误链可能拼接上下文（anyhow context），固定前缀匹配过于脆弱。
  //    locked 优先于 uninit——这是更紧迫的修复路径（用户当前会话失效）。
  if (str.includes("未解锁") || str.includes("locked")) {
    return { kind: "locked" };
  }
  if (
    str.includes("未初始化") ||
    str.includes("not initialized") ||
    str.includes("uninitialized")
  ) {
    return { kind: "uninit" };
  }

  // 3) 其它原样返回（前端展示）
  return { kind: "error", message: str };
}
