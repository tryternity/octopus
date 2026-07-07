import { describe, it, expect } from "vitest";
import { moveIndex, moveTab } from "./clipboardNav";

describe("moveIndex", () => {
  const cases: Array<{ current: number | null; len: number; delta: number; want: number | null; note?: string }> = [
    // 正常移动
    { current: 0, len: 5, delta: 1, want: 1, note: "向下" },
    { current: 3, len: 5, delta: -1, want: 2, note: "向上" },
    // 边界夹紧（不循环）
    { current: 0, len: 5, delta: -1, want: 0, note: "首位再上停住" },
    { current: 4, len: 5, delta: 1, want: 4, note: "末位再下停住" },
    // null 初态
    { current: null, len: 5, delta: 1, want: 0, note: "null 向下落到首条" },
    { current: null, len: 5, delta: -1, want: 4, note: "null 向上落到末条" },
    // 空列表
    { current: null, len: 0, delta: 1, want: null, note: "空列表保持 null" },
    { current: 2, len: 0, delta: -1, want: null, note: "列表变空夹紧到 null" },
    // 越界夹紧（列表缩短后 current 超出）
    { current: 5, len: 3, delta: 1, want: 2, note: "current 越界向下夹到末位" },
    { current: 5, len: 3, delta: -1, want: 1, note: "current 越界向上从夹紧位置继续" },
  ];
  for (const c of cases) {
    it(`${c.note ?? "move"}: current=${c.current} len=${c.len} delta=${c.delta} → ${c.want}`, () => {
      expect(moveIndex(c.current, c.len, c.delta)).toBe(c.want);
    });
  }
});

describe("moveTab", () => {
  const len = 7;
  const cases: Array<{ current: number; delta: number; want: number; note?: string }> = [
    { current: 0, delta: 1, want: 1, note: "右移" },
    { current: 3, delta: -1, want: 2, note: "左移" },
    { current: 6, delta: 1, want: 0, note: "末位右移绕回首" },
    { current: 0, delta: -1, want: 6, note: "首位左移绕到末" },
  ];
  for (const c of cases) {
    it(`${c.note}: current=${c.current} delta=${c.delta} → ${c.want}`, () => {
      expect(moveTab(c.current, len, c.delta)).toBe(c.want);
    });
  }
});
