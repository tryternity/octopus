import { describe, it, expect, vi, beforeAll } from "vitest";
import { measureCaretPx, codePointOffsetTo, codePointOffsetBefore, placeCaretAtCodePoint } from "./caret";

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

  it("多文本节点：pos 越过首节点 → setStart 到后续节点", () => {
    // whitespace-pre-wrap 多行 / 编辑残留 <br> 可能使容器含多个 text node；
    // pos 超首节点长度时不应 clamp 到首节点末尾，而应定位到后续节点。
    const el = document.createElement("div");
    const t1 = document.createTextNode("abc");
    const br = document.createElement("br");
    const t2 = document.createTextNode("XYZ");
    el.appendChild(t1); el.appendChild(br); el.appendChild(t2);
    document.body.appendChild(el);
    // total code-point = 3 + 3 = 6；pos=4 落在 t2 第 1 位（4-3=1）
    const spy = vi.spyOn(Range.prototype, "setStart");
    measureCaretPx(el, 4);
    expect(spy.mock.calls[0]![0]).toBe(t2); // node 是第二个文本节点（非首节点）
    expect(spy.mock.calls[0]![1]).toBe(1); // t2 内 UTF-16 offset 1（"X" 前）
    spy.mockRestore();
  });

  it("多文本节点：pos=null → setStart 到最后节点末尾", () => {
    const el = document.createElement("div");
    const t1 = document.createTextNode("abc");
    const br = document.createElement("br");
    const t2 = document.createTextNode("XY");
    el.appendChild(t1); el.appendChild(br); el.appendChild(t2);
    document.body.appendChild(el);
    const spy = vi.spyOn(Range.prototype, "setStart");
    measureCaretPx(el, null);
    expect(spy.mock.calls[0]![0]).toBe(t2);
    expect(spy.mock.calls[0]![1]).toBe(2); // t2 末尾（全文末尾）
    spy.mockRestore();
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

describe("placeCaretAtCodePoint", () => {
  it("定位光标到指定 code-point：setStart 落点 + offset 正确", () => {
    const el = divWithText("abc");
    const spy = vi.spyOn(Range.prototype, "setStart");
    expect(placeCaretAtCodePoint(el, 2)).toBe(true);
    const last = spy.mock.calls.at(-1)!;
    expect(last[0]).toBe(el.firstChild); // text node
    expect(last[1]).toBe(2); // utf16 offset（ASCII = code-point）
    spy.mockRestore();
  });

  it("空容器返回 false", () => {
    const el = document.createElement("div");
    document.body.appendChild(el);
    expect(placeCaretAtCodePoint(el, 0)).toBe(false);
  });

  it("多节点 pos 越界 → 最后节点末尾", () => {
    const el = document.createElement("div");
    const t1 = document.createTextNode("ab");
    const t2 = document.createTextNode("cd");
    el.appendChild(t1); el.appendChild(t2);
    document.body.appendChild(el);
    const spy = vi.spyOn(Range.prototype, "setStart");
    expect(placeCaretAtCodePoint(el, 99)).toBe(true);
    const last = spy.mock.calls.at(-1)!;
    expect(last[0]).toBe(t2);
    expect(last[1]).toBe(2);
    spy.mockRestore();
  });
});
