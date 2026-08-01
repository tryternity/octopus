import { describe, it, expect } from "vitest";
import { pixelToCol, shouldMoveCursor, buildCursorMoveSequence } from "./clickCursor";

describe("pixelToCol", () => {
  it("像素坐标 → 列号（整除）", () => {
    // rect.left=100, width=800, cols=80 → cellWidth=10
    // clientX=145 → (145-100)/10 = 4.5 → floor = 4
    expect(pixelToCol(145, 100, 800, 80)).toBe(4);
  });

  it("左边缘 → 第 0 列", () => {
    expect(pixelToCol(100, 100, 800, 80)).toBe(0);
  });

  it("右边缘 → 最后一列", () => {
    // clientX=899 → (899-100)/10 = 79.9 → floor = 79
    expect(pixelToCol(899, 100, 800, 80)).toBe(79);
  });

  it("超出右边缘 → clamp 到最后一列", () => {
    expect(pixelToCol(1000, 100, 800, 80)).toBe(79);
  });

  it("超出左边缘 → clamp 到 0", () => {
    expect(pixelToCol(50, 100, 800, 80)).toBe(0);
  });
});

describe("shouldMoveCursor", () => {
  const base = { inCommand: false, bufferType: "normal" as const, clickRow: 5, cursorY: 5 };
  it("全满足 → true", () => {
    expect(shouldMoveCursor(base)).toBe(true);
  });
  it("inCommand=true → false（命令执行中）", () => {
    expect(shouldMoveCursor({ ...base, inCommand: true })).toBe(false);
  });
  it("bufferType=alternate → false（TUI 全屏）", () => {
    expect(shouldMoveCursor({ ...base, bufferType: "alternate" })).toBe(false);
  });
  it("clickRow != cursorY → false（非当前行）", () => {
    expect(shouldMoveCursor({ ...base, clickRow: 3, cursorY: 5 })).toBe(false);
  });
});

describe("buildCursorMoveSequence", () => {
  it("delta>0 → CUF 右移", () => {
    expect(buildCursorMoveSequence(5)).toBe("\x1b[5C");
  });
  it("delta<0 → CUB 左移", () => {
    expect(buildCursorMoveSequence(-3)).toBe("\x1b[3D");
  });
  it("delta=0 → 空字符串（不动）", () => {
    expect(buildCursorMoveSequence(0)).toBe("");
  });
  it("delta=1 → 单步右移", () => {
    expect(buildCursorMoveSequence(1)).toBe("\x1b[1C");
  });
});
