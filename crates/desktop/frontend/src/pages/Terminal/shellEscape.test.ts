import { describe, it, expect } from "vitest";
import { shellEscape } from "./shellEscape";

describe("shellEscape", () => {
  it("无特殊字符 → 原样（更可读）", () => {
    expect(shellEscape("file.txt")).toBe("file.txt");
    expect(shellEscape("src/a.ts")).toBe("src/a.ts");
  });

  it("含空格 → 单引号包裹", () => {
    expect(shellEscape("my file.txt")).toBe("'my file.txt'");
  });

  it("含单引号 → 单引号包裹 + 单引号转义（POSIX '\"'\"' 法）", () => {
    expect(shellEscape("it's.txt")).toBe("'it'\"'\"'s.txt'");
  });

  it("含 $ → 单引号包裹防变量展开", () => {
    expect(shellEscape("a$b.txt")).toBe("'a$b.txt'");
  });

  it("路径分隔符 / 是安全字符 → 原样", () => {
    expect(shellEscape("path/to/file")).toBe("path/to/file");
  });

  it("空字符串 → 原样（空无需转义）", () => {
    expect(shellEscape("")).toBe("");
  });

  it("安全字符集（字母数字 _ . / @ : -）→ 原样", () => {
    expect(shellEscape("a.b-c_d@e:f/g")).toBe("a.b-c_d@e:f/g");
  });
});
