import { useEffect, useMemo, useRef, useState } from "react";
import { openUrl } from "@tauri-apps/plugin-opener";
import { renderMarkdown } from "@/lib/markdown";
import { t } from "@/lib/i18n";

interface MarkdownPreviewProps {
  source: string;
  fontSize?: number;
}

function useDebounced<T>(value: T, delayMs: number): T {
  const [debounced, setDebounced] = useState(value);
  useEffect(() => {
    const id = window.setTimeout(() => setDebounced(value), delayMs);
    return () => window.clearTimeout(id);
  }, [value, delayMs]);
  return debounced;
}

export function MarkdownPreview({ source, fontSize }: MarkdownPreviewProps) {
  const debouncedSource = useDebounced(source, 150);
  const articleRef = useRef<HTMLElement>(null);

  // useMemo 同步派生 HTML（避免 setState 额外重绘周期）
  const html = useMemo(() => renderMarkdown(debouncedSource), [debouncedSource]);

  // innerHTML 命令式写入（非 dangerouslySetInnerHTML——保留 mermaid/代码块 DOM 装饰）
  useEffect(() => {
    if (!articleRef.current) return;
    articleRef.current.innerHTML = html;
  }, [html]);

  // 全局事件委托：链接拦截 + 代码块复制（仅挂载一次，html 变化不重绑）
  useEffect(() => {
    const article = articleRef.current;
    if (!article) return;
    const onClick = async (e: MouseEvent) => {
      // 代码块复制按钮
      const copyBtn = (e.target as HTMLElement).closest<HTMLElement>("[data-copy]");
      if (copyBtn) {
        const code = copyBtn.parentElement?.querySelector("code");
        if (code) {
          try { await navigator.clipboard.writeText(code.textContent ?? ""); } catch { /* WKWebView */ }
          copyBtn.textContent = t("editor.copied");
          window.setTimeout(() => { copyBtn.textContent = t("editor.copyCode"); }, 1400);
        }
        return;
      }
      // 链接拦截
      const a = (e.target as HTMLElement).closest("a");
      if (!a) return;
      const href = a.getAttribute("href");
      if (!href) return;
      if (href.startsWith("#")) {
        e.preventDefault();
        const id = decodeURIComponent(href.slice(1));
        const target = article.querySelector(`[id="${CSS.escape(id)}"]`);
        target?.scrollIntoView({ behavior: "smooth", block: "start" });
      } else if (/^https?:\/\//.test(href)) {
        e.preventDefault();
        openUrl(href).catch(() => {});
      } else {
        e.preventDefault();
      }
    };
    article.addEventListener("click", onClick);
    return () => article.removeEventListener("click", onClick);
  }, []);

  if (source.trim().length === 0) {
    return (
      <div className="md-preview flex-1 flex items-center justify-center">
        <span className="text-sm text-muted-foreground">{t("editor.previewEmpty")}</span>
      </div>
    );
  }

  return (
    <div className="md-preview flex-1 overflow-auto p-5" style={{ userSelect: "text", fontSize: fontSize ? `${fontSize}px` : undefined }}>
      <article ref={articleRef} className="md-prose" />
    </div>
  );
}
