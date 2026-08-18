// 大文档预览截断（z_perf 2026-08-18：打开大文件冻结修复）。
// baseline（largeDocPerf.test.ts）：renderMarkdown ~100ms/MB，且整块 innerHTML
// 的 WKWebView DOM 布局成本随文档线性放大（秒级冻结主因）。截断后 DOM/渲染
// 成本 O(PREVIEW_LIMIT)，编辑栏仍承载全文。

/** 预览截断上限（UTF-16 code unit）。256KB 实测 markdown-it ~25-30ms，DOM 有界。 */
export const PREVIEW_LIMIT = 256 * 1024;

/** 截断到 limit 内最近的行边界（不拆行）。返回 null = 无需截断。
 *
 * 行边界截断可能切在 ``` fence 内部——markdown-it 会把未闭合 fence 渲染为
 * 「代码块直到截断末尾」，视觉上就是一个代码块，无结构破坏，可接受。
 * 单行超长（无换行可回退）硬切 limit。
 */
export function sliceForPreview(source: string, limit: number = PREVIEW_LIMIT): string | null {
  if (source.length <= limit) return null;
  const cut = source.lastIndexOf("\n", limit);
  if (cut <= 0) return source.slice(0, limit); // 单行超长（无换行可回退）硬切
  // 含行尾换行（完整行）；换行恰在 limit 处时排除之，保证 ≤ limit
  return cut + 1 <= limit ? source.slice(0, cut + 1) : source.slice(0, cut);
}
