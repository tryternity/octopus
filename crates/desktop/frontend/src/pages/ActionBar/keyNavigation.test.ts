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

  // slash 模式 Tab → 补全（不切 tab）
  it("slash 模式 Tab → slash-complete", () => {
    expect(decideKeyAction(key({ key: "Tab" }), ctx({ mode: "search", searchResultsCount: 3, activeTab: "slash" }))).toEqual({ type: "slash-complete" });
  });
  it("slash 模式 Shift+Tab → slash-complete（补全对方向不敏感）", () => {
    expect(decideKeyAction(key({ key: "Tab", shiftKey: true }), ctx({ mode: "search", searchResultsCount: 3, activeTab: "slash" }))).toEqual({ type: "slash-complete" });
  });
  // slash 模式 ←/→ 放行给浏览器做原生光标移动（不触发补全，防误清空参数）
  it("slash 模式 ArrowLeft → passthrough（不补全）", () => {
    expect(decideKeyAction(key({ key: "ArrowLeft" }), ctx({ mode: "search", searchResultsCount: 3, activeTab: "slash" }))).toEqual({ type: "passthrough" });
  });
  it("slash 模式 ArrowRight → passthrough（不补全）", () => {
    expect(decideKeyAction(key({ key: "ArrowRight" }), ctx({ mode: "search", searchResultsCount: 3, activeTab: "slash" }))).toEqual({ type: "passthrough" });
  });
  // 非 slash 模式下 Tab 仍切 tab（回归保护）
  it("all 模式 Tab → search-tab（非 slash 不补全）", () => {
    expect(decideKeyAction(key({ key: "Tab" }), ctx({ mode: "search", searchResultsCount: 3, activeTab: "all" }))).toEqual({ type: "search-tab", dir: 1 });
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

describe("decideKeyAction - Alt 定位符", () => {
  // Alt+字母定位（a=第10项 idx=9，b=第11项 idx=10...）—— shortcut 废弃后字母改定位
  it("Alt+a → 定位第10项（idx=9）", () => {
    // 需 10 项才不越界（a=idx=9）
    const items = Array.from({ length: 10 }, (_, i) => item({ id: i + 1, actionType: "menu" }));
    const a = decideKeyAction(key({ key: "å", altKey: true, code: "KeyA" }), ctx({ mainItems: items }));
    expect(a).toEqual({ type: "alt-goto-main", idx: 9, expandSubmenu: false });
  });
  it("Alt+字母越界 → swallow（菜单项不足）", () => {
    // 只有 1 项，Alt+a（idx=9）越界
    expect(decideKeyAction(key({ key: "å", altKey: true, code: "KeyA" }), ctx({ mainItems: [item({ id: 1 })] }))).toEqual({ type: "swallow" });
  });
  it("Alt+字母焦点 sub → alt-goto-sub", () => {
    expect(decideKeyAction(key({ key: "å", altKey: true, code: "KeyA" }), ctx({ focusLayer: "sub" }))).toEqual({ type: "alt-goto-sub", idx: 9 });
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
  it("Alt 但 codeToChar 无效 → passthrough（§PRESERVE 行 778）", () => {
    // codeToChar("") 返回 falsy → 不进字母/数字分支 → passthrough（不 preventDefault）
    expect(decideKeyAction(key({ key: "¡", altKey: true, code: "" }), ctx())).toEqual({ type: "passthrough" });
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
