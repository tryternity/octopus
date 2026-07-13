import { describe, it, expect } from "vitest";

// 从 index.tsx 提取的纯函数（组件内未导出，此处复制逻辑做不变量测试）
function indexLabel(index: number): string {
  if (index <= 8) return String(index + 1);
  return String.fromCharCode(88 + index);
}

function labelToIndex(key: string): number {
  if (/^[1-9]$/.test(key)) return parseInt(key, 10) - 1;
  if (/^[a-z]$/.test(key)) return key.charCodeAt(0) - 88;
  return -1;
}

describe("indexLabel / labelToIndex", () => {
  it("前 9 项显示数字 1-9", () => {
    expect(indexLabel(0)).toBe("1");
    expect(indexLabel(8)).toBe("9");
  });

  it("第 10 项起显示字母 a-z", () => {
    expect(indexLabel(9)).toBe("a");
    expect(indexLabel(10)).toBe("b");
    expect(indexLabel(34)).toBe("z");
  });

  it("labelToIndex 正确反解数字", () => {
    expect(labelToIndex("1")).toBe(0);
    expect(labelToIndex("9")).toBe(8);
  });

  it("labelToIndex 正确反解字母", () => {
    expect(labelToIndex("a")).toBe(9);
    expect(labelToIndex("z")).toBe(34);
  });

  it("labelToIndex 对非法输入返回 -1", () => {
    expect(labelToIndex("0")).toBe(-1);
    expect(labelToIndex("!")).toBe(-1);
    expect(labelToIndex("")).toBe(-1);
    expect(labelToIndex("ab")).toBe(-1);
  });

  it("roundtrip 不变量：labelToIndex(indexLabel(i)) === i 对 0..34 全成立", () => {
    for (let i = 0; i < 35; i++) {
      expect(labelToIndex(indexLabel(i))).toBe(i);
    }
  });
});
