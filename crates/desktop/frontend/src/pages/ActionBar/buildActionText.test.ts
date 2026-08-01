import { describe, it, expect } from "vitest";
import { buildActionText } from "./index";

describe("buildActionText", () => {
  describe("url / copy_path（二选一，params 优先）", () => {
    it("url: 有 params 用 params", () => {
      expect(buildActionText("url", "搜索词", "选中文本")).toBe("搜索词");
    });

    it("url: 无 params 用选中文本", () => {
      expect(buildActionText("url", "", "选中文本")).toBe("选中文本");
    });

    it("url: 两者都空返回空字符串", () => {
      expect(buildActionText("url", "", "")).toBe("");
      expect(buildActionText("url", undefined, undefined)).toBe("");
    });

    it("copy_path 同 url 语义", () => {
      expect(buildActionText("copy_path", "/some/path", "/selected/file")).toBe("/some/path");
      expect(buildActionText("copy_path", "", "/selected/file")).toBe("/selected/file");
    });

    it("url: 不拼接（params 不会带入选中文本的换行）", () => {
      // 关键不变量：url 的 text 是单值，绝不能含换行
      const result = buildActionText("url", "keyword", "selected text");
      expect(result).toBe("keyword");
      expect(result).not.toContain("\n");
    });
  });

  describe("ai / script / agent（params 与选中文本拼接）", () => {
    it("ai: 有 params + 有选中文本 → 拼接（中间换行）", () => {
      expect(buildActionText("ai", "总结这段", "选中的长文本")).toBe("总结这段\n选中的长文本");
    });

    it("ai: 有 params 无选中文本 → 只 params（trim 掉空行）", () => {
      expect(buildActionText("ai", "总结这段", "")).toBe("总结这段");
      expect(buildActionText("ai", "总结这段", undefined)).toBe("总结这段");
    });

    it("ai: 无 params 有选中文本 → 只选中文本（trim 掉开头空行）", () => {
      expect(buildActionText("ai", "", "选中的长文本")).toBe("选中的长文本");
      // 直接点击场景：params 为空字符串
      expect(buildActionText("ai", "", "选中的长文本")).toBe("选中的长文本");
    });

    it("ai: 两者都空 → 空字符串", () => {
      expect(buildActionText("ai", "", "")).toBe("");
    });

    it("script 同 ai 拼接语义", () => {
      expect(buildActionText("script", "指令", "上下文")).toBe("指令\n上下文");
    });

    it("agent 同 ai 拼接语义", () => {
      expect(buildActionText("agent", "指令", "上下文")).toBe("指令\n上下文");
    });
  });

  describe("直接点击路径等价性（params 恒空）", () => {
    it("直接点击 url 等价于选中文本", () => {
      // executeItem 调 buildActionText(actionType, "", ctx.text)
      expect(buildActionText("url", "", "selected")).toBe("selected");
    });

    it("直接点击 ai 等价于选中文本（空 params 被 trim）", () => {
      expect(buildActionText("ai", "", "selected")).toBe("selected");
    });

    it("直接点击 script 等价于选中文本", () => {
      expect(buildActionText("script", "", "selected")).toBe("selected");
    });
  });
});
