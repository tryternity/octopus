import { describe, expect, it, vi } from "vitest";
import { resolveImgSrc } from "./resolveImgSrc";

describe("resolveImgSrc", () => {
  it("无 baseUrl 原样返回（convert 不被调用）", () => {
    const convert = vi.fn((s: string) => `conv(${s})`);
    expect(resolveImgSrc("img/a.png", undefined, convert)).toBe("img/a.png");
    expect(resolveImgSrc("./a.png", undefined, convert)).toBe("./a.png");
    expect(convert).not.toHaveBeenCalled();
  });

  it.each(["http://x.com/a.png", "https://x.com/a.png", "data:image/png;base64,xxx", "asset://localhost/abs.png", "blob:https://x.com/uuid", "tci:whatever"])(
    "外部 scheme 跳过：%s",
    (src) => {
      const convert = vi.fn((s: string) => `conv(${s})`);
      expect(resolveImgSrc(src, "/Users/x/Documents/octopus/md", convert)).toBe(src);
      expect(convert).not.toHaveBeenCalled();
    },
  );

  it("绝对路径（/ 开头）不经 join 原样返回", () => {
    const convert = vi.fn((s: string) => `conv(${s})`);
    expect(resolveImgSrc("/abs/img.png", "/Users/x/Documents/octopus/md", convert)).toBe("/abs/img.png");
    expect(convert).not.toHaveBeenCalled();
  });

  it("相对路径 ./ 形态 join 后经 convert", () => {
    const convert = vi.fn((s: string) => `conv(${s})`);
    expect(resolveImgSrc("./img_123/a.png", "/Users/x/Documents/octopus/md", convert)).toBe(
      "conv(/Users/x/Documents/octopus/md/img_123/a.png)",
    );
    expect(convert).toHaveBeenCalledTimes(1);
  });

  it("相对路径 dir/ 形态 join 后经 convert", () => {
    const convert = vi.fn((s: string) => `conv(${s})`);
    expect(resolveImgSrc("img_123/b/c.png", "/Users/x/Documents/octopus/md", convert)).toBe(
      "conv(/Users/x/Documents/octopus/md/img_123/b/c.png)",
    );
    expect(convert).toHaveBeenCalledTimes(1);
  });

  it("baseUrl 尾部斜杠归一（不产生 //）", () => {
    const convert = (s: string) => s;
    expect(resolveImgSrc("a.png", "/Users/x/md/", convert)).toBe("/Users/x/md/a.png");
    expect(resolveImgSrc("a.png", "/Users/x/md///", convert)).toBe("/Users/x/md/a.png");
  });
});
