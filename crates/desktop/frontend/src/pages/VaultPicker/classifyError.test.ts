import { describe, it, expect } from "vitest";
import { classifyError } from "./classifyError";

/**
 * classifyError 契约测试——锁定与后端 `vault_*` 命令的错误字符串。
 *
 * follow-up #9 起后端返回 JSON `{ code, message }`，旧版返回裸字符串。
 * 关键不变量：
 *   - 新格式 JSON `{ code: "locked" }` → locked
 *   - 新格式 JSON `{ code: "not_initialized" }` → uninit
 *   - 新格式 JSON `{ code: "invalid_master_password" }` → locked（重输路径）
 *   - 新格式 JSON `{ code: "keychain_unavailable" }` → locked（用主密码解锁）
 *   - 新格式其它 code → 显示稳定 message
 *   - 旧格式 "vault 未解锁" / "locked" → locked（向后兼容）
 *   - 旧格式 "vault 未初始化" / "not initialized" / "uninitialized" → uninit
 *   - 其他原样返回（前端展示）
 *   - 非 string 输入也安全（Number / Error / undefined）
 */

// === 新格式 JSON { code, message } ===

describe("classifyError — JSON code 映射（follow-up #9 新契约）", () => {
  it('code="locked" → locked', () => {
    const r = classifyError(JSON.stringify({ code: "locked", message: "密码库已锁定" }));
    expect(r).toEqual({ kind: "locked" });
  });
  it('code="not_initialized" → uninit', () => {
    const r = classifyError(
      JSON.stringify({ code: "not_initialized", message: "密码库尚未初始化" }),
    );
    expect(r).toEqual({ kind: "uninit" });
  });
  it('code="invalid_master_password" → locked（让用户重输）', () => {
    const r = classifyError(
      JSON.stringify({ code: "invalid_master_password", message: "主密码错误" }),
    );
    expect(r).toEqual({ kind: "locked" });
  });
  it('code="keychain_unavailable" → locked（提示用主密码解锁）', () => {
    const r = classifyError(
      JSON.stringify({ code: "keychain_unavailable", message: "系统密钥串不可用" }),
    );
    expect(r).toEqual({ kind: "locked" });
  });
  it('code="cipher_not_found" → 显示稳定 message', () => {
    const r = classifyError(
      JSON.stringify({ code: "cipher_not_found", message: "未找到该密码条目" }),
    );
    expect(r).toEqual({ kind: "error", message: "未找到该密码条目" });
  });
  it('code="totp_invalid" → 显示稳定 message', () => {
    const r = classifyError(
      JSON.stringify({ code: "totp_invalid", message: "TOTP 密钥格式无效" }),
    );
    expect(r.kind).toBe("error");
  });
  it('code="internal_error" → 显示稳定 message（不含内部细节）', () => {
    const r = classifyError(
      JSON.stringify({ code: "internal_error", message: "内部错误，请重试" }),
    );
    expect(r.kind).toBe("error");
    // 关键不变量：internal_error 的 message 绝不应包含路径/SQL/加密细节
    if (r.kind === "error") {
      expect(r.message).not.toMatch(/\.sqlite|SQL|ChaCha|nonce/i);
    }
  });
  it("未知 code → 显示 message（或退回 code 字符串）", () => {
    const r = classifyError(JSON.stringify({ code: "future_code", message: "未来错误" }));
    expect(r).toEqual({ kind: "error", message: "未来错误" });
  });
  it("JSON 缺 message → 退回 code 作为 message", () => {
    const r = classifyError(JSON.stringify({ code: "future_code" }));
    expect(r).toEqual({ kind: "error", message: "future_code" });
  });
  it("JSON code 不是 string → 退回 legacy 字符串匹配", () => {
    // code 是 number → 不符合契约 → 走 legacy（原文不含 locked/uninit 关键字 → error）
    const r = classifyError(JSON.stringify({ code: 42 }));
    expect(r.kind).toBe("error");
  });
  it('JSON 前后含杂质（Error: 前缀）→ 走 legacy 字符串匹配也能识别', () => {
    // Tauri 偶尔会把 err 包成 Error 对象，toString 含 "Error: " 前缀
    const r = classifyError('Error: {"code":"locked","message":"..."}');
    // 注意：当前实现只识别以 "{" 开头的（trim 后）—— "Error: {" 走 legacy
    // 这里验证 legacy 兜底也能识别（含 "locked" 关键字）
    expect(r).toEqual({ kind: "locked" });
  });
});

// === Legacy 字符串匹配（pre-#9 向后兼容）===

describe("classifyError — legacy 字符串匹配（向后兼容）", () => {
  it('"vault 未解锁" → locked', () => {
    expect(classifyError("vault 未解锁")).toEqual({ kind: "locked" });
  });
  it('含 "locked" 字样 → locked（兼容未来英文文案）', () => {
    expect(classifyError("vault is locked")).toEqual({ kind: "locked" });
  });
  it('"vault 未初始化" → uninit', () => {
    expect(classifyError("vault 未初始化")).toEqual({ kind: "uninit" });
  });
  it('"not initialized" → uninit', () => {
    expect(classifyError("Error: vault is not initialized")).toEqual({ kind: "uninit" });
  });
  it('"uninitialized" → uninit', () => {
    expect(classifyError("vault uninitialized")).toEqual({ kind: "uninit" });
  });
});

describe("classifyError — 其他错误原样返回", () => {
  it('含 URL 解析失败等 → 原样返回 message', () => {
    const r = classifyError("URL 解析失败: invalid");
    expect(r.kind).toBe("error");
    expect(r.kind === "error" && r.message).toBe("URL 解析失败: invalid");
  });
  it('空字符串 → error（不应被误判为 locked/uninit）', () => {
    const r = classifyError("");
    expect(r).toEqual({ kind: "error", message: "" });
  });
});

describe("classifyError — 非 string 输入", () => {
  it("Number → 转 string 后分类", () => {
    const r = classifyError(42);
    expect(r).toEqual({ kind: "error", message: "42" });
  });
  it("Error 对象 → 转 string（含 'Error: ' 前缀 + message）", () => {
    const r = classifyError(new Error("boom"));
    expect(r.kind).toBe("error");
    if (r.kind === "error") {
      expect(r.message).toContain("boom");
    }
  });
  it("undefined → 不抛异常", () => {
    const r = classifyError(undefined);
    expect(r.kind).toBe("error");
  });
  it("null → 不抛异常", () => {
    const r = classifyError(null);
    expect(r.kind).toBe("error");
  });
});

describe("classifyError — 边界 / 优先级", () => {
  // 当字符串同时含 "未解锁" 与 "未初始化"（理论上不该出现，但定义优先级）：
  // locked 优先于 uninit（更常见、更紧迫的修复路径）。
  it('同时含两类关键字时 → locked 优先', () => {
    const r = classifyError("vault 既未解锁也未初始化");
    expect(r).toEqual({ kind: "locked" });
  });
});
