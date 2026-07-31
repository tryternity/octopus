import { describe, it, expect } from "vitest";
import { clampPanelWidth } from "./clampPanelWidth";

describe("clampPanelWidth", () => {
  const MIN = 50;
  const TERMINAL_MIN = 320;

  it("正常值不动（在 min 与动态 max 之间）", () => {
    // container=1000, otherSide=240 → max = 1000-320-240 = 440；raw=220 在 [50,440]
    expect(clampPanelWidth(220, MIN, 1000, 240, TERMINAL_MIN)).toBe(220);
  });

  it("低于 min 收到 min", () => {
    expect(clampPanelWidth(30, MIN, 1000, 240, TERMINAL_MIN)).toBe(50);
    expect(clampPanelWidth(0, MIN, 1000, 240, TERMINAL_MIN)).toBe(50);
    expect(clampPanelWidth(-10, MIN, 1000, 240, TERMINAL_MIN)).toBe(50);
  });

  it("超过动态 max 收到 max", () => {
    // container=800, otherSide=240 → max = 800-320-240 = 240；raw=500 超过
    expect(clampPanelWidth(500, MIN, 800, 240, TERMINAL_MIN)).toBe(240);
  });

  it("对侧 panel 隐藏（otherSide=0）时 max 更大", () => {
    // container=800, otherSide=0 → max = 800-320 = 480；raw=600 超过 → 480
    expect(clampPanelWidth(600, MIN, 800, 0, TERMINAL_MIN)).toBe(480);
  });

  it("极小窗口：动态 max < min 时，min 优先（保证手柄可见）", () => {
    // container=400, otherSide=0 → max = 400-320 = 80；raw=200 超过 80
    expect(clampPanelWidth(200, MIN, 400, 0, TERMINAL_MIN)).toBe(80);
    // container=300, otherSide=0 → max = -20 < 50；min 优先
    expect(clampPanelWidth(100, MIN, 300, 0, TERMINAL_MIN)).toBe(50);
  });

  it("NaN raw 回退到 min", () => {
    expect(clampPanelWidth(NaN, MIN, 1000, 0, TERMINAL_MIN)).toBe(50);
  });
});
