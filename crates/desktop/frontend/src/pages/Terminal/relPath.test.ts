import { describe, it, expect } from "vitest";
import { relPath } from "./relPath";

describe("relPath", () => {
  it("子树内：去 cwd 前缀得相对路径", () => {
    expect(relPath("/proj/src/a.ts", "/proj")).toBe("src/a.ts");
  });

  it("多层子树", () => {
    expect(relPath("/proj/src/sub/b.ts", "/proj")).toBe("src/sub/b.ts");
  });

  it("等于 cwd 本身 → '.'", () => {
    expect(relPath("/proj", "/proj")).toBe(".");
  });

  it("外部文件 → 回退绝对路径", () => {
    expect(relPath("/other/file", "/proj")).toBe("/other/file");
  });

  it("父目录 → 回退绝对路径（避免 ../../）", () => {
    expect(relPath("/proj", "/other")).toBe("/proj");
  });

  it("cwd 尾斜杠规范化后匹配", () => {
    expect(relPath("/proj/src/a.ts", "/proj/")).toBe("src/a.ts");
  });

  it("cwd 多个尾斜杠规范化", () => {
    expect(relPath("/proj/src/a.ts", "/proj//")).toBe("src/a.ts");
  });

  it("cwd 为空 → 回退 fullPath", () => {
    expect(relPath("/proj/file", "")).toBe("/proj/file");
  });

  it("含空格的子树路径：relPath 不转义（留给 shellEscape）", () => {
    expect(relPath("/proj/my dir/a.ts", "/proj")).toBe("my dir/a.ts");
  });
});
