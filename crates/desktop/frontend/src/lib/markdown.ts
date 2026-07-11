import MarkdownIt from "markdown-it";
import mark from "markdown-it-mark";
import taskLists from "markdown-it-task-lists";

function escapeHtml(value: string): string {
  return value
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;");
}

function slugify(text: string): string {
  return text
    .toLowerCase()
    .replace(/[^\p{L}\p{N}\s-]/gu, "")
    .replace(/ /g, "-")
    .replace(/^-|-$/g, "");
}

const md = new MarkdownIt({
  html: false,
  linkify: true,
  typographer: true,
  breaks: false,
  highlight: (code, lang) => {
    // 埋点：mermaid 占位（后续渲染 SVG 的入口）
    if (lang === "mermaid") {
      return `<pre class="md-mermaid-pending"><code>${escapeHtml(code)}</code></pre>`;
    }
    // 埋点：其他语言高亮的入口（后续接 Shiki 在此返回 html）
    // 当前返回空 → markdown-it 走默认 <pre><code> 无高亮
    return "";
  },
});

md.use(taskLists, { enabled: false, label: true });
md.use(mark);

// GitHub 风格 heading slug（锚点跳转用）
md.renderer.rules.heading_open = (tokens, idx, options, _env, self) => {
  const inline = tokens[idx + 1];
  if (inline?.type === "inline") {
    const id = slugify(inline.content);
    if (id) tokens[idx].attrSet("id", id);
  }
  return self.renderToken(tokens, idx, options);
};

// 代码块渲染：包裹 .md-codeblock 容器 + 声明式复制按钮（消除命令式 DOM 注入）
// 按钮文案由 MarkdownPreview 事件委托在点击时动态更新（初始文本为中性占位）
const defaultCodeBlockRender = md.renderer.rules.code_block || ((tokens, idx, options, _env, self) => self.renderToken(tokens, idx, options));
md.renderer.rules.code_block = (tokens, idx, options, env, self) => {
  const pre = defaultCodeBlockRender(tokens, idx, options, env, self);
  return `<div class="md-codeblock">${pre}<button type="button" class="md-copy-btn" data-copy>copy</button></div>`;
};

/** 同步渲染 markdown → HTML（无 Shiki 异步加载） */
export function renderMarkdown(src: string): string {
  return md.render(src);
}
