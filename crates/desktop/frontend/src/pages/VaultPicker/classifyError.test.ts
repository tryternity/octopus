import { describe, it, expect } from "vitest";
import { classifyError } from "./classifyError";

/**
 * classifyError 契约测试——锁定与后端 `require_user_vault_key` / `unlock.rs` 的错误字符串。
 *
 * 关键不变量：
 *   - "vault 未解锁" / "locked" → locked（弹解锁框）
 *   - "vault 未初始化" / "not initialized" / "uninitialized" → uninit（提示初始化）
 *   - 其他原样返回（前端展示）
 *   - 非 string 输入也安全（Number / Error / undefined）
 */
describe("classifyError — locked 分支", () => {
  it('"vault 未解锁" → locked', () => {
    expect(classifyError("vault 未解锁")).toEqual({ kind: "locked" });
  });
  it('含 "locked" 字样 → locked（兼容未来英文文案）', () => {
    expect(classifyError("vault is locked")).toEqual({ kind: "locked" });
  });
});

describe("classifyError — uninit 分支", () => {
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
