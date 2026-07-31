/**
 * osc-handlers.ts 纯函数单测——parseOsc7 / cwdBasename / updateShellIntegration。
 */
import { describe, it, expect } from "vitest";
import { parseOsc7, cwdBasename, updateShellIntegration, createShellIntegrationState } from "./osc-handlers";

describe("parseOsc7", () => {
  it("正常 file://host/path → /path", () => {
    expect(parseOsc7("file://localhost/Users/foo")).toBe("/Users/foo");
    expect(parseOsc7("file://myhost/home/user/proj")).toBe("/home/user/proj");
  });

  it("percent-encode 路径 → decode", () => {
    expect(parseOsc7("file://host/Users/%E6%B5%8B%E8%AF%95/proj")).toBe("/Users/测试/proj");
    expect(parseOsc7("file://host/Users/my%20project")).toBe("/Users/my project");
  });

  it("根路径 → /", () => {
    expect(parseOsc7("file://host/")).toBe("/");
  });

  it("无效格式 → null", () => {
    expect(parseOsc7("not-a-url")).toBeNull();
    expect(parseOsc7("http://host/path")).toBeNull();
    expect(parseOsc7("")).toBeNull();
    expect(parseOsc7("file://host")).toBeNull(); // 无 path
  });
});

describe("cwdBasename", () => {
  it("正常路径 → 最后一级目录名", () => {
    expect(cwdBasename("/Users/foo/projects/octopus")).toBe("octopus");
    expect(cwdBasename("/home/user")).toBe("user");
  });

  it("根路径 → null", () => {
    expect(cwdBasename("/")).toBeNull();
  });

  it("null → null", () => {
    expect(cwdBasename(null)).toBeNull();
  });

  it("尾部斜杠 → 忽略空段", () => {
    expect(cwdBasename("/Users/foo/")).toBe("foo");
  });
});

describe("updateShellIntegration", () => {
  it("OSC 133 A（prompt 开始）→ inCommand=false", () => {
    const state = createShellIntegrationState();
    state.inCommand = true;
    updateShellIntegration(state, "A");
    expect(state.inCommand).toBe(false);
  });

  it("OSC 133 D（命令结束）→ inCommand=false", () => {
    const state = createShellIntegrationState();
    state.inCommand = true;
    updateShellIntegration(state, "D;0");
    expect(state.inCommand).toBe(false);
  });

  it("OSC 133 B（命令开始）→ inCommand=true", () => {
    const state = createShellIntegrationState();
    updateShellIntegration(state, "B");
    expect(state.inCommand).toBe(true);
  });

  it("OSC 133 C（pre-exec）→ inCommand=true", () => {
    const state = createShellIntegrationState();
    updateShellIntegration(state, "C;claude");
    expect(state.inCommand).toBe(true);
  });

  it("完整周期：A→B→D→A", () => {
    const state = createShellIntegrationState();
    updateShellIntegration(state, "A"); expect(state.inCommand).toBe(false);
    updateShellIntegration(state, "B"); expect(state.inCommand).toBe(true);
    updateShellIntegration(state, "D;0"); expect(state.inCommand).toBe(false);
    updateShellIntegration(state, "A"); expect(state.inCommand).toBe(false);
  });
});
