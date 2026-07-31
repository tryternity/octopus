/**
 * keymap.ts 纯函数单测——macOS Option/Cmd 组合键 → readline 转义序列。
 */
import { describe, it, expect } from "vitest";
import {
  wordNavigationSequence,
  lineNavigationSequence,
  deleteSequence,
  readlineSequence,
  isShiftEnter,
  type TerminalKeyEvent,
} from "./keymap";

/** 构造测试 KeyboardEvent 字段。 */
function evt(overrides: Partial<TerminalKeyEvent>): TerminalKeyEvent {
  return {
    altKey: false,
    ctrlKey: false,
    metaKey: false,
    shiftKey: false,
    key: "",
    code: "",
    ...overrides,
  };
}

describe("wordNavigationSequence", () => {
  it("Option+Left → \\x1bb（后退一个词）", () => {
    expect(wordNavigationSequence(evt({ altKey: true, key: "ArrowLeft" }))).toBe("\x1bb");
  });
  it("Option+Right → \\x1bf（前进一个词）", () => {
    expect(wordNavigationSequence(evt({ altKey: true, key: "ArrowRight" }))).toBe("\x1bf");
  });
  it("code 兜底（key 为空时用 code）", () => {
    expect(wordNavigationSequence(evt({ altKey: true, code: "ArrowLeft" }))).toBe("\x1bb");
  });
  it("无 Option 返回 null", () => {
    expect(wordNavigationSequence(evt({ key: "ArrowLeft" }))).toBeNull();
  });
  it("Option+Ctrl 干扰返回 null", () => {
    expect(wordNavigationSequence(evt({ altKey: true, ctrlKey: true, key: "ArrowLeft" }))).toBeNull();
  });
  it("Option+其他键返回 null", () => {
    expect(wordNavigationSequence(evt({ altKey: true, key: "a" }))).toBeNull();
  });
});

describe("lineNavigationSequence", () => {
  it("Mac Cmd+Left → \\x01（行首）", () => {
    expect(lineNavigationSequence(evt({ metaKey: true, key: "ArrowLeft" }), { isMac: true })).toBe("\x01");
  });
  it("Mac Cmd+Right → \\x05（行尾）", () => {
    expect(lineNavigationSequence(evt({ metaKey: true, key: "ArrowRight" }), { isMac: true })).toBe("\x05");
  });
  it("非 Mac 返回 null（Cmd 不作导航）", () => {
    expect(lineNavigationSequence(evt({ metaKey: true, key: "ArrowLeft" }), { isMac: false })).toBeNull();
  });
  it("Mac 但有 Ctrl 干扰返回 null", () => {
    expect(lineNavigationSequence(evt({ metaKey: true, ctrlKey: true, key: "ArrowLeft" }), { isMac: true })).toBeNull();
  });
  it("Mac 但无 Cmd 返回 null", () => {
    expect(lineNavigationSequence(evt({ key: "ArrowLeft" }), { isMac: true })).toBeNull();
  });
});

describe("deleteSequence", () => {
  it("Mac Cmd+Backspace → \\x15（删到行首）", () => {
    expect(deleteSequence(evt({ metaKey: true, key: "Backspace" }), { isMac: true })).toBe("\x15");
  });
  it("Mac Option+Backspace → \\x17（删一个词）", () => {
    expect(deleteSequence(evt({ altKey: true, key: "Backspace" }), { isMac: true })).toBe("\x17");
  });
  it("Mac 无修饰 Backspace 返回 null（普通退格交 xterm）", () => {
    expect(deleteSequence(evt({ key: "Backspace" }), { isMac: true })).toBeNull();
  });
  it("非 Mac 返回 null", () => {
    expect(deleteSequence(evt({ metaKey: true, key: "Backspace" }), { isMac: false })).toBeNull();
  });
  it("非 Backspace 键返回 null", () => {
    expect(deleteSequence(evt({ metaKey: true, key: "Enter" }), { isMac: true })).toBeNull();
  });
});

describe("readlineSequence", () => {
  it("alternate screen 返回 null（TUI 全屏，交应用自己处理）", () => {
    expect(readlineSequence(evt({ metaKey: true, key: "ArrowLeft" }), { isMac: true, isAlternateScreen: true })).toBeNull();
  });
  it("聚合行导航（优先）", () => {
    expect(readlineSequence(evt({ metaKey: true, key: "ArrowLeft" }), { isMac: true, isAlternateScreen: false })).toBe("\x01");
  });
  it("聚合词导航", () => {
    expect(readlineSequence(evt({ altKey: true, key: "ArrowLeft" }), { isMac: true, isAlternateScreen: false })).toBe("\x1bb");
  });
  it("聚合删除", () => {
    expect(readlineSequence(evt({ altKey: true, key: "Backspace" }), { isMac: true, isAlternateScreen: false })).toBe("\x17");
  });
  it("无匹配返回 null", () => {
    expect(readlineSequence(evt({ key: "a" }), { isMac: true, isAlternateScreen: false })).toBeNull();
  });
});

describe("isShiftEnter", () => {
  it("Shift+Enter → true", () => {
    expect(isShiftEnter(evt({ key: "Enter", shiftKey: true }))).toBe(true);
  });
  it("纯 Enter → false", () => {
    expect(isShiftEnter(evt({ key: "Enter" }))).toBe(false);
  });
  it("Ctrl+Shift+Enter → false（有其他修饰）", () => {
    expect(isShiftEnter(evt({ key: "Enter", shiftKey: true, ctrlKey: true }))).toBe(false);
  });
});
