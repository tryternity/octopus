import { describe, expect, it } from "vitest";
import { PREVIEW_LIMIT, sliceForPreview } from "./previewTruncate";

describe("sliceForPreview", () => {
  it("小文档不截断（返回 null）", () => {
    expect(sliceForPreview("short")).toBeNull();
    expect(sliceForPreview("a".repeat(PREVIEW_LIMIT))).toBeNull();
  });

  it("超限截断到行边界", () => {
    const line = "x".repeat(100) + "\n";
    const doc = line.repeat(6000); // ~600KB > 256KB limit
    const sliced = sliceForPreview(doc);
    expect(sliced).not.toBeNull();
    const s = sliced!;
    expect(s.length).toBeLessThanOrEqual(PREVIEW_LIMIT);
    expect(s.length).toBeGreaterThan(PREVIEW_LIMIT - 200); // 贴近上限（每行 101，最多丢 100）
    expect(s.endsWith("\n")).toBe(true); // 行边界
    expect(doc.startsWith(s)).toBe(true); // 前缀
  });

  it("单行超长硬切", () => {
    const doc = "y".repeat(PREVIEW_LIMIT + 5000);
    const sliced = sliceForPreview(doc)!;
    expect(sliced.length).toBe(PREVIEW_LIMIT);
  });

  it("空文档", () => {
    expect(sliceForPreview("")).toBeNull();
  });
});
