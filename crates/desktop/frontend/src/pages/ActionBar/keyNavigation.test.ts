import { describe, it, expect } from "vitest";
import { pickSubIdx } from "./keyNavigation";
import type { ActionBarItem } from "./types";

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
