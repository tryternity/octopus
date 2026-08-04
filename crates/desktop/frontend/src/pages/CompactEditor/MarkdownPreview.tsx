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
  // 第十二轮 P1-2：click listener 委托到容器 div（两分支都渲染的稳定容器）。
  // 旧实现绑到 <article>，但空内容分支不渲染 <article> → effect mount 时 articleRef=null
  // 早返回、listener 不注册 → 后续内容到达 <article> 挂载但 effect 不重跑（deps []）
  // → 链接点击无 preventDefault → WKWebView 导航到外链，编辑上下文丢失。
  // 委托到容器：click 冒泡，closest("a") 仍命中 article 内链接，空内容分支也生效。
  const containerRef = useRef<HTMLDivElement>(null);

  // useMemo 同步派生 HTML（避免 setState 额外重绘周期）
  const html = useMemo(() => renderMarkdown(debouncedSource), [debouncedSource]);

  // innerHTML 命令式写入（非 dangerouslySetInnerHTML——保留 mermaid/代码块 DOM 装饰）
  useEffect(() => {
    if (!articleRef.current) return;
    articleRef.current.innerHTML = html;
  }, [html]);

  // 全局事件委托：链接拦截 + 代码块复制（仅挂载一次，html 变化不重绑）
  useEffect(() => {
    const container = containerRef.current;
    if (!container) return;
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
        const target = container.querySelector(`[id="${CSS.escape(id)}"]`);
        target?.scrollIntoView({ behavior: "smooth", block: "start" });
      } else if (/^https?:\/\//.test(href)) {
        e.preventDefault();
        openUrl(href).catch(() => {});
      } else {
        e.preventDefault();
      }
    };
    container.addEventListener("click", onClick);
    return () => container.removeEventListener("click", onClick);
  }, []);

  if (source.trim().length === 0) {
    return (
      <div ref={containerRef} className="md-preview flex-1 flex items-center justify-center">
        <span className="text-sm text-muted-foreground">{t("editor.previewEmpty")}</span>
      </div>
    );
  }

  return (
    <div ref={containerRef} className="md-preview flex-1 overflow-auto p-5" style={{ userSelect: "text", fontSize: fontSize ? `${fontSize}px` : undefined }}>
      <article ref={articleRef} className="md-prose" />
    </div>
  );
}
