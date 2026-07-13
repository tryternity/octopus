import { describe, it, expect } from "vitest";
import { parseShortcut, matchShortcut } from "./shortcut";

function mkEvent(key: string, opts: Partial<{
  metaKey: boolean; ctrlKey: boolean; altKey: boolean; shiftKey: boolean;
}> = {}): KeyboardEvent {
  return { key, metaKey: false, ctrlKey: false, altKey: false, shiftKey: false, ...opts } as KeyboardEvent;
}

describe("parseShortcut", () => {
  it("解析 CmdOrCtrl+Enter", () => {
    const sc = parseShortcut("CmdOrCtrl+Enter");
    expect(sc.key).toBe("enter");
    expect(sc.cmdOrCtrl).toBe(true);
    expect(sc.meta).toBe(false);
    expect(sc.ctrl).toBe(false);
  });

  it("解析 Cmd+Shift+S", () => {
    const sc = parseShortcut("Cmd+Shift+S");
    expect(sc.key).toBe("s");
    expect(sc.meta).toBe(true);
    expect(sc.shift).toBe(true);
    expect(sc.cmdOrCtrl).toBe(false);
  });

  it("解析 Ctrl+Alt+T", () => {
    const sc = parseShortcut("Ctrl+Alt+T");
    expect(sc.key).toBe("t");
    expect(sc.ctrl).toBe(true);
    expect(sc.alt).toBe(true);
  });

  it("大小写不敏感", () => {
    const sc = parseShortcut("cmdorctrl+enter");
    expect(sc.cmdOrCtrl).toBe(true);
    expect(sc.key).toBe("enter");
  });
});

describe("matchShortcut", () => {
  it("CmdOrCtrl+Enter 匹配 metaKey", () => {
    const sc = parseShortcut("CmdOrCtrl+Enter");
    expect(matchShortcut(mkEvent("Enter", { metaKey: true }), sc)).toBe(true);
  });

  it("CmdOrCtrl+Enter 匹配 ctrlKey（跨平台）", () => {
    const sc = parseShortcut("CmdOrCtrl+Enter");
    expect(matchShortcut(mkEvent("Enter", { ctrlKey: true }), sc)).toBe(true);
  });

  it("key 不匹配 → false", () => {
    const sc = parseShortcut("CmdOrCtrl+Enter");
    expect(matchShortcut(mkEvent("a", { metaKey: true }), sc)).toBe(false);
  });

  it("缺修饰键 → false", () => {
    const sc = parseShortcut("CmdOrCtrl+Enter");
    expect(matchShortcut(mkEvent("Enter"), sc)).toBe(false);
  });

  it("多余修饰键 → false", () => {
    const sc = parseShortcut("CmdOrCtrl+Enter");
    expect(matchShortcut(mkEvent("Enter", { metaKey: true, shiftKey: true }), sc)).toBe(false);
  });

  it("无快捷键（key 为 undefined）→ false", () => {
    expect(matchShortcut(mkEvent("Enter"), parseShortcut(""))).toBe(false);
  });
});
