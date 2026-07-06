import { describe, it, expect } from "vitest";
import { detectUrl } from "./clipboard";

describe("detectUrl", () => {
  const cases: Array<{ input: string; isLink: boolean; url?: string; note?: string }> = [
    // 带协议：原样（含 trim）
    { input: "https://github.com/a", isLink: true, url: "https://github.com/a" },
    { input: "http://x.com", isLink: true, url: "http://x.com" },
    { input: "  https://a.com/p  ", isLink: true, url: "https://a.com/p" },
    // 路径 A：常用后缀域名 → 补 https://
    { input: "github.com/bingreeky/MemEvolve", isLink: true, url: "https://github.com/bingreeky/MemEvolve" },
    { input: "github.com", isLink: true, url: "https://github.com" },
    { input: "foo.com.cn/bar", isLink: true, url: "https://foo.com.cn/bar" },
    { input: "foo.cn", isLink: true, url: "https://foo.cn" },
    { input: "github.com:8080/x", isLink: true, url: "https://github.com:8080/x" },
    // 路径 B：localhost/IPv4 + 必带端口 → 补 http://
    { input: "localhost:3000", isLink: true, url: "http://localhost:3000" },
    { input: "localhost:3000/admin", isLink: true, url: "http://localhost:3000/admin" },
    { input: "127.0.0.1:8080", isLink: true, url: "http://127.0.0.1:8080" },
    { input: "192.168.1.10:80", isLink: true, url: "http://192.168.1.10:80" },
    { input: "0.0.0.0:5000", isLink: true, url: "http://0.0.0.0:5000" },
    { input: "localhost:1", isLink: true, url: "http://localhost:1" },           // 端口下界
    { input: "localhost:65535", isLink: true, url: "http://localhost:65535" },   // 端口上界
    // 否定
    { input: "", isLink: false },
    { input: "localhost", isLink: false, note: "无端口" },
    { input: "127.0.0.1", isLink: false, note: "无端口" },
    { input: "localhost:abc", isLink: false, note: "端口非数字" },
    { input: "localhost:0", isLink: false, note: "端口 0 非法" },
    { input: "localhost:65536", isLink: false, note: "端口超界" },
    { input: "256.1.1.1:80", isLink: false, note: "IPv4 段 >255" },
    { input: "file.txt", isLink: false, note: "后缀不在表" },
    { input: "main.rs", isLink: false },
    { input: "readme.md", isLink: false },
    { input: "v1.2.3", isLink: false },
    { input: "hello.world", isLink: false },
    { input: "看这个 github.com/foo", isLink: false, note: "含空格" },
    { input: "（github.com/foo）", isLink: false, note: "括号致 label 非法" },
  ];

  for (const c of cases) {
    it(`${c.isLink ? "链接" : "非链接"} ← ${JSON.stringify(c.input)}${c.note ? ` (${c.note})` : ""}`, () => {
      const r = detectUrl(c.input);
      expect(r.isLink).toBe(c.isLink);
      if (c.isLink) expect(r.url).toBe(c.url);
    });
  }
});
