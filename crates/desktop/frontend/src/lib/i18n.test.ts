import { describe, it, expect, beforeEach } from "vitest";
// initI18n 依赖 Tauri invoke，测试环境无法调用——仅测试纯函数
import { setLocale, getLocale, t } from "./i18n";

describe("i18n", () => {
  beforeEach(() => {
    setLocale("zh-CN");
  });

  it("中文翻译", () => {
    expect(t("editor.undo")).toBe("撤销");
    expect(t("editor.save")).toBe("保存");
  });

  it("英文翻译", () => {
    setLocale("en");
    expect(t("editor.undo")).toBe("Undo");
    expect(t("editor.save")).toBe("Save");
  });

  it("嵌套 key 查找（从 YAML 嵌套结构 flatten 后的 flat key）", () => {
    expect(t("editor.view.split")).toBe("分屏");
    expect(t("editor.view.editor")).toBe("编辑");
    setLocale("en");
    expect(t("editor.view.split")).toBe("Split");
    expect(t("editor.view.editor")).toBe("Editor");
  });

  it("插值", () => {
    expect(t("editor.charCount", { n: 42 })).toBe("42 字");
    setLocale("en");
    expect(t("editor.charCount", { n: 42 })).toBe("42 chars");
  });

  it("缺 key fallback 返回 key 本身", () => {
    expect(t("nonexistent.key")).toBe("nonexistent.key");
  });

  it("getLocale 反映当前 locale", () => {
    setLocale("en");
    expect(getLocale()).toBe("en");
    setLocale("zh-CN");
    expect(getLocale()).toBe("zh-CN");
  });
});
