import { describe, it, expect } from "vitest";
import { renderMarkdown } from "./markdown";

describe("renderMarkdown", () => {
  it("渲染标题", () => {
    expect(renderMarkdown("# Hello")).toContain("<h1");
    expect(renderMarkdown("## Sub")).toContain("<h2");
  });

  it("渲染粗体和斜体", () => {
    expect(renderMarkdown("**bold**")).toContain("<strong>");
    expect(renderMarkdown("*italic*")).toContain("<em>");
  });

  it("渲染代码块（无高亮）", () => {
    const html = renderMarkdown("```ts\nconst x = 1;\n```");
    expect(html).toContain("<pre");
    expect(html).toContain("<code");
    expect(html).toContain("const x = 1;");
  });

  it("渲染行内代码", () => {
    expect(renderMarkdown("`code`")).toContain("<code>");
  });

  it("渲染链接", () => {
    const html = renderMarkdown("[example](https://example.com)");
    expect(html).toContain('href="https://example.com"');
    expect(html).toContain("example");
  });

  it("渲染引用块", () => {
    expect(renderMarkdown("> quote")).toContain("<blockquote>");
  });

  it("渲染无序列表", () => {
    expect(renderMarkdown("- item1\n- item2")).toContain("<ul>");
  });

  it("渲染有序列表", () => {
    expect(renderMarkdown("1. first\n2. second")).toContain("<ol>");
  });

  it("渲染 task-list", () => {
    const html = renderMarkdown("- [x] done\n- [ ] todo");
    expect(html).toContain("task-list");
  });

  it("渲染 ==高亮==（markdown-it-mark）", () => {
    expect(renderMarkdown("==highlighted==")).toContain("<mark>");
  });

  it("渲染表格", () => {
    const html = renderMarkdown("| A | B |\n|---|---|\n| 1 | 2 |");
    expect(html).toContain("<table>");
  });

  it("mermaid 占位 class（埋点）", () => {
    const html = renderMarkdown("```mermaid\ngraph TD\nA-->B\n```");
    expect(html).toContain("md-mermaid-pending");
  });

  it("heading 锚点 id（slugify）", () => {
    const html = renderMarkdown("# Hello World");
    expect(html).toContain('id="hello-world"');
  });

  it("空字符串安全渲染", () => {
    expect(renderMarkdown("")).toBe("");
  });
});
