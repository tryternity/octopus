import { describe, it, expect } from "vitest";
import { mergePendingTabs } from "./mergePendingTabs";

// CompactEditor 首次开窗：URL 注入首个 text tab 占位（text="")，mount 后 pending 带真实 text。
// 同 key 时 pending 必须覆盖占位——旧逻辑 `continue` 跳过 pending，占位 text="" 永久保留，
// 首个文本 tab 内容空白（图片 tab 因按 itemId 加载不受影响）。

const textTab = (key: string, text: string) => ({
  key,
  source: "clipboard",
  itemId: "uuid-1",
  itemType: "text" as const,
  text,
});

const imageTab = (key: string) => ({
  key,
  source: "clipboard",
  itemId: key,
  itemType: "image" as const,
  text: "",
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

  // ── 图片 tab 上限强制（P2 2026-08-18）：pending 批量路径绕过 loadAndAddTab 的
  // 逐事件上限检查——关窗状态拖 8 张图会建 8 个图片 tab。合并时按现有逐事件语义
  // （超限挤掉最旧图片 tab）强制 ≤ MAX_IMAGE_TABS。

  it("pending 图片超上限挤掉最旧（8→5，保留最新）", () => {
    const pending = Array.from({ length: 8 }, (_, i) => imageTab(`img:${i}`));
    const result = mergePendingTabs([], pending);
    expect(result.filter((t) => t.itemType === "image")).toHaveLength(5);
    // 挤掉最旧（img:0..2），保留 img:3..7
    expect(result.map((t) => t.key)).toEqual(["img:3", "img:4", "img:5", "img:6", "img:7"]);
  });

  it("恰好等于上限不挤", () => {
    const pending = Array.from({ length: 5 }, (_, i) => imageTab(`img:${i}`));
    const result = mergePendingTabs([], pending);
    expect(result).toHaveLength(5);
  });

  it("文本 tab 不受上限影响（混合批次只挤图片）", () => {
    const existing = [textTab("clipboard:1", "a"), imageTab("img:old1"), imageTab("img:old2")];
    const pending = Array.from({ length: 4 }, (_, i) => imageTab(`img:new${i}`));
    const result = mergePendingTabs(existing, pending);
    const images = result.filter((t) => t.itemType === "image");
    expect(images).toHaveLength(5);
    expect(result).toHaveLength(6); // 1 文本 + 5 图片
    // 最旧两张（old1/old2）被挤，文本保留
    expect(result.some((t) => t.key === "clipboard:1")).toBe(true);
    expect(result.some((t) => t.key === "img:old1")).toBe(false);
  });
});
