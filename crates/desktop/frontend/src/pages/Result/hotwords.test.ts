import { describe, it, expect } from "vitest";
import {
  parseSegments,
  segmentsMatchText,
  hotwordRanges,
  applyCandidate,
  type Segment,
} from "./hotwords";

const raw = (t: string): Segment => ({ kind: "raw", text: t });
const hot = (t: string, c: string[]): Segment => ({ kind: "hotwords", text: t, candidates: c });

// ── parseSegments ──
describe("parseSegments", () => {
  it("解析含 hotwords 段的合法 JSON", () => {
    const json = JSON.stringify([
      { kind: "raw", text: "需要你修正这个" },
      { kind: "hotwords", text: "注释", candidates: ["注释", "主意", "注意"] },
      { kind: "raw", text: "修复下面的错误。" },
    ]);
    const segs = parseSegments(json);
    expect(segs).toHaveLength(3);
    expect(segs![0].kind).toBe("raw");
    expect(segs![1].kind).toBe("hotwords");
    expect(segs![1].candidates).toEqual(["注释", "主意", "注意"]);
  });

  it("null / 空串 / [] → null（无段信息，降级扁平 text）", () => {
    expect(parseSegments(null)).toBeNull();
    expect(parseSegments(undefined)).toBeNull();
    expect(parseSegments("")).toBeNull();
    expect(parseSegments("[]")).toBeNull();
  });

  it("坏 JSON → null（容错，不抛）", () => {
    expect(parseSegments("not json")).toBeNull();
    expect(parseSegments("{broken")).toBeNull();
  });

  it("非数组 → null", () => {
    expect(parseSegments('{"kind":"raw","text":"x"}')).toBeNull();
  });

  it("缺 kind / text 的项被跳过", () => {
    const json = JSON.stringify([
      { kind: "raw", text: "ok" },
      { kind: "raw" }, // 缺 text
      { text: "no kind" }, // 缺 kind
      { kind: "unknown", text: "bad kind" }, // kind 不在枚举
      { kind: "raw", text: "tail" },
    ]);
    const segs = parseSegments(json);
    expect(segs).toHaveLength(2);
    expect(segs![0].text).toBe("ok");
    expect(segs![1].text).toBe("tail");
  });

  it("hotwords 段无 candidates 字段时不崩溃（candidates undefined）", () => {
    const json = JSON.stringify([{ kind: "hotwords", text: "注释" }]);
    const segs = parseSegments(json);
    expect(segs).toHaveLength(1);
    expect(segs![0].candidates).toBeUndefined();
  });

  it("candidates 非字符串数组被丢弃", () => {
    const json = JSON.stringify([{ kind: "hotwords", text: "x", candidates: [1, 2, 3] }]);
    const segs = parseSegments(json);
    expect(segs![0].candidates).toBeUndefined();
  });
});

// ── segmentsMatchText ──
describe("segmentsMatchText", () => {
  it("段拼接 == doc → true", () => {
    const segs = [raw("abc"), hot("x", ["x", "y"]), raw("def")];
    expect(segmentsMatchText(segs, "abcxdef")).toBe(true);
  });

  it("段拼接 != doc → false（用户编辑后失配）", () => {
    const segs = [raw("abc"), hot("x", ["x", "y"]), raw("def")];
    expect(segmentsMatchText(segs, "abcdef")).toBe(false);
  });

  it("空段数组 vs 空串 → true", () => {
    expect(segmentsMatchText([], "")).toBe(true);
  });

  it("中文 char 计数（JS string.length = UTF-16 code unit）", () => {
    const segs = [raw("你好"), hot("世界", ["世界", "事迹"])];
    // "你好世界" 4 个 BMP 字符 = length 4
    expect(segmentsMatchText(segs, "你好世界")).toBe(true);
    expect(segmentsMatchText(segs, "你好")).toBe(false);
  });
});

// ── hotwordRanges ──
describe("hotwordRanges", () => {
  it("单 hotwords 段 → 正确 offset", () => {
    const segs = [raw("需要你修正这个"), hot("注释", ["注释", "主意", "注意"]), raw("修复。")];
    const doc = "需要你修正这个注释修复。";
    const ranges = hotwordRanges(segs, doc);
    expect(ranges).toHaveLength(1);
    expect(ranges[0].from).toBe(7); // "需要你修正这个" 7 字
    expect(ranges[0].to).toBe(9); // "注释" 2 字
    expect(ranges[0].candidates).toEqual(["注释", "主意", "注意"]);
  });

  it("多 hotwords 段 → 各自 offset", () => {
    const segs = [
      hot("甲", ["甲", "假"]),
      raw("中间"),
      hot("乙", ["乙", "已"]),
    ];
    const doc = "甲中间乙";
    const ranges = hotwordRanges(segs, doc);
    expect(ranges).toHaveLength(2);
    expect(ranges[0]).toEqual({ from: 0, to: 1, candidates: ["甲", "假"], segIndex: 0 });
    expect(ranges[1]).toEqual({ from: 3, to: 4, candidates: ["乙", "已"], segIndex: 2 });
  });

  it("无 hotwords 段 → 空数组", () => {
    const segs = [raw("a"), raw("b")];
    expect(hotwordRanges(segs, "ab")).toEqual([]);
  });

  it("segments 与 doc 失配 → 空数组（降级，防错位）", () => {
    const segs = [raw("abc"), hot("x", ["x", "y"]), raw("def")];
    expect(hotwordRanges(segs, "abcZZdef")).toEqual([]);
  });

  it("hotwords 段无 candidates → 跳过（容错）", () => {
    const segs: Segment[] = [
      { kind: "hotwords", text: "x" }, // 无 candidates
      raw("tail"),
    ];
    expect(hotwordRanges(segs, "xtail")).toEqual([]);
  });

  it("hotwords 段 candidates 空 数组 → 跳过", () => {
    const segs = [hot("x", [])];
    expect(hotwordRanges(segs, "x")).toEqual([]);
  });
});

// ── applyCandidate ──
describe("applyCandidate", () => {
  it("替换中间段 → 新 doc + dirtyRange", () => {
    const doc = "abc注释def";
    const { doc: newDoc, dirtyRange } = applyCandidate(doc, 3, 5, "注意");
    expect(newDoc).toBe("abc注意def");
    expect(dirtyRange).toEqual([3, 5]); // "注意" 2 字，[3,5)
  });

  it("选第一个候选（== 原文）→ doc 不变，dirtyRange 仍标记（后端 rebuild 标 Edited）", () => {
    const doc = "abc注释def";
    const { doc: newDoc, dirtyRange } = applyCandidate(doc, 3, 5, "注释");
    expect(newDoc).toBe("abc注释def");
    expect(dirtyRange).toEqual([3, 5]);
  });

  it("候选词长度 != 原文 → dirtyRange.to 正确反映新长度", () => {
    const doc = "甲乙丙";
    const { doc: newDoc, dirtyRange } = applyCandidate(doc, 1, 2, "长久");
    expect(newDoc).toBe("甲长久丙");
    expect(dirtyRange).toEqual([1, 3]);
  });

  it("候选词更短 → dirtyRange.to 收缩", () => {
    const doc = "abc长久的def";
    const { doc: newDoc, dirtyRange } = applyCandidate(doc, 3, 6, "甲");
    expect(newDoc).toBe("abc甲def");
    expect(dirtyRange).toEqual([3, 4]);
  });

  it("from/to 越界 → clamp 不崩", () => {
    const doc = "abc";
    const { doc: newDoc, dirtyRange } = applyCandidate(doc, -5, 999, "x");
    expect(newDoc).toBe("x");
    expect(dirtyRange).toEqual([0, 1]);
  });

  it("from > to → clamp 到 from（空替换）", () => {
    const doc = "abc";
    const { doc: newDoc, dirtyRange } = applyCandidate(doc, 2, 1, "x");
    // t = max(f, min(to, len)) = max(2, min(1,3)) = max(2,1) = 2
    expect(newDoc).toBe("abxc");
    expect(dirtyRange).toEqual([2, 3]);
  });
});
