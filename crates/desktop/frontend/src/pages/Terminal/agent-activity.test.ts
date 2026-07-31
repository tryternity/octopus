import { describe, it, expect } from "vitest";
import { phaseForSignal, displayLabel } from "./agent-activity";

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

describe("displayLabel", () => {
  const fallback = "终端";

  it("customName 优先（用户改名）", () => {
    expect(displayLabel("我的会话", "octopus", "claude", fallback)).toBe("我的会话");
  });

  it("customName 空白 → cwdBasename", () => {
    expect(displayLabel("   ", "octopus", "claude", fallback)).toBe("octopus");
    expect(displayLabel("", "proj", "codex", fallback)).toBe("proj");
  });

  it("无 customName 用 cwdBasename", () => {
    expect(displayLabel(undefined, "myproject", null, fallback)).toBe("myproject");
  });

  it("无 customName 无 cwdBasename 用 agentName", () => {
    expect(displayLabel(undefined, null, "gemini", fallback)).toBe("gemini");
  });

  it("无 customName 无 cwdBasename 无 agentName 用 fallback", () => {
    expect(displayLabel(undefined, null, null, fallback)).toBe(fallback);
    expect(displayLabel("", null, null, fallback)).toBe(fallback);
  });

  it("customName 优先于 cwdBasename + agentName", () => {
    expect(displayLabel("改名", "octopus", "claude", fallback)).toBe("改名");
  });
});
