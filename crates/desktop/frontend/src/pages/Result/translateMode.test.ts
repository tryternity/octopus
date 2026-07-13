import { describe, it, expect } from "vitest";
import {
  type TranslateMode,
  TRANSLATE_MODES,
  resolveRememberedTranslateMode,
  parseThrottleSeconds,
  buildTranslatePopupItems,
} from "./translateMode";

describe("resolveRememberedTranslateMode", () => {
  it("合法值原样返回", () => {
    expect(resolveRememberedTranslateMode("manual")).toBe("manual");
    expect(resolveRememberedTranslateMode("4s")).toBe("4s");
    expect(resolveRememberedTranslateMode("8s")).toBe("8s");
    expect(resolveRememberedTranslateMode("12s")).toBe("12s");
  });

  it("非法值回退 manual", () => {
    expect(resolveRememberedTranslateMode("")).toBe("manual");
    expect(resolveRememberedTranslateMode("off")).toBe("manual");
    expect(resolveRememberedTranslateMode("15s")).toBe("manual");
    expect(resolveRememberedTranslateMode("garbage")).toBe("manual");
    expect(resolveRememberedTranslateMode("3s")).toBe("manual");
  });
});

describe("parseThrottleSeconds", () => {
  it("自动档解析秒数", () => {
    expect(parseThrottleSeconds("4s")).toBe(4);
    expect(parseThrottleSeconds("8s")).toBe(8);
    expect(parseThrottleSeconds("12s")).toBe(12);
  });

  it("非自动档返回 null", () => {
    expect(parseThrottleSeconds("manual")).toBeNull();
    expect(parseThrottleSeconds("off" as TranslateMode)).toBeNull();
  });
});

describe("buildTranslatePopupItems", () => {
  const dummyLabel = (m: TranslateMode) => `L-${m}`;

  it("生成四个菜单项，顺序固定", () => {
    const items = buildTranslatePopupItems("manual", dummyLabel);
    expect(items).toHaveLength(4);
    expect(items.map((i) => i.name)).toEqual(TRANSLATE_MODES);
  });

  it("当前档位标记 current=true", () => {
    const items = buildTranslatePopupItems("8s", dummyLabel);
    const current = items.filter((i) => i.current);
    expect(current).toHaveLength(1);
    expect(current[0].name).toBe("8s");
  });

  it("label 经 labelFn 映射", () => {
    const items = buildTranslatePopupItems("manual", dummyLabel);
    expect(items[0].label).toBe("L-manual");
    expect(items[1].label).toBe("L-4s");
  });
});
