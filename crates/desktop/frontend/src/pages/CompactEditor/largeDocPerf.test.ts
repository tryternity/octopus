// CompactEditor 大文档基线测量（z_perf measurement-first，spec 2026-08-18 修订 9.2）。
// 复现「打开大文件加载慢」的三条成本路径，输出耗时供 before/after 对比：
//   1. renderMarkdown（markdown-it 全文渲染——MarkdownPreview 主成本）
//   2. EditorState.create + markdown()（Lezer 全文解析——CodeMirrorEditor 建态成本）
//   3. EditorState.create 无语言（大文档降级模式的目标成本）
// 断言只做正确性（慢机不抖红）；耗时经 console.log 输出，供人工对比。
import { describe, expect, it } from "vitest";
import { renderMarkdown } from "@/lib/markdown";
import { EditorState } from "@codemirror/state";
import { markdown } from "@codemirror/lang-markdown";
import { lineNumbers, EditorView } from "@codemirror/view";
import { syntaxHighlighting } from "@codemirror/language";
import { sliceForPreview } from "./previewTruncate";

/** 生成代表性大文档：标题/段落/列表/代码块循环（贴近文件夹转 Markdown 产物形态）。 */
export function genLargeDoc(targetBytes: number): string {
  const section = (i: number) =>
    `## 第 ${i} 节标题\n\n这是第 ${i} 节的正文段落，包含一些**加粗**、*斜体*与[链接](https://example.com/${i})。\n\n` +
    `- 要点一：数据 ${i * 3}\n- 要点二：说明文字 ${i}\n- 要点三：备注\n\n` +
    "```python\n" + `def f_${i}(x):\n    return x * ${i}\n` + "```\n\n";
  const out: string[] = ["# 大文档基准\n"];
  let bytes = out[0].length;
  for (let i = 1; bytes < targetBytes; i++) {
    const s = section(i);
    out.push(s);
    bytes += s.length;
  }
  return out.join("");
}

function timeMs(fn: () => void): number {
  const t0 = performance.now();
  fn();
  return performance.now() - t0;
}

describe("CompactEditor 大文档基线测量", () => {
  it("renderMarkdown 各尺寸耗时（记录用）", () => {
    for (const kb of [100, 500, 1024, 2048]) {
      const doc = genLargeDoc(kb * 1024);
      const ms = timeMs(() => {
        const html = renderMarkdown(doc);
        expect(html.length).toBeGreaterThan(0);
        expect(html).toContain("第 1 节标题");
      });
      // eslint-disable-next-line no-console
      console.log(`[perf-bench] renderMarkdown ${kb}KB → ${ms.toFixed(1)}ms (html chars=${renderMarkdown(doc).length})`);
    }
  });

  it("EditorState.create 有/无 markdown() 耗时对比（1MB）", () => {
    const doc = genLargeDoc(1024 * 1024);
    const plainMs = timeMs(() => {
      const st = EditorState.create({ doc, extensions: [lineNumbers(), EditorView.lineWrapping] });
      expect(st.doc.length).toBe(doc.length);
    });
    const langMs = timeMs(() => {
      const st = EditorState.create({
        doc,
        extensions: [lineNumbers(), syntaxHighlighting(undefined as never, { fallback: true }), markdown(), EditorView.lineWrapping],
      });
      expect(st.doc.length).toBe(doc.length);
    });
    // eslint-disable-next-line no-console
    console.log(`[perf-bench] EditorState.create 1MB → plain=${plainMs.toFixed(1)}ms / +markdown()=${langMs.toFixed(1)}ms`);
  });

  it("截断后 preview 渲染成本有界（修复 after）", () => {
    const doc = genLargeDoc(2048 * 1024);
    const sliced = sliceForPreview(doc);
    expect(sliced).not.toBeNull();
    expect(sliced!.length).toBeLessThanOrEqual(256 * 1024 + 1);
    const ms = timeMs(() => {
      const html = renderMarkdown(sliced!);
      expect(html.length).toBeGreaterThan(0);
    });
    // eslint-disable-next-line no-console
    console.log(`[perf-bench] renderMarkdown(2MB→截断 ${Math.round(sliced!.length / 1024)}KB) → ${ms.toFixed(1)}ms（修复后 preview 实际成本）`);
  });
});
