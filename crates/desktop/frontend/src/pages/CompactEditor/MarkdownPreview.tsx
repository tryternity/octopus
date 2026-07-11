import { useEffect, useRef, useState } from "react";
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

function decorateCodeBlocks(root: HTMLElement): () => void {
  const cleanups: Array<() => void> = [];
  const codeEls = root.querySelectorAll("pre > code");
  codeEls.forEach((code) => {
    const pre = code.parentElement;
    if (!pre || pre.parentElement?.classList.contains("md-codeblock")) return;

    const wrapper = document.createElement("div");
    wrapper.className = "md-codeblock";
    pre.parentNode?.insertBefore(wrapper, pre);
    wrapper.appendChild(pre);

    const btn = document.createElement("button");
    btn.type = "button";
    btn.className = "md-copy-btn";
    btn.textContent = t("editor.copyCode");
    const onClick = async () => {
      try {
        await navigator.clipboard.writeText(code.textContent ?? "");
      } catch {
        // WKWebView clipboard 可能受限，忽略
      }
      btn.textContent = t("editor.copied");
      window.setTimeout(() => { btn.textContent = t("editor.copyCode"); }, 1400);
    };
    btn.addEventListener("click", onClick);
    wrapper.appendChild(btn);
    cleanups.push(() => btn.removeEventListener("click", onClick));
  });
  return () => cleanups.forEach((fn) => fn());
}

export function MarkdownPreview({ source, fontSize }: MarkdownPreviewProps) {
  const debouncedSource = useDebounced(source, 150);
  const [html, setHtml] = useState("");
  const articleRef = useRef<HTMLElement>(null);

  useEffect(() => {
    setHtml(renderMarkdown(debouncedSource));
  }, [debouncedSource]);

  useEffect(() => {
    if (!articleRef.current) return;
    articleRef.current.innerHTML = html;
    const cleanup = decorateCodeBlocks(articleRef.current);
    return cleanup;
  }, [html]);

  useEffect(() => {
    const article = articleRef.current;
    if (!article) return;
    const onClick = (e: MouseEvent) => {
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
        // 其余协议/相对路径——阻止 webview 导航离开应用
        e.preventDefault();
      }
    };
    article.addEventListener("click", onClick);
    return () => article.removeEventListener("click", onClick);
  }, [html]);

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
