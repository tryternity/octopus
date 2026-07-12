import { describe, it, expect } from "vitest";
import { promoteTempTab } from "./promoteTempTab";
import type { Tab } from "./index";

// 「图文编辑」入口打开空白 CompactEditor（temp tab，isTemp=true，不写 DB）。
// 用户保存 → 后端 insert_clipboard_text_item 返回新 id → 前端把 temp tab 升级为
// 正式 clipboard tab（key/source/itemId/isTemp 同步），后续编辑走 update 路径。

const tempTab = (overrides: Partial<Tab> = {}): Tab => ({
  key: "temp:abc_1",
  source: "temp",
  itemId: 0,
  itemType: "text",
  text: "",
  isTemp: true,
  ...overrides,
});

describe("promoteTempTab", () => {
  it("把 temp tab 升级为 clipboard tab（key/source/itemId/isTemp/itemType 同步）", () => {
    const tabs = [tempTab({ text: "新内容" })];
    const result = promoteTempTab(tabs, 0, 12345);
    expect(result).toHaveLength(1);
    expect(result[0].key).toBe("clipboard:12345");
    expect(result[0].source).toBe("clipboard");
    expect(result[0].itemId).toBe(12345);
    expect(result[0].isTemp).toBe(false);
    expect(result[0].itemType).toBe("text");
    expect(result[0].text).toBe("新内容"); // text 保留（不丢内容）
  });

  it("不影响其他 tab", () => {
    const other = tempTab({ key: "temp:other", text: "其他" });
    const target = tempTab({ key: "temp:target", text: "x" });
    const result = promoteTempTab([other, target], 1, 99);
    expect(result[0]).toEqual(other); // 未变
    expect(result[1].key).toBe("clipboard:99");
    expect(result[1].itemId).toBe(99);
  });

  it("返回新数组、不修改原数组", () => {
    const tabs = [tempTab()];
    const result = promoteTempTab(tabs, 0, 1);
    expect(result).not.toBe(tabs);
    expect(tabs[0].isTemp).toBe(true); // 原数组保持 temp
    expect(tabs[0].key).toBe("temp:abc_1");
  });

  it("contrast temp 升级为 single clipboard，丢弃原文", () => {
    const tabs = [tempTab({
      text: "译文内容",
      mode: "contrast",
      originalText: "原文",
      translatedText: "译文内容",
    })];
    const result = promoteTempTab(tabs, 0, 42);
    expect(result[0].key).toBe("clipboard:42");
    expect(result[0].itemId).toBe(42);
    expect(result[0].isTemp).toBe(false);
    expect(result[0].mode).toBe("single");
    expect(result[0].originalText).toBeUndefined();
    expect(result[0].translatedText).toBeUndefined();
  });

  it("single temp 升级保持 mode undefined", () => {
    const tabs = [tempTab({ text: "内容" })];
    const result = promoteTempTab(tabs, 0, 42);
    expect(result[0].mode).toBeUndefined();
    expect(result[0].isTemp).toBe(false);
  });
});
