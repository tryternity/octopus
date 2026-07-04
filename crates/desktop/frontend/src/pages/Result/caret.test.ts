import { describe, it, expect, vi, beforeAll } from "vitest";
import { measureCaretPx, codePointOffsetTo, codePointOffsetBefore } from "./caret";

// jsdom 的 getBoundingClientRect 返回零矩形，测不了真实像素；本测试锁住**逻辑分支
// 与 code-point → UTF-16 offset 对齐**（光标错位/首位 bug 的核心），像素断言留给 e2e。

function divWithText(text: string): HTMLDivElement {
  const el = document.createElement("div");
  el.textContent = text;
  document.body.appendChild(el);
  return el;
}

// measureCaretPx 内唯一的 setStart 调用携带的 UTF-16 offset——用它断言光标逻辑位。
function capturedUtf16Offset(el: HTMLElement, pos: number | null): number {
  const spy = vi.spyOn(Range.prototype, "setStart");
  measureCaretPx(el, pos);
  const offset = spy.mock.calls[0]![1];
  spy.mockRestore();
  return offset;
}

describe("measureCaretPx", () => {
  beforeAll(() => {
    // jsdom 未实现 Range.prototype.getBoundingClientRect（Element 有），defineProperty 补零矩形。
    Object.defineProperty(Range.prototype, "getBoundingClientRect", {
      value: () => ({
        x: 0, y: 0, top: 0, left: 0, right: 0, bottom: 0, width: 0, height: 0,
        toJSON: () => ({}),
      }),
      configurable: true,
    });
  });

  it("null 容器返回 null", () => {
    expect(measureCaretPx(null, 0)).toBeNull();
  });

  it("无文本节点返回兜底结构（height 18）", () => {
    const el = document.createElement("div"); // 空 div，无文本节点
    document.body.appendChild(el);
    expect(measureCaretPx(el, null)).toEqual({ left: 0, top: 0, height: 18 });
  });

  it("pos=null → setStart 收到全文 UTF-16 长度（末尾）", () => {
    const el = divWithText("abc");
    expect(capturedUtf16Offset(el, null)).toBe(3);
  });

  it("pos 超出长度 → clamp 到末尾", () => {
    const el = divWithText("abc");
    expect(capturedUtf16Offset(el, 99)).toBe(3);
  });

  it("代理对：emoji 计 1 个 code-point，UTF-16 offset=2", () => {
    // "😀ab" = 3 code-point；😀 是代理对（UTF-16 length 2）
    const el = divWithText("😀ab");
    expect(capturedUtf16Offset(el, 1)).toBe(2); // 光标在 emoji 后
  });

  it("代理对：pos=2 → UTF-16 offset=3（跨 emoji）", () => {
    const el = divWithText("😀ab");
    expect(capturedUtf16Offset(el, 2)).toBe(3); // 'a' 后
  });
});

describe("codePointOffsetTo / Before", () => {
  it("ASCII：code-point = UTF-16 offset", () => {
    const el = divWithText("abc");
    expect(codePointOffsetTo(el, el.firstChild!, 2)).toBe(2);
  });

  it("代理对：UTF-16 offset 3 → code-point 2（跨 emoji）", () => {
    const el = divWithText("😀ab"); // UTF-16 len 4
    expect(codePointOffsetTo(el, el.firstChild!, 3)).toBe(2); // "😀a" = 2 code-point
  });

  it("codePointOffsetBefore 取 range.start", () => {
    const el = divWithText("abc");
    const range = document.createRange();
    range.setStart(el.firstChild!, 2);
    range.collapse(true);
    expect(codePointOffsetBefore(el, range)).toBe(2);
  });
});
