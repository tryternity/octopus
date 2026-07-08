import { describe, it, expect } from "vitest";
import {
  fmtBytes,
  sparklinePoints,
  newerSnapshot,
  fmtBytesOrDash,
  sparklineDataFromNullable,
} from "./systemStatusMath";

describe("fmtBytes", () => {
  const cases: Array<{ input: number | null | undefined; expected: string; note?: string }> = [
    { input: null, expected: "?", note: "null → ?" },
    { input: undefined, expected: "?", note: "undefined → ?" },
    { input: 0, expected: "0 B" },
    { input: 1023, expected: "1023 B" },
    { input: 1024, expected: "1.0 KB" },
    { input: 1048576, expected: "1.0 MB" },
    { input: 1073741824, expected: "1.00 GB" },
    { input: 1580, expected: "1.5 KB" },
  ];
  for (const c of cases) {
    it(`${JSON.stringify(c.input)} → ${c.expected}${c.note ? ` (${c.note})` : ""}`, () => {
      expect(fmtBytes(c.input)).toBe(c.expected);
    });
  }
});

describe("sparklinePoints", () => {
  it("空数组返回空串", () => {
    expect(sparklinePoints([])).toBe("");
  });
  it("单点返回空串", () => {
    expect(sparklinePoints([5])).toBe("");
  });
  it("正常序列返回点串", () => {
    const pts = sparklinePoints([0, 10]);
    expect(pts).toBe("0.0,32.0 100.0,0.0");
  });
  it("max 选项覆盖数据最大值", () => {
    // 数据 [0, 5]，传 max=10 → y 落在 32/2=16 而非 0
    const pts = sparklinePoints([0, 5], { max: 10 });
    const parts = pts.split(" ");
    expect(parts[1]).toBe("100.0,16.0");
  });
  it("返回点数 == data.length", () => {
    const data = [1, 2, 3, 4, 5];
    const pts = sparklinePoints(data);
    expect(pts.split(" ").length).toBe(data.length);
  });
  it("x 从 0 开始单调递增", () => {
    const data = [3, 7, 2, 9];
    const xs = sparklinePoints(data)
      .split(" ")
      .map((p) => parseFloat(p.split(",")[0]));
    expect(xs[0]).toBe(0);
    for (let i = 1; i < xs.length; i++) {
      expect(xs[i]).toBeGreaterThan(xs[i - 1]);
    }
    // 最后一个点应在右侧（w=100）
    expect(xs[xs.length - 1]).toBeCloseTo(100, 1);
  });
});

describe("newerSnapshot", () => {
  type S = { sampled_at: number; v: number };
  const make = (sampled_at: number, v: number): S => ({ sampled_at, v });

  it("prev=null → next", () => {
    const next = make(100, 2);
    expect(newerSnapshot<S>(null, next)).toBe(next);
  });
  it("next.sampled_at 严格大于 prev → next", () => {
    const prev = make(100, 1);
    const next = make(200, 2);
    expect(newerSnapshot(prev, next)).toBe(next);
  });
  it("next.sampled_at 等于 prev → prev（严格大于）", () => {
    const prev = make(100, 1);
    const next = make(100, 2);
    expect(newerSnapshot(prev, next)).toBe(prev);
  });
  it("next.sampled_at 小于 prev → prev", () => {
    const prev = make(200, 1);
    const next = make(100, 2);
    expect(newerSnapshot(prev, next)).toBe(prev);
  });
});

describe("fmtBytesOrDash", () => {
  it("null → '—'", () => {
    expect(fmtBytesOrDash(null)).toBe("—");
  });
  it("undefined → '—'", () => {
    expect(fmtBytesOrDash(undefined)).toBe("—");
  });
  it("0 → '0 B'", () => {
    expect(fmtBytesOrDash(0)).toBe("0 B");
  });
  it("正数走 fmtBytes", () => {
    expect(fmtBytesOrDash(1048576)).toBe("1.0 MB");
  });
});

describe("sparklineDataFromNullable", () => {
  it("全非 null 数组 → 原样返回", () => {
    expect(sparklineDataFromNullable([1, 2, 3], [9, 9])).toEqual([1, 2, 3]);
  });
  it("含 null → 退 fallback", () => {
    expect(sparklineDataFromNullable([1, null, 3], [9, 9])).toEqual([9, 9]);
  });
  it("混合 null（首尾非 null 中间 null）→ 退 fallback（保守）", () => {
    expect(sparklineDataFromNullable([10, null, 30], [1, 2, 3])).toEqual([1, 2, 3]);
  });
  it("空数组 → 退 fallback", () => {
    expect(sparklineDataFromNullable([], [9, 9])).toEqual([9, 9]);
  });
});
