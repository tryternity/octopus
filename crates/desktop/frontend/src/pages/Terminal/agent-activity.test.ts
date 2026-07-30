import { describe, it, expect } from "vitest";
import { phaseForSignal } from "./agent-activity";

describe("phaseForSignal", () => {
  it("started → working（agent 刚启动）", () => {
    expect(phaseForSignal("started")).toBe("working");
  });

  it("working → working（持续工作）", () => {
    expect(phaseForSignal("working")).toBe("working");
  });

  it("attention → attention（需要用户输入）", () => {
    expect(phaseForSignal("attention")).toBe("attention");
  });

  it("finished → finished（完成）", () => {
    expect(phaseForSignal("finished")).toBe("finished");
  });

  it("exited → exited（agent 退出）", () => {
    expect(phaseForSignal("exited")).toBe("exited");
  });

  it("未知信号 → null（忽略）", () => {
    expect(phaseForSignal("unknown")).toBeNull();
    expect(phaseForSignal("")).toBeNull();
  });
});
