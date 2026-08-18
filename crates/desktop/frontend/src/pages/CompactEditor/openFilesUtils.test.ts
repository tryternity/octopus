import { describe, expect, it } from "vitest";
import { normalizeDialogSelection, TEXT_IMAGE_EXTS } from "./openFilesUtils";

describe("openFilesUtils", () => {
  it("单选返回单元素数组", () => {
    expect(normalizeDialogSelection("/tmp/a.md")).toEqual(["/tmp/a.md"]);
  });
  it("多选原样返回", () => {
    expect(normalizeDialogSelection(["/a.md", "/b.png"])).toEqual(["/a.md", "/b.png"]);
  });
  it("null / 取消返回空", () => {
    expect(normalizeDialogSelection(null)).toEqual([]);
  });
  it("扩展名清单含文本与图片", () => {
    for (const ext of ["md", "txt", "json", "py", "png", "jpg", "webp"]) {
      expect(TEXT_IMAGE_EXTS).toContain(ext);
    }
  });
});
