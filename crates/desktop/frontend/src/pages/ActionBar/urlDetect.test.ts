import { describe, it, expect } from "vitest";
import { detectActionUrl } from "./urlDetect";

describe("detectActionUrl", () => {
  const cases: Array<{ input: string; isUrl: boolean; url?: string; note?: string }> = [
    // 域名格式
    { input: "apple.com", isUrl: true, url: "https://apple.com" },
    { input: "github.com/octopus", isUrl: true, url: "https://github.com/octopus" },
    { input: "a.b/c", isUrl: true, url: "https://a.b/c" },
    { input: "foo.com.cn/bar", isUrl: true, url: "https://foo.com.cn/bar" },
    // IP 地址
    { input: "192.168.1.100", isUrl: true, url: "http://192.168.1.100" },
    { input: "127.0.0.1:3000", isUrl: true, url: "http://127.0.0.1:3000" },
    // localhost
    { input: "localhost", isUrl: true, url: "http://localhost" },
    { input: "localhost:8080/api", isUrl: true, url: "http://localhost:8080/api" },
    // 否定
    { input: "hello world", isUrl: false, note: "有空格" },
    { input: "123.456", isUrl: false, note: "纯数字不像域名也不是IP" },
    { input: ".hidden", isUrl: false, note: "以.开头" },
    { input: "end.", isUrl: false, note: "以.结尾" },
    { input: "", isUrl: false },
    { input: "你好世界", isUrl: false, note: "无.无localhost" },
  ];
  for (const c of cases) {
    it(`${c.note ?? c.input}: isUrl=${c.isUrl}`, () => {
      const result = detectActionUrl(c.input);
      expect(result.isUrl).toBe(c.isUrl);
      if (c.url) expect(result.url).toBe(c.url);
    });
  }
});
