import { useEffect, useMemo, useRef, useState } from "react";
import { openUrl } from "@tauri-apps/plugin-opener";
import { convertFileSrc } from "@tauri-apps/api/core";
import { renderMarkdown } from "@/lib/markdown";
import { t } from "@/lib/i18n";
import { sliceForPreview } from "./previewTruncate";
import { resolveImgSrc } from "./resolveImgSrc";

interface MarkdownPreviewProps {
  source: string;
  fontSize?: number;
  /** 相对路径图片解析基目录（md 文件父目录）；file tab 传入，其余 tab 省略（相对路径原样） */
  baseUrl?: string;
}

function useDebounced<T>(value: T, delayMs: number): T {
  const [debounced, setDebounced] = useState(value);
  useEffect(() => {
    const id = window.setTimeout(() => setDebounced(value), delayMs);
    return () => window.clearTimeout(id);
  }, [value, delayMs]);
  return debounced;
}

export function MarkdownPreview({ source, fontSize, baseUrl }: MarkdownPreviewProps) {
  const debouncedSource = useDebounced(source, 150);
  const articleRef = useRef<HTMLElement>(null);
  // 第十二轮 P1-2：click listener 委托到容器 div（两分支都渲染的稳定容器）。
  // 旧实现绑到 <article>，空内容分支不渲染 <article> → effect mount 时 articleRef=null
  // 早返回、listener 不注册 → 后续内容到达 <article> 挂载但 effect 不重跑（deps []）
  // → 链接点击无 preventDefault → WKWebView 导航到外链，编辑上下文丢失。
  // 委托到容器：click 冒泡，closest("a") 仍命中 article 内链接，空内容分支也生效。
  const containerRef = useRef<HTMLDivElement>(null);

  // 大文档截断（z_perf 2026-08-18）：>PREVIEW_LIMIT 只渲染前缀（行边界），DOM 与
  // markdown-it 成本 O(limit)——打开 MB 级转 Markdown 产物不再整篇 innerHTML 冻结。
  const { html, truncatedInfo } = useMemo(() => {
    const sliced = sliceForPreview(debouncedSource);
    return {
      html: renderMarkdown(sliced ?? debouncedSource),
      truncatedInfo: sliced ? { shown: sliced.length, total: debouncedSource.length } : null,
    };
  }, [debouncedSource]);

  // innerHTML 命令式写入（非 dangerouslySetInnerHTML——保留 mermaid/代码块 DOM 装饰）
  useEffect(() => {
    if (!articleRef.current) return;
    articleRef.current.innerHTML = html;
    // 相对路径图片渲染层解析（spec §5）：md 源不动，仅 DOM src 替换为 asset: URL。
    // 用 getAttribute（非 .src——避免浏览器先解析相对路径失败后缓存），仅在结果变化时写回。
    if (baseUrl) {
      for (const img of articleRef.current.querySelectorAll("img")) {
        const raw = img.getAttribute("src");
        if (!raw) continue;
        const resolved = resolveImgSrc(raw, baseUrl, convertFileSrc);
        if (resolved !== raw) img.setAttribute("src", resolved);
      }
    }
  }, [html, baseUrl]);

  // 第十二轮 P1-2 + 第十九轮 P2-2：click listener 委托到单容器 div。
  // 第十二轮修了「空内容 mount 时 articleRef=null listener 不注册」，但两分支条件渲染
  // 是不同 DOM 节点 → 分支切换时旧 div 卸载 listener 失效，新 div 无 listener。
  // 第十九轮：合并为单容器（始终渲染同一 div），空内容用 CSS 隐藏 article 显示提示。
  const isEmpty = source.trim().length === 0;

  // 全局事件委托：链接拦截 + 代码块复制（仅挂载一次——单容器保证 div 不变）
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

  const fmtKB = (n: number) => `${Math.round(n / 1024)}KB`;

  // 单容器：空内容显示提示（CSS 隐藏 article），非空显示 article（CSS 隐藏提示）。
  // div 始终同一个 → listener 不随内容变化失效。
  return (
    <div ref={containerRef} className="md-preview flex-1 overflow-auto p-5" style={{ userSelect: "text", fontSize: fontSize ? `${fontSize}px` : undefined }}>
      {truncatedInfo && (
        <div className="mb-3 rounded-md border border-amber-500/40 bg-amber-500/10 px-3 py-1.5 text-xs text-muted-foreground">
          {t("editor.previewTruncated", {
            shown: fmtKB(truncatedInfo.shown),
            total: fmtKB(truncatedInfo.total),
          })}
        </div>
      )}
      {isEmpty ? (
        <div className="flex w-full h-full items-center justify-center">
          <span className="text-sm text-muted-foreground">{t("editor.previewEmpty")}</span>
        </div>
      ) : (
        <article ref={articleRef} className="md-prose" />
      )}
    </div>
  );
}
