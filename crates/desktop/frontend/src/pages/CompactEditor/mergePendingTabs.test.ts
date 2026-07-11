import { describe, it, expect } from "vitest";
import { mergePendingTabs } from "./mergePendingTabs";

// CompactEditor 首次开窗：URL 注入首个 text tab 占位（text="")，mount 后 pending 带真实 text。
// 同 key 时 pending 必须覆盖占位——旧逻辑 `continue` 跳过 pending，占位 text="" 永久保留，
// 首个文本 tab 内容空白（图片 tab 因按 itemId 加载不受影响）。

const textTab = (key: string, text: string) => ({
  key,
  source: "clipboard",
  itemId: 1,
  itemType: "text" as const,
  text,
});

describe("mergePendingTabs", () => {
  it("pending 覆盖同 key 的 URL 占位 tab（补全空 text）", () => {
    const placeholder = textTab("clipboard:1", "");
    const pending = textTab("clipboard:1", "真实内容");
    const result = mergePendingTabs([placeholder], [pending]);
    expect(result).toHaveLength(1);
    expect(result[0].text).toBe("真实内容");
  });

  it("新 key 的 pending 追加到末尾", () => {
    const result = mergePendingTabs([textTab("clipboard:1", "a")], [textTab("clipboard:2", "b")]);
    expect(result.map((t) => t.key)).toEqual(["clipboard:1", "clipboard:2"]);
    expect(result[1].text).toBe("b");
  });

  it("无 key 重叠：existing 保留、pending 全追加", () => {
    const result = mergePendingTabs([textTab("a", "x")], [textTab("b", "y"), textTab("c", "z")]);
    expect(result.map((t) => t.key)).toEqual(["a", "b", "c"]);
  });

  it("existing 为空：pending 全部成为结果", () => {
    const result = mergePendingTabs([], [textTab("clipboard:1", "内容")]);
    expect(result).toEqual([textTab("clipboard:1", "内容")]);
  });

  it("pending 为空：existing 原样返回", () => {
    const result = mergePendingTabs([textTab("a", "x")], []);
    expect(result).toEqual([textTab("a", "x")]);
  });
});
