import { describe, it, expect } from "vitest";
import {
  TERMINAL_FONT_SIZE_DEFAULT,
  TERMINAL_FONT_FAMILY_DEFAULT,
  isFontAtDefault,
} from "./fontPrefs";

describe("isFontAtDefault", () => {
  describe("默认状态（不偏离 → true，不显示按钮）", () => {
    it("字号=13 + 字体=Menlo → 在默认", () => {
      expect(isFontAtDefault(13, "Menlo")).toBe(true);
    });

    it("字号=13 + 字体缺失 → 在默认（旧库/损坏数据不显示按钮）", () => {
      expect(isFontAtDefault(13, undefined)).toBe(true);
    });

    it("字号=13 + 字体空串 → 在默认", () => {
      expect(isFontAtDefault(13, "")).toBe(true);
    });

    it("字号缺失 + 字体=Menlo → 在默认", () => {
      expect(isFontAtDefault(undefined, "Menlo")).toBe(true);
    });

    it("全部缺失 → 在默认", () => {
      expect(isFontAtDefault(undefined, undefined)).toBe(true);
    });
  });

  describe("偏离默认（→ false，显示「恢复默认」按钮）", () => {
    it("字号偏离（14）+ 字体默认 → 偏离", () => {
      expect(isFontAtDefault(14, "Menlo")).toBe(false);
    });

    it("字号偏离（8 下限）+ 字体默认 → 偏离", () => {
      expect(isFontAtDefault(8, "Menlo")).toBe(false);
    });

    it("字号默认 + 字体偏离（SF Mono）→ 偏离", () => {
      expect(isFontAtDefault(13, "SF Mono")).toBe(false);
    });

    it("字号默认 + 字体偏离（Monaco）→ 偏离", () => {
      expect(isFontAtDefault(13, "Monaco")).toBe(false);
    });

    it("字号 + 字体都偏离 → 偏离", () => {
      expect(isFontAtDefault(20, "JetBrains Mono")).toBe(false);
    });
  });

  describe("边界与异常输入", () => {
    it("字号非数字（字符串）→ 视为缺失，不因字号偏离", () => {
      // 异常数据防御：DB 损坏或旧格式
      expect(isFontAtDefault("13", "Menlo")).toBe(true);
    });

    it("字号=0 → 偏离（0 是有效数字但非默认）", () => {
      expect(isFontAtDefault(0, "Menlo")).toBe(false);
    });

    it("字号=负数 → 偏离", () => {
      expect(isFontAtDefault(-1, "Menlo")).toBe(false);
    });

    it("字体=点前缀系统字体（.SF NS Mono）→ 偏离", () => {
      // 虽然后端已过滤，但用户 DB 可能残留旧选择
      expect(isFontAtDefault(13, ".SF NS Mono")).toBe(false);
    });

    it("字体含大小写差异（menlo vs Menlo）→ 偏离（大小写敏感）", () => {
      expect(isFontAtDefault(13, "menlo")).toBe(false);
    });
  });

  describe("常量值（防默认值意外变更）", () => {
    it("默认字号 = 13", () => {
      expect(TERMINAL_FONT_SIZE_DEFAULT).toBe(13);
    });

    it("默认字体族 = Menlo", () => {
      expect(TERMINAL_FONT_FAMILY_DEFAULT).toBe("Menlo");
    });
  });
});
