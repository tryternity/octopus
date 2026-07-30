import { describe, it, expect } from "vitest";
import { pickSubIdx, decideKeyAction } from "./keyNavigation";
import type { ActionBarItem } from "./types";
import type { KeyContext } from "./keyNavigation";

/** 构造 ActionBarItem 的 helper（测试用，填默认值） */
function item(over: Partial<ActionBarItem> & { id: number }): ActionBarItem {
  return {
    parentId: null, title: "", icon: "", actionType: "", actionData: "",
    sortOrder: 0, isSystem: false, isEnabled: true, ...over,
  };
}

describe("pickSubIdx", () => {
  it("空列表 → 0", () => {
    expect(pickSubIdx([], "google")).toBe(0);
  });

  it("首项非 url 类型 → 0", () => {
    const subs = [item({ id: 1, actionType: "menu" })];
    expect(pickSubIdx(subs, "google")).toBe(0);
  });

  it("首项是 url，title 匹配 searchEngine（小写） → 返回匹配 idx", () => {
    const subs = [
      item({ id: 1, actionType: "url", title: "Google" }),
      item({ id: 2, actionType: "url", title: "Bing" }),
    ];
    expect(pickSubIdx(subs, "google")).toBe(0);
    expect(pickSubIdx(subs, "bing")).toBe(1);
  });

  it("首项是 url，无匹配 → 0", () => {
    const subs = [
      item({ id: 1, actionType: "url", title: "Google" }),
      item({ id: 2, actionType: "url", title: "Bing" }),
    ];
    expect(pickSubIdx(subs, "duckduckgo")).toBe(0);
  });
});

/** 构造 KeyContext 的 helper（测试用，填默认 menu 模式） */
function ctx(over: Partial<KeyContext> = {}): KeyContext {
  return {
    mode: "menu", view: "main", focusLayer: "main", query: "",
    mainItems: [], subItems: [], menuItems: [],
    selectedIdx: 0, subSelectedIdx: 0, searchSelectedIdx: 0,
    activeTab: "all", hasContext: false, searchEngine: "google",
    lastImeKeyTime: 0, searchResultsCount: 0,
    ...over,
  };
}

/** 构造 KeyboardEvent 的 helper */
function key(over: Partial<KeyboardEventInit> & { key: string }): KeyboardEvent {
  return new KeyboardEvent("keydown", {
    bubbles: true, cancelable: true,
    keyCode: 0, altKey: false, metaKey: false, ctrlKey: false,
    shiftKey: false, code: "", isComposing: false,
    ...over,
  });
}

describe("decideKeyAction - 公共前置", () => {
  it("IME 组合中（keyCode 229）→ ime-composing", () => {
    expect(decideKeyAction(key({ key: "a", keyCode: 229 }), ctx())).toEqual({ type: "ime-composing" });
  });
  it("IME 组合中（isComposing）→ ime-composing", () => {
    expect(decideKeyAction(key({ key: "a", isComposing: true }), ctx())).toEqual({ type: "ime-composing" });
  });
  it("Enter 在 IME 后 500ms 内 → ime-confirm-enter", () => {
    expect(decideKeyAction(key({ key: "Enter" }), ctx({ lastImeKeyTime: Date.now() - 100 }))).toEqual({ type: "ime-confirm-enter" });
  });
  it("loading 视图 → ignore", () => {
    expect(decideKeyAction(key({ key: "Tab" }), ctx({ view: "loading" }))).toEqual({ type: "ignore" });
  });
  it("无修饰可打印字符 → passthrough", () => {
    expect(decideKeyAction(key({ key: "a" }), ctx())).toEqual({ type: "passthrough" });
  });
});

describe("decideKeyAction - Escape", () => {
  it("search 模式 → escape-clear-query", () => {
    expect(decideKeyAction(key({ key: "Escape" }), ctx({ mode: "search" }))).toEqual({ type: "escape-clear-query" });
  });
  it("menu 模式 → escape-dismiss", () => {
    expect(decideKeyAction(key({ key: "Escape" }), ctx({ mode: "menu" }))).toEqual({ type: "escape-dismiss" });
  });
});

describe("decideKeyAction - 搜索模式", () => {
  const sctx = () => ctx({ mode: "search", searchResultsCount: 3 });
  it("Tab → search-tab dir=1", () => {
    expect(decideKeyAction(key({ key: "Tab" }), sctx())).toEqual({ type: "search-tab", dir: 1 });
  });
  it("Shift+Tab → search-tab dir=-1", () => {
    expect(decideKeyAction(key({ key: "Tab", shiftKey: true }), sctx())).toEqual({ type: "search-tab", dir: -1 });
  });
  it("ArrowDown → search-nav dir=1", () => {
    expect(decideKeyAction(key({ key: "ArrowDown" }), sctx())).toEqual({ type: "search-nav", dir: 1 });
  });
  it("ArrowUp → search-nav dir=-1", () => {
    expect(decideKeyAction(key({ key: "ArrowUp" }), sctx())).toEqual({ type: "search-nav", dir: -1 });
  });
  it("Enter → search-enter", () => {
    expect(decideKeyAction(key({ key: "Enter" }), ctx({ mode: "search", lastImeKeyTime: 0 }))).toEqual({ type: "search-enter" });
  });
});

describe("decideKeyAction - 菜单模式移动", () => {
  const items: ActionBarItem[] = [
    item({ id: 1, actionType: "submenu" }),
    item({ id: 2, actionType: "menu" }),
  ];
  it("Tab 焦点 main → menu-move（展开判断交给 hook）", () => {
    const a = decideKeyAction(key({ key: "Tab" }), ctx({ mainItems: items }));
    expect(a).toEqual({ type: "menu-move", forward: true });
  });
  it("Shift+Tab 焦点 main → menu-move forward=false", () => {
    const a = decideKeyAction(key({ key: "Tab", shiftKey: true }), ctx({ mainItems: items }));
    expect(a).toEqual({ type: "menu-move", forward: false });
  });
  it("Tab 焦点 sub → menu-move", () => {
    const a = decideKeyAction(key({ key: "Tab" }), ctx({ focusLayer: "sub" }));
    expect(a).toEqual({ type: "menu-move", forward: true });
  });
});

describe("decideKeyAction - 菜单模式切层", () => {
  const items: ActionBarItem[] = [item({ id: 1, actionType: "submenu" })];
  it("ArrowDown 焦点 sub → menu-toggle-layer", () => {
    expect(decideKeyAction(key({ key: "ArrowDown" }), ctx({ focusLayer: "sub" }))).toEqual({ type: "menu-toggle-layer" });
  });
  it("ArrowUp 焦点 main 当前项 submenu → menu-toggle-layer", () => {
    expect(decideKeyAction(key({ key: "ArrowUp" }), ctx({ mainItems: items, selectedIdx: 0 }))).toEqual({ type: "menu-toggle-layer" });
  });
  it("ArrowDown 焦点 main 当前项非 submenu → menu-toggle-layer（hook 内 no-op）", () => {
    expect(decideKeyAction(key({ key: "ArrowDown" }), ctx({ mainItems: [item({ id: 2, actionType: "menu" })], selectedIdx: 0 }))).toEqual({ type: "menu-toggle-layer" });
  });
});

describe("decideKeyAction - Alt 快捷键", () => {
  it("Alt+字母命中 → alt-execute", () => {
    const it1 = item({ id: 1, shortcut: "h" });
    const a = decideKeyAction(key({ key: "˙", altKey: true, code: "KeyH" }), ctx({ menuItems: [it1] }));
    expect(a.type).toBe("alt-execute");
    expect((a as any).item).toBe(it1);
  });
  it("Alt+字母未命中 → swallow（preventDefault 但不执行）", () => {
    expect(decideKeyAction(key({ key: "˙", altKey: true, code: "KeyH" }), ctx({ menuItems: [] }))).toEqual({ type: "swallow" });
  });
  it("Alt+数字越界 → swallow（preventDefault 但不做事）", () => {
    // mainItems 为空，idx=0 越界
    expect(decideKeyAction(key({ key: "1", altKey: true, code: "Digit1" }), ctx({ mainItems: [] }))).toEqual({ type: "swallow" });
  });
  it("Alt+数字焦点 sub → alt-goto-sub", () => {
    expect(decideKeyAction(key({ key: "1", altKey: true, code: "Digit1" }), ctx({ focusLayer: "sub" }))).toEqual({ type: "alt-goto-sub", idx: 0 });
  });
  it("Alt+数字焦点 main 命中 submenu → alt-goto-main expandSubmenu", () => {
    const items = [item({ id: 1, actionType: "submenu" })];
    const subs = [item({ id: 11, parentId: 1, actionType: "url", title: "Google" })];
    const a = decideKeyAction(key({ key: "1", altKey: true, code: "Digit1" }), ctx({ mainItems: items, menuItems: [...items, ...subs], searchEngine: "google" }));
    expect(a.type).toBe("alt-goto-main");
    expect((a as any).expandSubmenu).toBe(true);
    expect((a as any).parentId).toBe(1);
  });
  it("Alt+数字焦点 main 命中非 submenu → alt-goto-main 不展开", () => {
    const items = [item({ id: 2, actionType: "menu" })];
    const a = decideKeyAction(key({ key: "1", altKey: true, code: "Digit1" }), ctx({ mainItems: items }));
    expect(a).toMatchObject({ type: "alt-goto-main", idx: 0, expandSubmenu: false });
  });
});

describe("decideKeyAction - Enter/Space", () => {
  it("Enter → menu-enter", () => {
    expect(decideKeyAction(key({ key: "Enter" }), ctx({ mode: "menu", lastImeKeyTime: 0 }))).toEqual({ type: "menu-enter" });
  });
  it("Space → menu-enter", () => {
    expect(decideKeyAction(key({ key: " " }), ctx())).toEqual({ type: "menu-enter" });
  });
});
